//! Phase 11 的设备私有装配层：保险箱里的度量钥匙、随机密钥与 app-data 路径只在这里出现。

use base64::Engine as _;
use luna_config::{SecretStore, SecretVault, VaultSecretStore};
use luna_metrics::{DailyRollup, MetricsStore, MetricsSummary};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::metrics_authorization::{AuthorizationGrant, MetricsAuthorization};

/// 度量钥匙在保险箱里的逻辑条目名：`<crate::secrets::METRICS_NAMESPACE>/device_metrics_key`。
const KEY_NAME: &str = "device_metrics_key";

type KeyGenerator = dyn Fn() -> Result<Vec<u8>, String> + Send + Sync;
pub(crate) type AvailabilitySink = dyn Fn(MetricsAvailability) + Send + Sync;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricsAvailability {
    Ready,
    Cleared,
    KeychainUnavailable,
    KeyMissing,
    InvalidKey,
    DatabaseUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricsExportFormat {
    Json,
    Csv,
}

struct MetricsState {
    store: Option<MetricsStore>,
    availability: MetricsAvailability,
    /// 启动 / 打开开关时只记下「用到时再开」，不取度量钥匙（P4）：第一次真正记度量、或第一次
    /// 读汇总时才开库。用户显式「重试」与清除后的新周期另有入口。
    open_on_first_use: bool,
}

/// 一次知识观测的授权凭据（TDD-12 §2.1）。
///
/// **授权机制与工作区度量共用一套**（那份契约明写对两个库统一适用）：开始处取凭据、
/// 终结处以同一张凭据提交、关闭与清除都使既有凭据失效、复验与写入同处一个临界区。
///
/// **落账语义不共用**：工作区度量丢弃在途观测；本库按
/// TDD-06 §3.8 把未结束样本记为
/// **right-censored** 并在再开启后建新周期。截尾行是生存分析里有信息量的数据点，不是
/// "授权已收回还在写库"——`disable` 先让 `store.disable` 把开放样本折成截尾，再收回凭据。
/// 任何装配失败都只让度量不可用，不得降级成普通 hash，也不得影响知识主流程。
pub struct LocalKnowledgeMetrics {
    state: Mutex<MetricsState>,
    lifecycle: Mutex<()>,
    /// 开关、世代与管理屏障的单一运行时事实源；与工作区度量共用同一实现。
    auth: MetricsAuthorization,
    dir: PathBuf,
    secret: Arc<dyn SecretStore>,
    generate_key: Arc<KeyGenerator>,
    availability_sink: Arc<AvailabilitySink>,
}

/// 知识闭环观测专属凭据；不能误传给工作区度量存储。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KnowledgeMetricsGrant(AuthorizationGrant);

impl LocalKnowledgeMetrics {
    /// 只完成内存装配，不触碰保险箱（主密钥在钥匙串）或 SQLite。macOS 在锁屏、首次授权等
    /// 场景下可能让钥匙串调用等待用户交互；Tauri setup 必须先完成 AppState 注册，再把激活
    /// 放进 blocking worker，否则整个应用会停在尚未可用的半初始化状态。
    pub(crate) fn deferred(
        app_data_dir: PathBuf,
        vault: Arc<SecretVault>,
        collection_enabled: bool,
        availability_sink: Arc<AvailabilitySink>,
    ) -> Self {
        Self::deferred_with_dependencies(
            app_data_dir,
            Arc::new(VaultSecretStore::new(
                vault,
                crate::secrets::METRICS_NAMESPACE,
            )),
            Arc::new(|| {
                let mut key = [0_u8; 32];
                getrandom::fill(&mut key).map_err(|error| error.to_string())?;
                Ok(key.to_vec())
            }),
            collection_enabled,
            availability_sink,
        )
    }

    fn deferred_with_dependencies(
        app_data_dir: PathBuf,
        secret: Arc<dyn SecretStore>,
        generate_key: Arc<KeyGenerator>,
        collection_enabled: bool,
        availability_sink: Arc<AvailabilitySink>,
    ) -> Self {
        let dir = app_data_dir.join("metrics");
        Self {
            state: Mutex::new(MetricsState {
                store: None,
                availability: MetricsAvailability::DatabaseUnavailable,
                open_on_first_use: false,
            }),
            lifecycle: Mutex::new(()),
            auth: MetricsAuthorization::new(collection_enabled),
            dir,
            secret,
            generate_key,
            availability_sink,
        }
    }

    #[cfg(test)]
    fn with_dependencies(
        app_data_dir: PathBuf,
        secret: Arc<dyn SecretStore>,
        generate_key: Arc<KeyGenerator>,
        collection_enabled: bool,
    ) -> Self {
        let this = Self::deferred_with_dependencies(
            app_data_dir,
            secret,
            generate_key,
            collection_enabled,
            Arc::new(|_| {}),
        );
        this.activate(collection_enabled);
        // 测试装配等同「已经用过一次」：各用例断言的是开库之后的状态。
        this.open_deferred();
        this
    }

    /// 在 blocking worker 中激活持久层。
    ///
    /// 采集开着、或有历史库可读时**只记下「用到时再开」**，不取度量钥匙（P4）：钥匙在第一次
    /// 真正记度量（[`Self::with_store`]）或第一次读汇总时才取。关闭采集且没有历史库时仍要检查
    /// 是否存在孤立密钥，以免把“不完整清除”谎报为已清除——那一条不存在时不碰钥匙串（保险箱
    /// 先看条目名，没有就不取主密钥）。这一步同样不允许阻塞 Tauri setup。
    pub fn activate(&self, collection_enabled: bool) {
        if collection_enabled || MetricsStore::database_files_exist(&self.dir) {
            if !self.is_ready() {
                self.defer_open();
            }
            return;
        }
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            return;
        };
        let availability = match self.secret.get(KEY_NAME) {
            Ok(None) => MetricsAvailability::Cleared,
            Ok(Some(_)) => MetricsAvailability::DatabaseUnavailable,
            Err(_) => MetricsAvailability::KeychainUnavailable,
        };
        if let Ok(mut state) = self.state.lock() {
            // 关闭采集且没有历史库时，启动不得偷偷创建数据库或设备密钥——此前推迟的开库也一并作废。
            state.store = None;
            state.open_on_first_use = false;
            state.availability = availability;
        }
        (self.availability_sink)(availability);
    }

    pub fn availability(&self) -> MetricsAvailability {
        self.state
            .lock()
            .map(|state| state.availability)
            .unwrap_or(MetricsAvailability::DatabaseUnavailable)
    }

    pub fn is_ready(&self) -> bool {
        self.availability() == MetricsAvailability::Ready
    }

    fn defer_open(&self) {
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            state.open_on_first_use = true;
        }
    }

    fn deferred_open_pending(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.open_on_first_use)
            .unwrap_or(false)
    }

    /// 启动时推迟的开库在此兑现（读汇总 / 导出 / 趋势与显式重试的入口）；没有推迟则什么都不做。
    pub fn open_deferred(&self) {
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            return;
        };
        if self.deferred_open_pending() {
            self.retry_holding_lifecycle();
        }
    }

    /// 开库（必要时懒建新周期），**调用方必须已持有 `lifecycle`**。入口只有推迟的开库兑现
    /// （[`Self::open_deferred`]、`with_store` 的首次使用）与清除后的首次允许写入。凭据丢失但
    /// 旧库仍在时绝不擅自生成第二把密钥；那会把一份无法解释的旧库与一个新周期混在同一路径。
    ///
    /// 持锁版本单独存在只为一件事：`with_store` 的惰性新周期要与凭据复验处在同一个临界区里——
    /// 再抢一次 `lifecycle` 不是重入死锁就是又把建库这一步漏到临界区外面。
    fn retry_holding_lifecycle(&self) {
        let result = self.open_or_create();
        if let Ok(mut state) = self.state.lock() {
            state.open_on_first_use = false;
            match result {
                Ok(store) => {
                    state.store = Some(store);
                    state.availability = MetricsAvailability::Ready;
                }
                Err(status) => {
                    state.store = None;
                    state.availability = status;
                }
            }
        }
        (self.availability_sink)(self.availability());
    }

    fn open_or_create(&self) -> Result<MetricsStore, MetricsAvailability> {
        let key = match self.secret.get(KEY_NAME) {
            Ok(Some(encoded)) => decode_key(&encoded)?,
            // 保险箱时代建过钥匙之后再丢（钥匙串被清、保险箱认证失败）：如实报缺钥，绝不擅自
            // 换第二把钥匙——那会把一份无法解释的旧库与一个新周期混在同一路径。
            Ok(None)
                if MetricsStore::database_files_exist(&self.dir)
                    && vault_key_cycle_started(&self.dir) =>
            {
                return Err(MetricsAvailability::KeyMissing);
            }
            // 升级上来的旧库（钥匙在旧钥匙串条目里，按裁决不读）：改名归档到同目录、开新周期。
            // 只发生一次——归档之后立刻落下周期标记，之后再丢钥匙就走上面那一支。
            Ok(None) if MetricsStore::database_files_exist(&self.dir) => {
                archive_legacy_database(
                    &self.dir,
                    &chrono::Local::now().format("%Y%m%d").to_string(),
                )
                .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
                mark_vault_key_cycle(&self.dir)?;
                if !self.auth.enabled() {
                    // 采集关着：归档之后已无历史，与「关闭且没有历史库」同一状态，不建钥匙与新库。
                    return Err(MetricsAvailability::Cleared);
                }
                self.create_key()?
            }
            Ok(None) => {
                mark_vault_key_cycle(&self.dir)?;
                self.create_key()?
            }
            Err(_) => return Err(MetricsAvailability::KeychainUnavailable),
        };
        MetricsStore::open(&self.dir, key).map_err(|_| MetricsAvailability::DatabaseUnavailable)
    }

    fn create_key(&self) -> Result<Vec<u8>, MetricsAvailability> {
        let key = (self.generate_key)().map_err(|_| MetricsAvailability::KeychainUnavailable)?;
        if key.len() != 32 {
            return Err(MetricsAvailability::InvalidKey);
        }
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&key);
        self.secret
            .set(KEY_NAME, &encoded)
            .map_err(|_| MetricsAvailability::KeychainUnavailable)?;
        Ok(key)
    }

    /// 取得一次观测的授权凭据；`None` = 此刻未授权，调用方什么都不用做。
    ///
    /// 开关与世代由同一个同步事实源一次读取，调用方不再先读设置再传布尔值；因此不存在
    /// “读到旧开关、拿到新世代”的撕裂窗口。
    #[must_use]
    pub(crate) fn begin(&self) -> Option<KnowledgeMetricsGrant> {
        self.auth.begin().map(KnowledgeMetricsGrant)
    }

    /// 凭凭据写一次。
    ///
    /// **整段都在 `lifecycle` 里**：`clear` / `disable` / `retry` 走的是同一把锁，于是
    /// "复验凭据"与"真的写进去"之间插不进任何东西。此前这里是**锁外读**——先在锁外看
    /// `availability()`、发现是 `Cleared` 就调 `retry()` 建库，然后才去抢 `state`。清除刚
    /// 返回、这条在途观测紧接着到达时，它会拿一张过期凭据把新密钥与空库当场建出来：用户
    /// 按下「清除」，磁盘上又出现一份东西。惰性新周期本身是对的（TDD-06 §3.8），错的是
    /// 让**上一代**的写者去开它。
    pub fn with_store<R>(
        &self,
        grant: KnowledgeMetricsGrant,
        f: impl FnOnce(&MetricsStore) -> Result<R, luna_metrics::MetricsError>,
    ) -> Option<R> {
        let _lifecycle = self.lifecycle.lock().ok()?;
        if !self.auth.is_current(grant.0) {
            return None;
        }
        if self.availability() == MetricsAvailability::Cleared || self.deferred_open_pending() {
            self.retry_holding_lifecycle();
        }
        let mut guard = self.state.lock().ok()?;
        let store = guard.store.as_ref()?;
        let result = match f(store) {
            Ok(value) => Some(value),
            Err(_) => {
                guard.store = None;
                guard.availability = MetricsAvailability::DatabaseUnavailable;
                None
            }
        };
        if result.is_none() {
            drop(guard);
            (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
        }
        result
    }

    pub fn summary(
        &self,
        vault_root: &str,
        now_ms: i64,
    ) -> Result<MetricsSummary, MetricsAvailability> {
        self.open_deferred();
        let mut guard = self
            .state
            .lock()
            .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
        let status = guard.availability;
        let store = guard.store.as_ref().ok_or(status)?;
        let result = match store.summary_for_vault(vault_root, now_ms) {
            Ok(summary) => Ok(summary),
            Err(_) => {
                guard.store = None;
                guard.availability = MetricsAvailability::DatabaseUnavailable;
                Err(MetricsAvailability::DatabaseUnavailable)
            }
        };
        if result.is_err() {
            drop(guard);
            (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
        }
        result
    }

    pub fn export(
        &self,
        vault_root: &str,
        now_ms: i64,
        start_day: Option<i64>,
        end_day: Option<i64>,
        format: MetricsExportFormat,
    ) -> Result<String, MetricsAvailability> {
        self.open_deferred();
        let mut guard = self
            .state
            .lock()
            .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
        let status = guard.availability;
        let store = guard.store.as_ref().ok_or(status)?;
        let export = match format {
            MetricsExportFormat::Json => {
                store.export_aggregate_json(vault_root, now_ms, start_day, end_day)
            }
            MetricsExportFormat::Csv => {
                store.export_aggregate_csv(vault_root, now_ms, start_day, end_day)
            }
        };
        let result = match export {
            Ok(serialized) => Ok(serialized),
            Err(_) => {
                guard.store = None;
                guard.availability = MetricsAvailability::DatabaseUnavailable;
                Err(MetricsAvailability::DatabaseUnavailable)
            }
        };
        if result.is_err() {
            drop(guard);
            (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
        }
        result
    }

    pub fn daily(
        &self,
        vault_root: &str,
        now_ms: i64,
        days: u16,
    ) -> Result<Vec<DailyRollup>, MetricsAvailability> {
        self.open_deferred();
        let mut guard = self
            .state
            .lock()
            .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
        let status = guard.availability;
        let store = guard.store.as_ref().ok_or(status)?;
        let result = match store.daily_aggregates_for_vault(vault_root, now_ms, days) {
            Ok(daily) => Ok(daily),
            Err(_) => {
                guard.store = None;
                guard.availability = MetricsAvailability::DatabaseUnavailable;
                Err(MetricsAvailability::DatabaseUnavailable)
            }
        };
        if result.is_err() {
            drop(guard);
            (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
        }
        result
    }

    /// 「停止本地记录」：只拦新写入并把未决样本记为截尾，不暗删旧数据（TDD-06 §3.8）。
    ///
    /// **顺序是「先截尾、再收回凭据」**：截尾是这些开放样本的**落账方式**，不是收回
    /// 授权的副作用。先把 `knowledge_metric_state` 里未结束的折成 right-censored（那是生存
    /// 分析里有信息量的数据点），再递增世代让在途凭据失效——反过来做的话，被收回的凭据
    /// 会连带把"这些样本当时开着"这件事一起丢掉，cohort 的分母从此少一块且看不出来。
    pub fn disable(&self, now_ms: i64) {
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            // 生命周期锁异常时仍必须 fail closed；不能因无法完成截尾就继续签发新凭据。
            self.auth.set_enabled(false);
            return;
        };
        let mut unavailable = false;
        if let Ok(mut state) = self.state.lock() {
            if let Some(store) = state.store.as_ref() {
                if store.disable(now_ms).is_err() {
                    state.store = None;
                    state.availability = MetricsAvailability::DatabaseUnavailable;
                    unavailable = true;
                }
            }
        }
        // 关闭授权使既有凭据失效（TDD-12 §2.1）：关掉记录就是收回**这一段**的观测授权，
        // 在途的那条不能再凭旧凭据提交一条正常的终结观测；再开启后是新周期，不补报。
        self.auth.set_enabled(false);
        if unavailable {
            (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
        }
    }

    /// 真清除：先关闭句柄并删主库/WAL/SHM，再删旧设备密钥。任一步失败都如实返回，
    /// 且不会用旧密钥立刻重建空库；新周期只在下次允许采集时懒创建。
    pub fn clear(&self) -> Result<(), MetricsAvailability> {
        // 屏障先阻止新凭据并作废旧凭据；并发设置变化写入同一授权事实，不会在清除结束时丢失。
        let _authorization_suspension = self.auth.suspend();
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| MetricsAvailability::DatabaseUnavailable)?;
        if let Some(store) = state.store.take() {
            if store.clear().is_err() {
                state.availability = MetricsAvailability::DatabaseUnavailable;
                drop(state);
                (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
                return Err(MetricsAvailability::DatabaseUnavailable);
            }
        } else {
            if remove_database_files(&self.dir).is_err() {
                state.availability = MetricsAvailability::DatabaseUnavailable;
                drop(state);
                (self.availability_sink)(MetricsAvailability::DatabaseUnavailable);
                return Err(MetricsAvailability::DatabaseUnavailable);
            }
        }
        if self.secret.delete(KEY_NAME).is_err() {
            state.availability = MetricsAvailability::KeychainUnavailable;
            drop(state);
            (self.availability_sink)(MetricsAvailability::KeychainUnavailable);
            return Err(MetricsAvailability::KeychainUnavailable);
        }
        state.availability = MetricsAvailability::Cleared;
        // 清除之后只走「下次允许记录时懒建新周期」那一条（`with_store`），读汇总不得把库开回来。
        state.open_on_first_use = false;
        drop(state);
        (self.availability_sink)(MetricsAvailability::Cleared);
        Ok(())
    }

    /// 设置打开后的同步授权投影。持久层激活仍在 blocking worker，二者职责不混合。
    pub fn enable(&self) {
        self.auth.set_enabled(true);
    }
}

/// 周期标记：存在 = 本目录的度量库（若有）由保险箱里的钥匙写成。建第一把保险箱钥匙之前落下。
///
/// 放在度量目录而不是保险箱里：整份保险箱丢失（主密钥被清）时标记必须还在，否则「之后丢了钥匙」
/// 会被认成「刚从旧版本升级」而静默换钥，`key_missing` 这条不变量就没了。
const VAULT_KEY_CYCLE_MARKER: &str = "vault-key-cycle";
/// 旧库归档名：`<库文件名>.legacy-keychain-<YYYYMMDD>`，WAL / SHM 跟在后面（`…-wal` / `…-shm`），
/// 归档的库仍能被 SQLite 原样打开。
const LEGACY_ARCHIVE_INFIX: &str = ".legacy-keychain-";

fn vault_key_cycle_started(dir: &std::path::Path) -> bool {
    dir.join(VAULT_KEY_CYCLE_MARKER).exists()
}

fn mark_vault_key_cycle(dir: &std::path::Path) -> Result<(), MetricsAvailability> {
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(dir.join(VAULT_KEY_CYCLE_MARKER), b""))
        .map_err(|_| MetricsAvailability::DatabaseUnavailable)
}

/// 把旧钥匙串时代的度量库连同 WAL / SHM 改名归档（同目录，**永不删除**；「清除」也不碰它）。
/// 当天已有同名归档时加序号。
fn archive_legacy_database(dir: &std::path::Path, date: &str) -> Result<(), std::io::Error> {
    let path = MetricsStore::database_path(dir);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let taken = |base: &str| {
        ["", "-wal", "-shm"]
            .iter()
            .any(|suffix| dir.join(format!("{base}{suffix}")).exists())
    };
    let mut base = format!("{name}{LEGACY_ARCHIVE_INFIX}{date}");
    let mut n = 2;
    while taken(&base) {
        base = format!("{name}{LEGACY_ARCHIVE_INFIX}{date}-{n}");
        n += 1;
    }
    for suffix in ["", "-wal", "-shm"] {
        match std::fs::rename(
            dir.join(format!("{name}{suffix}")),
            dir.join(format!("{base}{suffix}")),
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn decode_key(encoded: &str) -> Result<Vec<u8>, MetricsAvailability> {
    let key = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| MetricsAvailability::InvalidKey)?;
    if key.len() == 32 {
        Ok(key)
    } else {
        Err(MetricsAvailability::InvalidKey)
    }
}

fn remove_database_files(dir: &std::path::Path) -> Result<(), std::io::Error> {
    let path = MetricsStore::database_path(dir);
    for candidate in [
        path.clone(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        match std::fs::remove_file(candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_config::{MemorySecretStore, SecretError};

    struct DeniedSecret;
    impl SecretStore for DeniedSecret {
        fn get(&self, _key: &str) -> Result<Option<String>, SecretError> {
            Err(SecretError::Backend("denied".into()))
        }
        fn set(&self, _key: &str, _value: &str) -> Result<(), SecretError> {
            Err(SecretError::Backend("denied".into()))
        }
        fn delete(&self, _key: &str) -> Result<(), SecretError> {
            Err(SecretError::Backend("denied".into()))
        }
    }

    struct PanicSecret;
    impl SecretStore for PanicSecret {
        fn get(&self, _key: &str) -> Result<Option<String>, SecretError> {
            panic!("deferred construction must not read the secret store")
        }
        fn set(&self, _key: &str, _value: &str) -> Result<(), SecretError> {
            panic!("deferred construction must not write the secret store")
        }
        fn delete(&self, _key: &str) -> Result<(), SecretError> {
            panic!("deferred construction must not delete from the secret store")
        }
    }

    fn service(dir: &std::path::Path, secret: Arc<MemorySecretStore>) -> LocalKnowledgeMetrics {
        LocalKnowledgeMetrics::with_dependencies(
            dir.to_owned(),
            secret,
            Arc::new(|| Ok(vec![9; 32])),
            true,
        )
    }

    /// 同步路径的「取凭据 + 立刻提交」：开始与提交之间没有可插入的窗口。
    fn record<R>(
        metrics: &LocalKnowledgeMetrics,
        f: impl FnOnce(&MetricsStore) -> Result<R, luna_metrics::MetricsError>,
    ) -> Option<R> {
        let grant = metrics.begin()?;
        metrics.with_store(grant, f)
    }

    #[test]
    fn deferred_startup_does_not_touch_keychain_or_database() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = LocalKnowledgeMetrics::deferred_with_dependencies(
            dir.path().to_owned(),
            Arc::new(PanicSecret),
            Arc::new(|| panic!("deferred construction must not generate a key")),
            false,
            Arc::new(|_| {}),
        );
        assert_eq!(
            metrics.availability(),
            MetricsAvailability::DatabaseUnavailable
        );
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
    }

    /// 度量钥匙懒取（P4）：启动激活不读保险箱，第一次真正记度量时才取；读汇总也会把推迟的
    /// 开库兑现（否则设置页冷启动停在「不可用」）。
    #[test]
    fn startup_activation_reads_the_key_only_on_first_use() {
        struct Counting {
            inner: MemorySecretStore,
            reads: std::sync::atomic::AtomicUsize,
        }
        impl SecretStore for Counting {
            fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.inner.get(key)
            }
            fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
                self.inner.set(key, value)
            }
            fn delete(&self, key: &str) -> Result<(), SecretError> {
                self.inner.delete(key)
            }
        }
        let reads = |secret: &Counting| secret.reads.load(std::sync::atomic::Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(Counting {
            inner: MemorySecretStore::new(),
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let metrics = LocalKnowledgeMetrics::deferred_with_dependencies(
            dir.path().to_owned(),
            Arc::clone(&secret) as Arc<dyn SecretStore>,
            Arc::new(|| Ok(vec![9; 32])),
            true,
            Arc::new(|_| {}),
        );
        metrics.activate(true);
        assert_eq!(reads(&secret), 0, "启动激活不该取度量钥匙");
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
        assert!(record(&metrics, |_| Ok(())).is_some());
        assert_eq!(reads(&secret), 1, "第一次记度量时取一次");
        assert_eq!(metrics.availability(), MetricsAvailability::Ready);
        assert!(record(&metrics, |_| Ok(())).is_some());
        assert_eq!(reads(&secret), 1, "开库之后不再取");

        // 有历史库、采集关着：读汇总时才开库。
        let later = LocalKnowledgeMetrics::deferred_with_dependencies(
            dir.path().to_owned(),
            Arc::clone(&secret) as Arc<dyn SecretStore>,
            Arc::new(|| Ok(vec![9; 32])),
            false,
            Arc::new(|_| {}),
        );
        later.activate(false);
        assert_eq!(reads(&secret), 1);
        assert!(later.summary("/v", 1).is_ok());
        assert_eq!(reads(&secret), 2);
    }

    #[test]
    fn first_use_creates_random_key_only_in_secret_store() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = service(dir.path(), Arc::clone(&secret));
        assert_eq!(metrics.availability(), MetricsAvailability::Ready);
        assert!(secret.get(KEY_NAME).unwrap().is_some());
        let bytes = std::fs::read(MetricsStore::database_path(dir.path().join("metrics"))).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&secret.get(KEY_NAME).unwrap().unwrap()));
    }

    /// 保险箱时代建过钥匙之后再丢：如实报缺钥，不归档、不换钥。
    #[test]
    fn later_key_loss_with_existing_database_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let first = service(dir.path(), Arc::clone(&secret));
        record(&first, |store| {
            store.suggestion(
                "/v",
                "token",
                luna_metrics::SuggestionEvent::Shown,
                luna_metrics::SuggestionSurface::Graph,
                1,
            )
        })
        .unwrap();
        drop(first);
        secret.delete(KEY_NAME).unwrap();

        let metrics = service(dir.path(), Arc::new(MemorySecretStore::new()));
        assert_eq!(metrics.availability(), MetricsAvailability::KeyMissing);
        assert!(archives(&dir.path().join("metrics")).is_empty(), "不得归档");
        assert!(MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
    }

    fn archives(metrics_dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(metrics_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(LEGACY_ARCHIVE_INFIX))
            .collect();
        names.sort();
        names
    }

    /// 从旧版本升级：旧库的钥匙在旧钥匙串条目里（按裁决不读）。旧库连同 WAL / SHM 改名归档、
    /// 新钥匙开新周期，不出现 `key_missing`；只做一次，归档永不删除。
    #[test]
    fn legacy_database_without_a_vault_key_is_archived_and_a_new_cycle_starts() {
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        let legacy = MetricsStore::open(&metrics_dir, vec![1; 32]).unwrap();
        drop(legacy);
        std::fs::write(
            format!(
                "{}-wal",
                MetricsStore::database_path(&metrics_dir).display()
            ),
            b"legacy wal",
        )
        .unwrap();

        let secret = Arc::new(MemorySecretStore::new());
        let metrics = service(dir.path(), Arc::clone(&secret));
        assert_eq!(metrics.availability(), MetricsAvailability::Ready);
        assert!(secret.get(KEY_NAME).unwrap().is_some(), "换了新钥匙");
        let archived = archives(&metrics_dir);
        assert_eq!(archived.len(), 2, "库与 WAL 都归档：{archived:?}");
        assert!(archived[0].starts_with("knowledge-metrics.sqlite.legacy-keychain-"));
        assert!(archived[1].ends_with("-wal"));
        assert!(record(&metrics, |_| Ok(())).is_some());

        // 清除新周期不碰归档。
        metrics.clear().unwrap();
        assert_eq!(archives(&metrics_dir), archived);

        // 只做一次：之后再丢钥匙 → key_missing，不再归档。
        assert!(record(&metrics, |_| Ok(())).is_some());
        drop(metrics);
        secret.delete(KEY_NAME).unwrap();
        let later = service(dir.path(), Arc::clone(&secret));
        assert_eq!(later.availability(), MetricsAvailability::KeyMissing);
        assert_eq!(archives(&metrics_dir), archived);
    }

    /// 采集关着时升级：归档旧库后与「关闭且没有历史」同一状态，不建钥匙与新库。
    #[test]
    fn legacy_database_archived_while_collection_is_off_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        drop(MetricsStore::open(&metrics_dir, vec![1; 32]).unwrap());
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = LocalKnowledgeMetrics::with_dependencies(
            dir.path().to_owned(),
            Arc::clone(&secret) as Arc<dyn SecretStore>,
            Arc::new(|| Ok(vec![9; 32])),
            false,
        );
        assert_eq!(metrics.availability(), MetricsAvailability::Cleared);
        assert!(secret.get(KEY_NAME).unwrap().is_none());
        assert!(!MetricsStore::database_files_exist(&metrics_dir));
        assert_eq!(archives(&metrics_dir).len(), 1);
    }

    #[test]
    fn clear_deletes_key_and_lazily_starts_a_new_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = service(dir.path(), Arc::clone(&secret));
        metrics.clear().unwrap();
        assert_eq!(metrics.availability(), MetricsAvailability::Cleared);
        assert!(secret.get(KEY_NAME).unwrap().is_none());
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
        assert!(record(&metrics, |_| Ok(())).is_some());
        assert_eq!(metrics.availability(), MetricsAvailability::Ready);
        assert!(secret.get(KEY_NAME).unwrap().is_some());
    }

    #[test]
    fn lazy_new_cycle_publishes_cleared_then_ready() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_from_sink = Arc::clone(&seen);
        let metrics = LocalKnowledgeMetrics::deferred_with_dependencies(
            dir.path().to_owned(),
            secret,
            Arc::new(|| Ok(vec![9; 32])),
            true,
            Arc::new(move |status| seen_from_sink.lock().unwrap().push(status)),
        );
        metrics.activate(true);
        assert!(record(&metrics, |_| Ok(())).is_some());
        metrics.clear().unwrap();
        assert!(record(&metrics, |_| Ok(())).is_some());
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                MetricsAvailability::Ready,
                MetricsAvailability::Cleared,
                MetricsAvailability::Ready,
            ]
        );
    }

    #[test]
    fn reading_after_clear_does_not_recreate_storage() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = service(dir.path(), Arc::clone(&secret));
        metrics.clear().unwrap();
        assert_eq!(metrics.summary("/v", 1), Err(MetricsAvailability::Cleared));
        assert!(secret.get(KEY_NAME).unwrap().is_none());
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
    }

    #[test]
    fn keychain_denial_is_honest_and_captures_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = LocalKnowledgeMetrics::with_dependencies(
            dir.path().to_owned(),
            Arc::new(DeniedSecret),
            Arc::new(|| Ok(vec![9; 32])),
            true,
        );
        assert_eq!(
            metrics.availability(),
            MetricsAvailability::KeychainUnavailable
        );
        assert!(record(&metrics, |_| Ok(())).is_none());
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
    }

    #[test]
    fn disabled_collection_runs_no_write_closure_and_keeps_history_readable() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = service(dir.path(), Arc::new(MemorySecretStore::new()));
        record(&metrics, |store| {
            store.suggestion(
                "/v",
                "token",
                luna_metrics::SuggestionEvent::Shown,
                luna_metrics::SuggestionSurface::Graph,
                1,
            )
        })
        .unwrap();
        metrics.disable(2);
        let mut called = false;
        assert!(record(&metrics, |_| {
            called = true;
            Ok(())
        })
        .is_none());
        assert!(!called);
        assert_eq!(metrics.summary("/v", 2).unwrap().suggestions.shown, 1);
    }

    #[test]
    fn corrupt_database_fails_closed_without_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        std::fs::create_dir_all(&metrics_dir).unwrap();
        let path = MetricsStore::database_path(&metrics_dir);
        std::fs::write(&path, b"not a sqlite database").unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        secret
            .set(
                KEY_NAME,
                &base64::engine::general_purpose::STANDARD_NO_PAD.encode([9_u8; 32]),
            )
            .unwrap();
        let metrics = service(dir.path(), secret);
        assert_eq!(
            metrics.availability(),
            MetricsAvailability::DatabaseUnavailable
        );
        assert_eq!(std::fs::read(path).unwrap(), b"not a sqlite database");
    }

    #[test]
    fn simulated_sync_vault_never_receives_metrics_files() {
        let app_data = tempfile::tempdir().unwrap();
        let sync_root = tempfile::tempdir().unwrap();
        let vault = sync_root.path().join("Dropbox").join("Vault");
        std::fs::create_dir_all(&vault).unwrap();
        let metrics = service(app_data.path(), Arc::new(MemorySecretStore::new()));
        let root = vault.display().to_string();
        record(&metrics, |store| {
            store.suggestion(
                &root,
                "token",
                luna_metrics::SuggestionEvent::Shown,
                luna_metrics::SuggestionSurface::Graph,
                1,
            )
        })
        .unwrap();
        assert!(std::fs::read_dir(&vault).unwrap().next().is_none());
        assert!(MetricsStore::database_files_exist(
            app_data.path().join("metrics")
        ));
    }

    /// **「停止本地记录」的落账语义逐条不变**（TDD-06 §3.8）：未结束样本记为
    /// **right-censored**，不是丢弃。收回凭据与截尾是两件事——前者只是不再接受正常的
    /// 终结提交，后者是这条样本本来就该有的落账方式。这条测试是把它钉住的回归锁：机制
    /// 对齐（`disable` 现在会递增授权世代）不得顺手把 cohort 设计删掉。
    #[test]
    fn turning_recording_off_censors_open_samples_instead_of_dropping_them() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = service(dir.path(), Arc::new(MemorySecretStore::new()));
        // 从 `<3` 首次跨到 `>=3` 才开一个 cohort 周期（TDD-06 §3.8）。
        record(&metrics, |store| {
            store.observe_topic_clusters("/v", &[("rust".to_owned(), 2)], 1)
        })
        .unwrap();
        record(&metrics, |store| {
            store.observe_topic_clusters("/v", &[("rust".to_owned(), 3)], 2)
        })
        .unwrap();
        let before = metrics.summary("/v", 2).unwrap();
        assert_eq!((before.topic_time.open, before.topic_time.censored), (1, 0));

        metrics.disable(3);

        let after = metrics.summary("/v", 4).unwrap();
        assert_eq!(
            (after.topic_time.open, after.topic_time.censored),
            (0, 1),
            "未结束样本必须记为截尾，不是消失"
        );
    }

    /// 关闭记录**使既有凭据失效**（TDD-12 §2.1，两个度量库共用的机制）。
    #[test]
    fn a_write_that_began_before_recording_was_turned_off_is_not_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = service(dir.path(), Arc::new(MemorySecretStore::new()));
        let grant = metrics.begin().expect("已授权");
        metrics.disable(1);
        let mut called = false;
        assert!(
            metrics
                .with_store(grant, |_| {
                    called = true;
                    Ok(())
                })
                .is_none(),
            "关掉记录之后，旧凭据不能再提交一条正常的终结观测"
        );
        assert!(!called);
    }

    #[test]
    fn a_poisoned_storage_lifecycle_still_fails_closed_on_disable() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = service(dir.path(), Arc::new(MemorySecretStore::new()));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = metrics.lifecycle.lock().unwrap();
            panic!("poison lifecycle");
        }));
        metrics.disable(1);
        assert!(metrics.begin().is_none(), "存储异常不能让隐私开关失效");
    }

    /// 清除历史同样使既有凭据失效——否则一条在途观测会把新密钥与空库当场建出来，
    /// 用户看到的「已清除」当场失效。惰性新周期本身是对的，错的是让**上一代**的写者去开它。
    #[test]
    fn a_write_that_began_before_the_clear_does_not_start_a_new_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = service(dir.path(), Arc::clone(&secret));
        let grant = metrics.begin().expect("已授权");
        metrics.clear().unwrap();

        assert!(metrics.with_store(grant, |_| Ok(())).is_none());
        assert_eq!(metrics.availability(), MetricsAvailability::Cleared);
        assert!(secret.get(KEY_NAME).unwrap().is_none(), "不得重建设备密钥");
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
        // 新一代的凭据照常懒建新周期——作废的是那张凭据，不是这个功能（TDD-06 §3.8）。
        assert!(record(&metrics, |_| Ok(())).is_some());
        assert_eq!(metrics.availability(), MetricsAvailability::Ready);
    }

    /// 顺序只能由结构看住：复验与惰性新周期都必须在 `lifecycle` 临界区**之内**。
    ///
    /// 从外面看不见它——把复验挪回锁外，上面那些用例照样全绿，因为"过了检查、正在等锁"
    /// 这个中间态没有任何外部手段能停住（能停住它的探针只在有 bug 的版本里才有位置可放）。
    #[test]
    fn the_credential_is_re_checked_inside_the_lifecycle_section() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub fn with_store<R>(")
            .nth(1)
            .expect("找不到 with_store")
            .split("\n    pub fn ")
            .next()
            .expect("with_store 的边界变了");
        let lifecycle = body
            .find("self.lifecycle.lock()")
            .expect("with_store 必须进生命周期临界区");
        let recheck = body
            .find("self.auth.is_current(grant.0)")
            .expect("with_store 必须复验凭据");
        let lazy_cycle = body
            .find("self.retry_holding_lifecycle()")
            .expect("惰性新周期必须走持锁版本");
        assert!(lifecycle < recheck, "复验必须在生命周期临界区之内");
        assert!(recheck < lazy_cycle, "复验必须在惰性新周期之前");
        assert!(
            !body.contains("self.retry()"),
            "临界区里不得再去抢一次 lifecycle"
        );
    }

    #[test]
    fn disabled_fresh_launch_creates_neither_database_nor_key() {
        let dir = tempfile::tempdir().unwrap();
        let secret = Arc::new(MemorySecretStore::new());
        let metrics = LocalKnowledgeMetrics::with_dependencies(
            dir.path().to_owned(),
            Arc::clone(&secret) as Arc<dyn SecretStore>,
            Arc::new(|| Ok(vec![9; 32])),
            false,
        );
        assert_eq!(metrics.availability(), MetricsAvailability::Cleared);
        assert!(secret.get(KEY_NAME).unwrap().is_none());
        assert!(!MetricsStore::database_files_exist(
            dir.path().join("metrics")
        ));
    }
}
