//! 密钥存储：本机生成的密钥（如本地度量的 HMAC 钥匙）只进本机保险箱（[`crate::vault`]：主密钥
//! 在系统钥匙串、条目逐条加密落盘），**绝不入配置文件**（TDD-12 §2）。抽象为 `SecretStore`
//! trait——生产用 [`VaultSecretStore`]，测试用 `MemorySecretStore`（不触碰真实钥匙串，避免授权
//! 弹窗 / CI 失败）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::vault::{SecretVault, VaultError};

/// 密钥存储错误。
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// 后端（钥匙串 / 保险箱）错误。
    #[error("secret storage error: {0}")]
    Backend(String),
}

/// 密钥读写抽象。
pub trait SecretStore: Send + Sync {
    /// 读取（不存在返回 `None`）。
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    /// 写入 / 覆盖。
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    /// 删除（不存在视为成功）。
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// 保险箱后端：一个命名空间下的逻辑条目（`<namespace>/<key>`）存进 [`SecretVault`]。
///
/// 读取把「条目认证失败」当作「没有」（理由见 [`SecretVault::get_or_lost`]）；主密钥读不出、
/// 文件读写失败如实报 [`SecretError::Backend`]。
pub struct VaultSecretStore {
    vault: Arc<SecretVault>,
    namespace: String,
}

impl VaultSecretStore {
    /// 以保险箱与命名空间（如 `knowledge-metrics.v1`）构造。
    #[must_use]
    pub fn new(vault: Arc<SecretVault>, namespace: impl Into<String>) -> Self {
        Self {
            vault,
            namespace: namespace.into(),
        }
    }

    fn name(&self, key: &str) -> String {
        format!("{}/{key}", self.namespace)
    }
}

fn backend(err: VaultError) -> SecretError {
    SecretError::Backend(err.to_string())
}

impl SecretStore for VaultSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.vault.get_or_lost(&self.name(key)).map_err(backend)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.vault.set(&self.name(key), value).map_err(backend)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.vault.delete(&self.name(key)).map_err(backend)
    }
}

/// 内存后端（仅测试 / 临时）。
#[derive(Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    /// 新建空存储。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let store = MemorySecretStore::new();
        assert_eq!(store.get("k").unwrap(), None);
        store.set("k", "secret").unwrap();
        assert_eq!(store.get("k").unwrap(), Some("secret".to_string()));
        store.delete("k").unwrap();
        assert_eq!(store.get("k").unwrap(), None);
        // 删不存在的也成功。
        store.delete("k").unwrap();
    }

    /// 生产后端照同一份契约：读写删 + 命名空间隔离 + 篡改按「没有」处理。
    #[test]
    fn vault_store_honours_the_secret_store_contract() {
        use crate::vault::{KeyringBackend, MemoryKeyring};
        let dir = tempfile::tempdir().unwrap();
        let keyring: Arc<dyn KeyringBackend> = Arc::new(MemoryKeyring::new());
        let vault = Arc::new(SecretVault::new(dir.path(), keyring));
        let metrics = VaultSecretStore::new(Arc::clone(&vault), "knowledge-metrics.v1");
        let other = VaultSecretStore::new(Arc::clone(&vault), "other");
        assert_eq!(metrics.get("k").unwrap(), None);
        metrics.set("k", "secret").unwrap();
        assert_eq!(metrics.get("k").unwrap().as_deref(), Some("secret"));
        assert_eq!(other.get("k").unwrap(), None, "命名空间互不可见");
        assert!(vault.contains("knowledge-metrics.v1/k").unwrap());
        metrics.delete("k").unwrap();
        assert_eq!(metrics.get("k").unwrap(), None);
        metrics.delete("k").unwrap();

        metrics.set("k", "secret").unwrap();
        std::fs::write(
            vault.path(),
            std::fs::read_to_string(vault.path()).unwrap().replace(
                "\"knowledge-metrics.v1/k\":\"",
                "\"knowledge-metrics.v1/k\":\"AAAA",
            ),
        )
        .unwrap();
        assert_eq!(
            metrics.get("k").unwrap(),
            None,
            "篡改 = 丢失，不是「暂时不可用」"
        );
    }
}
