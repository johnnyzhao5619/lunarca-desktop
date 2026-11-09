//! 解封后的 CMK 只存本机保险箱（TDD-12 §2 / §10.2：主密钥在系统钥匙串，条目逐条加密落盘）。
//! 本 crate 只定义接口与存储形状；实现在应用装配层（云端凭据命名空间下 `sync-cmk/<远端库 id>`
//! 一项），测试用内存实现。
//!
//! 一个库的全部 CMK 版本存成一项：轮换是懒的，旧块仍用旧版本加密，读它们要旧钥匙。

use std::collections::BTreeMap;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

use luna_sync_core::crypto::Cmk;

/// 凭据存储里条目名的前缀（与会话凭据同一个命名空间）。
pub const KEYCHAIN_ENTRY_PREFIX: &str = "sync-cmk/";

#[must_use]
pub fn keychain_entry(vault_id: Uuid) -> String {
    format!("{KEYCHAIN_ENTRY_PREFIX}{vault_id}")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyStoreError {
    /// 本机保险箱不可用（主密钥读不出 / 文件读写失败；原因码只进日志）。
    #[error("key store unavailable: {0}")]
    Unavailable(String),
    #[error("stored keys are malformed")]
    Malformed,
}

/// 一个库的 CMK 在本机的存取。
pub trait CmkStore: Send + Sync {
    /// 这个库在本机的全部 CMK 版本（没有 = 空）。
    fn load(&self, vault_id: Uuid) -> Result<Vec<Cmk>, KeyStoreError>;
    /// 存一个版本（同版本覆盖）。
    fn save(&self, vault_id: Uuid, cmk: &Cmk) -> Result<(), KeyStoreError>;
    /// 停用同步或换库时忘掉这个库的全部钥匙。
    fn forget(&self, vault_id: Uuid) -> Result<(), KeyStoreError>;
}

#[derive(Serialize, Deserialize)]
struct Stored {
    keys: Vec<StoredKey>,
}

#[derive(Serialize, Deserialize, Zeroize)]
struct StoredKey {
    version: u32,
    key: String,
}

/// 凭据存储里那一项的文本形状（实现方只管存取这串文本）。
#[must_use]
pub fn encode(cmks: &[Cmk]) -> String {
    let mut stored = Stored {
        keys: cmks
            .iter()
            .map(|c| StoredKey {
                version: c.version(),
                key: STANDARD.encode(c.expose_bytes()),
            })
            .collect(),
    };
    let text = serde_json::to_string(&stored).unwrap_or_default();
    stored.keys.iter_mut().for_each(Zeroize::zeroize);
    text
}

pub fn decode(text: &str) -> Result<Vec<Cmk>, KeyStoreError> {
    let mut stored: Stored = serde_json::from_str(text).map_err(|_| KeyStoreError::Malformed)?;
    let mut out = Vec::with_capacity(stored.keys.len());
    for entry in &mut stored.keys {
        let mut bytes = STANDARD
            .decode(&entry.key)
            .map_err(|_| KeyStoreError::Malformed)?;
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| KeyStoreError::Malformed)?;
        bytes.zeroize();
        out.push(Cmk::from_bytes(entry.version, key));
        entry.zeroize();
    }
    out.sort_by_key(Cmk::version);
    Ok(out)
}

/// 把一个版本并进已有的版本集合（同版本替换）。
#[must_use]
pub fn merged(mut existing: Vec<Cmk>, cmk: &Cmk) -> Vec<Cmk> {
    existing.retain(|c| c.version() != cmk.version());
    existing.push(cmk.clone());
    existing.sort_by_key(Cmk::version);
    existing
}

/// 测试与模拟器用：存在内存里（以文本形状存，走一遍编解码）。
#[derive(Default)]
pub struct MemoryCmkStore(Mutex<BTreeMap<Uuid, String>>);

impl CmkStore for MemoryCmkStore {
    fn load(&self, vault_id: Uuid) -> Result<Vec<Cmk>, KeyStoreError> {
        let map = self
            .0
            .lock()
            .map_err(|_| KeyStoreError::Unavailable("poisoned".into()))?;
        map.get(&vault_id)
            .map_or(Ok(Vec::new()), |text| decode(text))
    }

    fn save(&self, vault_id: Uuid, cmk: &Cmk) -> Result<(), KeyStoreError> {
        let existing = self.load(vault_id)?;
        let text = encode(&merged(existing, cmk));
        self.0
            .lock()
            .map_err(|_| KeyStoreError::Unavailable("poisoned".into()))?
            .insert(vault_id, text);
        Ok(())
    }

    fn forget(&self, vault_id: Uuid) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .map_err(|_| KeyStoreError::Unavailable("poisoned".into()))?
            .remove(&vault_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_accumulate_and_survive_the_text_round_trip() {
        let store = MemoryCmkStore::default();
        let vault = Uuid::from_u128(1);
        assert!(store.load(vault).unwrap().is_empty());
        let v1 = Cmk::generate(1);
        let v2 = Cmk::generate(2);
        store.save(vault, &v2).unwrap();
        store.save(vault, &v1).unwrap();
        let got = store.load(vault).unwrap();
        assert_eq!(got.iter().map(Cmk::version).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(got[0].expose_bytes(), v1.expose_bytes());
        // 同版本覆盖。
        let v1b = Cmk::generate(1);
        store.save(vault, &v1b).unwrap();
        assert_eq!(
            store.load(vault).unwrap()[0].expose_bytes(),
            v1b.expose_bytes()
        );
        store.forget(vault).unwrap();
        assert!(store.load(vault).unwrap().is_empty());
        assert_eq!(keychain_entry(vault), format!("sync-cmk/{vault}"));
        assert_eq!(decode("not json").unwrap_err(), KeyStoreError::Malformed);
    }
}
