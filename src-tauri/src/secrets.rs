//! 本机保险箱的装配（TDD-12 §2）：一个进程一份 [`SecretVault`]，云端凭据、同步库主密钥缓存与
//! 本地度量钥匙都是它里面的逻辑条目；系统钥匙串里只有那一条主密钥。
//!
//! 逻辑条目名（与此前的钥匙串条目一一对应，独立清除、最小读取的语义不变）：
//! - `cloud/<服务端桶>/access-token`、`…/refresh-token`、`…/profile`（`luna_cloud::store`），
//!   `cloud/<服务端桶>/write-probe`（领取一次性凭据前的试写，写完即删）；
//! - `cloud/sync-cmk/<远端库 id>`（`luna_sync::keystore`，经 `cloud_sync::CredentialCmkStore`）；
//! - `knowledge-metrics.v1/device_metrics_key`（`metrics`）；
//! - `meta/legacy-keychain-cleared`（旧钥匙串条目已清理的标记，不是秘密）。
//!
//! 旧条目硬切、不迁移（用户裁决 2026-10-16）：升级后重新登录一次，同步库主密钥从账户托管信封
//! 重取，度量钥匙换新。旧条目在**用户于新版本登录之后**静默删除一次（[`spawn_legacy_cleanup`]），
//! 只删不读——读旧条目正是那一串授权框的来源。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use luna_cloud::error::CloudError;
use luna_cloud::store::CredentialStore;
use luna_config::{KeyringBackend, SecretVault, VaultError};
use uuid::Uuid;

use crate::cloud_sync::CredentialCmkStore;

/// 云端凭据（会话三条 + 同步库主密钥缓存）的命名空间。
pub(crate) const CLOUD_NAMESPACE: &str = "cloud";
/// 本地度量钥匙的命名空间（沿用此前钥匙串 service 的后缀，带版本）。
pub(crate) const METRICS_NAMESPACE: &str = "knowledge-metrics.v1";
/// 旧钥匙串条目已清理的标记。
const LEGACY_CLEANUP_MARKER: &str = "meta/legacy-keychain-cleared";

/// 旧版本的钥匙串布局（只为删除而保留的名字，绝不读取）。
const LEGACY_CLOUD_SERVICE: &str = "lunarca-cloud";
const LEGACY_SESSION_ENTRIES: [&str; 4] =
    ["access-token", "refresh-token", "profile", "write-probe"];
const LEGACY_CMK_PREFIX: &str = "sync-cmk/";
const LEGACY_METRICS_SERVICE_SUFFIX: &str = "knowledge-metrics.v1";
const LEGACY_METRICS_ACCOUNT: &str = "device_metrics_key";

static VAULT: OnceLock<Arc<SecretVault>> = OnceLock::new();

/// 以应用配置目录装上本进程的保险箱（幂等：先到者定位置，之后返回同一份）。
///
/// 云插件的 setup 与应用自己的 setup 都会调它——前者先跑，二者给的是同一个目录。
pub(crate) fn attach(config_dir: &Path) -> Arc<SecretVault> {
    Arc::clone(VAULT.get_or_init(|| Arc::new(SecretVault::system(config_dir))))
}

/// 已装上的保险箱（装配之前为 `None`）。
pub(crate) fn installed() -> Option<Arc<SecretVault>> {
    VAULT.get().cloned()
}

/// 云插件的凭据出处（`luna_cloud::init` 的 `credentials`）。
pub(crate) fn credential_factory() -> luna_cloud::CredentialStoreFactory {
    Box::new(|config_dir| Box::new(VaultCredentialStore::new(Some(attach(config_dir)))))
}

/// 同步库主密钥的本机缓存（与会话凭据同一个保险箱、同一个命名空间）。
pub(crate) fn cmk_store() -> CredentialCmkStore {
    CredentialCmkStore::new(Arc::new(VaultCredentialStore::new(installed())))
}

/// `luna_cloud` 的凭据存储接到保险箱的 `cloud/` 命名空间上。
pub(crate) struct VaultCredentialStore {
    /// `None` = 保险箱还没装上（只会出现在装配完成之前）：一律如实报不可用。
    vault: Option<Arc<SecretVault>>,
}

impl VaultCredentialStore {
    pub(crate) fn new(vault: Option<Arc<SecretVault>>) -> Self {
        Self { vault }
    }

    fn vault(&self) -> Result<&SecretVault, CloudError> {
        self.vault.as_deref().ok_or_else(|| CloudError::Keychain {
            detail: "secret vault not attached".to_owned(),
        })
    }
}

fn cloud_name(key: &str) -> String {
    format!("{CLOUD_NAMESPACE}/{key}")
}

fn unavailable(err: VaultError) -> CloudError {
    CloudError::Keychain {
        detail: err.to_string(),
    }
}

impl CredentialStore for VaultCredentialStore {
    fn get(&self, key: &str) -> Result<Option<String>, CloudError> {
        self.vault()?
            .get_or_lost(&cloud_name(key))
            .map_err(unavailable)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), CloudError> {
        self.vault()?
            .set(&cloud_name(key), value)
            .map_err(unavailable)
    }

    fn delete(&self, key: &str) -> Result<(), CloudError> {
        self.vault()?.delete(&cloud_name(key)).map_err(unavailable)
    }
}

/// 旧版本留在钥匙串里的条目名（`(service, account)`）。只用于删除。
///
/// 分桶与远端库 id 只能列出「这台设备知道的」：记住的服务器、内置官方后端、各同步绑定里的
/// 那一台与那一库。钥匙串没有不读内容的枚举接口，列不到的就留着——尽力而为。
fn legacy_items(
    app_identifier: &str,
    buckets: &[String],
    remote_vault_ids: &[Uuid],
) -> Vec<(String, String)> {
    let mut items = Vec::new();
    // 分桶之前的全局条目（`access-token` 等不带桶名）也一并删掉。
    for entry in LEGACY_SESSION_ENTRIES {
        items.push((LEGACY_CLOUD_SERVICE.to_owned(), entry.to_owned()));
    }
    for bucket in buckets {
        for entry in LEGACY_SESSION_ENTRIES {
            items.push((LEGACY_CLOUD_SERVICE.to_owned(), format!("{bucket}/{entry}")));
        }
    }
    for id in remote_vault_ids {
        items.push((
            LEGACY_CLOUD_SERVICE.to_owned(),
            format!("{LEGACY_CMK_PREFIX}{id}"),
        ));
    }
    items.push((
        format!("{app_identifier}.{LEGACY_METRICS_SERVICE_SUFFIX}"),
        LEGACY_METRICS_ACCOUNT.to_owned(),
    ));
    items.sort();
    items.dedup();
    items
}

/// 一次旧条目清理的结局（测试据此断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyCleanup {
    /// 已清理过（保险箱里有标记）。
    AlreadyDone,
    /// 还没在新版本登录过：旧条目留着，什么都不碰。
    NotSignedIn,
    /// 删了一遍（`failed` = 删不掉的条数；失败不重试、不阻断任何东西）。
    Done { failed: usize },
}

/// 清理的核心：登录过才删、只删不读、做过一次就记下来。
///
/// `signed_in` 只在标记不在时才问；它读的是**保险箱里**的会话（新版本登录才写得进去），所以
/// 「已登录」本身就意味着用户已在新版本确认过登录——不需要另判「是不是刚登录」。
fn clean_legacy(
    vault: &SecretVault,
    keyring: &dyn KeyringBackend,
    signed_in: impl FnOnce() -> bool,
    items: impl FnOnce() -> Vec<(String, String)>,
) -> LegacyCleanup {
    if vault.contains(LEGACY_CLEANUP_MARKER).unwrap_or(false) {
        return LegacyCleanup::AlreadyDone;
    }
    if !signed_in() {
        return LegacyCleanup::NotSignedIn;
    }
    let failed = items()
        .iter()
        .filter(|(service, account)| keyring.delete_unread(service, account).is_err())
        .count();
    if failed > 0 {
        eprintln!("[secrets] {failed} legacy keychain item(s) could not be removed; leaving them");
    }
    // 不论删没删干净都只做这一次：删除若也要授权，重复尝试就是又一串弹窗。
    if let Err(err) = vault.set(LEGACY_CLEANUP_MARKER, "1") {
        // 只带步骤与 io 错误种类（`VaultError` 的显示不含任何值）。
        eprintln!("[secrets] could not record legacy keychain cleanup: {err}");
    }
    LegacyCleanup::Done { failed }
}

/// 进程内只排一次（标记写进保险箱之前，账户变更可能连着来好几次）。
static LEGACY_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);

/// 用户在新版本登录之后，静默删掉旧版本留在钥匙串里的条目（尽力而为，失败不阻断）。
///
/// 启动装配与每次账户变化都会调它；没登录、或已清理过就什么都不做。钥匙串删除可能阻塞，放在
/// 阻塞线程池上跑。
pub(crate) fn spawn_legacy_cleanup(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let Some(vault) = installed() else {
        return;
    };
    if vault.contains(LEGACY_CLEANUP_MARKER).unwrap_or(false) {
        return;
    }
    if LEGACY_CLEANUP_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let client = app
            .try_state::<luna_cloud::commands::CloudState>()
            .map(|state| Arc::clone(&state.0));
        let outcome = clean_legacy(
            &vault,
            &luna_config::SystemKeyring,
            || client.as_ref().is_some_and(|c| c.logged_in()),
            || {
                let config_dir = app.path().app_config_dir().ok();
                let (mut buckets, remote_ids) = config_dir
                    .as_deref()
                    .map(known_sync_bindings)
                    .unwrap_or_default();
                buckets.push(luna_cloud::config::server_bucket(
                    luna_cloud::config::DEFAULT_API_BASE,
                ));
                if let Some(client) = &client {
                    buckets.push(client.server_bucket());
                }
                legacy_items(
                    luna_config::identity::LEGACY_IDENTIFIER,
                    &buckets,
                    &remote_ids,
                )
            },
        );
        if outcome == LegacyCleanup::NotSignedIn {
            // 还没登录：留给下一次账户变化再试。
            LEGACY_CLEANUP_STARTED.store(false, Ordering::SeqCst);
        }
    });
}

/// 这台设备上各同步绑定记着的服务端桶与远端库 id（`<配置目录>/sync/*/state.sqlite`，只读）。
fn known_sync_bindings(config_dir: &Path) -> (Vec<String>, Vec<Uuid>) {
    let mut buckets = Vec::new();
    let mut ids = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir.join("sync")) else {
        return (buckets, ids);
    };
    for entry in entries.flatten() {
        let path = entry.path().join("state.sqlite");
        if !path.is_file() {
            continue;
        }
        if let Ok(Some(binding)) = luna_sync::state::peek_binding(&path) {
            buckets.push(binding.owner.server_bucket);
            ids.push(binding.remote_vault_id);
        }
    }
    (buckets, ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_cloud::store::{KEY_ACCESS, KEY_PROFILE, KEY_REFRESH};
    use luna_config::{KeyringCall, MemoryKeyring, MASTER_KEY_SERVICE};
    use luna_sync::keystore::CmkStore as _;

    fn vault(dir: &Path, keyring: &Arc<MemoryKeyring>) -> Arc<SecretVault> {
        Arc::new(SecretVault::new(
            dir,
            Arc::clone(keyring) as Arc<dyn KeyringBackend>,
        ))
    }

    /// `CredentialStore` 契约照旧：读写删、逻辑条目互不牵连、没登录过 = 没有。
    #[test]
    fn credential_store_contract_holds_on_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let store = VaultCredentialStore::new(Some(vault(dir.path(), &keyring)));
        let key = |k: &str| format!("api.example/{k}");
        assert_eq!(store.get(&key(KEY_REFRESH)).unwrap(), None);
        store.set(&key(KEY_ACCESS), "a").unwrap();
        store.set(&key(KEY_REFRESH), "r").unwrap();
        store.set(&key(KEY_PROFILE), "{}").unwrap();
        store.delete(&key(KEY_REFRESH)).unwrap();
        assert_eq!(store.get(&key(KEY_REFRESH)).unwrap(), None);
        assert_eq!(store.get(&key(KEY_ACCESS)).unwrap().as_deref(), Some("a"));
        assert_eq!(store.get(&key(KEY_PROFILE)).unwrap().as_deref(), Some("{}"));
        // 别的服务端桶互不可见。
        assert_eq!(store.get(&format!("other/{KEY_ACCESS}")).unwrap(), None);
    }

    #[test]
    fn an_unattached_vault_reports_unavailable_rather_than_signed_out() {
        let store = VaultCredentialStore::new(None);
        assert!(matches!(store.get("x"), Err(CloudError::Keychain { .. })));
    }

    /// `CmkStore` 契约在保险箱上照旧：版本累积、同版本覆盖、忘掉一个库不碰别的库与会话。
    #[test]
    fn cmk_store_contract_holds_on_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let shared = vault(dir.path(), &keyring);
        let session = VaultCredentialStore::new(Some(Arc::clone(&shared)));
        session.set("b/refresh-token", "r").unwrap();
        let cmks = CredentialCmkStore::new(Arc::new(VaultCredentialStore::new(Some(Arc::clone(
            &shared,
        )))));
        let (one, two) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert!(cmks.load(one).unwrap().is_empty());
        let v1 = luna_sync_core::crypto::Cmk::generate(1);
        let v2 = luna_sync_core::crypto::Cmk::generate(2);
        cmks.save(one, &v2).unwrap();
        cmks.save(one, &v1).unwrap();
        cmks.save(two, &v1).unwrap();
        let got = cmks.load(one).unwrap();
        assert_eq!(
            got.iter().map(|c| c.version()).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(shared
            .contains("cloud/sync-cmk/00000000-0000-0000-0000-000000000001")
            .unwrap());
        cmks.forget(one).unwrap();
        assert!(cmks.load(one).unwrap().is_empty());
        assert_eq!(cmks.load(two).unwrap().len(), 1);
        assert_eq!(
            session.get("b/refresh-token").unwrap().as_deref(),
            Some("r")
        );
    }

    /// 被改坏的库主密钥缓存按「没有」处理——同步服务据此从托管信封重取，而不是卡在「取不到密钥」。
    #[test]
    fn a_tampered_cmk_cache_reads_as_empty_so_it_is_refetched() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let shared = vault(dir.path(), &keyring);
        let cmks = CredentialCmkStore::new(Arc::new(VaultCredentialStore::new(Some(Arc::clone(
            &shared,
        )))));
        let id = Uuid::from_u128(7);
        cmks.save(id, &luna_sync_core::crypto::Cmk::generate(1))
            .unwrap();
        let raw = std::fs::read_to_string(shared.path()).unwrap();
        let name = format!("\"cloud/sync-cmk/{id}\":\"");
        std::fs::write(shared.path(), raw.replace(&name, &format!("{name}AAAA"))).unwrap();
        assert!(cmks.load(id).unwrap().is_empty());
    }

    #[test]
    fn legacy_items_cover_every_old_entry_shape() {
        let id = Uuid::from_u128(9);
        let items = legacy_items(
            luna_config::identity::LEGACY_IDENTIFIER,
            &["api.example".into()],
            &[id],
        );
        let has =
            |service: &str, account: &str| items.iter().any(|(s, a)| s == service && a == account);
        for entry in LEGACY_SESSION_ENTRIES {
            assert!(has("lunarca-cloud", entry), "{entry}");
            assert!(
                has("lunarca-cloud", &format!("api.example/{entry}")),
                "{entry}"
            );
        }
        assert!(has("lunarca-cloud", &format!("sync-cmk/{id}")));
        assert!(has(
            &format!(
                "{}.knowledge-metrics.v1",
                luna_config::identity::LEGACY_IDENTIFIER
            ),
            "device_metrics_key"
        ));
        assert!(
            !items.iter().any(|(s, _)| s == MASTER_KEY_SERVICE),
            "新的主密钥绝不在删除清单里"
        );
    }

    /// 只在登录之后清理、只删不读、做过一次就不再做。
    #[test]
    fn legacy_cleanup_waits_for_sign_in_and_never_reads_legacy_items() {
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        keyring.insert("lunarca-cloud", "api.example/refresh-token", "old");
        keyring.insert(
            &format!(
                "{}.knowledge-metrics.v1",
                luna_config::identity::LEGACY_IDENTIFIER
            ),
            "device_metrics_key",
            "old",
        );
        let shared = vault(dir.path(), &keyring);
        let items = || {
            legacy_items(
                luna_config::identity::LEGACY_IDENTIFIER,
                &["api.example".into()],
                &[],
            )
        };

        // 没登录：一个钥匙串调用都没有，旧条目原样留着。
        assert_eq!(
            clean_legacy(&shared, keyring.as_ref(), || false, items),
            LegacyCleanup::NotSignedIn
        );
        assert!(keyring.calls().is_empty());
        assert!(keyring.contains("lunarca-cloud", "api.example/refresh-token"));

        // 在新版本登录（会话写进保险箱）之后再来：只删、不读任何旧条目。
        VaultCredentialStore::new(Some(Arc::clone(&shared)))
            .set("api.example/refresh-token", "new")
            .unwrap();
        let before = keyring.calls().len();
        assert_eq!(
            clean_legacy(&shared, keyring.as_ref(), || true, items),
            LegacyCleanup::Done { failed: 0 }
        );
        let calls = keyring.calls()[before..].to_vec();
        assert!(!calls.is_empty());
        assert!(
            calls.iter().all(|call| match call {
                KeyringCall::DeleteUnread { service, .. } => service != MASTER_KEY_SERVICE,
                // 写标记用的是已缓存的主密钥，不该再碰钥匙串。
                KeyringCall::Read { .. } | KeyringCall::Write { .. } => false,
            }),
            "旧条目只删不读：{calls:?}"
        );
        assert!(!keyring.contains("lunarca-cloud", "api.example/refresh-token"));
        assert!(!keyring.contains(
            &format!(
                "{}.knowledge-metrics.v1",
                luna_config::identity::LEGACY_IDENTIFIER
            ),
            "device_metrics_key"
        ));
        assert!(keyring.contains(MASTER_KEY_SERVICE, luna_config::MASTER_KEY_ACCOUNT));

        // 做过一次就不再做（连「登录了没有」都不问）。
        let after = keyring.calls().len();
        assert_eq!(
            clean_legacy(&shared, keyring.as_ref(), || panic!("不该再问"), items),
            LegacyCleanup::AlreadyDone
        );
        assert_eq!(keyring.calls().len(), after);
        // 新会话不受清理影响。
        assert_eq!(
            VaultCredentialStore::new(Some(shared))
                .get("api.example/refresh-token")
                .unwrap()
                .as_deref(),
            Some("new")
        );
    }

    /// 删不掉也只试这一次（删除若要授权，重试就是又一串弹窗），且不影响任何别的东西。
    #[test]
    fn a_failed_legacy_delete_is_not_retried() {
        struct Refusing;
        impl KeyringBackend for Refusing {
            fn read(&self, _: &str, _: &str) -> Result<Option<String>, luna_config::SecretError> {
                panic!("清理不得读钥匙串")
            }
            fn write(&self, _: &str, _: &str, _: &str) -> Result<(), luna_config::SecretError> {
                panic!("清理不得写钥匙串")
            }
            fn delete_unread(&self, _: &str, _: &str) -> Result<(), luna_config::SecretError> {
                Err(luna_config::SecretError::Backend("denied".into()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let keyring = Arc::new(MemoryKeyring::new());
        let shared = vault(dir.path(), &keyring);
        shared.set("cloud/b/refresh-token", "r").unwrap();
        let items = || legacy_items(luna_config::identity::LEGACY_IDENTIFIER, &[], &[]);
        assert!(matches!(
            clean_legacy(&shared, &Refusing, || true, items),
            LegacyCleanup::Done { failed } if failed > 0
        ));
        assert_eq!(
            clean_legacy(&shared, &Refusing, || true, items),
            LegacyCleanup::AlreadyDone
        );
    }

    #[test]
    fn sync_bindings_are_read_without_a_config_dir_crashing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(known_sync_bindings(dir.path()), (Vec::new(), Vec::new()));
        std::fs::create_dir_all(dir.path().join("sync").join("local-1")).unwrap();
        assert_eq!(known_sync_bindings(dir.path()), (Vec::new(), Vec::new()));
    }
}
