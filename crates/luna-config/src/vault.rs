//! 本机密钥保险箱（TDD-12 §2）：系统钥匙串里**只有一条**主密钥，其余密钥逐条加密后放在应用
//! 配置目录下的一个文件里。
//!
//! # 为什么不再一条秘密一个钥匙串条目
//!
//! macOS 的文件式钥匙串按「条目 × 代码签名」记授权：条目越多，首次运行、重装或换签名时连续弹出
//! 的系统密码框就越多（此前一次启动五六个：每台服务器的 access / refresh / profile、每个同步库的
//! 库主密钥、本地度量钥匙各问一次）。Chromium / Electron 系应用的做法是钥匙串里只放一把
//! 「<应用> Safe Storage」随机密钥、其余秘密用它加密落盘——于是至多问一次，「始终允许」之后
//! 不再问。本模块照此实现；威胁模型与之相同：磁盘上只有密文，钥匙在钥匙串里。
//!
//! # 文件形状
//!
//! `secrets.vault`（JSON，权限 0600）：
//!
//! ```text
//! { "version": 1,
//!   "key_check": base64(nonce ‖ AES-256-GCM(主密钥, 常量, aad = 「key-check」)),
//!   "entries": { "<逻辑条目名>": base64(nonce ‖ AES-256-GCM(主密钥, 值, aad = 「entry/<名>」)) } }
//! ```
//!
//! - **逐条加密**：读一条只解这一条——读度量钥匙不会把云端凭据解出来，坏一条只丢这一条；
//!   条目名进附加认证数据，把密文挪到别的名字下即认证失败。条目名本身是明文（与钥匙串里条目名
//!   可见同一口径），于是「有没有这一条」不需要主密钥：没有的条目一次钥匙串都不碰。
//! - **`key_check`**：认出「这份文件是不是用这把主密钥写的」。钥匙串里的主密钥丢了、被换了，
//!   整份文件当作全部丢失（要求重新登录、同步密钥从托管信封重取、度量开新周期），下一次写入
//!   从一份新文件开始，而不是把认不出的旧条目一直背着。
//!
//! # 读写纪律
//!
//! - 主密钥进程内只读一次并缓存（`Zeroizing`）；**解出来的值不缓存**，每次读都重读文件——
//!   另一个窗口 / 进程写过之后这里自然读到新值，不需要任何失效。
//! - 写入：同目录临时文件 → fsync → 改名，进程间以 `secrets.vault.lock` 的文件锁串行
//!   （读改写整段在锁里，两个进程各写一条不会互相吞掉）。崩在中途只会留下一个读方从不看的
//!   临时文件。
//! - 主密钥读失败（用户拒绝授权、钥匙串锁着）后静默 60 s 再试，与此前各条目的口径相同；
//!   写入永不静默（那是用户主动的时刻，也是唯一的自愈路径）。
//! - 只在「钥匙串里确实没有」（`NoEntry`）时才生成新主密钥；读失败绝不生成——那会把一把
//!   读不出来的好钥匙覆盖掉。

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aes_gcm::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::secret::SecretError;

/// 钥匙串里那一条的 service 名（Chromium / Electron 系的「<应用> Safe Storage」同一命名）。
pub const MASTER_KEY_SERVICE: &str = "Lunarca Safe Storage";
/// 钥匙串里那一条的 account 名。
pub const MASTER_KEY_ACCOUNT: &str = "Lunarca";
/// 保险箱文件名（应用配置目录下）。
pub const VAULT_FILE: &str = "secrets.vault";
/// 进程间写锁文件名（与保险箱同目录）。
pub const VAULT_LOCK_FILE: &str = "secrets.vault.lock";

const FORMAT_VERSION: u32 = 1;
const NONCE_LEN: usize = 12;
const KEY_CHECK_PLAINTEXT: &[u8] = b"lunarca-secret-vault";
const AAD_PREFIX: &str = "lunarca-secret-vault/v1/";
const BACKOFF_KEY: &str = "master-key";
/// 主密钥读失败后的静默期（理由见模块顶注「读写纪律」）。
const FAILURE_QUIET_PERIOD: Duration = Duration::from_secs(60);

/// 调试构建可用 `LUNARCA_DEV_KEYRING_SERVICE` 换主密钥的 service 名：钥匙串跟登录用户走、不跟
/// `HOME`，临时 HOME 起的第二个实例想与本机实例彻底分开时用它。发行构建里这一支在编译期就
/// 不存在，服务名恒为 [`MASTER_KEY_SERVICE`]。
fn master_key_service() -> String {
    #[cfg(debug_assertions)]
    if let Some(service) = std::env::var("LUNARCA_DEV_KEYRING_SERVICE")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return service;
    }
    MASTER_KEY_SERVICE.to_owned()
}

/// 保险箱的失败。都不含任何密钥、条目值或条目名。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    /// 钥匙串读不出主密钥、或文件读写失败（原因只描述后端与 IO 类别）。
    #[error("secret vault unavailable: {0}")]
    Unavailable(String),
    /// 条目（或整份文件）认证失败：被改过、截断过，或格式认不出。
    #[error("secret vault entry failed authentication")]
    Tampered,
}

/// 系统钥匙串的最小接口：保险箱只用它存取主密钥，旧条目清理只用它的 [`Self::delete_unread`]。
///
/// 抽成 trait 是为了测试**绝不碰真实钥匙串**（授权弹窗 / CI 无交互面），并能记下每一次调用
/// ——「旧条目只删不读」这条要靠调用记录来证明。
pub trait KeyringBackend: Send + Sync {
    /// 读一条；没有 = `Ok(None)`。在 macOS 上这一步可能弹授权框。
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError>;
    /// 写一条（覆盖）。
    fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError>;
    /// 删一条，**不先读它的内容**；没有也算成功。
    fn delete_unread(&self, service: &str, account: &str) -> Result<(), SecretError>;
}

/// 系统钥匙串（macOS Security.framework / Windows 凭据管理器）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemKeyring;

impl KeyringBackend for SystemKeyring {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
        let entry = keyring::Entry::new(service, account)
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }

    fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError> {
        keyring::Entry::new(service, account)
            .and_then(|entry| entry.set_password(value))
            .map_err(|e| SecretError::Backend(e.to_string()))
    }

    /// macOS 上**不能**走 `keyring` 的 `delete_credential`：它先 `find_generic_password`
    /// 取回条目内容再删，而读内容正是触发授权框的那一步（旧条目的 ACL 记的是旧签名）。
    /// `SecItemDelete` 只按 service + account 匹配删除，不取内容。Windows 的 `CredDeleteW`
    /// 本来就不读，照用 `keyring`。
    fn delete_unread(&self, service: &str, account: &str) -> Result<(), SecretError> {
        #[cfg(target_os = "macos")]
        {
            /// `errSecItemNotFound`。
            const ITEM_NOT_FOUND: i32 = -25300;
            match security_framework::passwords::delete_generic_password(service, account) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
                Err(e) => Err(SecretError::Backend(format!(
                    "keychain status {}",
                    e.code()
                ))),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let entry = keyring::Entry::new(service, account)
                .map_err(|e| SecretError::Backend(e.to_string()))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(SecretError::Backend(e.to_string())),
            }
        }
    }
}

/// 测试替身：一次钥匙串调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringCall {
    Read { service: String, account: String },
    Write { service: String, account: String },
    DeleteUnread { service: String, account: String },
}

/// 内存钥匙串（测试用）：记下每一次调用，可预置条目、可让读取失败。
#[derive(Default)]
pub struct MemoryKeyring {
    items: Mutex<HashMap<(String, String), String>>,
    calls: Mutex<Vec<KeyringCall>>,
    deny_reads: std::sync::atomic::AtomicBool,
}

impl MemoryKeyring {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 预置一条（不计入调用记录）。
    pub fn insert(&self, service: &str, account: &str, value: &str) {
        self.items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((service.to_owned(), account.to_owned()), value.to_owned());
    }

    /// 移除一条（不计入调用记录），模拟用户在钥匙串里删掉了它。
    pub fn remove(&self, service: &str, account: &str) {
        self.items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(service.to_owned(), account.to_owned()));
    }

    /// 此刻有没有这一条（不计入调用记录）。
    #[must_use]
    pub fn contains(&self, service: &str, account: &str) -> bool {
        self.items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&(service.to_owned(), account.to_owned()))
    }

    /// 之后的读取一律失败（模拟用户拒绝授权 / 钥匙串锁着）。
    pub fn deny_reads(&self, deny: bool) {
        self.deny_reads
            .store(deny, std::sync::atomic::Ordering::SeqCst);
    }

    /// 迄今的调用记录。
    #[must_use]
    pub fn calls(&self) -> Vec<KeyringCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, call: KeyringCall) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }
}

impl KeyringBackend for MemoryKeyring {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, SecretError> {
        self.record(KeyringCall::Read {
            service: service.to_owned(),
            account: account.to_owned(),
        });
        if self.deny_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SecretError::Backend("denied".into()));
        }
        Ok(self
            .items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn write(&self, service: &str, account: &str, value: &str) -> Result<(), SecretError> {
        self.record(KeyringCall::Write {
            service: service.to_owned(),
            account: account.to_owned(),
        });
        self.insert(service, account, value);
        Ok(())
    }

    fn delete_unread(&self, service: &str, account: &str) -> Result<(), SecretError> {
        self.record(KeyringCall::DeleteUnread {
            service: service.to_owned(),
            account: account.to_owned(),
        });
        self.remove(service, account);
        Ok(())
    }
}

type MasterKey = Zeroizing<[u8; 32]>;

#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    key_check: String,
    entries: BTreeMap<String, String>,
}

/// 读到的文件：没有 / 认得出 / 认不出（坏了、截断了、别的格式版本）。
enum Loaded {
    Absent,
    Readable(Document),
    Unreadable,
}

/// 本机密钥保险箱。一个进程一份（装配层持有），可同时有别的进程开着同一个文件。
pub struct SecretVault {
    path: PathBuf,
    lock_path: PathBuf,
    keyring: Arc<dyn KeyringBackend>,
    service: String,
    master: Mutex<Option<MasterKey>>,
    /// 主密钥读失败的静默期（[`luna_core::Backoff`]，与网络熔断同一条策略）。
    backoff: luna_core::Backoff,
}

impl SecretVault {
    /// 以 `config_dir/secrets.vault` 为文件、系统钥匙串存主密钥。
    #[must_use]
    pub fn system(config_dir: &Path) -> Self {
        Self::new(config_dir, Arc::new(SystemKeyring))
    }

    /// 以 `config_dir/secrets.vault` 为文件、给定的钥匙串后端存主密钥（测试注入内存钥匙串）。
    #[must_use]
    pub fn new(config_dir: &Path, keyring: Arc<dyn KeyringBackend>) -> Self {
        Self {
            path: config_dir.join(VAULT_FILE),
            lock_path: config_dir.join(VAULT_LOCK_FILE),
            keyring,
            service: master_key_service(),
            master: Mutex::new(None),
            backoff: luna_core::Backoff::new(1, FAILURE_QUIET_PERIOD),
        }
    }

    /// 保险箱文件的位置。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 有没有这一条（**不碰钥匙串**：条目名是明文）。文件认不出时当作没有。
    ///
    /// # Errors
    /// 文件读不了（权限 / IO）时 [`VaultError::Unavailable`]。
    pub fn contains(&self, name: &str) -> Result<bool, VaultError> {
        Ok(match self.load_shared()? {
            Loaded::Readable(doc) => doc.entries.contains_key(name),
            Loaded::Absent | Loaded::Unreadable => false,
        })
    }

    /// 读一条。没有这一条、没有文件、或主密钥已不是写这份文件的那一把，都是 `Ok(None)`；
    /// 只有在文件里确有这一条时才去取主密钥。
    ///
    /// # Errors
    /// 主密钥读不出（拒绝授权 / 锁着）或文件 IO 失败 → [`VaultError::Unavailable`]；这一条
    /// （或整份文件）认证失败 → [`VaultError::Tampered`]。
    pub fn get(&self, name: &str) -> Result<Option<String>, VaultError> {
        let doc = match self.load_shared()? {
            Loaded::Absent => return Ok(None),
            Loaded::Unreadable => return Err(VaultError::Tampered),
            Loaded::Readable(doc) => doc,
        };
        let Some(sealed) = doc.entries.get(name) else {
            return Ok(None);
        };
        let Some(key) = self.key_for_read(&doc)? else {
            return Ok(None);
        };
        let plain = open(&key, &entry_aad(name), sealed)?;
        String::from_utf8(plain.to_vec())
            .map(Some)
            .map_err(|_| VaultError::Tampered)
    }

    /// [`Self::get`]，但认证失败按「这一条丢了」处理（记一行不含内容的日志）。
    ///
    /// 各类秘密的恢复路径都挂在「没有」上——没有会话就要求登录、没有库主密钥就从托管信封重取、
    /// 没有度量钥匙就提示清除旧统计后开新周期。把篡改报成「读不出来」会落进「钥匙串暂时不可用、
    /// 稍后重试」那条永远不会自愈的路，故统一归到「没有」。下一次写入这一条即覆盖掉坏的那份。
    ///
    /// # Errors
    /// 只剩 [`VaultError::Unavailable`]。
    pub fn get_or_lost(&self, name: &str) -> Result<Option<String>, VaultError> {
        match self.get(name) {
            Err(VaultError::Tampered) => {
                eprintln!("[secrets] a vault entry failed authentication; treating it as absent");
                Ok(None)
            }
            other => other,
        }
    }

    /// 写一条（覆盖）。必要时生成主密钥并存进钥匙串（只在钥匙串里确实没有时）。
    ///
    /// # Errors
    /// 主密钥取不到 / 存不进，或文件写失败 → [`VaultError::Unavailable`]。
    pub fn set(&self, name: &str, value: &str) -> Result<(), VaultError> {
        let _lock = self.lock_for_write()?;
        let loaded = self.load()?;
        let key = self.key_for_write(&loaded)?;
        let mut doc = match loaded {
            Loaded::Readable(doc) if key_matches(&key, &doc) => doc,
            Loaded::Readable(_) => {
                eprintln!("[secrets] vault was written under another master key; starting over");
                fresh_document(&key)?
            }
            Loaded::Unreadable => {
                eprintln!("[secrets] vault file is unreadable; starting over");
                fresh_document(&key)?
            }
            Loaded::Absent => fresh_document(&key)?,
        };
        doc.entries.insert(
            name.to_owned(),
            seal(&key, &entry_aad(name), value.as_bytes())?,
        );
        self.persist(&doc)
    }

    /// 删一条（没有也算成功）。**不碰钥匙串**：别的条目的密文原样留着。
    ///
    /// # Errors
    /// 文件写失败 → [`VaultError::Unavailable`]。
    pub fn delete(&self, name: &str) -> Result<(), VaultError> {
        let _lock = self.lock_for_write()?;
        let Loaded::Readable(mut doc) = self.load()? else {
            return Ok(());
        };
        if doc.entries.remove(name).is_none() {
            return Ok(());
        }
        self.persist(&doc)
    }

    /// 读侧：在锁文件上拿**共享锁**读一次文件，读完即放（取主密钥可能弹授权框，不能让写入方
    /// 一直等着）。Windows 上目标文件被打开时「改名覆盖」会失败，读与写不串行就会让写入
    /// 随机失败（Windows 真机 2026-10-16：登录刚完成时清理标记写不进）。
    fn load_shared(&self) -> Result<Loaded, VaultError> {
        let _lock = self.lock_for_read()?;
        self.load()
    }

    /// 只在已持有锁时调用（写侧持排他锁、读侧经 [`Self::load_shared`] 持共享锁）。
    fn load(&self) -> Result<Loaded, VaultError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded::Absent),
            Err(e) => return Err(io_unavailable("read", &e)),
        };
        Ok(match serde_json::from_slice::<Document>(&bytes) {
            Ok(doc) if doc.version == FORMAT_VERSION => Loaded::Readable(doc),
            _ => Loaded::Unreadable,
        })
    }

    fn cached(&self) -> Option<MasterKey> {
        self.master
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn remember(&self, key: &MasterKey) {
        *self
            .master
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key.clone());
    }

    /// 读钥匙串里的主密钥：`Ok(None)` = 没有或不是一把合法的钥匙。
    fn read_master(&self) -> Result<Option<MasterKey>, VaultError> {
        match self.keyring.read(&self.service, MASTER_KEY_ACCOUNT) {
            Ok(text) => {
                self.backoff.record_success(BACKOFF_KEY);
                let key = text.and_then(|text| decode_master(&text));
                if let Some(key) = &key {
                    self.remember(key);
                }
                Ok(key)
            }
            Err(e) => {
                self.backoff.record_failure(BACKOFF_KEY);
                Err(VaultError::Unavailable(e.to_string()))
            }
        }
    }

    /// 读一条时要的主密钥：`None` = 这份文件已无法解开（主密钥丢了或换了）。
    fn key_for_read(&self, doc: &Document) -> Result<Option<MasterKey>, VaultError> {
        if let Some(key) = self.cached() {
            if key_matches(&key, doc) {
                return Ok(Some(key));
            }
        }
        if self.backoff.should_skip(BACKOFF_KEY) {
            return Err(VaultError::Unavailable(
                "master key temporarily unreadable (retrying later)".into(),
            ));
        }
        match self.read_master()? {
            Some(key) if key_matches(&key, doc) => Ok(Some(key)),
            Some(_) | None => {
                eprintln!("[secrets] vault cannot be opened with the current master key; treating its entries as absent");
                Ok(None)
            }
        }
    }

    /// 写入时要的主密钥：缓存 → 钥匙串 → 只在钥匙串里确实没有时生成并存入。**不受静默期约束**。
    fn key_for_write(&self, loaded: &Loaded) -> Result<MasterKey, VaultError> {
        if let Some(key) = self.cached() {
            let usable = match loaded {
                Loaded::Readable(doc) => key_matches(&key, doc),
                Loaded::Absent | Loaded::Unreadable => true,
            };
            if usable {
                return Ok(key);
            }
        }
        if let Some(key) = self.read_master()? {
            return Ok(key);
        }
        let mut bytes = Zeroizing::new([0_u8; 32]);
        OsRng.fill_bytes(bytes.as_mut());
        let encoded = Zeroizing::new(STANDARD.encode(bytes.as_ref()));
        self.keyring
            .write(&self.service, MASTER_KEY_ACCOUNT, &encoded)
            .map_err(|e| VaultError::Unavailable(e.to_string()))?;
        self.remember(&bytes);
        Ok(bytes)
    }

    fn lock_for_write(&self) -> Result<std::fs::File, VaultError> {
        let file = self.open_lock()?;
        file.lock().map_err(|e| io_unavailable("lock", &e))?;
        Ok(file)
    }

    fn lock_for_read(&self) -> Result<std::fs::File, VaultError> {
        let file = self.open_lock()?;
        file.lock_shared()
            .map_err(|e| io_unavailable("lock shared", &e))?;
        Ok(file)
    }

    fn open_lock(&self) -> Result<std::fs::File, VaultError> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_unavailable("create dir", &e))?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options
            .open(&self.lock_path)
            .map_err(|e| io_unavailable("open lock", &e))
    }

    fn persist(&self, doc: &Document) -> Result<(), VaultError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| VaultError::Unavailable("vault path has no parent".into()))?;
        std::fs::create_dir_all(dir).map_err(|e| io_unavailable("create dir", &e))?;
        let json =
            serde_json::to_vec(doc).map_err(|_| VaultError::Unavailable("serialize".into()))?;
        // 临时文件在 Unix 上以 0600 创建；改名保留它的权限位。
        let mut temp = tempfile::Builder::new()
            .prefix(".secrets.vault.")
            .suffix(".tmp")
            .tempfile_in(dir)
            .map_err(|e| io_unavailable("create temp", &e))?;
        temp.write_all(&json)
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|e| io_unavailable("write temp", &e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            temp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_unavailable("chmod", &e))?;
        }
        persist_with_retry(temp, &self.path)?;
        #[cfg(unix)]
        if let Ok(handle) = std::fs::File::open(dir) {
            // 目录项落盘：断电后改名不丢。失败不影响这次写入已经可见。
            let _ = handle.sync_all();
        }
        Ok(())
    }
}

/// 改名覆盖到位。Windows 上目标或临时文件被别的进程短暂打开（杀毒扫描新文件、索引服务）时
/// 改名报拒绝访问，过一会儿就好——同类原子写入库在 Windows 上都按此退避重试。别的平台与别的
/// 错误不重试。
fn persist_with_retry(
    mut temp: tempfile::NamedTempFile,
    path: &std::path::Path,
) -> Result<(), VaultError> {
    const BACKOFF_MS: [u64; 7] = [10, 20, 50, 100, 200, 400, 400];
    let mut attempt = 0;
    loop {
        match temp.persist(path) {
            Ok(_) => return Ok(()),
            Err(err)
                if cfg!(windows)
                    && err.error.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt < BACKOFF_MS.len() =>
            {
                std::thread::sleep(std::time::Duration::from_millis(BACKOFF_MS[attempt]));
                attempt += 1;
                temp = err.file;
            }
            Err(err) => return Err(io_unavailable("rename", &err.error)),
        }
    }
}

fn io_unavailable(step: &str, err: &std::io::Error) -> VaultError {
    VaultError::Unavailable(format!("{step}: {:?}", err.kind()))
}

fn decode_master(text: &str) -> Option<MasterKey> {
    let bytes = Zeroizing::new(STANDARD.decode(text.trim()).ok()?);
    let array: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(Zeroizing::new(array))
}

fn entry_aad(name: &str) -> String {
    format!("{AAD_PREFIX}entry/{name}")
}

fn key_check_aad() -> String {
    format!("{AAD_PREFIX}key-check")
}

fn fresh_document(key: &MasterKey) -> Result<Document, VaultError> {
    Ok(Document {
        version: FORMAT_VERSION,
        key_check: seal(key, &key_check_aad(), KEY_CHECK_PLAINTEXT)?,
        entries: BTreeMap::new(),
    })
}

fn key_matches(key: &MasterKey, doc: &Document) -> bool {
    open(key, &key_check_aad(), &doc.key_check).is_ok_and(|plain| *plain == KEY_CHECK_PLAINTEXT)
}

fn seal(key: &MasterKey, aad: &str, plaintext: &[u8]) -> Result<String, VaultError> {
    let cipher = Aes256Gcm::new(key.as_ref().into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| VaultError::Unavailable("encrypt".into()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(out))
}

fn open(key: &MasterKey, aad: &str, sealed: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let bytes = STANDARD.decode(sealed).map_err(|_| VaultError::Tampered)?;
    if bytes.len() < NONCE_LEN {
        return Err(VaultError::Tampered);
    }
    let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(key.as_ref().into());
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| VaultError::Tampered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(dir: &Path, keyring: &Arc<MemoryKeyring>) -> SecretVault {
        SecretVault::new(dir, Arc::clone(keyring) as Arc<dyn KeyringBackend>)
    }

    fn reads(keyring: &MemoryKeyring) -> usize {
        keyring
            .calls()
            .iter()
            .filter(|c| matches!(c, KeyringCall::Read { .. }))
            .count()
    }

    fn rewrite_entry(path: &Path, name: &str, edit: impl FnOnce(&mut Vec<u8>)) {
        let mut doc: Document = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut raw = STANDARD.decode(&doc.entries[name]).unwrap();
        edit(&mut raw);
        doc.entries.insert(name.to_owned(), STANDARD.encode(raw));
        std::fs::write(path, serde_json::to_vec(&doc).unwrap()).unwrap();
    }

    #[test]
    fn round_trip_and_the_file_holds_only_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        assert_eq!(v.get("cloud/a/refresh-token").unwrap(), None);
        v.set("cloud/a/refresh-token", "r-secret-value").unwrap();
        v.set("cloud/a/profile", "{\"email\":\"x@y\"}").unwrap();
        assert_eq!(
            v.get("cloud/a/refresh-token").unwrap().as_deref(),
            Some("r-secret-value")
        );
        let raw = std::fs::read_to_string(v.path()).unwrap();
        assert!(!raw.contains("r-secret-value"), "磁盘上只该有密文");
        assert!(!raw.contains("x@y"));
        // 钥匙串里只有一条：主密钥。
        assert!(keyring.contains(&master_key_service(), MASTER_KEY_ACCOUNT));
        assert!(keyring.calls().iter().all(|c| match c {
            KeyringCall::Read { service, account } | KeyringCall::Write { service, account } =>
                service == &master_key_service() && account == MASTER_KEY_ACCOUNT,
            KeyringCall::DeleteUnread { .. } => false,
        }));
    }

    #[cfg(unix)]
    #[test]
    fn the_vault_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("k", "v").unwrap();
        let mode = std::fs::metadata(v.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn logical_entries_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("cloud/a/access-token", "a").unwrap();
        v.set("cloud/a/refresh-token", "r").unwrap();
        v.set("knowledge-metrics.v1/device_metrics_key", "m")
            .unwrap();
        v.delete("cloud/a/refresh-token").unwrap();
        assert_eq!(v.get("cloud/a/refresh-token").unwrap(), None);
        assert_eq!(v.get("cloud/a/access-token").unwrap().as_deref(), Some("a"));
        assert_eq!(
            v.get("knowledge-metrics.v1/device_metrics_key")
                .unwrap()
                .as_deref(),
            Some("m")
        );
        // 删不存在的也成功。
        v.delete("cloud/a/refresh-token").unwrap();
        v.delete("never-there").unwrap();
    }

    /// 读一条只解这一条：坏掉的云端凭据不影响度量钥匙（反过来也一样）。
    #[test]
    fn reading_one_entry_never_decrypts_another() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("cloud/a/refresh-token", "r").unwrap();
        v.set("knowledge-metrics.v1/device_metrics_key", "m")
            .unwrap();
        rewrite_entry(v.path(), "cloud/a/refresh-token", |raw| {
            let last = raw.len() - 1;
            raw[last] ^= 1;
        });
        assert_eq!(
            v.get("knowledge-metrics.v1/device_metrics_key")
                .unwrap()
                .as_deref(),
            Some("m")
        );
        assert_eq!(v.get("cloud/a/refresh-token"), Err(VaultError::Tampered));
    }

    #[test]
    fn tampering_is_detected_and_treated_as_loss() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("x", "secret").unwrap();
        v.set("y", "other").unwrap();
        // 翻一位密文。
        rewrite_entry(v.path(), "x", |raw| raw[NONCE_LEN] ^= 0x80);
        assert_eq!(v.get("x"), Err(VaultError::Tampered));
        assert_eq!(v.get_or_lost("x").unwrap(), None);
        // 把 y 的密文挪到 x 名下：条目名在附加认证数据里，同样认证失败。
        let mut doc: Document = serde_json::from_slice(&std::fs::read(v.path()).unwrap()).unwrap();
        let y = doc.entries["y"].clone();
        doc.entries.insert("x".into(), y);
        std::fs::write(v.path(), serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(v.get("x"), Err(VaultError::Tampered));
        // 再写一次即覆盖掉坏的那份。
        v.set("x", "fresh").unwrap();
        assert_eq!(v.get("x").unwrap().as_deref(), Some("fresh"));
        assert_eq!(v.get("y").unwrap().as_deref(), Some("other"));
    }

    #[test]
    fn a_garbled_file_is_reported_and_the_next_write_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("x", "secret").unwrap();
        std::fs::write(v.path(), b"{\"version\":1,\"key_ch").unwrap();
        assert_eq!(v.get("x"), Err(VaultError::Tampered));
        assert!(!v.contains("x").unwrap());
        v.set("y", "new").unwrap();
        assert_eq!(v.get("y").unwrap().as_deref(), Some("new"));
        assert_eq!(v.get("x").unwrap(), None);
    }

    /// 写到一半崩掉：只会留下一个读方从不看的临时文件，原文件照常可读。
    #[test]
    fn a_partial_temp_file_left_by_a_crash_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let v = vault(dir.path(), &keyring);
        v.set("x", "secret").unwrap();
        std::fs::write(
            dir.path().join(".secrets.vault.crash.tmp"),
            b"{\"version\":1,",
        )
        .unwrap();
        assert_eq!(v.get("x").unwrap().as_deref(), Some("secret"));
        v.set("y", "z").unwrap();
        assert_eq!(v.get("x").unwrap().as_deref(), Some("secret"));
    }

    /// 主密钥只读一次；没有的条目一次钥匙串都不碰；没有文件也不碰。
    #[test]
    fn the_master_key_is_read_once_and_only_when_an_entry_exists() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let first = vault(dir.path(), &keyring);
        assert_eq!(first.get("absent").unwrap(), None);
        assert!(keyring.calls().is_empty(), "没有文件时不该碰钥匙串");
        first.set("x", "1").unwrap();

        // 新进程：缓存是空的。
        let second = vault(dir.path(), &keyring);
        let before = reads(&keyring);
        assert_eq!(second.get("absent").unwrap(), None);
        assert_eq!(reads(&keyring), before, "没有的条目不该读主密钥");
        for _ in 0..5 {
            assert_eq!(second.get("x").unwrap().as_deref(), Some("1"));
        }
        assert_eq!(reads(&keyring), before + 1, "主密钥一个进程只读一次");
    }

    /// 主密钥缺失（钥匙串被清、换机迁移了文件）= 全部秘密丢失；下一次写入换新钥、从新文件开始。
    #[test]
    fn a_missing_master_key_behaves_as_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        vault(dir.path(), &keyring).set("x", "1").unwrap();
        keyring.remove(&master_key_service(), MASTER_KEY_ACCOUNT);

        let v = vault(dir.path(), &keyring);
        assert_eq!(v.get("x").unwrap(), None);
        v.set("y", "2").unwrap();
        assert_eq!(v.get("y").unwrap().as_deref(), Some("2"));
        assert_eq!(v.get("x").unwrap(), None, "旧钥匙写的条目不随新文件留下");
        assert!(!v.contains("x").unwrap());
    }

    /// 钥匙串读失败（拒绝授权）≠ 没有：不生成新钥匙（那会覆盖一把读不出的好钥匙），
    /// 读取静默一段时间，写入照常再试。
    #[test]
    fn a_denied_master_key_is_unavailable_and_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        vault(dir.path(), &keyring).set("x", "1").unwrap();
        keyring.deny_reads(true);

        let v = vault(dir.path(), &keyring);
        assert!(matches!(v.get("x"), Err(VaultError::Unavailable(_))));
        let before = reads(&keyring);
        assert!(matches!(v.get("x"), Err(VaultError::Unavailable(_))));
        assert_eq!(reads(&keyring), before, "读失败之后应静默，不再弹框");
        assert!(matches!(v.set("y", "2"), Err(VaultError::Unavailable(_))));
        assert_eq!(reads(&keyring), before + 1, "写入不受静默期约束");
        let writes = keyring
            .calls()
            .iter()
            .filter(|c| matches!(c, KeyringCall::Write { .. }))
            .count();
        assert_eq!(
            writes, 1,
            "只有第一次建钥时写过钥匙串，读失败绝不生成新钥匙"
        );

        // 用户授权之后，写入即自愈（写入不静默），之后的读取用上缓存的钥匙。
        keyring.deny_reads(false);
        v.set("y", "2").unwrap();
        assert_eq!(v.get("x").unwrap().as_deref(), Some("1"));
        assert_eq!(v.get("y").unwrap().as_deref(), Some("2"));
    }

    /// 两个句柄（两个窗口 / 进程）交错写：文件锁串行读改写，谁也不吞掉谁的条目。
    #[test]
    fn concurrent_handles_never_lose_each_others_writes() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        vault(dir.path(), &keyring).set("seed", "0").unwrap();
        let a = Arc::new(vault(dir.path(), &keyring));
        let b = Arc::new(vault(dir.path(), &keyring));
        let threads: Vec<_> = [("a", Arc::clone(&a)), ("b", Arc::clone(&b))]
            .into_iter()
            .map(|(tag, handle)| {
                std::thread::spawn(move || {
                    for i in 0..25 {
                        handle.set(&format!("{tag}/{i}"), &i.to_string()).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for tag in ["a", "b"] {
            for i in 0..25 {
                assert_eq!(
                    a.get(&format!("{tag}/{i}")).unwrap(),
                    Some(i.to_string()),
                    "{tag}/{i}"
                );
            }
        }
        // 另一个句柄写过之后，这一个直接读到新值（不缓存值）。
        b.set("seed", "changed").unwrap();
        assert_eq!(a.get("seed").unwrap().as_deref(), Some("changed"));
    }

    /// 读与写交错：写入方每次都改名覆盖，读者不停地读。读侧持共享锁，两边都不失败
    /// （Windows 上读者开着文件时改名覆盖会报拒绝访问——真机 2026-10-16 清理标记写不进）。
    #[test]
    fn readers_and_writers_interleave_without_failures() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        vault(dir.path(), &keyring).set("token", "0").unwrap();
        let writer = Arc::new(vault(dir.path(), &keyring));
        let mut threads = vec![std::thread::spawn({
            let writer = Arc::clone(&writer);
            move || {
                for i in 0..60 {
                    writer.set("token", &i.to_string()).unwrap();
                    writer.set(&format!("marker/{i}"), "1").unwrap();
                }
            }
        })];
        for _ in 0..3 {
            let reader = Arc::new(vault(dir.path(), &keyring));
            threads.push(std::thread::spawn(move || {
                for _ in 0..120 {
                    assert!(reader.get("token").unwrap().is_some());
                    reader.contains("marker/0").unwrap();
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(writer.get("token").unwrap().as_deref(), Some("59"));
        assert_eq!(writer.get("marker/59").unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn delete_never_touches_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        vault(dir.path(), &keyring).set("x", "1").unwrap();
        let v = vault(dir.path(), &keyring);
        let before = keyring.calls().len();
        v.delete("x").unwrap();
        assert_eq!(keyring.calls().len(), before);
        assert_eq!(v.get("x").unwrap(), None);
    }
}
