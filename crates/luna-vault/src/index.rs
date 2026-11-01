//! SQLite 派生索引（TDD-06 §3）——**仅用于性能**（搜索、按 type/时间列出、反向链接）。
//!
//! 本地文件是唯一事实源；本索引删了能从 Vault 文件全量重建。关键点：
//! - **FTS5 必须 `trigram` 分词器**（中文搜索命门：默认 unicode61 把整句中文当一个 token）。
//! - **FTS 索引卫生**：入库前剥 frontmatter + 时间码（[`crate::parse::clean_for_fts`]）。
//! - **`title` = 文件名 stem**（单一来源，§3）；**缺省 `type=note`**（§5 互操作命门）。
//! - **`user_version` 自愈重建**：内置 schema 版本变更时整库重建（派生数据无迁移链负担，§3）。
//!
//! 短查询（<3 字符）trigram 失效，自动降级为 `LIKE` 子串扫描——覆盖 2 字中文词（"会议"）这类
//! trigram 命门外的高频场景（§3 假设：trigram 不够再上 jieba/ICU，此为务实兜底）。

use crate::error::{Result, VaultError};
use crate::frontmatter::{read_luna, NoteStage, NoteType};
use crate::hashing::ContentHash;
use crate::parse::{clean_for_fts, extract_links, extract_tags, title_of, TagSource};
use rusqlite::{
    functions::FunctionFlags, params, params_from_iter, Connection, OpenFlags,
    OptionalExtension as _,
};

/// 连接级 SQLite 函数调用 [`crate::is_ingest_candidate`]；各查询只保留这一段调用。
/// 要求表别名为 `n`，并传入派生索引已有的类型、成熟度、产物身份和正文长度事实。
pub(crate) const INGEST_CANDIDATE_SQL: &str =
    "luna_ingest_candidate(n.path, n.type, n.stage, n.is_topic, n.is_knowledge_map, n.body_chars) = 1";
use serde::Serialize;

/// 内置 schema 版本：变更即触发全库重建（派生可重建，无迁移链，§3）。
// v3：`notes.topic_src` 加列（PRD-16-016 主题页建议要查「这个标签已经有主题页了吗」）。
// v4：`ingest_suggestions` 表（PRD-16-025 归入建议，ADR-026 ⑦ 的「派生归 index.sqlite」）。
// v5：`embeddings` 表（PRD-16-028 语义检索，ADR-027：向量是派生数据，可从笔记重建；
//     含 `chunk_hash` 支撑分片级复用——改一行不必整篇重嵌）。
// v6：知识体检（PRD-16-030/031）。`notes` 加三列 + `checkup_findings` 表。三列都是
//     「T0 要毫秒级答出来」的前提：`is_topic` 让「哪些是主题页」不必逐篇读 frontmatter，
//     `body_chars` 让空页判定不必读全文，`distill_note` 让待整理积压不必跑全库读盘扫描。
// v7：`links.dst_path` 改存**规范形**（`pathutil::canonical_link_target`：百分号解码 +
//     按源笔记目录归一化）。此前存的是链接原样，于是「链接目标」与「库内路径」两种口径
//     混在一列里：外链面板把 `../Transcripts/名字%20带空格.md` 交给打开笔记的命令 →
//     用户读到「路径越出 Vault 根」；反链按 `dst = 路径` 匹配也对不上（第五轮真机上报）。
// v8：`notes.is_knowledge_map` 加列（知识地图唯一性与文件树标识无需逐篇读盘）。
// v9：`embeddings` 的向量空间身份改为结构化六元组：目标、运行时摘要、维度、Profile、
//     归一化与分块策略。任一项变化都必须重建，绝不把不可比的向量混入同一矩阵。
// v10：`notes.stage` 派生列 + 索引（ADR-086 成熟度轴）。存的是解析那一刻算出的结论
//     （`NoteStage::column`：记录态 `draft` / 其余 NULL；资料类型恒 NULL——来源轴由
//     `type` 列表达）。搜索「记录」分组、图谱记录过滤、问答降权、主题页建议计数全部
//     只读这一列，不各自去读 frontmatter（TDD-06 §3）。
// 索引是**可重建的派生数据**，故版本递增即整库自愈重建，无迁移链负担（§3）。
// v13：从新格式主题 Markdown 派生逐条出处依赖；索引删除后可由文件重建。
// v14：全库忽略的 AI 关系标签（PRD-16-051）。权威清单在随库 meta store，这里只放副本
//     `ai_tag_ignores`；被忽略的 AI 标签建索引时进 `ai_tags_suppressed` 而不进 `tags`——
//     标签树、图谱边、主题候选、检索过滤都只读 `tags`，一处生效、各视角自动一致。
// v16：`notes.fts_rowid` 把每篇笔记挂到它的全文行（笔记库对标轮 V1）。`fts.note_path` 是
//     UNINDEXED 列，此前按它删除是全表扫描——重索引一篇就扫一遍整个全文索引，首次接入、
//     改名改写引用方、重建索引都随篇数平方增长（5 万篇改名 27.7 s）。改按行号删。升版本会
//     整库重建并清掉语义向量（万片约 40 分钟后台重算），用户 2026-10-05 裁决接受。
const SCHEMA_VERSION: i64 = 16;

const SCHEMA_SQL: &str = "\
CREATE TABLE notes (
    path     TEXT PRIMARY KEY,
    title    TEXT NOT NULL,
    type     TEXT NOT NULL,
    created  TEXT,
    modified INTEGER NOT NULL DEFAULT 0,
    hash     TEXT NOT NULL,
    -- 主题页的出身标签（`luna.topic_src`）；非主题页为 NULL。
    topic_src TEXT,
    -- 这是不是一篇主题页（`luna.topic`）。与 `topic_src` 不是一回事：手动标成主题页的没有
    -- 出身标签，而知识体检要问的正是「哪些页在做 AI 综合声明」（ADR-026 ①）。
    is_topic INTEGER NOT NULL DEFAULT 0,
    -- 这是不是知识地图（`luna.knowledge_map`）。独立列保证唯一性检查与文件树标识 O(1)。
    is_knowledge_map INTEGER NOT NULL DEFAULT 0,
    -- **正文**字符数（不含 frontmatter）。空页体检按它判定——否则要么读全文（几千篇即秒级，
    -- 而 T0 承诺毫秒级），要么去扫 fts 的内容列（拿全文检索索引当长度统计用）。
    body_chars INTEGER NOT NULL DEFAULT 0,
    -- （资料侧）这篇资料的蒸馏笔记路径（`luna.distill_note`）；没整理过为 NULL。
    -- 待整理积压按它数——判定的完整版（含「源已更新」）仍在 `Vault::distill_scan`。
    distill_note TEXT,
    -- **成熟度轴**（ADR-086）：记录态 = `draft`，知识态 = NULL，资料类型恒 NULL。
    -- 值由 `NoteStage::resolve` 在解析那一处算出（`NoteStage::column`），本列只是它的
    -- 落盘形态——消费面读列，绝不重跑缺省规则、更不去读 frontmatter。
    -- 派生数据可重建；**绝不写回缺省值**（不给无键笔记补 `stage`，TDD-06 §5）。
    stage TEXT,
    -- 语音速记 AI 提炼的落盘时间（`luna.refined_at`，PRD-06-058 C′）；速记行据此挂「已提炼」。
    refined_at TEXT,
    -- `luna.created` 解析成的 Unix 毫秒（解析失败 / 无键 = NULL）。录音行按「录于何时」排序用
    -- （PRD-07-007）：RFC3339 原样字符串带时区偏移，按字符串比大小是错的，在 Rust 解析一次落成数。
    created_ms INTEGER,
    -- 这篇的全文行（`fts.rowid`）。`fts.note_path` 不可索引，删改一律按它（v16）。
    fts_rowid INTEGER NOT NULL
);
CREATE INDEX idx_notes_type ON notes(type);
CREATE INDEX idx_notes_stage ON notes(stage);
CREATE TABLE media (
    path           TEXT PRIMARY KEY,
    note_path      TEXT NOT NULL,
    duration       REAL,
    has_transcript INTEGER NOT NULL DEFAULT 0,
    has_translation INTEGER NOT NULL DEFAULT 0,
    model          TEXT
);
CREATE INDEX idx_media_note ON media(note_path);
CREATE TABLE segments (
    id          TEXT PRIMARY KEY,
    media_path  TEXT NOT NULL,
    start_ms    INTEGER NOT NULL,
    end_ms      INTEGER NOT NULL,
    text        TEXT NOT NULL,
    translation TEXT
);
CREATE INDEX idx_segments_media ON segments(media_path);
CREATE TABLE links (
    src_path TEXT NOT NULL,
    dst_path TEXT NOT NULL
);
CREATE INDEX idx_links_src ON links(src_path);
CREATE INDEX idx_links_dst ON links(dst_path);
-- 新格式主题的来源→声明依赖；只作定位投影，正文与出处身份仍在 Markdown。
CREATE TABLE topic_citation_dependencies (
    topic_path  TEXT NOT NULL,
    citation_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    PRIMARY KEY (topic_path, citation_id)
);
CREATE INDEX idx_topic_citation_source ON topic_citation_dependencies(source_path, topic_path, citation_id);
CREATE TABLE tags (
    note_path TEXT NOT NULL,
    tag       TEXT NOT NULL,
    -- 标签来源（PRD-06-046）：user（顶层 tags 与内联 #tag）/ ai（luna.ai_tags）。
    -- 同池检索不受影响，来源只服务「哪些标签完全是机器造出来的」这一区分呈现。
    source    TEXT NOT NULL
);
CREATE INDEX idx_tags_tag ON tags(tag);
CREATE INDEX idx_tags_note ON tags(note_path);
-- frontmatter 顶层 `aliases`（对标方案 C2）：链接按别名也认得出这一篇（解析、反链、图谱、补全）。
CREATE TABLE aliases (
    note_path TEXT NOT NULL,
    alias     TEXT NOT NULL
);
CREATE INDEX idx_aliases_note ON aliases(note_path);
CREATE TABLE ai_tag_ignores (tag TEXT PRIMARY KEY);
CREATE TABLE ai_tags_suppressed (
    note_path TEXT NOT NULL,
    tag       TEXT NOT NULL
);
CREATE INDEX idx_ai_suppressed_tag ON ai_tags_suppressed(tag);
CREATE INDEX idx_ai_suppressed_note ON ai_tags_suppressed(note_path);
CREATE VIRTUAL TABLE fts USING fts5(note_path UNINDEXED, content, tokenize='trigram');
-- 语义检索的向量（PRD-16-028 / ADR-027）。分片粒度：一篇笔记有若干行。
--
-- **不建向量索引、不引 sqlite-vec**：查询走内存暴力精确余弦（S16 实测 1 万片 39 MB、
-- 毫秒级）。这张表只是**持久层**——它要回答的问题只有两个：「这篇的向量还新不新」
-- （`src_hash`）与「启动时把全部向量读进内存」（全表扫）。两者都不需要索引结构。
CREATE TABLE embeddings (
    note_path  TEXT NOT NULL,
    chunk_seq  INTEGER NOT NULL,
    -- 该分片在**原文**里的起始行（1-based，含 frontmatter）：命中后直接用它跳转。
    line_start INTEGER NOT NULL,
    -- 向量：f32 小端字节序。写入与读取都校验字节维度等于下方的空间维度。
    vec        BLOB NOT NULL,
    -- 生成时**笔记正文**的指纹：正文一变，这篇的全部向量即过期（增量建库的幂等门控）。
    src_hash   TEXT NOT NULL,
    -- 该**分片文本自己**的指纹。有了它，改一行只需重嵌被改的那一两片：
    -- 编辑器每 ~800ms 自动保存一次、每次正文指纹都变，只按 `src_hash` 判定会让一篇 40 片的
    -- 长笔记在每个停顿处整篇重嵌（约 10 秒 GPU）。分片级复用把它降到一两片。
    chunk_hash TEXT NOT NULL,
    -- 向量空间的完整身份（TDD-06 §3.5.1）。这里只存事实字段，不存可漂移的拼接字符串。
    target_id       TEXT NOT NULL,
    runtime_digest  TEXT NOT NULL,
    dimension       INTEGER NOT NULL,
    profile_version INTEGER NOT NULL,
    normalization   TEXT NOT NULL,
    chunk_policy_id TEXT NOT NULL,
    PRIMARY KEY (note_path, chunk_seq)
);
CREATE INDEX idx_embeddings_note ON embeddings(note_path);
-- 深度体检的发现（PRD-16-031 / ADR-026 ⑦）。
--
-- **只存 T1 的发现，绝不存 T0 的**：T0 五类判定是索引上的几条 SQL（毫秒级），存下来
-- 只会多一份会过期的副本；T1 的矛盾/过时判定各烧一次本地 LLM，不落盘就意味着关掉应用
-- 那半小时的算力白花了。判据即「重算一次要多少钱」，与归入建议缓存同一条分层理由。
--
-- 用户的「忽略」不在这里——它是**权威决定**，随库存 `.luna/checkup_ignores.json`
-- （ADR-026 ⑦：随派生索引重建而复活的「我明明关掉了它」就是真 bug）。
CREATE TABLE checkup_findings (
    -- 稳定 id（`kind:主体…`，见 `crate::checkup::Finding::id`）：忽略清单与它对齐，
    -- 故同一条发现在两次体检之间能被认出来是同一条。
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    -- 涉及页，JSON 字符串数组（第一项 = 卡片「打开」的落点）。
    paths      TEXT NOT NULL,
    -- 「打开」时要落到的行（1-based）；0 = 只打开笔记。
    line       INTEGER NOT NULL DEFAULT 0,
    -- 被指认的原文（那条声明）。读侧据它校验发现是否仍成立——声明被改写/删掉了，
    -- 这条发现当场作废，绝不拿一份对不上正文的旧判定继续报警。
    subject    TEXT NOT NULL DEFAULT '',
    -- 模型给的理由（**模型产出**，不是 UI 文案）。
    reason     TEXT NOT NULL DEFAULT '',
    created_ms INTEGER NOT NULL DEFAULT 0
);
";

/// `NoteType` 的数据库字符串 = frontmatter kebab 键（[`NoteType::key`] 单一事实源）。
fn note_type_str(t: NoteType) -> &'static str {
    t.key()
}

/// 一行索引记录的**记录态事实**：`notes.stage` 列非 NULL 即记录态。
///
/// 写入侧由 [`crate::NoteStage::column`] 保证「资料类型恒 NULL」，故这里不必也**不该**
/// 再看一眼 `type`——那就是把缺省规则第二次写下来。收成一个函数是为了让「这一列怎么读」
/// 在全仓只有一处写法（五个 `NoteListEntry` 查询共用它）。
fn row_draft(stage_column: Option<String>) -> bool {
    stage_column.is_some()
}

/// 一段时间内收进来的一篇来源（[`Index::sources_created_since`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentSource {
    pub path: String,
    pub note_type: NoteType,
    /// 已被至少一页主题页引用。
    pub cited: bool,
}

/// 列表查询的页大小（TDD-07 §4.1a）。
///
/// **单一常量**：前端不写第二份，也不需要读——列表请求把 `limit` 留空即「按这个页大小
/// 取一页」（[`Query::limit`] 的 0 = 由本层给默认）。两处各写一个 200 就是这次要修掉的那个
/// 固定上限的翻版：同一条规则两份定义，改一边不会有任何东西变红。
pub const LIST_PAGE_SIZE: usize = 200;

/// 列表排序键（TDD-07 §4.1a）：过滤 + 排序 + 分页全部在 SQL 层完成，前端不再重排。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListSort {
    /// 修改时间倒序（最近在前）。
    #[default]
    Modified,
    /// 标题 **UTF-8 码点序**（`COLLATE BINARY`）。
    ///
    /// 与旧版前端 `localeCompare` 的结果不同，中文标题的先后会变——本仓没有 ICU
    /// collator，把排序搬进 SQL 就必须接受一个确定的、跨平台一致的序。确定性是分页正确性
    /// 的前提：游标比较必须与 `ORDER BY` 用同一套比较规则，否则跨页会重复或漏行。
    Name,
    /// 关联媒体总时长倒序（长的在前：找「那个两小时的会议」比找最短的常见得多）。
    Duration,
    /// 「录于何时」倒序（`luna.created`；没记的老笔记退到修改时间，PRD-07-007）。
    Created,
}

impl ListSort {
    /// 稳定键名（游标里编着它，故改名 = 让所有在途游标失效）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Name => "name",
            Self::Duration => "duration",
            Self::Created => "created",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "modified" => Some(Self::Modified),
            "name" => Some(Self::Name),
            "duration" => Some(Self::Duration),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

/// 一次**列表语义**的取数（TDD-07 §4.1a）：排序键 + 键集游标。
///
/// 它在 [`Query`] 上是 `Option`，而这个 `Option` 就是「列表」与「top-N」的分界：
/// `None` = 既有的 top-N 语义（`limit` 即上限），`Some` = 「符合条件的**全部条目**可被游标
/// 穷尽遍历」。全文检索路径**不接受**它（相关度序做不出稳定游标，契约本期不给它分页）。
#[derive(Clone, Debug, Default)]
pub struct ListPage {
    pub sort: ListSort,
    /// 上一页末行的游标；`None` = 取第一页。**不透明字符串**，由本层生成、原样回传。
    pub cursor: Option<String>,
}

/// 键集游标的解出形态：`(排序值, path)` 复合键。
struct Keyset {
    /// 排序值的**原样文本**（整数键为十进制、`name` 为标题本身）。
    value: String,
    path: String,
}

/// 游标编码版本。形状变了就把它撞掉——旧游标随即被判 `LIST_CURSOR_INVALID`，
/// 前端从第一页重来，而不是拿着一个解释错的复合键跨页漏行。
const CURSOR_VERSION: &str = "1";

/// 载荷分隔符（US，U+001F）：路径与标题里出现它的可能性为零，且长度前缀已经让
/// 「值里恰好有一个分隔符」也无从歧义。
const CURSOR_SEP: char = '\u{1f}';

/// 编码一行的键集游标。
///
/// 载荷 = `版本 ␟ 排序键 ␟ 值字节数 ␟ <值><路径>`，再整体十六进制。**十六进制不是加密**，
/// 它只是把这串东西变得对前端毫无可读性——契约要的是"前端只回传、不解析、不自行构造"，
/// 而一个看得懂的游标迟早会被就地拼一个出来。
fn encode_cursor(sort: ListSort, value: &str, path: &str) -> String {
    let payload = format!(
        "{CURSOR_VERSION}{CURSOR_SEP}{}{CURSOR_SEP}{}{CURSOR_SEP}{value}{path}",
        sort.key(),
        value.len()
    );
    let mut out = String::with_capacity(payload.len() * 2);
    for byte in payload.as_bytes() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 解出游标，并校验它与**本次请求的排序键**一致。
///
/// 任何一步不成立都是同一个结论（契约：只扩一个码）：这个游标对本次查询不成立。
fn decode_cursor(raw: &str, sort: ListSort) -> Result<Keyset> {
    let invalid = |why: &str| VaultError::ListCursorInvalid(why.to_owned());
    if !raw.len().is_multiple_of(2) || raw.is_empty() {
        return Err(invalid("游标不是合法编码"));
    }
    let mut bytes = Vec::with_capacity(raw.len() / 2);
    for pair in raw.as_bytes().chunks(2) {
        let hex = std::str::from_utf8(pair).map_err(|_| invalid("游标不是合法编码"))?;
        bytes.push(u8::from_str_radix(hex, 16).map_err(|_| invalid("游标不是合法编码"))?);
    }
    let payload = String::from_utf8(bytes).map_err(|_| invalid("游标不是合法编码"))?;
    let mut parts = payload.splitn(4, CURSOR_SEP);
    let (Some(version), Some(key), Some(len), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid("游标载荷缺字段"));
    };
    if version != CURSOR_VERSION {
        return Err(invalid("游标版本已失效"));
    }
    if key != sort.key() {
        return Err(invalid("游标与本次请求的排序键不匹配"));
    }
    let len: usize = len.parse().map_err(|_| invalid("游标载荷缺字段"))?;
    if len > rest.len() || !rest.is_char_boundary(len) {
        return Err(invalid("游标载荷缺字段"));
    }
    let (value, path) = rest.split_at(len);
    if path.is_empty() {
        return Err(invalid("游标缺次序键"));
    }
    Ok(Keyset {
        value: value.to_owned(),
        path: path.to_owned(),
    })
}

/// 一个查询绑定值。
///
/// **整数键不能再一律走字符串**：SQLite 里 TEXT 与 INTEGER 的比较走两套规则，而键集
/// 游标的续行谓词（`modified < ?`）恰恰是比较——把 `'1700000000000'` 绑上去，跨页就会在
/// 字典序与数值序不一致的地方漏行。
enum Bind {
    Text(String),
    Int(i64),
}

impl rusqlite::ToSql for Bind {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match self {
            Self::Text(value) => value.to_sql(),
            Self::Int(value) => value.to_sql(),
        }
    }
}

/// 关联媒体总时长（**毫秒整数**）的 SQL 表达式，供 `notes n` 作用域内复用。
///
/// 毫秒转换在 SQL 里做而不是读出来再乘：`ORDER BY`、游标续行谓词与下发给前端的
/// `duration_ms` 必须是**同一个数**，否则两条秒数不同、毫秒相同的记录会在跨页时打架。
const DURATION_MS_SQL: &str = "CAST(COALESCE((SELECT SUM(m.duration) FROM media m \
     WHERE m.note_path = n.path), 0) * 1000 AS INTEGER)";

/// 命中行的两列附加事实（作用域内 `notes` 别名固定为 `n`，三条列表查询共用）：
/// 创建时间原样、以及「至少一条媒体已转写」。
const HIT_EXTRA_SQL: &str = "n.created, EXISTS(SELECT 1 FROM media m \
     WHERE m.note_path = n.path AND m.has_transcript = 1), n.created_ms";

/// 「录于何时」的排序值：有创建时间用创建时间，没记的老笔记退到修改时间——退路在 SQL 里，
/// 于是排序值、游标与下发值仍是同一个数（`hit_sort_value` 与此同口径）。
const CREATED_SORT_SQL: &str = "COALESCE(n.created_ms, n.modified)";

/// `luna.created`（RFC3339 原样）→ Unix 毫秒；无键或解析不了 = `None`（绝不猜一个时间）。
pub(crate) fn created_ms_of(created: Option<&str>) -> Option<i64> {
    created
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c.trim()).ok())
        .map(|d| d.timestamp_millis())
}

/// 排序值的 SQL 表达式（作用域内 `notes` 的别名固定为 `n`，三条列表查询共用）。
fn sort_value_sql(sort: ListSort) -> &'static str {
    match sort {
        ListSort::Modified => "n.modified",
        // 显式 `COLLATE BINARY`：`ORDER BY` 与游标续行谓词必须用**同一套**比较规则，
        // 而"列上写的是什么 collation"是一句离这里很远的建表语句。写死它，两处就不会漂。
        ListSort::Name => "n.title COLLATE BINARY",
        ListSort::Duration => DURATION_MS_SQL,
        ListSort::Created => CREATED_SORT_SQL,
    }
}

/// 追加键集分页子句：`AND <续行谓词> ORDER BY <排序值>, n.path LIMIT ?`。
///
/// **禁 OFFSET**（契约明写）：并发写入下同一条会跨页重复出现或整条漏掉，而「符合条件的
/// 全部条目」这句承诺挡住的正是这种失效。键集比较的是**值本身**，插入/删除只会改变还没走到
/// 的那一段，不会把已经走过的行推回来。
fn append_keyset(
    sql: &mut String,
    binds: &mut Vec<Bind>,
    page: &ListPage,
    limit: i64,
) -> Result<()> {
    let expr = sort_value_sql(page.sort);
    // `name` 升序（码点序），另两个降序（新的 / 长的在前）；次序键 `path` 恒升序。
    let descending = !matches!(page.sort, ListSort::Name);
    if let Some(raw) = page.cursor.as_deref() {
        let keyset = decode_cursor(raw, page.sort)?;
        binds.push(match page.sort {
            ListSort::Name => Bind::Text(keyset.value.clone()),
            _ => Bind::Int(
                keyset
                    .value
                    .parse()
                    .map_err(|_| VaultError::ListCursorInvalid("游标载荷缺字段".to_owned()))?,
            ),
        });
        let value = binds.len();
        binds.push(Bind::Text(keyset.path));
        let path = binds.len();
        let cmp = if descending { '<' } else { '>' };
        sql.push_str(&format!(
            " AND (({expr} {cmp} ?{value}) OR ({expr} = ?{value} AND n.path > ?{path}))"
        ));
    }
    let dir = if descending { "DESC" } else { "ASC" };
    sql.push_str(&format!(" ORDER BY {expr} {dir}, n.path ASC LIMIT ?"));
    binds.push(Bind::Int(limit));
    sql.push_str(&binds.len().to_string());
    Ok(())
}

/// 收尾一页：查询多取了一条，它只用来回答「后面还有没有」——**取满就有下一页**这种反推
/// 要求调用方知道页大小，而页大小恰恰是这次要收敛成单一常量的东西。
///
/// 返回下一页的游标（`None` = 已遍历到底）。
fn take_page<T>(
    rows: &mut Vec<T>,
    limit: i64,
    sort: ListSort,
    key: impl Fn(&T) -> (String, String),
) -> Option<String> {
    let limit = limit.max(0) as usize;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    if !has_more {
        return None;
    }
    let last = rows.last()?;
    let (value, path) = key(last);
    Some(encode_cursor(sort, &value, &path))
}

/// 一行命中在给定排序键下的排序值（与 `ORDER BY` 用的是同一个数/同一个字符串）。
fn hit_sort_value(hit: &SearchHit, sort: ListSort) -> String {
    match sort {
        ListSort::Modified => hit.modified_ms.to_string(),
        ListSort::Name => hit.title.clone(),
        ListSort::Duration => hit.duration_ms.to_string(),
        ListSort::Created => hit.created_ms.unwrap_or(hit.modified_ms).to_string(),
    }
}

/// 搜索查询（结构化过滤 + 全文，§3）。各字段可布尔组合（AND）。
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// 全文检索词（None = 无全文约束，仅按过滤列出）。
    pub text: Option<String>,
    /// 按功能视图类型过滤（§5）。
    pub note_type: Option<NoteType>,
    /// 按标签过滤（§2.3）。
    pub tag: Option<String>,
    /// 按路径前缀过滤。
    pub path_prefix: Option<String>,
    /// 标题 / 路径的**子串**筛选（列表控件那只搜索框；空白 = 不筛）。
    ///
    /// 与 [`Query::text`] 是两件事：那个查正文（FTS），这个只看列表里**看得见**的两样。
    /// 它必须下沉到 SQL——留在前端的话，分页之后它只筛已加载的那几页，于是"筛出 0 条"
    /// 与"库里没有"变成同一个画面，而后者是错的。
    pub name_like: Option<String>,
    /// 只要**知识态**（排除记录态，ADR-086）：读 `notes.stage` 列，不重判缺省。
    ///
    /// 目前唯一的开启方——主题页合成建议的「≥3 篇知识笔记」计数（PRD-16-016 修订）：
    /// 零散速记凑满一个标签不该触发合成建议。搜索面**不**开它（记录态不降权、不排除，
    /// 只分组，PRD-06-052）。
    pub knowledge_only: bool,
    /// 只要**该被我们替用户挑成候选**的那些笔记（[`crate::is_ingest_candidate`] 的
    /// SQLite 函数投影）：正式知识笔记 ∧ 非聚合页 ∧ 有正文。
    ///
    /// **它是一格开关，而不是让调用方自己拼四个条件**——这正是它存在的理由：
    /// 判据有四条轴，而真机上出事的那一份恰恰只设了三条（漏了产物轴，主题页于是被当成
    /// 源笔记喂进了合成）。给一格原子的开关，「设了 3/4」这种形状就不再表达得出来。
    /// 与 [`Self::note_type`] / [`Self::knowledge_only`] **不叠加使用**：它已经蕴含两者。
    pub ingest_candidates_only: bool,
    /// 结果上限 = **页大小 / top-N 条数**（TDD-07 §4.1a：`limit` 的语义不变）。
    /// `0` = 由本层给默认：列表语义取 [`LIST_PAGE_SIZE`]，top-N 取 50。
    pub limit: usize,
    /// 列表语义（排序键 + 键集游标）；`None` = top-N（既有语义，见 [`ListPage`]）。
    pub page: Option<ListPage>,
}

impl Query {
    /// 全文检索快捷构造。
    #[must_use]
    pub fn text(q: impl Into<String>) -> Self {
        Self {
            text: Some(q.into()),
            limit: 50,
            ..Default::default()
        }
    }
}

/// 一条搜索命中（§3）。`snippet` 为带高亮的上下文片段。
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    /// 笔记相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 相关度（越小越相关，bm25；LIKE/过滤路径为 0）。
    pub score: f64,
    /// 高亮上下文片段。
    pub snippet: String,
    /// 最后修改时间（epoch ms；索引已存，列表排序用，PRD-07-007）。
    pub modified_ms: i64,
    /// 关联媒体总时长（毫秒；无媒体或未知 = 0）。转写/录音列表按时长排序与显示用。
    pub duration_ms: i64,
    /// 创建时间（frontmatter `created`，RFC3339 原样；无该键 = `None`）。录音行显示的是
    /// 「它录于何时」而不是最近修改（PRD-07-007）——改一次标题就会把修改时间顶到今天。
    pub created: Option<String>,
    /// 关联媒体里有没有至少一条已转写（`media.has_transcript`）。录音行据此答
    /// 「这条还要不要处理」（PRD-07-007）；无媒体 = `false`。
    pub transcribed: bool,
    /// `created` 解析成的 Unix 毫秒；无键或解析不了 = `None`。行显示与「录制时间」排序同一个数。
    pub created_ms: Option<i64>,
    /// 下一页的键集游标，**只挂在本页末行**；`None` = 没有下一页（TDD-07 §4.1a）。
    ///
    /// 消费方式恒定一句：`hits.last().and_then(|h| h.cursor)`。top-N 语义
    /// （`Query::page` 未给）下恒为 `None`——那条路径本来就没有下一页，它给的是
    /// 「最相关的前 N 条」，不是「全部条目」。
    ///
    /// **不透明字符串**：调用方只回传，不解析、不自行构造（契约明写）。
    pub cursor: Option<String>,
}

/// Tag usage count in the vault index.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TagCount {
    pub tag: String,
    pub count: u32,
    /// 该标签**只**出现在 AI 标注里（PRD-06-046）：没有任何一篇笔记是用户自己给它打的。
    ///
    /// 用「只」而不是「有多少篇来自 AI」——用户想过滤掉的是**完全由机器造出来的**标签；
    /// 一个他自己也在用的标签，AI 恰好也提到并不降低它的价值。
    pub ai_only: bool,
}

/// 标签浏览器的一行后端事实（PRD-06-033）。
///
/// 与 [`TagCount`] 的「真实标签精确计数」不同，本结构会补齐嵌套标签的中间层，并给出
/// **自身直挂笔记数**与**整支去重笔记数**。例如同一篇笔记同时带 `#a` 与 `#a/b` 时，
/// `a.note_count` 仍为 1，而不是把两条 tag 行相加成 2；侧栏数字因此与点开后的结果一致。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TagSummary {
    pub tag: String,
    pub direct_count: u32,
    pub note_count: u32,
    /// 这一支（自身 + 子孙）没有任何用户标签；中间层的来源判定同样由 Rust 给出。
    pub ai_only: bool,
}

/// 按类型列出的笔记条目（功能视图消费，§5：如对话历史会话列表）。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NoteListEntry {
    /// 笔记相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 文件修改时间（Unix 毫秒；索引缺省 0）。
    pub modified_ms: i64,
    /// 笔记类型 kebab 键（`NoteType::key()` 口径；索引缺省 `note`）。
    /// 搜索结果据此分「笔记 / 资料」两组呈现（ADR-024）——判定在 Rust，前端只渲染。
    pub note_type: String,
    /// 本篇是**记录态**吗（`notes.stage`，ADR-086）——搜索「记录」分组、问答降权与
    /// 引用标注的事实来源。判定在 Rust（写索引那一刻由 [`NoteStage::resolve`] 算出），
    /// 消费面只搬运这个结论。
    pub draft: bool,
}

/// 速记侧栏的筛选口径（PRD-06-058 A′）。
///
/// 判定在这里而不在前端（ADR-003）：前端就地筛「已加载的那几页」会当场说谎——清单一
/// 分页，被筛掉的条目里有多少还没取回来，界面根本不知道。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftFilter {
    /// 全部速记。
    #[default]
    All,
    /// 留了原始录音的（行上画麦克风并报时长的那些）。
    WithAudio,
    /// AI 已补过标题与待办（`luna.refined_at` 有值）。
    Refined,
    /// 搁置到该催了——门槛与知识体检的积压行**同一个常量**（PRD-06-055）。
    Stale,
}

/// 速记侧栏的一次取数请求。
#[derive(Clone, Copy, Debug)]
pub struct DraftInboxQuery<'a> {
    /// 就地查找的关键词（标题或正文的**子串**；`None` / 空白 = 不过滤）。
    ///
    /// 这是筛选不是搜索：全库检索（FTS / trigram / 语义排序）归「搜索」空间。这里回答
    /// 的是「我刚才那条哪去了」，子串匹配既够用又可预期，侧栏里再放一套引擎才是重复。
    pub text: Option<&'a str>,
    /// 类别筛选。
    pub filter: DraftFilter,
    /// 「此刻」（Unix 毫秒）：`Stale` 的门槛与逐行的 `stale` 标记都由它现算——
    /// 两者必须出自同一个时刻，否则筛出来的行里会有几条自己不带警示色。
    pub now_ms: i64,
}

/// 速记清单与计数**共用的** FROM/WHERE 骨架（`notes` 别名恒为 `n`）。
const DRAFT_BASE_SQL: &str =
    "FROM notes n LEFT JOIN fts f ON f.note_path = n.path WHERE n.type = ?1";

/// 把筛选条件追加成 SQL 片段并压入绑定值。
///
/// 清单与计数走同一个函数：计数是「筛出来有几条」，与清单必须是同一个谓词，各写一遍
/// 的表现是「头上说 3 条、列表里 5 行」。
fn draft_filter_sql(query: &DraftInboxQuery<'_>, binds: &mut Vec<Bind>) -> String {
    let mut sql = String::new();
    match query.filter {
        DraftFilter::All => {}
        DraftFilter::WithAudio => {
            sql.push_str(" AND EXISTS(SELECT 1 FROM media m2 WHERE m2.note_path = n.path)");
        }
        DraftFilter::Refined => sql.push_str(" AND n.refined_at IS NOT NULL"),
        DraftFilter::Stale => {
            binds.push(Bind::Int(query.now_ms - crate::SETTLE_BACKLOG_STALE_MS));
            sql.push_str(&format!(" AND n.modified <= ?{}", binds.len()));
        }
    }
    if let Some(text) = query.text.map(str::trim).filter(|t| !t.is_empty()) {
        binds.push(Bind::Text(format!("%{}%", like_escape(text))));
        let at = binds.len();
        sql.push_str(&format!(
            " AND (n.title LIKE ?{at} ESCAPE '\\' OR f.content LIKE ?{at} ESCAPE '\\')"
        ));
    }
    sql
}

/// 转义 `LIKE` 的元字符。不转义的话，用户搜一个 `_` 会命中任意单字符——而那不是他要的。
fn like_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 记录收件箱的一行（PRD-06-058，[`Index::draft_inbox`]）。
///
/// 与 [`NoteListEntry`] 刻意分家而不是加字段：那一个是「按类型列出」的通用条目（四条
/// 功能清单共用），这一个多带**预览**与**来源**，且天然只有记录态才有——把预览塞进
/// 通用条目，等于让每条清单都白拉一次 FTS 全文。
#[derive(Clone, Debug, PartialEq)]
pub struct DraftInboxEntry {
    /// 笔记相对路径。
    pub path: String,
    /// 标题（= 文件名 stem；文字捕获的文件名由首行派生，故它就是首行）。
    pub title: String,
    /// 单行预览（正文首段压成一行，已剥 frontmatter 与时间码）。
    pub preview: String,
    /// 文件修改时间（Unix 毫秒）——时间桶分组与「时龄」都出自它。
    pub modified_ms: i64,
    /// 它是**速记**（`type: quick-note`，收件箱原生条目）还是被标记为记录的普通笔记。
    ///
    /// 下发的是**已判定的布尔**而不是 `type` 字符串：行首那枚小图标要回答的是
    /// 「这条是从哪儿进来的」，而不是让前端再认识一遍类型键（ADR-003）。
    pub quick_note: bool,
    /// 语音速记已跑过 AI 提炼（`luna.refined_at` 有值，PRD-06-058 C′）。
    pub refined: bool,
    /// 有登记的媒体（速记留了原始录音，PRD-02-030）：右键给「重新转写」。
    pub has_media: bool,
    /// 正文里的待办统计（PRD-06-058 G）。
    ///
    /// 取自 FTS 的派生正文（与预览同一份），不回头读文件——这条清单没有条数上界，
    /// 一行一次磁盘 IO 是不可接受的。代价是转写笔记里被剥掉的时间码行不参与统计，
    /// 而那些行本来也不会是待办。
    pub todos: crate::parse::TodoStats,
    /// 关联媒体总时长（毫秒；无媒体为 0）——列表上「这条有多长」那一格（PRD-06-058 A）。
    ///
    /// 取的是既有派生索引，不新增任何持久化字段：`media.duration` 建索引时就写进去了，
    /// 这里只是把它捞出来（复用 [`DURATION_MS_SQL`]，与转写清单同一个表达式）。
    pub duration_ms: i64,
    /// 它已经**搁置到该催了**（修改时间早于知识体检的陈化门槛，PRD-06-055 同一常量）。
    ///
    /// 与「久未整理」那一档筛选同源：判定挪进索引之后，「筛得到」与「带警示色」恒为
    /// 同一批行；此前它在 IPC 层另算一遍，两处迟早会对不上。
    pub stale: bool,
    /// 它能不能参与「多条 → 一篇」（判据 [`crate::merge_block`]，与落盘前重验同源）。
    ///
    /// **不是 `quick_note` 的同义词**：被手工标成 `stage: draft` 的主题页会落进这个
    /// 收件箱（成熟度与聚合身份是两根正交的轴），而把一张聚合页当碎片再合一次是让它
    /// 吞自己。用 `quick_note` 或目录代替这一位，就是把判据搬回前端再写一遍。
    pub mergeable: bool,
}

/// 一位「标签邻居」：与某篇笔记共享标签的另一篇笔记（[`Index::tag_neighbors`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct TagNeighbor {
    /// 邻居的相对路径。
    pub path: String,
    /// 它是不是主题页（`luna.topic: true`）——归入只往主题页里写。
    pub is_topic: bool,
    /// 它的**出身标签**正是源笔记的标签之一（主题页专属；最强的一条归属信号）。
    pub birth: bool,
    /// 与源笔记共享的标签数（大小写不敏感去重后）。
    pub shared: u32,
}

/// Phase 11 度量所需的同一索引快照。路径与标签只在调用栈内存在，装配层会先 HMAC 再落盘。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnowledgeMetricsFacts {
    /// 全部叶子知识笔记（明确排除主题页与知识地图）。
    pub leaf_paths: Vec<String>,
    /// 已被任一真实主题页链接的叶子知识笔记；链接解析与图谱共用同一实现。
    pub linked_leaf_paths: Vec<String>,
    /// 叶子知识笔记的标签簇大小（展示原形 + 大小写不敏感计数）。
    pub topic_clusters: Vec<(String, u32)>,
}

/// 一个可比较向量空间的完整身份。任何字段不同都意味着必须重建派生索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingIndexIdentity {
    pub target_id: String,
    pub runtime_digest: String,
    pub dimension: u32,
    pub profile_version: u32,
    pub normalization: String,
    pub chunk_policy_id: String,
}

/// `embeddings` 表的一行（一个分片的向量与它的有效期凭据）。
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingRow {
    /// 篇内分片序号。
    pub chunk_seq: u32,
    /// 该分片在原文里的起始行（1-based，含 frontmatter）。
    pub line_start: u32,
    /// 向量（f32；bge-m3 出厂即 L2 归一化，故消费侧余弦 = 点积）。
    pub vector: Vec<f32>,
    /// 生成时笔记正文的指纹。
    pub src_hash: String,
    /// 该分片文本自己的指纹（分片级复用的凭据）。
    pub chunk_hash: String,
    /// 生成该向量的完整空间身份。
    pub identity: EmbeddingIndexIdentity,
}

/// f32 序列 → 小端字节。
#[must_use]
fn vec_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// 小端字节 → f32 序列（尾部不足 4 字节的残余丢弃：那种 BLOB 本就是坏的）。
#[must_use]
fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// 派生索引句柄（持有 SQLite 连接）。
pub struct Index {
    conn: Connection,
}

/// 来源版本变化时要刷新状态的逐条出处；ID 与主题 Markdown 中的键完全相同。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicCitationDependency {
    pub topic_path: String,
    pub citation_id: String,
}

impl Index {
    /// 打开（或新建）磁盘索引；`user_version` 落后于内置版本则整库重建（自愈）。
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(map_sqlite)?;
        Self::init(conn)
    }

    /// 内存索引（测试用）。
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        Self::init(conn)
    }

    /// 只读打开磁盘索引（CLI，ADR-141 ④）：不建库、不升级、不改 pragma——应用可能正开着同一份
    /// 索引（回滚日志下读与应用的提交互等，由默认的 5 秒忙等超时吸收），两个进程都去写就没有赢家。索引不存在报 [`VaultError::NotFound`]；
    /// 版本与本程序不一致报 [`VaultError::Decode`]——旧 schema 上跑新查询只会给出错的结果。
    pub fn open_read_only(path: &std::path::Path) -> Result<Self> {
        if !path.is_file() {
            return Err(VaultError::NotFound(path.to_path_buf()));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sqlite)?;
        Self::register_functions(&conn)?;
        let version = Self::schema_version(&conn)?;
        if version != SCHEMA_VERSION {
            return Err(VaultError::Decode(format!(
                "index schema version {version}, expected {SCHEMA_VERSION}"
            )));
        }
        Ok(Self { conn })
    }

    fn schema_version(conn: &Connection) -> Result<i64> {
        conn.pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(map_sqlite)
    }

    fn init(conn: Connection) -> Result<Self> {
        // **不用 WAL**：WAL 的索引是内存映射的 `-shm` 文件，系统一旦收回它的底层 vnode（2026-10-22 真机：
        // 内核记为 ungraft，库在 iCloud 管理的「文稿」里时更容易遇到），下一次读就是 SIGBUS、整个应用
        // 退出。回滚日志没有内存映射，同样的事只是一次 I/O 错误。`TRUNCATE` 而不是 `DELETE`：日志
        // 文件留着、每次提交只截断，不在库目录里反复建删文件（云盘与文件监听都会看到每一次建删）。
        // `synchronous` 用 `FULL`：回滚日志下 `NORMAL` 断电时有极小概率损坏库，而打开库时没有「索引
        // 坏了就重建」的自愈；FULL 在 macOS 上与 NORMAL 实测持平（fsync 不是真刷盘）。代价（3000 篇、
        // 释放构建、对 WAL）：单篇保存约 9→10 ms，改名改写 500 篇引用方约 0.45→0.71 s，CLI 并发读时
        // 保存约 8.5→15 ms，搜索与首次建索引不变或略快。WAL + `locking_mode=EXCLUSIVE` 同样没有内存映射、
        // 写入不慢，但应用开着时 CLI 打不开索引（ADR-141 ④），不取。
        conn.pragma_update(None, "journal_mode", "TRUNCATE").ok();
        conn.pragma_update(None, "synchronous", "FULL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        Self::register_functions(&conn)?;
        if Self::schema_version(&conn)? != SCHEMA_VERSION {
            Self::rebuild_schema(&conn)?;
        }
        Ok(Self { conn })
    }

    /// 连接级 SQLite 函数（每条连接都要注册，读写与只读同一份）。
    fn register_functions(conn: &Connection) -> Result<()> {
        conn.create_scalar_function(
            "luna_ingest_candidate",
            6,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let path: String = ctx.get(0)?;
                let note_type: String = ctx.get(1)?;
                let stage: Option<String> = ctx.get(2)?;
                let is_topic: i64 = ctx.get(3)?;
                let is_knowledge_map: i64 = ctx.get(4)?;
                let body_chars: i64 = ctx.get(5)?;
                let eligible = NoteType::from_key(&note_type).is_some_and(|note_type| {
                    crate::is_ingest_candidate(
                        path.to_ascii_lowercase().ends_with(".md"),
                        note_type,
                        NoteStage::from_index_row(note_type, stage.is_some()),
                        is_topic != 0 || is_knowledge_map != 0,
                        body_chars > 0,
                    )
                });
                Ok(i64::from(eligible))
            },
        )
        .map_err(map_sqlite)
    }

    /// 丢弃并重建空 schema（自愈/一键重建索引的底座，§3）。调用方随后重新索引全 Vault。
    ///
    /// **删的是库里现有的全部表，而不是一份手写清单**：v14 加了 `ai_tag_ignores` /
    /// `ai_tags_suppressed` 却没进清单，之后任何一次升版本都在 `CREATE TABLE ai_tag_ignores`
    /// 上报「已存在」——表结构停在半截、版本号不变，每次启动重试、每次失败，库打不开
    /// （笔记库对标轮 V1 真机发现）。这个文件里全是派生数据（`ai_tag_ignores` 每次开库从随库
    /// 清单重同步），没有哪张表该活过一次升版本。先删虚表（连带其影子表），再删其余；整个
    /// 过程一个事务，中途失败原样回滚，不留半截。
    fn rebuild_schema(conn: &Connection) -> Result<()> {
        let tables = |virtual_only: bool| -> Result<Vec<String>> {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
                     AND (sql LIKE 'CREATE VIRTUAL TABLE%') = ?1",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![virtual_only], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<_, _>>()
                .map_err(map_sqlite)
        };
        let drop = |name: &str| {
            conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS \"{}\"",
                name.replace('"', "\"\"")
            ))
            .map_err(map_sqlite)
        };
        conn.execute_batch("BEGIN IMMEDIATE").map_err(map_sqlite)?;
        let out = (|| -> Result<()> {
            for name in tables(true)? {
                drop(&name)?;
            }
            for name in tables(false)? {
                drop(&name)?;
            }
            conn.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(map_sqlite)
        })();
        let end = if out.is_ok() { "COMMIT" } else { "ROLLBACK" };
        conn.execute_batch(end).map_err(map_sqlite)?;
        out
    }

    /// 清空**从 Vault 文件投影出来的**派生行（保留 schema），供一键重建前清场。
    ///
    /// **刻意不清 `embeddings` / `checkup_findings`**：重扫的语义是「从文件重新算一遍投影」，
    /// 而这两张表都**不是从文件算得出来的**——嵌入向量（S16 实测万片约 40 分钟）与深度体检的
    /// 判定各自烧过一轮模型。而 `scan()` 在每次打开库时
    /// 都会跑，一并清掉等于它们永远活不过一次启动，那样存不存都一样。两者的失效都由
    /// **源指纹/正文比对**负责（向量另加「模型是否换过」），删除由各自的显式路径负责。
    /// 一篇笔记在索引里的内容指纹（没入索引为 `None`）。库外改名配对要在同步索引**之前**
    /// 取「消失的那篇」原来是什么内容（PRD-06-030 ⑤）。
    pub fn note_hash(&self, rel_path: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT hash FROM notes WHERE path = ?1",
                params![rel_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)
    }

    /// 已入索引的每篇笔记：路径 → （修改时间毫秒, 内容指纹）。增量扫描据此判「变没变」。
    pub fn fingerprints(&self) -> Result<std::collections::HashMap<String, (i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, modified, hash FROM notes")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, i64>(1)?, r.get::<_, String>(2)?),
                ))
            })
            .map_err(map_sqlite)?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (path, fingerprint) = row.map_err(map_sqlite)?;
            out.insert(path, fingerprint);
        }
        Ok(out)
    }

    /// 内容没变、只是修改时间变了（同步盘下载回来常见）：只记新的修改时间，不重建这一篇。
    pub fn touch_modified(&self, rel_path: &str, modified_ms: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE notes SET modified = ?2 WHERE path = ?1",
                params![rel_path, modified_ms],
            )
            .map(|_| ())
            .map_err(map_sqlite)
    }

    /// 在一个事务里做一批索引写入：一篇笔记入索引是好几条语句，逐条自动提交时每条都要落一次
    /// 日志；整批一次提交把扫描从「每篇几毫秒」降到「每篇零点几毫秒」（笔记库对标轮 V1 基准）。
    pub fn in_transaction<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        let out = work()?;
        tx.commit().map_err(map_sqlite)?;
        Ok(out)
    }

    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "DELETE FROM notes; DELETE FROM media; DELETE FROM segments;
                 DELETE FROM links; DELETE FROM topic_citation_dependencies;
                 DELETE FROM tags; DELETE FROM aliases; DELETE FROM ai_tags_suppressed; DELETE FROM fts;",
            )
            .map_err(map_sqlite)
    }

    /// 用随库权威清单覆盖忽略副本，并双向搬行：新忽略的 AI 标签从 `tags` 压下，恢复的从压下表
    /// 搬回。只动派生索引，不读不写任何笔记文件；同一清单重复同步是空操作。
    pub fn sync_ai_tag_ignores(&self, ignored: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite)?;
        tx.execute("DELETE FROM ai_tag_ignores", [])
            .map_err(map_sqlite)?;
        for tag in ignored {
            tx.execute(
                "INSERT OR IGNORE INTO ai_tag_ignores(tag) VALUES (lower(?1))",
                params![tag],
            )
            .map_err(map_sqlite)?;
        }
        tx.execute_batch(
            "INSERT INTO ai_tags_suppressed(note_path, tag)
               SELECT note_path, tag FROM tags
                WHERE source = 'ai' AND lower(tag) IN (SELECT tag FROM ai_tag_ignores);
             DELETE FROM tags
                WHERE source = 'ai' AND lower(tag) IN (SELECT tag FROM ai_tag_ignores);
             INSERT INTO tags(note_path, tag, source)
               SELECT note_path, tag, 'ai' FROM ai_tags_suppressed
                WHERE lower(tag) NOT IN (SELECT tag FROM ai_tag_ignores);
             DELETE FROM ai_tags_suppressed
                WHERE lower(tag) NOT IN (SELECT tag FROM ai_tag_ignores);",
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)
    }

    /// 带这条标签或其子标签的笔记（任一来源，含被忽略而压下的 AI 标签），按路径排序。
    /// 标签改名的影响面（ADR-135）；比较口径与索引一致（ASCII 大小写不敏感）。
    pub fn notes_with_tag_branch(&self, tag: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT note_path FROM tags
                  WHERE lower(tag) = lower(?1) OR lower(tag) LIKE lower(?1) || '/%'
                 UNION
                 SELECT note_path FROM ai_tags_suppressed
                  WHERE lower(tag) = lower(?1) OR lower(tag) LIKE lower(?1) || '/%'
                 ORDER BY 1",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![tag], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 库里有没有这条标签本身（任一来源）——改名目标已存在即合并。
    pub fn tag_exists(&self, tag: &str) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tags WHERE lower(tag) = lower(?1))",
                params![tag],
                |row| row.get(0),
            )
            .map_err(map_sqlite)
    }

    /// 被全库忽略的 AI 标签各压下了几篇（设置/标签页的「已忽略」清单用；按标签名排序）。
    pub fn suppressed_ai_tag_counts(&self) -> Result<Vec<(String, u32)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT i.tag, COUNT(DISTINCT s.note_path) FROM ai_tag_ignores i
                   LEFT JOIN ai_tags_suppressed s ON lower(s.tag) = i.tag
                  GROUP BY i.tag ORDER BY i.tag",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 回收 SQLite 文件里已删除页的空间（`VACUUM`）。「清缓存」之后才有意义：只删行
    /// 文件不会变小，用户看到的占用数字纹丝不动，等于"点了没反应"（真机 2026-09-06）。
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM;").map_err(map_sqlite)
    }

    /// 索引（或重索引）一篇笔记。`modified_ms` 由调用方提供（C4 的 mtime）。
    ///
    /// 整篇是一个原子单元：全文行与 notes 行要么同在、要么同无。中途失败留下一行没有 notes
    /// 指向的全文行，按行号删除就再也删不到它，搜索里这篇会出现两次。
    ///
    /// 已在外层事务里（增量扫描的批，[`Index::in_transaction`]）就不另开：出错一路 `?` 上抛、
    /// 整批回滚。**不用保存点**——开着保存点时 SQLite 要为每个被改的页记子日志，1 万篇首次
    /// 接入实测从 1.7 s 涨到 3.4 s。单独调用时开一个事务，顺带把原先每条语句各自提交的
    /// 十来次提交并成一次。
    pub fn index_note(&self, rel_path: &str, content: &str, modified_ms: i64) -> Result<()> {
        // 隐藏路径不进索引（唯一一处判定，所有写入路径都经过这里）：「显示隐藏文件」下打开
        // 编辑一篇点文件时，它若进了索引就会出现在搜索里，而下次启动扫描又会把它当成已删除。
        if crate::pathutil::is_hidden_rel(rel_path) {
            return Ok(());
        }
        if !self.conn.is_autocommit() {
            return self.index_note_inner(rel_path, content, modified_ms);
        }
        self.in_transaction(|| self.index_note_inner(rel_path, content, modified_ms))
    }

    fn index_note_inner(&self, rel_path: &str, content: &str, modified_ms: i64) -> Result<()> {
        let meta = read_luna(content);
        let title = title_of(rel_path);
        let hash = ContentHash::of(content.as_bytes());
        let cleaned = clean_for_fts(content);

        self.remove_note(rel_path)?;

        // 正文字符数按 `body_of` 口径（与所有权指纹、蒸馏指纹同源）；trim 后计数——
        // 只剩空白行的页与真空页对用户是同一件事（"这一页什么都没有"）。
        let body_chars = crate::frontmatter::body_of(content).trim().chars().count() as i64;
        self.conn
            .execute(
                "INSERT INTO fts(note_path, content) VALUES (?1, ?2)",
                params![rel_path, cleaned],
            )
            .map_err(map_sqlite)?;
        let fts_rowid = self.conn.last_insert_rowid();
        self.conn
            .execute(
                "INSERT INTO notes(path, title, type, created, modified, hash, topic_src,
                                   is_topic, is_knowledge_map, body_chars, distill_note, stage,
                                   refined_at, created_ms, fts_rowid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    rel_path,
                    title,
                    note_type_str(meta.note_type),
                    meta.created,
                    modified_ms,
                    hash.as_str(),
                    meta.topic_src,
                    i64::from(meta.topic),
                    i64::from(meta.knowledge_map),
                    body_chars,
                    meta.distill_note,
                    // 成熟度轴（ADR-086）：判定在 `NoteStage::resolve` 一处，这里只落它的结论。
                    meta.stage().column(),
                    meta.refined_at,
                    created_ms_of(meta.created.as_deref()),
                    fts_rowid,
                ],
            )
            .map_err(map_sqlite)?;

        for term in extract_tags(content) {
            // 全库忽略的 AI 标签进压下表；用户自己写的同名标签在 `extract_tags` 里已优先，
            // 不会走到这里被压下。
            let suppressed = term.source == crate::parse::TagSource::Ai
                && self
                    .conn
                    .query_row(
                        "SELECT 1 FROM ai_tag_ignores WHERE tag = lower(?1)",
                        params![term.tag],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(map_sqlite)?
                    .is_some();
            if suppressed {
                self.conn
                    .execute(
                        "INSERT INTO ai_tags_suppressed(note_path, tag) VALUES (?1, ?2)",
                        params![rel_path, term.tag],
                    )
                    .map_err(map_sqlite)?;
            } else {
                self.conn
                    .execute(
                        "INSERT INTO tags(note_path, tag, source) VALUES (?1, ?2, ?3)",
                        params![rel_path, term.tag, term.source.key()],
                    )
                    .map_err(map_sqlite)?;
            }
        }

        for alias in crate::parse::frontmatter_aliases(content) {
            self.conn
                .execute(
                    "INSERT INTO aliases(note_path, alias) VALUES (?1, ?2)",
                    params![rel_path, alias],
                )
                .map_err(map_sqlite)?;
        }

        // 存**规范形**而不是链接原样（schema v7）：这一列的消费者（反链、外链面板、
        // 图谱、断链体检）要的都是「库内路径」，而链接是相对源笔记写的、还转过义。
        // 换算只有一处口径（`canonical_link_target`），故不会再出现"某个面把它当路径用、
        // 另一个面把它当链接用"。
        for dst in extract_links(content) {
            let dst = crate::pathutil::canonical_link_target(rel_path, &dst);
            self.conn
                .execute(
                    "INSERT INTO links(src_path, dst_path) VALUES (?1, ?2)",
                    params![rel_path, dst],
                )
                .map_err(map_sqlite)?;
        }

        // 解析失败的旧/未知/坏格式主题由维护状态明确报出；索引扫描本身不能被一篇坏文件
        // 阻断。只把可验证的新格式出处入派生表，来源删除时保留依赖以供失效定位。
        if meta.topic && !meta.knowledge_map {
            if let Ok(citations) = crate::knowledge_evidence::topic_citations(rel_path, content) {
                for citation in citations {
                    if let Some(source_path) = citation.source_path {
                        self.conn
                            .execute(
                                "INSERT INTO topic_citation_dependencies(topic_path, citation_id, source_path)
                                 VALUES (?1, ?2, ?3)",
                                params![rel_path, citation.id, source_path],
                            )
                            .map_err(map_sqlite)?;
                    }
                }
            }
        }

        for m in &meta.media {
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO media(path, note_path, duration, has_transcript, has_translation, model)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        m.path,
                        rel_path,
                        m.duration,
                        m.transcript as i64,
                        m.translation as i64,
                        m.model,
                    ],
                )
                .map_err(map_sqlite)?;
        }
        Ok(())
    }

    /// 从索引移除一篇笔记的所有派生行（删除/move 前调用）。
    pub fn remove_note(&self, rel_path: &str) -> Result<()> {
        let tx = |sql: &str| {
            self.conn
                .execute(sql, params![rel_path])
                .map_err(map_sqlite)
                .map(|_| ())
        };
        // 先删全文行（按 notes 记下的行号），再删 notes——反过来就找不到行号了。
        tx("DELETE FROM fts WHERE rowid = (SELECT fts_rowid FROM notes WHERE path = ?1)")?;
        tx("DELETE FROM notes WHERE path = ?1")?;
        tx("DELETE FROM tags WHERE note_path = ?1")?;
        tx("DELETE FROM aliases WHERE note_path = ?1")?;
        tx("DELETE FROM ai_tags_suppressed WHERE note_path = ?1")?;
        tx("DELETE FROM links WHERE src_path = ?1")?;
        tx("DELETE FROM topic_citation_dependencies WHERE topic_path = ?1")?;
        // 该笔记的 media → 其 segments 一并清。
        self.conn
            .execute(
                "DELETE FROM segments WHERE media_path IN (SELECT path FROM media WHERE note_path = ?1)",
                params![rel_path],
            )
            .map_err(map_sqlite)?;
        tx("DELETE FROM media WHERE note_path = ?1")?;
        // **刻意不清 `embeddings`**：本方法同时服务「笔记没了」与 [`Index::index_note`] 的
        // 「重索引前清旧行」两种语境，而后者每次写盘都会走一遍。真删除由
        // [`Index::forget_note`] 显式承担（删除/移动两处调用点）。
        Ok(())
    }

    /// 按来源定位受影响声明；稳定键分页，避免一次来源被大量主题引用时全表读入。
    /// 来源文件删除仍可查到依赖，因为删除只清该文件作为主题的投影行。
    pub fn topic_citation_dependents(
        &self,
        source_path: &str,
        after: Option<(&str, &str)>,
        limit: u32,
    ) -> Result<Vec<TopicCitationDependency>> {
        let (after_topic, after_citation) = after.unwrap_or(("", ""));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT topic_path, citation_id FROM topic_citation_dependencies
                 WHERE source_path = ?1
                   AND (topic_path > ?2 OR (topic_path = ?2 AND citation_id > ?3))
                 ORDER BY topic_path, citation_id LIMIT ?4",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![
                    source_path,
                    after_topic,
                    after_citation,
                    i64::from(limit.clamp(1, 100))
                ],
                |row| {
                    Ok(TopicCitationDependency {
                        topic_path: row.get(0)?,
                        citation_id: row.get(1)?,
                    })
                },
            )
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    // ── 语义检索的向量（PRD-16-028 / ADR-027）────────────────────────────────

    /// 顶替一篇笔记的全部向量（先删后插，同一事务）。
    ///
    /// **先删后插而不是 upsert**：重新分片后片数可能变少（用户删了一段），残留的旧片
    /// 会以一个**指向已不存在内容**的向量继续参与检索——而它的 `src_hash` 是新的，
    /// 于是没有任何机制会发现它过期。这与批次 3 `slot.sources` 那条教训同源。
    pub fn put_embeddings(&self, note_path: &str, rows: &[EmbeddingRow]) -> Result<()> {
        for row in rows {
            if row.identity.dimension == 0
                || row.vector.len() != row.identity.dimension as usize
                || row.identity.target_id.is_empty()
                || row.identity.runtime_digest.is_empty()
                || row.identity.normalization.is_empty()
                || row.identity.chunk_policy_id.is_empty()
            {
                return Err(VaultError::InvalidReplace(format!(
                    "嵌入向量身份无效：目标={}，声明维度={}，实际维度={}",
                    row.identity.target_id,
                    row.identity.dimension,
                    row.vector.len()
                )));
            }
        }
        self.conn
            .execute(
                "DELETE FROM embeddings WHERE note_path = ?1",
                params![note_path],
            )
            .map_err(map_sqlite)?;
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO embeddings
                     (note_path, chunk_seq, line_start, vec, src_hash, chunk_hash,
                      target_id, runtime_digest, dimension, profile_version, normalization,
                      chunk_policy_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )
            .map_err(map_sqlite)?;
        for row in rows {
            stmt.execute(params![
                note_path,
                i64::from(row.chunk_seq),
                i64::from(row.line_start),
                vec_to_blob(&row.vector),
                row.src_hash,
                row.chunk_hash,
                row.identity.target_id,
                row.identity.runtime_digest,
                i64::from(row.identity.dimension),
                i64::from(row.identity.profile_version),
                row.identity.normalization,
                row.identity.chunk_policy_id,
            ])
            .map_err(map_sqlite)?;
        }
        Ok(())
    }

    /// 一篇笔记的向量此刻记的是哪个指纹 / 哪个向量空间（`None` = 从未嵌入过）。
    ///
    /// 增量建库的幂等门控只需要这一个问题的答案，故**不读向量本身**——那是几十 KB 的
    /// BLOB，为了判断"要不要重算"去读它是纯浪费。
    pub fn embedding_state(
        &self,
        note_path: &str,
    ) -> Result<Option<(String, EmbeddingIndexIdentity)>> {
        self.conn
            .query_row(
                "SELECT src_hash, target_id, runtime_digest, dimension, profile_version,
                        normalization, chunk_policy_id
                 FROM embeddings WHERE note_path = ?1 LIMIT 1",
                params![note_path],
                |r| {
                    Ok((
                        r.get(0)?,
                        EmbeddingIndexIdentity {
                            target_id: r.get(1)?,
                            runtime_digest: r.get(2)?,
                            dimension: r.get::<_, i64>(3)? as u32,
                            profile_version: r.get::<_, i64>(4)? as u32,
                            normalization: r.get(5)?,
                            chunk_policy_id: r.get(6)?,
                        },
                    ))
                },
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(map_sqlite(other)),
            })
    }

    /// 读出**指定向量空间**的全部向量（启动时建内存矩阵用）。
    ///
    /// 六个身份字段必须全部相等；只看模型名无法识别模型内容、维度或分块策略变化。
    pub fn all_embeddings(
        &self,
        identity: &EmbeddingIndexIdentity,
    ) -> Result<Vec<(EmbeddingRow, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT note_path, chunk_seq, line_start, vec, src_hash, chunk_hash
                 FROM embeddings
                 WHERE target_id = ?1 AND runtime_digest = ?2 AND dimension = ?3
                   AND profile_version = ?4 AND normalization = ?5 AND chunk_policy_id = ?6
                 ORDER BY note_path, chunk_seq",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![
                    identity.target_id,
                    identity.runtime_digest,
                    i64::from(identity.dimension),
                    i64::from(identity.profile_version),
                    identity.normalization,
                    identity.chunk_policy_id,
                ],
                |r| {
                    let blob: Vec<u8> = r.get(3)?;
                    Ok((
                        EmbeddingRow {
                            chunk_seq: r.get::<_, i64>(1)? as u32,
                            line_start: r.get::<_, i64>(2)? as u32,
                            vector: blob_to_vec(&blob),
                            src_hash: r.get(4)?,
                            chunk_hash: r.get(5)?,
                            identity: identity.clone(),
                        },
                        r.get::<_, String>(0)?,
                    ))
                },
            )
            .map_err(map_sqlite)?;
        let rows = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        if rows
            .iter()
            .any(|(row, _)| row.vector.len() != identity.dimension as usize)
        {
            return Err(VaultError::InvalidReplace(format!(
                "嵌入索引维度与身份不一致：声明 {}",
                identity.dimension
            )));
        }
        Ok(rows)
    }

    /// 已嵌入的笔记数与分片数（设置页与建库预检如实报数用）。
    pub fn embedding_counts(&self, identity: &EmbeddingIndexIdentity) -> Result<(u32, u32)> {
        self.conn
            .query_row(
                "SELECT COUNT(DISTINCT note_path), COUNT(*) FROM embeddings
                 WHERE target_id = ?1 AND runtime_digest = ?2 AND dimension = ?3
                   AND profile_version = ?4 AND normalization = ?5 AND chunk_policy_id = ?6",
                params![
                    identity.target_id,
                    identity.runtime_digest,
                    i64::from(identity.dimension),
                    i64::from(identity.profile_version),
                    identity.normalization,
                    identity.chunk_policy_id,
                ],
                |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32)),
            )
            .map_err(map_sqlite)
    }

    /// 一篇笔记**此刻在库里**的「分片指纹 → 向量」映射（分片级复用的取数口）。
    ///
    /// 改一行只有那一两片的文本变了，其余分片的向量原样可用——重嵌它们纯属浪费
    /// （S16 实测 ~0.25 秒/片，一篇 40 片的长笔记就是 10 秒 GPU）。
    pub fn reusable_vectors(
        &self,
        note_path: &str,
        identity: &EmbeddingIndexIdentity,
    ) -> Result<std::collections::HashMap<String, Vec<f32>>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT chunk_hash, vec FROM embeddings
                 WHERE note_path = ?1 AND target_id = ?2 AND runtime_digest = ?3
                   AND dimension = ?4 AND profile_version = ?5 AND normalization = ?6
                   AND chunk_policy_id = ?7",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![
                    note_path,
                    identity.target_id,
                    identity.runtime_digest,
                    i64::from(identity.dimension),
                    i64::from(identity.profile_version),
                    identity.normalization,
                    identity.chunk_policy_id,
                ],
                |r| {
                    let blob: Vec<u8> = r.get(1)?;
                    Ok((r.get::<_, String>(0)?, blob_to_vec(&blob)))
                },
            )
            .map_err(map_sqlite)?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (hash, vector) = row.map_err(map_sqlite)?;
            if vector.len() != identity.dimension as usize {
                return Err(VaultError::InvalidReplace(format!(
                    "可复用向量维度与身份不一致：声明 {}，实际 {}",
                    identity.dimension,
                    vector.len()
                )));
            }
            out.insert(hash, vector);
        }
        Ok(out)
    }

    /// 删掉一篇笔记的全部向量。
    pub fn delete_embeddings(&self, note_path: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM embeddings WHERE note_path = ?1",
                params![note_path],
            )
            .map_err(map_sqlite)
            .map(|_| ())
    }

    /// 删掉**非指定向量空间**的全部向量（身份变化后的清场：旧向量不可用也白占磁盘）。
    /// 返回删掉的行数。
    pub fn prune_embeddings_except(&self, keep: &EmbeddingIndexIdentity) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM embeddings
                 WHERE target_id <> ?1 OR runtime_digest <> ?2 OR dimension <> ?3
                    OR profile_version <> ?4 OR normalization <> ?5 OR chunk_policy_id <> ?6",
                params![
                    keep.target_id,
                    keep.runtime_digest,
                    i64::from(keep.dimension),
                    i64::from(keep.profile_version),
                    keep.normalization,
                    keep.chunk_policy_id,
                ],
            )
            .map_err(map_sqlite)
    }

    /// 一篇笔记**真的不在了**（删除或移动走）：连同不属于投影的派生物一起忘掉。
    ///
    /// 与 [`Index::remove_note`] 分开的理由见后者的注释——重索引也走 `remove_note`，
    /// 两者若合一，每次保存都会把这篇的向量一并抹掉。
    pub fn forget_note(&self, rel_path: &str) -> Result<()> {
        self.remove_note(rel_path)?;
        self.delete_embeddings(rel_path)
    }

    /// 从索引批量移除文件夹及其下所有子笔记的派生行。
    pub fn remove_directory(&self, rel_dir: &str) -> Result<()> {
        let prefix = format!("{}/", rel_dir);
        let tx = |sql: &str| {
            self.conn
                .execute(sql, params![rel_dir, prefix])
                .map_err(map_sqlite)
                .map(|_| ())
        };
        // 先 fts 后 notes：全文行号记在 notes 里。
        tx("DELETE FROM fts WHERE rowid IN (SELECT fts_rowid FROM notes WHERE path = ?1 OR path LIKE ?2 || '%')")?;
        tx("DELETE FROM notes WHERE path = ?1 OR path LIKE ?2 || '%'")?;
        tx("DELETE FROM tags WHERE note_path = ?1 OR note_path LIKE ?2 || '%'")?;
        tx("DELETE FROM aliases WHERE note_path = ?1 OR note_path LIKE ?2 || '%'")?;
        tx("DELETE FROM links WHERE src_path = ?1 OR src_path LIKE ?2 || '%'")?;
        self.conn
            .execute(
                "DELETE FROM segments WHERE media_path IN (SELECT path FROM media WHERE note_path = ?1 OR note_path LIKE ?2 || '%')",
                params![rel_dir, prefix],
            )
            .map_err(map_sqlite)?;
        tx("DELETE FROM media WHERE note_path = ?1 OR note_path LIKE ?2 || '%'")?;
        tx("DELETE FROM embeddings WHERE note_path = ?1 OR note_path LIKE ?2 || '%'")?;
        Ok(())
    }

    // ── 知识体检（PRD-16-030/031）────────────────────────────────────────────

    /// T0 即时体检的**全部索引侧输入**，一次查齐（PRD-16-030 的「毫秒级」就落在这里：
    /// 四条 SQL，零文件读盘）。
    pub fn checkup_rows(&self) -> Result<crate::checkup::CheckupRows> {
        let mut notes = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT path, title, type, is_topic, is_knowledge_map, body_chars, modified,
                            stage
                     FROM notes
                     ORDER BY path",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(crate::checkup::CheckupNote {
                        path: r.get(0)?,
                        title: r.get(1)?,
                        note_type: r.get(2)?,
                        is_topic: r.get::<_, i64>(3)? != 0,
                        is_knowledge_map: r.get::<_, i64>(4)? != 0,
                        body_chars: u32::try_from(r.get::<_, i64>(5)?).unwrap_or(u32::MAX),
                        modified_ms: r.get(6)?,
                        draft: row_draft(r.get(7)?),
                    })
                })
                .map_err(map_sqlite)?;
            for row in rows {
                notes.push(row.map_err(map_sqlite)?);
            }
        }
        let mut links = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT src_path, dst_path FROM links")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(map_sqlite)?;
            for row in rows {
                links.push(row.map_err(map_sqlite)?);
            }
        }
        let mut tags = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT note_path, tag FROM tags")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(map_sqlite)?;
            for row in rows {
                tags.push(row.map_err(map_sqlite)?);
            }
        }
        // 待整理积压：资料区、有正文、还没有整理笔记。判定的**完整版**（含「源已更新」
        // 那一档）在 `Vault::distill_scan`，它要逐篇读文件比对指纹——T0 只回答"有多少篇
        // 从来没整理过"，点进去的批量确认卡再给全口径的四桶数字。
        let keys: Vec<&str> = NoteType::ALL
            .into_iter()
            .filter(|t| t.is_source_material())
            .map(NoteType::key)
            .collect();
        let placeholders = (1..=keys.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let distill_backlog: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM notes
                     WHERE type IN ({placeholders}) AND distill_note IS NULL AND body_chars > 0"
                ),
                params_from_iter(keys.iter()),
                |r| r.get(0),
            )
            .map_err(map_sqlite)?;

        Ok(crate::checkup::CheckupRows {
            notes,
            links,
            tags,
            aliases: self.note_aliases()?,
            distill_backlog: u32::try_from(distill_backlog).unwrap_or(u32::MAX),
            // 设置读不到索引层，由 `Vault::checkup_t0` 的调用方显式覆盖（见那里的说明）。
            ai_tags_enabled: false,
            // 「现在几点」同理不属于索引层：`0` 让所有笔记看起来来自未来，于是待沉淀
            // 积压不报——忘了覆盖的后果是少报一条，不是凭空催人。
            now_ms: 0,
        })
    }

    /// 清空深度体检的发现（一次新的深度体检**开始时**调用）。
    ///
    /// 报告的语义是「最近一次深度体检看到的」，故新一轮开跑即翻篇——留着上一轮的结果与
    /// 这一轮的混在一起，用户无从知道哪条是刚判的、哪条是三周前那次留下的。
    pub fn clear_checkup_findings(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM checkup_findings", [])
            .map_err(map_sqlite)
            .map(|_| ())
    }

    /// 落一条深度体检发现（**边跑边落**：半小时的判定被中途取消时，已判出来的不该丢）。
    pub fn put_checkup_finding(
        &self,
        finding: &crate::checkup::Finding,
        created_ms: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO checkup_findings(id, kind, paths, line, subject, reason, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                   kind = excluded.kind, paths = excluded.paths, line = excluded.line,
                   subject = excluded.subject, reason = excluded.reason,
                   created_ms = excluded.created_ms",
                params![
                    finding.id,
                    finding.kind.key(),
                    serde_json::to_string(&finding.paths).unwrap_or_else(|_| "[]".to_string()),
                    i64::from(finding.line.unwrap_or(0)),
                    finding.subject,
                    finding.reason,
                    created_ms,
                ],
            )
            .map_err(map_sqlite)
            .map(|_| ())
    }

    /// 读回全部深度体检发现（有效性校验在 `Vault` 侧——那要拿此刻的正文比对）。
    pub fn checkup_findings(&self) -> Result<Vec<crate::checkup::Finding>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, paths, line, subject, reason FROM checkup_findings ORDER BY id",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| {
                let kind: String = r.get(1)?;
                let line: i64 = r.get(3)?;
                Ok((
                    kind,
                    crate::checkup::Finding {
                        id: r.get(0)?,
                        // 占位：kind 在下面用 `from_key` 还原，还原不出的行如实丢弃。
                        kind: crate::checkup::FindingKind::Contradiction,
                        paths: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                        line: u32::try_from(line).ok().filter(|n| *n > 0),
                        subject: r.get(4)?,
                        reason: r.get(5)?,
                        count: 0,
                    },
                ))
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, mut finding) = row.map_err(map_sqlite)?;
            // 未知 kind = 表里躺着一条本版本读不懂的行（降级安装等）→ 丢弃，不猜。
            let Some(kind) = crate::checkup::FindingKind::from_key(&kind) else {
                continue;
            };
            finding.kind = kind;
            out.push(finding);
        }
        Ok(out)
    }

    /// 删除一条深度体检发现（用户按 ✕ 忽略 / 读侧判定它已不成立）。
    pub fn delete_checkup_finding(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM checkup_findings WHERE id = ?1", params![id])
            .map_err(map_sqlite)
            .map(|_| ())
    }

    /// 搜索（结构化过滤 + 全文，返回带 snippet 的命中，§3）。
    ///
    /// `q.page` 决定语义（TDD-07 §4.1a）：`None` = top-N（既有语义），`Some` = 列表分页
    /// （键集游标，可穷尽遍历）。**全文/LIKE 路径不接受 `page`** ——相关度序不是存储列、
    /// 随查询计算且不唯一，做不出稳定游标；给它硬凑一个只会把这份不确定性固化成跨页漏行。
    pub fn search(&self, q: &Query) -> Result<Vec<SearchHit>> {
        let limit = match (q.limit, q.page.is_some()) {
            (0, true) => LIST_PAGE_SIZE,
            (0, false) => 50,
            (n, _) => n,
        } as i64;
        match q.text.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(text) => {
                if q.page.is_some() {
                    return Err(VaultError::ListCursorInvalid(
                        "全文检索按相关度排序，本期不分页（TDD-07 §4.1a）".to_owned(),
                    ));
                }
                let trigram_ok = text.chars().count() >= 3;
                if trigram_ok {
                    self.search_fts(text, q, limit)
                } else {
                    self.search_like(text, q, limit)
                }
            }
            None => self.search_filter_only(q, limit),
        }
    }

    /// 全文检索（trigram FTS，相关度序）。
    ///
    /// **它不分页**（契约本期的裁决）：bm25 不是存储列、随查询计算且不唯一，做不出稳定
    /// 游标。但"不分页"不等于"返回序可以不确定"——`ORDER BY score` 单独一列时，**平分的
    /// 命中之间返回序未定义**：同一个词搜两次，可能给出不同的前 N 条边界，而没有任何东西
    /// 会因此变红。故补 `n.path` 作次序键：这是**取数可复现性**，与分页是两件事。
    fn search_fts(&self, text: &str, q: &Query, limit: i64) -> Result<Vec<SearchHit>> {
        let mut sql = format!(
            "SELECT n.path, n.title, bm25(fts) AS score, \
             snippet(fts, 1, '\u{300a}', '\u{300b}', '\u{2026}', 12) AS snip, n.modified, \
             {DURATION_MS_SQL} AS dur, {HIT_EXTRA_SQL} \
             FROM fts JOIN notes n ON n.path = fts.note_path WHERE fts MATCH ?1",
        );
        // trigram MATCH：用引号包裹为短语，避免词法符号干扰中文子串。
        let mut binds: Vec<Bind> = vec![Bind::Text(format!("\"{}\"", text.replace('"', "")))];
        self.append_filters(&mut sql, &mut binds, q);
        sql.push_str(" ORDER BY score, n.path ASC LIMIT ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Bind::Int(limit));
        self.run_hits(&sql, &binds)
    }

    /// 短词（<3 字符，trigram 失效）的 LIKE 兜底路径。
    ///
    /// **它现在有确定性排序了**：此前只有 `LIMIT`，返回序完全未定义——同一个库、同一个
    /// 词，"前 N 条"是哪 N 条取决于 SQLite 这一次怎么扫表。它与全文检索同属"检索"语义
    /// （契约里那两条路的**第二种**处置：明确不分页、如实说明取的是前 N 条），故排序键取
    /// **修改时间倒序 + path 次序**：LIKE 路径的 `score` 恒为 0，"最相关"在这里无从谈起，
    /// 而"最近改过的先给你看"是这套界面通用的那一条，且它可复现。
    fn search_like(&self, text: &str, q: &Query, limit: i64) -> Result<Vec<SearchHit>> {
        let mut sql = format!(
            "SELECT n.path, n.title, 0.0 AS score, fts.content AS snip, n.modified, \
             {DURATION_MS_SQL} AS dur, {HIT_EXTRA_SQL} \
             FROM fts JOIN notes n ON n.path = fts.note_path WHERE fts.content LIKE '%'||?1||'%' ESCAPE '\\'",
        );
        let mut binds: Vec<Bind> = vec![Bind::Text(escape_like(text))];
        self.append_filters(&mut sql, &mut binds, q);
        sql.push_str(" ORDER BY n.modified DESC, n.path ASC LIMIT ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(Bind::Int(limit));
        let mut hits = self.run_hits(&sql, &binds)?;
        // LIKE 路径手工裁片段（trigram snippet 不可用）。
        for h in &mut hits {
            h.snippet = manual_snippet(&h.snippet, text);
        }
        Ok(hits)
    }

    /// 结构化列表查询（无全文约束）：功能列表（录音 / 导入与转写 / 标签结果）走这条。
    ///
    /// 这是三条键集分页路径里唯一支持全部三个排序键的——另两条（记录收件箱 / 按类型列出）
    /// 的语义本身就钉死在修改时间上，给它们开排序参数只是造一个没有消费者的旋钮。
    fn search_filter_only(&self, q: &Query, limit: i64) -> Result<Vec<SearchHit>> {
        let mut sql = format!(
            "SELECT n.path, n.title, 0.0 AS score, '' AS snip, n.modified, \
             {DURATION_MS_SQL} AS dur, {HIT_EXTRA_SQL} \
             FROM notes n WHERE 1=1",
        );
        let mut binds: Vec<Bind> = Vec::new();
        self.append_filters(&mut sql, &mut binds, q);
        let page = q.page.clone().unwrap_or_default();
        // 多取一条只为回答"后面还有没有"：末行游标据此给 `None`，前端因此不必拿
        // 「这一页有没有取满」去反推是否到底（那个反推要求它知道页大小）。
        append_keyset(&mut sql, &mut binds, &page, limit + 1)?;
        let mut hits = self.run_hits(&sql, &binds)?;
        let next = take_page(&mut hits, limit, page.sort, |hit| {
            (hit_sort_value(hit, page.sort), hit.path.clone())
        });
        // 游标挂在**末行**上，因为这条命令的返回是裸数组：它同时服务两个 top-N 消费者
        // （链接补全 / 悬浮菜单「查找相关笔记」），套一层信封就得改它们的取值，而
        // TDD-07 §4.1a 定的是「`limit` 语义不变 ⇒ 既有消费者零改动」。
        // top-N 语义（`page` 未给）下末行同样是 `None`——那条路径本来就没有下一页。
        if q.page.is_some() {
            if let Some(last) = hits.last_mut() {
                last.cursor = next;
            }
        }
        Ok(hits)
    }

    /// 追加结构化过滤子句（type/tag/path/名称子串），绑定参数按 1-based 顺序续接。
    fn append_filters(&self, sql: &mut String, binds: &mut Vec<Bind>, q: &Query) {
        if let Some(t) = q.note_type {
            binds.push(Bind::Text(note_type_str(t).to_owned()));
            sql.push_str(&format!(" AND n.type = ?{}", binds.len()));
        }
        if let Some(p) = &q.path_prefix {
            binds.push(Bind::Text(format!("{p}%")));
            sql.push_str(&format!(" AND n.path LIKE ?{}", binds.len()));
        }
        if let Some(needle) = q
            .name_like
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            // 标题**或**路径命中即可——列表行上看得见的就是这两样（大小写不敏感由 SQLite
            // 的 LIKE 给，与旧前端 `toLowerCase()` 一样只覆盖 ASCII）。
            binds.push(Bind::Text(escape_like(needle)));
            let i = binds.len();
            sql.push_str(&format!(
                " AND (n.title LIKE '%'||?{i}||'%' ESCAPE '\\' \
                 OR n.path LIKE '%'||?{i}||'%' ESCAPE '\\')"
            ));
        }
        if q.ingest_candidates_only {
            sql.push_str(&format!(" AND {INGEST_CANDIDATE_SQL}"));
        }
        if q.knowledge_only {
            // 成熟度轴（ADR-086）：列非 NULL 即记录态（写入侧已保证资料类型恒 NULL），
            // 故这一句就是「排除记录态」的全部——不在这里重判缺省，也不看 `type`。
            sql.push_str(" AND n.stage IS NULL");
        }
        if let Some(tag) = &q.tag {
            // 嵌套标签语义（Obsidian 对标，PRD-06-033）：按 `a` 过滤同时命中
            // `a` 本身与 `a/b`、`a/b/c` 等子孙标签。
            binds.push(Bind::Text(tag.clone()));
            let eq = binds.len();
            binds.push(Bind::Text(format!("{}/%", escape_like(tag))));
            let like = binds.len();
            // 过滤与 `list_tags` 的分组**同一口径**（`COLLATE NOCASE`）：分组把
            // `Data_Security` 与 `data_security` 并成一条，过滤却大小写敏感的话，点开那一条
            // 只能看到其中一半的笔记——数字与内容当场对不上。LIKE 在 SQLite 里对 ASCII
            // 本就大小写不敏感，这里补的是等值那一半。
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM tags t WHERE t.note_path = n.path \
                 AND (t.tag = ?{eq} COLLATE NOCASE OR t.tag LIKE ?{like} ESCAPE '\\'))"
            ));
        }
    }

    fn run_hits(&self, sql: &str, binds: &[Bind]) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |row| {
                Ok(SearchHit {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    score: row.get(2)?,
                    snippet: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    modified_ms: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    // media.duration 是秒（REAL）；毫秒化在 SQL 里做（`DURATION_MS_SQL`），
                    // 排序、游标与下发值因此是同一个整数。
                    duration_ms: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    created: row.get::<_, Option<String>>(6)?,
                    transcribed: row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
                    created_ms: row.get::<_, Option<i64>>(8)?,
                    cursor: None,
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 搜索视图的**候选集**（PRD-06-031）：结构化过滤 + 内容粗筛，按修改时间倒序。
    ///
    /// 返回的是**超集**——精确的「每个词都在 / 排除词不在 / 命中在第几行」由读文件那一遍
    /// 决定（[`crate::search`] 模块文档口径 3）。粗筛只取一个最长正向词：正向词是 AND
    /// 关系，任取其一都不会漏掉真命中，而多词 MATCH 表达式在 trigram 分词器下反而容易
    /// 因短词失配而**漏掉**结果。
    pub fn search_candidates(
        &self,
        query: &crate::search::SearchQuery,
        limit: usize,
    ) -> Result<Vec<NoteListEntry>> {
        let mut binds: Vec<String> = Vec::new();
        let mut sql = match query.narrowing_term() {
            Some(term) if term.chars().count() >= 3 => {
                binds.push(format!("\"{}\"", term.replace('"', "")));
                String::from(
                    "SELECT n.path, n.title, n.modified, n.type, n.stage FROM fts \
                     JOIN notes n ON n.path = fts.note_path WHERE fts MATCH ?1",
                )
            }
            Some(term) => {
                // trigram 对 <3 字符失效，退 LIKE 子串扫描（与 `search_like` 同一兜底口径）。
                binds.push(escape_like(term));
                String::from(
                    "SELECT n.path, n.title, n.modified, n.type, n.stage FROM fts \
                     JOIN notes n ON n.path = fts.note_path \
                     WHERE fts.content LIKE '%'||?1||'%' ESCAPE '\\'",
                )
            }
            None => String::from(
                "SELECT n.path, n.title, n.modified, n.type, n.stage FROM notes n WHERE 1=1",
            ),
        };

        for filter in &query.filters {
            append_search_filter(&mut sql, &mut binds, filter);
        }
        sql.push_str(" ORDER BY n.modified DESC LIMIT ?");
        sql.push_str(&(binds.len() + 1).to_string());
        binds.push(limit.to_string());

        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |row| {
                Ok(NoteListEntry {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    modified_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    note_type: row.get(3)?,
                    draft: row_draft(row.get(4)?),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 全部**记录态**笔记的路径（ADR-086）。文件树的淡显 + 徽标（PRD-06-052）按它渲染
    /// ——与主题页/知识地图标识同一形态：判定在 Rust，树只搬运结论。
    /// 全部笔记的 `path → luna.type`（文件树一次查齐，PRD-06-048①/⑥）：树行的类型徽标与
    /// 「是不是来源层」都从这一张表派生，前端不自带类型清单。
    pub fn note_types(&self) -> Result<std::collections::HashMap<String, NoteType>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, type FROM notes")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_sqlite)?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (path, key) = row.map_err(map_sqlite)?;
            if let Some(t) = NoteType::from_key(&key) {
                out.insert(path, t);
            }
        }
        Ok(out)
    }

    pub fn draft_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM notes WHERE stage IS NOT NULL ORDER BY path")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 速记流（PRD-06-058 A / PRD-07-066）：全部 `type: quick-note`，按修改时间倒序，带单行预览。
    ///
    /// 收录口径是 **type 驱动**：速记是「先捕获、后整理」的捕获流，收的是文字速记与语音
    /// 速记的产物；被「标记为待整理」的普通笔记**留在「笔记」空间**，以淡显与徽标表达
    /// （[`Self::draft_paths`] 那条 stage 驱动的口径服务于文件树，不服务于这里）。速记转正
    /// 即换类型（PRD-06-053），于是它自然离开本流，不需要第二个判据。
    ///
    /// 预览取 FTS 那份**派生内容**（已剥 frontmatter 与时间码）而不是回头读文件：一行
    /// 预览值不上 N 次磁盘 IO，而这个列表的语义是"全部速记"，条数没有上界。
    /// 语义 = 全库速记**全部可被游标穷尽遍历**（TDD-07 §4.1a），排序键恒为修改时间
    /// （速记流按时间桶分组，别的序在这里说不通）。`limit == 0` = 取 [`LIST_PAGE_SIZE`] 一页。
    ///
    /// 返回 `(这一页, 下一页游标)`——游标挂信封而不是逐行，因为这条命令的返回本来就是
    /// 一个结构体（`DraftInboxDto` 已经在信封上带着权威总数与汇总限额）。
    pub fn draft_inbox(
        &self,
        query: &DraftInboxQuery<'_>,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<DraftInboxEntry>, Option<String>)> {
        let limit = if limit == 0 { LIST_PAGE_SIZE } else { limit } as i64;
        let mut sql = format!(
            "SELECT n.path, n.title, n.type, n.modified, f.content, \
                    n.is_topic, n.is_knowledge_map, n.refined_at, \
                    EXISTS(SELECT 1 FROM media m WHERE m.note_path = n.path), \
                    {DURATION_MS_SQL} \
             {DRAFT_BASE_SQL}"
        );
        let mut binds: Vec<Bind> = vec![Bind::Text(note_type_str(NoteType::QuickNote).to_owned())];
        sql.push_str(&draft_filter_sql(query, &mut binds));
        let stale_before = query.now_ms - crate::SETTLE_BACKLOG_STALE_MS;
        let page = ListPage {
            sort: ListSort::Modified,
            cursor: cursor.map(str::to_owned),
        };
        append_keyset(&mut sql, &mut binds, &page, limit + 1)?;
        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |row| {
                let note_type: String = row.get(2)?;
                let is_aggregate_page: bool =
                    row.get::<_, i64>(5)? != 0 || row.get::<_, i64>(6)? != 0;
                Ok(DraftInboxEntry {
                    // 汇总资格走共享谓词（2026-09-02 起只挡聚合页，PRD-06-047 修订）：
                    // 收件箱里唯一可能被挡的就是聚合页那一类，成熟度不再参与判定。
                    mergeable: crate::merge_block(is_aggregate_page).is_none(),
                    path: row.get(0)?,
                    title: row.get(1)?,
                    quick_note: note_type == note_type_str(NoteType::QuickNote),
                    refined: row.get::<_, Option<String>>(7)?.is_some(),
                    has_media: row.get::<_, i64>(8)? != 0,
                    duration_ms: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    // 与「搁置太久」筛选**同一个门槛**：筛出来的行必然都带警示色，
                    // 反过来带警示色的行也必然筛得到。两处各算一遍就会有一头对不上。
                    stale: row.get::<_, Option<i64>>(3)?.unwrap_or(0) <= stale_before,
                    modified_ms: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    todos: crate::parse::todo_stats(
                        row.get::<_, Option<String>>(4)?
                            .unwrap_or_default()
                            .as_str(),
                    ),
                    preview: one_line_preview(
                        row.get::<_, Option<String>>(4)?
                            .unwrap_or_default()
                            .as_str(),
                    ),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        let next = take_page(&mut out, limit, ListSort::Modified, |row| {
            (row.modified_ms.to_string(), row.path.clone())
        });
        Ok((out, next))
    }

    /// 速记的**总数**（「速记」活动栏图标的计数徽标，PRD-07-066），口径与 [`Self::draft_inbox`] 同为
    /// `type: quick-note`。
    ///
    /// 与列表分开一条 `COUNT(*)` 而不是数返回的行数：列表带上限，而徽标上的数字是承诺给
    /// 用户的「还剩几篇」——被上限截断的徽标就是在谎报。
    pub fn draft_count(&self, query: &DraftInboxQuery<'_>) -> Result<u32> {
        let mut sql = format!("SELECT COUNT(*) {DRAFT_BASE_SQL}");
        let mut binds: Vec<Bind> = vec![Bind::Text(note_type_str(NoteType::QuickNote).to_owned())];
        sql.push_str(&draft_filter_sql(query, &mut binds));
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(binds.iter()), |r| r.get(0))
            .map_err(map_sqlite)?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// 按 `luna.type` 分组的笔记计数（PRD-07-024a ①「规模」）。
    ///
    /// 只回**原始分组**，不在这里合成「知识 vs 资料」两个数——那条线的判据是
    /// [`crate::NoteType::is_source_material`]，它已经存在且被四五个面共用；在 SQL 里
    /// 再写一次 `type IN (…)` 就是把同一条线画到第二个地方，而它必然会漏掉将来新增的
    /// 类型。分组交给调用方按那个判据折叠。
    pub fn note_counts_by_type(&self) -> Result<Vec<(String, u32)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT type, COUNT(*) FROM notes GROUP BY type")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
                ))
            })
            .map_err(map_sqlite)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    /// 近期活跃标签（PRD-07-024a ①「最近在写什么」）：`since_ms` 之后被改过的笔记里，
    /// 哪些标签出现得最多。
    ///
    /// **口径与标签浏览器一致**：只统计**知识笔记**（`type='note'` 且非主题页非知识
    /// 地图）——资料区本就不自动打标（ADR-024），聚合页则会把自己算进去。这与
    /// `knowledge_metrics_facts` 里那条标签簇 join 是同一个 WHERE，刻意保持逐字一致。
    ///
    /// **大小写不敏感聚合**：`Data_Security` 与 `data_security` 是同一个概念
    /// （PRD-06-035 修订），裂开会让「最近在写什么」把同一件事说两遍。
    pub fn active_tags(&self, since_ms: i64, limit: u32) -> Result<Vec<(String, u32)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT MAX(t.tag),COUNT(DISTINCT t.note_path) AS n
                 FROM tags t JOIN notes n ON n.path=t.note_path
                 WHERE n.type='note' AND n.is_topic=0 AND n.is_knowledge_map=0
                   AND n.modified >= ?1
                 GROUP BY t.tag COLLATE NOCASE
                 ORDER BY n DESC, MAX(t.tag) ASC
                 LIMIT ?2",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(rusqlite::params![since_ms, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
                ))
            })
            .map_err(map_sqlite)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    /// 一条**值得回看的旧笔记**（PRD-07-024a ①）：最久没被动过的那一篇知识笔记。
    ///
    /// **必须真的旧**（`older_than_ms` 由调用方给门槛）：库里最久没动的那一篇在一个
    /// 刚建的库里可能是昨天写的——把它端出来说「值得回看」是无中生有。现状块的纪律是
    /// 没东西说就安静，而不是凑一条。
    ///
    /// 排除三类：**空页**（没内容可回看）、**主题页与知识地图**（Lunarca 维护的聚合页，
    /// 不是你当初的思考）、**记录态**（那是待整理积压，已经有别的地方在说它，
    /// 见 PRD-06-055）。资料区本就不在 `type='note'` 里。
    pub fn revisit_candidate(&self, older_than_ms: i64) -> Result<Option<(String, String)>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT n.path, n.title FROM notes n
                     WHERE {INGEST_CANDIDATE_SQL}
                       AND n.modified > 0 AND n.modified <= ?1
                     ORDER BY n.modified ASC LIMIT 1"
                ),
                rusqlite::params![older_than_ms],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite)
    }

    /// 全部**知识产物**的路径（主题页 + 知识地图，ADR-096）。
    ///
    /// 供前端做路由闸口：产物在应用内归知识库空间，编辑桌与「笔记」投影据此让路。
    /// 判定只认 `is_topic` / `is_knowledge_map` 两列——与树过滤、目录查询同一份身份语义。
    pub fn knowledge_product_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path FROM notes WHERE is_topic = 1 OR is_knowledge_map = 1 ORDER BY path",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 按类型列出笔记（modified 倒序）：功能视图（§5）消费，如对话历史会话列表（PRD-03-013）。
    ///
    /// 键集游标同 [`Self::draft_inbox`]（排序键恒为修改时间，返回 `(这一页, 下一页游标)`）。
    /// `limit == 0` = 一页。
    pub fn list_by_type(
        &self,
        note_type: NoteType,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<NoteListEntry>, Option<String>)> {
        let limit = if limit == 0 { LIST_PAGE_SIZE } else { limit } as i64;
        let mut sql = String::from(
            "SELECT n.path, n.title, n.modified, n.type, n.stage FROM notes n WHERE n.type = ?1",
        );
        let mut binds: Vec<Bind> = vec![Bind::Text(note_type_str(note_type).to_owned())];
        let page = ListPage {
            sort: ListSort::Modified,
            cursor: cursor.map(str::to_owned),
        };
        append_keyset(&mut sql, &mut binds, &page, limit + 1)?;
        let mut stmt = self.conn.prepare(&sql).map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |row| {
                Ok(NoteListEntry {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    modified_ms: row.get(2)?,
                    note_type: row.get(3)?,
                    draft: row_draft(row.get(4)?),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        let next = take_page(&mut out, limit, ListSort::Modified, |row| {
            (row.modified_ms.to_string(), row.path.clone())
        });
        Ok((out, next))
    }

    /// 某一类型的笔记**总数**（对话历史侧栏的计数徽标，PRD-03-013）。
    ///
    /// 与 [`Self::draft_count`] 同一条理由：列表带上限，而徽标上的数字是承诺给用户的
    /// 「一共有几场」——被上限截断的徽标就是在谎报。
    ///
    /// 刻意不去调 [`Self::note_counts_by_type`]：那一条答的是"每一类各有多少"（知识库
    /// 规模面一次要全部），为拿一个数把全库分组一遍是把它的代价借过来用。
    pub fn count_by_type(&self, note_type: NoteType) -> Result<u32> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE type = ?1",
                [note_type_str(note_type)],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// 已有主题页的**出身标签**清单（`luna.topic_src`，PRD-16-016）。
    ///
    /// 只回答「这个标签已经有主题页了吗」这一个问题，故不建索引列——主题页数量级远小于
    /// 笔记总数，扫一遍 frontmatter 不值得为它加一列。走 `notes` 表的 path 再读文件会更慢，
    /// 故复用既有 `fts` 之外的最省路径：直接查 `notes` 里被标记的那些。
    pub fn topic_sources(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT topic_src FROM notes WHERE topic_src IS NOT NULL")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 库里全部主题页的路径（`luna.topic: true`，按路径稳定排序）。
    ///
    /// 「库里到底有没有主题页」是个**事实**，必须能一句 SQL 答出来：归入的空态要靠它区分
    /// 「你还没有主题页」和「有 N 页，但没有一页在讲同一件事」——这两句话对用户来说是
    /// 完全不同的下一步（去合成 vs 去补标签），拿别的数字硬凑就是撒谎。
    pub fn topic_pages(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM notes WHERE is_topic = 1 ORDER BY path")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 某时刻以来收进来的来源（PRD-16-067「近 7 天」）：笔记、速记、录音与转写，按「录于何时」
    /// （`luna.created`）计，没记录时间的不算——修改时间会把改个标签的老笔记也算成新来的。
    /// 主题页、知识地图与对话不是来源。`cited` = 已被至少一页主题页链接（证据格式页的出处
    /// 就是声明行里的链接，旧格式页同理），按录于时间新→旧。
    pub fn sources_created_since(&self, since_ms: i64) -> Result<Vec<RecentSource>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT n.path, n.type,
                        EXISTS (SELECT 1 FROM links l JOIN notes t ON t.path = l.src_path
                                 WHERE t.is_topic = 1 AND l.dst_path = n.path)
                   FROM notes n
                  WHERE n.created_ms >= ?1 AND n.is_topic = 0 AND n.is_knowledge_map = 0
                    AND n.type IN ('note', 'quick-note', 'recording', 'transcript')
                  ORDER BY n.created_ms DESC, n.path ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![since_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            let (path, kind, cited) = row.map_err(map_sqlite)?;
            if let Some(note_type) = NoteType::from_key(&kind) {
                out.push(RecentSource {
                    path,
                    note_type,
                    cited,
                });
            }
        }
        Ok(out)
    }

    /// 库里全部知识地图（正常为 0/1；>1 是冲突，调用方必须 fail closed）。
    pub fn knowledge_maps(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM notes WHERE is_knowledge_map = 1 ORDER BY path")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 与某篇笔记**共享标签**的其它笔记，按「出身标签命中 → 共享标签数 → 路径」排序。
    ///
    /// 这是「这两篇在讲同一件事」的**结构信号**，与字面/语义相似度**互不替代**：
    /// 一篇繁体笔记与一页简体主题页字面重叠接近零、嵌入也可能不到位，但它们都挂着
    /// `#趋势分析` —— 而这正是那一页当初被建议出来的判据（PRD-16-016：同标签 ≥3 篇）。
    /// 靠相似度去"顺便捞"主题页，在库里只有一两页主题时会**结构性地永远捞不到**
    /// （摘要页天生最短、最不含原文字面词，抢不过原文本身）。
    ///
    /// `birth` 单独成列而不并进 `shared`：主题页的 `topic_src` 是它的**定义**，
    /// 一篇挂着该标签的笔记就该优先落到它头上，这比"多共享一个标签"强得多。
    ///
    /// 两侧都把 `topic_src` 当作自己的一个标签，关系才**对称**：刚合成的主题页
    /// `tags` 表里一行都没有（AI 标签要等下一次扫描），只看 `tags` 的话「从页出发找同伴」
    /// 会当场空手而归——而"刚合成第一页"正是最需要它工作的时刻。
    pub fn tag_neighbors(&self, rel_path: &str) -> Result<Vec<TagNeighbor>> {
        let mut stmt = self
            .conn
            .prepare(
                "WITH src(tag) AS ( \
                   SELECT DISTINCT lower(tag) FROM tags WHERE note_path = ?1 \
                   UNION \
                   SELECT lower(topic_src) FROM notes \
                    WHERE path = ?1 AND topic_src IS NOT NULL \
                 ) \
                 SELECT path, is_topic, birth, shared FROM ( \
                   SELECT n.path AS path, \
                          n.is_topic AS is_topic, \
                          CASE WHEN n.topic_src IS NOT NULL \
                                     AND lower(n.topic_src) IN (SELECT tag FROM src) \
                               THEN 1 ELSE 0 END AS birth, \
                          (SELECT COUNT(DISTINCT lower(t.tag)) FROM tags t \
                            WHERE t.note_path = n.path \
                              AND lower(t.tag) IN (SELECT tag FROM src)) AS shared \
                   FROM notes n WHERE n.path <> ?1 \
                 ) WHERE birth = 1 OR shared > 0 \
                 ORDER BY birth DESC, shared DESC, path",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![rel_path], |row| {
                Ok(TagNeighbor {
                    path: row.get(0)?,
                    is_topic: row.get::<_, i64>(1)? != 0,
                    birth: row.get::<_, i64>(2)? != 0,
                    shared: u32::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 以某条标签为出身的主题页（`ORDER BY path` 取第一页，稳定）；`None` = 还没有。
    ///
    /// 大小写不敏感：与 [`Self::topic_sources`] 的消费方（主题页建议的「已有页」集合）
    /// 同一口径——那里比的是 `to_lowercase()`，这里若大小写敏感，同一条标签会一边说
    /// "已经有页了"、一边找不到那一页。
    pub fn topic_page_for_tag(&self, tag: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT path FROM notes \
                  WHERE is_topic = 1 AND topic_src IS NOT NULL \
                    AND lower(topic_src) = lower(?1) \
                  ORDER BY path LIMIT 1",
                params![tag],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
    }

    /// 与某页主题页共享标签的**知识笔记**（`ORDER BY path`，稳定序）。
    ///
    /// 「知识笔记」= 非资料（`type = note`）且非记录态（`stage IS NULL`，ADR-086）且既不是
    /// 主题页也不是知识地图——与 PRD-16-016 那条「≥3 篇知识笔记」是同一口径，聚合页不
    /// 互相喂养（否则主题页会吞自己）。共享标签那一路的定义与 [`Self::tag_neighbors`]
    /// 逐字相同（`tags` ∪ `topic_src`，两侧都把出身标签当作自己的一个标签）。
    ///
    /// **它是一条 SQL，不是一条新执行栈**（TDD-07 §4.1d）：主题页目录要在同一次查询里
    /// 现算「待归入数」，而"这一页还能收哪几篇"这个判据只允许有一个定义——故「立即归入」
    /// 的共享标签那一段与目录计数**调的是同一个函数**（见 [`crate::Vault::topic_ingest_pending`]）。
    /// 各写一遍的表现是目录上写着 2、点下去排了 3 篇，而两处都"看起来对"。
    pub fn topic_tag_candidates(&self, topic_rel: &str) -> Result<Vec<String>> {
        self.topic_tag_matches(topic_rel, INGEST_CANDIDATE_SQL)
    }

    /// 与某页主题页共享标签的**录音/转写**（`ORDER BY path`）。共享标签的判据与
    /// [`Self::topic_tag_candidates`] 是同一段 SQL，只换了对象；是否已完成权威转写、本设备是否
    /// 授权由调用方读文件判定（PRD-16-047），这里不预设它们能进候选。
    pub fn topic_tag_source_material(&self, topic_rel: &str) -> Result<Vec<String>> {
        self.topic_tag_matches(topic_rel, "n.type IN ('recording', 'transcript')")
    }

    fn topic_tag_matches(&self, topic_rel: &str, filter_sql: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "WITH src(tag) AS ( \
                   SELECT DISTINCT lower(tag) FROM tags WHERE note_path = ?1 \
                   UNION \
                   SELECT lower(topic_src) FROM notes \
                    WHERE path = ?1 AND topic_src IS NOT NULL \
                 ) \
                 SELECT n.path FROM notes n \
                  WHERE n.path <> ?1 \
                    AND {filter_sql} \
                    AND ( \
                      (n.topic_src IS NOT NULL AND lower(n.topic_src) IN (SELECT tag FROM src)) \
                      OR EXISTS (SELECT 1 FROM tags t WHERE t.note_path = n.path \
                                  AND lower(t.tag) IN (SELECT tag FROM src)) \
                    ) \
                  ORDER BY n.path"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![topic_rel], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 按**声明的范围**取知识笔记：证据格式主题页的候选只看 `luna.knowledge.scope`
    /// 的包含/排除标签，不看这一页自己身上的标签——系统自动打的 `ai_tags`（如「功能/网页端」）
    /// 挂在主题页上时，按页面标签取会把别的项目里同标签的笔记拉进来（ADR-134 范围先于总结）。
    /// 标签比较与 [`Self::topic_tag_candidates`] 同口径（不分大小写、逐字相等）。
    pub fn scope_tag_candidates(
        &self,
        topic_rel: &str,
        scope: &crate::TopicScope,
    ) -> Result<Vec<String>> {
        self.scope_matches(topic_rel, scope, INGEST_CANDIDATE_SQL)
    }

    /// [`Self::scope_tag_candidates`] 的录音/转写版本（资格与授权由调用方读文件判定）。
    pub fn scope_tag_source_material(
        &self,
        topic_rel: &str,
        scope: &crate::TopicScope,
    ) -> Result<Vec<String>> {
        self.scope_matches(topic_rel, scope, "n.type IN ('recording', 'transcript')")
    }

    /// [`Self::scope_tag_candidates`] 的速记版本（ADR-136 ①）：速记按范围标签直接作证据，
    /// 空正文与排除由调用方读文件判定。
    pub fn scope_tag_quick_notes(
        &self,
        topic_rel: &str,
        scope: &crate::TopicScope,
    ) -> Result<Vec<String>> {
        self.scope_matches(topic_rel, scope, "n.type = 'quick-note'")
    }

    /// 范围命中 = 带任一包含标签（或出身标签），**或正文写到了本主题的名字**
    /// （[`crate::TopicScope::mention_names`]，全文索引短语匹配）；带任一排除标签即不进。
    /// 「写到名字」一路让没有 AI 标签的录音、转写与速记也能按内容到达主题（PRD-16-063）。
    fn scope_matches(
        &self,
        topic_rel: &str,
        scope: &crate::TopicScope,
        filter_sql: &str,
    ) -> Result<Vec<String>> {
        let (includes, excludes) = (&scope.includes, &scope.excludes);
        if includes.is_empty() {
            return Ok(Vec::new());
        }
        // trigram 短语匹配：名字恒为三字以上；引号去掉，免得打断 MATCH 表达式。
        let mentions = scope
            .mention_names()
            .iter()
            .map(|name| format!("\"{}\"", name.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let list = |tags: &[String]| {
            serde_json::to_string(
                &tags
                    .iter()
                    .map(|tag| tag.to_lowercase())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".to_owned())
        };
        // 没有可用的名字就不拼这一支：SQL 不保证 `AND` 短路，空的 MATCH 会直接报错。
        let mention_clause = if mentions.is_empty() {
            ""
        } else {
            "OR n.path IN (SELECT note_path FROM fts WHERE fts MATCH ?4)"
        };
        let mut binds = vec![topic_rel.to_owned(), list(includes), list(excludes)];
        if !mentions.is_empty() {
            binds.push(mentions);
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT n.path FROM notes n \
                  WHERE n.path <> ?1 \
                    AND {filter_sql} \
                    AND ( \
                      (n.topic_src IS NOT NULL \
                        AND lower(n.topic_src) IN (SELECT value FROM json_each(?2))) \
                      OR EXISTS (SELECT 1 FROM tags t WHERE t.note_path = n.path \
                                  AND lower(t.tag) IN (SELECT value FROM json_each(?2))) \
                      {mention_clause} \
                    ) \
                    AND NOT EXISTS (SELECT 1 FROM tags t WHERE t.note_path = n.path \
                                     AND lower(t.tag) IN (SELECT value FROM json_each(?3))) \
                  ORDER BY n.path"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 单篇笔记的身份行（`None` = 索引里没有它）。
    pub fn note_entry(&self, rel_path: &str) -> Result<Option<NoteListEntry>> {
        self.conn
            .query_row(
                "SELECT path, title, modified, type, stage FROM notes WHERE path = ?1",
                params![rel_path],
                |row| {
                    Ok(NoteListEntry {
                        path: row.get(0)?,
                        title: row.get(1)?,
                        modified_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                        note_type: row.get(3)?,
                        draft: row_draft(row.get(4)?),
                    })
                },
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(map_sqlite(other)),
            })
    }

    /// 某篇笔记的全部标签（用户标签与 AI 标签同池——`@` 定范围问的是「这篇算不算
    /// 那个主题的」，而不是「谁给它打的标」，PRD-16-009 / PRD-06-046）。
    pub fn note_tags(&self, rel_path: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT tag FROM tags WHERE note_path = ?1")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![rel_path], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 全部笔记的身份行，按路径排序（`luna note list`）。
    pub fn notes_by_path(&self) -> Result<Vec<NoteListEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, title, modified, type, stage FROM notes ORDER BY path ASC")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(NoteListEntry {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    modified_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    note_type: row.get(3)?,
                    draft: row_draft(row.get(4)?),
                })
            })
            .map_err(map_sqlite)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)
    }

    /// 全部笔记的身份行（路径 / 标题 / 类型），按标题长度降序。
    ///
    /// 供「未链接提及」扫描（PRD-16-013）：要在一篇正文里找出所有**其它笔记的标题**，
    /// 就得先有全部标题。长标题排前面是为了**最长优先匹配**——「项目 X 周报」与
    /// 「项目 X」同时存在时，短的先匹配会把长的那个吃掉一半。
    pub fn all_titles(&self, limit: usize) -> Result<Vec<NoteListEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, title, modified, type, stage FROM notes
                 ORDER BY LENGTH(title) DESC, path ASC LIMIT ?1",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(NoteListEntry {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    modified_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    note_type: row.get(3)?,
                    draft: row_draft(row.get(4)?),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 全部笔记的「(路径, 标题)」，路径升序——[`crate::graph::LinkIndex`] 的输入。
    pub fn note_identities(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, title FROM notes ORDER BY path ASC")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 全部笔记的「(路径, 别名)」，按路径、书写顺序——与 [`Self::note_identities`] 一起是
    /// [`crate::graph::LinkIndex`] 的输入（对标方案 C2）。
    pub fn note_aliases(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT note_path, alias FROM aliases ORDER BY note_path ASC, rowid ASC")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 与 `rel_path` 共享标签的笔记，按共享标签数降序。
    ///
    /// 「相关笔记」的批次 1 信号（PRD-16-013）：标签共现是库里现成的、零成本的相关性——
    /// 用户/AI 已经表达过「这几篇讲同一件事」，直接用它比再猜一遍强。
    pub fn tag_neighbours(&self, rel_path: &str, limit: usize) -> Result<Vec<NoteListEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT n.path, n.title, n.modified, n.type, n.stage, COUNT(*) AS shared
                 FROM tags t
                 JOIN tags mine ON mine.tag = t.tag AND mine.note_path = ?1
                 JOIN notes n ON n.path = t.note_path
                 WHERE t.note_path <> ?1
                 GROUP BY n.path ORDER BY shared DESC, n.modified DESC LIMIT ?2",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![rel_path, limit as i64], |row| {
                Ok(NoteListEntry {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    modified_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    note_type: row.get(3)?,
                    draft: row_draft(row.get(4)?),
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 反向链接：指向 `rel_path` 的笔记（按规范化目标匹配，§3）。MVP 直接按 dst 字符串。
    ///
    /// 路径与文件名各按原样与 NFC 两种形态比：macOS 上文件名可能以 NFD 落盘，而链接是输入法
    /// 敲的 NFC（ADR-148 ⑥）。两种形态都是索引查找，不扫全表。
    pub fn backlinks(&self, rel_path: &str) -> Result<Vec<String>> {
        let title = title_of(rel_path);
        let (path_nfc, title_nfc) = (crate::pathutil::nfc(rel_path), crate::pathutil::nfc(&title));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT src_path FROM links WHERE dst_path IN (?1, ?2, ?3, ?4)
                 OR lower(dst_path) IN (SELECT lower(alias) FROM aliases WHERE note_path = ?1)",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![rel_path, title, path_nfc.as_ref(), title_nfc.as_ref()],
                |r| r.get::<_, String>(0),
            )
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 链接目标的**末段名字**（去掉 `.md`、不分大小写）落在 `names` 里的源笔记。
    ///
    /// 改名重构的候选篇（TDD-06 §3 改名重构 ①）：`links.dst_path` 存的是按源目录归一化的
    /// 规范形，`[[目录/名字]]` 会存成「源目录/目录/名字」，按整串比对必然漏；但不论怎么写，
    /// 末段都是那个名字。宁可多给几篇（真正改不改由解析决定），不能少给。
    pub fn sources_linking_to_names(
        &self,
        names: &std::collections::HashSet<String>,
    ) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT src_path, dst_path FROM links")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(map_sqlite)?;
        let mut out = std::collections::BTreeSet::new();
        for row in rows {
            let (src, dst) = row.map_err(map_sqlite)?;
            if names.contains(&link_name_key(&dst)) {
                out.insert(src);
            }
        }
        Ok(out.into_iter().collect())
    }

    /// 这些笔记的 frontmatter 别名（不分大小写）。
    pub fn aliases_of(&self, paths: &[String]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT alias FROM aliases WHERE note_path = ?1")
            .map_err(map_sqlite)?;
        for path in paths {
            let rows = stmt
                .query_map(params![path], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?;
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
        }
        Ok(out)
    }

    /// 出链：`src_path` 引用的目标笔记路径列表（按 dst 升序）。
    pub fn outgoing_links(&self, src_path: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT dst_path FROM links WHERE src_path = ? ORDER BY dst_path ASC")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![src_path], |r| r.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 拉取关系图谱底表（PRD-06-034）：笔记（路径/标题/类型）+ 链接边 + 标签归属，
    /// 供 [`crate::graph::build_graph`] 纯函数解析构图（一次快照，避免多查询间不一致）。
    pub fn graph_rows(&self) -> Result<crate::graph::GraphRows> {
        let mut rows = crate::graph::GraphRows::default();
        let mut stmt = self
            .conn
            .prepare("SELECT path, title, type, stage FROM notes ORDER BY path ASC")
            .map_err(map_sqlite)?;
        let notes = stmt
            .query_map([], |r| {
                Ok(crate::graph::GraphNoteRow {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    note_type: r.get(2)?,
                    draft: row_draft(r.get(3)?),
                })
            })
            .map_err(map_sqlite)?;
        for row in notes {
            rows.notes.push(row.map_err(map_sqlite)?);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT src_path, dst_path FROM links ORDER BY src_path, dst_path")
            .map_err(map_sqlite)?;
        let links = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(map_sqlite)?;
        for row in links {
            rows.links.push(row.map_err(map_sqlite)?);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT note_path, tag FROM tags ORDER BY note_path, tag")
            .map_err(map_sqlite)?;
        let tags = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(map_sqlite)?;
        for row in tags {
            rows.tags.push(row.map_err(map_sqlite)?);
        }
        rows.aliases = self.note_aliases()?;
        Ok(rows)
    }

    /// 列出所有标签及其出现次数（按次数降序，同名升序）。
    /// 全部标签及其计数。
    ///
    /// **分组大小写不敏感**（`COLLATE NOCASE`）：真机第一轮的库里同时存在
    /// `Data_Security` 与 `data_security`——同一个概念被模型在两篇笔记里写成两种大小写，
    /// 于是本该 2 篇的簇裂成两个 1 篇，标签树重复呈现、主题页建议（≥3 篇同标签）永远
    /// 凑不够。而**别处早已是大小写不敏感**（`topic_suggestions` 的已有页判定、忽略清单
    /// 比对都用 `to_lowercase` / `eq_ignore_ascii_case`）——只有分组这一处漏了，是漂移不是设计。
    /// 展示取**出现最多的那种原形**（`MAX(tag)` 在同组里稳定选一个，避免每次刷新换个写法）。
    /// CJK 不受影响（中文没有大小写），故这条只在中英混排的标签上生效——而那正是真机现场。
    pub fn list_tags(&self) -> Result<Vec<TagCount>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT MAX(tag) AS tag, COUNT(*) AS c, \
                        SUM(CASE WHEN source = ?1 THEN 1 ELSE 0 END) AS user_hits \
                 FROM tags GROUP BY tag COLLATE NOCASE ORDER BY c DESC, tag ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![TagSource::User.key()], |row| {
                Ok(TagCount {
                    tag: row.get::<_, String>(0)?,
                    count: row.get::<_, i64>(1)? as u32,
                    ai_only: row.get::<_, i64>(2)? == 0,
                })
            })
            .map_err(map_sqlite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sqlite)?);
        }
        Ok(out)
    }

    /// 标签浏览器的层级摘要（PRD-06-033）：补齐中间层，整支按笔记路径去重计数。
    ///
    /// 这里直接读取派生 `tags` 表的一次快照，在 Rust 汇总所有路径前缀。前端不得再把
    /// 子标签的精确计数相加猜父级数字，否则一篇同时使用父/子标签的笔记会被重复计算。
    pub fn tag_summaries(&self) -> Result<Vec<TagSummary>> {
        #[derive(Default)]
        struct Accumulator {
            display: String,
            direct_notes: std::collections::BTreeSet<String>,
            branch_notes: std::collections::BTreeSet<String>,
            has_user_tag: bool,
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT note_path, tag, source FROM tags \
                 ORDER BY tag COLLATE NOCASE, tag, note_path",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(map_sqlite)?;

        let mut grouped = std::collections::BTreeMap::<String, Accumulator>::new();
        for row in rows {
            let (note_path, tag, source) = row.map_err(map_sqlite)?;
            let parts = tag.split('/').filter(|part| !part.is_empty());
            let mut prefix = String::new();
            let full_key = tag.to_ascii_lowercase();
            for part in parts {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                let key = prefix.to_ascii_lowercase();
                let entry = grouped.entry(key.clone()).or_default();
                // 与既有 `MAX(tag)` 展示选择保持同一稳定口径；大小写分组仍按 SQLite NOCASE。
                if entry.display.is_empty() || prefix > entry.display {
                    entry.display.clone_from(&prefix);
                }
                entry.branch_notes.insert(note_path.clone());
                if source == TagSource::User.key() {
                    entry.has_user_tag = true;
                }
                if key == full_key {
                    entry.direct_notes.insert(note_path.clone());
                }
            }
        }

        let mut summaries: Vec<TagSummary> = grouped
            .into_values()
            .map(|entry| TagSummary {
                tag: entry.display,
                direct_count: entry.direct_notes.len() as u32,
                note_count: entry.branch_notes.len() as u32,
                ai_only: !entry.has_user_tag,
            })
            .collect();
        summaries.sort_by(|a, b| {
            b.note_count
                .cmp(&a.note_count)
                .then_with(|| a.tag.cmp(&b.tag))
        });
        Ok(summaries)
    }

    /// 本地闭环度量的一次事实快照。三个结果必须来自同一索引句柄，避免「链接已经更新、
    /// 叶子清单还是上一拍」导致 24 小时样本被误判。
    pub fn knowledge_metrics_facts(&self) -> Result<KnowledgeMetricsFacts> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path FROM notes WHERE type='note' AND is_topic=0 AND is_knowledge_map=0
                 ORDER BY path",
            )
            .map_err(map_sqlite)?;
        let leaf_paths = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        let leaf_set = leaf_paths
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let topic_set = self
            .topic_pages()?
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let graph = crate::graph::build_graph(&self.graph_rows()?);
        let mut linked = std::collections::BTreeSet::new();
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.kind == crate::graph::GraphEdgeKind::Link)
        {
            if topic_set.contains(&edge.source) && leaf_set.contains(&edge.target) {
                linked.insert(edge.target.clone());
            }
            if topic_set.contains(&edge.target) && leaf_set.contains(&edge.source) {
                linked.insert(edge.source.clone());
            }
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT MAX(t.tag),COUNT(DISTINCT t.note_path)
                 FROM tags t JOIN notes n ON n.path=t.note_path
                 WHERE n.type='note' AND n.is_topic=0 AND n.is_knowledge_map=0
                 GROUP BY t.tag COLLATE NOCASE ORDER BY MAX(t.tag)",
            )
            .map_err(map_sqlite)?;
        let topic_clusters = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
                ))
            })
            .map_err(map_sqlite)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(KnowledgeMetricsFacts {
            leaf_paths,
            linked_leaf_paths: linked.into_iter().collect(),
            topic_clusters,
        })
    }
}

/// 把一条搜索过滤器（[`crate::search::Filter`]）落成 SQL 子句。
///
/// 取反一律写成 `NOT (…)` 包住肯定式，而不是各写一版否定 SQL——`tag:` 的嵌套语义
/// （命中 `a` 与 `a/b`）取反后是「既不是 a 也不是 a 的子孙」，分写两版必然漂移。
fn append_search_filter(sql: &mut String, binds: &mut Vec<String>, filter: &crate::search::Filter) {
    use crate::search::FilterField;
    let clause = match filter.field {
        FilterField::Type => {
            // 未知类型串不该退化成「无过滤」（那是静默扩大范围），故绑一个不存在的类型。
            let value = crate::frontmatter::NoteType::from_key(&filter.value)
                .map_or_else(|| String::from("\u{0}unknown"), |t| note_type_str(t).into());
            binds.push(value);
            format!("n.type = ?{}", binds.len())
        }
        FilterField::Path => {
            binds.push(escape_like(&filter.value));
            format!("n.path LIKE '%'||?{}||'%' ESCAPE '\\'", binds.len())
        }
        FilterField::File => {
            binds.push(escape_like(&filter.value));
            format!("n.title LIKE '%'||?{}||'%' ESCAPE '\\'", binds.len())
        }
        FilterField::Tag => {
            binds.push(filter.value.clone());
            let eq = binds.len();
            binds.push(format!("{}/%", escape_like(&filter.value)));
            let like = binds.len();
            format!(
                "EXISTS (SELECT 1 FROM tags t WHERE t.note_path = n.path \
                 AND (t.tag = ?{eq} OR t.tag LIKE ?{like} ESCAPE '\\'))"
            )
        }
    };
    if filter.negated {
        sql.push_str(&format!(" AND NOT ({clause})"));
    } else {
        sql.push_str(&format!(" AND {clause}"));
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 记录收件箱一行预览的上限（字符数）。够看清"这条在讲什么"，又不至于把列表撑成两栏。
const PREVIEW_CHARS: usize = 80;

/// 把一篇正文压成**一行预览**（[`Index::draft_inbox`]）。
///
/// 输入是 FTS 那份派生内容（frontmatter 与时间码已剥）。行内换行折成空格、连续空白收成
/// 一个——列表里一行就是一行，让 `\n` 原样进 DOM 只会得到一串莫名其妙的间隔。
/// **按字符而不是字节截断**：中文一个字三字节，按字节切会切出半个字。
fn one_line_preview(content: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in content.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if out.chars().count() >= PREVIEW_CHARS {
            return out;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// LIKE 路径的手工片段：截取命中词周围的上下文并高亮。
fn manual_snippet(content: &str, needle: &str) -> String {
    let Some(pos) = content.find(needle) else {
        return content.chars().take(40).collect();
    };
    let start = content[..pos]
        .char_indices()
        .rev()
        .nth(12)
        .map_or(0, |(i, _)| i);
    let end_rel = content[pos + needle.len()..]
        .char_indices()
        .nth(12)
        .map_or(content.len() - pos - needle.len(), |(i, _)| i);
    let end = pos + needle.len() + end_rel;
    format!(
        "{}\u{300a}{}\u{300b}{}",
        &content[start..pos],
        needle,
        &content[pos + needle.len()..end]
    )
}

fn map_sqlite(e: rusqlite::Error) -> VaultError {
    VaultError::Decode(format!("sqlite: {e}"))
}

/// 一条链接目标（或一个文件路径）的末段名字键：去 `.md`、百分号解码、小写。
#[must_use]
pub(crate) fn link_name_key(target: &str) -> String {
    let decoded = crate::pathutil::percent_decode(target.trim());
    let last = decoded.rsplit('/').next().unwrap_or(&decoded);
    let lower = crate::pathutil::name_key(last);
    lower
        .strip_suffix(".md")
        .map_or(lower.clone(), str::to_owned)
}

#[cfg(test)]
mod tests {
    /// 测试夹具：不带任何筛选的取数请求（`now_ms` 取此刻，陈化判定按真实时钟）。
    fn all_drafts() -> DraftInboxQuery<'static> {
        DraftInboxQuery {
            text: None,
            filter: DraftFilter::All,
            now_ms: chrono::Local::now().timestamp_millis(),
        }
    }

    use super::*;

    fn idx() -> Index {
        Index::in_memory().unwrap()
    }

    #[test]
    fn chinese_fts_trigram_hits() {
        // "中文搜不到" 回归：trigram 下纯中文 ≥3 字子串能命中。
        let i = idx();
        i.index_note(
            "日常/会议纪要.md",
            "---\nluna:\n  type: note\n---\n今天的产品评审会议讨论了路线图",
            0,
        )
        .unwrap();
        let hits = i.search(&Query::text("产品评审")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "日常/会议纪要.md");
        assert!(
            hits[0].snippet.contains("\u{300a}"),
            "应有高亮：{}",
            hits[0].snippet
        );
    }

    #[test]
    fn short_chinese_query_like_fallback() {
        // 2 字中文（trigram 失效）走 LIKE 兜底，仍命中。
        let i = idx();
        i.index_note("a.md", "本周的会议安排", 0).unwrap();
        let hits = i.search(&Query::text("会议")).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("会议"));
    }

    #[test]
    fn fts_hygiene_no_yaml_or_timecode_hits() {
        let i = idx();
        i.index_note(
            "rec.md",
            "---\nluna:\n  type: recording\n  created: 2026-05-28T10:00:00+08:00\n---\n[00:00:12.480] 大家好开始评审",
            0,
        )
        .unwrap();
        // 不该命中 frontmatter 键或 luna: 块内容。
        assert!(i.search(&Query::text("recording")).unwrap().is_empty());
        assert!(i.search(&Query::text("created")).unwrap().is_empty());
        assert!(
            i.search(&Query::text("luna")).unwrap().is_empty(),
            "luna: 键不应命中 FTS"
        );
        // 时间码字符串不应命中（[HH:MM:SS.mmm] 已在 clean_for_fts 剥除）。
        assert!(
            i.search(&Query::text("00:00:12")).unwrap().is_empty(),
            "时间码不应命中 FTS"
        );
        // 正文命中正常。
        assert_eq!(i.search(&Query::text("开始评审")).unwrap().len(), 1);
    }

    #[test]
    fn default_type_note_and_type_filter() {
        let i = idx();
        i.index_note("plain.md", "# 普通笔记\n无 luna 块", 0)
            .unwrap();
        i.index_note("rec.md", "---\nluna:\n  type: recording\n---\n录音笔记", 0)
            .unwrap();
        // 缺省 type=note。
        let notes = i
            .search(&Query {
                note_type: Some(NoteType::Note),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].path, "plain.md");
        // recording 视图只含 rec.md。
        let recs = i
            .search(&Query {
                note_type: Some(NoteType::Recording),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].path, "rec.md");
    }

    /// `notes.stage` 派生列（ADR-086 / TDD-06 §3）：写入口径 = `NoteStage::resolve` 的
    /// 结论，而不是「frontmatter 里有没有这个词」——速记无键也是记录态、资料有键也不是。
    #[test]
    fn stage_column_stores_the_resolved_conclusion_not_the_raw_key() {
        let i = idx();
        i.index_note("知识.md", "---\nluna:\n  type: note\n---\n正文", 0)
            .unwrap();
        i.index_note(
            "草稿.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n正文",
            0,
        )
        .unwrap();
        i.index_note("速记.md", "---\nluna:\n  type: quick-note\n---\n正文", 0)
            .unwrap();
        i.index_note(
            "转写.md",
            "---\nluna:\n  type: transcript\n  stage: draft\n---\n正文",
            0,
        )
        .unwrap();
        i.index_note(
            "未知.md",
            "---\nluna:\n  type: note\n  stage: wip\n---\n正文",
            0,
        )
        .unwrap();

        let mut drafts = i.draft_paths().unwrap();
        drafts.sort();
        assert_eq!(
            drafts,
            vec!["未知.md".to_string(), "草稿.md".into(), "速记.md".into()],
            "记录态 = 显式 draft + 缺省记录态的速记 + 未知取值；资料恒不在其中"
        );
        // 逐行事实也随 `NoteListEntry` 下发（搜索命中、问答候选共用这一条）。
        assert!(i.note_entry("草稿.md").unwrap().unwrap().draft);
        assert!(i.note_entry("速记.md").unwrap().unwrap().draft);
        assert!(!i.note_entry("知识.md").unwrap().unwrap().draft);
        assert!(
            !i.note_entry("转写.md").unwrap().unwrap().draft,
            "资料类型忽略该键（来源轴优先）"
        );
        // 图谱节点携带同一事实，构图口径不变。
        let rows = i.graph_rows().unwrap();
        assert!(rows.notes.iter().any(|n| n.path == "草稿.md" && n.draft));
        assert!(rows.notes.iter().any(|n| n.path == "转写.md" && !n.draft));
    }

    /// 列是**派生数据**：重索引（同一路径重新写盘）与整库重建都要把它算对。
    #[test]
    fn stage_column_is_rebuilt_from_the_file_every_time() {
        let i = idx();
        i.index_note(
            "a.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n正文",
            0,
        )
        .unwrap();
        assert_eq!(i.draft_paths().unwrap(), vec!["a.md".to_string()]);
        // 沉淀（去键）后重索引 → 列自动回到 NULL，无需任何迁移动作。
        i.index_note("a.md", "---\nluna:\n  type: note\n---\n正文", 1)
            .unwrap();
        assert!(i.draft_paths().unwrap().is_empty());
        // 反向：重新标成记录态。
        i.index_note(
            "a.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n正文",
            2,
        )
        .unwrap();
        assert_eq!(i.draft_paths().unwrap(), vec!["a.md".to_string()]);
        // 整库清空重建（`clear` 是「重建索引」路径的第一步）后同样正确。
        i.clear().unwrap();
        assert!(i.draft_paths().unwrap().is_empty());
        i.index_note(
            "a.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n正文",
            3,
        )
        .unwrap();
        assert_eq!(i.draft_paths().unwrap(), vec!["a.md".to_string()]);
    }

    /// `knowledge_only` 只服务主题页建议计数（PRD-16-016 修订）：搜索面**不**开它。
    #[test]
    fn knowledge_only_filter_drops_drafts() {
        let i = idx();
        i.index_note("知识.md", "---\nluna:\n  type: note\n---\n项目评审", 0)
            .unwrap();
        i.index_note(
            "草稿.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n项目评审",
            0,
        )
        .unwrap();
        let all = i.search(&Query::text("项目评审")).unwrap();
        assert_eq!(all.len(), 2, "缺省不过滤——搜索面记录态不降权不排除");
        let knowledge = i
            .search(&Query {
                text: Some("项目评审".into()),
                knowledge_only: true,
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            knowledge
                .iter()
                .map(|h| h.path.as_str())
                .collect::<Vec<_>>(),
            vec!["知识.md"]
        );
    }

    #[test]
    fn list_by_type_orders_by_modified_desc() {
        // PRD-03-013 对话历史会话列表：按类型列出 + 最近修改在前。
        let i = idx();
        i.index_note("chats/old.md", "---\nluna:\n  type: chat\n---\n旧会话", 100)
            .unwrap();
        i.index_note("chats/new.md", "---\nluna:\n  type: chat\n---\n新会话", 200)
            .unwrap();
        i.index_note("plain.md", "普通笔记", 300).unwrap();
        let (chats, _) = i.list_by_type(NoteType::Chat, 10, None).unwrap();
        assert_eq!(
            chats.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["chats/new.md", "chats/old.md"],
        );
        assert_eq!(chats[0].title, "new");
        assert_eq!(chats[0].modified_ms, 200);
    }

    #[test]
    fn tag_filter_and_backlinks() {
        let i = idx();
        i.index_note(
            "a.md",
            "---\ntags:\n  - 工作\n---\n见 [[b]] 与 [详情](c.md)",
            0,
        )
        .unwrap();
        i.index_note("b.md", "目标笔记", 0).unwrap();
        let tagged = i
            .search(&Query {
                tag: Some("工作".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(tagged.len(), 1);
        // wikilink [[b]] 建边 → b 的反链含 a。
        assert_eq!(i.backlinks("b.md").unwrap(), vec!["a.md"]);
        // md 链接 c.md 也建边。
        assert_eq!(i.backlinks("c.md").unwrap(), vec!["a.md"]);
    }

    #[test]
    fn nested_tag_filter_includes_children() {
        // PRD-06-033 嵌套标签（Obsidian 对标）：按父标签过滤命中自身与全部子孙标签，
        // 前缀相似但非层级子级（`工作台`）不误命中。
        let i = idx();
        i.index_note("p.md", "---\ntags: [工作]\n---\n父级", 0)
            .unwrap();
        i.index_note("c.md", "---\ntags: [工作/评审]\n---\n子级", 0)
            .unwrap();
        i.index_note("x.md", "---\ntags: [工作台]\n---\n伪前缀", 0)
            .unwrap();
        let by_parent = i
            .search(&Query {
                tag: Some("工作".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        let mut paths: Vec<_> = by_parent.iter().map(|h| h.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["c.md", "p.md"]);
        // 精确子标签仍只命中子级。
        let by_child = i
            .search(&Query {
                tag: Some("工作/评审".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_child.len(), 1);
        assert_eq!(by_child[0].path, "c.md");
    }

    #[test]
    fn reindex_is_idempotent() {
        let i = idx();
        i.index_note("a.md", "第一版内容评审", 0).unwrap();
        i.index_note("a.md", "第二版内容讨论", 0).unwrap();
        // 旧内容不再命中，新内容命中，无重复行。
        assert!(i.search(&Query::text("第一版内容")).unwrap().is_empty());
        assert_eq!(i.search(&Query::text("第二版内容")).unwrap().len(), 1);
    }

    /// 全文行按 `notes.fts_rowid` 删（v16）：重索引必须真删掉旧全文行、不留第二行（搜索里
    /// 同一篇出现两次），删笔记与删文件夹都不留孤儿全文行。
    #[test]
    fn fulltext_rows_follow_their_note_by_rowid() {
        let i = idx();
        let fts_rows = |i: &Index| -> i64 {
            i.conn
                .query_row("SELECT count(*) FROM fts", [], |r| r.get(0))
                .unwrap()
        };
        i.index_note("a.md", "旧的正文内容", 0).unwrap();
        i.index_note("a.md", "新的正文内容", 1).unwrap();
        assert!(i.search(&Query::text("旧的正文")).unwrap().is_empty());
        assert_eq!(i.search(&Query::text("新的正文")).unwrap().len(), 1);
        assert_eq!(fts_rows(&i), 1);
        i.remove_note("a.md").unwrap();
        assert_eq!(fts_rows(&i), 0);
        i.index_note("夹/b.md", "乙", 0).unwrap();
        i.index_note("夹/子/c.md", "丙", 0).unwrap();
        i.index_note("夹外.md", "丁", 0).unwrap();
        i.remove_directory("夹").unwrap();
        assert_eq!(fts_rows(&i), 1, "只剩夹外那篇");
    }

    /// 磁盘索引不用 WAL：开着、写着时都没有内存映射的 `-shm`——系统收回那个文件的底层 vnode 会让读它的
    /// 进程 SIGBUS 退出（2026-10-22 真机）。回滚日志选 TRUNCATE、刷盘选 FULL（见 `init`）；CLI 的只读打开
    /// 在应用开着并写入时照样能读（ADR-141 ④）。
    #[test]
    fn the_disk_index_uses_a_truncated_rollback_journal_and_never_maps_a_shm_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let i = Index::open(&path).unwrap();
        i.index_note("a.md", "正文 [[b]]", 0).unwrap();
        let mode: String = i
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "truncate");
        let sync: i64 = i
            .conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 2, "FULL");
        let reader = Index::open_read_only(&path).unwrap();
        i.index_note("b.md", "乙", 0).unwrap();
        let seen: i64 = reader
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seen, 2, "应用写完，CLI 读得到");
        assert!(!dir.path().join("index.sqlite-shm").exists());
        assert!(!dir.path().join("index.sqlite-wal").exists());
    }

    /// 以前落下的 WAL 索引：打开时就地切到回滚日志，`-shm` 与 `-wal` 随之消失，内容不丢。
    #[test]
    fn an_index_left_in_wal_mode_is_switched_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        {
            let i = Index::open(&path).unwrap();
            i.index_note("a.md", "正文", 0).unwrap();
            i.conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        }
        let i = Index::open(&path).unwrap();
        let mode: String = i
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "truncate");
        assert!(!dir.path().join("index.sqlite-shm").exists());
        let notes: i64 = i
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notes, 1);
    }

    /// 升版本必须能从**上一版的完整 schema** 重建出来（不是从空库）：手写 DROP 清单漏表时，
    /// 每次升版本都在建表上报「已存在」，库从此打不开。这里拿当前 schema 当「旧版」，只改
    /// 版本号再打开——漏删任何一张表即红。
    #[test]
    fn schema_bump_rebuilds_from_a_full_previous_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        {
            let i = Index::open(&path).unwrap();
            i.index_note("a.md", "---\ntags: [x]\n---\n正文 [[b]]", 0)
                .unwrap();
            i.sync_ai_tag_ignores(&["忽略".to_owned()]).unwrap();
            i.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
                .unwrap();
        }
        let i = Index::open(&path).unwrap();
        let version: i64 = i
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let notes: i64 = i
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notes, 0, "升版本 = 清空重建，由随后的扫描补回");
        i.index_note("a.md", "正文", 0).unwrap();
        assert_eq!(i.search(&Query::text("正文")).unwrap().len(), 1);
    }

    #[test]
    fn remove_note_clears_all() {
        let i = idx();
        i.index_note("a.md", "---\ntags:\n  - x\n---\n内容讨论 [[b]]", 0)
            .unwrap();
        i.remove_note("a.md").unwrap();
        assert!(i.search(&Query::text("内容讨论")).unwrap().is_empty());
        assert!(i.backlinks("b").unwrap().is_empty());
    }

    #[test]
    fn list_tags_returns_grouped_counts() {
        let i = idx();
        i.index_note("a.md", "---\ntags: [foo, bar]\n---\n正文", 0)
            .unwrap();
        i.index_note("b.md", "---\ntags: [foo]\n---\n正文", 0)
            .unwrap();
        i.index_note("c.md", "无tags正文", 0).unwrap();
        let tags = i.list_tags().unwrap();
        assert!(!tags.is_empty(), "should have tags");
        let foo = tags.iter().find(|t| t.tag == "foo").unwrap();
        assert_eq!(foo.count, 2);
        let bar = tags.iter().find(|t| t.tag == "bar").unwrap();
        assert_eq!(bar.count, 1);
        // No empty tags
        assert!(tags.iter().all(|t| !t.tag.is_empty()));
        // Should be ordered by count DESC, tag ASC
        assert!(tags.windows(2).all(|w| w[0].count >= w[1].count));
    }

    #[test]
    fn knowledge_metrics_facts_use_leaf_and_resolved_link_identity() {
        let i = idx();
        i.index_note("Notes/a.md", "---\ntags: [Topic]\n---\nA", 0)
            .unwrap();
        i.index_note("Notes/b.md", "---\ntags: [topic]\n---\nB", 0)
            .unwrap();
        i.index_note(
            "Topics/topic.md",
            "---\nluna:\n  topic: true\n  topic_src: Topic\n---\n- [A](../Notes/a.md)",
            0,
        )
        .unwrap();
        i.index_note(
            "Maps/map.md",
            "---\ntags: [Topic]\nluna:\n  knowledge_map: true\n---\nmap",
            0,
        )
        .unwrap();
        i.index_note(
            "Sources/raw.md",
            "---\ntags: [Topic]\nluna:\n  type: transcript\n---\nraw",
            0,
        )
        .unwrap();
        let facts = i.knowledge_metrics_facts().unwrap();
        assert_eq!(facts.leaf_paths, vec!["Notes/a.md", "Notes/b.md"]);
        assert_eq!(facts.linked_leaf_paths, vec!["Notes/a.md"]);
        assert_eq!(facts.topic_clusters, vec![("topic".into(), 2)]);
    }

    #[test]
    fn list_tags_marks_only_the_purely_ai_generated_ones() {
        // PRD-06-046：`ai_only` 的语义是「**没有任何一篇**是用户自己打的」——
        // 用户也在用的标签不因 AI 恰好也提到而被标成机器标签（否则「只看用户标签」会藏错东西）。
        let i = idx();
        i.index_note(
            "甲.md",
            "---\ntags: [共用]\nluna:\n  ai_tags: [共用, 纯机器]\n---\n正文",
            0,
        )
        .unwrap();
        i.index_note("乙.md", "---\nluna:\n  ai_tags: [纯机器]\n---\n正文", 0)
            .unwrap();

        let tags = i.list_tags().unwrap();
        let by = |name: &str| tags.iter().find(|t| t.tag == name).expect("标签在场");
        assert!(!by("共用").ai_only, "用户也打过 → 不是纯 AI 标签");
        assert!(by("纯机器").ai_only, "两篇都只来自 ai_tags → 纯 AI 标签");
        assert_eq!(by("纯机器").count, 2, "计数照常统计全部来源");
    }

    #[test]
    fn tag_summaries_deduplicate_notes_across_parent_and_child_tags() {
        let i = idx();
        i.index_note("甲.md", "---\ntags: [项目, 项目/luna]\n---\n正文", 0)
            .unwrap();
        i.index_note("乙.md", "---\ntags: [项目/luna]\n---\n正文", 0)
            .unwrap();

        let summaries = i.tag_summaries().unwrap();
        let by = |name: &str| {
            summaries
                .iter()
                .find(|summary| summary.tag == name)
                .expect("标签摘要在场")
        };
        assert_eq!(by("项目").direct_count, 1);
        assert_eq!(
            by("项目").note_count,
            2,
            "同篇父子标签不能把父级数字重复加一"
        );
        assert_eq!(by("项目/luna").direct_count, 2);
        assert_eq!(by("项目/luna").note_count, 2);
    }

    #[test]
    fn tag_summaries_create_intermediate_nodes_and_derive_branch_source() {
        let i = idx();
        i.index_note(
            "用户.md",
            "---\ntags: [研究/市场/信号]\nluna:\n  ai_tags: [研究/自动]\n---\n正文",
            0,
        )
        .unwrap();
        i.index_note(
            "机器.md",
            "---\nluna:\n  ai_tags: [纯机器/子级]\n---\n正文",
            0,
        )
        .unwrap();

        let summaries = i.tag_summaries().unwrap();
        let by = |name: &str| {
            summaries
                .iter()
                .find(|summary| summary.tag == name)
                .expect("标签摘要在场")
        };
        assert_eq!(by("研究").direct_count, 0, "缺失的中间层由后端补壳");
        assert_eq!(by("研究/市场").direct_count, 0);
        assert!(
            !by("研究").ai_only,
            "任一子孙含用户标签，整支即属于用户在用"
        );
        assert!(by("研究/自动").ai_only);
        assert!(by("纯机器").ai_only);
    }

    #[test]
    fn outgoing_links_returns_dst_for_src() {
        let i = idx();
        i.index_note("a.md", "[[b]] [text](c.md)", 0).unwrap();
        i.index_note("b.md", "正文", 0).unwrap();
        i.index_note("c.md", "正文", 0).unwrap();
        let out = i.outgoing_links("a.md").unwrap();
        assert_eq!(out.len(), 2, "a.md should link to b.md and c.md");
        // wikilink [[b]] stores "b" (stem), md link [text](c.md) stores "c.md" (full path)
        assert!(out.contains(&"b".to_string()));
        assert!(out.contains(&"c.md".to_string()));
        // b.md has no outgoing links
        assert!(i.outgoing_links("b.md").unwrap().is_empty());
    }

    /// `links.dst_path` 必须是**库内路径**，不是链接原样（schema v7）。
    ///
    /// 真机第五轮的现场：一篇 `Notes/` 里的蒸馏笔记引用它的转写源，Lunarca 自己写出来的链接
    /// 形态是 `../Transcripts/名字%20带空格.md`；外链面板把这一串直接交给「打开笔记」，
    /// 用户读到「路径越出 Vault 根」。反链同时也丢——`dst = 路径` 恒对不上。
    #[test]
    fn link_targets_are_stored_as_vault_paths_not_as_written() {
        let i = idx();
        let src = "Notes/知识笔记.md";
        i.index_note(
            src,
            "见 [源](../Transcripts/名字%20带空格.md) 与 [同目录](四月%20纪要.md) 与 [[裸 stem]]",
            0,
        )
        .unwrap();
        let out = i.outgoing_links(src).unwrap();
        assert!(
            out.contains(&"Transcripts/名字 带空格.md".to_owned()),
            "跨目录 + 转义的链接要落成库内路径，才点得开：{out:?}"
        );
        assert!(
            out.contains(&"Notes/四月 纪要.md".to_owned()),
            "同目录链接也要补上目录段：{out:?}"
        );
        assert!(
            out.contains(&"裸 stem".to_owned()),
            "wikilink 的裸 stem 必须原样留着（它靠 stem 匹配反链）：{out:?}"
        );

        // 反链：被引用的那一篇按**自己的库内路径**就能查到引用者。
        i.index_note("Transcripts/名字 带空格.md", "转写正文", 0)
            .unwrap();
        assert_eq!(
            i.backlinks("Transcripts/名字 带空格.md").unwrap(),
            vec![src.to_owned()],
            "规范形入库后，反链不必再靠标题兜底"
        );
    }

    // ---------------------------------------------------------------------
    // 列表分页（键集游标，TDD-07 §4.1a / PRD-07-007）
    // ---------------------------------------------------------------------

    /// 建一批只有标题与修改时间不同的笔记（`type: note`）。
    fn seed(i: &Index, rows: &[(&str, i64)]) {
        for (path, modified) in rows {
            i.index_note(path, "正文", *modified).unwrap();
        }
    }

    /// 一次分页请求。
    fn page_of(i: &Index, sort: ListSort, cursor: Option<&str>, limit: usize) -> Vec<SearchHit> {
        i.search(&Query {
            limit,
            page: Some(ListPage {
                sort,
                cursor: cursor.map(str::to_owned),
            }),
            ..Default::default()
        })
        .unwrap()
    }

    /// 从给定游标沿键集走到底，返回全部路径（外加一个「页数不会失控」的保险）。
    fn walk_from(
        i: &Index,
        sort: ListSort,
        page_size: usize,
        start: Option<String>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = start;
        for _ in 0..1000 {
            let page = page_of(i, sort, cursor.as_deref(), page_size);
            let next = page.last().and_then(|hit| hit.cursor.clone());
            out.extend(page.into_iter().map(|hit| hit.path));
            match next {
                Some(c) => cursor = Some(c),
                None => return out,
            }
        }
        panic!("游标没有走到头——这本身就是分页坏了");
    }

    fn walk_all(i: &Index, sort: ListSort, page_size: usize) -> Vec<String> {
        walk_from(i, sort, page_size, None)
    }

    /// 跨页穷尽遍历：无重复、无遗漏，且与一次性全量查询**逐条同序**。
    #[test]
    fn keyset_pages_traverse_every_row_exactly_once() {
        let i = idx();
        seed(
            &i,
            &[
                ("a.md", 50),
                ("b.md", 40),
                ("c.md", 30),
                ("d.md", 20),
                ("e.md", 10),
                ("f.md", 5),
                ("g.md", 1),
            ],
        );
        let one_shot: Vec<String> = i
            .search(&Query {
                limit: 1000,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.path)
            .collect();
        assert_eq!(one_shot.len(), 7);

        for page_size in [1, 2, 3, 7, 100] {
            assert_eq!(
                walk_all(&i, ListSort::Modified, page_size),
                one_shot,
                "页大小 {page_size} 下的遍历结果必须与一次性全量查询逐条相同"
            );
        }
    }

    /// **排序值相同**时次序键仍给出全序：只比排序值的游标会在这里跨页重复或漏行。
    #[test]
    fn rows_sharing_a_sort_value_are_still_totally_ordered_by_path() {
        let i = idx();
        seed(&i, &[("a.md", 7), ("b.md", 7), ("c.md", 7), ("d.md", 7)]);
        assert_eq!(
            walk_all(&i, ListSort::Modified, 2),
            vec!["a.md", "b.md", "c.md", "d.md"],
        );
    }

    /// 遍历途中写入 / 删除：键集游标仍不重复不漏。**这正是禁 OFFSET 的理由**——
    /// 用 `OFFSET 2` 取第二页，新插进来的那一条会把 `c.md` 顶出窗口，用户永远看不到它。
    #[test]
    fn concurrent_writes_mid_traversal_neither_duplicate_nor_skip_rows() {
        let i = idx();
        seed(
            &i,
            &[
                ("a.md", 50),
                ("b.md", 40),
                ("c.md", 30),
                ("d.md", 20),
                ("e.md", 10),
            ],
        );
        let first = page_of(&i, ListSort::Modified, None, 2);
        assert_eq!(
            first.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a.md", "b.md"]
        );
        let cursor = first.last().unwrap().cursor.clone().unwrap();

        // 两页之间：插进一条最新的、删掉一条还没走到的。
        seed(&i, &[("新来的.md", 99)]);
        i.remove_note("d.md").unwrap();

        assert_eq!(
            walk_from(&i, ListSort::Modified, 2, Some(cursor)),
            vec!["c.md", "e.md"],
            "已走过的两条不该再出现，插入的更新行也不该被推进这一页"
        );
    }

    /// `name` = **UTF-8 码点序**，不是 `localeCompare` 的本地化序（契约已裁决接受）。
    ///
    /// 这条用例把差异**钉住**而不是留给将来某个人当 bug 报：同样五个中文标题，码点序是
    /// 张 < 李 < 王 < 阿 < 陈（U+5F20 / U+674E / U+738B / U+963F / U+9648），而 zh-CN 的
    /// `localeCompare` 给的是拼音序 阿 < 陈 < 李 < 王 < 张——**几乎正好相反**。本仓没有
    /// ICU collator，而确定性是分页正确性的前提（游标比较必须与 `ORDER BY` 同一套规则），
    /// 故这是一次显式取舍，不是回归。
    #[test]
    fn name_sort_is_utf8_codepoint_order_not_locale_order() {
        let i = idx();
        seed(
            &i,
            &[
                ("陈六.md", 1),
                ("阿一.md", 2),
                ("王五.md", 3),
                ("李四.md", 4),
                ("张三.md", 5),
            ],
        );
        let codepoint = vec!["张三.md", "李四.md", "王五.md", "阿一.md", "陈六.md"];
        let pinyin = vec!["阿一.md", "陈六.md", "李四.md", "王五.md", "张三.md"];
        assert_eq!(walk_all(&i, ListSort::Name, 2), codepoint);
        assert_ne!(
            walk_all(&i, ListSort::Name, 2),
            pinyin,
            "拼音序是旧前端 localeCompare 的结果；这里给的必须是码点序"
        );
    }

    /// 同一条差异的 ASCII 面：码点序**大小写敏感**（大写整体排在小写之前）。
    #[test]
    fn name_sort_puts_uppercase_before_lowercase() {
        let i = idx();
        seed(&i, &[("apple.md", 1), ("Zebra.md", 2)]);
        assert_eq!(
            walk_all(&i, ListSort::Name, 10),
            vec!["Zebra.md", "apple.md"],
            "`Z`(U+005A) < `a`(U+0061)；localeCompare 会给出相反的顺序"
        );
    }

    /// 时长排序：长的在前，跨页仍然全序（无媒体 = 0，排最后）。
    #[test]
    fn duration_sort_pages_longest_first() {
        let i = idx();
        for (path, seconds) in [("long.md", 90.0), ("mid.md", 30.0), ("short.md", 5.0)] {
            i.index_note(
                path,
                &format!(
                    "---\nluna:\n  type: transcript\n  media:\n    - path: {path}.wav\n      duration: {seconds}\n---\n正文"
                ),
                0,
            )
            .unwrap();
        }
        i.index_note(
            "silent.md",
            "---\nluna:\n  type: transcript\n---\n没有媒体",
            0,
        )
        .unwrap();
        assert_eq!(
            walk_all(&i, ListSort::Duration, 2),
            vec!["long.md", "mid.md", "short.md", "silent.md"],
        );
        let first = page_of(&i, ListSort::Duration, None, 1);
        assert_eq!(
            first[0].duration_ms, 90_000,
            "毫秒化在 SQL 里做，排序值与下发值是同一个数"
        );
    }

    /// 边界：空库 / 恰好一页 / 最后一页不足一页。
    #[test]
    fn created_sort_orders_by_recording_time_and_falls_back_to_modified() {
        // 「录于何时」是录音的身份时间（PRD-07-007）：改一次标题不该把它顶到最前；
        // 没记创建时间的老笔记退到修改时间，仍能排进同一条序列里。
        let i = idx();
        i.index_note(
            "old-recorded.md",
            "---\nluna:\n  type: recording\n  created: 2026-01-01T10:00:00+08:00\n---\n早录的，刚改过标题",
            9_000_000_000_000,
        )
        .unwrap();
        i.index_note(
            "new-recorded.md",
            "---\nluna:\n  type: recording\n  created: 2026-09-03T10:00:00-04:00\n---\n新录的",
            1,
        )
        .unwrap();
        i.index_note(
            "legacy.md",
            "---\nluna:\n  type: recording\n---\n没记创建时间",
            1_770_000_000_000, // 2026-02-02：落在两条创建时间之间
        )
        .unwrap();
        assert_eq!(
            walk_all(&i, ListSort::Created, 2),
            vec!["new-recorded.md", "legacy.md", "old-recorded.md"],
        );
        let first = page_of(&i, ListSort::Created, None, 1);
        assert_eq!(
            first[0].created_ms,
            created_ms_of(Some("2026-09-03T10:00:00-04:00")),
            "下发的毫秒与排序用的是同一个解析结果"
        );
        assert!(first[0].created_ms.is_some());
        assert_eq!(created_ms_of(Some("not a date")), None);
    }

    #[test]
    fn page_boundaries_report_exhaustion_honestly() {
        let empty = idx();
        assert!(page_of(&empty, ListSort::Modified, None, 3).is_empty());
        assert!(walk_all(&empty, ListSort::Modified, 3).is_empty());

        let exact = idx();
        seed(&exact, &[("a.md", 3), ("b.md", 2), ("c.md", 1)]);
        let page = page_of(&exact, ListSort::Modified, None, 3);
        assert_eq!(page.len(), 3);
        assert!(
            page.last().unwrap().cursor.is_none(),
            "恰好取满最后一页时也必须报「没有下一页」，否则前端会白跑一次空请求"
        );

        let partial = idx();
        seed(
            &partial,
            &[("a.md", 4), ("b.md", 3), ("c.md", 2), ("d.md", 1)],
        );
        let first = page_of(&partial, ListSort::Modified, None, 3);
        assert!(first.last().unwrap().cursor.is_some());
        let last = page_of(
            &partial,
            ListSort::Modified,
            first.last().unwrap().cursor.as_deref(),
            3,
        );
        assert_eq!(last.len(), 1);
        assert!(last[0].cursor.is_none());
    }

    /// 只有**末行**带游标——消费方式因此恒定一句 `hits.last()`。
    #[test]
    fn only_the_last_row_of_a_page_carries_the_cursor() {
        let i = idx();
        seed(&i, &[("a.md", 3), ("b.md", 2), ("c.md", 1)]);
        let page = page_of(&i, ListSort::Modified, None, 2);
        assert!(page[0].cursor.is_none());
        assert!(page[1].cursor.is_some());
    }

    /// top-N 语义（未给 `page`）不带任何游标——它给的是「前 N 条」，本来就没有下一页。
    #[test]
    fn top_n_queries_carry_no_cursor_and_keep_their_limit_meaning() {
        let i = idx();
        seed(&i, &[("a.md", 3), ("b.md", 2), ("c.md", 1)]);
        let hits = i
            .search(&Query {
                limit: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 2, "`limit` 仍是「取几条」，不是别的意思");
        assert!(hits.iter().all(|hit| hit.cursor.is_none()));
    }

    /// `LIST_CURSOR_INVALID`：游标乱码。
    #[test]
    fn a_garbled_cursor_is_rejected_with_the_stable_code() {
        let i = idx();
        seed(&i, &[("a.md", 1)]);
        for garbage in ["zzz", "xyxyxy", "0", "", "6162"] {
            let err = i
                .search(&Query {
                    limit: 2,
                    page: Some(ListPage {
                        sort: ListSort::Modified,
                        cursor: Some(garbage.to_owned()),
                    }),
                    ..Default::default()
                })
                .unwrap_err();
            assert_eq!(err.code(), "LIST_CURSOR_INVALID", "乱码游标：{garbage:?}");
            // 载荷说得出**是哪一个**游标坏了，码由 `code()` 单独给（ADR-105）。
            //
            // 这里此前断言的是「`Display` 必须以 `LIST_CURSOR_INVALID:` 开头」——那是
            // 「IPC 面只搬 `to_string()`、码到不了前端」时代的手工绕法，本变体为此在自己
            // 的 `#[error]` 里塞了一份码。批次 I 之后 `vault_err` 对每一条都发「码: 载荷」，
            // 那个特例撤销；把这条断言留着会把绕法钉死成契约。
            assert!(
                !err.to_string().contains("LIST_CURSOR_INVALID"),
                "码又被塞回 Display 了——那是 `vault_err` 的活：{err}"
            );
            assert!(!err.detail().is_empty(), "载荷不该是空的：{err}");
        }
    }

    /// `LIST_CURSOR_INVALID`：游标与本次请求的排序键不匹配。
    ///
    /// 这条不是理论危险：用户切一下排序控件、旧游标还在手上，若不拒就会拿名称序的
    /// 复合键去和时间序比较——结果既不报错也不重复，只是**静默漏掉**一段。
    #[test]
    fn a_cursor_from_another_sort_key_is_rejected() {
        let i = idx();
        seed(&i, &[("a.md", 3), ("b.md", 2), ("c.md", 1)]);
        let by_name = page_of(&i, ListSort::Name, None, 1);
        let cursor = by_name.last().unwrap().cursor.clone().unwrap();
        let err = i
            .search(&Query {
                limit: 2,
                page: Some(ListPage {
                    sort: ListSort::Modified,
                    cursor: Some(cursor),
                }),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.code(), "LIST_CURSOR_INVALID");
    }

    /// 全文检索（相关度序）**本期不分页**：给它分页语义当场被拒，而不是硬凑一个稳不住
    /// 的序。短词走的是 LIKE 兜底，同属检索路径，同样拒绝。
    #[test]
    fn full_text_search_refuses_list_paging() {
        let i = idx();
        i.index_note("a.md", "产品评审会议纪要", 0).unwrap();
        for text in ["产品评审", "会议"] {
            let err = i
                .search(&Query {
                    text: Some(text.to_owned()),
                    limit: 2,
                    page: Some(ListPage::default()),
                    ..Default::default()
                })
                .unwrap_err();
            assert_eq!(
                err.code(),
                "LIST_CURSOR_INVALID",
                "「{text}」走的是检索路径（长词 FTS / 短词 LIKE），两条都不接受分页"
            );
        }
        // 不给 page 就还是原来的 top-N，一个字没变。
        assert_eq!(i.search(&Query::text("产品评审")).unwrap().len(), 1);
    }

    /// 全文检索**仍然不分页**，但它的返回序必须可复现。
    ///
    /// 与分页是两件事：`ORDER BY score` 单独一列时，**bm25 平分的命中之间返回序未定义**
    /// ——同一个词搜两次可能给出不同的前 N 条边界，而没有任何东西会因此变红。夹具用几篇
    /// **正文逐字节相同**的笔记逼出必然的平分（先断言分数确实相等，免得这条用例空跑），
    /// 再断言：① 平分之间按 path 升序；② 同一查询两次逐条相同；③ 被 `limit` 截断时，
    /// 两次拿到的是**同一批**命中。
    #[test]
    fn full_text_search_is_reproducible_when_bm25_ties() {
        let i = idx();
        for path in ["d.md", "b.md", "e.md", "a.md", "c.md"] {
            i.index_note(path, "产品评审会议纪要", 0).unwrap();
        }
        let all = i
            .search(&Query {
                text: Some("产品评审".to_owned()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 5);
        assert!(
            all.iter().all(|hit| hit.score == all[0].score),
            "夹具没有逼出平分，这条用例就没在测它要测的东西：{:?}",
            all.iter().map(|h| h.score).collect::<Vec<_>>()
        );
        assert_eq!(
            all.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a.md", "b.md", "c.md", "d.md", "e.md"],
            "平分的命中之间按 path 升序——次序键是返回序可复现的唯一来源"
        );

        let truncated = |n: usize| -> Vec<String> {
            i.search(&Query {
                text: Some("产品评审".to_owned()),
                limit: n,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.path)
            .collect()
        };
        assert_eq!(truncated(10), truncated(10), "同一查询两次必须逐条相同");
        assert_eq!(
            truncated(2),
            vec!["a.md", "b.md"],
            "被 limit 截断时，「前 N 条」是哪 N 条也必须是确定的"
        );
        assert_eq!(truncated(2), truncated(2));
    }

    /// LIKE 兜底路径**有了确定性排序**：此前它只有 `LIMIT`，返回序完全未定义。
    #[test]
    fn the_like_fallback_now_has_a_deterministic_order() {
        let i = idx();
        i.index_note("老的.md", "会议记录", 10).unwrap();
        i.index_note("新的.md", "会议纪要", 20).unwrap();
        let hits = i.search(&Query::text("会议")).unwrap();
        assert_eq!(
            hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["新的.md", "老的.md"],
            "LIKE 路径的 score 恒为 0，故排序键取修改时间倒序 + path 次序"
        );
    }

    /// 名称筛选**下沉到 SQL**：先筛全库再分页，而不是筛已加载的那几页。
    ///
    /// 留在前端的表现是**静默给出错误结果**：第一页里没有匹配项时界面会说「筛选无
    /// 结果」，而库里明明有——只是它在第三页。
    #[test]
    fn the_name_filter_runs_before_paging_not_after() {
        let i = idx();
        seed(
            &i,
            &[
                ("其它一.md", 90),
                ("其它二.md", 80),
                ("Recordings/周会.md", 70),
                ("其它三.md", 60),
                ("周会补充.md", 50),
            ],
        );
        let hits = i
            .search(&Query {
                name_like: Some("周会".to_owned()),
                limit: 1,
                page: Some(ListPage::default()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "第一页就该给出匹配项——它在全库第三位，前端筛已加载页会看不见它"
        );
        assert_eq!(
            walk_all_named(&i, "周会"),
            vec!["Recordings/周会.md", "周会补充.md"],
        );
        // 路径命中同样算数（列表行上看得见的就是标题与路径这两样）。
        assert_eq!(
            walk_all_named(&i, "Recordings/"),
            vec!["Recordings/周会.md"]
        );
        // 全空白 = 不筛。
        assert_eq!(walk_all_named(&i, "   ").len(), 5);
    }

    fn walk_all_named(i: &Index, needle: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..1000 {
            let page = i
                .search(&Query {
                    name_like: Some(needle.to_owned()),
                    limit: 1,
                    page: Some(ListPage {
                        sort: ListSort::Modified,
                        cursor: cursor.clone(),
                    }),
                    ..Default::default()
                })
                .unwrap();
            let next = page.last().and_then(|hit| hit.cursor.clone());
            out.extend(page.into_iter().map(|hit| hit.path));
            match next {
                Some(c) => cursor = Some(c),
                None => return out,
            }
        }
        panic!("游标没有走到头");
    }

    /// 记录收件箱同样可被穷尽遍历（排序键恒为修改时间，游标挂信封）。
    #[test]
    fn the_draft_inbox_can_be_walked_to_the_end() {
        let i = idx();
        for (path, modified) in [("q1.md", 30), ("q2.md", 20), ("q3.md", 10)] {
            i.index_note(
                path,
                "---\nluna:\n  type: quick-note\n---\n随手记",
                modified,
            )
            .unwrap();
        }
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let (page, next) = i.draft_inbox(&all_drafts(), 2, cursor.as_deref()).unwrap();
            seen.extend(page.into_iter().map(|row| row.path));
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, vec!["q1.md", "q2.md", "q3.md"]);
        assert_eq!(
            i.draft_inbox(&all_drafts(), 2, Some("zzz"))
                .unwrap_err()
                .code(),
            "LIST_CURSOR_INVALID"
        );
    }

    #[test]
    fn the_draft_inbox_reports_ai_refinement_from_the_frontmatter_stamp() {
        let i = idx();
        i.index_note("a.md", "---\nluna:\n  type: quick-note\n---\n随手记", 20)
            .unwrap();
        i.index_note(
            "b.md",
            "---\nluna:\n  type: quick-note\n  refined_at: 2026-09-03T10:00:00+08:00\n---\n# 标题\n随手记",
            10,
        )
        .unwrap();
        let (page, _) = i.draft_inbox(&all_drafts(), 10, None).unwrap();
        let refined: Vec<(String, bool)> = page.into_iter().map(|r| (r.path, r.refined)).collect();
        assert_eq!(
            refined,
            vec![("a.md".to_owned(), false), ("b.md".to_owned(), true)]
        );
    }

    /// 按类型列出同样走键集游标（当前唯一消费者「对话历史」仍是 top-N，见 `ipc::chat`）。
    #[test]
    fn list_by_type_pages_with_a_keyset_cursor() {
        let i = idx();
        for (path, modified) in [("c1.md", 30), ("c2.md", 20), ("c3.md", 10)] {
            i.index_note(path, "---\nluna:\n  type: chat\n---\n会话", modified)
                .unwrap();
        }
        let (first, next) = i.list_by_type(NoteType::Chat, 2, None).unwrap();
        assert_eq!(
            first.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["c1.md", "c2.md"]
        );
        let (second, tail) = i.list_by_type(NoteType::Chat, 2, next.as_deref()).unwrap();
        assert_eq!(
            second.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["c3.md"]
        );
        assert!(tail.is_none());
    }

    /// 页大小只有 Rust 一处定义：请求把 `limit` 留空即「按它取一页」。
    #[test]
    fn an_omitted_limit_falls_back_to_the_single_page_size_constant() {
        let i = idx();
        for n in 0..(LIST_PAGE_SIZE + 5) {
            i.index_note(&format!("n{n:04}.md"), "正文", n as i64)
                .unwrap();
        }
        let page = page_of(&i, ListSort::Modified, None, 0);
        assert_eq!(page.len(), LIST_PAGE_SIZE);
        assert!(page.last().unwrap().cursor.is_some());
        assert_eq!(
            i.search(&Query::default()).unwrap().len(),
            50,
            "不给 page 时的默认上限仍是 50——`limit` 语义不变，不被页大小顺手改掉"
        );
    }

    /// 越出库根的链接**原样留着**：它是一条真断链，体检该报它，而不是被悄悄改成能打开的路径。
    #[test]
    fn a_link_that_escapes_the_vault_stays_as_written() {
        let i = idx();
        i.index_note("a.md", "[跑出去了](../../外面.md)", 0)
            .unwrap();
        assert_eq!(i.outgoing_links("a.md").unwrap(), vec!["../../外面.md"]);
    }
}
