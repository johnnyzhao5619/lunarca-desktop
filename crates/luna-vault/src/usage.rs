//! per-Vault 用量事实库（TDD-01 §5.8）。
//!
//! `usage.sqlite` 是 durable 历史事实，与可重建的 `index.sqlite` 分离。

use luna_core::{Capability, JobOrigin, Money, ProviderId, TruncatedBy, Usage, UsageRecord};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Result, VaultError};

const SCHEMA_VERSION: i64 = 7;

const SCHEMA_SQL: &str = "\
CREATE TABLE usage_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    capability TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_location TEXT,
    model_id TEXT,
    audio_seconds REAL,
    tokens_in INTEGER,
    tokens_out INTEGER,
    est_cost_minor INTEGER,
    terminal_status TEXT NOT NULL,
    output_truncated TEXT,
    service_class TEXT,
    error_code TEXT,
    finished_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_usage_finished_at ON usage_records(finished_at_ms);
CREATE INDEX idx_usage_provider ON usage_records(provider_id);
CREATE TABLE ai_outcomes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    origin TEXT NOT NULL,
    action TEXT NOT NULL,
    landing TEXT NOT NULL,
    recorded_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_ai_outcomes_recorded_at ON ai_outcomes(recorded_at_ms);
CREATE TABLE queue_outcomes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    note_rel TEXT,
    outcome TEXT NOT NULL,
    code TEXT,
    finished_at_ms INTEGER NOT NULL
);
CREATE INDEX idx_queue_outcomes_finished_at ON queue_outcomes(finished_at_ms);
CREATE TABLE playback_positions (
    vault_key TEXT NOT NULL,
    note_rel TEXT NOT NULL,
    position_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (vault_key, note_rel)
);
";

/// 队列**非成功终态**的持久记录（PRD-04-062）：失败 / 取消项重启后仍能回看「为什么」。
///
/// 只存这一半：成功项的收据就是它的产物（且几类另有完成通知随库持久），再存是重复事实源；
/// 续跑档只存未完成载荷，历史塞进去会无界增长且带着执行才需要的路径 / URL。
/// **只存**任务类型、展示标签、归属笔记相对路径、终态与失败码、时刻——绝不存载荷路径、抓取 URL、
/// 任何正文（与队列 DTO 同口径，TDD-12）。`queue_outcome_columns` 的断言钉住这份字段清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOutcome {
    /// 行 id；插入前为 `None`，读回时在场（逐条移除靠它）。
    pub id: Option<i64>,
    /// 任务类型稳定标签（`import` / `fetch` / `distill` / …，同 `ProcTaskDto.kind`）。
    pub kind: String,
    /// 展示标签（文件名 / 笔记名，脱敏，同 `ProcTaskDto.label`）。
    pub label: String,
    /// 归属笔记相对路径（有则）。
    pub note_rel: Option<String>,
    /// `failed` | `cancelled`。
    pub outcome: String,
    /// 失败码（取消为 `None`）；只放稳定码，不放上游原文。
    pub code: Option<String>,
    /// 终止时刻（Unix ms）。
    pub finished_at_ms: i64,
}

/// 历史表的字段清单（隐私断言用：新增字段必须先过这份清单，再过「它是不是载荷」的追问）。
pub const QUEUE_OUTCOME_COLUMNS: [&str; 7] = [
    "id",
    "kind",
    "label",
    "note_rel",
    "outcome",
    "code",
    "finished_at_ms",
];

/// AI 结果的**落地回执**（PRD-06-021）：用户对一份 AI 结果最终做了什么——替换 / 插入 /
/// 丢弃。`usage_records` 答的是「生成了多少」，这张表答的是「生成的东西有没有被用上」，
/// 两者相除才是接受率；没有它，「AI 动作有没有用」在本机没有任何地方回答得了。
///
/// 只存动作名与落地方式，**绝不含正文**（日志脱敏铁律同口径）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiOutcome {
    /// 发起来源（与 `usage_records.origin` 同一枚举、同一序列化）。
    pub origin: JobOrigin,
    /// 动作稳定 id（`summarize` / `translate` / `optimize` / `ask` / `continue`）。
    pub action: String,
    /// 落地方式稳定 id（`replace` / `insert_below` / … / `discard`）。
    pub landing: String,
    /// 记录时间（Unix ms）。
    pub recorded_at_ms: i64,
}

/// 落地回执合计：`landed` = 进了正文（替换或插入），`discarded` = 丢弃。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiOutcomeSummary {
    /// 进了正文的份数。
    pub landed: u64,
    /// 被丢弃的份数。
    pub discarded: u64,
}

/// 基础用量汇总（P1 聚合/过滤/导出不在本阶段）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageSummary {
    /// 记录数。
    pub records: u64,
    /// 输入 token 合计。
    pub tokens_in: u64,
    /// 输出 token 合计。
    pub tokens_out: u64,
    /// 音频秒数合计。
    pub audio_seconds: f64,
    /// 估算成本合计（minor units）。
    pub est_cost_minor: i64,
    /// 按「在哪跑的」分开的音频秒数与 token 合计（ADR-126 第 6 条）。
    ///
    /// 「本机省下多少」与「服务端花了多少」是**两个方向相反的数**，而此前只有一个不分
    /// 位置的合计——服务端跑掉的那些于是被算进了「为你节省」。位置未知的历史行两边都
    /// 不计：算不出来就不算，不往任何一边凑。
    pub local_audio_seconds: f64,
    pub local_tokens: u64,
    pub server_audio_seconds: f64,
    pub server_tokens: u64,
    /// 其中**触顶收尾**的记录数（三档不分，用户先要知道的是"有没有、有几条"）。
    ///
    /// 触顶不是失败，故它在 `records` 里、也不在任何失败计数里——不单独数一次的话，
    /// 「我这个月有几篇被截断了」这个问题在本机没有任何地方回答得了。
    pub truncated: u64,
}

/// 查询范围。`None` 字段表示不设边界。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageRange {
    /// 起始 Unix ms。
    pub from_ms: Option<i64>,
    /// 结束 Unix ms。
    pub to_ms: Option<i64>,
    /// 上限。
    pub limit: Option<usize>,
}

/// 单个 Vault 的用量库句柄。
#[derive(Clone)]
pub struct UsageStore {
    path: PathBuf,
}

impl UsageStore {
    /// 打开或初始化 usage.sqlite。
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        init_db(&path)?;
        Ok(Self { path })
    }

    /// 指向 usage.sqlite 但不建库（只读打开 Vault 用，[`crate::Vault::open_read_only`]）：
    /// 句柄只记路径，真正连库在各操作里。
    pub(crate) fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 数据库路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 插入一条用量记录。
    pub fn insert(&self, record: &UsageRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO usage_records (
                job_id, origin, capability, provider_id, model_id, audio_seconds,
                tokens_in, tokens_out, est_cost_minor, terminal_status, output_truncated,
                service_class, error_code, finished_at_ms, provider_location
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                record.job_id.to_string(),
                enum_to_str(&record.origin)?,
                enum_to_str(&record.capability)?,
                record.provider_id.as_str(),
                record.model_id.as_ref().map(|m| m.as_str().to_owned()),
                record.usage.audio_seconds,
                record.usage.tokens_in.map(i64::from),
                record.usage.tokens_out.map(i64::from),
                record.usage.est_cost.map(|m| m.minor_units),
                record.terminal_status,
                record.output_truncated.map(|by| by.key()),
                record.service_class.map(luna_core::ServiceClass::as_str),
                record.error_code.as_deref(),
                record.finished_at_ms,
                record
                    .provider_location
                    .map(luna_core::ProviderLocation::as_str),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// 记一条 AI 结果落地回执。
    pub fn insert_outcome(&self, outcome: &AiOutcome) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO ai_outcomes (origin, action, landing, recorded_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                enum_to_str(&outcome.origin)?,
                outcome.action,
                outcome.landing,
                outcome.recorded_at_ms,
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// 记一条队列非成功终态（PRD-04-062）；返回行 id。
    pub fn insert_queue_outcome(&self, outcome: &QueueOutcome) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO queue_outcomes (kind, label, note_rel, outcome, code, finished_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                outcome.kind,
                outcome.label,
                outcome.note_rel,
                outcome.outcome,
                outcome.code,
                outcome.finished_at_ms,
            ],
        )
        .map_err(map_sqlite)?;
        Ok(conn.last_insert_rowid())
    }

    /// 按时间倒序列出队列非成功终态。
    pub fn list_queue_outcomes(&self, limit: usize) -> Result<Vec<QueueOutcome>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, label, note_rel, outcome, code, finished_at_ms
                 FROM queue_outcomes ORDER BY finished_at_ms DESC, id DESC LIMIT ?1",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(QueueOutcome {
                    id: Some(row.get(0)?),
                    kind: row.get(1)?,
                    label: row.get(2)?,
                    note_rel: row.get(3)?,
                    outcome: row.get(4)?,
                    code: row.get(5)?,
                    finished_at_ms: row.get(6)?,
                })
            })
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 删除一条队列历史（面板逐条移除）。
    pub fn delete_queue_outcome(&self, id: i64) -> Result<bool> {
        let conn = self.conn()?;
        let changed = conn
            .execute("DELETE FROM queue_outcomes WHERE id = ?1", params![id])
            .map_err(map_sqlite)?;
        Ok(changed > 0)
    }

    /// 清空队列历史（面板「清除未成功」）。
    pub fn clear_queue_outcomes(&self) -> Result<usize> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM queue_outcomes", [])
            .map_err(map_sqlite)
    }

    /// 条数 + 天数双封顶（PRD-04-062）：先按时间淘汰，再只留最新 `max_records` 条。
    pub fn prune_queue_outcomes(
        &self,
        max_records: usize,
        oldest_allowed_ms: i64,
    ) -> Result<usize> {
        let conn = self.conn()?;
        let by_age = conn
            .execute(
                "DELETE FROM queue_outcomes WHERE finished_at_ms < ?1",
                params![oldest_allowed_ms],
            )
            .map_err(map_sqlite)?;
        let by_count = conn
            .execute(
                "DELETE FROM queue_outcomes
                 WHERE id NOT IN (
                    SELECT id FROM queue_outcomes ORDER BY finished_at_ms DESC, id DESC LIMIT ?1
                 )",
                params![max_records as i64],
            )
            .map_err(map_sqlite)?;
        Ok(by_age + by_count)
    }

    /// 历史表实际列名（隐私断言：与 [`QUEUE_OUTCOME_COLUMNS`] 逐一相等）。
    pub fn queue_outcome_columns(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("PRAGMA table_info(queue_outcomes)")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 落地回执合计（接受率 = landed / (landed + discarded)）。
    pub fn outcome_summary(&self) -> Result<AiOutcomeSummary> {
        let conn = self.conn()?;
        let (landed, discarded): (i64, i64) = conn
            .query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN landing <> 'discard' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN landing = 'discard' THEN 1 ELSE 0 END), 0)
                 FROM ai_outcomes",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sqlite)?;
        Ok(AiOutcomeSummary {
            landed: landed.max(0) as u64,
            discarded: discarded.max(0) as u64,
        })
    }

    /// 按时间倒序列出用量记录。
    pub fn list(&self, range: UsageRange) -> Result<Vec<UsageRecord>> {
        let conn = self.conn()?;
        let from = range.from_ms.unwrap_or(i64::MIN);
        let to = range.to_ms.unwrap_or(i64::MAX);
        let limit = range.limit.unwrap_or(500) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT job_id, origin, capability, provider_id, model_id, audio_seconds,
                    tokens_in, tokens_out, est_cost_minor, terminal_status, output_truncated,
                    service_class, error_code, finished_at_ms, provider_location
                 FROM usage_records
                 WHERE finished_at_ms >= ?1 AND finished_at_ms <= ?2
                 ORDER BY finished_at_ms DESC, id DESC
                 LIMIT ?3",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![from, to, limit], record_from_row)
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 基础合计。
    pub fn summary(&self) -> Result<UsageSummary> {
        let conn = self.conn()?;
        conn.query_row(
            // 位置分组用 `CASE WHEN`：`NULL`（补列前的历史行）两边都落不进去，正是
            // 「算不出来就不算」。`cloud` 是唯一外发的一档（`ProviderLocation::is_external`），
            // 本机那一侧含进程内引擎与 localhost 服务。
            "SELECT COUNT(*), COALESCE(SUM(tokens_in), 0), COALESCE(SUM(tokens_out), 0),
                    COALESCE(SUM(audio_seconds), 0), COALESCE(SUM(est_cost_minor), 0),
                    COUNT(output_truncated),
                    COALESCE(SUM(CASE WHEN provider_location IN ('device', 'local_service')
                        THEN audio_seconds ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN provider_location IN ('device', 'local_service')
                        THEN COALESCE(tokens_in, 0) + COALESCE(tokens_out, 0) ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN provider_location = 'cloud'
                        THEN audio_seconds ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN provider_location = 'cloud'
                        THEN COALESCE(tokens_in, 0) + COALESCE(tokens_out, 0) ELSE 0 END), 0)
             FROM usage_records",
            [],
            |row| {
                Ok(UsageSummary {
                    records: row.get::<_, i64>(0)? as u64,
                    tokens_in: row.get::<_, i64>(1)? as u64,
                    tokens_out: row.get::<_, i64>(2)? as u64,
                    audio_seconds: row.get(3)?,
                    est_cost_minor: row.get(4)?,
                    truncated: row.get::<_, i64>(5)? as u64,
                    local_audio_seconds: row.get(6)?,
                    local_tokens: row.get::<_, i64>(7)? as u64,
                    server_audio_seconds: row.get(8)?,
                    server_tokens: row.get::<_, i64>(9)? as u64,
                })
            },
        )
        .map_err(map_sqlite)
    }

    /// 保留最新 `max_records` 条，删除更老记录。
    pub fn prune_to_latest(&self, max_records: usize) -> Result<usize> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "DELETE FROM usage_records
                 WHERE id NOT IN (
                    SELECT id FROM usage_records ORDER BY finished_at_ms DESC, id DESC LIMIT ?1
                 )",
                params![max_records as i64],
            )
            .map_err(map_sqlite)?;
        Ok(changed)
    }

    /// 清空。
    pub fn clear(&self) -> Result<usize> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM usage_records", [])
            .map_err(map_sqlite)
    }

    /// 记住这一篇上次听到的位置（PRD-04-068）。`position_ms` 为 0 = 从头（播完即归零）。
    ///
    /// `vault_key` 是库根路径：设备级用量库里同一个相对路径可能属于两个库。
    pub fn save_playback_position(
        &self,
        vault_key: &str,
        note_rel: &str,
        position_ms: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO playback_positions (vault_key, note_rel, position_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(vault_key, note_rel) DO UPDATE SET
               position_ms = excluded.position_ms,
               updated_at_ms = excluded.updated_at_ms",
            params![vault_key, note_rel, position_ms.max(0), unix_now_ms()],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// 上次听到的位置；没记过返回 `None`。
    pub fn load_playback_position(&self, vault_key: &str, note_rel: &str) -> Result<Option<i64>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT position_ms FROM playback_positions WHERE vault_key = ?1 AND note_rel = ?2",
            params![vault_key, note_rel],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_sqlite)
    }

    fn conn(&self) -> Result<Connection> {
        Connection::open(&self.path).map_err(map_sqlite)
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn init_db(path: &Path) -> Result<()> {
    match try_init_db(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            quarantine(path);
            try_init_db(path)
        }
    }
}

fn try_init_db(path: &Path) -> Result<()> {
    let conn = Connection::open(path).map_err(map_sqlite)?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sqlite)?;
    match version {
        SCHEMA_VERSION => {}
        // 认得出的旧版本：逐版补列，历史记录原样保留。
        //
        // **这一支不是 ADR-046 说的兼容层**：它不双读双写、不留可选字段过渡，跑完之后
        // 库里只有一种形状。要区别对待的是这张表存的东西——`usage.sqlite` 是 durable
        // 历史事实（与可重建的 `index.sqlite` 分开正是为此）。为了加一列就把它整张重建，
        // 用户失去的是「这几个月我用了多少」，而那正是这张表存在的全部理由。
        v if v > 0 && v < SCHEMA_VERSION => add_missing_columns(&conn, v)?,
        // 认不出的版本（未来版本回退、文件被写坏）：重建成当前形状，同既有口径。
        _ => rebuild_schema(&conn)?,
    }
    Ok(())
}

/// 逐版增量表：`(该项首次出现的 schema 版本, DDL 语句)`——加列或加表。
///
/// **是一张表而不是一串 `match` 分支**：分支写法要为每个起点版本各抄一遍后续步骤，
/// 而抄漏一步的表现是「从 v1 直接升到 v3 的库少一列」——只有那一批用户会遇到，
/// 且直到读取时才炸。表驱动让「从哪来」只影响筛选条件，不影响步骤本身。
const SCHEMA_STEPS: [(i64, &str); 7] = [
    (
        2,
        "ALTER TABLE usage_records ADD COLUMN output_truncated TEXT",
    ),
    (3, "ALTER TABLE usage_records ADD COLUMN service_class TEXT"),
    (3, "ALTER TABLE usage_records ADD COLUMN error_code TEXT"),
    // v4：AI 结果落地回执表（PRD-06-021 接受率）。历史用量记录原样保留。
    (
        4,
        "CREATE TABLE IF NOT EXISTS ai_outcomes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            origin TEXT NOT NULL,
            action TEXT NOT NULL,
            landing TEXT NOT NULL,
            recorded_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ai_outcomes_recorded_at ON ai_outcomes(recorded_at_ms);",
    ),
    // v5：队列非成功终态历史（PRD-04-062）。历史用量记录与落地回执原样保留。
    (
        5,
        "CREATE TABLE IF NOT EXISTS queue_outcomes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            label TEXT NOT NULL,
            note_rel TEXT,
            outcome TEXT NOT NULL,
            code TEXT,
            finished_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_queue_outcomes_finished_at ON queue_outcomes(finished_at_ms);",
    ),
    // v6：每篇的播放位置（PRD-04-068）。设备侧阅读进度：键 = 库根路径 + 库内相对路径，
    // 只在**设备级**用量库里写（随库的那份永远空表）——跟着库走会让同一篇在两台机器上互相覆盖。
    (
        6,
        "CREATE TABLE IF NOT EXISTS playback_positions (
            vault_key TEXT NOT NULL,
            note_rel TEXT NOT NULL,
            position_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY (vault_key, note_rel)
        );",
    ),
    // v7：这一次在哪跑的（ADR-126 第 6 条）。历史行留空——不给它们猜一个位置。
    (
        7,
        "ALTER TABLE usage_records ADD COLUMN provider_location TEXT",
    ),
];

/// 这一步是不是「列已经在场」——加列必须幂等，理由见 [`add_missing_columns`]。
///
/// 只认这一条消息：rusqlite 把它归在通用 `SQLITE_ERROR` 下，没有可比对的细分码。
/// 其余错误一律不吞。
fn is_duplicate_column(err: &rusqlite::Error) -> bool {
    err.to_string().contains("duplicate column name")
}

fn add_missing_columns(conn: &Connection, from_version: i64) -> Result<()> {
    for (introduced_in, sql) in SCHEMA_STEPS {
        if from_version < introduced_in {
            // **加列这一步必须幂等**：列已经在场时 SQLite 报 `duplicate column name`，
            // 而 `init_db` 对任何错误的处置是**隔离并重建**——一条本可跳过的语句会让
            // durable 历史事实整库清空，而用户失去的正是「这几个月我用了多少」。
            // 版本号与真实形状对不上不是假想：装过新版再回退、跨版本升级中途断电、
            // 或者像 v4/v5 那两条用例那样"以当前形状建库再回拨版本号"都会走到这里。
            // 建表那几步本就带 `IF NOT EXISTS`，这一条把加列补齐到同一条纪律上。
            if let Err(err) = conn.execute_batch(sql) {
                if !is_duplicate_column(&err) {
                    return Err(map_sqlite(err));
                }
            }
        }
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(map_sqlite)?;
    Ok(())
}

fn rebuild_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS usage_records;
         DROP INDEX IF EXISTS idx_usage_finished_at;
         DROP INDEX IF EXISTS idx_usage_provider;
         DROP TABLE IF EXISTS ai_outcomes;
         DROP INDEX IF EXISTS idx_ai_outcomes_recorded_at;
         DROP TABLE IF EXISTS queue_outcomes;
         DROP INDEX IF EXISTS idx_queue_outcomes_finished_at;
         DROP TABLE IF EXISTS playback_positions;",
    )
    .map_err(map_sqlite)?;
    conn.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(map_sqlite)?;
    Ok(())
}

fn quarantine(path: &Path) {
    let backup = path.with_extension("sqlite.corrupt");
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::rename(path, backup);
}

fn enum_to_str<T: Serialize>(value: &T) -> Result<String> {
    let json = serde_json::to_value(value).map_err(|e| VaultError::Decode(e.to_string()))?;
    json.as_str()
        .map(str::to_owned)
        .ok_or_else(|| VaultError::Decode("enum did not serialize to string".into()))
}

fn enum_from_str<T: DeserializeOwned>(value: String) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    let job_id: String = row.get(0)?;
    let origin: String = row.get(1)?;
    let capability: String = row.get(2)?;
    let provider_id: String = row.get(3)?;
    let model_id: Option<String> = row.get(4)?;
    let est_cost_minor: Option<i64> = row.get(8)?;
    Ok(UsageRecord {
        job_id: enum_from_str(job_id)?,
        origin: enum_from_str::<JobOrigin>(origin)?,
        capability: enum_from_str::<Capability>(capability)?,
        provider_id: ProviderId::from(provider_id),
        model_id: model_id.map(luna_core::ModelId::from),
        usage: Usage {
            audio_seconds: row.get(5)?,
            tokens_in: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
            tokens_out: row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
            est_cost: est_cost_minor.map(|minor_units| Money { minor_units }),
        },
        terminal_status: row.get(9)?,
        // 「取，不猜」（同 `TruncatedBy::from_key` 的口径）：认不出的字符串按没说处理，
        // 绝不猜一个归属——猜错会把「上游机器上限」说成「你自己的额度设小了」。
        output_truncated: row
            .get::<_, Option<String>>(10)?
            .as_deref()
            .and_then(TruncatedBy::from_key),
        // 「取，不猜」（同上一格）：补列之前写下的历史行没有这一格，认不出的字符串
        // 也按没说处理。猜一个默认档会让当时的实时档记录在账本上变成批量档，
        // 而这一格存在的全部理由正是分辨这两者。
        service_class: row
            .get::<_, Option<String>>(11)?
            .as_deref()
            .and_then(luna_core::ServiceClass::parse),
        // 「取，不猜」（同上两格）：补列之前的历史行没有这一格。猜一个位置会让当时在
        // 服务端跑的那些记录变成「本机省下的」。
        provider_location: row
            .get::<_, Option<String>>(14)?
            .as_deref()
            .and_then(luna_core::ProviderLocation::parse),
        error_code: row.get(12)?,
        finished_at_ms: row.get(13)?,
    })
}

fn map_sqlite(e: rusqlite::Error) -> VaultError {
    VaultError::Decode(format!("usage sqlite: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::TruncatedBy;
    use luna_core::{Job, JobKind, JobResult, Lang, ResultPayload, TextRef, Timestamp, Timings};

    fn sample_record(finished_at_ms: i64, tokens_out: u32) -> UsageRecord {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("private source".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::FloatingMenu,
        );
        let result = JobResult {
            job_id: job.id,
            provider_id: ProviderId::from("local-llm"),
            model_id: None,
            payload: ResultPayload::Text("private output".into()),
            usage: Usage {
                tokens_in: Some(5),
                tokens_out: Some(tokens_out),
                ..Default::default()
            },
            timings: Timings::default(),
            output_truncated: None,
            finished_at: Timestamp::from_millis(finished_at_ms),
        };
        UsageRecord::from_finished(&job, &result, luna_core::ProviderLocation::Cloud)
    }

    /// **加列必须幂等，否则整库被隔离清空**（ADR-126 第 6 条同批加固）。
    ///
    /// 库已经是当前形状、而版本号写着旧值时（装过新版再回退、升级中途断电），迁移会对
    /// 一个已经在场的列再 `ADD COLUMN` 一次 —— SQLite 报 `duplicate column name`，
    /// 而 `init_db` 对错误的处置是隔离并重建：用户失去的是「这几个月我用了多少」，
    /// 也就是这张表存在的全部理由。反向验证：把 `is_duplicate_column` 那一支去掉，
    /// 这条当场红。
    #[test]
    fn rerunning_a_migration_over_an_existing_column_keeps_the_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let store = UsageStore::open(&path).unwrap();
        store.insert(&sample_record(100, 3)).unwrap();
        drop(store);

        // 形状已是当前版，版本号却回拨到最早——每一条加列步骤都会撞上已存在的列。
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        assert_eq!(
            store.list(UsageRange::default()).unwrap().len(),
            1,
            "重跑迁移把 durable 历史整库清空了"
        );
    }

    /// **「在哪跑的」必须落库、读得回来、并按位置分开合计**（ADR-126 第 6 条）。
    ///
    /// 反向验证：把 `summary()` 里那两个 `CASE WHEN` 的位置集合对调，这条当场红。它拦的是
    /// 一个方向性错误——服务端跑掉的任务被算进「本机省下」，等于把花费说成节省。
    #[test]
    fn summary_splits_totals_by_where_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();

        let mut local = sample_record(100, 3);
        local.provider_location = Some(luna_core::ProviderLocation::LocalService);
        local.usage.audio_seconds = Some(60.0);
        let mut cloud = sample_record(200, 11);
        cloud.provider_location = Some(luna_core::ProviderLocation::Cloud);
        cloud.usage.audio_seconds = Some(30.0);
        // 补列之前写下的历史行：位置未知，**两边都不该算进去**。
        let mut historical = sample_record(300, 100);
        historical.provider_location = None;
        historical.usage.audio_seconds = Some(999.0);

        for record in [&local, &cloud, &historical] {
            store.insert(record).unwrap();
        }

        let summary = store.summary().unwrap();
        // 本机那一侧含进程内引擎与 localhost 服务（`is_external` 为假的两档）。
        assert!((summary.local_audio_seconds - 60.0).abs() < f64::EPSILON);
        assert_eq!(summary.local_tokens, 8, "本机：输入 5 + 输出 3");
        assert!((summary.server_audio_seconds - 30.0).abs() < f64::EPSILON);
        assert_eq!(summary.server_tokens, 16, "服务端：输入 5 + 输出 11");
        // 不分位置的合计照旧含全部三条——它回答的是另一个问题。
        assert_eq!(summary.records, 3);
        assert_eq!(summary.tokens_out, 114);

        // 位置真的读得回来（列写进去了、也读对了列序）。
        let records = store.list(UsageRange::default()).unwrap();
        assert_eq!(records[0].provider_location, None, "历史行不猜位置");
        assert_eq!(
            records[2].provider_location,
            Some(luna_core::ProviderLocation::LocalService)
        );
    }

    #[test]
    fn insert_list_summary_and_prune_usage_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        store.insert(&sample_record(100, 3)).unwrap();
        store.insert(&sample_record(200, 7)).unwrap();

        let records = store.list(UsageRange::default()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].finished_at_ms, 200);
        assert_eq!(store.summary().unwrap().tokens_out, 10);

        assert_eq!(store.prune_to_latest(1).unwrap(), 1);
        let remaining = store.list(UsageRange::default()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].finished_at_ms, 200);
    }

    /// **触顶归属必须真的落库、真的读得回来，且真的被数出来**（R1.4）。
    ///
    /// 计划文档写着「用户出口 = 本机 `UsageRecord` 同批带该字段」，而这张表此前只有 8 列，
    /// 一条触顶记录与一条正常完成的记录在库里**一模一样**——`terminal_status` 两边都是
    /// `finished`（触顶是完成 + 标注，不是失败，ADR-089 ④）。于是「我这个月有几篇被截断
    /// 了」在本机没有任何地方回答得了。
    #[test]
    fn a_capped_run_is_recorded_with_its_attribution_and_counted_in_the_summary() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        store.insert(&sample_record(100, 3)).unwrap();
        let mut capped = sample_record(200, 7);
        capped.output_truncated = Some(TruncatedBy::Upstream);
        store.insert(&capped).unwrap();

        let records = store.list(UsageRange::default()).unwrap();
        assert_eq!(records[0].output_truncated, Some(TruncatedBy::Upstream));
        assert_eq!(records[1].output_truncated, None);
        // 两条都是 `finished`：不单独数一次的话，触顶在这张表上没有任何痕迹。
        assert_eq!(records[0].terminal_status, "finished");
        let summary = store.summary().unwrap();
        assert_eq!(summary.records, 2);
        assert_eq!(summary.truncated, 1);
    }

    /// v1 库升到 v2 只加一列，**历史记录一条不丢**。
    ///
    /// `usage.sqlite` 是 durable 历史事实（与可重建的 `index.sqlite` 分开正是为此）：
    /// 为了加一列就整张重建，用户失去的是「这几个月我用了多少」。
    #[test]
    fn opening_a_v1_usage_db_keeps_its_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL, origin TEXT NOT NULL, capability TEXT NOT NULL,
                provider_id TEXT NOT NULL, model_id TEXT, audio_seconds REAL,
                tokens_in INTEGER, tokens_out INTEGER, est_cost_minor INTEGER,
                terminal_status TEXT NOT NULL, finished_at_ms INTEGER NOT NULL);
             INSERT INTO usage_records
                (job_id, origin, capability, provider_id, terminal_status, tokens_out, finished_at_ms)
             VALUES ('01890a5d-ac96-774b-bcce-b302099a8057', 'floating_menu', 'refine',
                     'local-llm', 'finished', 5, 100);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        let summary = store.summary().unwrap();
        assert_eq!(summary.records, 1, "升级把历史记录整张重建掉了");
        assert_eq!(summary.tokens_out, 5);
        assert_eq!(summary.truncated, 0);
        // 升级后照常写得进新列。
        let mut capped = sample_record(300, 2);
        capped.output_truncated = Some(TruncatedBy::Absolute);
        store.insert(&capped).unwrap();
        assert_eq!(store.summary().unwrap().truncated, 1);
        // v1 是**一次跨两版**的升级（v1 → v3）：逐版分支写法会在这里漏掉后一版的列，
        // 而漏掉的表现只有「从 v1 直升的那批用户读不出档位」——只有这条断言看得见它。
        let migrated = store.list(UsageRange::default()).unwrap();
        let historical = migrated
            .iter()
            .find(|record| record.finished_at_ms == 100)
            .expect("历史行还在");
        assert_eq!(
            historical.service_class, None,
            "补列之前的历史行没有档位，不许猜一个"
        );
        assert_eq!(historical.error_code, None);
    }

    /// v2 → v3：补上「哪一档 + 什么码」两列，历史记录同样一条不丢。
    ///
    /// 这两列是 2026-08-31 那次真机排查的直接产物：弹窗翻译连续失败 9 次，本机账本上
    /// 确实有 9 行，而每一行都是 `provider_id = 'unresolved'` + `terminal_status = 'failed'`
    /// ——答不出哪条腿、哪一档、什么码，于是排查只能靠复现。
    #[test]
    fn opening_a_v2_usage_db_gains_the_lane_and_error_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL, origin TEXT NOT NULL, capability TEXT NOT NULL,
                provider_id TEXT NOT NULL, model_id TEXT, audio_seconds REAL,
                tokens_in INTEGER, tokens_out INTEGER, est_cost_minor INTEGER,
                terminal_status TEXT NOT NULL, output_truncated TEXT,
                finished_at_ms INTEGER NOT NULL);
             INSERT INTO usage_records
                (job_id, origin, capability, provider_id, terminal_status, tokens_out, finished_at_ms)
             VALUES ('01890a5d-ac96-774b-bcce-b302099a8058', 'popup_translate', 'translate',
                     'unresolved', 'failed', 0, 100);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        let records = store.list(UsageRange::default()).unwrap();
        assert_eq!(records.len(), 1, "升级把历史记录整张重建掉了");
        assert_eq!(records[0].service_class, None);
        assert_eq!(records[0].error_code, None);

        // 新写入的失败记录必须答得出那三个问题。
        let mut failed = sample_record(200, 0);
        failed.terminal_status = "failed".into();
        failed.error_code = Some("SERVER_SIDE_FAILURE".into());
        failed.service_class = Some(luna_core::ServiceClass::Realtime);
        store.insert(&failed).unwrap();
        let records = store.list(UsageRange::default()).unwrap();
        assert_eq!(
            records[0].error_code.as_deref(),
            Some("SERVER_SIDE_FAILURE")
        );
        assert_eq!(
            records[0].service_class,
            Some(luna_core::ServiceClass::Realtime)
        );
    }

    #[test]
    fn corrupt_usage_db_recovers_to_empty_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        std::fs::write(&path, b"not sqlite").unwrap();

        let store = UsageStore::open(&path).unwrap();

        assert_eq!(store.summary().unwrap().records, 0);
        assert!(dir.path().join("usage.sqlite.corrupt").exists());
    }

    #[test]
    fn ai_outcomes_count_landed_and_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        for (landing, at) in [("replace", 1), ("insert_below", 2), ("discard", 3)] {
            store
                .insert_outcome(&AiOutcome {
                    origin: JobOrigin::FloatingMenu,
                    action: "optimize".into(),
                    landing: landing.into(),
                    recorded_at_ms: at,
                })
                .unwrap();
        }
        assert_eq!(
            store.outcome_summary().unwrap(),
            AiOutcomeSummary {
                landed: 2,
                discarded: 1
            }
        );
    }

    /// v3 库升到 v4 只加一张回执表，历史用量记录一条不丢。
    #[test]
    fn opening_a_v3_usage_db_adds_outcomes_table_and_keeps_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL, origin TEXT NOT NULL, capability TEXT NOT NULL,
                provider_id TEXT NOT NULL, model_id TEXT, audio_seconds REAL,
                tokens_in INTEGER, tokens_out INTEGER, est_cost_minor INTEGER,
                terminal_status TEXT NOT NULL, output_truncated TEXT, service_class TEXT,
                error_code TEXT, finished_at_ms INTEGER NOT NULL);
             INSERT INTO usage_records
                (job_id, origin, capability, provider_id, terminal_status, tokens_out, finished_at_ms)
             VALUES ('01890a5d-ac96-774b-bcce-b302099a8057', 'floating_menu', 'refine',
                     'local-llm', 'finished', 5, 100);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        assert_eq!(
            store.summary().unwrap().records,
            1,
            "升级把历史记录整张重建掉了"
        );
        store
            .insert_outcome(&AiOutcome {
                origin: JobOrigin::FloatingMenu,
                action: "ask".into(),
                landing: "replace".into(),
                recorded_at_ms: 1,
            })
            .unwrap();
        assert_eq!(store.outcome_summary().unwrap().landed, 1);
    }

    /// v4 → v5：加历史表，用量记录与落地回执原样保留。
    #[test]
    fn opening_a_v4_usage_db_adds_queue_outcomes_and_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let store = UsageStore::open(&path).unwrap();
        store
            .insert_outcome(&AiOutcome {
                origin: JobOrigin::FloatingMenu,
                action: "ask".into(),
                landing: "replace".into(),
                recorded_at_ms: 1,
            })
            .unwrap();
        drop(store);
        // 退回 v4 形状（把新表拆掉、版本号回拨），再打开走升级路径。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE queue_outcomes;").unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        assert_eq!(
            store.outcome_summary().unwrap().landed,
            1,
            "升级丢了落地回执"
        );
        let id = store
            .insert_queue_outcome(&QueueOutcome {
                id: None,
                kind: "distill".into(),
                label: "讲座".into(),
                note_rel: Some("Notes/a.md".into()),
                outcome: "failed".into(),
                code: Some("QUOTA_EXHAUSTED".into()),
                finished_at_ms: 10,
            })
            .unwrap();
        let rows = store.list_queue_outcomes(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, Some(id));
        assert_eq!(rows[0].code.as_deref(), Some("QUOTA_EXHAUSTED"));
    }

    /// v5 → v6：加播放位置表，队列历史与用量记录原样保留。
    #[test]
    fn opening_a_v5_usage_db_adds_playback_positions_and_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let store = UsageStore::open(&path).unwrap();
        store
            .insert_queue_outcome(&QueueOutcome {
                id: None,
                kind: "distill".into(),
                label: "讲座".into(),
                note_rel: Some("Notes/a.md".into()),
                outcome: "failed".into(),
                code: None,
                finished_at_ms: 10,
            })
            .unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE playback_positions;")
            .unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();
        assert_eq!(
            store.list_queue_outcomes(10).unwrap().len(),
            1,
            "升级丢了队列历史"
        );
        store
            .save_playback_position("/v", "Recordings/a.md", 4200)
            .unwrap();
        assert_eq!(
            store
                .load_playback_position("/v", "Recordings/a.md")
                .unwrap(),
            Some(4200)
        );
    }

    /// 播放位置：同一篇覆盖不累积；不同库同一相对路径互不串；没记过是 `None`；负数夹到 0。
    #[test]
    fn playback_positions_are_keyed_by_vault_and_note_and_overwrite_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        assert_eq!(store.load_playback_position("/a", "R/x.md").unwrap(), None);
        store.save_playback_position("/a", "R/x.md", 1000).unwrap();
        store.save_playback_position("/a", "R/x.md", 2500).unwrap();
        store.save_playback_position("/b", "R/x.md", -7).unwrap();
        assert_eq!(
            store.load_playback_position("/a", "R/x.md").unwrap(),
            Some(2500)
        );
        assert_eq!(
            store.load_playback_position("/b", "R/x.md").unwrap(),
            Some(0)
        );
    }

    /// 隐私断言（PRD-04-062）：历史表只有这七列——任何一列都装不下载荷路径 / URL / 正文。
    /// 新增字段必须先改这份清单，再回答「它是不是载荷」。
    #[test]
    fn queue_outcomes_table_has_exactly_the_declared_columns() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        assert_eq!(
            store.queue_outcome_columns().unwrap(),
            QUEUE_OUTCOME_COLUMNS
        );
    }

    /// 条数 + 天数双封顶，逐条删除与清空。
    #[test]
    fn queue_outcomes_prune_delete_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.sqlite")).unwrap();
        let row = |ms: i64| QueueOutcome {
            id: None,
            kind: "import".into(),
            label: format!("f{ms}"),
            note_rel: None,
            outcome: "cancelled".into(),
            code: None,
            finished_at_ms: ms,
        };
        let ids: Vec<i64> = (1..=6)
            .map(|ms| store.insert_queue_outcome(&row(ms)).unwrap())
            .collect();
        // 天数封顶：早于 3 的两条淘汰；条数封顶：只留最新 3 条 → 剩 4,5,6。
        store.prune_queue_outcomes(3, 3).unwrap();
        let left: Vec<i64> = store
            .list_queue_outcomes(10)
            .unwrap()
            .iter()
            .map(|r| r.finished_at_ms)
            .collect();
        assert_eq!(left, vec![6, 5, 4]);
        assert!(store.delete_queue_outcome(ids[5]).unwrap());
        assert!(
            !store.delete_queue_outcome(ids[0]).unwrap(),
            "已淘汰的行再删返回 false"
        );
        assert_eq!(store.list_queue_outcomes(10).unwrap().len(), 2);
        assert_eq!(store.clear_queue_outcomes().unwrap(), 2);
        assert!(store.list_queue_outcomes(10).unwrap().is_empty());
    }

    #[test]
    fn unknown_usage_db_version_recovers_to_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);

        let store = UsageStore::open(&path).unwrap();

        assert_eq!(store.summary().unwrap().records, 0);
    }
}
