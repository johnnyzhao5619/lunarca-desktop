//! 本地知识闭环度量命令域（PRD-16 §3.10，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：五项指标的读取、清除、重试与导出，以及可用性变化事件。
//! 存储与聚合在 [`luna_metrics`]，隔离与脱敏口径见 TDD-06 §3.8——此处不复制。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// Phase 11 的纯聚合投影：没有 token、路径、事件明细或自由文本，前端只负责显示。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeMetricsStatusDto {
    Ready,
    Cleared,
    VaultUnavailable,
    KeychainUnavailable,
    KeyMissing,
    InvalidKey,
    DatabaseUnavailable,
}

impl From<crate::metrics::MetricsAvailability> for KnowledgeMetricsStatusDto {
    fn from(value: crate::metrics::MetricsAvailability) -> Self {
        use crate::metrics::MetricsAvailability as M;
        match value {
            M::Ready => Self::Ready,
            M::Cleared => Self::Cleared,
            M::KeychainUnavailable => Self::KeychainUnavailable,
            M::KeyMissing => Self::KeyMissing,
            M::InvalidKey => Self::InvalidKey,
            M::DatabaseUnavailable => Self::DatabaseUnavailable,
        }
    }
}

/// 指标持久层在后台完成激活后的状态变更。事件不携带任何度量值或设备标识；前端收到后
/// 重新读取当前 Vault 的汇总投影，避免冷启动设置页停在瞬时的“未就绪”状态。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "knowledge-metrics://availability")]
pub struct KnowledgeMetricsAvailabilityChanged {
    pub status: KnowledgeMetricsStatusDto,
}

pub(crate) fn emit_local_knowledge_metrics_availability_changed(app: &tauri::AppHandle) {
    if let Some(metrics) = app.try_state::<crate::metrics::LocalKnowledgeMetrics>() {
        let _ = (KnowledgeMetricsAvailabilityChanged {
            status: metrics.availability().into(),
        })
        .emit(app);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MetricRatioDto {
    pub numerator: u32,
    pub denominator: u32,
    pub censored: u32,
    pub open: u32,
    /// 百分比的万分位（10000 = 100%）；无分母为 `None`，前端不反算事实。
    pub percentage_basis_points: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LocalKnowledgeMetricsDto {
    pub status: KnowledgeMetricsStatusDto,
    pub range_start_day: Option<String>,
    pub range_end_day: Option<String>,
    pub suggestion_accepted: u32,
    pub suggestion_shown: u32,
    pub suggestion_dismissed: u32,
    pub suggestion_rolled_back: u32,
    pub suggestion_percentage_basis_points: Option<u16>,
    pub topic_completed: u32,
    pub topic_censored: u32,
    pub topic_open: u32,
    pub topic_median_minutes: Option<u32>,
    pub linked_within_24h: MetricRatioDto,
    pub linked_automatic: u32,
    pub linked_confirmed: u32,
    pub linked_manual: u32,
    pub citation_attempts: u32,
    pub citation_succeeded: u32,
    pub citation_stale_anchor: u32,
    pub citation_source_missing: u32,
    pub model_supplement: MetricRatioDto,
    pub trend: Vec<KnowledgeMetricTrendDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeMetricKindDto {
    Suggestions,
    TopicTime,
    LinkedWithin24Hours,
    Citations,
    ModelSupplement,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct KnowledgeMetricTrendDto {
    pub day: String,
    pub metric: KnowledgeMetricKindDto,
    pub numerator: u32,
    pub denominator: u32,
    pub censored: u32,
    pub percentage_basis_points: Option<u16>,
}

pub(crate) fn metric_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 把同一索引快照投给 cohort/24h 状态机。这里只传事实；HMAC、窗口和分母判定仍全部在
/// `luna-metrics`。关闭设置后不读取/不写入任何新样本。
fn reconcile_local_knowledge_metrics(app: &tauri::AppHandle, allow_new_cycle: bool) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let enabled = state
        .settings_snapshot()
        .get_bool("privacy.local_knowledge_metrics")
        .unwrap_or(false);
    if !enabled {
        return;
    }
    // 授权凭据在**动作开始处**取（TDD-12 §2.1）：下面要抢 Vault 锁、算一遍事实，这期间
    // 用户完全可能关掉记录或清除历史；那时这条基线重算就该整条不记，而不是到写入处再问
    // 一次"现在开着吗"。
    let grant = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| metrics.begin());
    let facts = state.vault.lock().ok().and_then(|guard| {
        guard.as_ref().and_then(|vault| {
            vault
                .knowledge_metrics_facts()
                .ok()
                .map(|facts| (vault.root().display().to_string(), facts))
        })
    });
    let Some((root, facts)) = facts else {
        return;
    };
    let now_ms = metric_now_ms();
    let metrics = app.try_state::<crate::metrics::LocalKnowledgeMetrics>();
    if !allow_new_cycle && metrics.as_ref().is_some_and(|metrics| !metrics.is_ready()) {
        return;
    }
    let _ = metrics.zip(grant).and_then(|(metrics, grant)| {
        metrics.with_store(grant, |store| {
            store.observe_topic_clusters(&root, &facts.topic_clusters, now_ms)?;
            store.observe_leaf_facts(&root, &facts.leaf_paths, &facts.linked_leaf_paths, now_ms)
        })
    });
}

/// 写盘调用常仍持有 Vault 锁；异步排一拍，避免在 `emit_vault_changed` 内重入同一把锁。
/// 基线重算的去抖计次——与知识地图刷新（`schedule_knowledge_map_refresh`）同一形状。
static METRICS_RECONCILE_REVISION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// 去抖 350ms 后重算一次度量基线（同一条 Vault 变更边界上的另一个派生刷新）。
///
/// **它必须与知识地图刷新一样去抖**：这个函数做的是"抢 Vault 锁 → 全库遍历 → 写
/// sqlite"，而调用它的边界一次可以发出很多条（批量移动/删除、外部编辑器一次保存多文件）。
/// 不去抖的表现不是"慢一点"：N 次全库遍历会把 tokio 工作线程占满、并全部排在触发它的
/// 那把锁后面，界面此时发出的任何命令都排不进去（2026-09-02 真机「拖拽后整个应用不动」
/// 的两个放大器之一，另一个是批量在锁内逐条广播，见 `emit_vault_changes`）。
pub(crate) fn schedule_local_knowledge_metrics_reconcile(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    let revision = METRICS_RECONCILE_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        // 这一拍里又来了新的变更 → 让给最后那一次，本次直接作废。
        if METRICS_RECONCILE_REVISION.load(Ordering::SeqCst) != revision {
            return;
        }
        reconcile_local_knowledge_metrics(&app, true);
    });
}

/// 启动与清除后重新开启共用这一条编排：阻塞 I/O 不进 Tauri setup/命令线程，状态事件
/// 一定先于基线重算发出。`activate` 本身幂等，已有可读历史时不会重开数据库。
pub(crate) fn activate_local_knowledge_metrics(app: &tauri::AppHandle, enabled: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(metrics) = app.try_state::<crate::metrics::LocalKnowledgeMetrics>() {
            metrics.activate(enabled);
            schedule_local_knowledge_metrics_reconcile(&app);
        }
    });
}

pub(crate) fn record_local_knowledge_metric_moves(
    app: &tauri::AppHandle,
    root: &Path,
    moves: impl IntoIterator<Item = (String, String)>,
) {
    let root = root.display().to_string();
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                for (from, to) in moves {
                    store.leaf_moved(&root, &from, &to)?;
                }
                Ok(())
            })
        });
}

pub(crate) fn record_local_knowledge_metric_deletions(
    app: &tauri::AppHandle,
    root: &Path,
    paths: impl IntoIterator<Item = String>,
) {
    let root = root.display().to_string();
    let now_ms = metric_now_ms();
    let paths = paths.into_iter().collect::<Vec<_>>();
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.leaf_deleted_paths(&root, &paths, now_ms)
            })
        });
}

/// Summary is available even after collection is disabled: disabling stops writes, not history.
#[tauri::command]
#[specta::specta]
pub(crate) fn local_knowledge_metrics_summary(app: tauri::AppHandle) -> LocalKnowledgeMetricsDto {
    reconcile_local_knowledge_metrics(&app, false);
    let Some(root) = app
        .try_state::<AppState>()
        .and_then(|state| state.vault_root())
        .map(|path| path.display().to_string())
    else {
        return empty_knowledge_metrics(KnowledgeMetricsStatusDto::VaultUnavailable);
    };
    let Some(metrics) = app.try_state::<crate::metrics::LocalKnowledgeMetrics>() else {
        return empty_knowledge_metrics(KnowledgeMetricsStatusDto::DatabaseUnavailable);
    };
    let now_ms = metric_now_ms();
    match metrics.summary(&root, now_ms) {
        Ok(summary) => LocalKnowledgeMetricsDto {
            status: KnowledgeMetricsStatusDto::Ready,
            range_start_day: summary.range_start_day.map(metric_day_label),
            range_end_day: summary.range_end_day.map(metric_day_label),
            suggestion_accepted: u32::try_from(summary.suggestions.accepted).unwrap_or(u32::MAX),
            suggestion_shown: u32::try_from(summary.suggestions.shown).unwrap_or(u32::MAX),
            suggestion_dismissed: u32::try_from(summary.suggestions.dismissed).unwrap_or(u32::MAX),
            suggestion_rolled_back: u32::try_from(summary.suggestions.rolled_back)
                .unwrap_or(u32::MAX),
            suggestion_percentage_basis_points: percentage_basis_points(
                summary.suggestions.accepted,
                summary.suggestions.shown,
            ),
            topic_completed: u32::try_from(summary.topic_time.completed).unwrap_or(u32::MAX),
            topic_censored: u32::try_from(summary.topic_time.censored).unwrap_or(u32::MAX),
            topic_open: u32::try_from(summary.topic_time.open).unwrap_or(u32::MAX),
            topic_median_minutes: summary
                .topic_time
                .median_ms
                .map(|value| u32::try_from(value.max(0) / 60_000).unwrap_or(u32::MAX)),
            linked_automatic: u32::try_from(summary.linked_within_24h.automatic)
                .unwrap_or(u32::MAX),
            linked_confirmed: u32::try_from(summary.linked_within_24h.confirmed)
                .unwrap_or(u32::MAX),
            linked_manual: u32::try_from(summary.linked_within_24h.manual).unwrap_or(u32::MAX),
            linked_within_24h: link_ratio_dto(summary.linked_within_24h),
            citation_attempts: u32::try_from(summary.citations.attempts).unwrap_or(u32::MAX),
            citation_succeeded: u32::try_from(summary.citations.succeeded).unwrap_or(u32::MAX),
            citation_stale_anchor: u32::try_from(summary.citations.stale_anchor)
                .unwrap_or(u32::MAX),
            citation_source_missing: u32::try_from(summary.citations.source_missing)
                .unwrap_or(u32::MAX),
            model_supplement: ratio_dto(summary.model_supplement),
            trend: metrics
                .daily(&root, now_ms, 30)
                .unwrap_or_default()
                .into_iter()
                .filter_map(metric_trend_dto)
                .collect(),
        },
        Err(status) => empty_knowledge_metrics(status.into()),
    }
}

fn metric_trend_dto(row: luna_metrics::DailyRollup) -> Option<KnowledgeMetricTrendDto> {
    let metric = match row.metric.as_str() {
        "suggestions" => KnowledgeMetricKindDto::Suggestions,
        "topic_time" => KnowledgeMetricKindDto::TopicTime,
        "linked_24h" => KnowledgeMetricKindDto::LinkedWithin24Hours,
        "citations" => KnowledgeMetricKindDto::Citations,
        "model_supplement" => KnowledgeMetricKindDto::ModelSupplement,
        _ => return None,
    };
    Some(KnowledgeMetricTrendDto {
        day: metric_day_label(row.day),
        metric,
        numerator: u32::try_from(row.numerator).unwrap_or(u32::MAX),
        denominator: u32::try_from(row.denominator).unwrap_or(u32::MAX),
        censored: u32::try_from(row.censored).unwrap_or(u32::MAX),
        percentage_basis_points: percentage_basis_points(row.numerator, row.denominator),
    })
}

fn ratio_dto(summary: luna_metrics::RatioSummary) -> MetricRatioDto {
    ratio_dto_values(
        summary.numerator,
        summary.denominator,
        summary.censored,
        summary.open,
    )
}

fn link_ratio_dto(summary: luna_metrics::LinkSummary) -> MetricRatioDto {
    ratio_dto_values(
        summary.numerator,
        summary.denominator,
        summary.censored,
        summary.open,
    )
}

fn ratio_dto_values(numerator: u64, denominator: u64, censored: u64, open: u64) -> MetricRatioDto {
    MetricRatioDto {
        numerator: u32::try_from(numerator).unwrap_or(u32::MAX),
        denominator: u32::try_from(denominator).unwrap_or(u32::MAX),
        censored: u32::try_from(censored).unwrap_or(u32::MAX),
        open: u32::try_from(open).unwrap_or(u32::MAX),
        percentage_basis_points: percentage_basis_points(numerator, denominator),
    }
}

fn percentage_basis_points(numerator: u64, denominator: u64) -> Option<u16> {
    (denominator > 0).then(|| {
        u16::try_from(numerator.saturating_mul(10_000) / denominator)
            .unwrap_or(10_000)
            .min(10_000)
    })
}

fn metric_day_label(day: i64) -> String {
    chrono::DateTime::from_timestamp(day.saturating_mul(86_400), 0)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| day.to_string())
}

fn empty_knowledge_metrics(status: KnowledgeMetricsStatusDto) -> LocalKnowledgeMetricsDto {
    let empty = MetricRatioDto {
        numerator: 0,
        denominator: 0,
        censored: 0,
        open: 0,
        percentage_basis_points: None,
    };
    LocalKnowledgeMetricsDto {
        status,
        range_start_day: None,
        range_end_day: None,
        suggestion_accepted: 0,
        suggestion_shown: 0,
        suggestion_dismissed: 0,
        suggestion_rolled_back: 0,
        suggestion_percentage_basis_points: None,
        topic_completed: 0,
        topic_censored: 0,
        topic_open: 0,
        topic_median_minutes: None,
        linked_within_24h: empty.clone(),
        linked_automatic: 0,
        linked_confirmed: 0,
        linked_manual: 0,
        citation_attempts: 0,
        citation_succeeded: 0,
        citation_stale_anchor: 0,
        citation_source_missing: 0,
        model_supplement: empty,
        trend: Vec::new(),
    }
}

/// User-facing destructive action: it removes the database and its WAL/SHM, never a Vault asset.
#[tauri::command]
#[specta::specta]
pub(crate) async fn clear_local_knowledge_metrics(app: tauri::AppHandle) -> Result<(), String> {
    // 清除会关闭 SQLite 并删除 Keychain 凭据；两者都可能被系统授权、锁屏或钥匙串守护进程
    // 暂时阻塞。命令线程必须只等待结果，不能让确认弹窗、可访问性树和整个 WebView 冻住。
    // 与 retry 的激活器一致，生命周期互斥仍由 LocalKnowledgeMetrics 独占，不能另造清除路径。
    tauri::async_runtime::spawn_blocking(move || {
        app.try_state::<crate::metrics::LocalKnowledgeMetrics>()
            .ok_or_else(|| coded_error(&app, "METRICS_UNAVAILABLE", "metrics-unavailable"))?
            .clear()
            .map_err(|status| format!("METRICS_CLEAR_INCOMPLETE: {status:?}"))
    })
    .await
    .unwrap_or_else(|_| Err("METRICS_CLEAR_INCOMPLETE: worker unavailable".to_owned()))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn retry_local_knowledge_metrics(
    app: tauri::AppHandle,
) -> KnowledgeMetricsStatusDto {
    let app_for_retry = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(metrics) = app_for_retry.try_state::<crate::metrics::LocalKnowledgeMetrics>()
        else {
            return KnowledgeMetricsStatusDto::DatabaseUnavailable;
        };
        // 用户显式重试：当场开库并如实返回结果，不等第一次记度量。
        metrics.activate(true);
        metrics.open_deferred();
        schedule_local_knowledge_metrics_reconcile(&app_for_retry);
        metrics.availability().into()
    })
    .await
    .unwrap_or(KnowledgeMetricsStatusDto::DatabaseUnavailable)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeMetricsExportOutcomeDto {
    Exported,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeMetricsExportFormatDto {
    Json,
    Csv,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn export_local_knowledge_metrics(
    app: tauri::AppHandle,
    start_day: Option<String>,
    end_day: Option<String>,
    format: KnowledgeMetricsExportFormatDto,
) -> Result<KnowledgeMetricsExportOutcomeDto, String> {
    let root = app
        .try_state::<AppState>()
        .and_then(|state| state.vault_root())
        .ok_or_else(|| {
            coded_error(
                &app,
                "METRICS_VAULT_UNAVAILABLE",
                "metrics-vault-unavailable",
            )
        })?;
    let parse_day = |value: Option<String>| -> Result<Option<i64>, String> {
        value
            .map(|value| {
                chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                    .map(|day| {
                        day.and_hms_opt(0, 0, 0)
                            .expect("midnight is valid")
                            .and_utc()
                            .timestamp()
                            .div_euclid(86_400)
                    })
                    .map_err(|_| coded_error(&app, "METRICS_RANGE_INVALID", "metrics-range-format"))
            })
            .transpose()
    };
    let start_day = parse_day(start_day)?;
    let end_day = parse_day(end_day)?;
    if start_day
        .zip(end_day)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(coded_error(
            &app,
            "METRICS_RANGE_INVALID",
            "metrics-range-start-after-end",
        ));
    }
    let extension = match format {
        KnowledgeMetricsExportFormatDto::Json => "json",
        KnowledgeMetricsExportFormatDto::Csv => "csv",
    };
    let file_name = format!(
        "luna-knowledge-metrics-{}.{}",
        chrono::Local::now().format("%Y-%m-%d"),
        extension
    );
    // **保存对话框返回之后才取快照**，口径与工作区那条导出逐字一致（TDD-12 §2.1
    // 「导出竞态」——该契约对本设备的两个度量库统一适用）。对话框会开着几秒到几分钟，
    // 用户完全可能在这期间去把历史清掉；先取快照的写法会把一份**已经被清除的**历史照原样
    // 写到磁盘上。取快照与清除同在 `LocalKnowledgeMetrics` 的生命周期锁上排队，因此要么
    // 清除还没发生、快照有效，要么清除已经完成、导出如实报不可用。
    //
    // **范围校验仍留在对话框之前**：日期不合法是用户此刻就能改的输入错误，让他先挑完
    // 文件再告诉他"日期写错了"，等于用一个原生面板换一句本来可以立刻说的话。
    let Some(destination) = playback::pick_export_destination(&app, &file_name, extension).await?
    else {
        return Ok(KnowledgeMetricsExportOutcomeDto::Cancelled);
    };
    // 聚合、序列化与磁盘 IO 都进 blocking worker：命令线程只等结果，不冻住 WebView
    //（口径同 `clear_local_knowledge_metrics` 与工作区那条导出）。
    tauri::async_runtime::spawn_blocking(move || {
        let serialized = app
            .try_state::<crate::metrics::LocalKnowledgeMetrics>()
            .ok_or_else(|| coded_error(&app, "METRICS_UNAVAILABLE", "metrics-unavailable"))?
            .export(
                &root.display().to_string(),
                metric_now_ms(),
                start_day,
                end_day,
                match format {
                    KnowledgeMetricsExportFormatDto::Json => {
                        crate::metrics::MetricsExportFormat::Json
                    }
                    KnowledgeMetricsExportFormatDto::Csv => {
                        crate::metrics::MetricsExportFormat::Csv
                    }
                },
            )
            .map_err(|status| format!("METRICS_EXPORT_UNAVAILABLE: {status:?}"))?;
        // **原子提交**（同 `write_atomically` 的抬头）：`fs::write` 先截断再写，中途失败
        // 留下的是一个长度对不上、格式打不开的文件，而用户手里那份"导出"看上去是存在的。
        // 临时文件与目标同目录——跨卷 rename 直接 `EXDEV` 失败，而导出目录可能在外置卷上。
        write_atomically(&destination, serialized.as_bytes())
            .map_err(|error| format!("METRICS_EXPORT_FAILED: {error}"))?;
        Ok(KnowledgeMetricsExportOutcomeDto::Exported)
    })
    .await
    .map_err(|error| format!("METRICS_EXPORT_FAILED: {error}"))?
}

impl TryFrom<SettingValue> for SettingValueDto {
    type Error = String;

    fn try_from(value: SettingValue) -> Result<Self, Self::Error> {
        match value {
            SettingValue::Bool(value) => Ok(Self::Bool(value)),
            SettingValue::Int(value) => i32::try_from(value)
                .map(Self::Int)
                .map_err(|_| format!("setting integer {value} is outside supported IPC range")),
            SettingValue::Str(value) => Ok(Self::Str(value)),
            SettingValue::StringList(value) => Ok(Self::StringList(value)),
        }
    }
}

impl From<SettingValueDto> for SettingValue {
    fn from(value: SettingValueDto) -> Self {
        match value {
            SettingValueDto::Bool(value) => Self::Bool(value),
            SettingValueDto::Int(value) => Self::Int(i64::from(value)),
            SettingValueDto::Str(value) => Self::Str(value),
            SettingValueDto::StringList(value) => Self::StringList(value),
        }
    }
}

// ── 工作区任务观测（PRD-07-070 / PRD-09-076 / TDD-12 §2.1）──────────────────────
//
// **与知识度量共处一个文件，但没有任何共用状态**：它们各有各的开关、库文件与清除
// 边界。放在一起只因为"本机聚合观测"是同一个域（§2.1 明写「指标归 `ipc/metrics.rs`」）。

/// 用户去过的界面（封闭维度，与 `WORKSPACE_IA` 的位置一一对应）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricSurfaceDto {
    Search,
    Notes,
    // 只有知识库有二级分区（ADR-087 ① / ADR-088）：观测维度与 `WORKSPACE_IA` 的位置
    // 一一对应，分区拆分不是给这条枚举松绑，而是让它继续对得上（TDD-12 §2.1）。
    KnowledgeTopics,
    KnowledgeTags,
    KnowledgeNetwork,
    QuickNotes,
    Recordings,
    Transcripts,
    Chat,
    Creator,
    Apps,
    Trash,
    Settings,
}

/// 一次创建是从哪个入口发起的。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricEntrySurfaceDto {
    AddContent,
    QuickStart,
    Sidebar,
    StatusBar,
    DeepLink,
    Shortcut,
    Tray,
}

/// 三条状态路径各自的展示面。
///
/// **与 [`WorkspaceMetricSurfaceDto`] 是两个维度**：状态面覆盖在当前上下文之上，不是
/// 一级导航的目的地；混进同一个枚举，两种含义不同的分母会并排出现在同一张表里。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricStatusSurfaceDto {
    Notifications,
    Queue,
    Usage,
}

/// 一次尝试的终态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricEntryResultDto {
    Arrived,
    Cancelled,
    Rejected,
    Failed,
}

/// 耗时桶（左闭右开，首桶从 0 起）。
///
/// **封闭枚举而不是自由 `String`**（TDD-12 §2.1 逐字给定取值）：桶名从库里一路透传成
/// 字符串时，前端只能拿它去拼 i18n key——某天桶边界一改、桶名一换，界面上出现的是一条
/// 裸 key，而没有任何东西会红。收成枚举之后，两边一起编译不过。
/// **取值逐条手写而不是 `rename_all = "snake_case"`**：serde 的 snake_case 会把
/// `Under1s` 变成 `under1s`、`From1sTo10s` 变成 `from1s_to10s`（它只在大小写交界处断词，
/// 不认识数字后面的那一刀），而 §2.1 与库里存的都是 `under_1s` / `from_1s_to_10s`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum WorkspaceMetricDurationBucketDto {
    #[serde(rename = "under_1s")]
    Under1s,
    #[serde(rename = "from_1s_to_10s")]
    From1sTo10s,
    #[serde(rename = "from_10s_to_60s")]
    From10sTo60s,
    #[serde(rename = "from_1m_to_5m")]
    From1mTo5m,
    #[serde(rename = "at_least_5m")]
    AtLeast5m,
    /// 调用方没有可信耗时（跨进程任务）——诚实留空，不猜一个。
    #[serde(rename = "unknown")]
    Unknown,
}

/// 前端能上报的工作区事实——**只有这三种**。
///
/// **没有自由文本、没有 id、没有路径、没有修复深链**（PRD-09-076 / TDD-12 §2.1）：修复深链
/// 里带着分组名与键名，而对象路径就是内容本身的一半。另外两类事实（转正次数、导入终态）
/// 只由 Rust 在真正落定的那一刻自己记，前端根本没有机会上报——它看不见"索引已经提交了"。
///
/// **`deny_unknown_fields` 挂在枚举容器上，一次盖住三个变体**（§2.1「反序列化拒绝未知
/// 字段」）。serde 的内部标签会先把 `kind` 摘掉、再把剩下的交给变体的字段访问器，因此
/// 容器级的这条属性对内部标签枚举是成立的——已用一条行为测试逐个变体钉住，而不是靠
/// "读了文档觉得应该行"。不加它的表现是：多送来的字段被**静默忽略**，于是一次前端的
/// 手误（把 `note_rel` 塞进来）在开发期一路绿，直到有人去看库里到底存了什么。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceMetricEventDto {
    /// 到过某个界面。
    SurfaceViewed {
        /// 哪个界面。
        surface: WorkspaceMetricSurfaceDto,
    },
    /// 一次创建尝试结束了。
    CreateEntryFinished {
        /// 五个稳定 action id 之一。
        action: create_actions::CreateActionIdDto,
        /// 从哪个入口发起。
        entry: WorkspaceMetricEntrySurfaceDto,
        /// 终态。
        result: WorkspaceMetricEntryResultDto,
        /// 耗时毫秒。
        ///
        /// **必填**：前端是在同一个 tick 里拿两个时间点相减，"没有可信耗时"这回事
        /// 只属于后端跨进程任务（那边记 `unknown` 桶）。留成 `Option` 等于给 UI 一个
        /// 它没有资格表达的说法。
        duration_ms: u32,
    },
    /// 一次状态面到访结束了。
    ///
    /// 三个状态面各只有**唯一宿主**在报（§2.1）：两个展示面各报一次的话，同一次到访会被
    /// 记成两次，而这条指标的全部用途就是"用户到底去不去、去了有没有用起来"。
    StatusVisitFinished {
        /// 哪个状态面。
        surface: WorkspaceMetricStatusSurfaceDto,
        /// 这一次到访里用户**真的操作过**面板内容（不是只打开又关掉）。
        interacted: bool,
        /// 从打开算到关闭的毫秒数。
        duration_ms: u32,
    },
}

/// 查询窗口（UTC `YYYY-MM-DD` 闭区间）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspaceMetricsRangeDto {
    /// 起始日（含）。
    pub start_day: String,
    /// 结束日（含）。
    pub end_day: String,
}

/// 比率：分子分母恒可见，样本不足时 `value` 为 `null`。
///
/// **不下发一个光秃秃的百分数**：3/5 与 300/500 在界面上都是 60%，而它们的可信度差
/// 两个量级。把分子分母一起给出去，界面才有可能诚实地呈现。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub struct WorkspaceMetricRatioDto {
    /// 分子。
    pub numerator: u32,
    /// 分母。
    pub denominator: u32,
    /// 比值；样本不足门槛时为 `null`。
    pub value: Option<f64>,
}

/// 一个耗时桶的次数。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub struct WorkspaceDurationCountDto {
    /// 桶（左闭右开，首桶从 0 起）。
    pub bucket: WorkspaceMetricDurationBucketDto,
    /// 次数。
    pub count: u32,
}

/// 聚合行（封闭变体）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceMetricAggregateDto {
    /// 某个界面被访问了几次。
    SurfaceViewed {
        /// 界面。
        surface: WorkspaceMetricSurfaceDto,
        /// 次数。
        count: u32,
    },
    /// 某个入口 × 某个动作的终态分布。
    CreateEntry {
        /// 动作 id。
        action: create_actions::CreateActionIdDto,
        /// 入口。
        entry: WorkspaceMetricEntrySurfaceDto,
        /// 到达次数。
        arrived: u32,
        /// 取消次数。
        cancelled: u32,
        /// 被拒次数。
        rejected: u32,
        /// 失败次数。
        failed: u32,
        /// 到达率。
        ratio: WorkspaceMetricRatioDto,
        /// 耗时分布。
        durations: Vec<WorkspaceDurationCountDto>,
    },
    /// 转为正式笔记的次数（**只给次数**，不除以本期新建数——那个分母跨越了本次授权区间
    /// 之外的历史，算出来的"转化率"没有任何一个诚实的解释）。
    QuickNoteSettled {
        /// 次数。
        count: u32,
    },
    /// 资料导入的终态分布。
    ///
    /// **`searchable` 不是"保存成功"**：它是内容落盘**且全文索引已提交**——真的搜得到
    /// （§2.1）。这条聚合此前一直在记、也一直在算，只是摘要组装不把它下发——数据被记录却
    /// 对用户不可读、不可导出，那是这个功能里最不该出现的一种状态。
    MaterialImport {
        /// 真的搜得到的次数。
        searchable: u32,
        /// 取消次数。
        cancelled: u32,
        /// 被拒次数。
        rejected: u32,
        /// 失败次数。
        failed: u32,
        /// 可检索终态占比。
        ratio: WorkspaceMetricRatioDto,
        /// 耗时分布（跨进程任务缺完整计时的落在 `unknown` 桶）。
        durations: Vec<WorkspaceDurationCountDto>,
    },
    /// 某个状态面的到访分布。
    StatusVisit {
        /// 状态面。
        surface: WorkspaceMetricStatusSurfaceDto,
        /// 完成的到访次数。
        total: u32,
        /// 其中 `interacted == false && duration < 10s` 的那些。
        ///
        /// **它是代理指标，不是"用户困惑的原因"**（§2.1 末条）：界面上只能说"多少次
        /// 短停留且没有操作"，不能替用户解释他当时在想什么。
        short_without_action: u32,
        /// 短停留未操作占比。
        ratio: WorkspaceMetricRatioDto,
        /// 停留时长分布。
        durations: Vec<WorkspaceDurationCountDto>,
    },
}

/// 读不到观测的原因（封闭）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricsFailureDto {
    /// 此刻打不开（IO / 锁）——重试即可。
    StorageUnavailable,
    /// schema 对不上：我们不认识这个库，也不会去动它（fail closed）。
    SchemaInvalid,
}

/// 观测存储此刻的状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceMetricsAvailabilityDto {
    /// 有数据。
    Ready,
    /// 库是空的（或还没建）。
    Empty,
    /// 读不到——**带上具体原因**：「我们不认识这个库、不会去动它」与「此刻拿不到」
    /// 给用户的行动完全不同（前者要他知道数据没被我们迁移或删掉）。
    Unavailable {
        /// 原因。
        code: WorkspaceMetricsFailureDto,
    },
}

/// 一次查询的完整结果。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LocalWorkspaceMetricsDto {
    /// 开关此刻的状态。与 `availability` **正交**：关掉之后仍可读历史。
    pub enabled: bool,
    /// 存储状态。
    pub availability: WorkspaceMetricsAvailabilityDto,
    /// 实际查询的窗口。
    pub range: WorkspaceMetricsRangeDto,
    /// 保留多少天。
    pub retention_days: u32,
    /// 比率的最小样本数（界面据此解释"为什么这里没有百分比"）。
    pub minimum_ratio_samples: u32,
    /// 聚合行。
    pub rows: Vec<WorkspaceMetricAggregateDto>,
}

/// 观测存储的状态变了（`workspace-metrics://availability`，TDD-12 §2.1 末条）。
///
/// **只发 `enabled` / `availability`，不带任何聚合值**：事件是"去重查一遍"的信号，不是
/// 第二条数据通路。把行数或计数塞进来，界面上就会出现两个可能不一致的事实源，而它们的
/// 时间窗还不一定相同。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "workspace-metrics://availability")]
pub struct WorkspaceMetricsAvailabilityChanged {
    /// 开关此刻的状态。
    pub enabled: bool,
    /// 存储此刻的状态。
    pub availability: WorkspaceMetricsAvailabilityDto,
}

/// 重查一次当前默认窗口，把 `enabled`/`availability` 广播出去。
///
/// **不在这里判断"该不该发"**：调用点（开关翻转、清除完成）都是用户刚做完一个动作的
/// 时刻，此时面板上那句话必须立刻跟上。摘要查询本身不建库（§2.1），故广播不会把一个
/// "从没打开过这个功能"的用户的磁盘上凭空变出文件。
pub(crate) fn emit_workspace_metrics_availability_changed(app: &tauri::AppHandle) {
    let Ok(summary) = local_workspace_metrics_summary(app.clone(), None) else {
        return;
    };
    let _ = (WorkspaceMetricsAvailabilityChanged {
        enabled: summary.enabled,
        availability: summary.availability,
    })
    .emit(app);
}

fn workspace_day_of(i18n: BackendCopy<'_>, text: &str) -> Result<i64, String> {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map(|day| {
            day.and_hms_opt(0, 0, 0)
                .expect("midnight is valid")
                .and_utc()
                .timestamp()
                .div_euclid(86_400)
        })
        .map_err(|_| {
            coded_error_with(
                i18n,
                "WORKSPACE_METRICS_INVALID_RANGE",
                "workspace-metrics-range-format",
            )
        })
}

fn workspace_day_text(day: i64) -> String {
    chrono::DateTime::from_timestamp(day * 86_400, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 记一条工作区事实（唯一 UI 写入口，TDD-12 §2.1）。
///
/// 关闭授权时**静默 no-op**：前端不必先问"能不能记"再决定要不要调——那会让每个观测点
/// 都多一次往返，而它们本来就该是最便宜的一行。
#[tauri::command]
#[specta::specta]
pub(crate) fn record_workspace_metric(
    app: tauri::AppHandle,
    event: WorkspaceMetricEventDto,
) -> Result<(), String> {
    let Some(metrics) = app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>() else {
        return Ok(());
    };
    let now = metric_now_ms();
    match event {
        WorkspaceMetricEventDto::SurfaceViewed { surface } => metrics.record_now(
            luna_metrics::workspace::WorkspaceEvent::SurfaceViewed {
                surface: workspace_surface(surface),
            },
            now,
        ),
        WorkspaceMetricEventDto::CreateEntryFinished {
            action,
            entry,
            result,
            duration_ms,
        } => metrics.record_now(
            luna_metrics::workspace::WorkspaceEvent::CreateEntryFinished {
                // 存的是 serde 的稳定 id 而不是 `Debug`——后者是格式不是契约。
                action: create_action_metric_id(action),
                entry: workspace_entry(entry),
                result: workspace_result(result),
                duration_ms,
            },
            now,
        ),
        WorkspaceMetricEventDto::StatusVisitFinished {
            surface,
            interacted,
            duration_ms,
        } => metrics.record_now(
            luna_metrics::workspace::WorkspaceEvent::StatusVisitFinished {
                surface: workspace_status_surface(surface),
                interacted,
                duration_ms,
            },
            now,
        ),
    }
    Ok(())
}

/// 把一次查询的结果摊成封闭的聚合行。
///
/// **五种聚合一个都不能少**：`material_import` 与 `status_visit` 此前根本没有被组装
/// 出来，于是这两类事实**被记录了却对用户不可读、不可导出**——比"没记"更糟，因为面板
/// 上那句"还没有可看的记录"是假的。
fn workspace_aggregate_rows(
    summary: &luna_metrics::workspace::WorkspaceSummary,
) -> Result<Vec<WorkspaceMetricAggregateDto>, WorkspaceMetricsFailureDto> {
    let mut rows = Vec::new();
    for surface in &summary.surfaces {
        rows.push(WorkspaceMetricAggregateDto::SurfaceViewed {
            surface: workspace_surface_dto(surface.surface),
            count: surface.count,
        });
    }
    for entry in &summary.entries {
        let action = create_actions::CreateActionIdDto::from_str(&entry.action)
            .ok_or(WorkspaceMetricsFailureDto::SchemaInvalid)?;
        rows.push(WorkspaceMetricAggregateDto::CreateEntry {
            action,
            entry: workspace_entry_dto(entry.entry),
            arrived: entry.arrived,
            cancelled: entry.cancelled,
            rejected: entry.rejected,
            failed: entry.failed,
            ratio: WorkspaceMetricRatioDto {
                numerator: entry.arrived,
                denominator: entry.total(),
                value: entry.arrival_ratio(),
            },
            durations: duration_counts(&entry.durations),
        });
    }
    if summary.quick_notes_settled > 0 {
        rows.push(WorkspaceMetricAggregateDto::QuickNoteSettled {
            count: summary.quick_notes_settled,
        });
    }
    let imports = &summary.imports;
    if imports.total() > 0 {
        rows.push(WorkspaceMetricAggregateDto::MaterialImport {
            // `arrived` 在导入这条路上的名字是 `searchable`：落盘且全文索引已提交。
            searchable: imports.arrived,
            cancelled: imports.cancelled,
            rejected: imports.rejected,
            failed: imports.failed,
            ratio: WorkspaceMetricRatioDto {
                numerator: imports.arrived,
                denominator: imports.total(),
                value: imports.arrival_ratio(),
            },
            durations: duration_counts(&imports.durations),
        });
    }
    for visit in &summary.status_visits {
        rows.push(WorkspaceMetricAggregateDto::StatusVisit {
            surface: workspace_status_surface_dto(visit.surface),
            total: visit.total,
            short_without_action: visit.short_without_action,
            ratio: WorkspaceMetricRatioDto {
                numerator: visit.short_without_action,
                denominator: visit.total,
                value: visit.short_without_action_ratio(),
            },
            durations: duration_counts(&visit.durations),
        });
    }
    Ok(rows)
}

fn duration_counts(
    durations: &[(luna_metrics::workspace::DurationBucket, u32)],
) -> Vec<WorkspaceDurationCountDto> {
    durations
        .iter()
        .map(|(bucket, count)| WorkspaceDurationCountDto {
            bucket: workspace_bucket_dto(*bucket),
            count: *count,
        })
        .collect()
}

/// 工作区观测摘要（关掉授权后仍可读历史）。
#[tauri::command]
#[specta::specta]
pub(crate) fn local_workspace_metrics_summary(
    app: tauri::AppHandle,
    range: Option<WorkspaceMetricsRangeDto>,
) -> Result<LocalWorkspaceMetricsDto, String> {
    use luna_metrics::workspace as ws;
    let metrics = app
        .try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>()
        .ok_or_else(|| {
            coded_error(
                &app,
                "WORKSPACE_METRICS_UNAVAILABLE",
                "workspace-metrics-unavailable",
            )
        })?;
    let today = metric_now_ms().div_euclid(86_400_000);
    // **省略整个 range 才用默认**，不支持只给一端：只给一端时"另一端是什么"没有一个
    // 不会让人误解的答案，而它直接决定分母。
    let copy = backend_copy_arc(&app);
    let i18n = copy.as_deref();
    let (start, end) = match &range {
        Some(range) => (
            workspace_day_of(i18n, &range.start_day)?,
            workspace_day_of(i18n, &range.end_day)?,
        ),
        None => (today - ws::DEFAULT_RANGE_DAYS + 1, today),
    };
    if start > end {
        return Err(coded_error(
            &app,
            "WORKSPACE_METRICS_INVALID_RANGE",
            "workspace-metrics-range-start-after-end",
        ));
    }
    let outcome = metrics.summary(start, end)?;
    let mut rows = Vec::new();
    let availability = match &outcome {
        Err(failure) => WorkspaceMetricsAvailabilityDto::Unavailable {
            code: match failure {
                crate::workspace_metrics::StorageFailure::StorageUnavailable => {
                    WorkspaceMetricsFailureDto::StorageUnavailable
                }
                crate::workspace_metrics::StorageFailure::SchemaInvalid => {
                    WorkspaceMetricsFailureDto::SchemaInvalid
                }
            },
        },
        Ok(None) => WorkspaceMetricsAvailabilityDto::Empty,
        Ok(Some(summary)) => match workspace_aggregate_rows(summary) {
            // **组装不出来就如实说读不了，不是少下发几行**：库里出现这一代不认识的
            // 动作 id 只可能来自 schema 代际错配或人为改库，两者都该走 fail closed。静默
            // 丢行的表现是"这个入口没人用过"，而库里明明记着——一个看不出错的错。
            Err(code) => WorkspaceMetricsAvailabilityDto::Unavailable { code },
            Ok(built) => {
                rows = built;
                if rows.is_empty() {
                    WorkspaceMetricsAvailabilityDto::Empty
                } else {
                    WorkspaceMetricsAvailabilityDto::Ready
                }
            }
        },
    };
    Ok(LocalWorkspaceMetricsDto {
        enabled: metrics.enabled(),
        availability,
        range: WorkspaceMetricsRangeDto {
            start_day: workspace_day_text(start),
            end_day: workspace_day_text(end),
        },
        retention_days: u32::try_from(ws::RETENTION_DAYS).unwrap_or(u32::MAX),
        minimum_ratio_samples: ws::MINIMUM_RATIO_SAMPLES,
        rows,
    })
}

/// 导出的结局。
///
/// **不复用知识度量那个**：那边只有「导出了 / 取消了」两态，而这里必须能说「没有可导
/// 出的内容」——写一个 0 字节的"导出成功"比一句实话更难理解。为了这一个变体去改另一条
/// 通路的 DTO，会让两个功能的 wire 形状从此绑在一起。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceMetricsExportOutcomeDto {
    /// 写出去了，并如实说写了多少行。
    Exported {
        /// 行数。
        row_count: u32,
    },
    /// 用户在保存对话框里取消了。
    Cancelled,
    /// 这段窗口里没有任何聚合行——不写文件。
    Empty,
}

/// 导出格式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMetricsExportFormatDto {
    /// 结构保真：聚合行原样，判别联合不被压平。
    Json,
    /// 表格工具直接打得开：固定列，不适用的格留空。
    Csv,
}

/// CSV 的固定列（TDD-12 §2.1 逐字给定）。
///
/// **固定列 + 留空，而不是每种行一张表**：聚合行是判别联合，按变体各出一张表的 CSV
/// 打开来是三个互不相干的区块；固定列让"同一段窗口里发生了什么"能被一次读完，代价只是
/// 几个空格。
pub(crate) const WORKSPACE_CSV_HEADER: &str =
    "metric,surface,action,entry,result,bucket,count,numerator,denominator,ratio,start_day,end_day";

/// CSV 转义：只允许枚举值、数字与日期进来，故只需处理逗号与引号。
fn csv_cell(value: &str) -> String {
    if value.contains(',') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn workspace_metrics_csv(summary: &LocalWorkspaceMetricsDto) -> String {
    let mut out = String::from(WORKSPACE_CSV_HEADER);
    out.push('\n');
    let start = &summary.range.start_day;
    let end = &summary.range.end_day;
    let mut push = |cells: [String; 12]| {
        out.push_str(&cells.map(|c| csv_cell(&c)).join(","));
        out.push('\n');
    };
    let empty = String::new();
    for row in &summary.rows {
        match row {
            WorkspaceMetricAggregateDto::SurfaceViewed { surface, count } => push([
                "surface_viewed".into(),
                enum_wire(surface),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                count.to_string(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                start.clone(),
                end.clone(),
            ]),
            WorkspaceMetricAggregateDto::CreateEntry {
                action,
                entry,
                arrived,
                cancelled,
                rejected,
                failed,
                ratio,
                durations,
            } => {
                // 四个终态各占一行：把它们塞进一行意味着新增 `result` 列的取值集合，
                // 而那正是"固定列"要避免的东西。
                for (result, count) in [
                    ("arrived", arrived),
                    ("cancelled", cancelled),
                    ("rejected", rejected),
                    ("failed", failed),
                ] {
                    push([
                        "create_entry".into(),
                        empty.clone(),
                        enum_wire(action),
                        enum_wire(entry),
                        result.into(),
                        empty.clone(),
                        count.to_string(),
                        ratio.numerator.to_string(),
                        ratio.denominator.to_string(),
                        ratio.value.map(|v| format!("{v:.4}")).unwrap_or_default(),
                        start.clone(),
                        end.clone(),
                    ]);
                }
                for bucket in durations {
                    push([
                        "create_entry_duration".into(),
                        empty.clone(),
                        enum_wire(action),
                        enum_wire(entry),
                        empty.clone(),
                        enum_wire(&bucket.bucket),
                        bucket.count.to_string(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        start.clone(),
                        end.clone(),
                    ]);
                }
            }
            WorkspaceMetricAggregateDto::QuickNoteSettled { count } => push([
                "quick_note_settled".into(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                count.to_string(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                start.clone(),
                end.clone(),
            ]),
            WorkspaceMetricAggregateDto::MaterialImport {
                searchable,
                cancelled,
                rejected,
                failed,
                ratio,
                durations,
            } => {
                // 四个终态各占一行，口径同 `create_entry`。`searchable` 是导入这条路上的
                // 到达态名字（§2.1），故它是 `result` 列的合法取值之一。
                for (result, count) in [
                    ("searchable", searchable),
                    ("cancelled", cancelled),
                    ("rejected", rejected),
                    ("failed", failed),
                ] {
                    push([
                        "material_import".into(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        result.into(),
                        empty.clone(),
                        count.to_string(),
                        ratio.numerator.to_string(),
                        ratio.denominator.to_string(),
                        ratio.value.map(|v| format!("{v:.4}")).unwrap_or_default(),
                        start.clone(),
                        end.clone(),
                    ]);
                }
                for bucket in durations {
                    push([
                        "material_import_duration".into(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        enum_wire(&bucket.bucket),
                        bucket.count.to_string(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        start.clone(),
                        end.clone(),
                    ]);
                }
            }
            // **一行，不是"总数一行 + 短停留一行"**：`short_without_action` 就是这一行
            // `ratio` 的分子（组装处如此构造），再单列一行等于让同一个数在导出里出现两次，
            // 而两次之间没有任何东西保证它们一致。
            WorkspaceMetricAggregateDto::StatusVisit {
                surface,
                total,
                ratio,
                durations,
                ..
            } => {
                push([
                    "status_visit".into(),
                    enum_wire(surface),
                    empty.clone(),
                    empty.clone(),
                    empty.clone(),
                    empty.clone(),
                    total.to_string(),
                    ratio.numerator.to_string(),
                    ratio.denominator.to_string(),
                    ratio.value.map(|v| format!("{v:.4}")).unwrap_or_default(),
                    start.clone(),
                    end.clone(),
                ]);
                for bucket in durations {
                    push([
                        "status_visit_duration".into(),
                        enum_wire(surface),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        enum_wire(&bucket.bucket),
                        bucket.count.to_string(),
                        empty.clone(),
                        empty.clone(),
                        empty.clone(),
                        start.clone(),
                        end.clone(),
                    ]);
                }
            }
        }
    }
    out
}

/// 取一个枚举的 **wire 值**（serde 的 snake_case），不是 `Debug` 格式。
///
/// `Debug` 是给人看的格式，不是契约：拿它当 CSV 的取值，某天派生一变，导出的文件
/// 就悄悄换了一套词汇。
fn enum_wire<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .map(|text| text.trim_matches('"').to_owned())
        .unwrap_or_default()
}

/// 临时文件 + rename 的原子提交。
///
/// **临时文件必须与目标同目录**：跨卷 `rename` 在 POSIX 上直接 `EXDEV` 失败，而用户
/// 选的导出目录完全可能在外置卷或网络卷上——把临时文件放系统临时目录的写法在开发机上
/// 永远测不出来，只在别人的 U 盘上炸。
///
/// **写失败不留半截文件**：中途失败就把临时文件删掉，目标位置要么是完整的旧内容、
/// 要么是完整的新内容。`fs::write` 做不到这一点：它先截断再写，中途失败留下的是一个
/// 长度对不上、格式打不开的文件，而用户手里那份"导出"看上去是存在的。
fn write_atomically(destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let dir = destination.parent().unwrap_or_else(|| Path::new("."));
    let stem = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temporary = dir.join(format!(".{stem}.{}.{unique}.part", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// 导出同口径聚合（用户手势打开原生保存对话框）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_local_workspace_metrics(
    app: tauri::AppHandle,
    range: Option<WorkspaceMetricsRangeDto>,
    format: WorkspaceMetricsExportFormatDto,
) -> Result<WorkspaceMetricsExportOutcomeDto, String> {
    let extension = match format {
        WorkspaceMetricsExportFormatDto::Json => "json",
        WorkspaceMetricsExportFormatDto::Csv => "csv",
    };
    let file_name = format!(
        "luna-workspace-metrics-{}.{extension}",
        chrono::Local::now().format("%Y-%m-%d")
    );
    // **保存对话框返回之后才取快照**（TDD-12 §2.1「导出竞态」）：对话框会开着几秒到
    // 几分钟，用户完全可能在这期间去把历史清掉。先取快照的写法会把一份**已经被清除的**
    // 历史照原样写到磁盘上——他刚按过「清除」，转头就在自己选的目录里拿到了那份数据。
    // 取快照走的是摘要那条路，因而与清除同在一把管理锁上排队：要么清除还没发生、快照
    // 有效，要么清除已经完成、快照是空的。
    let Some(destination) = playback::pick_export_destination(&app, &file_name, extension).await?
    else {
        return Ok(WorkspaceMetricsExportOutcomeDto::Cancelled);
    };
    // 序列化与磁盘 IO 都进 blocking worker：命令线程只等结果，不冻住 WebView（口径同
    // `clear_local_workspace_metrics`）。
    tauri::async_runtime::spawn_blocking(move || {
        let summary = local_workspace_metrics_summary(app, range)?;
        if summary.rows.is_empty() {
            // 空数据不写文件——一个 0 字节的"导出成功"比一句"没有可导出的内容"更难理解。
            return Ok(WorkspaceMetricsExportOutcomeDto::Empty);
        }
        let serialized = match format {
            WorkspaceMetricsExportFormatDto::Json => {
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema_version": 1,
                    "range": summary.range,
                    "rows": summary.rows,
                }))
                .map_err(|error| format!("WORKSPACE_METRICS_EXPORT_FAILED: {error}"))?
            }
            WorkspaceMetricsExportFormatDto::Csv => workspace_metrics_csv(&summary),
        };
        // **原子提交成功之后才返回 exported**：先报成功再写盘，用户会在磁盘上找不到
        // 那个文件；写到一半失败也不能算数——那份文件打不开。
        write_atomically(&destination, serialized.as_bytes())
            .map_err(|error| format!("WORKSPACE_METRICS_EXPORT_FAILED: {error}"))?;
        Ok(WorkspaceMetricsExportOutcomeDto::Exported {
            row_count: u32::try_from(summary.rows.len()).unwrap_or(u32::MAX),
        })
    })
    .await
    .map_err(|error| format!("WORKSPACE_METRICS_EXPORT_FAILED: {error}"))?
}

/// 清除工作区观测（二次确认由界面承担）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn clear_local_workspace_metrics(app: tauri::AppHandle) -> Result<(), String> {
    // 删文件可能被系统短暂阻塞；命令线程只等结果，不冻住 WebView（同知识度量的清除）。
    let worker = app.clone();
    // 文案在**进闭包之前**取好：`app` 已经被 `move` 走了，而这句话本来就与线程无关。
    let unavailable = coded_error(
        &app,
        "WORKSPACE_METRICS_CLEAR_FAILED",
        "workspace-metrics-clear-failed",
    );
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        worker
            .try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>()
            .ok_or(unavailable)?
            .clear()
    })
    .await
    .map_err(|error| format!("WORKSPACE_METRICS_CLEAR_FAILED: {error}"))?;
    // **成功与失败都广播一次**：清除失败时存储状态同样可能已经变了（比如它本来就是
    // `schema_invalid`），而面板上那句"必须先清除"必须跟着此刻的事实走，不能停在按下按钮
    // 之前的那一帧。
    emit_workspace_metrics_availability_changed(&app);
    outcome
}

fn create_action_metric_id(id: create_actions::CreateActionIdDto) -> &'static str {
    match id {
        create_actions::CreateActionIdDto::TextQuickNote => "text_quick_note",
        create_actions::CreateActionIdDto::VoiceQuickNote => "voice_quick_note",
        create_actions::CreateActionIdDto::NewNote => "new_note",
        create_actions::CreateActionIdDto::StartRecording => "start_recording",
        create_actions::CreateActionIdDto::ImportMaterial => "import_material",
    }
}

fn workspace_surface(
    surface: WorkspaceMetricSurfaceDto,
) -> luna_metrics::workspace::WorkspaceSurface {
    use luna_metrics::workspace::WorkspaceSurface as S;
    match surface {
        WorkspaceMetricSurfaceDto::Search => S::Search,
        WorkspaceMetricSurfaceDto::Notes => S::Notes,
        WorkspaceMetricSurfaceDto::KnowledgeTopics => S::KnowledgeTopics,
        WorkspaceMetricSurfaceDto::KnowledgeTags => S::KnowledgeTags,
        WorkspaceMetricSurfaceDto::KnowledgeNetwork => S::KnowledgeNetwork,
        WorkspaceMetricSurfaceDto::QuickNotes => S::QuickNotes,
        WorkspaceMetricSurfaceDto::Recordings => S::Recordings,
        WorkspaceMetricSurfaceDto::Transcripts => S::Transcripts,
        WorkspaceMetricSurfaceDto::Chat => S::Chat,
        WorkspaceMetricSurfaceDto::Creator => S::Creator,
        WorkspaceMetricSurfaceDto::Apps => S::Apps,
        WorkspaceMetricSurfaceDto::Trash => S::Trash,
        WorkspaceMetricSurfaceDto::Settings => S::Settings,
    }
}

fn workspace_surface_dto(
    surface: luna_metrics::workspace::WorkspaceSurface,
) -> WorkspaceMetricSurfaceDto {
    use luna_metrics::workspace::WorkspaceSurface as S;
    match surface {
        S::Search => WorkspaceMetricSurfaceDto::Search,
        S::Notes => WorkspaceMetricSurfaceDto::Notes,
        S::KnowledgeTopics => WorkspaceMetricSurfaceDto::KnowledgeTopics,
        S::KnowledgeTags => WorkspaceMetricSurfaceDto::KnowledgeTags,
        S::KnowledgeNetwork => WorkspaceMetricSurfaceDto::KnowledgeNetwork,
        S::QuickNotes => WorkspaceMetricSurfaceDto::QuickNotes,
        S::Recordings => WorkspaceMetricSurfaceDto::Recordings,
        S::Transcripts => WorkspaceMetricSurfaceDto::Transcripts,
        S::Chat => WorkspaceMetricSurfaceDto::Chat,
        S::Creator => WorkspaceMetricSurfaceDto::Creator,
        S::Apps => WorkspaceMetricSurfaceDto::Apps,
        S::Trash => WorkspaceMetricSurfaceDto::Trash,
        S::Settings => WorkspaceMetricSurfaceDto::Settings,
    }
}

fn workspace_entry(entry: WorkspaceMetricEntrySurfaceDto) -> luna_metrics::workspace::EntrySurface {
    use luna_metrics::workspace::EntrySurface as E;
    match entry {
        WorkspaceMetricEntrySurfaceDto::AddContent => E::AddContent,
        WorkspaceMetricEntrySurfaceDto::QuickStart => E::QuickStart,
        WorkspaceMetricEntrySurfaceDto::Sidebar => E::Sidebar,
        WorkspaceMetricEntrySurfaceDto::StatusBar => E::StatusBar,
        WorkspaceMetricEntrySurfaceDto::DeepLink => E::DeepLink,
        WorkspaceMetricEntrySurfaceDto::Shortcut => E::Shortcut,
        WorkspaceMetricEntrySurfaceDto::Tray => E::Tray,
    }
}

fn workspace_entry_dto(
    entry: luna_metrics::workspace::EntrySurface,
) -> WorkspaceMetricEntrySurfaceDto {
    use luna_metrics::workspace::EntrySurface as E;
    match entry {
        E::AddContent => WorkspaceMetricEntrySurfaceDto::AddContent,
        E::QuickStart => WorkspaceMetricEntrySurfaceDto::QuickStart,
        E::Sidebar => WorkspaceMetricEntrySurfaceDto::Sidebar,
        E::StatusBar => WorkspaceMetricEntrySurfaceDto::StatusBar,
        E::DeepLink => WorkspaceMetricEntrySurfaceDto::DeepLink,
        E::Shortcut => WorkspaceMetricEntrySurfaceDto::Shortcut,
        E::Tray => WorkspaceMetricEntrySurfaceDto::Tray,
    }
}

fn workspace_status_surface(
    surface: WorkspaceMetricStatusSurfaceDto,
) -> luna_metrics::workspace::StatusSurface {
    use luna_metrics::workspace::StatusSurface as S;
    match surface {
        WorkspaceMetricStatusSurfaceDto::Notifications => S::Notifications,
        WorkspaceMetricStatusSurfaceDto::Queue => S::Queue,
        WorkspaceMetricStatusSurfaceDto::Usage => S::Usage,
    }
}

fn workspace_status_surface_dto(
    surface: luna_metrics::workspace::StatusSurface,
) -> WorkspaceMetricStatusSurfaceDto {
    use luna_metrics::workspace::StatusSurface as S;
    match surface {
        S::Notifications => WorkspaceMetricStatusSurfaceDto::Notifications,
        S::Queue => WorkspaceMetricStatusSurfaceDto::Queue,
        S::Usage => WorkspaceMetricStatusSurfaceDto::Usage,
    }
}

/// **穷尽映射，不是把 `as_str()` 的结果原样透传**：透传等于让库里的字符串直接成为
/// wire 契约，某天桶名一改，前端拿到的是一个它不认识的取值而编译期毫无察觉。
fn workspace_bucket_dto(
    bucket: luna_metrics::workspace::DurationBucket,
) -> WorkspaceMetricDurationBucketDto {
    use luna_metrics::workspace::DurationBucket as B;
    match bucket {
        B::Under1s => WorkspaceMetricDurationBucketDto::Under1s,
        B::From1sTo10s => WorkspaceMetricDurationBucketDto::From1sTo10s,
        B::From10sTo60s => WorkspaceMetricDurationBucketDto::From10sTo60s,
        B::From1mTo5m => WorkspaceMetricDurationBucketDto::From1mTo5m,
        B::AtLeast5m => WorkspaceMetricDurationBucketDto::AtLeast5m,
        B::Unknown => WorkspaceMetricDurationBucketDto::Unknown,
    }
}

fn workspace_result(result: WorkspaceMetricEntryResultDto) -> luna_metrics::workspace::EntryResult {
    use luna_metrics::workspace::EntryResult as R;
    match result {
        WorkspaceMetricEntryResultDto::Arrived => R::Arrived,
        WorkspaceMetricEntryResultDto::Cancelled => R::Cancelled,
        WorkspaceMetricEntryResultDto::Rejected => R::Rejected,
        WorkspaceMetricEntryResultDto::Failed => R::Failed,
    }
}

#[cfg(test)]
mod workspace_metrics_ipc_tests {
    use super::*;

    /// **同一条 Vault 变更边界上的两个派生刷新必须同形**：知识地图刷新一直是去抖的，
    /// 度量基线重算却是"来一条跑一趟"，而它做的是抢 Vault 锁 + 全库遍历 + 写 sqlite。
    /// 一次批量移动逐条广播时，这条不去抖的路会把 tokio 工作线程占满并全部堵在锁上——
    /// 界面此刻发出的任何命令都排不进去（2026-09-02 真机「拖完之后应用不动」）。
    ///
    /// 判据只能读源码：去抖与否运行期不报错，只在真机上表现为"某次操作莫名卡住"。
    #[test]
    fn metrics_reconcile_is_debounced_like_the_knowledge_map_refresh() {
        let code = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = code
            .split("pub(crate) fn schedule_local_knowledge_metrics_reconcile(")
            .nth(1)
            .expect("找不到 schedule_local_knowledge_metrics_reconcile");
        let body = body.split("\npub(crate) ").next().unwrap_or(body);
        assert!(
            body.contains("METRICS_RECONCILE_REVISION"),
            "去抖计次没了：同一阵变更会各跑一遍全库遍历"
        );
        assert!(
            body.contains("tokio::time::sleep"),
            "没有合并窗口就不叫去抖——`yield_now` 只是让出一次调度"
        );
    }

    /// 前端能上报的事实里**不得出现任何可关联字段**（PRD-09-076 / TDD-12 §2.1）。
    ///
    /// 读源码而不是跑命令：它要真 `AppHandle`。而这里要拦的恰好是源码层面的加字段——
    /// 谁往 `WorkspaceMetricEventDto` 里塞一个 `path` / `title` / `route`，不会有任何既有
    /// 用例变红，而那条路径会一路写进磁盘。
    #[test]
    fn the_reportable_event_carries_no_correlatable_field() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub enum WorkspaceMetricEventDto {")
            .nth(1)
            .expect("找不到事件 DTO")
            .split("\n}")
            .next()
            .expect("枚举边界变了");
        for banned in [
            "path", "title", "route", "repair", "note_rel", "query", "text", "token", "id:",
            "session",
        ] {
            assert!(
                !body.contains(banned),
                "工作区观测事件里出现了可关联字段：{banned}"
            );
        }
        // 只有三个变体：另外两类事实由 Rust 在落定那一刻自己记，前端看不见"索引已提交"。
        assert_eq!(body.matches("    SurfaceViewed {").count(), 1);
        assert_eq!(body.matches("    CreateEntryFinished {").count(), 1);
        assert_eq!(body.matches("    StatusVisitFinished {").count(), 1);
        assert!(!body.contains("MaterialImportFinished"));
        assert!(!body.contains("QuickNoteSettled"));
    }

    /// 未知字段被**结构拒绝**，不是被忽略（TDD-12 §2.1「反序列化拒绝未知字段」）。
    ///
    /// 三个变体各一条：`deny_unknown_fields` 挂在枚举容器上，而"容器级属性对内部标签枚举
    /// 到底生不生效"不是可以靠读文档决定的事——它决定了一次前端手误是当场报错还是**静默
    /// 落库一条少了字段的记录**。
    #[test]
    fn an_unknown_field_is_rejected_by_the_structure_not_quietly_ignored() {
        use super::WorkspaceMetricEventDto as E;
        for (name, good, bad) in [
            (
                "surface_viewed",
                r#"{"kind":"surface_viewed","surface":"search"}"#,
                r#"{"kind":"surface_viewed","surface":"search","note_rel":"a/b.md"}"#,
            ),
            (
                "create_entry_finished",
                r#"{"kind":"create_entry_finished","action":"new_note","entry":"sidebar","result":"arrived","duration_ms":12}"#,
                r#"{"kind":"create_entry_finished","action":"new_note","entry":"sidebar","result":"arrived","duration_ms":12,"repair":"vault://x"}"#,
            ),
            (
                "status_visit_finished",
                r#"{"kind":"status_visit_finished","surface":"queue","interacted":true,"duration_ms":900}"#,
                r#"{"kind":"status_visit_finished","surface":"queue","interacted":true,"duration_ms":900,"title":"重要"}"#,
            ),
        ] {
            serde_json::from_str::<E>(good)
                .unwrap_or_else(|error| panic!("{name} 的合法样例反而被拒：{error}"));
            let refused = serde_json::from_str::<E>(bad);
            assert!(refused.is_err(), "{name} 放过了未知字段：{refused:?}");
        }
    }

    /// **`duration_ms` 必填**：前端在同一个 tick 里算两个时间点之差，没有"不知道"这回事。
    /// `unknown` 桶属于后端跨进程任务，UI 写入口不该有资格表达它。
    #[test]
    fn the_ui_cannot_say_it_has_no_idea_how_long_something_took() {
        use super::WorkspaceMetricEventDto as E;
        assert!(serde_json::from_str::<E>(
            r#"{"kind":"create_entry_finished","action":"new_note","entry":"sidebar","result":"arrived","duration_ms":null}"#
        )
        .is_err());
        assert!(serde_json::from_str::<E>(
            r#"{"kind":"status_visit_finished","surface":"usage","interacted":false}"#
        )
        .is_err());
    }

    /// 前端**伪报后端终态**必须被拒（TDD-12 §2.1 反向验收逐字一条）。
    ///
    /// "类型层保证"不算数：类型是我们写的，而 wire 上来的是一段 JSON。这条用真的 JSON
    /// 走一遍反序列化，证明这两个 kind 在写入口上根本不存在。
    #[test]
    fn the_ui_cannot_forge_a_terminal_state_only_rust_can_observe() {
        use super::WorkspaceMetricEventDto as E;
        for forged in [
            r#"{"kind":"quick_note_settled"}"#,
            r#"{"kind":"material_import_finished","result":"searchable","duration_ms":900}"#,
        ] {
            let refused = serde_json::from_str::<E>(forged);
            assert!(refused.is_err(), "前端伪报了后端终态：{forged}");
        }
    }

    /// 耗时桶的 wire 值逐字对上 §2.1 与库里存的字符串。
    ///
    /// serde 的 `rename_all = "snake_case"` 在这里是**错的**（`Under1s` → `under1s`），
    /// 它只在大小写交界处断词。取值逐条手写，并由这条钉住。
    #[test]
    fn the_duration_buckets_use_the_names_the_contract_and_the_database_agree_on() {
        use luna_metrics::workspace::DurationBucket as B;
        for bucket in [
            B::Under1s,
            B::From1sTo10s,
            B::From10sTo60s,
            B::From1mTo5m,
            B::AtLeast5m,
            B::Unknown,
        ] {
            assert_eq!(
                super::enum_wire(&super::workspace_bucket_dto(bucket)),
                bucket.as_str(),
                "DTO 的桶名与库里存的对不上"
            );
        }
    }

    /// 摘要**只在省略整个 range 时**用默认窗口，不支持只给一端。
    #[test]
    fn a_half_open_range_is_not_a_thing() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub(crate) fn local_workspace_metrics_summary(")
            .nth(1)
            .expect("找不到摘要命令")
            .split("\n/// ")
            .next()
            .expect("命令边界变了");
        // 入参是一个整体的 `Option<Range>`——只给一端时"另一端是什么"没有一个不会让人
        // 误解的答案，而它直接决定分母。
        assert!(body.contains("range: Option<WorkspaceMetricsRangeDto>"));
        assert!(body.contains("ws::DEFAULT_RANGE_DAYS"));
        assert!(body.contains("WORKSPACE_METRICS_INVALID_RANGE"));
    }

    /// 导出：**对话框返回之后才取快照**，空数据不写文件，原子提交成功之后才返回
    /// exported（TDD-12 §2.1「导出竞态」）。
    ///
    /// 顺序只能由结构看住：这条命令要真 `AppHandle` 和一个原生保存面板，行为测试进不去；
    /// 而"先取快照还是先开对话框"从外面看恰好完全一样——直到用户在对话框开着的时候按了
    /// 「清除」，然后在自己选的目录里拿到那份刚被清掉的历史。
    #[test]
    fn export_takes_its_snapshot_after_the_dialog_and_only_claims_success_once_committed() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub(crate) async fn export_local_workspace_metrics(")
            .nth(1)
            .expect("找不到导出命令")
            .split("\n/// ")
            .next()
            .expect("命令边界变了");
        let dialog = body
            .find("pick_export_destination")
            .expect("导出必须经用户手势打开保存对话框");
        let snapshot = body
            .find("local_workspace_metrics_summary(app, range)")
            .expect("导出的是同口径聚合——它就是摘要本身，不另算一遍");
        let empty = body
            .find("WorkspaceMetricsExportOutcomeDto::Empty")
            .expect("空数据要有出口");
        let write = body.find("write_atomically(").expect("写盘必须是原子提交");
        let exported = body
            .find("WorkspaceMetricsExportOutcomeDto::Exported")
            .expect("成功要有出口");
        assert!(dialog < snapshot, "旧快照不得在清除成功之后仍被导出");
        assert!(
            snapshot < empty && empty < write,
            "空数据必须在写盘之前返回"
        );
        assert!(
            write < exported,
            "先报成功再写盘，用户会在磁盘上找不到那个文件"
        );
        // 序列化与磁盘 IO 不占命令线程（口径同清除命令）。
        assert!(body.contains("spawn_blocking"));
        assert!(!body.contains("std::fs::write("), "非原子写会留下半截文件");
    }

    /// **知识度量那条导出走同一口径**——它与工作区那条是同一个根因的孪生，不是"另一个
    /// 功能"：快照先于对话框、裸 `fs::write`、在 async 命令上下文做阻塞 IO，三条一模一样。
    ///
    /// 分成两条测试而不是一条循环，是因为两条命令的出口枚举不同（知识侧没有 `Empty`——
    /// 它的聚合导出总有内容，为对齐而给它新加一个变体会动 wire）。共用的是**顺序**，
    /// 而顺序在这里各断各的更读得懂。
    #[test]
    fn the_knowledge_export_takes_its_snapshot_after_the_dialog_and_commits_atomically() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub(crate) async fn export_local_knowledge_metrics(")
            .nth(1)
            .expect("找不到知识度量导出命令")
            .split("\n#[derive")
            .next()
            .expect("命令边界变了");
        let range_check = body
            .find("METRICS_RANGE_INVALID")
            .expect("范围校验要有出口");
        let dialog = body
            .find("pick_export_destination")
            .expect("导出必须经用户手势打开保存对话框");
        let snapshot = body.find(".export(").expect("找不到取快照那一步");
        let write = body.find("write_atomically(").expect("写盘必须是原子提交");
        let exported = body
            .find("KnowledgeMetricsExportOutcomeDto::Exported")
            .expect("成功要有出口");
        assert!(
            range_check < dialog,
            "日期不合法是用户此刻就能改的输入错误，不该让他先挑完文件才被告知"
        );
        assert!(dialog < snapshot, "旧快照不得在清除成功之后仍被导出");
        assert!(snapshot < write && write < exported, "写盘成功之后才算导出");
        assert!(body.contains("spawn_blocking"));
        assert!(!body.contains("std::fs::write("), "非原子写会留下半截文件");
    }

    /// 全文件不得再出现裸的 `std::fs::write` 导出：解药已经在场，留一处不用就是
    /// 亲手制造"同一件事两种写法"。
    #[test]
    fn no_export_in_this_file_writes_without_an_atomic_commit() {
        let source = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        assert!(
            !source.contains("std::fs::write("),
            "度量导出必须一律走 write_atomically"
        );
        assert_eq!(
            source.matches("fn write_atomically(").count(),
            1,
            "原子提交只能有一份实现"
        );
    }

    /// 原子提交：目标位置要么是完整的旧内容、要么是完整的新内容，中途失败不留半截。
    #[test]
    fn an_atomic_write_replaces_the_target_or_leaves_it_alone() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("export.json");
        std::fs::write(&destination, b"old contents").unwrap();

        super::write_atomically(&destination, b"new contents").unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new contents");
        // 临时文件不留在用户选的目录里。
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "export.json")
            .collect();
        assert!(leftovers.is_empty(), "导出目录里留下了残渣：{leftovers:?}");
    }

    #[test]
    fn a_write_that_cannot_be_created_leaves_nothing_behind() {
        // 目录不存在 ⇒ 临时文件建不出来 ⇒ 如实报错，且不在任何地方留下半截文件。
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope").join("export.json");
        assert!(super::write_atomically(&missing, b"payload").is_err());
        assert!(!missing.exists());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    /// CSV 的固定列逐字对上 §2.1，且**取值走 serde 的 wire 值而不是 `Debug`**。
    #[test]
    fn the_csv_header_is_the_one_the_contract_names() {
        assert_eq!(
            crate::ipc::metrics::WORKSPACE_CSV_HEADER,
            "metric,surface,action,entry,result,bucket,count,numerator,denominator,ratio,start_day,end_day"
        );
        // `Debug` 是给人看的格式不是契约：某天派生一变，导出的文件就悄悄换了一套词汇。
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("fn workspace_metrics_csv(")
            .nth(1)
            .expect("找不到 CSV 序列化")
            .split("\n}")
            .next()
            .expect("函数边界变了");
        assert!(!body.contains("{:?}"), "CSV 取值不得用 Debug 格式");
        assert!(body.contains("enum_wire("));
    }

    /// 记录 → 聚合 → 摘要行 → CSV / JSON：**新表的数据一路走得通**。
    ///
    /// 这条覆盖的是本批最贵的一处缺陷：`imports` 早就在记、早就在聚合，只是摘要组装
    /// 不下发它——数据被记录却对用户不可读、不可导出，而面板上那句"还没有可看的记录"是假的。
    /// 命令外壳（`local_workspace_metrics_summary`）要真 `AppHandle` 进不去，故从聚合函数
    /// 起断，但**库是真的、写进去的也是真的**，不是手搭的 DTO。
    #[test]
    fn the_new_aggregates_survive_the_whole_way_to_both_export_formats() {
        use luna_metrics::workspace as ws;

        let dir = tempfile::tempdir().unwrap();
        let store = ws::WorkspaceMetrics::open(dir.path()).unwrap();
        let day = 20 * 86_400_000_i64;
        for _ in 0..3 {
            store
                .record(
                    ws::WorkspaceEvent::MaterialImportFinished {
                        result: ws::EntryResult::Arrived,
                        duration_ms: Some(120_000),
                    },
                    day,
                )
                .unwrap();
        }
        store
            .record(
                ws::WorkspaceEvent::MaterialImportFinished {
                    result: ws::EntryResult::Failed,
                    // 跨进程任务缺完整计时 ⇒ `unknown` 桶（§2.1）。
                    duration_ms: None,
                },
                day,
            )
            .unwrap();
        for (interacted, ms) in [(false, 400_u32), (false, 800), (true, 45_000)] {
            store
                .record(
                    ws::WorkspaceEvent::StatusVisitFinished {
                        surface: ws::StatusSurface::Queue,
                        interacted,
                        duration_ms: ms,
                    },
                    day,
                )
                .unwrap();
        }

        let summary = store.summary(0, 30).unwrap();
        let rows = super::workspace_aggregate_rows(&summary).expect("聚合必须组装得出来");

        let imports = rows
            .iter()
            .find(|row| matches!(row, WorkspaceMetricAggregateDto::MaterialImport { .. }))
            .expect("导入终态必须下发给用户");
        let WorkspaceMetricAggregateDto::MaterialImport {
            searchable,
            failed,
            ratio,
            durations,
            ..
        } = imports
        else {
            unreachable!()
        };
        assert_eq!((*searchable, *failed), (3, 1));
        assert_eq!((ratio.numerator, ratio.denominator), (3, 4));
        // **小样本：分子分母仍然可见，只有 `value` 是 null**——不下发一个骗人的 75%。
        assert_eq!(ratio.value, None);
        assert!(durations
            .iter()
            .any(|it| it.bucket == WorkspaceMetricDurationBucketDto::Unknown && it.count == 1));

        let visit = rows
            .iter()
            .find(|row| matches!(row, WorkspaceMetricAggregateDto::StatusVisit { .. }))
            .expect("状态面到访必须下发给用户");
        let WorkspaceMetricAggregateDto::StatusVisit {
            surface,
            total,
            short_without_action,
            ratio,
            ..
        } = visit
        else {
            unreachable!()
        };
        assert!(matches!(surface, WorkspaceMetricStatusSurfaceDto::Queue));
        assert_eq!((*total, *short_without_action), (3, 2));
        assert_eq!((ratio.numerator, ratio.denominator), (2, 3));
        assert_eq!(ratio.value, None, "样本不足门槛不给比值");

        let dto = LocalWorkspaceMetricsDto {
            enabled: true,
            availability: WorkspaceMetricsAvailabilityDto::Ready,
            range: WorkspaceMetricsRangeDto {
                start_day: "2026-01-01".to_owned(),
                end_day: "2026-01-31".to_owned(),
            },
            retention_days: 90,
            minimum_ratio_samples: ws::MINIMUM_RATIO_SAMPLES,
            rows,
        };

        // CSV：**列集合逐字不变**，新行类型只是新的 `metric` 取值。
        let csv = super::workspace_metrics_csv(&dto);
        let header_columns = WORKSPACE_CSV_HEADER.split(',').count();
        for line in csv.lines() {
            assert_eq!(line.split(',').count(), header_columns, "列数变了：{line}");
        }
        assert!(csv.contains("material_import,,,,searchable,,3,3,4,,"));
        assert!(csv.contains("material_import_duration,,,,,unknown,1,,,,"));
        assert!(csv.contains("status_visit,queue,,,,,3,2,3,,"));
        assert!(csv.contains("status_visit_duration,queue,,,,under_1s,2,,,,"));

        // JSON：判别联合原样，不被压平。
        let json = serde_json::to_string(&dto.rows).unwrap();
        assert!(json.contains(r#""kind":"material_import""#));
        assert!(json.contains(r#""kind":"status_visit""#));
        assert!(json.contains(r#""bucket":"unknown""#));
    }

    /// **零分母**：一次都没发生过时不下发这一行——而不是下发一个 `0/0` 的空比率。
    #[test]
    fn a_metric_that_never_happened_is_absent_rather_than_a_zero_over_zero() {
        use luna_metrics::workspace as ws;
        let dir = tempfile::tempdir().unwrap();
        let store = ws::WorkspaceMetrics::open(dir.path()).unwrap();
        store
            .record(
                ws::WorkspaceEvent::SurfaceViewed {
                    surface: ws::WorkspaceSurface::Trash,
                },
                0,
            )
            .unwrap();
        let rows = super::workspace_aggregate_rows(&store.summary(0, 1).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows
            .iter()
            .any(|row| matches!(row, WorkspaceMetricAggregateDto::MaterialImport { .. })));
        assert!(!rows
            .iter()
            .any(|row| matches!(row, WorkspaceMetricAggregateDto::StatusVisit { .. })));
    }

    /// 库里出现这一代不认识的动作 id 时**如实说读不了**，不是少下发几行。
    #[test]
    fn an_action_id_from_another_generation_fails_closed_instead_of_dropping_rows() {
        use luna_metrics::workspace as ws;
        let summary = ws::WorkspaceSummary {
            entries: vec![ws::EntryAggregate {
                action: "summon_a_dragon".to_owned(),
                arrived: 7,
                ..ws::EntryAggregate::default()
            }],
            ..ws::WorkspaceSummary::default()
        };
        assert!(matches!(
            super::workspace_aggregate_rows(&summary),
            Err(WorkspaceMetricsFailureDto::SchemaInvalid)
        ));
    }

    /// typed availability 事件**只发 enabled/availability**（TDD-12 §2.1 末条）。
    #[test]
    fn the_availability_event_carries_no_aggregate_value() {
        let body_src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let body = body_src
            .split("pub struct WorkspaceMetricsAvailabilityChanged {")
            .nth(1)
            .expect("找不到 availability 事件")
            .split("\n}")
            .next()
            .expect("事件边界变了");
        assert!(body.contains("pub enabled: bool"));
        assert!(body.contains("pub availability: WorkspaceMetricsAvailabilityDto"));
        // 多一个字段就是第二条数据通路：两个可能不一致的事实源，时间窗还不一定相同。
        assert_eq!(body.matches("pub ").count(), 2, "事件不得携带聚合值");
    }

    /// 四条稳定错误码齐全（调用方按前缀分支）。
    #[test]
    fn the_four_stable_codes_are_all_reachable() {
        let src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let state = crate::ipc::source_without_tests(include_str!("../workspace_metrics.rs"));
        let both = format!("{src}{state}");
        for code in [
            "WORKSPACE_METRICS_INVALID_RANGE",
            "WORKSPACE_METRICS_UNAVAILABLE",
            "WORKSPACE_METRICS_EXPORT_FAILED",
            "WORKSPACE_METRICS_CLEAR_FAILED",
        ] {
            assert!(both.contains(code), "缺少稳定错误码：{code}");
        }
    }

    /// 两套观测**不共用任何状态**（PRD-09-076 / TDD-12 §2.1）。
    #[test]
    fn the_two_metric_stores_never_touch_each_other() {
        let src = crate::ipc::source_without_tests(include_str!("metrics.rs"));
        let workspace = src
            .split("// ── 工作区任务观测")
            .nth(1)
            .expect("找不到工作区观测那一段");
        // 工作区那一段里不得出现知识度量的状态类型或它的开关。
        assert!(!workspace.contains("LocalKnowledgeMetrics"));
        assert!(!workspace.contains("privacy.local_knowledge_metrics"));
    }
}
