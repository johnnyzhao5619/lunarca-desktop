//! 分块与补齐（TDD-14 §5、TDD-12 §10.2）。
//!
//! - `.md` 与 ≤ 1 MiB 的文件整篇一块；更大的文件用 FastCDC（256 KiB / 1 MiB / 4 MiB）按内容切，
//!   改动只影响附近的块，尾部追加只多出尾部的块。
//! - 补齐模糊长度：块前加 4 字节真实长度，后面补零到分档大小——整篇块 4 KiB 起、按 2 的幂分档；
//!   大块按 32 KiB 取整（再按 2 的幂会让 4 MiB 的块翻倍成 8 MiB，越过服务端单块上限）。补零是
//!   确定的，同一明文补齐后仍是同一份字节，块 id 与密文因此稳定。
//!
//! 补齐只模糊长度、不消除它（隐私页如实写明）。

use std::io::Read;
use std::ops::Range;

/// 整篇一块的上限。
pub const WHOLE_FILE_MAX: usize = 1 << 20;
/// FastCDC 的最小 / 平均 / 最大块。
pub const CDC_MIN: u32 = 256 * 1024;
pub const CDC_AVG: u32 = 1 << 20;
pub const CDC_MAX: u32 = 4 << 20;
const MIN_BUCKET: usize = 4096;
const LARGE_STEP: usize = 32 * 1024;
const LEN_PREFIX: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("padding malformed")]
pub struct PaddingError;

/// 一份文件内容的切块边界（空文件是一个空块）。
#[must_use]
pub fn boundaries(data: &[u8]) -> Vec<Range<usize>> {
    if data.len() <= WHOLE_FILE_MAX {
        return std::iter::once(0..data.len()).collect();
    }
    fastcdc::v2020::FastCDC::new(data, CDC_MIN, CDC_AVG, CDC_MAX)
        .map(|c| c.offset..c.offset + c.length)
        .collect()
}

/// 流式切块：与 [`boundaries`] 同一规则，但从 `reader` 逐块读出，大文件不必整份进内存。
///
/// `len` 是调用方事先 stat 到的大小，只用来选规则（≤ [`WHOLE_FILE_MAX`] 整篇一块）。读到的内容
/// 与 `len` 不符（读的途中被改了）时切法可能与整份切块不同——调用方读完核对内容哈希，不符即丢弃
/// 这一次的结果，所以不会有一份「半新半旧」的块列表被提交。
pub fn stream_chunks<R: Read + Send + 'static>(
    mut reader: R,
    len: u64,
) -> Box<dyn Iterator<Item = std::io::Result<Vec<u8>>> + Send> {
    if len <= WHOLE_FILE_MAX as u64 {
        let mut whole = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        return Box::new(std::iter::once(
            reader.read_to_end(&mut whole).map(|_| whole),
        ));
    }
    Box::new(
        fastcdc::v2020::StreamCDC::new(reader, CDC_MIN, CDC_AVG, CDC_MAX).map(|chunk| {
            chunk
                .map(|c| c.data)
                .map_err(|e| std::io::Error::other(e.to_string()))
        }),
    )
}

/// 补齐之后的长度。
#[must_use]
pub fn padded_len(len: usize) -> usize {
    let total = len + LEN_PREFIX;
    if total <= WHOLE_FILE_MAX {
        total.next_power_of_two().max(MIN_BUCKET)
    } else {
        total.div_ceil(LARGE_STEP) * LARGE_STEP
    }
}

/// 补齐一块：`[真实长度 u32 大端][数据][零]`。
#[must_use]
pub fn pad(chunk: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(padded_len(chunk.len()));
    out.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk);
    out.resize(padded_len(chunk.len()), 0);
    out
}

/// 去掉补齐。长度越界或补的不是零（被动过）都拒绝。
pub fn unpad(padded: &[u8]) -> Result<&[u8], PaddingError> {
    if padded.len() < LEN_PREFIX {
        return Err(PaddingError);
    }
    let len = u32::from_be_bytes([padded[0], padded[1], padded[2], padded[3]]) as usize;
    let end = LEN_PREFIX.checked_add(len).ok_or(PaddingError)?;
    if end > padded.len()
        || padded.len() != padded_len(len)
        || padded[end..].iter().any(|b| *b != 0)
    {
        return Err(PaddingError);
    }
    Ok(&padded[LEN_PREFIX..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 服务端单块密文上限（sp-domain `MAX_CHUNK_CT_BYTES`）与密文开销（头 17 + 标签 16）。
    const SERVER_MAX_CHUNK: usize = 4 * 1024 * 1024 + 64 * 1024;
    const CT_OVERHEAD: usize = 17 + 16;

    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    /// 服务端 `file_ciphertext_ceiling`（sp-domain）的公式：账户原文上限对应的密文上限。
    fn server_ciphertext_ceiling(cap: usize) -> usize {
        cap * 2 + (cap / CDC_MIN as usize + 1) * (2 * LEN_PREFIX + CT_OVERHEAD) + MIN_BUCKET
    }

    /// 预检放行的文件必须提交得进去：不管内容切成什么样，密文都在服务端的上限之内。
    #[test]
    fn any_chunking_within_the_cap_fits_the_server_ciphertext_ceiling() {
        let cap = 50 * 1024 * 1024usize;
        let just_over_a_bucket = |bucket: usize| bucket - LEN_PREFIX + 1;
        for chunk_len in [
            CDC_MIN as usize,
            just_over_a_bucket(256 * 1024),
            just_over_a_bucket(512 * 1024),
            WHOLE_FILE_MAX,
            CDC_MAX as usize,
        ] {
            let tail = cap % chunk_len;
            let mut ciphertext = (cap / chunk_len) * (padded_len(chunk_len) + CT_OVERHEAD);
            if tail > 0 {
                ciphertext += padded_len(tail) + CT_OVERHEAD;
            }
            assert!(
                ciphertext <= server_ciphertext_ceiling(cap),
                "{chunk_len}: {ciphertext}"
            );
        }
    }

    #[test]
    fn small_files_are_one_chunk_padded_to_power_of_two_buckets() {
        assert_eq!(boundaries(b"").as_slice(), std::slice::from_ref(&(0..0)));
        assert_eq!(
            boundaries(&[1; 1000]).as_slice(),
            std::slice::from_ref(&(0..1000))
        );
        assert_eq!(padded_len(0), 4096);
        assert_eq!(padded_len(4092), 4096);
        assert_eq!(padded_len(4093), 8192);
        assert_eq!(padded_len(WHOLE_FILE_MAX - 4), WHOLE_FILE_MAX);
        for len in [0, 1, 100, 5000, 70_000] {
            let data = noise(len, len as u64);
            let padded = pad(&data);
            assert_eq!(padded.len(), padded_len(len));
            assert_eq!(unpad(&padded).unwrap(), data.as_slice());
        }
    }

    #[test]
    fn large_files_split_by_content_and_every_chunk_fits_the_server() {
        let data = noise(9 << 20, 7);
        let parts = boundaries(&data);
        assert!(parts.len() > 2);
        assert_eq!(parts.first().unwrap().start, 0);
        assert_eq!(parts.last().unwrap().end, data.len());
        assert!(parts.windows(2).all(|w| w[0].end == w[1].start));
        for range in &parts {
            assert!(range.len() <= CDC_MAX as usize);
            assert!(padded_len(range.len()) + CT_OVERHEAD <= SERVER_MAX_CHUNK);
        }
        // 最大的块也放得下。
        assert!(padded_len(CDC_MAX as usize) + CT_OVERHEAD <= SERVER_MAX_CHUNK);
    }

    #[test]
    fn appending_to_a_large_file_keeps_the_earlier_chunks() {
        let base = noise(8 << 20, 3);
        let mut grown = base.clone();
        grown.extend_from_slice(&noise(512 * 1024, 4));
        let before: Vec<&[u8]> = boundaries(&base).into_iter().map(|r| &base[r]).collect();
        let after: Vec<&[u8]> = boundaries(&grown).into_iter().map(|r| &grown[r]).collect();
        let kept = before.iter().filter(|c| after.contains(c)).count();
        assert!(
            kept >= before.len() - 1,
            "只有尾部那一块会变：{kept}/{}",
            before.len()
        );
    }

    /// 流式切块与整份切块必须切在同一处：同一份录音在两条路径上算出两套块 id，去重与
    /// 「只改尾部只传尾部」就都落空了。
    #[test]
    fn streaming_chunks_match_whole_file_chunks() {
        for (len, seed) in [
            (100usize, 1u64),
            (WHOLE_FILE_MAX, 2),
            (WHOLE_FILE_MAX + 1, 3),
            (9 << 20, 4),
            (13 << 20, 5),
        ] {
            let data = noise(len, seed);
            let whole: Vec<Vec<u8>> = boundaries(&data)
                .into_iter()
                .map(|r| data[r].to_vec())
                .collect();
            let streamed: Vec<Vec<u8>> =
                stream_chunks(std::io::Cursor::new(data.clone()), len as u64)
                    .collect::<std::io::Result<_>>()
                    .unwrap();
            assert_eq!(whole, streamed, "{len} 字节切法不同");
        }
    }

    #[test]
    fn tampered_padding_is_refused() {
        let mut padded = pad(b"hello");
        let last = padded.len() - 1;
        padded[last] = 1;
        assert_eq!(unpad(&padded), Err(PaddingError));
        // 声称的长度超出这一档：拒绝。
        let mut liar = pad(b"hello");
        liar[2] = 0x40;
        assert_eq!(unpad(&liar), Err(PaddingError));
        assert_eq!(unpad(&[0, 0]), Err(PaddingError));
    }
}
