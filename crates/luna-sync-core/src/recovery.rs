//! 恢复密钥（TDD-12 §10.2、PRD-13-009）：256 位随机数，交给用户离线保存。
//!
//! 显示成 14 组 × 4 个字符的 Crockford base32（`XXXX-XXXX-…`）：32 字节密钥 + 3 字节校验 = 35 字节
//! = 280 位 = 恰好 56 个字符。字母表去掉了 I / L / O / U，读的时候大小写不敏感、`I`/`L` 当 `1`、
//! `O` 当 `0`、空格与连字符忽略——用户从纸上抄回来时最常见的几种走样都能认。校验位让抄错一个
//! 字符当场被发现，而不是落到「解不开信封」那一步才让人以为密码错了。

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const KEY_LEN: usize = 32;
const CHECK_LEN: usize = 3;
const CHARS: usize = 56;
const GROUP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecoveryKeyError {
    #[error("recovery key has the wrong length")]
    Length,
    #[error("recovery key contains a character outside the alphabet")]
    Character,
    #[error("recovery key checksum does not match")]
    Checksum,
}

/// 一把恢复密钥。离开作用域即清零；`Debug` 不显示内容。
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; KEY_LEN]);

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryKey(…)")
    }
}

fn check(key: &[u8; KEY_LEN]) -> [u8; CHECK_LEN] {
    let digest = Sha256::digest(key);
    [digest[0], digest[1], digest[2]]
}

impl RecoveryKey {
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    #[must_use]
    pub fn expose_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// 给用户看的分组形式。
    #[must_use]
    pub fn display(&self) -> String {
        let mut raw = Vec::with_capacity(KEY_LEN + CHECK_LEN);
        raw.extend_from_slice(&self.0);
        raw.extend_from_slice(&check(&self.0));
        let mut chars = Vec::with_capacity(CHARS);
        let (mut acc, mut bits) = (0_u32, 0_u32);
        for byte in &raw {
            acc = (acc << 8) | u32::from(*byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                chars.push(ALPHABET[((acc >> bits) & 31) as usize]);
            }
        }
        raw.zeroize();
        let text: Vec<String> = chars
            .chunks(GROUP)
            .map(|g| String::from_utf8_lossy(g).into_owned())
            .collect();
        chars.zeroize();
        text.join("-")
    }

    /// 从用户输入还原（宽容走样，见模块头）。
    pub fn parse(input: &str) -> Result<Self, RecoveryKeyError> {
        let mut values = Vec::with_capacity(CHARS);
        for c in input.chars() {
            let v = match c.to_ascii_uppercase() {
                ' ' | '-' | '\t' | '\n' | '\r' => continue,
                'I' | 'L' => 1,
                'O' => 0,
                up => ALPHABET
                    .iter()
                    .position(|a| char::from(*a) == up)
                    .ok_or(RecoveryKeyError::Character)? as u32,
            };
            values.push(v);
        }
        if values.len() != CHARS {
            return Err(RecoveryKeyError::Length);
        }
        let mut raw = Vec::with_capacity(KEY_LEN + CHECK_LEN);
        let (mut acc, mut bits) = (0_u32, 0_u32);
        for v in &values {
            acc = (acc << 5) | v;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                raw.push(((acc >> bits) & 0xFF) as u8);
            }
        }
        values.zeroize();
        let mut key = [0_u8; KEY_LEN];
        key.copy_from_slice(&raw[..KEY_LEN]);
        let ok = raw[KEY_LEN..] == check(&key);
        raw.zeroize();
        if !ok {
            key.zeroize();
            return Err(RecoveryKeyError::Checksum);
        }
        Ok(Self(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_displays_as_fourteen_groups_and_parses_back() {
        let key = RecoveryKey::generate();
        let shown = key.display();
        let groups: Vec<&str> = shown.split('-').collect();
        assert_eq!(groups.len(), 14);
        assert!(groups.iter().all(|g| g.len() == 4));
        assert!(shown
            .chars()
            .all(|c| c == '-' || ALPHABET.contains(&(c as u8))));
        let back = RecoveryKey::parse(&shown).unwrap();
        assert_eq!(back.expose_bytes(), key.expose_bytes());
    }

    #[test]
    fn common_transcription_slips_are_forgiven() {
        let key = RecoveryKey::generate();
        let shown = key.display();
        let sloppy = shown
            .to_lowercase()
            .replace('-', " ")
            .replace('1', "l")
            .replace('0', "o");
        assert_eq!(
            RecoveryKey::parse(&sloppy).unwrap().expose_bytes(),
            key.expose_bytes()
        );
    }

    #[test]
    fn a_wrong_character_is_caught_by_the_checksum() {
        let key = RecoveryKey::generate();
        let mut chars: Vec<char> = key.display().chars().collect();
        // 改第一个字符为字母表里的另一个字符。
        chars[0] = if chars[0] == 'Z' { 'Y' } else { 'Z' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(
            RecoveryKey::parse(&typo).unwrap_err(),
            RecoveryKeyError::Checksum
        );
        assert_eq!(
            RecoveryKey::parse("ABCD").unwrap_err(),
            RecoveryKeyError::Length
        );
        assert_eq!(
            RecoveryKey::parse(&"U".repeat(56)).unwrap_err(),
            RecoveryKeyError::Character
        );
        assert_eq!(format!("{key:?}"), "RecoveryKey(…)");
    }
}
