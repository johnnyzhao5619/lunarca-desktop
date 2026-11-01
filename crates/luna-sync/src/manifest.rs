//! 清单条目（每个文件一条，加密后作为 `entry_ct` 上传；TDD-14 §5）与文件 ⇄ 块的往返。
//!
//! 条目明文只在客户端存在：路径、大小、提示性 mtime、内容哈希、块 id 列表。服务端只见它的
//! 密文、`file_id`、rev 与块 id。路径一律 NFC（ADR-148：传输与比较用 NFC）。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::chunk::{boundaries, pad, padded_len, unpad};
use crate::crypto::{CryptoError, KeyRing, VaultKeys};

/// 条目明文的格式版本。
pub const ENTRY_FORMAT: u8 = 1;

/// 一个文件的清单条目（明文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPlain {
    /// 格式版本。
    pub v: u8,
    /// 库相对路径（`/` 分隔、NFC）。
    pub path: String,
    /// 明文字节数。
    pub size: u64,
    /// 提示性修改时间（Unix 毫秒）：只用于展示，不参与任何判定（时钟不可信）。
    pub mtime_ms: Option<i64>,
    /// 明文的 sha256（小写十六进制）：三态比对与组装后的核对都用它。
    pub content_hash: String,
    /// 依次拼起来就是文件内容的块（可重复）。
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("chunk padding malformed")]
    Padding,
    #[error("entry malformed")]
    Malformed,
    /// 块都解开了，拼出来的内容与条目里的哈希 / 大小不符。
    #[error("assembled content does not match the entry")]
    ContentMismatch,
    /// 取块失败（原因码由调用方给，只进日志）。
    #[error("fetching chunk failed: {0}")]
    Fetch(String),
}

/// 一个要上传的块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedChunk {
    pub id: String,
    pub sealed: Vec<u8>,
    /// 密文的 sha256（上传时给服务端校验）。
    pub ct_sha256: [u8; 32],
}

/// 一个文件准备上传的全部东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFile {
    pub entry: EntryPlain,
    /// 补齐后的总大小（服务端可见的那个数）。
    pub size_padded: i64,
    /// 去重后的块（同一文件里重复的块只传一次）。
    pub chunks: Vec<SealedChunk>,
}

/// 内容的 sha256（小写十六进制）。与本地库的内容指纹是同一份实现（`luna_vault::hashing`）：
/// 同步核对「本地这份是不是基线那份」时，两边算出来的必须逐字相同。
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    luna_vault::hashing::ContentHash::of(bytes)
        .as_str()
        .to_owned()
}

/// 切块、补齐、加密，并写出条目。
#[must_use]
pub fn prepare(keys: &VaultKeys, path: &str, bytes: &[u8], mtime_ms: Option<i64>) -> PreparedFile {
    let mut ids = Vec::new();
    let mut chunks = Vec::new();
    let mut seen = HashSet::new();
    let mut size_padded: i64 = 0;
    for range in boundaries(bytes) {
        let padded = pad(&bytes[range.clone()]);
        size_padded += padded_len(range.len()) as i64;
        let id = keys.chunk_id(&padded);
        if seen.insert(id.clone()) {
            let sealed = keys.seal_chunk(&id, &padded);
            let ct_sha256 = Sha256::digest(&sealed).into();
            chunks.push(SealedChunk {
                id: id.clone(),
                sealed,
                ct_sha256,
            });
        }
        ids.push(id);
    }
    PreparedFile {
        entry: EntryPlain {
            v: ENTRY_FORMAT,
            path: luna_vault::pathutil::nfc(path).into_owned(),
            size: bytes.len() as u64,
            mtime_ms,
            content_hash: content_hash(bytes),
            chunks: ids,
        },
        size_padded,
        chunks,
    }
}

/// 大文件的「第一遍」：流式切块，只算块 id、补齐后的大小与内容哈希，不加密、不留块
/// （[`PreparedFile::chunks`] 为空）。要上传的块由调用方问过服务端缺哪些之后再流式读第二遍。
///
/// # Errors
/// 读失败。
pub fn prepare_streamed(
    keys: &VaultKeys,
    path: &str,
    reader: impl std::io::Read + Send + 'static,
    len: u64,
    mtime_ms: Option<i64>,
) -> std::io::Result<PreparedFile> {
    let mut ids = Vec::new();
    let mut size_padded: i64 = 0;
    let mut size: u64 = 0;
    let mut hasher = Sha256::new();
    for chunk in crate::chunk::stream_chunks(reader, len) {
        let chunk = chunk?;
        hasher.update(&chunk);
        size += chunk.len() as u64;
        size_padded += padded_len(chunk.len()) as i64;
        ids.push(keys.chunk_id(&pad(&chunk)));
    }
    Ok(PreparedFile {
        entry: EntryPlain {
            v: ENTRY_FORMAT,
            path: luna_vault::pathutil::nfc(path).into_owned(),
            size,
            mtime_ms,
            content_hash: hex(&hasher.finalize()),
            chunks: ids,
        },
        size_padded,
        chunks: Vec::new(),
    })
}

fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// 加密条目（绑定 file_id 与这次提交将得到的 rev）。
#[must_use]
pub fn seal_entry(keys: &VaultKeys, file_id: Uuid, rev: i64, entry: &EntryPlain) -> Vec<u8> {
    let json = serde_json::to_vec(entry).expect("entry serializes");
    keys.seal_entry(file_id, rev, &json)
}

/// 解开条目并核对形状。
pub fn open_entry(
    ring: &KeyRing,
    file_id: Uuid,
    rev: i64,
    entry_ct: &[u8],
) -> Result<EntryPlain, ManifestError> {
    let plain = ring
        .for_ciphertext(entry_ct)?
        .open_entry(file_id, rev, entry_ct)?;
    let entry: EntryPlain = serde_json::from_slice(&plain).map_err(|_| ManifestError::Malformed)?;
    let hex64 =
        |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if entry.v != ENTRY_FORMAT
        || entry.path.is_empty()
        || !hex64(&entry.content_hash)
        || entry.chunks.is_empty()
        || !entry.chunks.iter().all(|c| hex64(c))
    {
        return Err(ManifestError::Malformed);
    }
    Ok(entry)
}

/// 按条目把块拼回文件内容。`fetch` 给出某块的密文；拼完核对大小与哈希。
pub fn assemble(
    ring: &KeyRing,
    entry: &EntryPlain,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, ManifestError> {
    let mut out = Vec::with_capacity(usize::try_from(entry.size).unwrap_or(0));
    for id in &entry.chunks {
        let sealed = fetch(id).map_err(ManifestError::Fetch)?;
        let padded = ring.for_ciphertext(&sealed)?.open_chunk(id, &sealed)?;
        out.extend_from_slice(unpad(&padded).map_err(|_| ManifestError::Padding)?);
    }
    if out.len() as u64 != entry.size || content_hash(&out) != entry.content_hash {
        return Err(ManifestError::ContentMismatch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::crypto::Cmk;

    fn ring() -> KeyRing {
        KeyRing::new(Uuid::from_u128(1), &[Cmk::generate(1)]).unwrap()
    }

    fn store(prepared: &PreparedFile) -> HashMap<String, Vec<u8>> {
        prepared
            .chunks
            .iter()
            .map(|c| (c.id.clone(), c.sealed.clone()))
            .collect()
    }

    #[test]
    fn a_note_round_trips_through_entry_and_chunks() {
        let ring = ring();
        let keys = ring.current();
        // NFD 的名字进清单时被规范成 NFC。
        let nfd = "Cafe\u{301}.md";
        let prepared = prepare(keys, nfd, b"# hello\n", Some(1_700_000_000_000));
        assert_eq!(prepared.entry.path, "Caf\u{e9}.md");
        assert_eq!(prepared.chunks.len(), 1);
        assert_eq!(prepared.size_padded, 4096);
        let file = Uuid::from_u128(9);
        let ct = seal_entry(keys, file, 1, &prepared.entry);
        let entry = open_entry(&ring, file, 1, &ct).unwrap();
        assert_eq!(entry, prepared.entry);
        let chunks = store(&prepared);
        let bytes = assemble(&ring, &entry, |id| Ok(chunks[id].clone())).unwrap();
        assert_eq!(bytes, b"# hello\n");
        // 同一内容再准备一次：块 id 与密文都不变（上传幂等）。
        let again = prepare(keys, nfd, b"# hello\n", None);
        assert_eq!(again.chunks, prepared.chunks);
    }

    /// 流式第一遍算出的条目必须与整份准备的逐字相同：同一份录音走哪条路都是同一套块。
    #[test]
    fn a_streamed_entry_matches_the_whole_file_entry() {
        let ring = ring();
        let keys = ring.current();
        let mut x: u64 = 7;
        let bytes: Vec<u8> = (0..(5 << 20) + 123)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let whole = prepare(keys, "rec.wav", &bytes, Some(5));
        let streamed = prepare_streamed(
            keys,
            "rec.wav",
            std::io::Cursor::new(bytes.clone()),
            bytes.len() as u64,
            Some(5),
        )
        .unwrap();
        assert_eq!(streamed.entry, whole.entry);
        assert_eq!(streamed.size_padded, whole.size_padded);
        assert!(streamed.chunks.is_empty(), "第一遍不加密、不留块");
    }

    #[test]
    fn a_large_file_repeats_chunks_but_uploads_each_once() {
        let ring = ring();
        let keys = ring.current();
        // 两段完全相同的大块：清单里列两次，上传只一份。
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let block: Vec<u8> = (0..(6 << 20))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let mut bytes = block.clone();
        bytes.extend_from_slice(&block);
        let prepared = prepare(keys, "big.bin", &bytes, None);
        assert!(prepared.entry.chunks.len() > prepared.chunks.len());
        let chunks = store(&prepared);
        let back = assemble(&ring, &prepared.entry, |id| Ok(chunks[id].clone())).unwrap();
        assert_eq!(back, bytes);
        for chunk in &prepared.chunks {
            assert_eq!(
                chunk.ct_sha256,
                <[u8; 32]>::from(Sha256::digest(&chunk.sealed))
            );
        }
    }

    #[test]
    fn a_swapped_or_missing_chunk_is_caught() {
        let ring = ring();
        let keys = ring.current();
        let a = prepare(keys, "a.md", b"aaaa", None);
        let b = prepare(keys, "b.md", b"bbbb", None);
        // 服务端把 b 的块当成 a 的块给回来：认证失败（块绑块 id）。
        let swapped = assemble(&ring, &a.entry, |_| Ok(b.chunks[0].sealed.clone()));
        assert_eq!(
            swapped,
            Err(ManifestError::Crypto(CryptoError::Authentication))
        );
        let missing = assemble(&ring, &a.entry, |_| Err("not_found".into()));
        assert_eq!(missing, Err(ManifestError::Fetch("not_found".into())));
        // 条目被挪给别的文件或别的 rev：认证失败。
        let ct = seal_entry(keys, Uuid::from_u128(1), 2, &a.entry);
        assert!(open_entry(&ring, Uuid::from_u128(1), 3, &ct).is_err());
        assert!(open_entry(&ring, Uuid::from_u128(2), 2, &ct).is_err());
    }

    #[test]
    fn an_entry_whose_hash_lies_is_refused_after_assembly() {
        let ring = ring();
        let keys = ring.current();
        let prepared = prepare(keys, "a.md", b"real", None);
        let mut liar = prepared.entry.clone();
        liar.content_hash = content_hash(b"fake");
        let chunks = store(&prepared);
        assert_eq!(
            assemble(&ring, &liar, |id| Ok(chunks[id].clone())),
            Err(ManifestError::ContentMismatch)
        );
    }
}
