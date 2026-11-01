//! 应用层用量归属路由（TDD-01 §5.8）。

use luna_core::{JobOrigin, UsageRecord};
use luna_jobqueue::UsageSink;
use luna_vault::{Result as VaultResult, UsageRange, UsageStore, UsageSummary, Vault};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 用量归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageRoute {
    /// 设备级全局库。
    Device,
    /// 当前 Vault 级库。
    Vault,
}

/// 设备级 usage 库路径：与 models_root 同层。
#[must_use]
pub fn device_usage_path(app_data_dir: &Path) -> PathBuf {
    // 与模型存储根**解耦**（2026-09-06，PRD-10-014）：此前取 `models_root.parent()`，
    // 模型搬去外置卷后设备用量库会跟着跑到那块盘上——它是设备记录，不是模型资产。
    app_data_dir.join("usage-device.sqlite")
}

/// 打开设备级 usage store。
pub fn open_device_usage_store(app_data_dir: &Path) -> VaultResult<UsageStore> {
    UsageStore::open(device_usage_path(app_data_dir))
}

/// 按 JobOrigin 归属用量记录。
#[must_use]
pub fn route_for_origin(origin: JobOrigin) -> UsageRoute {
    match origin {
        // 自检是**这台设备**上的一次链路验证，与哪一个库无关（换库不该让它消失）。
        JobOrigin::PopupTranslate
        | JobOrigin::Chat
        | JobOrigin::Cli
        | JobOrigin::Screenshot
        | JobOrigin::SelfCheck => UsageRoute::Device,
        JobOrigin::FloatingMenu
        | JobOrigin::VoiceInput
        | JobOrigin::Recording
        | JobOrigin::Batch
        | JobOrigin::SecondPass
        // 关系标签生成、资料蒸馏、记录沉淀、归入主题页、语义建库与知识体检都作用于
        // Vault 内笔记，用量随库记账。
        | JobOrigin::GraphTags
        | JobOrigin::Distill
        | JobOrigin::Settle
        | JobOrigin::Ingest
        | JobOrigin::Embed
        | JobOrigin::Checkup => UsageRoute::Vault,
    }
}

/// JobQueue 注入的 usage sink。只做路由与写入，不改变 Job 执行路径。
pub struct AppUsageSink {
    device: UsageStore,
    vault: Arc<Mutex<Option<Vault>>>,
}

impl AppUsageSink {
    /// 构造。
    #[must_use]
    pub fn new(device: UsageStore, vault: Arc<Mutex<Option<Vault>>>) -> Self {
        Self { device, vault }
    }
}

/// 持久化通知历史记录。只含脱敏 key/元数据/route。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotificationHistoryRecord {
    pub id: String,
    pub class: String,
    pub severity: String,
    pub channels: Vec<String>,
    pub target: Option<String>,
    pub i18n_key: String,
    pub params: HashMap<String, String>,
    pub created_at_ms: i64,
    pub read: bool,
}

/// 当前 Unix 毫秒。
///
/// 住在这里而不是 `notifications.rs`：那边此前有一份私有实现，而**记录本身**（写入
/// 时刻、保留裁剪的截止线）属于存储层——高层模块反过来向下取时间，比下层向上取要正常。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 通知历史的保留上限（PRD-15 §13 的开放问题在此收口）。
///
/// **与用量记录分开**：两者同源不同任务——用量是审计投影、通知是提醒投影，各自清空、
/// 各自确认（清通知时顺带删掉审计记录，是把两件事混成一件）。
///
/// 条数管"短时间内爆量"（一个反复复发的高危事件一天就能写几千条），天数管"多久以前的
/// 已经没有参考价值"；两条同时生效，先到先裁。
pub const HISTORY_MAX_ROWS: u64 = 2_000;
pub const HISTORY_MAX_AGE_MS: i64 = 180 * 24 * 60 * 60 * 1_000;

/// 翻页游标 = 上一页最后一条的 `(created_at_ms, id)`。
///
/// 两个字段缺一不可：同一毫秒内可能写进多条（批量事件），只按时间翻页会漏掉或重复
/// 同毫秒的兄弟条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCursor {
    pub created_at_ms: i64,
    pub id: String,
}

/// 通知历史 store，落在 usage sqlite 的 `notification_history` 表。
#[derive(Clone)]
pub struct NotificationHistoryStore {
    path: PathBuf,
}

impl NotificationHistoryStore {
    pub fn open(path: impl Into<PathBuf>) -> VaultResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = Self { path };
        store.init()?;
        // 开库时裁一次：这台设备可能几个月没开过，攒下的旧记录不该等到下一条通知才被清。
        let _ = store.prune(HISTORY_MAX_ROWS, HISTORY_MAX_AGE_MS, now_ms());
        Ok(store)
    }

    pub fn append(&self, record: &NotificationHistoryRecord) -> VaultResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO notification_history (
                id, class, severity, channels_json, target, i18n_key, params_json, created_at_ms, read
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.class,
                record.severity,
                to_json(&record.channels)?,
                record.target,
                record.i18n_key,
                to_json(&record.params)?,
                record.created_at_ms,
                i64::from(record.read),
            ],
        )
        .map_err(map_sqlite)?;
        // 写一条、顺带裁一次：两条删除都走索引，在通知这个速率下开销可以忽略，而它换来的是
        // "这张表永远不会悄悄长到没人敢查"。
        let _ = self.prune(HISTORY_MAX_ROWS, HISTORY_MAX_AGE_MS, now_ms());
        Ok(())
    }

    /// 一页历史（时间倒序）；`after` = 上一页最后一条的游标，`None` 取第一页。
    ///
    /// **排序键与游标键必须是同一对 `(created_at_ms, id)`**：`rowid` 在两个库
    /// （设备级 / 当前 Vault）里各排各的，拿它当游标，合并投影翻页时会跳条或重复。
    pub fn list(
        &self,
        limit: usize,
        after: Option<&HistoryCursor>,
    ) -> VaultResult<Vec<NotificationHistoryRecord>> {
        const COLUMNS: &str = "SELECT id, class, severity, channels_json, target, i18n_key,
                params_json, created_at_ms, read
             FROM notification_history";
        const ORDER: &str = "ORDER BY created_at_ms DESC, id DESC LIMIT ?";
        let conn = self.conn()?;
        let rows = match after {
            None => {
                let mut stmt = conn
                    .prepare(&format!("{COLUMNS} {ORDER}1"))
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_notification)
                    .map_err(map_sqlite)?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
            }
            Some(cursor) => {
                let mut stmt = conn
                    .prepare(&format!(
                        "{COLUMNS} WHERE created_at_ms < ?1
                            OR (created_at_ms = ?1 AND id < ?2) {ORDER}3"
                    ))
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(
                        params![cursor.created_at_ms, cursor.id, limit as i64],
                        row_to_notification,
                    )
                    .map_err(map_sqlite)?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
            }
        };
        rows.map_err(map_sqlite)
    }

    /// 全部条数（页头说的是"一共有多少条"，不是"这一页装了多少条"）。
    pub fn total(&self) -> VaultResult<u64> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM notification_history", [], |row| {
            Ok(row.get::<_, i64>(0)? as u64)
        })
        .map_err(map_sqlite)
    }

    /// 保留策略：先按天数删旧，再按条数封顶（PRD-15 §13 的开放问题在此收口）。
    ///
    /// 这张表此前**没有任何上限**，随设备使用无限增长——一份永远只增不减的记录，
    /// 迟早会变成"没人敢清、也没人查得动"的东西。两条判据一起用：天数管"多久以前的
    /// 已经没有参考价值"，条数管"短时间内爆量"（一个反复复发的高危事件一天能写几千条）。
    ///
    /// 返回删掉几条。惰性执行——开库时一次，此后每次写入顺带一次（都是走索引的删除）。
    pub fn prune(&self, max_rows: u64, max_age_ms: i64, now_ms: i64) -> VaultResult<usize> {
        let conn = self.conn()?;
        let mut removed = conn
            .execute(
                "DELETE FROM notification_history WHERE created_at_ms < ?1",
                params![now_ms.saturating_sub(max_age_ms)],
            )
            .map_err(map_sqlite)?;
        removed += conn
            .execute(
                "DELETE FROM notification_history WHERE id IN (
                     SELECT id FROM notification_history
                     ORDER BY created_at_ms DESC, id DESC LIMIT -1 OFFSET ?1
                 )",
                params![max_rows as i64],
            )
            .map_err(map_sqlite)?;
        Ok(removed)
    }

    pub fn unread_count(&self) -> VaultResult<u64> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM notification_history WHERE read = 0",
            [],
            |row| Ok(row.get::<_, i64>(0)? as u64),
        )
        .map_err(map_sqlite)
    }

    pub fn mark_read(&self, id: Option<&str>) -> VaultResult<usize> {
        let conn = self.conn()?;
        let changed = if let Some(id) = id {
            conn.execute(
                "UPDATE notification_history SET read = 1 WHERE id = ?1",
                params![id],
            )
        } else {
            conn.execute("UPDATE notification_history SET read = 1", [])
        }
        .map_err(map_sqlite)?;
        Ok(changed)
    }

    pub fn clear(&self) -> VaultResult<usize> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM notification_history", [])
            .map_err(map_sqlite)
    }

    /// 只清掉**已读**的那些（`PRD-15-065` 的非核爆出口）。
    ///
    /// 此前这份历史只有「全清」一种出口：用户为了不丢掉一条还没处理的未读，只能
    /// 什么都不清，于是历史越积越长、越长越不敢清。已读＝已经看过并处理过的，清掉它
    /// 不损失任何待办信息。
    pub fn clear_read(&self) -> VaultResult<usize> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM notification_history WHERE read = 1", [])
            .map_err(map_sqlite)
    }

    fn init(&self) -> VaultResult<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notification_history (
                id TEXT PRIMARY KEY,
                class TEXT NOT NULL,
                severity TEXT NOT NULL,
                channels_json TEXT NOT NULL,
                target TEXT,
                i18n_key TEXT NOT NULL,
                params_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                read INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_notification_history_created
                ON notification_history(created_at_ms);
            CREATE INDEX IF NOT EXISTS idx_notification_history_read
                ON notification_history(read);
            -- 翻页与裁剪都按 `(created_at_ms, id)` 走，单列索引在同毫秒批量事件上帮不上忙。
            CREATE INDEX IF NOT EXISTS idx_notification_history_page
                ON notification_history(created_at_ms DESC, id DESC);",
        )
        .map_err(map_sqlite)
    }

    fn conn(&self) -> VaultResult<Connection> {
        Connection::open(&self.path).map_err(map_sqlite)
    }
}

pub fn notification_store_for_usage_store(
    usage_store: &UsageStore,
) -> VaultResult<NotificationHistoryStore> {
    NotificationHistoryStore::open(usage_store.path())
}

pub fn merged_usage_records(
    device: &UsageStore,
    vault: &Arc<Mutex<Option<Vault>>>,
    range: UsageRange,
) -> Vec<UsageRecord> {
    let mut out = device.list(range).unwrap_or_default();
    if let Ok(guard) = vault.lock() {
        if let Some(vault) = guard.as_ref() {
            out.extend(vault.usage_store().list(range).unwrap_or_default());
        }
    }
    out.sort_by_key(|record| std::cmp::Reverse(record.finished_at_ms));
    if let Some(limit) = range.limit {
        out.truncate(limit);
    }
    out
}

pub fn merged_usage_summary(
    device: &UsageStore,
    vault: &Arc<Mutex<Option<Vault>>>,
) -> UsageSummary {
    let mut summary = device.summary().unwrap_or_default();
    if let Ok(guard) = vault.lock() {
        if let Some(vault) = guard.as_ref() {
            if let Ok(vault_summary) = vault.usage_store().summary() {
                summary.records += vault_summary.records;
                summary.tokens_in += vault_summary.tokens_in;
                summary.tokens_out += vault_summary.tokens_out;
                summary.audio_seconds += vault_summary.audio_seconds;
                summary.est_cost_minor += vault_summary.est_cost_minor;
                summary.local_audio_seconds += vault_summary.local_audio_seconds;
                summary.local_tokens += vault_summary.local_tokens;
                summary.server_audio_seconds += vault_summary.server_audio_seconds;
                summary.server_tokens += vault_summary.server_tokens;
                summary.truncated += vault_summary.truncated;
            }
        }
    }
    summary
}

pub fn clear_merged_usage(device: &UsageStore, vault: &Arc<Mutex<Option<Vault>>>) -> usize {
    let mut changed = device.clear().unwrap_or_default();
    if let Ok(guard) = vault.lock() {
        if let Some(vault) = guard.as_ref() {
            changed += vault.usage_store().clear().unwrap_or_default();
        }
    }
    changed
}

fn to_json<T: Serialize>(value: &T) -> VaultResult<String> {
    serde_json::to_string(value).map_err(|e| luna_vault::VaultError::Decode(e.to_string()))
}

fn from_json<T: for<'de> Deserialize<'de>>(value: String) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn row_to_notification(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationHistoryRecord> {
    Ok(NotificationHistoryRecord {
        id: row.get(0)?,
        class: row.get(1)?,
        severity: row.get(2)?,
        channels: from_json(row.get(3)?)?,
        target: row.get(4)?,
        i18n_key: row.get(5)?,
        params: from_json(row.get(6)?)?,
        created_at_ms: row.get(7)?,
        read: row.get::<_, i64>(8)? != 0,
    })
}

fn map_sqlite(e: rusqlite::Error) -> luna_vault::VaultError {
    luna_vault::VaultError::Decode(format!("notification history sqlite: {e}"))
}

impl UsageSink for AppUsageSink {
    fn record(&self, record: UsageRecord) {
        match route_for_origin(record.origin) {
            UsageRoute::Device => {
                let _ = self.device.insert(&record);
            }
            UsageRoute::Vault => {
                if let Ok(guard) = self.vault.lock() {
                    if let Some(vault) = guard.as_ref() {
                        let _ = vault.usage_store().insert(&record);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::{Capability, Job, JobKind, Lang, ProviderId, TextRef, Timestamp, Usage};

    fn record(origin: JobOrigin) -> UsageRecord {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("private source".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            origin,
        );
        UsageRecord {
            job_id: job.id,
            origin,
            capability: Capability::Translate,
            service_class: Some(job.service_class()),
            provider_id: ProviderId::from("test"),
            provider_location: Some(luna_core::ProviderLocation::Device),
            model_id: None,
            usage: Usage {
                tokens_out: Some(1),
                ..Default::default()
            },
            terminal_status: "finished".into(),
            output_truncated: None,
            error_code: None,
            finished_at_ms: Timestamp::from_millis(1).as_millis(),
        }
    }

    #[test]
    fn origin_routing_table_matches_tdd_01() {
        assert_eq!(
            route_for_origin(JobOrigin::PopupTranslate),
            UsageRoute::Device
        );
        assert_eq!(route_for_origin(JobOrigin::Chat), UsageRoute::Device);
        assert_eq!(route_for_origin(JobOrigin::Cli), UsageRoute::Device);
        assert_eq!(route_for_origin(JobOrigin::FloatingMenu), UsageRoute::Vault);
        assert_eq!(route_for_origin(JobOrigin::VoiceInput), UsageRoute::Vault);
        assert_eq!(route_for_origin(JobOrigin::Recording), UsageRoute::Vault);
        assert_eq!(route_for_origin(JobOrigin::Batch), UsageRoute::Vault);
        assert_eq!(route_for_origin(JobOrigin::SecondPass), UsageRoute::Vault);
    }

    #[test]
    fn app_usage_sink_routes_device_and_current_vault_records() {
        let device_dir = tempfile::tempdir().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let device = UsageStore::open(device_dir.path().join("usage-device.sqlite")).unwrap();
        let vault = Arc::new(Mutex::new(Some(Vault::open(vault_dir.path()).unwrap())));
        let sink = AppUsageSink::new(device.clone(), vault.clone());

        sink.record(record(JobOrigin::PopupTranslate));
        sink.record(record(JobOrigin::FloatingMenu));

        assert_eq!(device.summary().unwrap().records, 1);
        let vault_records = vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .usage_store()
            .summary()
            .unwrap()
            .records;
        assert_eq!(vault_records, 1);
    }

    #[test]
    fn notification_history_store_persists_unread_and_redacted_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = NotificationHistoryStore::open(dir.path().join("usage-device.sqlite")).unwrap();
        let mut params = HashMap::new();
        params.insert("count".to_string(), "2".to_string());
        store
            .append(&NotificationHistoryRecord {
                id: "n1".into(),
                class: "system_health".into(),
                severity: "critical".into(),
                channels: vec!["banner".into(), "status".into()],
                target: Some("settings://privacy".into()),
                i18n_key: "notifications:strict_local_blocked".into(),
                params,
                // 用**真实时刻**：保留策略会把超过 180 天的记录当场裁掉，而 `10` 是
                // 1970 年——这条断言此前之所以过，只是因为那时还没有保留策略。
                created_at_ms: now_ms(),
                read: false,
            })
            .unwrap();

        assert_eq!(store.unread_count().unwrap(), 1);
        let history = store.list(20, None).unwrap();
        assert_eq!(history.len(), 1);
        assert!(!serde_json::to_string(&history[0])
            .unwrap()
            .contains("private"));
        store.mark_read(Some("n1")).unwrap();
        assert_eq!(store.unread_count().unwrap(), 0);
    }

    /// 保留策略两条判据各自生效，且**不碰还在保留期内的记录**。
    #[test]
    fn prune_drops_old_and_overflow_rows_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = NotificationHistoryStore::open(dir.path().join("usage-device.sqlite")).unwrap();
        let now = now_ms();
        let write = |id: &str, at: i64| {
            store
                .append(&NotificationHistoryRecord {
                    id: id.into(),
                    class: "info".into(),
                    severity: "info".into(),
                    channels: vec!["status".into()],
                    target: None,
                    i18n_key: "notifications:fallback".into(),
                    params: HashMap::new(),
                    created_at_ms: at,
                    read: true,
                })
                .unwrap();
        };

        write("新", now);
        write("将好在期内", now - HISTORY_MAX_AGE_MS / 2);
        assert_eq!(store.total().unwrap(), 2, "保留期内的记录被误裁");

        // 超期的那条在写入时就被裁掉（`append` 顺带 prune），一条都不留。
        write("太旧", now - HISTORY_MAX_AGE_MS - 1);
        assert_eq!(store.total().unwrap(), 2);

        // 条数封顶：按时间倒序留最新的 N 条，裁掉最旧的。
        assert_eq!(store.prune(1, HISTORY_MAX_AGE_MS, now).unwrap(), 1);
        let left = store.list(10, None).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "新");
    }

    /// 游标翻页：同毫秒写进的多条也必须**不重不漏**地翻过去。
    #[test]
    fn cursor_paging_walks_same_millisecond_siblings_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = NotificationHistoryStore::open(dir.path().join("usage-device.sqlite")).unwrap();
        let now = now_ms();
        for id in ["a", "b", "c", "d"] {
            store
                .append(&NotificationHistoryRecord {
                    id: id.into(),
                    class: "info".into(),
                    severity: "info".into(),
                    channels: vec![],
                    target: None,
                    i18n_key: "notifications:fallback".into(),
                    params: HashMap::new(),
                    // 四条**同一毫秒**——只按时间翻页会在这里跳条或重复。
                    created_at_ms: now,
                    read: true,
                })
                .unwrap();
        }

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<HistoryCursor> = None;
        for _ in 0..4 {
            let page = store.list(2, cursor.as_ref()).unwrap();
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|last| HistoryCursor {
                created_at_ms: last.created_at_ms,
                id: last.id.clone(),
            });
            seen.extend(page.into_iter().map(|record| record.id));
        }
        assert_eq!(seen, vec!["d", "c", "b", "a"]);
    }

    /// 用量文案所在的前端命名空间——**跟着职责走**（ADR-087 ②）。
    ///
    /// 它此前叫 `activity`，名字跟着那个把「通知」与「用量」塞进一页的容器；容器退役后
    /// 用量文案随视图搬到了 `usage` 命名空间。这条常量是本文件唯一提到该文件名的地方。
    const USAGE_LOCALE_NS: &str = "usage.json";

    /// **每个 `JobOrigin` 都必须有中英文标签**：用量视图按 origin 的 serde 串查 i18n，
    /// 漏一个的表现是那一行显示原始 key（`embed`）而不是"语义索引"——批次 2/3/4 各漏了
    /// 一个，三次都通过了 typecheck / lint / 中英 parity（两边**同样地**缺）。
    /// 判据只能是"Rust 的枚举 ↔ 前端的 JSON"，故这条守护读的是真实的语言文件。
    #[test]
    fn every_job_origin_has_a_label_in_both_locales() {
        for locale in ["zh-CN", "en-US"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../apps/desktop/src/i18n/locales")
                .join(locale)
                .join(USAGE_LOCALE_NS);
            let raw = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
            let json: serde_json::Value =
                serde_json::from_str(&raw).expect("用量文案文件是合法 JSON");
            let labels = json
                .get("origin")
                .and_then(serde_json::Value::as_object)
                .expect("origin 段在场");
            for origin in luna_core::JobOrigin::ALL {
                assert!(
                    labels.contains_key(origin.key()),
                    "{locale}/{USAGE_LOCALE_NS} 缺少 origin 标签：{}",
                    origin.key()
                );
            }
        }
    }
}
