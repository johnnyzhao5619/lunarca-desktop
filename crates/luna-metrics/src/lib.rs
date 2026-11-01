//! 本地知识闭环度量（PRD-16-040 / TDD-06 §3.8）。
//!
//! 这里不是通用埋点 SDK。它只接受五类已经完成的产品事实；所有可关联值在进入 SQLite
//! 前都经设备密钥 HMAC，数据库不保存 Vault 路径、标题、标签、正文、问题、回答、裸 token
//! 或裸 operation id。调用失败必须 fail closed，且永远不能阻断知识资产的主流程。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub mod workspace;

const SCHEMA_VERSION: i64 = 1;
const DAY_MS: i64 = 86_400_000;
const RAW_RETENTION_DAYS: i64 = 180;
const ROLLUP_RETENTION_DAYS: i64 = 730;
const OPEN_SAMPLE_MAX_MS: i64 = 365 * DAY_MS;
const LEAF_WINDOW_MS: i64 = DAY_MS;
const MAX_RAW_EVENTS: i64 = 50_000;

const EVENT_KINDS: &[&str] = &[
    "suggestion_shown",
    "suggestion_accepted",
    "suggestion_dismissed",
    "suggestion_rolled_back",
    "cluster_reached_three",
    "topic_created",
    "topic_censored",
    "knowledge_created",
    "knowledge_eligible",
    "topic_ingested",
    "knowledge_censored",
    "citation_jump_attempt",
    "citation_succeeded",
    "citation_stale_anchor",
    "citation_source_missing",
    "grounded_answer_finished",
    "model_supplement_present",
];

#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error("metrics database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("metrics file system: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown metrics schema {0}")]
    UnknownSchema(i64),
    #[error("invalid closed metrics value: {0}")]
    InvalidValue(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionEvent {
    Shown,
    Accepted,
    Dismissed,
}

impl SuggestionEvent {
    fn kind(self) -> &'static str {
        match self {
            Self::Shown => "suggestion_shown",
            Self::Accepted => "suggestion_accepted",
            Self::Dismissed => "suggestion_dismissed",
        }
    }
}

/// 一条建议是在**哪一格**被看见/接受/忽略的。
///
/// **「启动概览弹窗」与「知识库 → 概览」是两格，不是一格**（2026-08-30 分开）：它们
/// 复用同一个 `KnowledgePulse` 组件，于是曝光此前一律上报 `StartupOverview`——而那两处
/// 的性质完全不同（一个是每次运行只呈现一次的模态覆层，一个是用户主动走进来的常驻分区）。
/// 合并上报的后果是这条度量**答不出它唯一该答的问题**：用户到底是在哪儿看见这条建议、
/// 又是在哪儿动手的。复用组件是对的，共用一个 surface 常量不是。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionSurface {
    /// 每次运行只呈现一次的启动概览**弹窗**（PRD-07-050）。
    StartupOverview,
    /// 知识库工作台（「主题页」分区空选态）的常驻下一步（PRD-07-024a ② 2026-08-31
    /// 修订：原「概览」分区退役，本面随内容迁入工作台；wire 取值不变——它标识的是
    /// 「库级常驻建议位」这一个面，不是某个分区 id）。
    KnowledgeOverview,
    Graph,
    Note,
}

impl SuggestionSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::StartupOverview => "startup_overview",
            Self::KnowledgeOverview => "knowledge_overview",
            Self::Graph => "graph",
            Self::Note => "note",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CitationOutcome {
    Exact,
    Moved,
    HeadingOnly,
    Lost,
    SourceMissing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CohortCensor {
    FellBelowThreshold,
    Dismissed,
    ObservationEnded,
    Disabled,
}

impl CohortCensor {
    fn as_str(self) -> &'static str {
        match self {
            Self::FellBelowThreshold => "fell_below_threshold",
            Self::Dismissed => "dismissed",
            Self::ObservationEnded => "observation_ended",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOrigin {
    Automatic,
    Confirmed,
    Manual,
}

impl LinkOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Confirmed => "confirmed",
            Self::Manual => "manual",
        }
    }

    fn from_str(value: &str) -> Result<Self, MetricsError> {
        match value {
            "automatic" => Ok(Self::Automatic),
            "confirmed" => Ok(Self::Confirmed),
            "manual" => Ok(Self::Manual),
            _ => Err(MetricsError::InvalidValue("link origin")),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RatioSummary {
    pub numerator: u64,
    pub denominator: u64,
    pub censored: u64,
    pub open: u64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinkSummary {
    pub numerator: u64,
    pub denominator: u64,
    pub censored: u64,
    pub open: u64,
    pub automatic: u64,
    pub confirmed: u64,
    pub manual: u64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SuggestionSummary {
    pub accepted: u64,
    pub shown: u64,
    pub dismissed: u64,
    pub rolled_back: u64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimeSummary {
    pub completed: u64,
    pub censored: u64,
    pub open: u64,
    pub median_ms: Option<i64>,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CitationSummary {
    pub attempts: u64,
    pub succeeded: u64,
    pub stale_anchor: u64,
    pub source_missing: u64,
    pub exact: u64,
    pub moved: u64,
    pub heading_only: u64,
    pub lost: u64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetricsSummary {
    pub range_start_day: Option<i64>,
    pub range_end_day: Option<i64>,
    pub suggestions: SuggestionSummary,
    pub topic_time: TimeSummary,
    pub linked_within_24h: LinkSummary,
    pub citations: CitationSummary,
    pub model_supplement: RatioSummary,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DailyRollup {
    pub day: i64,
    pub metric: String,
    pub numerator: u64,
    pub denominator: u64,
    pub censored: u64,
    pub aux_value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MetricsExport {
    pub schema: &'static str,
    pub range_start_day: Option<i64>,
    pub range_end_day: Option<i64>,
    pub summary: MetricsSummary,
    pub daily: Vec<DailyRollup>,
}

struct EventFact<'a> {
    kind: &'static str,
    entity: Option<&'a [u8]>,
    operation: Option<&'a [u8]>,
    surface: Option<&'static str>,
    value_ms: Option<i64>,
    reason: Option<&'static str>,
    now_ms: i64,
    day: Option<i64>,
    cycle: i64,
}

impl EventFact<'_> {
    fn new(kind: &'static str, now_ms: i64) -> Self {
        Self {
            kind,
            entity: None,
            operation: None,
            surface: None,
            value_ms: None,
            reason: None,
            now_ms,
            day: None,
            cycle: 0,
        }
    }
}

type LeafStateRow = (Vec<u8>, Vec<u8>, i64, Vec<u8>, String);
type RollupKey = (i64, String);
type RollupValue = (u64, u64, u64, Option<f64>);
type RawRollup = (String, u64, u64, u64, Option<f64>);

pub struct RollbackWindow<'a> {
    pub suggestion_token: &'a str,
    pub operation_id: &'a str,
    pub note_path: &'a str,
    pub before_fingerprint: &'a str,
    pub after_fingerprint: &'a str,
    pub now_ms: i64,
}

/// SQLite 只在应用私有目录。`vault_root` 仅在调用栈上存在，落盘前立即 HMAC。
pub struct MetricsStore {
    conn: Connection,
    key: Vec<u8>,
    path: PathBuf,
}

impl MetricsStore {
    pub fn database_path(app_private_dir: impl AsRef<Path>) -> PathBuf {
        app_private_dir.as_ref().join("knowledge-metrics.sqlite")
    }

    pub fn database_files_exist(app_private_dir: impl AsRef<Path>) -> bool {
        let path = Self::database_path(app_private_dir);
        [path.clone(), sidecar(&path, "-wal"), sidecar(&path, "-shm")]
            .iter()
            .any(|candidate| candidate.exists())
    }

    pub fn open(app_private_dir: impl AsRef<Path>, key: Vec<u8>) -> Result<Self, MetricsError> {
        if key.is_empty() {
            return Err(MetricsError::InvalidValue("empty device key"));
        }
        let dir = app_private_dir.as_ref();
        fs::create_dir_all(dir)?;
        let path = Self::database_path(dir);
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master
             WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let tables = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let has_tables = !tables.is_empty();
        match (version, has_tables) {
            (0, false) => Self::create_schema(&conn)?,
            (SCHEMA_VERSION, true)
                if tables
                    == [
                        "knowledge_daily_rollups",
                        "knowledge_events",
                        "knowledge_metric_state",
                    ] => {}
            (other, _) => return Err(MetricsError::UnknownSchema(other)),
        }
        Ok(Self { conn, key, path })
    }

    fn create_schema(conn: &Connection) -> Result<(), MetricsError> {
        conn.execute_batch(
            "CREATE TABLE knowledge_events (
               event_id BLOB PRIMARY KEY,
               vault_token BLOB NOT NULL,
               ts_ms INTEGER NOT NULL,
               day INTEGER NOT NULL,
               kind TEXT NOT NULL CHECK(kind IN (
                 'suggestion_shown','suggestion_accepted','suggestion_dismissed','suggestion_rolled_back',
                 'cluster_reached_three','topic_created','topic_censored',
                 'knowledge_created','knowledge_eligible','topic_ingested','knowledge_censored',
                 'citation_jump_attempt','citation_succeeded','citation_stale_anchor','citation_source_missing',
                 'grounded_answer_finished','model_supplement_present')),
               entity_token BLOB,
               operation_token BLOB,
               surface TEXT CHECK(surface IS NULL OR surface IN ('startup_overview','graph','note','chat')),
               value_ms INTEGER,
               reason TEXT CHECK(reason IS NULL OR reason IN (
                 'fell_below_threshold','dismissed','observation_ended','disabled','deleted',
                 'automatic','confirmed','manual','exact','moved','heading_only','lost','source_missing'))
             );
             CREATE INDEX knowledge_events_vault_day ON knowledge_events(vault_token, day);
             CREATE INDEX knowledge_events_vault_kind_entity ON knowledge_events(vault_token, kind, entity_token);
             CREATE TABLE knowledge_metric_state (
               vault_token BLOB NOT NULL,
               state_kind TEXT NOT NULL CHECK(state_kind IN ('topic_cohort','ingest_24h','rollback_window')),
               entity_token BLOB NOT NULL,
               started_ms INTEGER NOT NULL,
               deadline_ms INTEGER,
               operation_token BLOB,
               locator_token BLOB,
               before_token BLOB,
               after_token BLOB,
               value_int INTEGER NOT NULL DEFAULT 0,
               phase TEXT NOT NULL CHECK(phase IN ('baseline_low','baseline_high','active','active_linked','disabled')),
               cycle INTEGER NOT NULL DEFAULT 0,
               PRIMARY KEY(vault_token, state_kind, entity_token)
             );
             CREATE TABLE knowledge_daily_rollups (
               vault_token BLOB NOT NULL,
               day INTEGER NOT NULL,
               metric TEXT NOT NULL,
               numerator INTEGER NOT NULL,
               denominator INTEGER NOT NULL,
               censored INTEGER NOT NULL DEFAULT 0,
               aux_value REAL,
               PRIMARY KEY(vault_token, day, metric)
             );
             PRAGMA user_version=1;",
        )?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn token(&self, namespace: &str, raw: &str) -> Vec<u8> {
        hmac_sha256(&self.key, &[namespace.as_bytes(), &[0], raw.as_bytes()]).to_vec()
    }

    fn vault(&self, root: &str) -> Vec<u8> {
        let canonical = fs::canonicalize(root)
            .unwrap_or_else(|_| PathBuf::from(root))
            .to_string_lossy()
            .into_owned();
        self.token("vault", &canonical)
    }

    fn day(now_ms: i64) -> i64 {
        now_ms.div_euclid(DAY_MS)
    }

    fn insert_event(&self, vault: &[u8], fact: EventFact<'_>) -> Result<(), MetricsError> {
        if !EVENT_KINDS.contains(&fact.kind) {
            return Err(MetricsError::InvalidValue("event kind"));
        }
        let cycle_bytes = fact.cycle.to_le_bytes();
        let event_id = hmac_sha256(
            &self.key,
            &[
                b"event",
                &[0],
                vault,
                &[0],
                fact.kind.as_bytes(),
                &[0],
                fact.entity.unwrap_or_default(),
                &[0],
                fact.operation.unwrap_or_default(),
                &[0],
                &cycle_bytes,
            ],
        );
        self.conn.execute(
            "INSERT OR IGNORE INTO knowledge_events
             (event_id,vault_token,ts_ms,day,kind,entity_token,operation_token,surface,value_ms,reason)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                event_id.as_slice(),
                vault,
                fact.now_ms,
                fact.day.unwrap_or_else(|| Self::day(fact.now_ms)),
                fact.kind,
                fact.entity,
                fact.operation,
                fact.surface,
                fact.value_ms,
                fact.reason
            ],
        )?;
        Ok(())
    }

    /// 同一稳定建议跨表面/重启只形成一个实体；surface 仅用于封闭诊断，不影响漏斗去重。
    pub fn suggestion<S: Into<Option<SuggestionSurface>>>(
        &self,
        vault_root: &str,
        suggestion_token: &str,
        event: SuggestionEvent,
        surface: S,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("suggestion", suggestion_token);
        let surface = surface.into();
        let shown_day = if matches!(
            event,
            SuggestionEvent::Accepted | SuggestionEvent::Dismissed
        ) {
            self.conn
                .query_row(
                    "SELECT day FROM knowledge_events
                 WHERE vault_token=?1 AND entity_token=?2 AND kind='suggestion_shown'",
                    params![vault, entity],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            None
        };
        if matches!(
            event,
            SuggestionEvent::Accepted | SuggestionEvent::Dismissed
        ) && shown_day.is_none()
        {
            return self.maintain(now_ms);
        }
        self.insert_event(
            &vault,
            EventFact {
                entity: Some(&entity),
                surface: surface.map(SuggestionSurface::as_str),
                day: shown_day,
                ..EventFact::new(event.kind(), now_ms)
            },
        )?;
        self.maintain(now_ms)
    }

    /// 观察完整标签簇计数。首次看到 >=3 只建立 high baseline；必须真的经历 <3→>=3 才计时。
    pub fn observe_topic_clusters(
        &self,
        vault_root: &str,
        clusters: &[(String, u32)],
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let mut seen = HashSet::new();
        for (topic_src, count) in clusters {
            let entity = self.token("topic_cohort", &canonical_topic_src(topic_src));
            seen.insert(entity.clone());
            let prior: Option<(String, i64)> = self
                .conn
                .query_row(
                    "SELECT phase,cycle FROM knowledge_metric_state
                     WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",
                    params![vault, entity],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match prior {
                None => {
                    let phase = if *count >= 3 {
                        "baseline_high"
                    } else {
                        "baseline_low"
                    };
                    self.conn.execute(
                        "INSERT INTO knowledge_metric_state
                         (vault_token,state_kind,entity_token,started_ms,value_int,phase)
                         VALUES(?1,'topic_cohort',?2,?3,?4,?5)",
                        params![vault, entity, now_ms, i64::from(*count), phase],
                    )?;
                }
                Some((phase, cycle)) if phase == "baseline_low" && *count >= 3 => {
                    let next_cycle = cycle + 1;
                    self.conn.execute(
                        "UPDATE knowledge_metric_state SET started_ms=?3,deadline_ms=?4,value_int=?5,
                         phase='active',cycle=?6 WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",
                        params![vault, entity, now_ms, now_ms + OPEN_SAMPLE_MAX_MS, i64::from(*count), next_cycle],
                    )?;
                    self.insert_event(
                        &vault,
                        EventFact {
                            entity: Some(&entity),
                            cycle: next_cycle,
                            ..EventFact::new("cluster_reached_three", now_ms)
                        },
                    )?;
                }
                Some((phase, cycle)) if phase == "active" && *count < 3 => {
                    self.finish_topic_censored(
                        &vault,
                        &entity,
                        CohortCensor::FellBelowThreshold,
                        now_ms,
                        cycle,
                    )?;
                    self.set_topic_baseline(&vault, &entity, now_ms, *count, "baseline_low")?;
                }
                Some((phase, _)) if phase == "disabled" => {
                    let next = if *count >= 3 {
                        "baseline_high"
                    } else {
                        "baseline_low"
                    };
                    self.set_topic_baseline(&vault, &entity, now_ms, *count, next)?;
                }
                Some((phase, _)) => {
                    let next = if phase == "baseline_high" && *count < 3 {
                        "baseline_low"
                    } else if phase == "baseline_low" && *count >= 3 {
                        unreachable!("crossing handled above")
                    } else {
                        phase.as_str()
                    };
                    self.conn.execute(
                        "UPDATE knowledge_metric_state SET value_int=?3,phase=?4
                         WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",
                        params![vault, entity, i64::from(*count), next],
                    )?;
                }
            }
        }
        let mut stmt = self.conn.prepare(
            "SELECT entity_token,phase,cycle FROM knowledge_metric_state
             WHERE vault_token=?1 AND state_kind='topic_cohort'",
        )?;
        let existing = stmt
            .query_map([&vault], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (entity, phase, cycle) in existing {
            if !seen.contains(&entity) {
                if phase == "active" {
                    self.finish_topic_censored(
                        &vault,
                        &entity,
                        CohortCensor::FellBelowThreshold,
                        now_ms,
                        cycle,
                    )?;
                }
                self.set_topic_baseline(&vault, &entity, now_ms, 0, "baseline_low")?;
            }
        }
        self.maintain(now_ms)
    }

    fn set_topic_baseline(
        &self,
        vault: &[u8],
        entity: &[u8],
        now_ms: i64,
        count: u32,
        phase: &'static str,
    ) -> Result<(), MetricsError> {
        self.conn.execute(
            "UPDATE knowledge_metric_state SET started_ms=?3,deadline_ms=NULL,value_int=?4,phase=?5
             WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",
            params![vault, entity, now_ms, i64::from(count), phase],
        )?;
        Ok(())
    }

    fn finish_topic_censored(
        &self,
        vault: &[u8],
        entity: &[u8],
        why: CohortCensor,
        now_ms: i64,
        cycle: i64,
    ) -> Result<(), MetricsError> {
        let started: i64 = self.conn.query_row(
            "SELECT started_ms FROM knowledge_metric_state
             WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",
            params![vault, entity],
            |row| row.get(0),
        )?;
        self.insert_event(
            vault,
            EventFact {
                entity: Some(entity),
                value_ms: Some(now_ms.saturating_sub(started)),
                reason: Some(why.as_str()),
                cycle,
                ..EventFact::new("topic_censored", now_ms)
            },
        )
    }

    pub fn topic_created(
        &self,
        vault_root: &str,
        topic_src: &str,
        operation_id: &str,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("topic_cohort", &canonical_topic_src(topic_src));
        let operation = self.token("operation", operation_id);
        let active: Option<(i64, i64, i64)> = self.conn.query_row(
            "SELECT started_ms,cycle,value_int FROM knowledge_metric_state
             WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2 AND phase='active'",
            params![vault, entity],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        if let Some((started, cycle, count)) = active {
            let transaction = self.conn.unchecked_transaction()?;
            self.insert_event(
                &vault,
                EventFact {
                    entity: Some(&entity),
                    operation: Some(&operation),
                    value_ms: Some(now_ms.saturating_sub(started)),
                    cycle,
                    ..EventFact::new("topic_created", now_ms)
                },
            )?;
            self.set_topic_baseline(
                &vault,
                &entity,
                now_ms,
                u32::try_from(count).unwrap_or(u32::MAX),
                "baseline_high",
            )?;
            transaction.commit()?;
        }
        self.maintain(now_ms)
    }

    pub fn topic_censored(
        &self,
        vault_root: &str,
        topic_src: &str,
        why: CohortCensor,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("topic_cohort", &canonical_topic_src(topic_src));
        let active: Option<(i64, i64)> = self.conn.query_row(
            "SELECT cycle,value_int FROM knowledge_metric_state
             WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2 AND phase='active'",
            params![vault, entity],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        if let Some((cycle, count)) = active {
            self.finish_topic_censored(&vault, &entity, why, now_ms, cycle)?;
            self.set_topic_baseline(
                &vault,
                &entity,
                now_ms,
                u32::try_from(count).unwrap_or(u32::MAX),
                if count >= 3 {
                    "baseline_high"
                } else {
                    "baseline_low"
                },
            )?;
        }
        Ok(())
    }

    /// 仅成功创建的新叶子知识操作入组；operation/path 都只以 HMAC 形态落盘。
    pub fn leaf_created(
        &self,
        vault_root: &str,
        operation_id: &str,
        note_path: &str,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("leaf_operation", operation_id);
        let operation = self.token("operation", operation_id);
        let locator = self.token("leaf_path", note_path);
        self.conn.execute(
            "INSERT OR IGNORE INTO knowledge_metric_state
             (vault_token,state_kind,entity_token,started_ms,deadline_ms,operation_token,locator_token,phase)
             VALUES(?1,'ingest_24h',?2,?3,?4,?5,?6,'active')",
            params![vault,entity,now_ms,now_ms + LEAF_WINDOW_MS,operation,locator],
        )?;
        self.insert_event(
            &vault,
            EventFact {
                entity: Some(&entity),
                operation: Some(&operation),
                ..EventFact::new("knowledge_created", now_ms)
            },
        )?;
        self.maintain(now_ms)
    }

    pub fn leaf_moved(&self, vault_root: &str, from: &str, to: &str) -> Result<(), MetricsError> {
        self.conn.execute(
            "UPDATE knowledge_metric_state SET locator_token=?3
             WHERE vault_token=?1 AND state_kind='ingest_24h' AND locator_token=?2",
            params![
                self.vault(vault_root),
                self.token("leaf_path", from),
                self.token("leaf_path", to)
            ],
        )?;
        Ok(())
    }

    pub fn leaf_linked_by_path(
        &self,
        vault_root: &str,
        note_path: &str,
        origin: LinkOrigin,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let locator = self.token("leaf_path", note_path);
        let rows = self.open_leaf_rows(&vault)?;
        for (entity, _locator, deadline, operation, phase) in rows {
            if _locator == locator && now_ms <= deadline && phase == "active" {
                self.insert_event(
                    &vault,
                    EventFact {
                        entity: Some(&entity),
                        operation: Some(&operation),
                        reason: Some(origin.as_str()),
                        ..EventFact::new("topic_ingested", now_ms)
                    },
                )?;
                self.conn.execute(
                    "UPDATE knowledge_metric_state SET phase='active_linked'
                     WHERE vault_token=?1 AND state_kind='ingest_24h' AND entity_token=?2",
                    params![vault, entity],
                )?;
            }
        }
        self.maintain(now_ms)
    }

    /// 已知发生在 `now_ms` 的真实删除。它与启动时“只看到文件已不存在”的未知历史不同：
    /// 删除发生在截止点之后时，笔记在 24h 观察点必然仍存在，必须先进入分母再关窗。
    pub fn leaf_deleted_by_path(
        &self,
        vault_root: &str,
        note_path: &str,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        self.leaf_deleted_paths(vault_root, &[note_path.to_owned()], now_ms)
    }

    pub fn leaf_deleted_paths(
        &self,
        vault_root: &str,
        note_paths: &[String],
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let locators = note_paths
            .iter()
            .map(|path| self.token("leaf_path", path))
            .collect::<HashSet<_>>();
        let transaction = self.conn.unchecked_transaction()?;
        for (entity, row_locator, deadline, operation, phase) in self.open_leaf_rows(&vault)? {
            if !locators.contains(&row_locator) {
                continue;
            }
            let fact = if now_ms >= deadline {
                let origin = self.leaf_link_origin(&vault, &entity, &phase)?;
                EventFact {
                    entity: Some(&entity),
                    operation: Some(&operation),
                    reason: origin.map(LinkOrigin::as_str),
                    ..EventFact::new("knowledge_eligible", deadline)
                }
            } else {
                EventFact {
                    entity: Some(&entity),
                    operation: Some(&operation),
                    reason: Some("deleted"),
                    ..EventFact::new("knowledge_censored", now_ms)
                }
            };
            self.insert_event(&vault, fact)?;
            self.delete_state(&vault, "ingest_24h", &entity)?;
        }
        transaction.commit()?;
        self.maintain(now_ms)
    }

    /// `existing_paths`/`linked_paths` 来自同一份 Vault 索引快照，只在内存里 HMAC 后即丢弃。
    pub fn observe_leaf_facts(
        &self,
        vault_root: &str,
        existing_paths: &[String],
        linked_paths: &[String],
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let existing = existing_paths
            .iter()
            .map(|p| self.token("leaf_path", p))
            .collect::<HashSet<_>>();
        let linked = linked_paths
            .iter()
            .map(|p| self.token("leaf_path", p))
            .collect::<HashSet<_>>();
        for (entity, locator, deadline, operation, phase) in self.open_leaf_rows(&vault)? {
            if !existing.contains(&locator) {
                self.insert_event(
                    &vault,
                    EventFact {
                        entity: Some(&entity),
                        operation: Some(&operation),
                        reason: Some("deleted"),
                        ..EventFact::new("knowledge_censored", now_ms)
                    },
                )?;
                self.delete_state(&vault, "ingest_24h", &entity)?;
                continue;
            }
            let newly_linked = linked.contains(&locator) && now_ms <= deadline && phase == "active";
            if newly_linked {
                self.insert_event(
                    &vault,
                    EventFact {
                        entity: Some(&entity),
                        operation: Some(&operation),
                        reason: Some("manual"),
                        ..EventFact::new("topic_ingested", now_ms)
                    },
                )?;
                self.conn.execute("UPDATE knowledge_metric_state SET phase='active_linked' WHERE vault_token=?1 AND state_kind='ingest_24h' AND entity_token=?2",params![vault,entity])?;
            }
            if now_ms >= deadline {
                let origin = if newly_linked {
                    Some(LinkOrigin::Manual)
                } else {
                    self.leaf_link_origin(&vault, &entity, &phase)?
                };
                self.insert_event(
                    &vault,
                    EventFact {
                        entity: Some(&entity),
                        operation: Some(&operation),
                        reason: origin.map(LinkOrigin::as_str),
                        ..EventFact::new("knowledge_eligible", deadline)
                    },
                )?;
                self.delete_state(&vault, "ingest_24h", &entity)?;
            }
        }
        self.maintain(now_ms)
    }

    fn open_leaf_rows(&self, vault: &[u8]) -> Result<Vec<LeafStateRow>, MetricsError> {
        let mut stmt = self.conn.prepare(
            "SELECT entity_token,locator_token,deadline_ms,operation_token,phase
             FROM knowledge_metric_state WHERE vault_token=?1 AND state_kind='ingest_24h'",
        )?;
        let rows = stmt
            .query_map([vault], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 资格事实必须自带窗口内的链接结果；否则跨日的 `topic_ingested` 会先于
    /// `knowledge_eligible` 被原始事件保留策略清理，历史分子与 origin 随即失真。
    fn leaf_link_origin(
        &self,
        vault: &[u8],
        entity: &[u8],
        phase: &str,
    ) -> Result<Option<LinkOrigin>, MetricsError> {
        if phase != "active_linked" {
            return Ok(None);
        }
        let reason: Option<String> = self
            .conn
            .query_row(
                "SELECT reason FROM knowledge_events
                 WHERE vault_token=?1 AND entity_token=?2 AND kind='topic_ingested'",
                params![vault, entity],
                |row| row.get(0),
            )
            .optional()?;
        reason.as_deref().map(LinkOrigin::from_str).transpose()
    }

    fn delete_state(&self, vault: &[u8], kind: &str, entity: &[u8]) -> Result<(), MetricsError> {
        self.conn.execute("DELETE FROM knowledge_metric_state WHERE vault_token=?1 AND state_kind=?2 AND entity_token=?3",params![vault,kind,entity])?;
        Ok(())
    }

    pub fn citation(
        &self,
        vault_root: &str,
        attempt_id: &str,
        outcome: CitationOutcome,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("citation_attempt", attempt_id);
        let transaction = self.conn.unchecked_transaction()?;
        self.insert_event(
            &vault,
            EventFact {
                entity: Some(&entity),
                surface: Some("chat"),
                ..EventFact::new("citation_jump_attempt", now_ms)
            },
        )?;
        let (kind, reason) = match outcome {
            CitationOutcome::Exact => ("citation_succeeded", "exact"),
            CitationOutcome::Moved => ("citation_succeeded", "moved"),
            CitationOutcome::HeadingOnly => ("citation_succeeded", "heading_only"),
            CitationOutcome::Lost => ("citation_stale_anchor", "lost"),
            CitationOutcome::SourceMissing => ("citation_source_missing", "source_missing"),
        };
        self.insert_event(
            &vault,
            EventFact {
                entity: Some(&entity),
                surface: Some("chat"),
                reason: Some(reason),
                ..EventFact::new(kind, now_ms)
            },
        )?;
        transaction.commit()?;
        self.maintain(now_ms)
    }

    pub fn answer(
        &self,
        vault_root: &str,
        answer_id: &str,
        has_model_supplement: bool,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("grounded_answer", answer_id);
        let transaction = self.conn.unchecked_transaction()?;
        self.insert_event(
            &vault,
            EventFact {
                entity: Some(&entity),
                surface: Some("chat"),
                ..EventFact::new("grounded_answer_finished", now_ms)
            },
        )?;
        if has_model_supplement {
            self.insert_event(
                &vault,
                EventFact {
                    entity: Some(&entity),
                    surface: Some("chat"),
                    ..EventFact::new("model_supplement_present", now_ms)
                },
            )?;
        }
        transaction.commit()?;
        self.maintain(now_ms)
    }

    pub fn register_rollback(
        &self,
        vault_root: &str,
        window: RollbackWindow<'_>,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let entity = self.token("suggestion", window.suggestion_token);
        let shown_day: Option<i64> = self
            .conn
            .query_row(
                "SELECT day FROM knowledge_events
             WHERE vault_token=?1 AND entity_token=?2 AND kind='suggestion_shown'",
                params![vault, entity],
                |row| row.get(0),
            )
            .optional()?;
        let Some(shown_day) = shown_day else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO knowledge_metric_state
             (vault_token,state_kind,entity_token,started_ms,deadline_ms,operation_token,locator_token,before_token,after_token,value_int,phase)
             VALUES(?1,'rollback_window',?2,?3,?4,?5,?6,?7,?8,?9,'active')",
            params![vault,entity,window.now_ms,window.now_ms + OPEN_SAMPLE_MAX_MS,self.token("operation",window.operation_id),self.token("rollback_path",window.note_path),self.token("body",window.before_fingerprint),self.token("body",window.after_fingerprint),shown_day],
        )?;
        Ok(())
    }

    pub fn observe_restore(
        &self,
        vault_root: &str,
        note_path: &str,
        current_fingerprint: &str,
        target_fingerprint: &str,
        now_ms: i64,
    ) -> Result<(), MetricsError> {
        let vault = self.vault(vault_root);
        let locator = self.token("rollback_path", note_path);
        let current = self.token("body", current_fingerprint);
        let target = self.token("body", target_fingerprint);
        let hit: Option<(Vec<u8>, Vec<u8>, i64)> = self
            .conn
            .query_row(
                "SELECT entity_token,operation_token,value_int FROM knowledge_metric_state
             WHERE vault_token=?1 AND state_kind='rollback_window' AND locator_token=?2
               AND after_token=?3 AND before_token=?4 AND deadline_ms>=?5",
                params![vault, locator, current, target, now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((entity, operation, shown_day)) = hit {
            self.insert_event(
                &vault,
                EventFact {
                    entity: Some(&entity),
                    operation: Some(&operation),
                    day: Some(shown_day),
                    ..EventFact::new("suggestion_rolled_back", now_ms)
                },
            )?;
            self.delete_state(&vault, "rollback_window", &entity)?;
        }
        Ok(())
    }

    /// 关闭采集是 cohort 边界，不删历史，也不允许重新开启后跨越停用区间续算。
    pub fn disable(&self, now_ms: i64) -> Result<(), MetricsError> {
        let transaction = self.conn.unchecked_transaction()?;
        let mut stmt = self.conn.prepare(
            "SELECT vault_token,entity_token,cycle FROM knowledge_metric_state
             WHERE state_kind='topic_cohort' AND phase='active'",
        )?;
        let topics = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (vault, entity, cycle) in topics {
            self.finish_topic_censored(&vault, &entity, CohortCensor::Disabled, now_ms, cycle)?;
        }
        self.conn.execute("UPDATE knowledge_metric_state SET phase='disabled',deadline_ms=NULL WHERE state_kind='topic_cohort'",[])?;
        let mut stmt = self.conn.prepare("SELECT vault_token,entity_token,operation_token FROM knowledge_metric_state WHERE state_kind='ingest_24h'")?;
        let leaves = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (vault, entity, operation) in leaves {
            self.insert_event(
                &vault,
                EventFact {
                    entity: Some(&entity),
                    operation: Some(&operation),
                    reason: Some("disabled"),
                    ..EventFact::new("knowledge_censored", now_ms)
                },
            )?;
        }
        self.conn.execute("DELETE FROM knowledge_metric_state WHERE state_kind IN ('ingest_24h','rollback_window')",[])?;
        transaction.commit()?;
        self.maintain(now_ms)
    }

    pub fn summary_for_vault(
        &self,
        vault_root: &str,
        now_ms: i64,
    ) -> Result<MetricsSummary, MetricsError> {
        self.maintain(now_ms)?;
        let vault = self.vault(vault_root);
        let (start, end): (Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT MIN(day),MAX(day) FROM (
               SELECT day FROM knowledge_events WHERE vault_token=?1
               UNION ALL SELECT day FROM knowledge_daily_rollups WHERE vault_token=?1)",
            [&vault],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let count_kind = |kind: &str| -> Result<u64, MetricsError> {
            let raw: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM knowledge_events WHERE vault_token=?1 AND kind=?2",
                params![vault, kind],
                |row| row.get(0),
            )?;
            let (metric, column) = rollup_projection(kind);
            let sql = match column {
                RollupColumn::Numerator => "SELECT COALESCE(SUM(numerator),0) FROM knowledge_daily_rollups WHERE vault_token=?1 AND metric=?2",
                RollupColumn::Denominator => "SELECT COALESCE(SUM(denominator),0) FROM knowledge_daily_rollups WHERE vault_token=?1 AND metric=?2",
                RollupColumn::Censored => "SELECT COALESCE(SUM(censored),0) FROM knowledge_daily_rollups WHERE vault_token=?1 AND metric=?2",
                RollupColumn::None => "SELECT 0",
            };
            let rolled: i64 = if column == RollupColumn::None {
                self.conn.query_row(sql, [], |row| row.get(0))?
            } else {
                self.conn
                    .query_row(sql, params![vault, metric], |row| row.get(0))?
            };
            Ok((raw + rolled).max(0) as u64)
        };
        let shown = count_kind("suggestion_shown")?;
        let accepted_raw: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM knowledge_events WHERE vault_token=?1 AND kind='suggestion_accepted'",
            [&vault],|row|row.get(0))?;
        let accepted_roll: i64 = self.conn.query_row("SELECT COALESCE(SUM(numerator),0) FROM knowledge_daily_rollups WHERE vault_token=?1 AND metric='suggestions'",[&vault],|row|row.get(0))?;
        let completed = count_kind("topic_created")?;
        let topic_censored = count_kind("topic_censored")?;
        let eligible = count_kind("knowledge_eligible")?;
        let linked_raw: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM knowledge_events
             WHERE vault_token=?1 AND kind='knowledge_eligible'
               AND reason IN ('automatic','confirmed','manual')",
            [&vault],
            |row| row.get(0),
        )?;
        let linked_roll: i64 = self.conn.query_row("SELECT COALESCE(SUM(numerator),0) FROM knowledge_daily_rollups WHERE vault_token=?1 AND metric='linked_24h'",[&vault],|row|row.get(0))?;
        let linked_automatic = self.count_reason_rollup(
            &vault,
            "knowledge_eligible",
            "automatic",
            "linked_24h_automatic",
        )?;
        let linked_confirmed = self.count_reason_rollup(
            &vault,
            "knowledge_eligible",
            "confirmed",
            "linked_24h_confirmed",
        )?;
        let linked_manual =
            self.count_reason_rollup(&vault, "knowledge_eligible", "manual", "linked_24h_manual")?;
        let attempts = count_kind("citation_jump_attempt")?;
        let succeeded = count_kind("citation_succeeded")?;
        let stale = count_kind("citation_stale_anchor")?;
        let missing = count_kind("citation_source_missing")?;
        let answers = count_kind("grounded_answer_finished")?;
        let supplements = count_kind("model_supplement_present")?;
        let open_topics: i64 = self.conn.query_row("SELECT COUNT(*) FROM knowledge_metric_state WHERE vault_token=?1 AND state_kind='topic_cohort' AND phase='active'",[&vault],|row|row.get(0))?;
        let open_leaves: i64 = self.conn.query_row("SELECT COUNT(*) FROM knowledge_metric_state WHERE vault_token=?1 AND state_kind='ingest_24h'",[&vault],|row|row.get(0))?;
        let leaf_censored = count_kind("knowledge_censored")?;
        let median = self.topic_median_ms(&vault)?;
        let exact = self.count_reason(&vault, "citation_succeeded", "exact")?;
        let moved = self.count_reason(&vault, "citation_succeeded", "moved")?;
        let heading_only = self.count_reason(&vault, "citation_succeeded", "heading_only")?;
        let lost = self.count_reason(&vault, "citation_stale_anchor", "lost")?;
        Ok(MetricsSummary {
            range_start_day: start,
            range_end_day: end,
            suggestions: SuggestionSummary {
                accepted: (accepted_raw + accepted_roll).max(0) as u64,
                shown,
                dismissed: count_kind("suggestion_dismissed")?,
                rolled_back: count_kind("suggestion_rolled_back")?,
                insufficient: shown == 0,
            },
            topic_time: TimeSummary {
                completed,
                censored: topic_censored,
                open: open_topics.max(0) as u64,
                median_ms: median,
                insufficient: completed == 0,
            },
            linked_within_24h: LinkSummary {
                numerator: (linked_raw + linked_roll).max(0) as u64,
                denominator: eligible,
                censored: leaf_censored,
                open: open_leaves.max(0) as u64,
                automatic: linked_automatic,
                confirmed: linked_confirmed,
                manual: linked_manual,
                insufficient: eligible == 0,
            },
            citations: CitationSummary {
                attempts,
                succeeded,
                stale_anchor: stale,
                source_missing: missing,
                exact,
                moved,
                heading_only,
                lost,
                insufficient: attempts == 0,
            },
            model_supplement: RatioSummary {
                numerator: supplements,
                denominator: answers,
                censored: 0,
                open: 0,
                insufficient: answers == 0,
            },
        })
    }

    fn count_reason(&self, vault: &[u8], kind: &str, reason: &str) -> Result<u64, MetricsError> {
        self.count_reason_rollup(vault, kind, reason, &format!("citation_{reason}"))
    }

    fn count_reason_rollup(
        &self,
        vault: &[u8],
        kind: &str,
        reason: &str,
        rollup_metric: &str,
    ) -> Result<u64, MetricsError> {
        let raw: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM knowledge_events WHERE vault_token=?1 AND kind=?2 AND reason=?3",
            params![vault, kind, reason],
            |row| row.get(0),
        )?;
        let rolled: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(numerator),0) FROM knowledge_daily_rollups
             WHERE vault_token=?1 AND metric=?2",
            params![vault, rollup_metric],
            |row| row.get(0),
        )?;
        Ok((raw + rolled).max(0) as u64)
    }

    fn topic_median_ms(&self, vault: &[u8]) -> Result<Option<i64>, MetricsError> {
        let mut minutes = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT MAX(value_ms,0) FROM knowledge_events
             WHERE vault_token=?1 AND kind='topic_created' AND value_ms IS NOT NULL",
        )?;
        for value in stmt.query_map([vault], |row| row.get::<_, i64>(0))? {
            minutes.push((value?, 1_u64));
        }
        drop(stmt);
        let mut stmt = self.conn.prepare(
            "SELECT metric,numerator FROM knowledge_daily_rollups
             WHERE vault_token=?1 AND metric LIKE 'topic_duration_minutes:%'",
        )?;
        for row in stmt.query_map([vault], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (metric, count) = row?;
            let Some(value) = metric
                .strip_prefix("topic_duration_minutes:")
                .and_then(|value| value.parse::<i64>().ok())
            else {
                return Err(MetricsError::InvalidValue("duration rollup"));
            };
            minutes.push((value.saturating_mul(60_000), count.max(0) as u64));
        }
        weighted_median(minutes)
    }

    pub fn export_aggregate_json(
        &self,
        vault_root: &str,
        now_ms: i64,
        start_day: Option<i64>,
        end_day: Option<i64>,
    ) -> Result<String, MetricsError> {
        let export = self.aggregate_export(vault_root, now_ms, start_day, end_day)?;
        Ok(serde_json::to_string_pretty(&export).expect("fixed metrics export serializes"))
    }

    pub fn export_aggregate_csv(
        &self,
        vault_root: &str,
        now_ms: i64,
        start_day: Option<i64>,
        end_day: Option<i64>,
    ) -> Result<String, MetricsError> {
        let export = self.aggregate_export(vault_root, now_ms, start_day, end_day)?;
        let mut csv = String::from(
            "scope,day,metric,numerator,denominator,censored,aux_value,range_start_day,range_end_day,schema\n",
        );
        csv.push_str(&format!(
            "meta,,,,,,,{},{},{}\n",
            export.range_start_day.unwrap_or_default(),
            export.range_end_day.unwrap_or_default(),
            export.schema
        ));
        for row in export.daily {
            csv.push_str(&format!(
                "daily,{},{},{},{},{},{},,,\n",
                row.day,
                row.metric,
                row.numerator,
                row.denominator,
                row.censored,
                row.aux_value
                    .map_or_else(String::new, |value| value.to_string())
            ));
        }
        Ok(csv)
    }

    fn aggregate_export(
        &self,
        vault_root: &str,
        now_ms: i64,
        start_day: Option<i64>,
        end_day: Option<i64>,
    ) -> Result<MetricsExport, MetricsError> {
        self.maintain(now_ms)?;
        let vault = self.vault(vault_root);
        let from = start_day
            .or(self.range_for_vault(&vault)?.0)
            .unwrap_or(Self::day(now_ms));
        let to = end_day
            .or(self.range_for_vault(&vault)?.1)
            .unwrap_or(Self::day(now_ms));
        if from > to || to.saturating_sub(from) >= ROLLUP_RETENTION_DAYS {
            return Err(MetricsError::InvalidValue("export range"));
        }
        let daily = self.daily_rollups(&vault, from, to)?;
        let summary = self.summary_from_rollups(
            &vault,
            from,
            to,
            &daily,
            from <= Self::day(now_ms) && to >= Self::day(now_ms),
        )?;
        Ok(MetricsExport {
            schema: "luna.local-knowledge-metrics.aggregate.v1",
            range_start_day: Some(from),
            range_end_day: Some(to),
            summary,
            daily: daily
                .into_iter()
                .filter(|row| !row.metric.starts_with("topic_duration_minutes:"))
                .collect(),
        })
    }

    fn range_for_vault(&self, vault: &[u8]) -> Result<(Option<i64>, Option<i64>), MetricsError> {
        self.conn
            .query_row(
                "SELECT MIN(day),MAX(day) FROM (
                   SELECT day FROM knowledge_events WHERE vault_token=?1
                   UNION ALL SELECT day FROM knowledge_daily_rollups WHERE vault_token=?1)",
                [vault],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
    }

    fn summary_from_rollups(
        &self,
        vault: &[u8],
        from: i64,
        to: i64,
        daily: &[DailyRollup],
        include_open: bool,
    ) -> Result<MetricsSummary, MetricsError> {
        let mut summary = empty_summary(Some(from), Some(to));
        let mut durations = Vec::new();
        for row in daily {
            match row.metric.as_str() {
                "suggestions" => {
                    summary.suggestions.accepted += row.numerator;
                    summary.suggestions.shown += row.denominator;
                    summary.suggestions.dismissed += row.censored;
                }
                "suggestion_rolled_back" => {
                    summary.suggestions.rolled_back += row.numerator;
                }
                "topic_time" => {
                    summary.topic_time.completed += row.numerator;
                    summary.topic_time.censored += row.censored;
                }
                "linked_24h" => {
                    summary.linked_within_24h.numerator += row.numerator;
                    summary.linked_within_24h.denominator += row.denominator;
                    summary.linked_within_24h.censored += row.censored;
                }
                "linked_24h_automatic" => {
                    summary.linked_within_24h.automatic += row.numerator;
                }
                "linked_24h_confirmed" => {
                    summary.linked_within_24h.confirmed += row.numerator;
                }
                "linked_24h_manual" => {
                    summary.linked_within_24h.manual += row.numerator;
                }
                "citations" => {
                    summary.citations.succeeded += row.numerator;
                    summary.citations.attempts += row.denominator;
                }
                "citation_stale_anchor" => {
                    summary.citations.stale_anchor += row.numerator;
                }
                "citation_source_missing" => {
                    summary.citations.source_missing += row.numerator;
                }
                "citation_exact" => summary.citations.exact += row.numerator,
                "citation_moved" => summary.citations.moved += row.numerator,
                "citation_heading_only" => summary.citations.heading_only += row.numerator,
                "citation_lost" => summary.citations.lost += row.numerator,
                "model_supplement" => {
                    summary.model_supplement.numerator += row.numerator;
                    summary.model_supplement.denominator += row.denominator;
                }
                metric if metric.starts_with("topic_duration_minutes:") => {
                    let value = metric
                        .strip_prefix("topic_duration_minutes:")
                        .and_then(|value| value.parse::<i64>().ok())
                        .ok_or(MetricsError::InvalidValue("duration rollup"))?;
                    durations.push((value, row.numerator));
                }
                _ => {}
            }
        }
        summary.topic_time.median_ms =
            weighted_median(durations)?.map(|value| value.saturating_mul(60_000));
        if include_open {
            let topics: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM knowledge_metric_state
                 WHERE vault_token=?1 AND state_kind='topic_cohort' AND phase='active'",
                [vault],
                |row| row.get(0),
            )?;
            let leaves: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM knowledge_metric_state
                 WHERE vault_token=?1 AND state_kind='ingest_24h'",
                [vault],
                |row| row.get(0),
            )?;
            summary.topic_time.open = topics.max(0) as u64;
            summary.linked_within_24h.open = leaves.max(0) as u64;
        }
        summary.suggestions.insufficient = summary.suggestions.shown == 0;
        summary.topic_time.insufficient = summary.topic_time.completed == 0;
        summary.linked_within_24h.insufficient = summary.linked_within_24h.denominator == 0;
        summary.citations.insufficient = summary.citations.attempts == 0;
        summary.model_supplement.insufficient = summary.model_supplement.denominator == 0;
        Ok(summary)
    }

    pub fn daily_aggregates_for_vault(
        &self,
        vault_root: &str,
        now_ms: i64,
        days: u16,
    ) -> Result<Vec<DailyRollup>, MetricsError> {
        self.maintain(now_ms)?;
        let end = Self::day(now_ms);
        let start = end.saturating_sub(i64::from(days.saturating_sub(1)));
        self.daily_rollups(&self.vault(vault_root), start, end)
    }

    fn daily_rollups(
        &self,
        vault: &[u8],
        from: i64,
        to: i64,
    ) -> Result<Vec<DailyRollup>, MetricsError> {
        let mut map: HashMap<RollupKey, RollupValue> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT day,metric,numerator,denominator,censored,aux_value FROM knowledge_daily_rollups WHERE vault_token=?1 AND day BETWEEN ?2 AND ?3")?;
        for row in stmt.query_map(params![vault, from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<f64>>(5)?,
            ))
        })? {
            let (day, metric, n, d, c, a) = row?;
            map.insert(
                (day, metric),
                (n.max(0) as u64, d.max(0) as u64, c.max(0) as u64, a),
            );
        }
        drop(stmt);
        for day in from..=to {
            let counts = self.raw_day_counts(vault, day)?;
            for (metric, n, d, c, a) in counts {
                let entry = map.entry((day, metric)).or_insert((0, 0, 0, None));
                entry.0 += n;
                entry.1 += d;
                entry.2 += c;
                if a.is_some() {
                    entry.3 = a;
                }
            }
        }
        let mut out = map
            .into_iter()
            .map(
                |((day, metric), (numerator, denominator, censored, aux_value))| DailyRollup {
                    day,
                    metric,
                    numerator,
                    denominator,
                    censored,
                    aux_value,
                },
            )
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.day.cmp(&b.day).then_with(|| a.metric.cmp(&b.metric)));
        Ok(out)
    }

    fn raw_day_counts(&self, vault: &[u8], day: i64) -> Result<Vec<RawRollup>, MetricsError> {
        let count = |kind: &str| -> Result<u64, rusqlite::Error> {
            self.conn.query_row("SELECT COUNT(*) FROM knowledge_events WHERE vault_token=?1 AND day=?2 AND kind=?3",params![vault,day,kind],|row|row.get::<_,i64>(0)).map(|v|v.max(0) as u64)
        };
        let linked: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM knowledge_events
             WHERE vault_token=?1 AND day=?2 AND kind='knowledge_eligible'
               AND reason IN ('automatic','confirmed','manual')",
            params![vault, day],
            |row| row.get(0),
        )?;
        let mut result = vec![
            (
                "suggestions".into(),
                count("suggestion_accepted")?,
                count("suggestion_shown")?,
                count("suggestion_dismissed")?,
                None,
            ),
            (
                "suggestion_rolled_back".into(),
                count("suggestion_rolled_back")?,
                0,
                0,
                None,
            ),
            (
                "topic_time".into(),
                count("topic_created")?,
                0,
                count("topic_censored")?,
                None,
            ),
            (
                "linked_24h".into(),
                linked.max(0) as u64,
                count("knowledge_eligible")?,
                count("knowledge_censored")?,
                None,
            ),
            (
                "linked_24h_automatic".into(),
                self.count_raw_reason(vault, day, "knowledge_eligible", "automatic")?,
                0,
                0,
                None,
            ),
            (
                "linked_24h_confirmed".into(),
                self.count_raw_reason(vault, day, "knowledge_eligible", "confirmed")?,
                0,
                0,
                None,
            ),
            (
                "linked_24h_manual".into(),
                self.count_raw_reason(vault, day, "knowledge_eligible", "manual")?,
                0,
                0,
                None,
            ),
            (
                "citations".into(),
                count("citation_succeeded")?,
                count("citation_jump_attempt")?,
                0,
                None,
            ),
            (
                "citation_stale_anchor".into(),
                count("citation_stale_anchor")?,
                0,
                0,
                None,
            ),
            (
                "citation_source_missing".into(),
                count("citation_source_missing")?,
                0,
                0,
                None,
            ),
            (
                "citation_exact".into(),
                self.count_raw_reason(vault, day, "citation_succeeded", "exact")?,
                0,
                0,
                None,
            ),
            (
                "citation_moved".into(),
                self.count_raw_reason(vault, day, "citation_succeeded", "moved")?,
                0,
                0,
                None,
            ),
            (
                "citation_heading_only".into(),
                self.count_raw_reason(vault, day, "citation_succeeded", "heading_only")?,
                0,
                0,
                None,
            ),
            (
                "citation_lost".into(),
                self.count_raw_reason(vault, day, "citation_stale_anchor", "lost")?,
                0,
                0,
                None,
            ),
            (
                "model_supplement".into(),
                count("model_supplement_present")?,
                count("grounded_answer_finished")?,
                0,
                None,
            ),
        ];
        let mut stmt = self.conn.prepare(
            "SELECT MAX(value_ms,0)/60000,COUNT(*) FROM knowledge_events
             WHERE vault_token=?1 AND day=?2 AND kind='topic_created' AND value_ms IS NOT NULL
             GROUP BY MAX(value_ms,0)/60000",
        )?;
        let duration_rows = stmt
            .query_map(params![vault, day], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        result.extend(duration_rows.into_iter().map(|(minutes, count)| {
            (
                format!("topic_duration_minutes:{minutes}"),
                count.max(0) as u64,
                0,
                0,
                None,
            )
        }));
        Ok(result)
    }

    fn count_raw_reason(
        &self,
        vault: &[u8],
        day: i64,
        kind: &str,
        reason: &str,
    ) -> Result<u64, MetricsError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM knowledge_events
             WHERE vault_token=?1 AND day=?2 AND kind=?3 AND reason=?4",
            params![vault, day, kind, reason],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    fn maintain(&self, now_ms: i64) -> Result<(), MetricsError> {
        let transaction = self.conn.unchecked_transaction()?;
        let expired: Vec<(Vec<u8>, Vec<u8>, i64)> = {
            let mut stmt = self.conn.prepare("SELECT vault_token,entity_token,cycle FROM knowledge_metric_state WHERE state_kind='topic_cohort' AND phase='active' AND deadline_ms<=?1")?;
            let rows = stmt
                .query_map([now_ms], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for (vault, entity, cycle) in expired {
            self.finish_topic_censored(
                &vault,
                &entity,
                CohortCensor::ObservationEnded,
                now_ms,
                cycle,
            )?;
            self.conn.execute("UPDATE knowledge_metric_state SET phase='disabled',deadline_ms=NULL WHERE vault_token=?1 AND state_kind='topic_cohort' AND entity_token=?2",params![vault,entity])?;
        }
        self.conn.execute("DELETE FROM knowledge_metric_state WHERE state_kind='rollback_window' AND deadline_ms<?1",[now_ms])?;
        self.rollup_old_events(now_ms)?;
        self.conn.execute(
            "DELETE FROM knowledge_daily_rollups WHERE day<?1",
            [Self::day(now_ms) - ROLLUP_RETENTION_DAYS + 1],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn rollup_old_events(&self, now_ms: i64) -> Result<(), MetricsError> {
        let cutoff = Self::day(now_ms) - RAW_RETENTION_DAYS + 1;
        let total: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM knowledge_events", [], |row| {
                    row.get(0)
                })?;
        let count_cutoff = if total > MAX_RAW_EVENTS {
            self.conn.query_row("SELECT COALESCE(MAX(day)+1,?1) FROM (SELECT day FROM knowledge_events ORDER BY day,event_id LIMIT ?2)",params![cutoff,total-MAX_RAW_EVENTS],|row|row.get::<_,i64>(0))?.max(cutoff)
        } else {
            cutoff
        };
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT vault_token,day FROM knowledge_events WHERE day<?1 ORDER BY day",
        )?;
        let buckets = stmt
            .query_map([count_cutoff], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (vault, day) in buckets {
            for (metric, n, d, c, a) in self.raw_day_counts(&vault, day)? {
                self.conn.execute(
                    "INSERT INTO knowledge_daily_rollups(vault_token,day,metric,numerator,denominator,censored,aux_value)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(vault_token,day,metric) DO UPDATE SET numerator=excluded.numerator,denominator=excluded.denominator,censored=excluded.censored,aux_value=excluded.aux_value",
                    params![vault,day,metric,n,d,c,a],
                )?;
            }
            self.conn.execute(
                "DELETE FROM knowledge_events WHERE vault_token=?1 AND day=?2",
                params![vault, day],
            )?;
        }
        Ok(())
    }

    pub fn clear(self) -> Result<(), MetricsError> {
        self.conn
            .close()
            .map_err(|(_, error)| MetricsError::Db(error))?;
        for path in [
            &self.path,
            &sidecar(&self.path, "-wal"),
            &sidecar(&self.path, "-shm"),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(MetricsError::Io(error)),
            }
        }
        Ok(())
    }
}

fn canonical_topic_src(value: &str) -> String {
    value.trim().to_lowercase()
}

fn empty_summary(range_start_day: Option<i64>, range_end_day: Option<i64>) -> MetricsSummary {
    MetricsSummary {
        range_start_day,
        range_end_day,
        suggestions: SuggestionSummary {
            accepted: 0,
            shown: 0,
            dismissed: 0,
            rolled_back: 0,
            insufficient: true,
        },
        topic_time: TimeSummary {
            completed: 0,
            censored: 0,
            open: 0,
            median_ms: None,
            insufficient: true,
        },
        linked_within_24h: LinkSummary {
            numerator: 0,
            denominator: 0,
            censored: 0,
            open: 0,
            automatic: 0,
            confirmed: 0,
            manual: 0,
            insufficient: true,
        },
        citations: CitationSummary {
            attempts: 0,
            succeeded: 0,
            stale_anchor: 0,
            source_missing: 0,
            exact: 0,
            moved: 0,
            heading_only: 0,
            lost: 0,
            insufficient: true,
        },
        model_supplement: RatioSummary {
            numerator: 0,
            denominator: 0,
            censored: 0,
            open: 0,
            insufficient: true,
        },
    }
}

fn weighted_median(
    values: impl IntoIterator<Item = (i64, u64)>,
) -> Result<Option<i64>, MetricsError> {
    let mut counts = std::collections::BTreeMap::<i64, u64>::new();
    for (value, count) in values {
        if value < 0 {
            return Err(MetricsError::InvalidValue("negative duration"));
        }
        *counts.entry(value).or_default() += count;
    }
    let total = counts.values().copied().sum::<u64>();
    if total == 0 {
        return Ok(None);
    }
    let target = (total - 1) / 2;
    let mut seen = 0_u64;
    for (value, count) in counts {
        seen += count;
        if seen > target {
            return Ok(Some(value));
        }
    }
    Err(MetricsError::InvalidValue("duration rollup count"))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RollupColumn {
    Numerator,
    Denominator,
    Censored,
    None,
}

fn rollup_projection(kind: &str) -> (&'static str, RollupColumn) {
    match kind {
        "suggestion_shown" => ("suggestions", RollupColumn::Denominator),
        "suggestion_dismissed" => ("suggestions", RollupColumn::Censored),
        "suggestion_rolled_back" => ("suggestion_rolled_back", RollupColumn::Numerator),
        "topic_created" => ("topic_time", RollupColumn::Numerator),
        "topic_censored" => ("topic_time", RollupColumn::Censored),
        "knowledge_eligible" => ("linked_24h", RollupColumn::Denominator),
        "knowledge_censored" => ("linked_24h", RollupColumn::Censored),
        "citation_jump_attempt" => ("citations", RollupColumn::Denominator),
        "citation_succeeded" => ("citations", RollupColumn::Numerator),
        "citation_stale_anchor" => ("citation_stale_anchor", RollupColumn::Numerator),
        "citation_source_missing" => ("citation_source_missing", RollupColumn::Numerator),
        "grounded_answer_finished" => ("model_supplement", RollupColumn::Denominator),
        "model_supplement_present" => ("model_supplement", RollupColumn::Numerator),
        _ => ("", RollupColumn::None),
    }
}

/// RFC 2104 HMAC-SHA-256。设备密钥为随机 32 字节；这里也支持任意长度的测试密钥。
fn hmac_sha256(key: &[u8], pieces: &[&[u8]]) -> [u8; 32] {
    let mut normalized = [0u8; 64];
    if key.len() > normalized.len() {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    for byte in normalized {
        inner.update([byte ^ 0x36]);
    }
    for piece in pieces {
        inner.update(piece);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    for byte in normalized {
        outer.update([byte ^ 0x5c]);
    }
    outer.update(inner);
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, MetricsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MetricsStore::open(dir.path(), vec![7; 32]).unwrap();
        (dir, store)
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_case_one() {
        assert_eq!(
            hmac_sha256(&[0x0b; 20], &[b"Hi There"]),
            [
                0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
                0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
                0x2e, 0x32, 0xcf, 0xf7,
            ]
        );
    }

    #[test]
    fn schema_has_exactly_three_domain_tables() {
        let (_dir, s) = fixture();
        let names=s.conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert_eq!(
            names,
            vec![
                "knowledge_daily_rollups",
                "knowledge_events",
                "knowledge_metric_state"
            ]
        );
    }
    #[test]
    fn database_uses_required_file_name() {
        let (d, s) = fixture();
        assert_eq!(s.path(), d.path().join("knowledge-metrics.sqlite"));
    }
    #[test]
    fn unknown_schema_fails_closed_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let path = MetricsStore::database_path(dir.path());
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE alien(x); PRAGMA user_version=99;")
            .unwrap();
        drop(c);
        assert!(matches!(
            MetricsStore::open(dir.path(), vec![1; 32]),
            Err(MetricsError::UnknownSchema(99))
        ));
        assert!(path.exists());
    }
    #[test]
    fn same_version_with_an_unapproved_table_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = MetricsStore::database_path(dir.path());
        let store = MetricsStore::open(dir.path(), vec![1; 32]).unwrap();
        store
            .conn
            .execute("CREATE TABLE legacy_events(x)", [])
            .unwrap();
        drop(store);
        assert!(matches!(
            MetricsStore::open(dir.path(), vec![1; 32]),
            Err(MetricsError::UnknownSchema(1))
        ));
        assert!(path.exists());
    }
    #[test]
    fn raw_identifiers_never_appear_in_database_bytes() {
        let (d, s) = fixture();
        s.suggestion(
            "/Users/alice/SecretVault",
            "salary-plan",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        s.leaf_created(
            "/Users/alice/SecretVault",
            "operation-secret",
            "Notes/secret-title.md",
            2,
        )
        .unwrap();
        drop(s);
        let bytes = fs::read(MetricsStore::database_path(d.path())).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for forbidden in [
            "SecretVault",
            "salary-plan",
            "operation-secret",
            "secret-title",
        ] {
            assert!(!text.contains(forbidden));
        }
    }
    #[test]
    fn shown_dedupes_across_surfaces_and_restart() {
        let (d, s) = fixture();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        drop(s);
        let s = MetricsStore::open(d.path(), vec![7; 32]).unwrap();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Shown,
            SuggestionSurface::StartupOverview,
            2,
        )
        .unwrap();
        assert_eq!(s.summary_for_vault("/v", 2).unwrap().suggestions.shown, 1);
    }
    #[test]
    fn content_revision_is_a_new_suggestion_sample() {
        let (_d, s) = fixture();
        for token in ["v1", "v2"] {
            s.suggestion(
                "/v",
                token,
                SuggestionEvent::Shown,
                SuggestionSurface::Graph,
                1,
            )
            .unwrap();
        }
        assert_eq!(s.summary_for_vault("/v", 1).unwrap().suggestions.shown, 2);
    }
    #[test]
    fn unshown_acceptance_is_not_numerator() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Accepted,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        let x = s.summary_for_vault("/v", 1).unwrap();
        assert_eq!((x.suggestions.accepted, x.suggestions.shown), (0, 0));
    }
    #[test]
    fn queued_to_updated_reuses_one_acceptance() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        for now_ms in [2, 3] {
            s.suggestion("/v", "t", SuggestionEvent::Accepted, None, now_ms)
                .unwrap();
        }
        assert_eq!(
            s.summary_for_vault("/v", 3).unwrap().suggestions.accepted,
            1
        );
    }
    #[test]
    fn unshown_dismissal_is_not_a_funnel_terminal() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Dismissed,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 1).unwrap().suggestions.dismissed,
            0
        );
    }
    #[test]
    fn explicit_dismiss_is_distinct_from_passive_close() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Shown,
            SuggestionSurface::StartupOverview,
            1,
        )
        .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 1).unwrap().suggestions.dismissed,
            0
        );
        s.suggestion(
            "/v",
            "t",
            SuggestionEvent::Dismissed,
            SuggestionSurface::StartupOverview,
            2,
        )
        .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 2).unwrap().suggestions.dismissed,
            1
        );
    }
    #[test]
    fn first_high_cluster_is_only_a_baseline() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("Tag".into(), 3)], 1)
            .unwrap();
        assert_eq!(s.summary_for_vault("/v", 1).unwrap().topic_time.open, 0);
    }
    #[test]
    fn exact_two_to_three_starts_cohort() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("Tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        assert_eq!(s.summary_for_vault("/v", 2).unwrap().topic_time.open, 1);
    }
    #[test]
    fn cohort_survives_store_restart() {
        let (dir, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        drop(s);
        let s = MetricsStore::open(dir.path(), vec![7; 32]).unwrap();
        s.topic_created("/v", "tag", "op", 12).unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 12).unwrap().topic_time.median_ms,
            Some(10)
        );
    }
    #[test]
    fn absent_high_baseline_can_cross_again() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[], 2).unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 3)
            .unwrap();
        assert_eq!(s.summary_for_vault("/v", 3).unwrap().topic_time.open, 1);
    }
    #[test]
    fn suggestion_revision_does_not_reset_topic_cohort() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("Tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("Tag".into(), 3)], 10)
            .unwrap();
        s.suggestion(
            "/v",
            "revision-2",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            20,
        )
        .unwrap();
        s.topic_created("/v", "tag", "op", 30).unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 30).unwrap().topic_time.median_ms,
            Some(20)
        );
    }
    #[test]
    fn falling_below_threshold_censors() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 3)
            .unwrap();
        let x = s.summary_for_vault("/v", 3).unwrap().topic_time;
        assert_eq!((x.censored, x.open), (1, 0));
    }
    #[test]
    fn ignore_censors_active_cohort() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        s.topic_censored("/v", "tag", CohortCensor::Dismissed, 3)
            .unwrap();
        assert_eq!(s.summary_for_vault("/v", 3).unwrap().topic_time.censored, 1);
    }
    #[test]
    fn observation_limit_right_censors_an_open_cohort() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        let x = s
            .summary_for_vault("/v", 2 + OPEN_SAMPLE_MAX_MS)
            .unwrap()
            .topic_time;
        assert_eq!((x.censored, x.open), (1, 0));
    }
    #[test]
    fn disable_does_not_bridge_cohort() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 1)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 2)
            .unwrap();
        s.disable(3).unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 4)
            .unwrap();
        let x = s.summary_for_vault("/v", 4).unwrap().topic_time;
        assert_eq!((x.censored, x.open), (1, 0));
    }
    #[test]
    fn leaf_enters_denominator_only_at_24h_and_if_present() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        s.observe_leaf_facts("/v", &["n.md".into()], &[], DAY_MS - 1)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", DAY_MS - 1)
                .unwrap()
                .linked_within_24h
                .denominator,
            0
        );
        s.observe_leaf_facts("/v", &["n.md".into()], &[], DAY_MS)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", DAY_MS)
                .unwrap()
                .linked_within_24h
                .denominator,
            1
        );
    }
    #[test]
    fn deletion_inside_window_is_censored_not_denominator() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        s.observe_leaf_facts("/v", &[], &[], 1).unwrap();
        let x = s.summary_for_vault("/v", DAY_MS).unwrap().linked_within_24h;
        assert_eq!((x.denominator, x.censored), (0, 1));
    }
    #[test]
    fn known_deletion_after_deadline_preserves_the_denominator() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        s.leaf_deleted_by_path("/v", "n.md", DAY_MS + 1).unwrap();
        let x = s
            .summary_for_vault("/v", DAY_MS + 1)
            .unwrap()
            .linked_within_24h;
        assert_eq!((x.denominator, x.censored), (1, 0));
    }
    #[test]
    fn real_manual_link_counts_once() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        for _ in 0..2 {
            s.observe_leaf_facts("/v", &["n.md".into()], &["n.md".into()], 10)
                .unwrap();
        }
        s.observe_leaf_facts("/v", &["n.md".into()], &["n.md".into()], DAY_MS)
            .unwrap();
        let x = s.summary_for_vault("/v", DAY_MS).unwrap().linked_within_24h;
        assert_eq!((x.numerator, x.denominator), (1, 1));
    }
    #[test]
    fn automatic_confirmed_and_manual_links_share_the_same_numerator_rule() {
        let (_d, s) = fixture();
        for (index, origin) in [
            LinkOrigin::Automatic,
            LinkOrigin::Confirmed,
            LinkOrigin::Manual,
        ]
        .into_iter()
        .enumerate()
        {
            let operation = format!("op-{index}");
            let path = format!("n-{index}.md");
            s.leaf_created("/v", &operation, &path, 0).unwrap();
            s.leaf_linked_by_path("/v", &path, origin, 1).unwrap();
        }
        s.observe_leaf_facts(
            "/v",
            &["n-0.md".into(), "n-1.md".into(), "n-2.md".into()],
            &["n-0.md".into(), "n-1.md".into(), "n-2.md".into()],
            DAY_MS,
        )
        .unwrap();
        let x = s.summary_for_vault("/v", DAY_MS).unwrap().linked_within_24h;
        assert_eq!((x.numerator, x.denominator), (3, 3));
        assert_eq!((x.automatic, x.confirmed, x.manual), (1, 1, 1));
        for reason in ["automatic", "confirmed", "manual"] {
            let count: i64 = s
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM knowledge_events WHERE kind='topic_ingested' AND reason=?1",
                    [reason],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{reason}");
        }
    }
    #[test]
    fn move_preserves_open_leaf_identity() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "old.md", 0).unwrap();
        s.leaf_moved("/v", "old.md", "new.md").unwrap();
        s.observe_leaf_facts("/v", &["new.md".into()], &["new.md".into()], DAY_MS)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", DAY_MS)
                .unwrap()
                .linked_within_24h
                .numerator,
            1
        );
    }
    #[test]
    fn late_link_is_not_backdated() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        s.observe_leaf_facts("/v", &["n.md".into()], &["n.md".into()], DAY_MS + 1)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", DAY_MS + 1)
                .unwrap()
                .linked_within_24h
                .numerator,
            0
        );
    }
    #[test]
    fn citation_outcomes_are_mutually_exclusive() {
        let (_d, s) = fixture();
        for (i, o) in [
            CitationOutcome::Exact,
            CitationOutcome::Moved,
            CitationOutcome::HeadingOnly,
            CitationOutcome::Lost,
            CitationOutcome::SourceMissing,
        ]
        .into_iter()
        .enumerate()
        {
            s.citation("/v", &i.to_string(), o, 1).unwrap();
        }
        let x = s.summary_for_vault("/v", 1).unwrap().citations;
        assert_eq!(
            (x.attempts, x.succeeded, x.stale_anchor, x.source_missing),
            (5, 3, 1, 1)
        );
        assert_eq!((x.exact, x.moved, x.heading_only, x.lost), (1, 1, 1, 1));
    }
    #[test]
    fn answer_metric_uses_only_structured_boolean() {
        let (_d, s) = fixture();
        s.answer("/v", "a", false, 1).unwrap();
        s.answer("/v", "b", true, 1).unwrap();
        let x = s.summary_for_vault("/v", 1).unwrap().model_supplement;
        assert_eq!((x.numerator, x.denominator), (1, 2));
    }
    #[test]
    fn exact_inverse_restore_is_the_only_rollback() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "suggest",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            0,
        )
        .unwrap();
        s.suggestion("/v", "suggest", SuggestionEvent::Accepted, None, 0)
            .unwrap();
        s.register_rollback(
            "/v",
            RollbackWindow {
                suggestion_token: "suggest",
                operation_id: "op",
                note_path: "n.md",
                before_fingerprint: "before",
                after_fingerprint: "after",
                now_ms: 0,
            },
        )
        .unwrap();
        s.observe_restore("/v", "n.md", "other", "before", 1)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 1)
                .unwrap()
                .suggestions
                .rolled_back,
            0
        );
        s.observe_restore("/v", "n.md", "after", "before", 2)
            .unwrap();
        assert_eq!(
            s.summary_for_vault("/v", 2)
                .unwrap()
                .suggestions
                .rolled_back,
            1
        );
    }
    #[test]
    fn vaults_are_isolated() {
        let (_d, s) = fixture();
        s.suggestion(
            "/one",
            "t",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        assert_eq!(s.summary_for_vault("/two", 1).unwrap().suggestions.shown, 0);
    }
    #[test]
    fn equivalent_vault_paths_share_one_canonical_scope() {
        let (_d, s) = fixture();
        let vault = tempfile::tempdir().unwrap();
        let alias = vault.path().join(".");
        s.suggestion(
            &vault.path().display().to_string(),
            "topic",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        assert_eq!(
            s.summary_for_vault(&alias.display().to_string(), 1)
                .unwrap()
                .suggestions
                .shown,
            1
        );
    }
    #[test]
    fn export_has_aggregates_and_no_tokens_or_content() {
        let (_d, s) = fixture();
        s.suggestion(
            "/secret",
            "secret-token",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        let json = s.export_aggregate_json("/secret", 1, None, None).unwrap();
        assert!(json.contains("luna.local-knowledge-metrics.aggregate.v1"));
        for forbidden in [
            "secret-token",
            "entity_token",
            "operation_token",
            "vault_token",
            "event_id",
        ] {
            assert!(!json.contains(forbidden));
        }
    }
    #[test]
    fn csv_export_has_only_aggregate_rows_and_range_metadata() {
        let (_d, s) = fixture();
        s.suggestion(
            "/secret",
            "secret-token",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            1,
        )
        .unwrap();
        let csv = s.export_aggregate_csv("/secret", 1, None, None).unwrap();
        assert!(csv.starts_with("scope,day,metric,numerator,denominator,censored,aux_value,"));
        assert!(csv.contains("meta,,,,,,,0,0,luna.local-knowledge-metrics.aggregate.v1"));
        for forbidden in [
            "secret-token",
            "entity_token",
            "operation_token",
            "vault_token",
            "event_id",
        ] {
            assert!(!csv.contains(forbidden));
        }
    }
    #[test]
    fn export_summary_respects_the_selected_day_range() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "old",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            0,
        )
        .unwrap();
        s.suggestion(
            "/v",
            "new",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            2 * DAY_MS,
        )
        .unwrap();
        let export = s
            .export_aggregate_json("/v", 2 * DAY_MS, Some(2), Some(2))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&export).unwrap();
        assert_eq!(value["summary"]["suggestions"]["shown"], 1);
        assert_eq!(value["range_start_day"], 2);
        assert_eq!(value["range_end_day"], 2);
    }
    #[test]
    fn clear_removes_main_wal_and_shm() {
        let (d, s) = fixture();
        let path = s.path().to_owned();
        // 只确保两份旁路文件在场，不截断：连接开着时 `-shm` 已被 SQLite 映射，Windows 拒绝截断
        // 已映射的文件（os error 1224）。
        for suffix in ["-wal", "-shm"] {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(sidecar(&path, suffix))
                .unwrap();
        }
        s.clear().unwrap();
        assert!(!MetricsStore::database_files_exist(d.path()));
    }
    #[test]
    fn event_retention_rolls_old_data_without_losing_summary() {
        let (_d, s) = fixture();
        s.suggestion(
            "/v",
            "old",
            SuggestionEvent::Shown,
            SuggestionSurface::Graph,
            0,
        )
        .unwrap();
        s.summary_for_vault("/v", (RAW_RETENTION_DAYS + 2) * DAY_MS)
            .unwrap();
        let x = s
            .summary_for_vault("/v", (RAW_RETENTION_DAYS + 2) * DAY_MS)
            .unwrap();
        assert_eq!(x.suggestions.shown, 1);
    }
    #[test]
    fn linked_result_and_origin_survive_cross_day_raw_retention() {
        let (_d, s) = fixture();
        s.leaf_created("/v", "op", "n.md", 0).unwrap();
        s.leaf_linked_by_path("/v", "n.md", LinkOrigin::Confirmed, 1)
            .unwrap();
        s.observe_leaf_facts("/v", &["n.md".into()], &["n.md".into()], DAY_MS)
            .unwrap();

        // 第一次维护只清理链接事件所在的前一天；资格事实仍须独立携带链接结果。
        s.summary_for_vault("/v", (RAW_RETENTION_DAYS + 1) * DAY_MS)
            .unwrap();
        let x = s
            .summary_for_vault("/v", (RAW_RETENTION_DAYS + 2) * DAY_MS)
            .unwrap()
            .linked_within_24h;
        assert_eq!((x.numerator, x.denominator), (1, 1));
        assert_eq!((x.automatic, x.confirmed, x.manual), (0, 1, 0));
    }
    #[test]
    fn citation_detail_survives_raw_event_retention() {
        let (_d, s) = fixture();
        for (id, outcome) in [
            ("exact", CitationOutcome::Exact),
            ("moved", CitationOutcome::Moved),
            ("heading", CitationOutcome::HeadingOnly),
            ("lost", CitationOutcome::Lost),
        ] {
            s.citation("/v", id, outcome, 0).unwrap();
        }
        let now_ms = (RAW_RETENTION_DAYS + 2) * DAY_MS;
        let x = s.summary_for_vault("/v", now_ms).unwrap().citations;
        assert_eq!((x.exact, x.moved, x.heading_only, x.lost), (1, 1, 1, 1));
        assert_eq!((x.attempts, x.succeeded, x.stale_anchor), (4, 3, 1));
    }
    #[test]
    fn topic_median_survives_raw_event_retention() {
        let (_d, s) = fixture();
        s.observe_topic_clusters("/v", &[("tag".into(), 2)], 0)
            .unwrap();
        s.observe_topic_clusters("/v", &[("tag".into(), 3)], 1)
            .unwrap();
        s.topic_created("/v", "tag", "op", 120_001).unwrap();
        let now_ms = (RAW_RETENTION_DAYS + 2) * DAY_MS;
        assert_eq!(
            s.summary_for_vault("/v", now_ms)
                .unwrap()
                .topic_time
                .median_ms,
            Some(120_000)
        );
    }
}
