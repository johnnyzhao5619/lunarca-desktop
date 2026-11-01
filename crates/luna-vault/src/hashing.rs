//! 内容寻址哈希（TDD-06 §4 自写抑制、§4.7 快照去重的共同地基）。
//!
//! 用 SHA-256 的十六进制小写串作内容指纹：① `last_written_hash` 自写回声抑制
//! 比较；② 快照内容寻址去重的 blob 键。两处共用同一口径（DRY），避免"自写判定"
//! 与"快照去重"对同一内容算出不同指纹。

use sha2::{Digest, Sha256};

/// 内容指纹（SHA-256 十六进制小写，64 字符）。
///
/// 对**字节**取哈希（非字符串），从而对换行风格、BOM 等字节差异敏感——这是自写抑制
/// 正确性的关键：只要落盘字节与上次自写完全一致才判为自写。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    /// 对任意字节计算指纹。
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self::from_digest(hasher)
    }

    /// 流式计算（与 [`Self::of`] 同一口径）：几百 MB 的录音不必整份读进内存。
    ///
    /// # Errors
    /// 读失败。
    pub fn of_reader(mut reader: impl std::io::Read) -> std::io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(Self::from_digest(hasher))
    }

    fn from_digest(hasher: Sha256) -> Self {
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        Self(hex)
    }

    /// 十六进制串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_matches_whole() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(
            ContentHash::of_reader(std::io::Cursor::new(&data)).unwrap(),
            ContentHash::of(&data)
        );
    }

    #[test]
    fn stable_and_lowercase_hex() {
        let h = ContentHash::of(b"hello");
        // 已知 SHA-256("hello")。
        assert_eq!(
            h.as_str(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn byte_sensitive_to_newline_style() {
        // 自写抑制命门：CRLF 与 LF 必须算出不同指纹，否则外部编辑器改了换行风格会被误判自写。
        assert_ne!(ContentHash::of(b"a\nb"), ContentHash::of(b"a\r\nb"));
    }
}
