//! 文本解码（ADR-164 ⑤）：BOM → 无 BOM 的 UTF-16 → 严格 UTF-8 → chardetng 猜测。

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

/// 只看开头这么多字节判断无 BOM 的 UTF-16（够判断，又不必扫完整个文件）。
const UTF16_PROBE_BYTES: usize = 4096;

/// 把文本文件的字节解成字符串；第二个值为真表示有字节解不出、被替换成了 �。
#[must_use]
pub fn decode_text(bytes: &[u8]) -> (String, bool) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, lossy) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (text.into_owned(), lossy);
    }
    // 无 BOM 的 UTF-16 先于 UTF-8：拉丁字符的 UTF-16 字节恰好也是合法 UTF-8（夹着零字节），
    // 而真正的 UTF-8 文本里几乎不会有零字节。
    if let Some(encoding) = utf16_without_bom(bytes) {
        let (text, lossy) = encoding.decode_without_bom_handling(bytes);
        return (text.into_owned(), lossy);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_owned(), false);
    }
    let encoding = {
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
        detector.feed(bytes, true);
        detector.guess(None, chardetng::Utf8Detection::Allow)
    };
    let (text, lossy) = encoding.decode_without_bom_handling(bytes);
    (text.into_owned(), lossy)
}

/// 按给定字符集（如 HTTP 响应头里的 `charset=`）解码；没给或认不出时走 [`decode_text`] 的猜测。
#[must_use]
pub fn decode_with_charset(charset: Option<&str>, bytes: &[u8]) -> String {
    match charset.and_then(|label| Encoding::for_label(label.trim().trim_matches('"').as_bytes())) {
        Some(encoding) => encoding.decode(bytes).0.into_owned(),
        None => decode_text(bytes).0,
    }
}

/// 无 BOM 的 UTF-16：拉丁与常用标点在 UTF-16 里每两个字节就有一个零字节，落在偶数位是大端、
/// 奇数位是小端。chardetng 刻意不管 UTF-16，这一层只能自己判断。
fn utf16_without_bom(bytes: &[u8]) -> Option<&'static Encoding> {
    let probe = &bytes[..bytes.len().min(UTF16_PROBE_BYTES)];
    let pairs = probe.len() / 2;
    if pairs < 2 {
        return None;
    }
    let even = probe.iter().step_by(2).filter(|b| **b == 0).count();
    let odd = probe.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    // 三成以上的字符带零字节、另一侧几乎没有：真正的 8 位编码文本里不会出现这种分布。
    let threshold = pairs * 3 / 10;
    if odd > threshold && even * 10 < odd {
        Some(UTF_16LE)
    } else if even > threshold && odd * 10 < even {
        Some(UTF_16BE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_with_and_without_bom() {
        assert_eq!(
            decode_text("中文 abc".as_bytes()),
            ("中文 abc".to_owned(), false)
        );
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("访谈".as_bytes());
        assert_eq!(decode_text(&bom), ("访谈".to_owned(), false));
    }

    #[test]
    fn utf16_with_bom_and_without() {
        // encoding_rs 不往 UTF-16 编码，字节手工拼。
        let text = "Hello, 世界 — interview notes";
        let le: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut le_bom = vec![0xFF, 0xFE];
        le_bom.extend_from_slice(&le);
        assert_eq!(decode_text(&le_bom).0, text);
        assert_eq!(decode_text(&le).0, text);
        assert_eq!(decode_text(&be).0, text);
    }

    #[test]
    fn gb18030_and_big5_are_guessed() {
        let simplified = "今天我们聊一聊怎么把视频脚本写得更顺口，开头先抛一个问题。";
        let (gbk, _, _) = encoding_rs::GB18030.encode(simplified);
        assert_eq!(decode_text(&gbk), (simplified.to_owned(), false));
        let traditional = "今天我們聊一聊怎麼把影片腳本寫得更順口，開頭先拋一個問題。";
        let (big5, _, _) = encoding_rs::BIG5.encode(traditional);
        assert_eq!(decode_text(&big5), (traditional.to_owned(), false));
    }
}
