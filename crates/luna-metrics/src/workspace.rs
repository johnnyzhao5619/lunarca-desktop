//! 工作区任务观测（PRD-07-070 / PRD-09-076 / TDD-12 §2.1）。
//!
//! **这不是埋点 SDK，也不是知识度量的第二个租户**：它有自己的数据库文件、自己的开关、
//! 自己的清除边界。用一个开关启停两类记录，等于用户为 A 打开的同意被拿去做了 B。
//!
//! **库里只有"哪一天、哪个封闭维度、哪个耗时桶、多少次"**——没有逐条事件、没有精确
//! 时间、没有会话/Job/Vault id、没有任何内容派生的 token。因此它也**不需要设备密钥**：
//! 没有可关联值要保护，凭空引入一个 Keychain 依赖只会多一处会失败的地方。
//!
//! 这条边界不是"少存点更安全"的直觉，而是**结构性的**：只存计数的库，即使整个泄露出去
//! 也重建不出任何一个用户做过什么；存了逐条事件的库做不到这一点，无论字段怎么脱敏。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::MetricsError;

/// 库的 schema 代际。
///
/// **加一张表、拆一个封闭维度都必须抬升它**（TDD-12 §2.1）：不抬升的表现不是报错，
/// 而是**数悄悄少了**——旧库里没有新表，读取侧一路读到空，界面上写着"这个分区没人去过"。
/// 抬升之后旧库按既有 fail closed 判据被拒（不迁移、不双读双写，ADR-046），用户经显式
/// 清除重建，起点即**新的观测周期**——清除前后的数字不可并排比较。
const SCHEMA_VERSION: i64 = 3;
const DAY_MS: i64 = 86_400_000;

/// 这个 schema 代际下库里**应当有且只有**的表，按 `ORDER BY name` 排好。
///
/// **它必须与 [`WorkspaceMetrics::create_schema`] 共用同一份**，不能在 `open` 里再抄一遍
/// 字面量：那份抄件依赖的是"我手写的顺序恰好等于 SQLite 的 `ORDER BY name`"。加一张表时
/// 顺序写错的表现是**新建的空库当场 `UnknownSchema`**——功能全灭，而且与"旧库 fail closed"
/// 的现象一模一样，归因极难。共用一份之后，"建出来的库能不能被打开"由一条测试直接回答。
const EXPECTED_TABLES: [&str; 5] = [
    "workspace_entries",
    "workspace_imports",
    "workspace_settled",
    "workspace_status_visits",
    "workspace_surfaces",
];

/// 库文件名——**删除与打开必须由同一个地方算出它**（见 [`WorkspaceMetrics::destroy`]）。
const DATABASE_FILE: &str = "workspace-metrics.sqlite";

/// 保留多少个 UTC 日（TDD-12 §2.1）。
///
/// 有界保留不是可选项：无限增长的观测库迟早成为一份"这台设备三年来的使用史"，而它的全部
/// 用途只是回答"最近这阵子任务好不好完成"。
pub const RETENTION_DAYS: i64 = 90;

/// 默认查询窗口（UTC 日）。
pub const DEFAULT_RANGE_DAYS: i64 = 30;

/// 比率至少要几个终结样本才给数值。
///
/// 少于这个数只给计数、不给比率：3 个样本里 1 个失败会显示成 33%，而它什么都不说明。
/// 小样本比率是本地观测最容易骗到自己的地方。
pub const MINIMUM_RATIO_SAMPLES: u32 = 5;

/// 用户可以去到的界面（封闭维度）。
///
/// **它是观测维度，不是导航实现的镜像**：值域跟着 `WORKSPACE_IA` 的位置走，但**不**
/// 由前端传字符串——传字符串意味着某天多一个界面时，库里会静静多出一个没人认识的值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WorkspaceSurface {
    Search,
    Notes,
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

/// 用户从哪个入口发起了一次创建（封闭维度）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum EntrySurface {
    AddContent,
    QuickStart,
    Sidebar,
    StatusBar,
    DeepLink,
    Shortcut,
    /// 系统托盘菜单（应用可能整个不在前台，PRD-06-058 B″ 的第二条全局入口）。
    Tray,
}

/// 三条状态路径各自的展示面（封闭维度）。
///
/// **它与 [`WorkspaceSurface`] 是两个维度，不是一个**：状态面回答的是"现在什么在处理 /
/// 有什么值得知道 / 这段时间花了多少"，它们覆盖在当前上下文之上，不是一级导航的目的地。
/// 混进同一个枚举里，"到访了 12 次通知"就会与"到访了 12 次搜索"并排出现在同一张表里，
/// 而它们的分母含义完全不同。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum StatusSurface {
    Notifications,
    Queue,
    Usage,
}

/// 一次创建尝试的**终态**。
///
/// 只有四个终态，且都是"已经结束"的：进行中的尝试不进库——那会让分母随时间漂移，
/// 同一天的比率每看一次都不一样。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum EntryResult {
    /// 到达了目的地（产物落成）。
    Arrived,
    /// 用户自己取消。
    Cancelled,
    /// 被拒绝（权限、算力、库未就绪…）。
    Rejected,
    /// 失败。
    Failed,
}

impl EntryResult {
    /// 分子只算"到达"，分母算全部四个终态。
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Arrived)
    }
}

/// 耗时桶（左闭右开，首桶从 0 起）。
///
/// **只存桶，不存毫秒**：精确耗时加上日期就足以还原"这个人那天几点在干什么"的节奏，
/// 而我们要回答的问题（"这件事一般要多久"）桶就够了。原始毫秒只在内存里活到分桶那一刻。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DurationBucket {
    Under1s,
    From1sTo10s,
    From10sTo60s,
    From1mTo5m,
    AtLeast5m,
    /// 调用方没有可信的耗时（如跨重启续跑）——诚实留空，不猜一个。
    Unknown,
}

impl DurationBucket {
    /// 毫秒 → 桶。
    #[must_use]
    pub fn of_ms(duration_ms: u32) -> Self {
        match duration_ms {
            ms if ms < 1_000 => Self::Under1s,
            ms if ms < 10_000 => Self::From1sTo10s,
            ms if ms < 60_000 => Self::From10sTo60s,
            ms if ms < 300_000 => Self::From1mTo5m,
            _ => Self::AtLeast5m,
        }
    }

    /// 毫秒 → 桶。`None` = 没有可信耗时。
    ///
    /// **只有跨进程/跨启动的后端任务才配拿到 `None`**（TDD-12 §2.1：「跨进程任务缺完整
    /// 计时记 `unknown`」）。前端上报的事实一律带着一个真实的毫秒数——它就在同一个 tick 里
    /// 拿到的两个时间点之差，没有"不知道"这回事。
    #[must_use]
    pub fn of(duration_ms: Option<u32>) -> Self {
        duration_ms.map_or(Self::Unknown, Self::of_ms)
    }

    /// 是不是"短停留"那一侧（< 10s）。
    ///
    /// **桶边界恰好落在 10s 上，所以从桶反推是精确的、不是近似**：`from_1s_to_10s` 是
    /// 左闭右开的 `[1s, 10s)`。若哪天桶边界改了而这里没跟上，`short_without_action` 会
    /// 悄悄换一个含义——故它写在桶自己身上，而不是在查询里写一串桶名。
    #[must_use]
    pub const fn is_under_10s(self) -> bool {
        matches!(self, Self::Under1s | Self::From1sTo10s)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Under1s => "under_1s",
            Self::From1sTo10s => "from_1s_to_10s",
            Self::From10sTo60s => "from_10s_to_60s",
            Self::From1mTo5m => "from_1m_to_5m",
            Self::AtLeast5m => "at_least_5m",
            Self::Unknown => "unknown",
        }
    }
}

/// 一条已经完成的工作区事实。
///
/// **没有自由文本、没有 id、没有路径**：`repair` 深链与对象路径显式不在这里（PRD-09-076 /
/// TDD-12 §2.1）——修复深链里带着分组名与键名，而路径就是内容本身的一半。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// 到过某个界面。
    SurfaceViewed { surface: WorkspaceSurface },
    /// 一次创建尝试结束了。
    CreateEntryFinished {
        /// 五个稳定 action id 之一（调用方保证，库里只存这个字符串）。
        action: &'static str,
        entry: EntrySurface,
        result: EntryResult,
        /// **必填**：这条事实由前端在同一个 tick 里算出来，"没有可信耗时"是后端跨进程
        /// 任务才有的情形（见 [`Self::MaterialImportFinished`]），不是它能表达的东西。
        duration_ms: u32,
    },
    /// 一次状态面到访结束了（TDD-12 §2.1）。
    ///
    /// `interacted` = 这一次到访里用户**真的操作过**面板内容（不是只打开又关掉）；
    /// `duration_ms` 从打开算到关闭。两者一起才拼得出"短停留未操作"这个代理指标——
    /// 它只是代理，不能被称作"用户困惑的原因"（§2.1 末条）。
    StatusVisitFinished {
        surface: StatusSurface,
        interacted: bool,
        duration_ms: u32,
    },
    /// 一条待整理内容被转成了正式笔记。
    ///
    /// **只给次数，不除以本期新建数**：那个比值的分母跨越了本次授权区间之外的历史，
    /// 算出来的"转化率"没有任何一个诚实的解释（TDD-12 §2.1）。
    QuickNoteSettled,
    /// 一次资料导入结束了（`arrived` = 落盘且全文索引已提交，真的搜得到）。
    MaterialImportFinished {
        result: EntryResult,
        duration_ms: Option<u32>,
    },
}

/// 设备本地的工作区观测库。
pub struct WorkspaceMetrics {
    conn: Connection,
    path: PathBuf,
}

/// 一天里某个维度的计数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SurfaceCount {
    pub surface: WorkspaceSurface,
    pub count: u32,
}

/// 一个入口 × 一个动作的终态分布。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntryAggregate {
    pub action: String,
    pub entry: EntrySurface,
    pub arrived: u32,
    pub cancelled: u32,
    pub rejected: u32,
    pub failed: u32,
    /// 每个桶的次数（只含有计数的桶）。
    pub durations: Vec<(DurationBucket, u32)>,
}

impl EntryAggregate {
    /// 分母 = 四个终态之和。
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.arrived + self.cancelled + self.rejected + self.failed
    }

    /// 到达率；样本不足时 `None`（见 [`MINIMUM_RATIO_SAMPLES`]）。
    #[must_use]
    pub fn arrival_ratio(&self) -> Option<f64> {
        let total = self.total();
        (total >= MINIMUM_RATIO_SAMPLES).then(|| f64::from(self.arrived) / f64::from(total))
    }
}

/// 一个状态面的到访分布。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatusVisitAggregate {
    pub surface: StatusSurface,
    /// 完成的到访次数（分母）。
    pub total: u32,
    /// 其中 `interacted == false && duration < 10s` 的那些（TDD-12 §2.1 逐字）。
    pub short_without_action: u32,
    /// 每个桶的次数（只含有计数的桶）。
    pub durations: Vec<(DurationBucket, u32)>,
}

impl StatusVisitAggregate {
    /// 短停留未操作占比；样本不足时 `None`（口径与 [`EntryAggregate::arrival_ratio`] 一致）。
    #[must_use]
    pub fn short_without_action_ratio(&self) -> Option<f64> {
        (self.total >= MINIMUM_RATIO_SAMPLES)
            .then(|| f64::from(self.short_without_action) / f64::from(self.total))
    }
}

/// 一次查询的结果。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WorkspaceSummary {
    pub surfaces: Vec<SurfaceCount>,
    pub entries: Vec<EntryAggregate>,
    pub quick_notes_settled: u32,
    pub imports: EntryAggregate,
    pub status_visits: Vec<StatusVisitAggregate>,
}

impl Default for EntryAggregate {
    fn default() -> Self {
        Self {
            action: String::new(),
            entry: EntrySurface::AddContent,
            arrived: 0,
            cancelled: 0,
            rejected: 0,
            failed: 0,
            durations: Vec::new(),
        }
    }
}

fn day_of(now_ms: i64) -> i64 {
    now_ms.div_euclid(DAY_MS)
}

fn surface_str(surface: WorkspaceSurface) -> &'static str {
    match surface {
        WorkspaceSurface::Search => "search",
        WorkspaceSurface::Notes => "notes",
        WorkspaceSurface::KnowledgeTopics => "knowledge_topics",
        WorkspaceSurface::KnowledgeTags => "knowledge_tags",
        WorkspaceSurface::KnowledgeNetwork => "knowledge_network",
        WorkspaceSurface::QuickNotes => "quick_notes",
        WorkspaceSurface::Recordings => "recordings",
        WorkspaceSurface::Transcripts => "transcripts",
        WorkspaceSurface::Chat => "chat",
        WorkspaceSurface::Creator => "creator",
        WorkspaceSurface::Apps => "apps",
        WorkspaceSurface::Trash => "trash",
        WorkspaceSurface::Settings => "settings",
    }
}

fn surface_from(text: &str) -> Option<WorkspaceSurface> {
    Some(match text {
        "search" => WorkspaceSurface::Search,
        "notes" => WorkspaceSurface::Notes,
        "knowledge_topics" => WorkspaceSurface::KnowledgeTopics,
        "knowledge_tags" => WorkspaceSurface::KnowledgeTags,
        "knowledge_network" => WorkspaceSurface::KnowledgeNetwork,
        "quick_notes" => WorkspaceSurface::QuickNotes,
        "recordings" => WorkspaceSurface::Recordings,
        "transcripts" => WorkspaceSurface::Transcripts,
        "chat" => WorkspaceSurface::Chat,
        "creator" => WorkspaceSurface::Creator,
        "apps" => WorkspaceSurface::Apps,
        "trash" => WorkspaceSurface::Trash,
        "settings" => WorkspaceSurface::Settings,
        _ => return None,
    })
}

fn entry_str(entry: EntrySurface) -> &'static str {
    match entry {
        EntrySurface::AddContent => "add_content",
        EntrySurface::QuickStart => "quick_start",
        EntrySurface::Sidebar => "sidebar",
        EntrySurface::StatusBar => "status_bar",
        EntrySurface::DeepLink => "deep_link",
        EntrySurface::Shortcut => "shortcut",
        EntrySurface::Tray => "tray",
    }
}

fn entry_from(text: &str) -> Option<EntrySurface> {
    Some(match text {
        "add_content" => EntrySurface::AddContent,
        "quick_start" => EntrySurface::QuickStart,
        "sidebar" => EntrySurface::Sidebar,
        "status_bar" => EntrySurface::StatusBar,
        "deep_link" => EntrySurface::DeepLink,
        "shortcut" => EntrySurface::Shortcut,
        "tray" => EntrySurface::Tray,
        _ => return None,
    })
}

fn status_surface_str(surface: StatusSurface) -> &'static str {
    match surface {
        StatusSurface::Notifications => "notifications",
        StatusSurface::Queue => "queue",
        StatusSurface::Usage => "usage",
    }
}

fn status_surface_from(text: &str) -> Option<StatusSurface> {
    Some(match text {
        "notifications" => StatusSurface::Notifications,
        "queue" => StatusSurface::Queue,
        "usage" => StatusSurface::Usage,
        _ => return None,
    })
}

fn result_str(result: EntryResult) -> &'static str {
    match result {
        EntryResult::Arrived => "arrived",
        EntryResult::Cancelled => "cancelled",
        EntryResult::Rejected => "rejected",
        EntryResult::Failed => "failed",
    }
}

/// 库里的桶名 → 桶。
///
/// **不认识的桶名不折成 `unknown`**：`unknown` 是一个**有含义**的取值（"没有可信耗时"），
/// 把读不懂的东西折进去等于给它编一个含义。读不懂就是读不懂，见 [`unknown_dimension`]。
fn bucket_from(text: &str) -> Option<DurationBucket> {
    Some(match text {
        "under_1s" => DurationBucket::Under1s,
        "from_1s_to_10s" => DurationBucket::From1sTo10s,
        "from_10s_to_60s" => DurationBucket::From10sTo60s,
        "from_1m_to_5m" => DurationBucket::From1mTo5m,
        "at_least_5m" => DurationBucket::AtLeast5m,
        "unknown" => DurationBucket::Unknown,
        _ => return None,
    })
}

/// 库里出现了这一代 schema 不认识的维度取值。
///
/// **报错而不是跳过那一行**（TDD-12 §2.1「计数溢出报存储故障，不绕回/静默饱和」的同一
/// 精神）：静默丢行的表现是**数悄悄少了**——界面上写着"这个分区没人去过"，而库里明明记着。
/// 一个看不出错的错，比一句"读不了"贵得多。这在实践中只可能来自 schema 代际错配或人为
/// 改库，两者都该走 fail closed。
const fn unknown_dimension() -> MetricsError {
    MetricsError::InvalidValue("workspace metrics database holds an unknown dimension value")
}

impl WorkspaceMetrics {
    /// 数据库路径——**与知识度量各用各的文件**（TDD-12 §2.1）。
    #[must_use]
    pub fn database_path(app_private_dir: impl AsRef<Path>) -> PathBuf {
        app_private_dir.as_ref().join(DATABASE_FILE)
    }

    /// 打开（不存在即建）。schema 对不上**fail closed**：不迁移、不自动删库。
    pub fn open(app_private_dir: impl AsRef<Path>) -> Result<Self, MetricsError> {
        let dir = app_private_dir.as_ref();
        fs::create_dir_all(dir)?;
        let path = Self::database_path(dir);
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master
             WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let tables = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        match (version, tables.is_empty()) {
            (0, true) => Self::create_schema(&conn)?,
            (SCHEMA_VERSION, false) if tables == EXPECTED_TABLES => {}
            (other, _) => return Err(MetricsError::UnknownSchema(other)),
        }
        Ok(Self { conn, path })
    }

    fn create_schema(conn: &Connection) -> Result<(), MetricsError> {
        // 这几张表**全是计数表**，主键即维度组合：没有一行代表"某一次"，故也没有任何
        // 一行可以被还原成"谁在什么时候做了什么"。
        conn.execute_batch(
            "CREATE TABLE workspace_surfaces (
               day INTEGER NOT NULL,
               surface TEXT NOT NULL,
               count INTEGER NOT NULL,
               PRIMARY KEY (day, surface)
             ) WITHOUT ROWID;
             CREATE TABLE workspace_entries (
               day INTEGER NOT NULL,
               action TEXT NOT NULL,
               entry TEXT NOT NULL,
               result TEXT NOT NULL,
               bucket TEXT NOT NULL,
               count INTEGER NOT NULL,
               PRIMARY KEY (day, action, entry, result, bucket)
             ) WITHOUT ROWID;
             CREATE TABLE workspace_settled (
               day INTEGER NOT NULL PRIMARY KEY,
               count INTEGER NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE workspace_imports (
               day INTEGER NOT NULL,
               result TEXT NOT NULL,
               bucket TEXT NOT NULL,
               count INTEGER NOT NULL,
               PRIMARY KEY (day, result, bucket)
             ) WITHOUT ROWID;
             CREATE TABLE workspace_status_visits (
               day INTEGER NOT NULL,
               surface TEXT NOT NULL,
               interacted INTEGER NOT NULL,
               bucket TEXT NOT NULL,
               count INTEGER NOT NULL,
               PRIMARY KEY (day, surface, interacted, bucket)
             ) WITHOUT ROWID;",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// 记一条（`now_ms` 由调用方给——本 crate 不读时钟，测试才能验保留窗口）。
    pub fn record(&self, event: WorkspaceEvent, now_ms: i64) -> Result<(), MetricsError> {
        let day = day_of(now_ms);
        match event {
            WorkspaceEvent::SurfaceViewed { surface } => self.conn.execute(
                "INSERT INTO workspace_surfaces (day, surface, count) VALUES (?1, ?2, 1)
                 ON CONFLICT(day, surface) DO UPDATE SET count = count + 1",
                params![day, surface_str(surface)],
            )?,
            WorkspaceEvent::CreateEntryFinished {
                action,
                entry,
                result,
                duration_ms,
            } => self.conn.execute(
                "INSERT INTO workspace_entries (day, action, entry, result, bucket, count)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)
                 ON CONFLICT(day, action, entry, result, bucket) DO UPDATE SET count = count + 1",
                params![
                    day,
                    action,
                    entry_str(entry),
                    result_str(result),
                    DurationBucket::of_ms(duration_ms).as_str()
                ],
            )?,
            WorkspaceEvent::StatusVisitFinished {
                surface,
                interacted,
                duration_ms,
            } => self.conn.execute(
                "INSERT INTO workspace_status_visits (day, surface, interacted, bucket, count)
                 VALUES (?1, ?2, ?3, ?4, 1)
                 ON CONFLICT(day, surface, interacted, bucket) DO UPDATE SET count = count + 1",
                params![
                    day,
                    status_surface_str(surface),
                    i64::from(interacted),
                    DurationBucket::of_ms(duration_ms).as_str()
                ],
            )?,
            WorkspaceEvent::QuickNoteSettled => self.conn.execute(
                "INSERT INTO workspace_settled (day, count) VALUES (?1, 1)
                 ON CONFLICT(day) DO UPDATE SET count = count + 1",
                params![day],
            )?,
            WorkspaceEvent::MaterialImportFinished {
                result,
                duration_ms,
            } => self.conn.execute(
                "INSERT INTO workspace_imports (day, result, bucket, count) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(day, result, bucket) DO UPDATE SET count = count + 1",
                params![
                    day,
                    result_str(result),
                    DurationBucket::of(duration_ms).as_str()
                ],
            )?,
        };
        self.prune(day)?;
        Ok(())
    }

    /// 丢弃超出保留窗口的日子（每次写入顺手做——不另起后台任务，那会是第二处会失败的地方）。
    fn prune(&self, today: i64) -> Result<(), MetricsError> {
        let oldest = today - RETENTION_DAYS + 1;
        for table in EXPECTED_TABLES {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE day < ?1"),
                params![oldest],
            )?;
        }
        Ok(())
    }

    /// 查一段窗口（含两端的 UTC 日序号）。
    pub fn summary(&self, start_day: i64, end_day: i64) -> Result<WorkspaceSummary, MetricsError> {
        if start_day > end_day {
            return Err(MetricsError::InvalidValue("range start after end"));
        }
        let mut summary = WorkspaceSummary::default();

        let mut stmt = self.conn.prepare(
            "SELECT surface, SUM(count) FROM workspace_surfaces
             WHERE day BETWEEN ?1 AND ?2 GROUP BY surface ORDER BY surface",
        )?;
        for row in stmt.query_map(params![start_day, end_day], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (text, count) = row?;
            let surface = surface_from(&text).ok_or_else(unknown_dimension)?;
            summary.surfaces.push(SurfaceCount {
                surface,
                count: u32::try_from(count).unwrap_or(u32::MAX),
            });
        }
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT action, entry, result, bucket, SUM(count) FROM workspace_entries
             WHERE day BETWEEN ?1 AND ?2 GROUP BY action, entry, result, bucket
             ORDER BY action, entry, result, bucket",
        )?;
        let rows = stmt
            .query_map(params![start_day, end_day], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (action, entry_text, result, bucket, count) in rows {
            let entry = entry_from(&entry_text).ok_or_else(unknown_dimension)?;
            let slot = match summary
                .entries
                .iter_mut()
                .find(|it| it.action == action && it.entry == entry)
            {
                Some(existing) => existing,
                None => {
                    summary.entries.push(EntryAggregate {
                        action: action.clone(),
                        entry,
                        ..EntryAggregate::default()
                    });
                    summary.entries.last_mut().expect("just pushed")
                }
            };
            bump(slot, &result, &bucket, count)?;
        }

        summary.quick_notes_settled = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(count), 0) FROM workspace_settled WHERE day BETWEEN ?1 AND ?2",
                params![start_day, end_day],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))?;

        let mut stmt = self.conn.prepare(
            "SELECT result, bucket, SUM(count) FROM workspace_imports
             WHERE day BETWEEN ?1 AND ?2 GROUP BY result, bucket ORDER BY result, bucket",
        )?;
        for row in stmt.query_map(params![start_day, end_day], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (result, bucket, count) = row?;
            bump(&mut summary.imports, &result, &bucket, count)?;
        }
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT surface, interacted, bucket, SUM(count) FROM workspace_status_visits
             WHERE day BETWEEN ?1 AND ?2 GROUP BY surface, interacted, bucket
             ORDER BY surface, interacted, bucket",
        )?;
        let visits = stmt
            .query_map(params![start_day, end_day], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for (surface_text, interacted, bucket_text, count) in visits {
            let surface = status_surface_from(&surface_text).ok_or_else(unknown_dimension)?;
            let bucket = bucket_from(&bucket_text).ok_or_else(unknown_dimension)?;
            let n = u32::try_from(count).unwrap_or(u32::MAX);
            let slot = match summary
                .status_visits
                .iter_mut()
                .find(|it| it.surface == surface)
            {
                Some(existing) => existing,
                None => {
                    summary.status_visits.push(StatusVisitAggregate {
                        surface,
                        total: 0,
                        short_without_action: 0,
                        durations: Vec::new(),
                    });
                    summary.status_visits.last_mut().expect("just pushed")
                }
            };
            slot.total += n;
            // 判据逐字来自 §2.1：**没操作过 且 不足 10 秒**。两者缺一都不算——只是停留
            // 很短却真的按了东西，那是"很快就办完了"；停留很久没按，那是在读。
            if !interacted && bucket.is_under_10s() {
                slot.short_without_action += n;
            }
            match slot.durations.iter_mut().find(|(b, _)| *b == bucket) {
                Some((_, existing)) => *existing += n,
                None => slot.durations.push((bucket, n)),
            }
        }
        Ok(summary)
    }

    /// 彻底删除这个目录里的工作区观测库（主文件 + WAL + SHM）——**唯一的删除实现**。
    ///
    /// **它不解析 schema**：坏 schema 一律 fail closed（[`Self::open`]），而清除重建正是
    /// §2.1 给出的**唯一恢复路径**。若删除也要先 `open` 一次，坏 schema 的用户就永远删不掉
    /// 那个库——面板永远显示 `schema_invalid`、清除永远失败，fail closed 从"我们不动你的
    /// 东西"变成"你也别想动"。
    ///
    /// **要删哪个文件只由 [`Self::database_path`] 回答**：调用方给的是**目录**，文件名
    /// 由这里自己算。因此这条通路结构上不可能删到知识度量库（那是同一目录下的另一个文件名）
    /// ——两套观测各有各的清除边界，用户按的是哪颗按钮就只删哪一份。
    pub fn destroy(app_private_dir: impl AsRef<Path>) -> Result<(), MetricsError> {
        let path = Self::database_path(app_private_dir);
        for candidate in [
            path.clone(),
            crate::sidecar(&path, "-wal"),
            crate::sidecar(&path, "-shm"),
        ] {
            match fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // **失败如实报错，不能报已清除**。
                Err(error) => return Err(MetricsError::Io(error)),
            }
        }
        Ok(())
    }

    /// 关掉这个连接并清除它的库：`drop` 之后就是 [`Self::destroy`]，**不另写一份删除逻辑**。
    pub fn clear(self) -> Result<(), MetricsError> {
        let dir = self
            .path
            .parent()
            .ok_or(MetricsError::InvalidValue("metrics database has no parent"))?
            .to_path_buf();
        debug_assert_eq!(Self::database_path(&dir), self.path);
        drop(self.conn);
        Self::destroy(dir)
    }
}

fn bump(
    slot: &mut EntryAggregate,
    result: &str,
    bucket: &str,
    count: i64,
) -> Result<(), MetricsError> {
    let n = u32::try_from(count).unwrap_or(u32::MAX);
    match result {
        "arrived" => slot.arrived += n,
        "cancelled" => slot.cancelled += n,
        "rejected" => slot.rejected += n,
        "failed" => slot.failed += n,
        // 读不懂的终态**不静默丢掉**：丢掉的是分母的一部分，而比率会因此偏高——一个
        // 看不出错的错（同 [`unknown_dimension`]）。
        _ => return Err(unknown_dimension()),
    }
    let bucket = bucket_from(bucket).ok_or_else(unknown_dimension)?;
    match slot.durations.iter_mut().find(|(b, _)| *b == bucket) {
        Some((_, existing)) => *existing += n,
        None => slot.durations.push((bucket, n)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = DAY_MS;

    fn open() -> (tempfile::TempDir, WorkspaceMetrics) {
        let dir = tempfile::tempdir().unwrap();
        let m = WorkspaceMetrics::open(dir.path()).unwrap();
        (dir, m)
    }

    #[test]
    fn the_schema_holds_counters_only_and_never_a_single_event() {
        // 这条守护的是**结构性隐私**：只存计数的库即使整个泄露也重建不出任何一个用户
        // 做过什么；一旦有人加进一张带时间戳或 id 的明细表，这条当场变红。
        let (_dir, m) = open();
        let mut stmt = m
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tables, EXPECTED_TABLES);
        drop(stmt);
        // 每张表的列名逐个过一遍：不得出现毫秒时间戳、id、路径、token 这类可关联字段。
        for table in &tables {
            let mut stmt = m
                .conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for col in &cols {
                assert!(
                    !["ts_ms", "id", "path", "token", "note_rel", "title", "route"]
                        .iter()
                        .any(|banned| col.contains(banned)),
                    "{table}.{col} 是可关联字段，不该进这个库"
                );
            }
        }
    }

    #[test]
    fn a_database_built_by_the_previous_schema_generation_cannot_be_opened() {
        // 把"加表/拆枚举"与"必须 bump `SCHEMA_VERSION`"**绑死**（TDD-12 §2.1）。忘了 bump
        // 的表现不是报错：旧库里没有新表，读取侧一路读到空，界面上写着"这个分区没人去过"
        // ——数悄悄少了，而没有任何东西会变红。
        //
        // **本代的抬升理由是「拆枚举」而不是「加表」**（ADR-088 把知识库那一格拆成
        // `knowledge_topics` / `knowledge_tags`），故这里造的上一代库**表清单一模一样**，
        // 只有 `user_version` 停在上一代——正是"看不出差别"的那种旧库。造一个少一张表的
        // 旧库会让这条用例被表清单那一半拦下，从而根本验不到「拆枚举也必须 bump」。
        let dir = tempfile::tempdir().unwrap();
        let path = WorkspaceMetrics::database_path(dir.path());
        let conn = Connection::open(&path).unwrap();
        WorkspaceMetrics::create_schema(&conn).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        drop(conn);

        assert!(
            matches!(
                WorkspaceMetrics::open(dir.path()),
                Err(MetricsError::UnknownSchema(version)) if version == SCHEMA_VERSION - 1
            ),
            "上一代 schema 的库必须 fail closed，不得被当成本代打开"
        );
        assert!(
            path.exists(),
            "fail closed 不等于删库——恢复路径是用户显式清除"
        );
    }

    #[test]
    fn buckets_are_left_closed_and_the_first_one_starts_at_zero() {
        assert_eq!(DurationBucket::of(Some(0)), DurationBucket::Under1s);
        assert_eq!(DurationBucket::of(Some(999)), DurationBucket::Under1s);
        assert_eq!(DurationBucket::of(Some(1_000)), DurationBucket::From1sTo10s);
        assert_eq!(DurationBucket::of(Some(9_999)), DurationBucket::From1sTo10s);
        assert_eq!(
            DurationBucket::of(Some(10_000)),
            DurationBucket::From10sTo60s
        );
        assert_eq!(
            DurationBucket::of(Some(59_999)),
            DurationBucket::From10sTo60s
        );
        assert_eq!(DurationBucket::of(Some(60_000)), DurationBucket::From1mTo5m);
        assert_eq!(
            DurationBucket::of(Some(299_999)),
            DurationBucket::From1mTo5m
        );
        assert_eq!(DurationBucket::of(Some(300_000)), DurationBucket::AtLeast5m);
        // 没有可信耗时时**诚实留空**，不猜一个桶——猜出来的桶会混进分布里而看不出来。
        assert_eq!(DurationBucket::of(None), DurationBucket::Unknown);
    }

    #[test]
    fn counts_accumulate_per_day_and_dimension() {
        let (_dir, m) = open();
        let now = 10 * DAY;
        for _ in 0..3 {
            m.record(
                WorkspaceEvent::SurfaceViewed {
                    surface: WorkspaceSurface::QuickNotes,
                },
                now,
            )
            .unwrap();
        }
        m.record(
            WorkspaceEvent::SurfaceViewed {
                surface: WorkspaceSurface::Search,
            },
            now,
        )
        .unwrap();
        let s = m.summary(0, 20).unwrap();
        assert_eq!(s.surfaces.len(), 2);
        let quick = s
            .surfaces
            .iter()
            .find(|it| it.surface == WorkspaceSurface::QuickNotes)
            .unwrap();
        assert_eq!(quick.count, 3);
    }

    #[test]
    fn the_denominator_is_every_terminal_state_and_the_numerator_only_arrivals() {
        let (_dir, m) = open();
        let now = 5 * DAY;
        let fire = |result: EntryResult| {
            m.record(
                WorkspaceEvent::CreateEntryFinished {
                    action: "new_note",
                    entry: EntrySurface::AddContent,
                    result,
                    duration_ms: 2_000,
                },
                now,
            )
            .unwrap();
        };
        fire(EntryResult::Arrived);
        fire(EntryResult::Arrived);
        fire(EntryResult::Cancelled);
        fire(EntryResult::Rejected);
        fire(EntryResult::Failed);

        let s = m.summary(0, 10).unwrap();
        let agg = &s.entries[0];
        assert_eq!(agg.total(), 5);
        assert_eq!(agg.arrived, 2);
        assert_eq!(agg.arrival_ratio(), Some(0.4));
    }

    #[test]
    fn a_ratio_needs_enough_samples_before_it_says_anything() {
        // 3 个样本里 1 个失败会显示成 33%，而它什么都不说明——小样本比率是本地观测
        // 最容易骗到自己的地方。不足门槛时只给计数。
        let (_dir, m) = open();
        for _ in 0..(MINIMUM_RATIO_SAMPLES - 1) {
            m.record(
                WorkspaceEvent::CreateEntryFinished {
                    action: "new_note",
                    entry: EntrySurface::Sidebar,
                    result: EntryResult::Arrived,
                    duration_ms: 500,
                },
                DAY,
            )
            .unwrap();
        }
        let s = m.summary(0, 5).unwrap();
        assert_eq!(s.entries[0].total(), MINIMUM_RATIO_SAMPLES - 1);
        assert_eq!(s.entries[0].arrival_ratio(), None, "样本不足不该给比率");
    }

    #[test]
    fn days_older_than_the_retention_window_are_gone() {
        let (_dir, m) = open();
        m.record(WorkspaceEvent::QuickNoteSettled, 0).unwrap();
        assert_eq!(m.summary(0, 0).unwrap().quick_notes_settled, 1);
        // 写入顺手清理：保留窗口之外的那一天在下一次写入时被丢掉。
        m.record(WorkspaceEvent::QuickNoteSettled, RETENTION_DAYS * DAY)
            .unwrap();
        assert_eq!(
            m.summary(0, 0).unwrap().quick_notes_settled,
            0,
            "超出保留窗口的日子必须消失"
        );
        assert_eq!(m.summary(0, RETENTION_DAYS).unwrap().quick_notes_settled, 1);
    }

    #[test]
    fn status_visits_are_counted_per_surface_and_never_mixed_with_navigation() {
        let (_dir, m) = open();
        let now = 3 * DAY;
        let visit = |surface, interacted, ms| {
            m.record(
                WorkspaceEvent::StatusVisitFinished {
                    surface,
                    interacted,
                    duration_ms: ms,
                },
                now,
            )
            .unwrap();
        };
        visit(StatusSurface::Notifications, false, 400);
        visit(StatusSurface::Notifications, true, 30_000);
        visit(StatusSurface::Queue, false, 2_000);

        let s = m.summary(0, 10).unwrap();
        assert_eq!(s.status_visits.len(), 2);
        let notifications = s
            .status_visits
            .iter()
            .find(|it| it.surface == StatusSurface::Notifications)
            .unwrap();
        assert_eq!(notifications.total, 2);
        assert_eq!(notifications.short_without_action, 1);
        // 状态面不该跑进导航维度里（两个维度的分母含义不同）。
        assert!(s.surfaces.is_empty());
    }

    #[test]
    fn short_without_action_needs_both_halves_and_ten_seconds_is_not_short() {
        // 边界逐字来自 §2.1：`interacted == false && duration < 10s`。10s 整**不算**短
        // （桶是左闭右开的 `[1s, 10s)`），而"很快但真的按了东西"是"办完了"，不是"没头绪"。
        let (_dir, m) = open();
        let visit = |interacted, ms| {
            m.record(
                WorkspaceEvent::StatusVisitFinished {
                    surface: StatusSurface::Usage,
                    interacted,
                    duration_ms: ms,
                },
                DAY,
            )
            .unwrap();
        };
        visit(false, 9_999); // 短且没操作 ⇒ 算
        visit(false, 10_000); // 恰好 10s ⇒ 不算
        visit(true, 200); // 很短但操作过 ⇒ 不算
        visit(false, 600_000); // 没操作但看了十分钟 ⇒ 不算（他在读）

        let s = m.summary(0, 5).unwrap();
        let usage = &s.status_visits[0];
        assert_eq!(usage.total, 4);
        assert_eq!(usage.short_without_action, 1);
        // 分母不足门槛时只给计数、不给比率（口径与到达率一致）。
        assert_eq!(usage.short_without_action_ratio(), None);
        visit(false, 100);
        let s = m.summary(0, 5).unwrap();
        assert_eq!(s.status_visits[0].total, MINIMUM_RATIO_SAMPLES);
        assert_eq!(s.status_visits[0].short_without_action_ratio(), Some(0.4));
    }

    #[test]
    fn a_dimension_value_we_cannot_read_is_reported_not_silently_dropped() {
        // 静默丢行的表现是**数悄悄少了**：界面写着"这个分区没人去过"，而库里明明记着。
        // 与"计数溢出报存储故障，不绕回/静默饱和"是同一条判据（TDD-12 §2.1）。
        let (_dir, m) = open();
        m.conn
            .execute(
                "INSERT INTO workspace_surfaces (day, surface, count) VALUES (1, 'from_the_future', 3)",
                [],
            )
            .unwrap();
        assert!(
            matches!(m.summary(0, 5), Err(MetricsError::InvalidValue(_))),
            "读不懂的维度取值必须如实报出来"
        );
    }

    #[test]
    fn a_range_the_wrong_way_round_is_rejected_not_silently_swapped() {
        let (_dir, m) = open();
        assert!(m.summary(9, 3).is_err());
    }

    #[test]
    fn clearing_removes_the_file_and_says_so_truthfully() {
        let dir = tempfile::tempdir().unwrap();
        let m = WorkspaceMetrics::open(dir.path()).unwrap();
        m.record(WorkspaceEvent::QuickNoteSettled, DAY).unwrap();
        let path = WorkspaceMetrics::database_path(dir.path());
        assert!(path.exists());
        m.clear().unwrap();
        assert!(!path.exists());
        // 清完还能重新打开成一个空库——清除不是"停用"。
        let again = WorkspaceMetrics::open(dir.path()).unwrap();
        assert_eq!(again.summary(0, 10).unwrap().quick_notes_settled, 0);
    }

    #[test]
    fn a_database_we_just_created_can_be_opened_again() {
        // 这条守的是 `create_schema` 与 `open` 用的**是同一份表清单**。它们曾经是两份：
        // `open` 里那个字面量依赖"我手写的顺序恰好等于 SQLite 的 `ORDER BY name`"。加一张表
        // 时顺序写错，表现是**刚建出来的空库当场 `UnknownSchema`**——功能全灭，且与"旧库
        // fail closed"的现象一模一样，谁都归不到"新表名排序不对"上去。
        let dir = tempfile::tempdir().unwrap();
        let first = WorkspaceMetrics::open(dir.path()).unwrap();
        drop(first);
        WorkspaceMetrics::open(dir.path()).expect("刚建出来的库必须能再打开");
    }

    #[test]
    fn destroying_a_database_we_do_not_understand_still_works() {
        // 坏 schema 下清除**必须仍然成功**：fail closed 之后清除重建是唯一恢复路径
        //（TDD-12 §2.1）。删除要是也得先 `open` 一次，用户就被永久锁在一个删不掉、也读不了
        // 的库上——面板永远 `schema_invalid`，清除永远失败。
        let dir = tempfile::tempdir().unwrap();
        let path = WorkspaceMetrics::database_path(dir.path());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE alien(x); PRAGMA user_version=99;")
            .unwrap();
        drop(conn);
        assert!(WorkspaceMetrics::open(dir.path()).is_err(), "前提：打不开");
        WorkspaceMetrics::destroy(dir.path()).expect("坏 schema 也必须能清掉");
        assert!(!path.exists());
        // 清完能重建成一个空库——这才叫"恢复路径"。
        assert_eq!(
            WorkspaceMetrics::open(dir.path())
                .unwrap()
                .summary(0, 10)
                .unwrap(),
            WorkspaceSummary::default()
        );
    }

    #[test]
    fn destroying_this_store_never_touches_the_knowledge_one() {
        // N16：两套观测同处一个应用私有目录，但**各有各的清除边界**。这里不满足于
        // "路径不同"的断言——真的把两个库都建出来，清掉一个，再逐字节确认另一个还在。
        let dir = tempfile::tempdir().unwrap();
        let workspace = WorkspaceMetrics::open(dir.path()).unwrap();
        workspace
            .record(WorkspaceEvent::QuickNoteSettled, DAY)
            .unwrap();
        drop(workspace);
        let knowledge = crate::MetricsStore::open(dir.path(), vec![7; 32]).unwrap();
        knowledge
            .suggestion(
                "/v",
                "token",
                crate::SuggestionEvent::Shown,
                crate::SuggestionSurface::Graph,
                DAY,
            )
            .unwrap();
        drop(knowledge);
        let knowledge_path = crate::MetricsStore::database_path(dir.path());
        let before = fs::read(&knowledge_path).unwrap();

        WorkspaceMetrics::destroy(dir.path()).unwrap();

        assert!(!WorkspaceMetrics::database_path(dir.path()).exists());
        assert!(knowledge_path.exists(), "清工作区不得动知识库");
        assert_eq!(fs::read(&knowledge_path).unwrap(), before, "内容也不得变");
        // 反向：清知识库不得动工作区库。
        let workspace = WorkspaceMetrics::open(dir.path()).unwrap();
        workspace
            .record(WorkspaceEvent::QuickNoteSettled, DAY)
            .unwrap();
        drop(workspace);
        let workspace_path = WorkspaceMetrics::database_path(dir.path());
        let before = fs::read(&workspace_path).unwrap();
        crate::MetricsStore::open(dir.path(), vec![7; 32])
            .unwrap()
            .clear()
            .unwrap();
        assert!(!knowledge_path.exists());
        assert!(workspace_path.exists(), "清知识库不得动工作区库");
        assert_eq!(fs::read(&workspace_path).unwrap(), before, "内容也不得变");
    }

    #[test]
    fn an_unknown_schema_fails_closed_without_deleting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let path = WorkspaceMetrics::database_path(dir.path());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE alien(x); PRAGMA user_version=99;")
            .unwrap();
        drop(conn);
        assert!(matches!(
            WorkspaceMetrics::open(dir.path()),
            Err(MetricsError::UnknownSchema(99))
        ));
        assert!(path.exists(), "fail closed 不等于删库");
    }

    #[test]
    fn this_store_is_a_different_file_from_the_knowledge_one() {
        // 两个开关、两份数据、两条清除边界（TDD-12 §2.1）：共用一个文件的话，
        // 「清除工作区记录」会连知识指标一起删掉，而用户按的是另一个按钮。
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(
            WorkspaceMetrics::database_path(dir.path()),
            crate::MetricsStore::database_path(dir.path())
        );
    }
}
