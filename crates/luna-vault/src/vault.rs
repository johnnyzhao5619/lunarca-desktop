//! `Vault` 生命周期与文件操作（TDD-06 §7 接口、§4 写入/删除/快照、§4.1 只读接入）。
//!
//! 把数据安全四件套（[`crate::atomic`] / [`crate::frontmatter`] / [`crate::suppress`] /
//! [`crate::snapshot`]）、派生索引（[`crate::index`]）与模板（[`crate::template`]）整合为一个
//! 具体 `Vault`。本地文件是唯一事实源；`.luna/index.sqlite` 仅派生可重建。
//!
//! **写入纪律**：每次 `write` 在覆盖前对旧内容留快照（§4.7，非致命降级）、原子写（§4.3）、
//! 登记 `last_written_hash`（§4 自写抑制）、再重索引。**只读接入既有 Obsidian Vault**（§4.1）：
//! 扫描绝不改写既有文件；仅 `.luna/` 私有目录与 `.gitignore` 的幂等 Lunarca 块是合法附加写。

use crate::atomic::write_atomic;
use crate::error::{Result, VaultError};
use crate::frontmatter::{
    body_of, read_luna, update_luna, LunaMeta, MediaRef, NoteStage, NoteType,
};
use crate::index::{Index, Query, SearchHit, TagCount, TagSummary};
use crate::pathutil::markdown_link;
use crate::retrieve::{
    allows_grounding, pack_passages, question_terms, resolve_anchor, score_note, Anchor, AnchorHit,
    Retrieval, RetrieveOptions, CANDIDATES_PER_TERM, MAX_CANDIDATES,
};
use crate::sidecar::{
    read_sidecar_for_media, sidecar_path_for, write_sidecar, Sidecar, SidecarLoad,
};
use crate::snapshot::{snapshots_root, SnapshotMeta, SnapshotStore};
use crate::suppress::WriteGuard;
use crate::template::{render as render_template, TemplateVars};
use crate::timecode::format_timecode;
use crate::topic::{ownership_of, TopicOwnership};
use crate::usage::UsageStore;
use luna_core::Segment;
use luna_sync_core::diff::{diff_lines, LineDiff};
use luna_sync_core::hashing::ContentHash;
use luna_sync_core::search::{
    match_note, replace_in_note, NoteMatch, ReplaceNote, ReplacePlan, SearchQuery, SearchResults,
    CANDIDATE_LIMIT, MAX_LINES_PER_NOTE, MAX_RESULT_NOTES,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

mod sync_ops;
pub use sync_ops::{read_sync_file, sync_scope_bytes, sync_scope_files, SyncFile};

/// 删除策略（§4.6）。默认 [`DeleteMode::Trash`]（系统回收站，不直接 unlink）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    /// 系统回收站（macOS Trash / 平台 API）——MVP 默认，可恢复。
    #[default]
    Trash,
    /// Vault 内 `.trash/`（应用内回收站视图为 P1，但后端 API 完整）。
    VaultTrash,
    /// 永久删除（无回收站）。
    Permanent,
}

/// 一篇笔记的读出结果。`content` 为唯一事实源（含 frontmatter）；`meta` 为派生解析。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteDoc {
    /// 相对路径（forward-slash）。
    pub path: String,
    /// 完整文件内容（事实源）。
    pub content: String,
    /// 解析出的 `luna:` 元数据（缺省 type=note）。
    pub meta: LunaMeta,
}

/// 编辑器保存回执：文件持久化与派生索引更新分别报告。
#[derive(Clone, Debug, PartialEq)]
pub enum NoteSave {
    Saved { index_updated: bool },
    Conflict { current_content: Option<String> },
}

/// 目录树节点（「笔记」功能的真实 Vault 目录树，§5）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteTree {
    /// 节点名（文件/目录名）。
    pub name: String,
    /// 相对路径（forward-slash）。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 文件系统修改时间（Unix 毫秒；取不到为 0）。供前端按修改时间排序（PRD-06-006）。
    pub mtime_ms: i64,
    /// 文件系统创建时间（Unix 毫秒；文件系统不提供时回落修改时间）。供按创建时间排序
    /// （PRD-06-006 ①，Obsidian 同口径：取文件的出生时间，不读 frontmatter）。
    pub created_ms: i64,
    /// 它是不是**记录态**（`luna.stage: draft` 或缺省记录态的速记，ADR-086；目录恒 `false`）。
    ///
    /// 树内呈现 = 淡显 + 「记录」徽标 + 树头「隐藏记录」过滤开关（PRD-06-052）。判定在
    /// Rust（`NoteStage::resolve` → `notes.stage` 派生列），前端只消费已判定的事实
    /// （ADR-003）——与 `is_source_material` 同一先例。
    /// **刻意不加第四个段控**（PRD-06-048④ 不变）：成熟度会流转，段控暗示静态归属。
    ///
    /// 树里**没有**主题页/知识地图身份字段：知识产物根本不进树（ADR-096 ②，建树时按
    /// 身份过滤），恒为 `false` 的字段只会引人写出永不生效的分支。
    pub draft: bool,
    /// 它能不能被**「标记为记录」**（PRD-06-058 反向动作；目录与非 `.md` 恒 `false`）。
    ///
    /// = 知识态（[`NoteStage::allows_mark_as_draft`]）且非资料。判定在 Rust——树行右键与
    /// 编辑器右键都要问它，让前端各拼一遍，同一篇笔记在两个菜单里能做的事就会不一样
    /// （ADR-003）。
    pub markable_as_draft: bool,
    /// 它能不能**生成知识笔记**（= 资料 ∪ 记录，[`NoteStage::allows_distill`]；目录与
    /// 非 `.md` 恒 `false`）。
    ///
    /// **判定必须下发，不能让前端拿两位布尔去拼**：文件树此前把这一项写成"记录态才给"，
    /// 于是同一篇录音在录音清单里有「生成知识笔记」、在文件树里没有（2026-09-02 真机
    /// 上报「批量生成知识笔记没有入口」）。能力位的事实源只有 Rust 一个（ADR-003）。
    pub distillable: bool,
    /// 它是不是**资料**（来源轴 `NoteType::is_source_material`；目录与非 `.md` 恒 `false`）。
    ///
    /// 树行据此给身份徽标；前端**不再按"路径落在哪个归档目录之下"猜**——目录可改名、
    /// 笔记可被移出归档目录，两种情形都会让猜出来的答案与 Rust 的真答案分叉（ADR-003）。
    pub is_source_material: bool,
    /// 功能类型（`luna.type`）；目录为 `None`。树行的类型徽标据此取词（PRD-06-048①）。
    pub note_type: Option<crate::frontmatter::NoteType>,
    /// 它能不能作为「多条 → 一篇」的源（[`crate::merge_block`]；目录与非 `.md` 恒 `false`）。
    ///
    /// 判定与 `DraftNoteDto.can_merge`、`merge_notes` 的落盘前重验**同一个函数**——前端不拿
    /// "是不是记录态"去顶替它（2026-09-02 候选集放开后，那种顶替会把资料挡在门外）。
    pub can_merge: bool,
    /// 还在 iCloud 上没下载到本机（只有 `.名字.icloud` 占位，[`crate::icloud`]）：树上显示成
    /// 「未下载」，点开时请系统下载。
    pub cloud_placeholder: bool,
    /// 官方同步里只在云端（ADR-156）：本机没有副本，点开时按需下载（与 iCloud 占位是两回事——
    /// 那个请系统下载，这个由同步服务下）。
    pub sync_cloud_only: bool,
    /// 被哪条排除规则覆盖（[`crate::exclusions`]，V4a）；没被排除为 `None`。树行据此淡显，
    /// 右键据此判勾选态或「已随上级排除」——前端不自己拿规则去比路径（ADR-003）。
    pub excluded_by: Option<String>,
    /// 隐藏项（自己或某一级上级以 `.` 开头，[`luna_sync_core::links::is_hidden_rel`]）：只在「显示隐藏文件」
    /// 打开时出现在树里（V4b），树行淡显；它们不进索引、搜索与链接解析。
    pub hidden: bool,
    /// 子节点（目录才有）。
    pub children: Vec<NoteTree>,
}

impl NoteTree {
    fn bare(name: &str, path: &str, is_dir: bool) -> Self {
        Self {
            name: name.to_owned(),
            path: path.to_owned(),
            is_dir,
            mtime_ms: 0,
            created_ms: 0,
            draft: false,
            markable_as_draft: false,
            distillable: false,
            is_source_material: false,
            note_type: None,
            can_merge: false,
            cloud_placeholder: false,
            sync_cloud_only: false,
            excluded_by: None,
            hidden: false,
            children: Vec::new(),
        }
    }

    /// 本机还不存在、只因里面有仅云端的文件而补出来的目录（ADR-156）。
    pub(crate) fn virtual_dir(name: &str, path: &str) -> Self {
        Self::bare(name, path, true)
    }

    /// 同步里只在云端的文件（ADR-156）。
    pub(crate) fn sync_cloud_only_file(name: &str, path: &str) -> Self {
        Self {
            sync_cloud_only: true,
            ..Self::bare(name, path, false)
        }
    }
}

/// 一份位于用户模板目录中的 Markdown 模板。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTemplateInfo {
    /// 相对模板目录的路径（forward-slash），作为实例化时的稳定标识。
    pub path: String,
    /// 供界面展示的名称（路径去 `.md`）。
    pub name: String,
}

/// `.trash/` 回收站条目（VaultTrash 策略，§4.6）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrashEntry {
    /// 稳定 id（= `.trash/` 内文件名）。
    pub id: String,
    /// 原相对路径（还原去向）。
    pub original_path: String,
    /// 删除时刻（Unix 毫秒）。
    pub deleted_ms: u64,
    /// **随哪一条一起被删的**（那一条的 `id`）：笔记独占的附件跟着笔记进回收站
    /// （PRD-06-012 删除跟随），它们不是用户单独删掉的东西。
    ///
    /// **有归属的条目不单独列出、也不单独还原**：用户删的是"这篇笔记"，回收站里就该
    /// 只有这一行；还原它时附件一起回来，永久删除它时附件一起清掉。否则用户会看到一列
    /// 认不出的 uuid 音频，还得自己记住"哪个属于哪篇"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// 回收站条目预览类别（PRD-06-009：永久删除前判断内容，避免仅凭文件名误删）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashPreviewKind {
    /// 文本笔记（Markdown/纯文本）——`content` 为正文（可能截断）。
    Note,
    /// 文件夹删除——`children` 为子项相对路径列表。
    Folder,
    /// 二进制/媒体——无文本预览，只给体积。
    Binary,
}

/// 回收站**列表项** = 索引行 + 此刻从磁盘量出来的两格（[`Vault::list_trash`] 产出）。
///
/// **体积与"是不是文件夹"不进索引文件**：它们是磁盘此刻的事实，而 `.index.json` 是
/// 一份写于删除那一刻的记录。写进去就得回答"外部改动之后谁来更新它"，而列一次回收站
/// 本来就要 stat 那几条——量出来永远是对的，还省掉一次持久化格式迁移。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashListEntry {
    /// 稳定 id（= `.trash/` 内文件名）。
    pub id: String,
    /// 删除前的库内相对路径。
    pub original_path: String,
    /// 删除时刻（Unix 毫秒）。
    pub deleted_ms: u64,
    /// 这一条是不是文件夹（列表据此给文件夹图标，而不是把目录画成文件）。
    pub is_dir: bool,
    /// 占用字节数（文件夹为递归求和）。
    pub byte_size: u64,
}

/// 永久删除的结果（[`Vault::purge`] 产出，IPC 层投影为 DTO）。
///
/// 形状与 `VaultDeleteResultDto` 同构（成了几件 / 哪几件没成）**是刻意的**：同一个
/// 产品动作在两条路径上不该有两种交代方式。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeOutcome {
    /// 真正抹掉的条目数（含随主条目一起走的附件）。
    pub purged: u32,
    /// 未能抹掉的条目**原路径**（它们仍留在索引里，界面上照旧可见、可重试）。
    pub failed: Vec<String>,
}

/// 还原时落点已被占该怎么办（PRD-06-009）。
///
/// 有这一档之前，"原位置已经有个同名文件"只能以一个错误收场，而用户手里那篇笔记
/// 是**取不回来**的——除非他先自己去把占位的那个挪开。访达「放回原处」遇冲突给的是
/// 「两者都保留」，Notion / Drive 干脆自动改名：可恢复性不该被一次重名卡死。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreConflict {
    /// 报错，什么都不动（默认；界面据此去问用户要不要留副本）。
    Fail,
    /// 还原为副本（落点取唯一名），**原地那个文件一根手指都不碰**。
    KeepBoth,
}

/// 回收站条目预览载荷（[`Vault::trash_preview`] 产出，IPC 层投影为 DTO）。
#[derive(Clone, Debug)]
pub struct TrashPreview {
    /// 条目 id。
    pub id: String,
    /// 原相对路径（还原去向 / 展示名来源）。
    pub original_path: String,
    /// 预览类别。
    pub kind: TrashPreviewKind,
    /// 文本正文（仅 `Note`，可能按上限截断）。
    pub content: Option<String>,
    /// 文件夹子项相对路径（仅 `Folder`，按上限截断）。
    pub children: Vec<String>,
    /// 条目字节数（`Folder` 为 0）。
    pub byte_size: u64,
    /// 文本/子项是否因上限被截断。
    pub truncated: bool,
}

/// 文本预览字节上限（256 KiB：足够判断内容，又不把大文件全量读进内存）。
const TRASH_PREVIEW_MAX_BYTES: usize = 256 * 1024;
/// 文件夹子项列举上限（避免超大目录预览卡顿）。
const TRASH_PREVIEW_MAX_CHILDREN: usize = 500;

/// 文本可预览扩展（Markdown/纯文本类）；其余按二进制处理。
fn is_previewable_text(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "txt" | "text" | "csv" | "json" | "yml" | "yaml" | "toml" | "log")
    ) && path.contains('.')
}

/// 递归收集回收站文件夹内文件的相对路径（相对被删根，跳过点目录，达上限即停）。
fn collect_trash_children(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        if out.len() >= TRASH_PREVIEW_MAX_CHILDREN {
            return;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_trash_children(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// 一个打开的 Vault。
/// `(笔记路径, 文字)` 列表（标题表 / 别名表）。
pub(crate) type PathPairs = Vec<(String, String)>;

pub struct Vault {
    root: PathBuf,
    /// 库身份（`.luna/vault-id`，ADR-140 ②）：设备状态按它存在库外。
    id: crate::identity::VaultId,
    index: Index,
    snapshots: SnapshotStore,
    usage: UsageStore,
    guard: Mutex<WriteGuard>,
    /// 最近看到的 iCloud 未下载占位（真名路径，[`crate::icloud`]）。
    pub(crate) placeholders: Mutex<std::collections::HashSet<String>>,
    /// 官方同步里只在云端、本机没有副本的文件（库相对路径，ADR-156）：由同步服务每轮刷新，
    /// 树上按原位置列出、链接解析当作在。
    pub(crate) sync_cloud_only: Mutex<std::collections::HashSet<String>>,
    /// 最近一次全库遍历读不了的子目录（库相对路径，[`crate::scan::Walk::skipped_dirs`]）：
    /// 其下的笔记不进索引，体检据此列出「有文件夹读不了」。只在内存，下一次遍历即刷新。
    unreadable_dirs: Mutex<Vec<String>>,
}

/// 一篇笔记的**对象级事实**（[`Vault::note_facts`]，PRD-07-031 右侧上下文面板）。
///
/// **刻意不含字数**：字数归状态栏（PRD-07-065 把「量化事实」判给了状态栏，且那条
/// 是真机上「同一件事同屏出现三次、字数两次且措辞还不一样」修出来的）。编辑中的字数
/// 随输入实时变化，而这里给的是落盘事实——两个数并排放在一屏上必然有一刻不相等，
/// 那比不显示更坏。
#[derive(Clone, Debug, PartialEq)]
pub struct NoteFacts {
    /// 功能类型（frontmatter `luna.type`）。
    pub note_type: NoteType,
    /// 是否记录态（ADR-086 的判定结论，不是原始取值）。
    pub draft: bool,
    /// 是否来源层（录音 / 转写 / 对话，`NoteType::is_source_material` 的判定结论，PRD-06-048②）：
    /// 上下文面板据此说明"这一篇不入图谱、不自动打标、搜索中折叠"，前端不自带类型清单。
    pub source_material: bool,
    /// 创建时间（RFC3339 原样字符串；无该键为 `None`）。
    pub created: Option<String>,
    /// 索引记录的文件修改时间（Unix 毫秒）；`None` = 尚未入索引。
    pub modified_ms: Option<i64>,
    /// 这一篇的全部标签（用户标签与 AI 标签同池，口径见 `Index::note_tags`）。
    pub tags: Vec<String>,
    /// 其中写在 frontmatter 顶层 `tags` 里、可以就地移除的那些（ADR-135）；正文 `#标签`
    /// 与 AI 标签不在此列。
    pub removable_tags: Vec<String>,
}

/// 一条 wikilink 的落点（[`Vault::resolve_wikilink`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLinkHit {
    /// 指向的笔记（库内相对路径）；`exists = false` 时是该新建的路径。
    pub path: String,
    /// 库里有没有这一篇。
    pub exists: bool,
    /// 片段所在行（1 起，按整个文件计）；无片段或没找到为 `None`。
    pub line: Option<u32>,
    /// 片段是标题时，它是正文里第几个标题（0 起）——阅读视图按标题序号定位。
    pub heading_index: Option<u32>,
}

impl Vault {
    /// 打开（或初始化）Vault 根。确保 `.luna/` 私有目录与 `.gitignore` 的幂等 Lunarca 块（§4.1/§4.7），
    /// 打开派生索引（落后则自愈重建），随后由调用方触发 [`Vault::scan`] 建索引（可放后台线程）。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(VaultError::NotFound(root));
        }
        // Vault 生命周期内只认打开这一刻的真实根。若保存一条可再次解析的软链路径，之后每次
        // `root.join(rel)` 都可能被根链接替换整体导向库外；单篇文件句柄绑定也救不了这一级。
        // canonical root 被改名/移除时操作会失败关闭，不会跟随同名新链接改换事实源。
        let root = std::fs::canonicalize(&root)?;
        let luna = root.join(".luna");
        std::fs::create_dir_all(luna.join("cache"))?;
        std::fs::create_dir_all(luna.join("sidecars"))?;
        ensure_gitignore(&root)?;
        let id = crate::identity::ensure(&luna)?;
        crate::identity::remove_legacy_device_state(&luna);
        let index = Index::open(&luna.join("index.sqlite"))?;
        // 忽略清单的派生副本在建索引之前就位，扫描时被忽略的 AI 标签直接压下。清单读不出
        // （未知版本）时按「没有忽略」建索引，不因一份看不懂的决定文件打不开整个库。
        index.sync_ai_tag_ignores(
            &crate::meta::read_meta::<String>(&root, "ai_tag_ignores").unwrap_or_default(),
        )?;
        let snapshots = SnapshotStore::new(snapshots_root(&root));
        let usage = UsageStore::open(luna.join("usage.sqlite"))?;
        Ok(Self {
            root,
            id,
            index,
            snapshots,
            usage,
            guard: Mutex::new(WriteGuard::new()),
            placeholders: Mutex::new(std::collections::HashSet::new()),
            sync_cloud_only: Mutex::new(std::collections::HashSet::new()),
            unreadable_dirs: Mutex::new(Vec::new()),
        })
    }

    /// 只读打开（CLI，ADR-141 ④）：**不往库里写任何东西**——不建 `.luna/`、不动 `.gitignore`、
    /// 不补身份、不建 / 升级索引。只能读：读笔记、建树、搜索。（SQLite 读 WAL 索引时会用到
    /// 它自己的 `-shm` / `-wal` 协调文件，那是并发读的前提，不是库的内容。）
    ///
    /// 只认已经被应用（或 `luna vault index`）打开过的库：没有派生索引报
    /// [`VaultError::NotFound`]（指向 `.luna/index.sqlite`），索引版本不一致报
    /// [`VaultError::Decode`]，调用方据此说明该怎么办。
    pub fn open_read_only(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(VaultError::NotFound(root));
        }
        let root = std::fs::canonicalize(&root)?;
        let luna = root.join(".luna");
        let index = Index::open_read_only(&luna.join("index.sqlite"))?;
        let id = crate::identity::read(&luna)
            .ok_or_else(|| VaultError::NotFound(luna.join(crate::identity::VAULT_ID_FILE)))?;
        Ok(Self {
            snapshots: SnapshotStore::new(snapshots_root(&root)),
            usage: UsageStore::at(luna.join("usage.sqlite")),
            root,
            id,
            index,
            guard: Mutex::new(WriteGuard::new()),
            placeholders: Mutex::new(std::collections::HashSet::new()),
            sync_cloud_only: Mutex::new(std::collections::HashSet::new()),
            unreadable_dirs: Mutex::new(Vec::new()),
        })
    }

    /// 库身份（ADR-140 ②）。
    #[must_use]
    pub fn id(&self) -> crate::identity::VaultId {
        self.id
    }

    /// 换一个新身份：同一身份已被另一份仍在的库占用（用户复制了整个库）时由应用层调用，
    /// 后打开的这一份让出——两份库共用一套设备状态会互相剪枝标签页（ADR-140 ②）。
    pub fn reassign_id(&mut self) -> Result<()> {
        self.id = crate::identity::write(&self.root.join(".luna"))?;
        Ok(())
    }

    /// 设快照保留上限（§4.7，来自 `vault.snapshot_max_versions` 设置）。链式调用，复用
    /// [`SnapshotStore::with_max_versions`]——避免设置项被忽略（dead setting）。
    #[must_use]
    pub fn with_snapshot_max_versions(mut self, max: usize) -> Self {
        self.snapshots = SnapshotStore::new(snapshots_root(&self.root)).with_max_versions(max);
        self
    }

    /// Vault 根。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Vault 级用量库（durable，不随清缓存删除）。
    #[must_use]
    pub fn usage_store(&self) -> UsageStore {
        self.usage.clone()
    }

    /// 增量扫描（§4.1：**只读，绝不改写既有文件**）：只重读新出现与修改时间变了的笔记，删掉
    /// 磁盘上已经没有的，不先清空索引。返回扫描后索引里的笔记数。应用层在后台分批跑同样的
    /// 步骤（见 [`crate::scan`]），这里是一次跑完的版本（副库窗口、CLI 与测试）。
    pub fn scan(&self) -> Result<usize> {
        Ok(self.scan_all(false)?.indexed)
    }

    /// 「重建索引」：全部重读一遍，但同样不先清空——遍历失败时手上的索引还在。
    pub fn rebuild_index(&self) -> Result<usize> {
        Ok(self.scan_all(true)?.indexed)
    }

    fn scan_all(&self, force: bool) -> Result<crate::scan::Outcome> {
        let indexed = self.index.fingerprints()?;
        let walk = crate::scan::walk(&self.root)?;
        self.set_cloud_placeholders(walk.placeholders.iter().cloned());
        self.set_unreadable_folders(walk.skipped_dirs.iter().cloned());
        let plan = crate::scan::plan(&walk, &indexed, force);
        let known = if force {
            std::collections::HashMap::new()
        } else {
            indexed
        };
        let mut read = 0;
        for batch in plan.to_read.chunks(crate::scan::BATCH) {
            let items = crate::scan::read_batch(&self.root, batch, &known);
            read += items.len();
            self.apply_scan_batch(&items)?;
        }
        let removals: Vec<crate::scan::Item> = plan
            .to_remove
            .iter()
            .map(|path| crate::scan::Item::Remove { path: path.clone() })
            .collect();
        self.apply_scan_batch(&removals)?;
        Ok(crate::scan::Outcome {
            indexed: self.index.fingerprints()?.len(),
            read,
            removed: removals.len(),
            skipped_dirs: walk.skipped_dirs,
        })
    }

    /// 把一次外部文件变更同步进派生索引。监听层只传相对路径；正文仍直接从 Vault
    /// 读取，删除则走真正的 forget 语义，避免手工链接/标签与后续度量一直看到旧快照。
    pub fn sync_external_note(&self, rel: &str) -> Result<()> {
        let normalized = normalize_rel(rel);
        let abs = self.resolve(&normalized)?;
        match std::fs::read_to_string(&abs) {
            Ok(content) => self
                .index
                .index_note(&normalized, &content, file_mtime_ms(&abs)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.index.forget_note(&normalized)
            }
            Err(error) => Err(VaultError::Io(error)),
        }
    }

    /// 读取一篇笔记。
    pub fn read(&self, rel: &str) -> Result<NoteDoc> {
        let abs = self.resolve(rel)?;
        let content = std::fs::read_to_string(&abs).map_err(|e| {
            if e.kind() != std::io::ErrorKind::NotFound {
                VaultError::Io(e)
            } else if self.is_cloud_placeholder(rel) {
                VaultError::CloudPlaceholder(rel.into())
            } else {
                VaultError::NotFound(rel.into())
            }
        })?;
        let meta = read_luna(&content);
        Ok(NoteDoc {
            path: normalize_rel(rel),
            content,
            meta,
        })
    }

    /// 写入一篇笔记（覆盖既有）。覆盖前留快照（§4.7）、原子写（§4.3）、登记自写指纹（§4）、重索引。
    pub fn write(&self, rel: &str, content: &str) -> Result<()> {
        let abs = self.resolve(rel)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 覆盖前留快照（非致命降级：失败不阻断保存，§4.7）。
        if let Ok(old) = std::fs::read(&abs) {
            let _ = self.snapshots.snapshot(&normalize_rel(rel), &old);
        }
        self.commit_written(rel, &abs, content)
    }

    /// 仅当盘上整份文件仍是 `expected_hash` 时覆盖；不符返回 `Ok(false)` 且一个字节不写。
    ///
    /// 核对与写前快照用**同一次读取**的字节，于是被放行覆盖的内容一定留在快照里。
    /// 外部编辑器不参与本进程的锁，这里不宣称跨进程 CAS：残留窗口只剩这次读取与
    /// 原子 rename 之间，落在窗口内的外部写入会被覆盖，调用方须回读核对提交结果。
    /// 返回 `Err` 时文件可能已写成（索引失败），调用方须回读判定。
    pub fn write_if_current(&self, rel: &str, expected_hash: &str, content: &str) -> Result<bool> {
        let abs = self.resolve(rel)?;
        let old = match std::fs::read(&abs) {
            Ok(old) => old,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultError::NotFound(rel.into()));
            }
            Err(error) => return Err(VaultError::Io(error)),
        };
        if ContentHash::of(&old).as_str() != expected_hash {
            return Ok(false);
        }
        let _ = self.snapshots.snapshot(&normalize_rel(rel), &old);
        self.commit_written(rel, &abs, content)?;
        Ok(true)
    }

    /// 与编辑器读出基线核对后保存。外部删除也属于冲突，不重新创建文件。
    /// 与 `write_if_current` 一样不宣称跨进程 CAS；覆盖前保留同次读取的快照。
    pub fn save_note(&self, rel: &str, expected_content: &str, content: &str) -> Result<NoteSave> {
        let abs = self.resolve(rel)?;
        let old = match std::fs::read_to_string(&abs) {
            Ok(old) => old,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(NoteSave::Conflict {
                    current_content: None,
                });
            }
            Err(error) => return Err(VaultError::Io(error)),
        };
        if old != expected_content {
            return Ok(NoteSave::Conflict {
                current_content: Some(old),
            });
        }
        let _ = self.snapshots.snapshot(&normalize_rel(rel), old.as_bytes());
        self.commit_file(&abs, content)?;
        let index_updated = self
            .index
            .index_note(&normalize_rel(rel), content, file_mtime_ms(&abs))
            .is_ok();
        Ok(NoteSave::Saved { index_updated })
    }

    fn commit_file(&self, abs: &Path, content: &str) -> Result<()> {
        write_atomic(abs, content.as_bytes())?;
        // 登记 last_written_hash → 监听到的本次事件判为自写、不触发冲突（§4）。
        if let Ok(mut g) = self.guard.lock() {
            g.record_write(abs, ContentHash::of(content.as_bytes()));
        }
        Ok(())
    }

    fn commit_written(&self, rel: &str, abs: &Path, content: &str) -> Result<()> {
        self.commit_file(abs, content)?;
        self.index
            .index_note(&normalize_rel(rel), content, file_mtime_ms(abs))?;
        Ok(())
    }

    /// 经模板新建笔记（§10）。目标已存在则报错（不覆盖）。
    pub fn create(
        &self,
        rel: &str,
        kind: crate::frontmatter::NoteType,
        vars: &TemplateVars,
    ) -> Result<NoteDoc> {
        let abs = self.resolve(rel)?;
        // **载荷是库相对路径，不是绝对路径**（真机 2026-08-31）：这一格现在直接
        // 上界面（ADR-105 的 `{ $detail }`），而绝对路径既把 `/Users/<用户名>/…` 摆给
        // 了用户，也与本仓其它每一处「是哪一篇」的写法不一致——用户认得的是
        // `Notes/周会纪要.md`，不是他家目录的全路径。本族全部路径变体同此口径。
        if abs.exists() {
            return Err(VaultError::AlreadyExists(rel.into()));
        }
        // 建档时间在**这一处**盖章（TDD-06 §2.1）：每一条建档路径（录音停止 / 导入 /
        // 对话存档 / 速记 / 「添加内容」）都经此，此前没有一条写 `created`，于是信息页签对每
        // 一篇录音都说「创建：未记录」，列表也只能拿修改时间冒充录制时间（PRD-07-007）。
        // 调用方给了就原样用（导入可带媒体自己的时间）；普通笔记走 lazy 骨架，不注入 `luna:`。
        let stamped;
        let vars = if vars.created.is_none() && !matches!(kind, crate::frontmatter::NoteType::Note)
        {
            stamped = TemplateVars {
                created: Some(chrono::Local::now().to_rfc3339()),
                ..vars.clone()
            };
            &stamped
        } else {
            vars
        };
        let content = render_template(kind, vars)?;
        self.write(rel, &content)?;
        self.read(rel)
    }

    /// 给笔记追加一条媒体映射（建映射，§7）。仅就地改写 `luna:` 块（§4.4 round-trip）。
    pub fn attach_media(&self, rel: &str, media: MediaRef) -> Result<()> {
        let doc = self.read(rel)?;
        let mut meta = doc.meta;
        meta.media.push(media);
        let new_content = update_luna(&doc.content, &meta)?;
        self.write(rel, &new_content)
    }

    /// 录音归档两阶段汇合（TDD-06 §2.1/§2.2）。
    ///
    /// **阶段①** 转录提交：
    /// - 为每个媒体分片写入耐久边车（`.luna/sidecars/<hash8>.segments.json`，原子写）；
    /// - 把全部 committed 段以 `[HH:MM:SS.mmm] 文本` 格式追加到笔记正文，
    ///   书签以 `[HH:MM:SS.mmm] 🔖` 行按时间序穿插（可读锚点层，PRD-04-005；
    ///   语言中立：正文是用户数据，不含 UI 文案）；
    /// - 更新 frontmatter `luna.media[]` 与 `luna.bookmarks`（毫秒精确层），
    ///   登记各分片绝对区间 `part_start_ms/part_end_ms`（单 part 不写区间字段）。
    ///
    /// **阶段②** 译文回填（实时转译已实现，TDD-08）：
    /// - `Segment.translation` 有值的段，在其正文行下追加 `> 译文` 引用行
    ///   （无行首时间标记——时间锚点唯一归属源文行，行级 seek 扫描不受译文行干扰）；
    /// - 边车随 `Segment` 序列化自带 `translation` 字段（双语回放/导出的精确层）；
    /// - `MediaRef.translation` 按分片内是否有译文段如实标记；
    /// - 无译文段（转译关闭/失败/超时）不写任何占位——`translation: pending`
    ///   永不写入假译文（诚实元数据，本 crate 规约）。
    ///
    /// # Errors
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：边车写入失败。
    pub fn finalize_recording(
        &self,
        note_rel: &str,
        parts: &[RecordingPart],
        segments: &[Segment],
        bookmarks: &[Bookmark],
        authority: TranscriptAuthority,
    ) -> Result<()> {
        // ── 阶段①：转录提交 ─────────────────────────────────────────────────
        let doc = self.read(note_rel)?;

        // 1. 构建带哨兵的转写块（转写行 + 译文引用行 + 书签锚点行按时间序合并）。
        //    哨兵使块**可定位替换**——重转写（[`Self::rewrite_recording_transcript`]）据此
        //    整块替换，既保持正文↔边车一致，又不误伤块外的用户散文（TDD-06 §2.2）。
        let sorted_bookmarks = normalize_bookmarks(bookmarks);
        let block = build_transcript_block(segments, &sorted_bookmarks);

        // 2. 在原有正文的哨兵块处替换（首次归档无块 → 追加），保留 frontmatter 原字节不变。
        let existing_body = body_of(&doc.content);
        let front_len = doc.content.len() - existing_body.len();
        let mut new_content = doc.content[..front_len].to_string();
        new_content.push_str(&replace_or_append_transcript_block(existing_body, &block));

        // 3. 为每个分片写边车，并构建更新后的 media[] 列表。
        let multi_part = parts.len() > 1;
        let mut meta = doc.meta.clone();
        for part in parts {
            // 筛出属于本分片时间区间 [start_ms, end_ms) 的段。
            let part_segs: Vec<Segment> = segments
                .iter()
                .filter(|s| {
                    let ms = s.start.as_millis() as u64;
                    ms >= part.start_ms && ms < part.end_ms
                })
                .cloned()
                .collect();

            // 诚实元数据：零段分片不写空边车、`transcript: false`（纯录音/无语音分片）。
            // 读取链已有降级（sidecar miss → empty cues）；二次转写补全后再写边车并翻正标记。
            let has_segments = !part_segs.is_empty();
            let has_translation = part_segs.iter().any(|s| s.translation.is_some());
            let segments_rel = if has_segments {
                let sidecar_rel = sidecar_path_for(&part.path);
                let sidecar_abs = self.root.join(&sidecar_rel);
                if let Some(parent) = sidecar_abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_sidecar(&sidecar_abs, &Sidecar::new(part_segs))?;
                Some(sidecar_rel)
            } else {
                // 同一不变量（见 `rewrite_recording_transcript`）：无段落即删边车文件，
                // 否则重新登记同一媒体时会读回上一轮的旧段落。
                crate::remove_sidecar_file(&self.root.join(sidecar_path_for(&part.path)))?;
                None
            };

            meta.media.push(MediaRef {
                path: part.path.clone(),
                duration: part.duration,
                transcript: has_segments,
                translation: has_translation,
                segments: segments_rel,
                model: part.model.clone(),
                lang: part.lang.clone(),
                // 多 part 时才写区间；单 part 不写（字段省略）。
                part_start_ms: multi_part.then_some(part.start_ms),
                part_end_ms: multi_part.then_some(part.end_ms),
                tracks: part.tracks.clone(),
            });
        }

        // 4. 书签精确层写入 frontmatter（升序去重；正文锚点行为可读层）。
        meta.bookmarks = sorted_bookmarks.iter().map(|b| b.ms).collect();
        authority.apply(&mut meta);

        // 5. 将更新后的 meta 写回 frontmatter（局部改写，§4.4）并持久化。
        let final_content = update_luna(&new_content, &meta)?;
        self.write(note_rel, &final_content)?;

        Ok(())
    }

    /// 权威重转写写回（PRD-04-030 / 停录后台重转写 / 导入）：用**全量新段**整体替换该录音
    /// 笔记的转写——重写正文哨兵块 + 各分片耐久边车 + `media[]` 的 `transcript`/`translation`
    /// 标记与 `model`，保持**正文↔边车一致**。段时间须为**会话绝对毫秒**（多分片调用方先加
    /// 分片偏移）。
    ///
    /// 与初次 [`Self::finalize_recording`] 的差异：媒体已登记 → 按 `media[].path` **原地更新**
    /// （不追加）；正文按哨兵 **整块替换**（不追加），块外的用户散文保持不动。零段媒体翻
    /// `transcript:false` 并去边车引用（诚实元数据，TDD-06 §2.2）。书签沿用已登记的精确层。
    ///
    /// # Errors
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：边车或笔记写入失败。
    pub fn rewrite_recording_transcript(
        &self,
        note_rel: &str,
        segments: &[Segment],
        model: Option<String>,
        authority: TranscriptAuthority,
    ) -> Result<()> {
        let doc = self.read(note_rel)?;
        let mut meta = doc.meta.clone();

        // 1. 整块替换正文转写（书签沿用精确层 `meta.bookmarks`；备注只活在可读层的 🔖 行里，
        //    重写前先从旧正文读回来，否则二次转写会把用户写的备注冲掉）。
        let bookmarks = bookmarks_of(&meta.bookmarks, &doc.content);
        let block = build_transcript_block(segments, &bookmarks);
        let existing_body = body_of(&doc.content);
        let front_len = doc.content.len() - existing_body.len();
        let mut new_content = doc.content[..front_len].to_string();
        new_content.push_str(&replace_or_append_transcript_block(existing_body, &block));

        // 2. 逐**挂载槽**按分片区间切分绝对段（单槽区间为 None → 全段归属），写边车。
        //    槽 = 有本地媒体走各分片、无本地媒体走来源地址（`transcript_slots`，读写同一口径）
        //    ——「仅存链接」的转写（PRD-05-006）没有 `meta.media`，段落挂在源地址那一个槽上。
        for slot in crate::transcript_slots(&meta) {
            let (lo, hi) = (slot.part_start_ms, slot.part_end_ms);
            let part_segs: Vec<Segment> = segments
                .iter()
                .filter(|s| {
                    let ms = s.start.as_millis() as u64;
                    match (lo, hi) {
                        (Some(a), Some(b)) => ms >= a && ms < b,
                        _ => true,
                    }
                })
                .cloned()
                .collect();
            let has_segments = !part_segs.is_empty();
            let sidecar_rel = sidecar_path_for(&slot.key);
            let sidecar_abs = self.root.join(&sidecar_rel);
            if has_segments {
                if let Some(parent) = sidecar_abs.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_sidecar(&sidecar_abs, &Sidecar::new(part_segs.clone()))?;
            } else {
                // **边车文件与段落一一对应**：段落没了就删文件，不能只把 `media.segments`
                // 置空——读取端认的是"文件在不在"（无媒体槽根本没有那个字段可查），留一个
                // "逻辑上已删、物理上还在"的边车会让下次加载把删掉的段落读回来。
                crate::remove_sidecar_file(&sidecar_abs)?;
            }
            // 媒体标记只对**本地媒体槽**有意义（无媒体时没有 MediaRef 可标）。
            if let Some(media) = meta.media.iter_mut().find(|m| m.path == slot.key) {
                media.segments = has_segments.then(|| sidecar_rel.clone());
                media.transcript = has_segments;
                media.translation = part_segs.iter().any(|s| s.translation.is_some());
                if model.is_some() {
                    media.model = model.clone();
                }
            }
        }
        authority.apply(&mut meta);

        let final_content = update_luna(&new_content, &meta)?;
        self.write(note_rel, &final_content)
    }

    /// 同步回放行内编辑：按毫秒级 cue 区间更新耐久边车，并尽力同步 md 可读时间戳行。
    ///
    /// 该操作只更新用户选中的既有分段，不拆分/合并结构；完整 Markdown 重排仍归编辑器。
    ///
    /// # Errors
    /// - `VaultError::InvalidCue`：文本为空、无可用边车或未找到匹配 cue。
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：边车或笔记写入失败。
    pub fn update_playback_cue(
        &self,
        note_rel: &str,
        start_ms: u64,
        end_ms: u64,
        text: &str,
        translation: Option<&str>,
    ) -> Result<()> {
        let next_text = text.trim();
        if next_text.is_empty() {
            return Err(VaultError::InvalidCue("字幕文本不能为空".into()));
        }

        let doc = self.read(note_rel)?;
        let mut meta = doc.meta.clone();
        let mut matched = false;

        for media_ref in &mut meta.media {
            if media_ref.segments.is_none() {
                continue;
            }

            let SidecarLoad::Ok(mut sidecar) = read_sidecar_for_media(&self.root, &media_ref.path)
            else {
                continue;
            };

            let segment = sidecar.segments.iter_mut().find(|seg| {
                seg.start.as_millis() as u64 == start_ms && seg.end.as_millis() as u64 == end_ms
            });

            let Some(segment) = segment else {
                continue;
            };

            segment.text = next_text.to_string();
            segment.words = None; // 词表描述旧文本的时间，文本一变即失效（TDD-03 §10.4）。
            if let Some(next_translation) = translation {
                let trimmed = next_translation.trim();
                segment.translation = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }

            let sidecar_rel = sidecar_path_for(&media_ref.path);
            let sidecar_abs = self.root.join(&sidecar_rel);
            if let Some(parent) = sidecar_abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_sidecar(&sidecar_abs, &sidecar)?;

            media_ref.segments = Some(sidecar_rel);
            media_ref.transcript = true;
            media_ref.translation = sidecar.segments.iter().any(|seg| {
                seg.translation
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty())
            });
            matched = true;
            break;
        }

        if !matched {
            return Err(VaultError::InvalidCue(format!(
                "未找到 {start_ms}-{end_ms}ms 对应分段"
            )));
        }

        let updated_content = replace_timed_text_line(&doc.content, start_ms, next_text);
        let final_content = update_luna(&updated_content, &meta)?;
        self.write(note_rel, &final_content)
    }

    /// 整篇转写编辑批量写回（PRD-04-031「文本编辑模式」的原子保存）：把用户在整篇校对
    /// 编辑器里改过的多条 cue 文本一次性权威落盘——按 `(start_ms, end_ms)` 匹配既有段更新
    /// `text`（**保留** `translation`/`speaker`/时间结构），随后经
    /// [`Self::rewrite_recording_transcript`] 整块重写正文哨兵块 + 各分片边车（单一写回点，
    /// 避免逐条 N 次全文重写）。`model=None` 保留原模型标记。
    ///
    /// 无匹配/无实际改动 = 幂等 no-op（不写盘）。空文本一律拒绝（与单条编辑同契约）。
    ///
    /// # Errors
    /// - `VaultError::InvalidCue`：任一编辑文本为空。
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：边车或笔记写入失败。
    pub fn update_playback_transcript(&self, note_rel: &str, edits: &[CueTextEdit]) -> Result<()> {
        let doc = self.read(note_rel)?;
        let mut segments = self.collect_note_segments(&doc.meta.media);

        let mut applied = 0usize;
        for edit in edits {
            let next_text = edit.text.trim();
            if next_text.is_empty() {
                return Err(VaultError::InvalidCue("字幕文本不能为空".into()));
            }
            if let Some(segment) = segments.iter_mut().find(|seg| {
                seg.start.as_millis() as u64 == edit.start_ms
                    && seg.end.as_millis() as u64 == edit.end_ms
            }) {
                if segment.text != next_text {
                    segment.text = next_text.to_string();
                    segment.words = None; // 词表描述旧文本的时间，文本一变即失效（TDD-03 §10.4）。
                    applied += 1;
                }
            }
        }

        if applied == 0 {
            return Ok(()); // 无实际改动：不写盘（避免无谓快照/重索引）。
        }
        self.rewrite_recording_transcript(note_rel, &segments, None, TranscriptAuthority::Unchanged)
    }

    /// 跨全部分片收集绝对时钟段（边车是事实源，时间已是会话绝对 ms），按起点升序。
    /// 整篇编辑与说话人手动校正共用此取数口径（DRY）。
    fn collect_note_segments(&self, media: &[MediaRef]) -> Vec<Segment> {
        let mut segments: Vec<Segment> = Vec::new();
        for media_ref in media {
            if media_ref.segments.is_none() {
                continue;
            }
            if let SidecarLoad::Ok(sidecar) = read_sidecar_for_media(&self.root, &media_ref.path) {
                segments.extend(sidecar.segments);
            }
        }
        segments.sort_by_key(|seg| seg.start);
        segments
    }

    /// 转写段人工校正（PRD-04-031 / PRD-04-056 / TDD-03 §10.4）：重指派 / 拆分 / 合并 / 删除
    /// 四件人工兜底，算法在 [`luna_core::segment`]（纯函数），本方法只负责「读边车段 →
    /// 应用 → 权威写回」。
    ///
    /// 写回复用 [`Self::rewrite_recording_transcript`]（正文哨兵块 + 各分片边车 + 标记一致的
    /// 单一写回点）；拆分/合并/删除会改变段数与段界，正文行标记因此整块重建。
    ///
    /// # Errors
    /// - `VaultError::InvalidCue`：段未匹配 / 拆分点切出空片 / 首段无「上一条」可合并。
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：边车或笔记写入失败。
    pub fn edit_transcript_segment(&self, note_rel: &str, edit: &SegmentEdit) -> Result<()> {
        let doc = self.read(note_rel)?;
        let mut segments = self.collect_note_segments(&doc.meta.media);
        let to_err = |e: luna_core::SegmentEditError| VaultError::InvalidCue(e.to_string());
        match edit {
            SegmentEdit::Assign {
                start_ms,
                end_ms,
                speaker,
                scope,
            } => {
                let changed =
                    luna_core::assign_speaker(&mut segments, *start_ms, *end_ms, speaker, *scope)
                        .map_err(to_err)?;
                if changed == 0 {
                    return Ok(()); // 幂等：目标与原值相同，不写盘。
                }
            }
            SegmentEdit::Split {
                start_ms,
                end_ms,
                char_offset,
                speaker,
            } => luna_core::split_segment(
                &mut segments,
                *start_ms,
                *end_ms,
                *char_offset,
                speaker.as_deref(),
            )
            .map_err(to_err)?,
            SegmentEdit::Merge { start_ms, end_ms } => {
                luna_core::merge_with_previous(&mut segments, *start_ms, *end_ms).map_err(to_err)?
            }
            SegmentEdit::Delete { start_ms, end_ms } => {
                luna_core::delete_segment(&mut segments, *start_ms, *end_ms).map_err(to_err)?
            }
        }
        self.rewrite_recording_transcript(note_rel, &segments, None, TranscriptAuthority::Unchanged)
    }

    /// 说话人改名持久化（PRD-04-054 / TDD-03 §10.3）：把 speaker id（`self`/`remote`/
    /// `remote:N`）的显示名写入笔记 frontmatter `luna.speakers`（笔记即文件，可追溯）。
    /// `name` 去空白后为空 = 删除映射（回退默认「我/对方/对方N」标签）。
    ///
    /// # Errors
    /// - `VaultError::InvalidCue`：speaker id 为空。
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：写入失败。
    pub fn set_speaker_name(&self, note_rel: &str, speaker: &str, name: &str) -> Result<()> {
        let speaker = speaker.trim();
        if speaker.is_empty() {
            return Err(VaultError::InvalidCue("speaker id 不能为空".into()));
        }
        let doc = self.read(note_rel)?;
        let mut meta = doc.meta.clone();
        let name = name.trim();
        if name.is_empty() {
            meta.speakers.remove(speaker);
        } else {
            meta.speakers.insert(speaker.to_owned(), name.to_owned());
        }
        if meta.speakers == doc.meta.speakers {
            return Ok(()); // 无变化不写盘（避免无谓快照/重索引）。
        }
        let final_content = update_luna(&doc.content, &meta)?;
        self.write(note_rel, &final_content)
    }

    /// 记录内容来源地址（PRD-05-006「从链接导入」）：把抓取来源写入 `luna.source_url`，
    /// 使笔记自身携带溯源信息（笔记即文件，随迁移不丢）。空串 = 清除该字段。
    ///
    /// 抓取任务在建好笔记后立即调用一次；与转写产物写回是两条独立路径，故不合并——
    /// 来源在"下载完成"那一刻就已确定，不该等转写跑完才落盘（中途取消也该留下来源痕迹）。
    ///
    /// # Errors
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io`：写入失败。
    pub fn set_source_url(&self, note_rel: &str, url: &str) -> Result<()> {
        let doc = self.read(note_rel)?;
        let mut meta = doc.meta.clone();
        let url = url.trim();
        meta.source_url = (!url.is_empty()).then(|| url.to_owned());
        if meta.source_url == doc.meta.source_url {
            return Ok(()); // 无变化不写盘（避免无谓快照/重索引）。
        }
        let final_content = update_luna(&doc.content, &meta)?;
        self.write(note_rel, &final_content)
    }

    /// 文件夹里（不含子文件夹）原文件指纹为 `sha256` 的那篇导入文档（ADR-164 ④）；没有 → `None`。
    ///
    /// 只看这一层：同一份资料用在两个项目里是正常的，跨文件夹认出来会让第二个项目里找不到它。
    ///
    /// # Errors
    /// 文件夹路径不合法时返回错误；文件夹不在视为没有。
    pub fn find_document(&self, folder: &str, sha256: &str) -> Result<Option<String>> {
        self.find_document_where(folder, |meta| {
            meta.original.as_ref().is_some_and(|o| o.sha256 == sha256)
        })
    }

    /// 文件夹里（不含子文件夹）来源地址为 `url` 的那篇剪藏（ADR-164 ⑦ 的去重）；没有 → `None`。
    ///
    /// # Errors
    /// 同 [`Self::find_document`]。
    pub fn find_clip(&self, folder: &str, url: &str) -> Result<Option<String>> {
        self.find_document_where(folder, |meta| meta.source_url.as_deref() == Some(url))
    }

    /// 文件夹这一层里第一篇满足条件的 `document` 笔记（按文件名排序，结果稳定）。
    fn find_document_where(
        &self,
        folder: &str,
        matches: impl Fn(&crate::LunaMeta) -> bool,
    ) -> Result<Option<String>> {
        let dir = self.resolve(folder)?;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(None);
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".md") && !name.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let rel = luna_sync_core::attachment::join_vault_rel(folder, &name);
            let Ok(doc) = self.read(&rel) else {
                continue;
            };
            if doc.meta.note_type == NoteType::Document && matches(&doc.meta) {
                return Ok(Some(rel));
            }
        }
        Ok(None)
    }

    /// 全库关系图谱（PRD-06-034）：由派生索引底表构建笔记/标签/幽灵节点与链接/标签边。
    ///
    /// 排除路径（V4a）在**建图之后**拿掉：先按全量底表建图，指向被排除笔记的链接才不会被误画成
    /// 「未创建」的幽灵节点。
    pub fn graph(&self) -> Result<crate::graph::VaultGraph> {
        let graph = crate::graph::build_graph(&self.index.graph_rows()?);
        Ok(self.exclusions().prune_graph(graph))
    }

    /// 枚举用户模板目录中的 Markdown 文件。模板目录缺失等价于空库，读取不创建任何文件。
    ///
    /// 模板本身不进入笔记索引；调用方只能拿到相对模板根的路径，实例化时再经过
    /// [`Self::read_user_template`] 的同一安全边界校验。
    pub fn list_user_templates(&self, template_dir: &str) -> Result<Vec<UserTemplateInfo>> {
        let Some(root) = self.user_template_root(template_dir)? else {
            return Ok(Vec::new());
        };
        let mut paths = Vec::new();
        walk_user_templates(&root, &root, &mut paths)?;
        paths.sort();
        Ok(paths
            .into_iter()
            .map(|path| UserTemplateInfo {
                name: path.trim_end_matches(".md").to_owned(),
                path,
            })
            .collect())
    }

    /// 读取一份用户选择的模板，拒绝路径穿越与逃出模板目录的符号链接。
    pub fn read_user_template(&self, template_dir: &str, template_path: &str) -> Result<String> {
        let rel = Path::new(template_path);
        if !template_path.to_ascii_lowercase().ends_with(".md")
            || rel.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(VaultError::PathEscape(rel.to_path_buf()));
        }
        let root = self
            .user_template_root(template_dir)?
            .ok_or_else(|| VaultError::NotFound(self.root.join(template_dir)))?;
        let candidate = root.join(rel).canonicalize()?;
        if !candidate.starts_with(&root) || !candidate.is_file() {
            return Err(VaultError::PathEscape(candidate));
        }
        std::fs::read_to_string(candidate).map_err(Into::into)
    }

    /// 往用户模板目录里加一份模板（「复制到库」，PRD-18-009）：`name.md` 被占就 `name-1.md`、`name-2.md`……
    /// **从不覆盖**已有文件；目录不存在时先建。返回相对模板根的路径（与 [`Self::list_user_templates`] 同一口径）。
    ///
    /// # Errors
    /// 名字带路径分隔符或为空、模板目录逃出 Vault、写盘失败时。
    pub fn add_user_template(
        &self,
        template_dir: &str,
        name: &str,
        content: &str,
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(VaultError::PathEscape(PathBuf::from(name)));
        }
        std::fs::create_dir_all(self.resolve(template_dir)?)?;
        let root = self
            .user_template_root(template_dir)?
            .ok_or_else(|| VaultError::NotFound(self.root.join(template_dir)))?;
        let file = (0..)
            .map(|index| {
                if index == 0 {
                    format!("{name}.md")
                } else {
                    format!("{name}-{index}.md")
                }
            })
            .find(|file| !root.join(file).exists())
            .expect("总能找到空位");
        crate::atomic::write_atomic(&root.join(&file), content.as_bytes())?;
        Ok(file)
    }

    /// 返回经真实路径校验的模板根。配置值在词法上已经是 Vault 相对路径仍不足以防住
    /// 「目录本身是指向库外的符号链接」；故这里再校验 canonical root 仍位于 Vault 内。
    fn user_template_root(&self, template_dir: &str) -> Result<Option<PathBuf>> {
        let declared = self.resolve(template_dir)?;
        if !declared.exists() {
            return Ok(None);
        }
        if !declared.is_dir() {
            return Err(VaultError::NotFound(declared));
        }
        let vault_root = self.root.canonicalize()?;
        let root = declared.canonicalize()?;
        if !root.starts_with(vault_root) {
            return Err(VaultError::PathEscape(root));
        }
        Ok(Some(root))
    }

    /// 写回 AI 关系标签（PRD-06-035）：整体替换 `luna.ai_tags` 并记录生成时正文指纹
    /// `luna.ai_tags_src`（幂等门控依据）。用户顶层 `tags` 与正文逐字不动（§4.4
    /// round-trip）。无变化不写盘；写盘返回新全文（供调用方广播 `note://written`）。
    ///
    /// # Errors
    /// - `VaultError::NotFound`：笔记不存在。
    /// - `VaultError::Io` / `VaultError::Decode`：写入或 YAML 渲染失败。
    pub fn set_ai_tags(
        &self,
        note_rel: &str,
        tags: &[String],
        src_hash: &str,
    ) -> Result<Option<String>> {
        let doc = self.read(note_rel)?;
        let mut meta = doc.meta.clone();
        meta.ai_tags = tags.to_vec();
        meta.ai_tags_src = Some(src_hash.to_owned());
        if meta.ai_tags == doc.meta.ai_tags && meta.ai_tags_src == doc.meta.ai_tags_src {
            return Ok(None); // 无变化不写盘（避免无谓快照/重索引）。
        }
        let final_content = update_luna(&doc.content, &meta)?;
        self.write(note_rel, &final_content)?;
        Ok(Some(final_content))
    }

    /// 蒸馏笔记状态（PRD-06-044 / ADR-024）：读源资料笔记，判定它此刻处于四态中的哪一态。
    ///
    /// 只读不写。四态判定的**全部依据**都在文件里（指针 + 两个指纹），故重启、换设备、
    /// 用户在外部编辑器里改动都自然成立——不引入任何设备级状态。
    pub fn distill_state(&self, note_rel: &str) -> Result<DistillState> {
        let source = self.read(note_rel)?;
        let Some(target) = source.meta.distill_note.clone() else {
            return Ok(DistillState::Absent);
        };
        // 指针悬空（蒸馏笔记被删/被移走）→ 回「未生成」。绝不复活：删除是显式意图。
        let Ok(distilled) = self.read(&target) else {
            return Ok(DistillState::Absent);
        };
        let source_hash = ContentHash::of(body_of(&source.content).as_bytes());
        let body_hash = ContentHash::of(body_of(&distilled.content).as_bytes());
        let edited = distilled.meta.distill_body.as_deref() != Some(body_hash.as_str());
        let stale = distilled.meta.distill_src.as_deref() != Some(source_hash.as_str());
        Ok(DistillState::Present {
            path: target,
            stale,
            edited,
            // 「不完整」与 `stale` 一样是**一态两标志**而不是第三个枚举变体：一篇产物完全
            // 可以同时是「不完整」且「源已更新」且「用户编辑过」，摊成互斥枚举会丢信息。
            // 「取，不猜」（同 [`luna_core::TruncatedBy::from_key`]）：frontmatter 是
            // 用户可以在外部编辑器里改的普通文本，认不出的字符串按**没说**处理——猜一个
            // 归属会把「上游机器上限」说成「你自己的额度设小了」，把人引去改一个改了也
            // 没用的设置。
            output_truncated: distilled
                .meta
                .distill_output_truncated
                .as_deref()
                .and_then(luna_core::TruncatedBy::from_key),
            // 模板与整理稿开关随状态一并读出（目标文件本就已读，零额外 IO）——
            // 重生成沿用它们，用户的显式选择不被规则建议覆盖。
            template: distilled.meta.distill_template.clone(),
            full_text: distilled.meta.distill_full_text,
        })
    }

    /// **frontmatter 局部改写的唯一咽喉**（§4.4）：读 → 就地改 `luna:` 块 → 写盘。
    ///
    /// `edit` 返回 `false` 表示这一篇无需改动——此时**不写盘**（避免无谓快照与重索引）。
    /// 返回落盘后的全文（`None` = 未写），供命令层广播给同文件的其它编辑器实例。
    ///
    /// 存在的理由是「改 frontmatter 的动作不止一个」：`luna.type` 改写与 `luna.stage`
    /// 去除各写一遍读-改-写，第二遍必然漏掉其中一步（round-trip 保护、无变化短路、快照）。
    pub(crate) fn rewrite_luna<F>(&self, rel: &str, edit: F) -> Result<Option<String>>
    where
        F: FnOnce(&mut LunaMeta) -> bool,
    {
        let doc = self.read(rel)?;
        let mut meta = doc.meta;
        if !edit(&mut meta) {
            return Ok(None);
        }
        let content = update_luna(&doc.content, &meta)?;
        self.write(rel, &content)?;
        Ok(Some(content))
    }

    /// 改写一篇笔记的 `luna.type`（速记「升级为笔记」用，PRD-02-055）。
    ///
    /// **同时去除显式 `luna.stage`**（PRD-06-053）：「升级为笔记」本身就是速记的**沉淀**
    /// 动作——type 改写已含毕业语义，若把 stage 留在原处，升级完的那一篇仍是记录态，
    /// 毕业就落了空（而用户看到的只是"升级了却还淡显着"）。故两根轴在同一次写盘里一起收。
    ///
    /// 走 frontmatter 局部精确改写（§4.4），正文与第三方键逐字节保留；写盘经
    /// [`Vault::write`] 故自动留快照，改错了可回滚。
    pub fn set_note_type(&self, rel: &str, note_type: crate::frontmatter::NoteType) -> Result<()> {
        self.rewrite_luna(rel, |meta| {
            let changed = meta.note_type != note_type || meta.stage_raw.is_some();
            meta.note_type = note_type;
            meta.stage_raw = None;
            changed
        })
        .map(|_| ())
    }

    /// 「沉淀为知识」（PRD-06-053）：去掉 `luna.stage` 这一个键，别的一律不动。
    ///
    /// 返回落盘后的全文；`None` = 这一篇本就没有该键（缺省记录态的速记走的是
    /// [`Vault::set_note_type`] 那条路——它的沉淀是「升级为笔记」，不是删一行）。
    ///
    /// **绝无自动档**：把草稿判成知识只有用户有资格。本方法只在用户显式触发的命令里
    /// 被调用，Vault 内部的任何自动路径都不得碰它（与「绝不静默覆盖用户资产」同一纪律）。
    pub fn promote_to_knowledge(&self, rel: &str) -> Result<Option<String>> {
        self.rewrite_luna(rel, |meta| {
            let changed = meta.stage_raw.is_some();
            meta.stage_raw = None;
            changed
        })
    }

    /// 「标记为记录」（PRD-06-058）：把一篇知识笔记降回记录态 = 加 `luna.stage` 一行。
    ///
    /// [`Self::promote_to_knowledge`] 的**反向动作**，走同一个 frontmatter 改写咽喉
    /// [`Self::rewrite_luna`]（round-trip §4.4、无变化不写盘、经 [`Self::write`] 留快照）。
    /// 返回落盘后的全文；`None` = 这一篇本来就是记录态，无需写盘。
    ///
    /// **成熟度必须双向可改**：Obsidian 社区的成熟度标签（#seedling/#evergreen）全是
    /// 双向的，单向门只会把用户逼去手改 frontmatter——而那正是这套动作要替他做的事。
    ///
    /// 三类拒绝（判定在此，前端只消费已判定的布尔）：
    /// - **资料笔记**：`stage` 在它上面被 [`NoteStage::resolve`] 忽略（来源轴优先、三态
    ///   互斥），写下去只得到一个谁都不读的键，界面却已答应了一件没发生的事；
    /// - **主题页 / 知识地图**：AI 维护的页面，其成熟度由所有权指纹（`luna.synth_body`，
    ///   ADR-026 ①）表达而**不走 stage**——把它标成记录只会让两套语义打架。
    pub fn mark_as_draft(&self, rel: &str) -> Result<Option<String>> {
        let meta = self.read(rel)?.meta;
        if meta.topic || meta.knowledge_map {
            return Err(VaultError::NotMarkableAsDraft(rel.to_owned()));
        }
        let stage = meta.stage();
        if stage.is_draft() {
            // 已经是记录态：重复动作是无操作，不是又一次写盘（免掉一条无谓快照）。
            return Ok(None);
        }
        if !stage.allows_mark_as_draft() {
            return Err(VaultError::NotMarkableAsDraft(rel.to_owned()));
        }
        self.rewrite_luna(rel, |meta| {
            meta.stage_raw = Some(crate::frontmatter::STAGE_DRAFT.to_owned());
            true
        })
    }

    /// 设 / 清字数目标（W6b，`luna.word_goal`）。返回改写后的全文；与现值相同为 `None`（不写盘、
    /// 不留快照）。走 `luna:` 块局部改写，正文与其它键逐字节不动。
    pub fn set_word_goal(&self, rel: &str, goal: Option<u32>) -> Result<Option<String>> {
        self.rewrite_luna(rel, |meta| {
            if meta.word_goal == goal {
                return false;
            }
            meta.word_goal = goal;
            true
        })
    }

    /// 批量蒸馏盘点（PRD-06-044 批量入口）：把范围内的**资料区**笔记按"要不要整理"分四桶。
    ///
    /// 只读不写，判据与 [`Vault::distill_state`] 同源——两边分家会让确认卡上的数字变成谎报。
    /// `scope = Some(文件夹)` 只盘点该文件夹（含子孙），`None` 为整库。
    ///
    /// **`edited` 单独成桶而不是并进 `fresh`**：它们被跳过的**理由完全不同**——一个是
    /// "已经是最新的、没必要重来"，一个是"你写过东西、我不敢动"。后者用户多半想知道
    /// （那几篇要不要自己去看看），把两者混成一个数字等于把这个信息抹掉。
    pub fn distill_scan(&self, scope: Option<&str>) -> Result<DistillScan> {
        if let Some(folder) = scope {
            Self::validate_scope_folder(folder)?;
        }
        let mut scan = DistillScan::default();
        for rel in self.walk_markdown()? {
            if let Some(folder) = scope {
                if !crate::graph::scope_covers(folder, &rel) {
                    continue;
                }
            }
            let Ok(doc) = self.read(&rel) else {
                continue; // 扫描期间被删/不可读：跳过，不让单篇故障拖垮整个盘点。
            };
            if !doc.meta.note_type.is_source_material() {
                continue; // 非资料区笔记根本不是蒸馏的对象，不计入任何桶。
            }
            if body_of(&doc.content).trim().is_empty() {
                scan.empty += 1;
                continue;
            }
            match self.distill_state(&rel)? {
                DistillState::Absent => scan.pending.push(rel),
                DistillState::Present { edited: true, .. } => scan.edited += 1,
                DistillState::Present { stale: true, .. } => scan.pending.push(rel),
                // **不完整产物属待办，不属「已最新」**：它的正文缺了一截（生成时输出
                // 触顶），落进 `fresh` 桶等于把它**永久排除**在提醒之外——界面写着
                // 「已是最新（跳过）」，而用户再也不会被提示去补那一篇。
                DistillState::Present {
                    output_truncated: Some(_),
                    ..
                } => scan.pending.push(rel),
                DistillState::Present { .. } => scan.fresh += 1,
            }
        }
        Ok(scan)
    }

    /// 落定一篇蒸馏笔记（PRD-06-044）：写正文 + 双向指针 + 两个指纹，返回其相对路径。
    ///
    /// `target` 为 `None` 时在 `notes_dir` 下新建（调用方给出唯一路径）；已存在则整体
    /// 覆盖正文——覆盖安全性由调用方经 [`Vault::distill_state`] 事先判定（未编辑过才自动
    /// 覆盖），本函数不自作主张。写盘走 [`Vault::write`]，故**每次都留快照**可回滚。
    ///
    /// `output_truncated` = 本次生成触顶的归属（[`luna_core::TruncatedBy::key`]），`None` =
    /// 没触顶。它随产物一起落进 frontmatter，理由见
    /// [`crate::frontmatter::LunaMeta::distill_output_truncated`]。
    pub fn write_distilled(
        &self,
        source_rel: &str,
        target_rel: &str,
        body: &str,
        template: &str,
        full_text: bool,
        output_truncated: Option<&str>,
    ) -> Result<()> {
        let source = self.read(source_rel)?;
        let source_hash = ContentHash::of(body_of(&source.content).as_bytes());

        // 蒸馏笔记恒为普通笔记（type: note）——它生来就是知识，全量参与图谱/标签/搜索。
        let mut meta = match self.read(target_rel) {
            Ok(existing) => existing.meta,
            Err(_) => LunaMeta {
                note_type: crate::frontmatter::NoteType::Note,
                ..LunaMeta::default()
            },
        };
        meta.note_type = crate::frontmatter::NoteType::Note;
        meta.distill_source = Some(source_rel.to_owned());
        meta.distill_src = Some(source_hash.as_str().to_owned());
        // 模板与整理稿开关必须在**第一趟渲染之前**设入：下面 `staged` 的渲染结果要
        // 拿去算 `distill_body` 自身指纹，事后再补写会让"设置 meta"散在两处而语义不明。
        // （二者只影响 frontmatter、不改 `body_of`，故指纹口径不受影响。）
        meta.distill_template = Some(template.to_owned());
        meta.distill_full_text = Some(full_text);
        // **无条件整体赋值**（含把 `None` 写回去）：重新整理成功、这一次没触顶时，
        // 上一版留下的截断标注必须**消失**。留着它会让一篇已经完整的笔记永远挂着
        // 「不完整」，于是补齐动作做了也像没做——而用户只会得出「这个提示是坏的」。
        meta.distill_output_truncated = output_truncated.map(str::to_owned);

        let skeleton = format!(
            "---\nluna:\n  schema: {}\n---\n\n{body}",
            crate::LUNA_SCHEMA
        );
        let staged = update_luna(&skeleton, &meta)?;
        // 自身指纹必须对 **`body_of(最终文件)`** 计算，而不是对传入的 `body` 字符串——
        // 二者差着 frontmatter 与其后的空行，口径分家会让**刚生成的**蒸馏笔记当场被判成
        // 「用户编辑过」，于是自动更新永远不生效而界面永远显示「源已更新」。
        // 写 `distill_body` 只动 frontmatter，不改 `body_of` 的结果，故可安全二次渲染。
        meta.distill_body = Some(
            ContentHash::of(body_of(&staged).as_bytes())
                .as_str()
                .to_owned(),
        );
        let content = update_luna(&staged, &meta)?;
        self.write(target_rel, &content)?;

        // 反向指针写回源笔记（写在蒸馏笔记落定**之后**：先有实体再有指针，中途失败
        // 只会留下一篇没被指向的笔记，而不是一个指向虚空的指针）。
        let mut source_meta = source.meta;
        if source_meta.distill_note.as_deref() != Some(target_rel) {
            source_meta.distill_note = Some(target_rel.to_owned());
            let updated = update_luna(&source.content, &source_meta)?;
            self.write(source_rel, &updated)?;
        }
        Ok(())
    }

    /// 写入一篇**主题页**（PRD-16-015 / ADR-026 ①）：落盘并把 `luna.synth_body` 更新为
    /// 新正文的指纹，于是这一页回到「AI 所有」态。
    ///
    /// 两趟渲染的理由与 [`Vault::write_distilled`] 完全一致（那里已踩过一次）：自身指纹
    /// 必须对 `body_of(最终文件)` 计算，对传入的 `body` 算会差着 frontmatter 与其后的空行，
    /// 口径分家会让**刚写完的**主题页当场被判成「用户已接管」。
    ///
    /// `topic_src` 只在**首次创建**时设入——它记的是「这一页从哪个标签簇长出来的」这一
    /// 历史事实，后续归入不该改写它（同「删模板不回改既有笔记」的判据）。
    pub fn write_topic(&self, rel: &str, body: &str, topic_src: Option<&str>) -> Result<()> {
        let content = self.topic_content(rel, body, topic_src)?;
        self.write(rel, &content)
    }

    /// 一张主题页**落盘前的完整文件内容**（渲染，不写盘）。
    ///
    /// 拆出来的唯一理由：汇总发布路径要先把最终内容准备好，再用 [`Vault::publish_new`]
    /// 原子地占住落点——渲染与写盘必须分成两步。**判定仍只有这一份**（`write_topic` 就是
    /// 「本函数 + `write`」），绝不允许发布路径自己再拼一遍 frontmatter：那就是同一套
    /// 所有权指纹口径的第二处实现，而口径分家会让刚写完的主题页当场被判成「用户已接管」。
    pub fn topic_content(&self, rel: &str, body: &str, topic_src: Option<&str>) -> Result<String> {
        let mut meta = match self.read(rel) {
            Ok(existing) => {
                let checked = crate::frontmatter::read_luna_checked(&existing.content)?;
                if checked.knowledge_map {
                    return Err(VaultError::InvalidReplace(
                        "知识地图不能用主题写入覆盖".into(),
                    ));
                }
                checked
            }
            Err(VaultError::NotFound(_)) => LunaMeta {
                note_type: NoteType::Note,
                ..LunaMeta::default()
            },
            Err(error) => return Err(error),
        };
        if meta.knowledge.is_some() {
            return Err(VaultError::InvalidReplace(
                "新版主题声明须走逐条证据计划，不能用旧版整页写入".into(),
            ));
        }
        meta.note_type = NoteType::Note;
        meta.topic = true;
        if meta.topic_src.is_none() {
            meta.topic_src = topic_src.map(str::to_owned);
        }
        Self::render_topic_content(body, &mut meta)
    }

    /// 只为**全新**主题渲染证据格式；调用方随后用 `publish_new` 原子占位。
    /// 旧主题升级必须走独立的预览/备份流程，不能靠此入口顺手覆盖。
    pub fn scoped_topic_content(
        &self,
        rel: &str,
        topic_src: Option<&str>,
        scope: crate::TopicScope,
        claims: &[crate::knowledge_evidence::NewClaim<'_>],
    ) -> Result<String> {
        self.resolve(rel)?;
        if !self.path_is_free(rel) {
            return Err(VaultError::AlreadyExists(rel.into()));
        }
        // 生成可能耗时数分钟；渲染前重读 Vault 正文版本，不能把生成开始时的
        // 旧 hash 和此刻已变的来源拼成一篇看似有据的新主题。正式发布入口仍须
        // 在写盘时再次校验，不能把此处的检查当成跨文件事务。
        for source in claims.iter().flat_map(|claim| claim.sources.iter()) {
            let doc = self.read(source.path)?;
            let current = ContentHash::of(body_of(&doc.content).as_bytes());
            if current.as_str() != source.source_hash {
                return Err(VaultError::InvalidReplace(format!(
                    "source changed before topic publish: {}",
                    source.path
                )));
            }
        }
        let (body, knowledge) = crate::knowledge_evidence::render_scoped_topic(rel, scope, claims)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        let mut meta = LunaMeta {
            note_type: NoteType::Note,
            topic: true,
            topic_src: topic_src.map(str::to_owned),
            knowledge: Some(
                serde_yaml_ng::to_value(&knowledge)
                    .map_err(|error| VaultError::Decode(error.to_string()))?,
            ),
            ..LunaMeta::default()
        };
        let content = Self::render_topic_content(&body, &mut meta)?;
        crate::validate_topic_document(&content)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        Ok(content)
    }

    /// 只读投影：从主题 Markdown 恢复每条出处，并与此刻的来源文件比较。
    /// 旧格式/坏格式不伪装为“没有出处”，而是明确返回格式错误。
    pub fn topic_evidence_states(
        &self,
        topic_rel: &str,
    ) -> Result<Vec<crate::knowledge_evidence::CitationEvidence>> {
        let topic = self.read(topic_rel)?;
        self.evidence_states_of(topic_rel, &topic.content)
    }

    /// 后台来源版本事件按稳定键分页定位受影响声明；结果仅是可重建的索引投影。
    pub fn topic_citation_dependents(
        &self,
        source_path: &str,
        after: Option<(&str, &str)>,
        limit: u32,
    ) -> Result<Vec<crate::index::TopicCitationDependency>> {
        self.index
            .topic_citation_dependents(source_path, after, limit)
    }

    /// 按主题文件 revision 分页；仅为当页出处读取来源，避免打开长主题时逐一读全库。
    pub fn topic_evidence_page(
        &self,
        topic_rel: &str,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
    ) -> Result<crate::knowledge_evidence::CitationEvidencePage> {
        let topic = self.read(topic_rel)?;
        let revision = ContentHash::of(topic.content.as_bytes()).to_string();
        if expected_revision.is_some_and(|expected| expected != revision) {
            return Err(VaultError::InvalidReplace(
                "topic changed during evidence pagination".into(),
            ));
        }
        let citations = crate::knowledge_evidence::topic_citations(topic_rel, &topic.content)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(citations.len());
        let end = start
            .saturating_add(limit.clamp(1, 100) as usize)
            .min(citations.len());
        let total = u32::try_from(citations.len()).unwrap_or(u32::MAX);
        let next_offset = (end < citations.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX));
        let items = citations[start..end]
            .iter()
            .cloned()
            .map(|citation| self.citation_evidence_of(citation))
            .collect::<Result<Vec<_>>>()?;
        Ok(crate::knowledge_evidence::CitationEvidencePage {
            revision,
            total,
            items,
            next_offset,
        })
    }

    pub(crate) fn evidence_states_of(
        &self,
        topic_rel: &str,
        content: &str,
    ) -> Result<Vec<crate::knowledge_evidence::CitationEvidence>> {
        let citations = crate::knowledge_evidence::topic_citations(topic_rel, content)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        citations
            .into_iter()
            .map(|citation| self.citation_evidence_of(citation))
            .collect()
    }

    fn citation_evidence_of(
        &self,
        citation: crate::knowledge_evidence::TopicCitation,
    ) -> Result<crate::knowledge_evidence::CitationEvidence> {
        use crate::knowledge_evidence::{CitationEvidence, EvidenceState};
        let state = match citation.source_path.as_deref() {
            None => EvidenceState::Ambiguous,
            Some(source_path) => match self.read(source_path) {
                Err(VaultError::NotFound(_)) => EvidenceState::Missing,
                Err(error) => return Err(error),
                Ok(source) => {
                    let body = body_of(&source.content);
                    let hash = ContentHash::of(body.as_bytes());
                    if hash.as_str() != citation.meta.source_hash {
                        EvidenceState::SourceChanged
                    } else if !crate::knowledge_evidence::citation_quote_is_unique_or_hinted(
                        &source.content,
                        &citation.meta.locator,
                    ) {
                        EvidenceState::Ambiguous
                    } else if !citation.meta.complete_input {
                        EvidenceState::Partial
                    } else {
                        EvidenceState::Current
                    }
                }
            },
        };
        Ok(CitationEvidence {
            id: citation.id,
            source_path: citation.source_path,
            state,
        })
    }

    /// 新证据主题的发布边界：比渲染更晚再次核对来源，并沿用原子占位。
    pub fn publish_scoped_topic(&self, rel: &str, content: &str) -> Result<()> {
        use crate::knowledge_evidence::EvidenceState;
        let evidence = self.evidence_states_of(rel, content)?;
        if evidence
            .iter()
            .any(|entry| !matches!(entry.state, EvidenceState::Current | EvidenceState::Partial))
        {
            return Err(VaultError::InvalidReplace(
                "topic evidence changed before publish".into(),
            ));
        }
        self.publish_new(rel, content)?;
        // 首版也要有可核验的系统底版；否则“正文未动、scope 被改”无从识别。
        // 与普通保存的快照口径一致：发布已经成功后快照失败不能倒报为发布失败，
        // 维护闸门会因找不到底版而保守停下。
        let _ = self.record_topic_baseline(rel, content);
        Ok(())
    }

    /// 新格式主题的系统底版快照；调用方须在确认文件已经落盘后调用。
    /// 写盘成功后快照失败只会让维护闸门停下，不得倒报成“文件没有写入”。
    pub(crate) fn record_topic_baseline(&self, rel: &str, content: &str) -> bool {
        self.snapshots
            .snapshot_system_baseline(&normalize_rel(rel), content.as_bytes())
            .is_ok()
    }

    /// 系统最近一次写入这一页的时刻（最近系统底版的时间，Unix 毫秒）；没有底版为 `None`。
    /// 知识地图与主题目录的「最近更新」都读它（PRD-16-050 / PRD-16-061）。
    #[must_use]
    pub fn topic_updated_ms(&self, rel: &str) -> Option<u64> {
        self.latest_topic_baseline_meta(rel)
            .map(|meta| meta.created_ms)
    }

    pub(crate) fn latest_topic_baseline_meta(&self, rel: &str) -> Option<SnapshotMeta> {
        self.snapshots
            .latest_system_baseline_meta(&normalize_rel(rel))
    }

    /// 版本链里正文指纹为 `hash` 的最近一版（撤销用：写前版本在写入那一刻已留快照）。
    /// blob 读不出、指纹对不上或不是 UTF-8 一律 `None`。
    pub(crate) fn snapshot_text_with_hash(&self, rel: &str, hash: &str) -> Option<String> {
        let rel = normalize_rel(rel);
        let meta = self
            .snapshots
            .list(&rel)
            .into_iter()
            .rev()
            .find(|meta| meta.hash == hash)?;
        let bytes = self.snapshots.read_snapshot(&rel, &meta.id).ok()?;
        if ContentHash::of(&bytes).as_str() != hash {
            return None;
        }
        String::from_utf8(bytes).ok()
    }

    /// 文件已与封存计划结果一致时，仅补索引和底版；不得再次改写正文。
    pub(crate) fn recover_committed_topic(&self, rel: &str, content: &str) -> (bool, bool) {
        let index_updated = self.resolve(rel).is_ok_and(|abs| {
            self.index
                .index_note(&normalize_rel(rel), content, file_mtime_ms(&abs))
                .is_ok()
        });
        let baseline_recorded = self.record_topic_baseline(rel, content);
        (index_updated, baseline_recorded)
    }

    /// 正文指纹为 `body_hash` 的最近一份系统底版（从新到旧找，只认系统底版身份的快照）。
    ///
    /// 不只看最新那份：用户在版本历史里回滚到系统写过的某一版（例如撤销入口已不在时的
    /// 退路），页面字节就是系统当时写下的内容，仍可放心维护；只认最新一份会把这一页
    /// 永久卡在「找不到系统底版」（2026-09-27 真机）。普通历史版本即使正文相同也不算数。
    fn system_baseline_with_body(&self, rel: &str, body_hash: &str) -> Option<String> {
        if body_hash.is_empty() {
            return None;
        }
        let rel = normalize_rel(rel);
        self.snapshots
            .list(&rel)
            .into_iter()
            .rev()
            .filter(|meta| meta.system_baseline)
            .find_map(|meta| {
                let bytes = self.snapshots.read_snapshot(&rel, &meta.id).ok()?;
                if ContentHash::of(&bytes).as_str() != meta.hash {
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                (ContentHash::of(body_of(&text).as_bytes()).as_str() == body_hash).then_some(text)
            })
    }

    /// 自动维护新格式主题之前的只读闸门；不改写、不重登记用户的元数据。
    pub fn topic_maintenance_state(
        &self,
        rel: &str,
    ) -> Result<crate::knowledge_evidence::TopicMaintenanceState> {
        use crate::knowledge_evidence::TopicMaintenanceState as State;
        let doc = self.read(rel)?;
        if !doc.meta.topic || doc.meta.knowledge_map {
            return Ok(State::NotTopic);
        }
        let current = match crate::validate_topic_document(&doc.content) {
            Ok(knowledge) => knowledge,
            Err(crate::KnowledgeError::NeedsUpgrade) => return Ok(State::NeedsUpgrade),
            Err(crate::KnowledgeError::UnknownSchema(_)) => return Ok(State::UnknownSchema),
            Err(_) => return Ok(State::InvalidEvidence),
        };
        if ownership_of(&doc.content) != TopicOwnership::AiOwned {
            return Ok(State::BodyChanged);
        }
        let Some(baseline) =
            self.system_baseline_with_body(rel, doc.meta.synth_body.as_deref().unwrap_or_default())
        else {
            return Ok(State::MissingBaseline);
        };
        let Ok(original) = crate::validate_topic_document(&baseline) else {
            return Ok(State::InvalidEvidence);
        };
        if current != original {
            return Ok(State::MetadataChanged);
        }
        Ok(State::Ready)
    }

    fn render_topic_content(body: &str, meta: &mut LunaMeta) -> Result<String> {
        let skeleton = format!(
            "---\nluna:\n  schema: {}\n---\n\n{body}",
            crate::LUNA_SCHEMA
        );
        let staged = update_luna(&skeleton, meta)?;
        meta.synth_body = Some(
            ContentHash::of(body_of(&staged).as_bytes())
                .as_str()
                .to_owned(),
        );
        update_luna(&staged, meta)
    }

    /// 从此刻全部主题页确定性生成知识地图正文。地图本身不进入输入集合。
    pub fn knowledge_map_body(&self, map_rel: &str, zh: bool) -> Result<String> {
        let pages = self.topic_pages()?;
        let docs = pages
            .iter()
            .map(|path| self.read(path))
            .collect::<Result<Vec<_>>>()?;
        let topics = docs
            .iter()
            .map(|doc| crate::KnowledgeMapTopic {
                path: &doc.path,
                content: &doc.content,
                updated_ms: self.topic_updated_ms(&doc.path),
            })
            .collect::<Vec<_>>();
        Ok(crate::render_knowledge_map(
            map_rel,
            &topics,
            crate::MapLabels {
                heading: if zh { "知识地图" } else { "Knowledge Map" },
                empty: if zh {
                    "还没有主题页。"
                } else {
                    "There are no topic pages yet."
                },
                other: if zh { "其他主题" } else { "Other topics" },
                recent: if zh {
                    "最近更新"
                } else {
                    "Recently updated"
                },
                related: if zh { "相关：" } else { "Related: " },
            },
        ))
    }

    /// 写入/刷新知识地图并登记所有权指纹。调用方负责唯一性与用户所有权判定。
    pub fn write_knowledge_map(&self, rel: &str, body: &str) -> Result<()> {
        let mut meta = match self.read(rel) {
            Ok(existing) => existing.meta,
            Err(_) => LunaMeta {
                note_type: NoteType::Note,
                ..LunaMeta::default()
            },
        };
        meta.note_type = NoteType::Note;
        meta.knowledge_map = true;
        meta.topic = false;
        meta.topic_src = None;
        // 地图不携带 AI 关系标签（PRD-16-023 修订）：它链接**所有**主题页，标签只会
        // 把「库级词汇表」的计数灌水。这里顺带清掉存量——重扫豁免之前误打上的标签没有
        // 别的收敛路径（重扫此后不碰它），而 `luna.ai_tags` 本就是 Lunarca 写的派生元数据，
        // 不是用户资产；下一次地图维护写回即自然消失。
        meta.ai_tags = Vec::new();
        meta.ai_tags_src = None;
        let skeleton = format!(
            "---\nluna:\n  schema: {}\n---\n\n{body}",
            crate::LUNA_SCHEMA
        );
        let staged = update_luna(&skeleton, &meta)?;
        meta.synth_body = Some(ContentHash::of(body_of(&staged).as_bytes()).to_string());
        self.write(rel, &update_luna(&staged, &meta)?)
    }

    /// 一页 AI 维护笔记的所有权（ADR-026 ①）。读不到即 `NotTopic`（不存在的页不属于任何人）。
    #[must_use]
    pub fn topic_ownership(&self, rel: &str) -> TopicOwnership {
        self.read(rel)
            .map(|doc| ownership_of(&doc.content))
            .unwrap_or(TopicOwnership::NotTopic)
    }

    /// 把产物页**此刻的正文重新登记为系统基线**（复核态「保留我的改动」在找不到底版
    /// 快照时的收口路径，ADR-096 ④；改动本身
    /// 已由调用方先另存成用户笔记，故以当前内容为新基线不制造第二份事实）。
    ///
    /// **不另造任何状态位**：判据只有 `luna.synth_body` 一行指纹（PRD-16-021），
    /// 本函数就是反向写它。造 `reviewed: true` 之类的标记会立刻产生第二套事实源。
    ///
    /// **改的只有 frontmatter 里那一行指纹，正文一个字节都不动**——故走 §4.4 的局部精确
    /// 改写（`update_luna`）而**不是** [`Vault::write_topic`]：后者是为"写正文顺带登记指纹"
    /// 造的，它会用骨架重排正文前的空行（守护测试逐字节比对时当场抓到——登记一次多一个
    /// 空行，登记三次多三个）。不写正文，就不该走写正文那条路。
    ///
    /// 写盘经 `write` 故**自动留版本快照**，错了可回滚。
    ///
    /// 拒绝两种情形：不是产物页（无所谓基线）、此刻并未被人工改动（无事可做——如实说
    /// 清楚，而不是静默成功让用户以为刚才那一下起了别的作用）。返回登记后的完整内容。
    pub fn reregister_product_baseline(&self, rel: &str) -> Result<String> {
        let doc = self.read(rel)?;
        match ownership_of(&doc.content) {
            TopicOwnership::UserOwned => {}
            TopicOwnership::AiOwned => {
                return Err(VaultError::InvalidReplace(
                    "这一页此刻并未被人工改动".into(),
                ))
            }
            TopicOwnership::NotTopic => {
                return Err(VaultError::InvalidReplace(
                    "这不是一篇由 Lunarca 维护的页面".into(),
                ))
            }
        }
        let mut meta = doc.meta.clone();
        // 指纹口径与 `write_topic` 逐字同源（`ContentHash` over `body_of` 的字节）：分家会让
        // 刚登记的页当场又被判成「被人工改动」。frontmatter 的局部改写不动 `body_of` 的结果，
        // 故这里一趟即精确，不必像 `write_topic` 那样为"先渲染再算自身指纹"跑两趟。
        meta.synth_body = Some(
            ContentHash::of(body_of(&doc.content).as_bytes())
                .as_str()
                .to_owned(),
        );
        let content = update_luna(&doc.content, &meta)?;
        self.write(rel, &content)?;
        Ok(self.read(rel)?.content)
    }

    /// 找回产物页的**系统底版**：最近一个「正文指纹 == 此刻 `luna.synth_body`」的版本
    /// 快照内容（复核态「舍弃改动」的恢复来源，ADR-096 ④）。
    ///
    /// 指纹口径与 [`Self::reregister_product_baseline`] 同一条（`ContentHash` over
    /// `body_of`）；`None` = 快照链里已找不到系统写入的那一版（被容量上限挤出等），
    /// 调用方如实拒绝「舍弃」并把用户引向「保留我的改动」——那条路恒可走。
    pub fn product_baseline_snapshot(&self, rel: &str) -> Result<Option<String>> {
        let doc = self.read(rel)?;
        let Some(expect) = doc.meta.synth_body.clone() else {
            return Ok(None);
        };
        for meta in self.snapshots(rel) {
            let Ok(content) = self.read_snapshot(rel, &meta.id) else {
                continue; // 单个快照读不出不拖垮整条链。
            };
            if ContentHash::of(body_of(&content).as_bytes()).as_str() == expect {
                return Ok(Some(content));
            }
        }
        Ok(None)
    }

    /// 主题页建议（PRD-16-016 aha 时刻）：**纯 Rust 零 LLM**。
    ///
    /// 判据：某标签下聚集 ≥ [`TOPIC_SUGGESTION_MIN_NOTES`] 篇**知识态**笔记，且尚无主题页
    /// 以它为出身（`luna.topic_src`），且不在忽略清单里。
    ///
    /// 只数**知识态**（非资料**且**非记录，ADR-086）：资料区的录音/转写/对话本就该先蒸馏
    /// 再谈归入，把它们数进来会让建议在「一堆还没整理的转写」上疯狂触发；而记录态同理
    /// 不计入也不进候选（PRD-16-016 修订）——零散速记凑满一个标签不该触发合成建议，
    /// 那正是二分模型的病灶之一。判定不在这里，走 `notes.stage` 列（`knowledge_only`）。
    pub fn topic_suggestions(&self, ignored: &[String]) -> Result<Vec<TopicSuggestion>> {
        let mut existing: std::collections::HashSet<String> = self
            .index
            .topic_sources()?
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect();
        existing.extend(self.scope_covered_tags()?.into_keys());

        let mut out = Vec::new();
        for tag in self.index.list_tags()? {
            if out.len() >= TOPIC_SUGGESTION_LIMIT {
                break;
            }
            if existing.contains(&tag.tag.to_lowercase())
                || ignored.iter().any(|i| i.eq_ignore_ascii_case(&tag.tag))
            {
                continue;
            }
            let notes = self.tag_knowledge_notes(&tag.tag)?;
            if notes.len() < TOPIC_SUGGESTION_MIN_NOTES {
                continue;
            }
            let note_count = self.tag_knowledge_note_count(&tag.tag)?;
            out.push(TopicSuggestion {
                tag: tag.tag,
                notes,
                note_count,
            });
        }
        // 篇数多的簇更值得先建页（信号更强），同数按标签名稳定排序。
        out.sort_by(|a, b| {
            b.note_count
                .cmp(&a.note_count)
                .then_with(|| a.tag.cmp(&b.tag))
        });
        Ok(out)
    }

    /// 一条标签下的**知识笔记**（PRD-16-016 的「≥3 篇」用的就是这一份）。
    ///
    /// 「知识笔记」的判据只有一份：[`crate::is_ingest_candidate`]（非资料 ∧ 非主题页/
    /// 知识地图 ∧ 非记录态）。零散速记凑满一个标签不该触发合成建议，聚合页也不该被当成
    /// 源笔记。上限取门槛的四倍：判据只问"够不够 3 篇"，而合成初稿也不该一次吞下整个标签。
    ///
    /// 它曾是旧版入队合成真正吃进去的源集，漏一条轴的代价直接写进了用户的 Vault（真机
    /// 2026-08-31，产物页进了另一个产物页的「来源」区）。新建主题页改走证据格式后，来源由
    /// 候选池另取；这一份仍决定「够不够格给建议」，漏掉产物轴同样会让聚合页替自己凑数。
    ///
    /// **主题页建议与「标签分区的动作分流」（PRD-06-033）问的是同一个问题**，故共用
    /// 本函数：一边算得进、另一边算不进的表现是标签页上摆着「合成主题页」而建议那一侧
    /// 说这个标签不够格（或反过来），两处都"看起来对"。
    pub fn tag_knowledge_notes(&self, tag: &str) -> Result<Vec<String>> {
        self.tag_knowledge_paths(tag, TOPIC_SUGGESTION_MIN_NOTES * 4)
    }

    /// 这条标签下知识笔记的**精确**篇数（PRD-16-051「计数注明统计对象」）：与
    /// [`Self::tag_knowledge_notes`] 同一判据，只是不截在门槛的四倍——建议文案要说的是
    /// 「有几篇」，截断后的清单长度会把 30 篇说成 12 篇。
    pub fn tag_knowledge_note_count(&self, tag: &str) -> Result<u32> {
        const COUNT_CEILING: usize = 1_000_000;
        Ok(u32::try_from(self.tag_knowledge_paths(tag, COUNT_CEILING)?.len()).unwrap_or(u32::MAX))
    }

    fn tag_knowledge_paths(&self, tag: &str, limit: usize) -> Result<Vec<String>> {
        Ok(self
            .index
            .search(&Query {
                tag: Some(tag.to_owned()),
                // **一格开关给全四条轴**（`ingest_candidates_only`），不是
                // `note_type` + `knowledge_only` 各设一半。真机 2026-08-31：这里此前正是
                // 那两格，漏了产物轴——三个主题页在索引里都是 `type='note', is_topic=1`，
                // 于是它们通过了这道筛，被当成源笔记喂进合成，`Knowledge/计划.md` 的
                // 「来源」里因此列着 `Datadog.md` 与 `I2X.md`：AI 产物回流进 AI 产物。
                ingest_candidates_only: true,
                limit,
                ..Query::default()
            })?
            .into_iter()
            .map(|hit| hit.path)
            .collect())
    }

    /// 带这条标签、已完成权威转写的录音/转写（新建主题初稿在本设备授权时的另一类来源，
    /// PRD-16-047）。是否授权由调用方决定；这里只回答「有哪些」。
    pub fn tag_transcript_sources(&self, tag: &str) -> Result<Vec<String>> {
        let mut paths = Vec::new();
        for note_type in [NoteType::Recording, NoteType::Transcript] {
            paths.extend(
                self.index
                    .search(&Query {
                        tag: Some(tag.to_owned()),
                        note_type: Some(note_type),
                        limit: TOPIC_SUGGESTION_MIN_NOTES * 4,
                        ..Query::default()
                    })?
                    .into_iter()
                    .map(|hit| hit.path),
            );
        }
        paths.sort();
        paths.dedup();
        paths.retain(|path| {
            self.read(path).is_ok_and(|doc| {
                crate::knowledge_generation::has_authoritative_transcript(&doc.content)
            })
        });
        Ok(paths)
    }

    /// 以这条标签为**出身**的主题页（`luna.topic_src`）；`None` = 还没有。
    ///
    /// 与 [`Self::topic_suggestions`] 的「尚无主题页」是同一个判据的两个问法（有没有 /
    /// 是哪一页），故都读 `topic_src` 这一列，不靠标题或路径去猜。
    pub fn topic_page_for_tag(&self, tag: &str) -> Result<Option<String>> {
        if let Some(page) = self.index.topic_page_for_tag(tag)? {
            return Ok(Some(page));
        }
        Ok(self.scope_covered_tags()?.remove(&tag.to_lowercase()))
    }

    /// 被证据格式页**声明范围**收进来的标签（小写）→ 那一页（ADR-134「候选页覆盖优先于
    /// 新建」）：一页的范围已经包含这个标签，就不该再建议为它另建一页，标签页的「创建主题页」
    /// 也改为指向那一页。出身标签那一路由索引回答，这里只补范围。读不出的页跳过。
    fn scope_covered_tags(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut covered = std::collections::HashMap::new();
        for page in self.index.topic_pages()? {
            let Ok(doc) = self.read(&page) else {
                continue;
            };
            if let Ok(knowledge) = crate::validate_topic_document(&doc.content) {
                for tag in &knowledge.scope.includes {
                    covered
                        .entry(tag.to_lowercase())
                        .or_insert_with(|| page.clone());
                }
            }
        }
        Ok(covered)
    }

    /// 这一页**还没收进来**的知识笔记（PRD-16-021 落地注④ 的反方向候选，共享标签那一路）。
    ///
    /// 判据两句话：① 与本页共享标签的知识笔记（SQL，见 [`crate::Index::topic_tag_candidates`]）；
    /// ② 正文里还没有链回它（[`crate::cites_source`]——已经被引用过的，要点早就在页内了）。
    ///
    /// **「主题页目录的待归入数」与「立即归入」的第一段调的是同一个函数**（TDD-07 §4.1d）：
    /// 共享标签那一路只允许有一个定义，各写一遍就会在同一页上给出两个数。
    ///
    /// **但它是一个下界，不是「立即归入」会排上的篇数**：后者还会并上检索那一路
    ///（PRD-16-021 落地注④ 的「共享标签 + 检索」），而检索属于执行栈——在目录这条列表
    /// 查询里对每一页跑一遍正是 ADR-088 ③ 禁的。故计数**只门控徽标的显隐**，行动作恒可达；
    /// 严禁反过来用它去禁用「立即归入」（那会让检索才找得到的那几篇永远进不来）。
    ///
    /// 返回**全部**候选而不截断——一次排队的上限（`MAX_INGEST_NOW_SOURCES`）是队列纪律，
    /// 不是「有几篇待归入」的答案；按上限截断会让「待归入 3」在 12 篇待归入时同样显示 3。
    pub fn topic_ingest_pending(&self, topic_rel: &str) -> Result<Vec<String>> {
        let doc = self.read(topic_rel)?;
        self.topic_ingest_pending_of(topic_rel, &doc.content)
    }

    /// 同上，但用调用方**手上已有的那份正文**。
    ///
    /// 不只是省一次读盘：目录那一行的摘要、所有权与待归入数因此描述**同一份字节**。
    /// 各读各的时，两次读之间的一次保存会让同一行里"这一页写了什么"和"它还差几篇"
    /// 来自两个版本，而界面上看不出这种错位。
    pub fn topic_ingest_pending_of(&self, topic_rel: &str, content: &str) -> Result<Vec<String>> {
        if !crate::ownership_of(content).is_topic() {
            return Ok(Vec::new());
        }
        // 正文只解析一次，再拿同一份链接集合去问每一条候选：`cites_source` 是"每问一次就
        // 重新解析整篇正文"，候选有 C 条就把同一篇正文扫 C 遍。判定规则没有变，两条路都走
        // `cites_any_of`。
        let cited = crate::cited_targets(crate::body_of(content));
        // 证据格式页按声明的范围取；旧格式页没有范围，仍按页面标签。
        let matched = match crate::validate_topic_document(content) {
            Ok(knowledge) => self
                .index
                .scope_tag_candidates(topic_rel, &knowledge.scope)?,
            Err(_) => self.index.topic_tag_candidates(topic_rel)?,
        };
        let renewed = self.cited_sources_with_new_versions(topic_rel, content, &matched);
        // 排除路径（V4a）里的笔记不当新候选；本页已经引用过的照旧跟踪新版本——那是用户点过名的。
        let excluded = self.exclusions();
        Ok(matched
            .into_iter()
            .filter(|path| !crate::cites_any_of(&cited, path) || renewed.contains(path))
            .filter(|path| !excluded.covers(path) || crate::cites_any_of(&cited, path))
            .collect())
    }

    /// 本页引用过、但当前正文已不是本页引用的任何一个版本的来源（K4「已有引用不屏蔽新版本」）：
    /// 来源改了之后仍是候选，新版本交给模型看；旧声明照旧标「来源已变」待复核，不被替换。
    /// 只看证据格式页（旧格式页没有逐条出处可比）。
    fn cited_sources_with_new_versions(
        &self,
        topic_rel: &str,
        content: &str,
        matched: &[String],
    ) -> std::collections::HashSet<String> {
        let Ok(citations) = crate::knowledge_evidence::topic_citations(topic_rel, content) else {
            return std::collections::HashSet::new();
        };
        let mut cited_versions: std::collections::HashMap<
            String,
            std::collections::HashSet<String>,
        > = std::collections::HashMap::new();
        for citation in citations {
            if let Some(path) = citation.source_path {
                cited_versions
                    .entry(path)
                    .or_default()
                    .insert(citation.meta.source_hash);
            }
        }
        matched
            .iter()
            .filter(|path| {
                cited_versions.get(path.as_str()).is_some_and(|versions| {
                    self.body_fingerprint(path)
                        .is_some_and(|current| !versions.contains(&current))
                })
            })
            .cloned()
            .collect()
    }

    /// 与主题页共享标签、已完成权威转写且尚未被本页引用的录音/转写（PRD-16-047）。
    /// 不看本设备是否授权：未授权时它只用来告诉用户「还有这些可以作为来源」。
    pub fn topic_transcript_candidates_of(
        &self,
        topic_rel: &str,
        content: &str,
    ) -> Result<Vec<String>> {
        if !crate::ownership_of(content).is_topic() {
            return Ok(Vec::new());
        }
        let cited = crate::cited_targets(crate::body_of(content));
        let matched = match crate::validate_topic_document(content) {
            Ok(knowledge) => self
                .index
                .scope_tag_source_material(topic_rel, &knowledge.scope)?,
            Err(_) => self.index.topic_tag_source_material(topic_rel)?,
        };
        let renewed = self.cited_sources_with_new_versions(topic_rel, content, &matched);
        Ok(matched
            .into_iter()
            .filter(|path| !crate::cites_any_of(&cited, path) || renewed.contains(path))
            .filter(|path| {
                self.read(path).is_ok_and(|doc| {
                    crate::knowledge_generation::has_authoritative_transcript(&doc.content)
                })
            })
            .collect())
    }

    /// 证据格式主题页按范围可用的速记（ADR-136 ①）：尚未被本页引用、或引用后又有新版本的，
    /// 且正文非空。旧格式页没有声明范围，不收速记（它们经显式升级后才进入增量计划）。
    pub fn topic_quick_note_candidates_of(
        &self,
        topic_rel: &str,
        content: &str,
    ) -> Result<Vec<String>> {
        let Ok(knowledge) = crate::validate_topic_document(content) else {
            return Ok(Vec::new());
        };
        let cited = crate::cited_targets(crate::body_of(content));
        let matched = self
            .index
            .scope_tag_quick_notes(topic_rel, &knowledge.scope)?;
        let renewed = self.cited_sources_with_new_versions(topic_rel, content, &matched);
        Ok(matched
            .into_iter()
            .filter(|path| !crate::cites_any_of(&cited, path) || renewed.contains(path))
            .filter(|path| {
                self.read(path)
                    .is_ok_and(|doc| !crate::body_of(&doc.content).trim().is_empty())
            })
            .collect())
    }

    /// 库里全部主题页的路径（[`crate::Index::topic_pages`] 的直通口）。
    pub fn topic_pages(&self) -> Result<Vec<String>> {
        self.index.topic_pages()
    }

    /// 某时刻以来收进来的来源（PRD-16-067），见 [`crate::Index::sources_created_since`]。
    pub fn sources_created_since(&self, since_ms: i64) -> Result<Vec<crate::RecentSource>> {
        self.index.sources_created_since(since_ms)
    }

    pub fn knowledge_maps(&self) -> Result<Vec<String>> {
        self.index.knowledge_maps()
    }

    /// 取一篇「多条 → 一篇」的源（PRD-06-047 / TDD-07 §2.1）。
    ///
    /// 一次读取里把四件事一起定死：**路径安全**（含符号链接逃逸）、**是不是 Markdown**、
    /// **喂给模型的那段字**（按来源类型选口径）、以及**发起那一刻的指纹**。
    ///
    /// **调用方拿到的要么是一个完整的源，要么是一个具体的原因**——没有"读到一半"的
    /// 中间态。三个汇总入口的差别只在**候选集政策**（整体拒绝还是跳过），而不在"这一篇
    /// 到底算什么"；后者各写一遍就会出现"同一篇笔记从不同入口喂进去的字不一样"。
    pub fn merge_source(
        &self,
        rel: &str,
    ) -> std::result::Result<crate::MergeSource, crate::MergeSourceError> {
        use crate::MergeSourceError as E;
        if !rel.to_lowercase().ends_with(".md") {
            return Err(E::NotMarkdown);
        }
        let opened = self.open_in_vault(rel).map_err(|error| match error {
            crate::VaultOpenError::PathEscape => E::PathEscape,
            crate::VaultOpenError::Unreadable => E::Unreadable,
        })?;
        let meta = read_luna(&opened.content);
        crate::merge::source_from_doc(&opened.rel, &opened.content, &meta, opened.id)
            .ok_or(E::Empty)
    }

    /// 安全打开一篇库内文件：**检查与读取绑定在同一个对象上**（见 [`crate::open`]）。
    ///
    /// 这是库内读取"喂给模型 / 写进产物"的**唯一**入口。[`Vault::read`] 走的是普通读取
    /// 路径（先解析、再按同一路径打开），它对编辑器/索引这类"读回自己刚写的文件"够用，
    /// 但对"把读到的字节交给模型再落回库里"不够——那条路上，检查过的对象与读到的对象必须
    /// 是同一个，否则库外内容会经它搬进库里。**不要为省事把这里换回 `read`。**
    pub fn open_in_vault(
        &self,
        rel: &str,
    ) -> std::result::Result<crate::VaultFile, crate::VaultOpenError> {
        self.open_in_vault_with_limit(rel, None)
    }

    /// 主题证据提取专用：沿同一安全打开链在读取前及读取中限制字节数。
    pub(crate) fn open_in_vault_limited(
        &self,
        rel: &str,
        max_bytes: usize,
    ) -> std::result::Result<crate::VaultFile, crate::VaultOpenError> {
        self.open_in_vault_with_limit(rel, Some(max_bytes))
    }

    fn open_in_vault_with_limit(
        &self,
        rel: &str,
        max_bytes: Option<usize>,
    ) -> std::result::Result<crate::VaultFile, crate::VaultOpenError> {
        // 词法防护：`..` / 绝对路径 / 盘符。
        let abs = self
            .resolve(rel)
            .map_err(|_| crate::VaultOpenError::PathEscape)?;
        let (id, content) = match max_bytes {
            Some(limit) => crate::open::open_bound_limited(&abs, &self.root, limit)?,
            None => crate::open::open_bound(&abs, &self.root)?,
        };
        Ok(crate::VaultFile {
            rel: normalize_rel(rel),
            id,
            content,
        })
    }

    /// 这条相对路径上此刻**什么都没有**（供唯一命名挑落点）。
    ///
    /// 判据是「文件系统里有没有这个条目」，不是「读不读得出来」：一篇权限不足或正在
    /// 发生瞬时 IO 错误的既有笔记，读会失败——把读失败当成"这里是空的"，下一步就会写到
    /// 用户已有的资产上。用 `symlink_metadata` 而不是 `metadata`：悬空软链也是占着的条目，
    /// 而 `metadata` 会跟随它、报"不存在"。
    #[must_use]
    pub fn path_is_free(&self, rel: &str) -> bool {
        match self.resolve(rel) {
            // 解析不了（越界）时当作"被占"：绝不把一条不合法的路径当成可以落笔的空位。
            Err(_) => false,
            Ok(abs) => std::fs::symlink_metadata(&abs).is_err(),
        }
    }

    /// 发布一篇**全新**笔记：同目录临时文件写全内容 + `link(2)` 原子落点。
    ///
    /// **落点被占返回 [`VaultError::AlreadyExists`]，绝不覆盖**。这是取代
    /// 「`read(target).is_ok()` = 被占」那个错误判据的唯一原子判据：读失败可能是权限、可能
    /// 是瞬时 IO，把它当成"目标不存在"就会写在别人头上；而 `link` 的 `EEXIST` 由内核给出，
    /// 无歧义、无窗口。
    ///
    /// **刻意不用"先建零字节文件占位"**：一是它会让知识度量里的"新建"判定（靠回读推断）
    /// 当场翻成 false，二是崩溃/被杀会在用户 Vault 里留下一篇**空笔记**——在「笔记即文件」
    /// 的仓库里那是用户可见事故。临时文件崩溃后只留下一个可安全清理的 `.tmp`，不在文件树
    /// 里冒出任何东西（点开头，`walk_markdown` 与文件树都跳过）。
    ///
    /// **临时文件必须与落点同目录**：`link` / `rename` 跨卷都会失败，而用户完全可以把
    /// 笔记目录挂在另一个卷上。
    pub fn publish_new(&self, rel: &str, content: &str) -> Result<()> {
        let abs = self.resolve(rel)?;
        let parent = abs
            .parent()
            .ok_or_else(|| VaultError::PathEscape(rel.into()))?
            .to_path_buf();
        std::fs::create_dir_all(&parent)?;
        let tmp = parent.join(format!(
            ".luna-publish-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::now_v7().simple()
        ));
        {
            use std::io::Write as _;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(content.as_bytes())?;
            // **先让内容落到盘上，再让它获得正式的名字**。反过来（link 完就返回）在掉电
            // 时留下的是一条**有名字、内容为空**的笔记：用户看到自己刚写的东西没了，而 Vault
            // 是唯一事实源（ADR-025），没有第二份可回滚。写完 fsync 的代价只在发布这一次。
            file.sync_all()?;
        }
        let linked = std::fs::hard_link(&tmp, &abs);
        // 临时文件在两种结局下都要清掉：成功后它只是同一份内容的第二个名字，失败后它是垃圾。
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(VaultError::AlreadyExists(rel.into()));
            }
            // 网络卷、exFAT/FAT 一类落点做不了 `link(2)`：macOS 上表现为 `EPERM`。这不是
            // "写失败"，是这个位置根本给不了原子发布，解法是换 Vault 位置——单独报出来。
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                return Err(VaultError::AtomicPublishUnsupported(abs));
            }
            Err(error) => return Err(VaultError::Io(error)),
        }
        // 内容持久了，但"这个名字存在"还只在目录的页缓存里；补一次目录 fsync 让它一起活过
        // 掉电。失败不回退：文件此刻已经在库里了，报错反而制造一条不存在的失败。
        if let Ok(dir) = std::fs::File::open(&parent) {
            let _ = dir.sync_all();
        }
        // 剩下的与 `Vault::write` 同一口径：登记自写指纹（监听到的本次事件判为自写、不触发
        // 冲突，§4）并重索引。不留快照——这是一篇刚出生的笔记，没有旧内容可回滚。
        if let Ok(mut g) = self.guard.lock() {
            g.record_write(&abs, ContentHash::of(content.as_bytes()));
        }
        self.index
            .index_note(&normalize_rel(rel), content, file_mtime_ms(&abs))?;
        Ok(())
    }

    /// 一批汇总源**此刻**是否仍与发起那一刻的快照逐条一致（对象身份 + 指纹）。
    ///
    /// 汇总要跨分钟级的模型等待，其间用户完全可以改稿、移走、或把某一篇沉淀掉。落盘前逐条
    /// 复验，任一条对不上即整体放弃——发布一篇与来源对不上的笔记，而来源区的链接还言之凿凿，
    /// 比直接失败糟得多。
    ///
    /// **刻意只比指纹、不比对象身份**（[`crate::ObjectId`]）——两者管的是不同的跨度：
    /// 身份回答"这两条是不是同一篇"（一瞬间的事实，去重用），指纹回答"这一篇有没有变过"
    /// （跨分钟级等待的事实）。而**原子写会换掉 inode**：任何一次写盘（含外部编辑器的
    /// 逐字保存）都会让身份改变，即使一个字节都没动。把身份塞进这里，等于让"有人保存过
    /// 这个文件"也算"源变了"，用户会拿到一条他改不掉的失败。
    ///
    /// "同一条相对路径此刻指向了另一个库里的同名文件"由**库代际**拦（ADR-025），不由这里
    /// 拦——它压根不是"源变了"，是整个世界换了。
    #[must_use]
    pub fn merge_sources_unchanged(&self, sources: &[crate::MergeSource]) -> bool {
        sources.iter().all(|src| {
            self.merge_source(&src.rel)
                .is_ok_and(|now| now.fingerprint == src.fingerprint)
        })
    }

    /// 某篇笔记的文件修改时间（Unix 毫秒；读不到时为 `0`）。
    ///
    /// 与文件树的 `mtime_ms`、收件箱的 `modified_ms` 是**同一个来源**（`file_mtime_ms`）
    /// ——凡是要按"什么时候写的"排序的地方，答案都必须是同一个，否则同一批笔记在两个
    /// 表面上的先后会不一样。读不到就当最早：把一篇取不到时间的笔记排到最前，比让整个
    /// 排序在它身上失败要诚实。
    #[must_use]
    pub fn modified_ms(&self, rel: &str) -> i64 {
        file_mtime_ms(&self.root.join(normalize_rel(rel)))
    }

    /// 与某篇笔记共享标签的其它笔记（[`crate::Index::tag_neighbors`] 的直通口）。
    ///
    /// 归入的两个方向都靠它起步：「这篇该进哪几页」与「这页该收哪几篇」是同一个共享标签
    /// 关系的两次读法，故只有一处实现（DRY），排序口径也只有一套。
    pub fn tag_neighbors(&self, rel: &str) -> Result<Vec<crate::TagNeighbor>> {
        self.index.tag_neighbors(rel)
    }

    /// 用户已忽略的主题页建议标签（PRD-16-016）。随库存 `.luna/topic_ignores.json`。
    pub fn topic_suggestion_ignores(&self) -> Result<Vec<String>> {
        crate::meta::read_meta(&self.root, "topic_ignores")
    }

    /// 全库忽略的 AI 关系标签（PRD-16-051），按忽略的先后。
    pub fn ai_tag_ignores(&self) -> Result<Vec<String>> {
        crate::meta::read_meta(&self.root, "ai_tag_ignores")
    }

    /// 忽略或恢复一个 AI 关系标签（全库，幂等）。只改随库清单与派生索引：笔记文件一字不动，
    /// 用户自己写的同名标签不受影响；恢复后原样回来。
    pub fn set_ai_tag_ignored(&self, tag: &str, ignored: bool) -> Result<()> {
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        let mut list = self.ai_tag_ignores()?;
        let present = list.iter().any(|t| t.eq_ignore_ascii_case(tag));
        if ignored && !present {
            list.push(tag.to_owned());
        } else if !ignored && present {
            list.retain(|t| !t.eq_ignore_ascii_case(tag));
        } else {
            return Ok(());
        }
        crate::meta::write_meta(&self.root, "ai_tag_ignores", &list)?;
        self.index.sync_ai_tag_ignores(&list)
    }

    /// 带这条标签或其子标签的笔记（标签改名的影响面，ADR-135）。
    pub(crate) fn notes_with_tag_branch(&self, tag: &str) -> Result<Vec<String>> {
        self.index.notes_with_tag_branch(tag)
    }

    /// 库里有没有这条标签本身。
    pub(crate) fn tag_exists(&self, tag: &str) -> Result<bool> {
        self.index.tag_exists(tag)
    }

    /// 已忽略的 AI 标签各压下了几篇笔记（按标签名排序）。
    pub fn ignored_ai_tags(&self) -> Result<Vec<(String, u32)>> {
        self.index.suppressed_ai_tag_counts()
    }

    /// 忽略一条主题页建议（幂等：已在清单里即无事发生）。
    pub fn ignore_topic_suggestion(&self, tag: &str) -> Result<()> {
        let mut list = self.topic_suggestion_ignores()?;
        if !list.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            list.push(tag.to_owned());
            crate::meta::write_meta(&self.root, "topic_ignores", &list)?;
        }
        Ok(())
    }

    // ── 正文指纹 ─────────────────────────────────────────────────────────────────

    /// 一篇知识笔记正文此刻的指纹（建议的有效期凭据；与 `synth_body` 同一口径）。
    #[must_use]
    pub fn body_fingerprint(&self, rel: &str) -> Option<String> {
        self.read(rel)
            .ok()
            .map(|doc| ContentHash::of(body_of(&doc.content).as_bytes()).to_string())
    }

    /// 知识生命周期意图的落盘校验指纹（[`crate::knowledge_body_hash`] 的唯一读侧）。
    ///
    /// 与 [`Vault::body_fingerprint`] **刻意分家**：后者服务「用户改过没有」（对落盘字节
    /// 敏感是它的正确性），前者服务「我打算写的那份内容是不是已经在盘上」——生产者手上
    /// 只有候选正文，两侧必须归一化到同一基准才可能相等。
    #[must_use]
    pub fn knowledge_body_fingerprint(&self, rel: &str) -> Option<String> {
        self.read(rel)
            .ok()
            .map(|doc| crate::knowledge_body_hash(body_of(&doc.content)))
    }

    /// AI 关系标签重扫盘点（PRD-06-035b/035c）：逐篇比对正文指纹与 `luna.ai_tags_src`，
    /// 只读不写。`scope = Some(文件夹)` 时只盘点该文件夹（含子孙）。分类：`stale` =
    /// 正文非空且指纹不一致（含从未生成过），`fresh` = 指纹仍最新，`empty` = 正文为空
    /// （无从提炼）。**被挡在外面的分两个数**（2026-07-30 真机第二轮的 D7）：
    /// `excluded_type` = 白名单外（ADR-024 资料区本就不打标；ADR-086 起**记录态一并计入
    /// 这一档**——同为「设计如此、用户无须处置」，不新开第三个计数），`excluded_folder` =
    /// 落在**用户停用的文件夹**内（[`Vault::graph_tag_exclusions`]）。
    ///
    /// **两个数必须分开**：合成一个"27 篇在排除范围"时，用户无从知道其中有 15 篇是
    /// **他自己某次误点停用了整个文件夹**——而那一下会让整个知识面静默失效（标签长不出来
    /// → 主题页建议永不触发 → 归入链路整条够不着）。规则的结果可见、规则本身也必须可见。
    ///
    /// 指纹判定口径与 [`Vault::set_ai_tags`] 写入的同源（`ContentHash` over 正文字节）——
    /// 两边口径分家会让「已最新」变成谎报。
    pub fn graph_tag_scan(&self, scope: Option<&str>) -> Result<GraphTagScan> {
        if let Some(folder) = scope {
            Self::validate_scope_folder(folder)?;
        }
        let excluded_folders = self.graph_tag_exclusions()?;
        let mut scan = GraphTagScan::default();
        for rel in self.walk_markdown()? {
            if let Some(folder) = scope {
                if !crate::graph::scope_covers(folder, &rel) {
                    continue;
                }
            }
            if crate::graph::is_tag_excluded(&rel, &excluded_folders) {
                scan.excluded_folder += 1;
                continue;
            }
            let Ok(doc) = self.read(&rel) else {
                continue; // 扫描期间被删/不可读：跳过，不让单篇故障拖垮整个盘点。
            };
            // 白名单（ADR-024 类型 × ADR-086 成熟度）：资料区与记录态本就不打标，
            // 这一档是**设计**，用户无须处置。判据是同一个 `allows_auto_ai_tags`——
            // 重扫盘点的排除计数与自动触发共用它，不在第二处散写。
            // 知识地图同档豁免（PRD-16-023 修订）：落盘路径本就跳过打标
            // （`origin == "knowledge_map"`），重扫不豁免就是「生成侧不打、重扫补上」
            // 的自相矛盾——真机第五轮坐实：地图被打上 `#cutover-迁移`，混进标签页计数。
            if doc.meta.knowledge_map || !doc.meta.stage().allows_auto_ai_tags(doc.meta.note_type) {
                scan.excluded_type += 1;
                continue;
            }
            let body = body_of(&doc.content);
            if body.trim().is_empty() {
                scan.empty += 1;
                continue;
            }
            let hash = ContentHash::of(body.as_bytes());
            if doc.meta.ai_tags_src.as_deref() == Some(hash.as_str()) {
                scan.fresh.push(rel);
            } else {
                scan.stale.push(rel);
            }
        }
        Ok(scan)
    }

    /// 盘点库内 AI 标签的语言归属（PRD-06-035d ②）。
    ///
    /// 「标签是**库级词汇表**、一个库一种语言」是 PRD-06-035 那条 2026-07-29 修订的前提：
    /// 同一概念裂成 `#architecture` 与 `#架构` 各挂一半笔记时，「同标签 ≥3 篇」的主题页
    /// 判据永远凑不满。本盘点回答的就是「这个库此刻裂了没有、裂了多少篇」。
    ///
    /// **判据逐条标签走 [`tag_fits_language`]，与注入词表的过滤同一份**：一篇笔记
    /// 只要有一条标签站不住（含可翻译的错语言普通词段）就算不一致，全部站得住才算一致。
    /// 专名形状（`#AWS`、`#Datadog`、`#I2X`）语言中性、两头都站得住——提示词明写着
    /// keep well-known technical acronyms as-is，把它们算成"待收敛"就是给用户一个重扫
    /// 也消不掉的数字。此前的判据是笔记级"有一个含 CJK 就算中文"：`#运维 #ops` 这种
    /// **一篇之内就分裂**的组合被数成一致，恰好漏掉分裂最常见的形态。
    ///
    /// **判定面与「全部重新生成」严格对齐**：类型白名单外与用户停用文件夹内的笔记一律
    /// 不计——它们不会被重扫覆盖，把它们算进"受影响篇数"就是给用户一个按下去也消不掉的
    /// 数字（PRD-16-039「不为凑数硬推操作」的同一条纪律）。
    ///
    /// `want_zh` = 此刻的界面语言是不是中文。
    pub fn graph_tag_language_scan(&self, want_zh: bool) -> Result<GraphTagLanguageScan> {
        let excluded_folders = self.graph_tag_exclusions()?;
        let mut scan = GraphTagLanguageScan::default();
        let mut offending: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for rel in self.walk_markdown()? {
            if crate::graph::is_tag_excluded(&rel, &excluded_folders) {
                continue;
            }
            let Ok(doc) = self.read(&rel) else {
                continue; // 同 graph_tag_scan：单篇不可读不拖垮整个盘点。
            };
            // 判定面与「全部重新生成」严格对齐（含知识地图豁免，理由见 `graph_tag_scan`）。
            if doc.meta.knowledge_map || !doc.meta.stage().allows_auto_ai_tags(doc.meta.note_type) {
                continue;
            }
            if doc.meta.ai_tags.is_empty() {
                continue; // 还没有标签的笔记归「缺标签」那张卡，不是语言问题。
            }
            let mut fits = true;
            for tag in doc
                .meta
                .ai_tags
                .iter()
                .filter(|tag| !tag_fits_language(tag, want_zh))
            {
                fits = false;
                *offending.entry(tag.clone()).or_default() += 1;
            }
            if fits {
                scan.matched += 1;
            } else {
                scan.mismatched += 1;
            }
        }
        scan.sample_tags = most_frequent(offending, TAG_LANGUAGE_SAMPLE_LIMIT);
        Ok(scan)
    }

    /// 读取 AI 标签生成的排除文件夹清单（PRD-06-035c，Vault 相对路径，排序去重）。
    /// 随库存储于 `.luna/graph_scope.json`——路径是库相对的，规则理应随库迁移
    /// （Obsidian「Excluded files」同构），存设备级配置会在换库后指向不存在的目录。
    pub fn graph_tag_exclusions(&self) -> Result<Vec<String>> {
        let mut folders: Vec<String> = crate::meta::read_meta(&self.root, "graph_scope")?;
        folders.sort();
        folders.dedup();
        Ok(folders)
    }

    /// 增删一条排除文件夹并返回更新后的完整清单（前端据此就地刷新，免二次读取）。
    /// 排除只约束**未来的自动生成与批量重扫**，绝不删除既有 `luna.ai_tags`——
    /// 排除不该是隐式销毁。
    pub fn set_graph_tag_exclusion(&self, folder: &str, excluded: bool) -> Result<Vec<String>> {
        let folder = Self::validate_scope_folder(folder)?;
        let mut folders = self.graph_tag_exclusions()?;
        if excluded {
            if !folders.iter().any(|f| f == &folder) {
                folders.push(folder);
                folders.sort();
            }
        } else {
            folders.retain(|f| f != &folder);
        }
        crate::meta::write_meta(&self.root, "graph_scope", &folders)?;
        Ok(folders)
    }

    /// 读取用户自定义蒸馏模板（PRD-06-051 批次 2）。
    ///
    /// **随库存储**于 `.luna/distill_templates.json`（同 `graph_scope` 的判据）：模板是
    /// 「这个库里的知识长什么样」的一部分——工作库的会议模板对学习库毫无意义，而蒸馏
    /// 笔记的 frontmatter 里存着模板键，模板若存设备级，换库后那些键会指向不存在的模板。
    ///
    /// # Errors
    /// 元数据文件损坏或不可读时返回错误。
    pub fn distill_templates(&self) -> Result<Vec<luna_core::UserTemplate>> {
        crate::meta::read_meta(&self.root, "distill_templates")
    }

    /// 整体写回用户模板清单（原子写）。
    ///
    /// 整体替换而非增量改：清单短（上限 30 条）、写入是用户显式动作，读改写一次比维护
    /// 一套增量接口简单得多，也天然没有"改到一半"的中间态。
    ///
    /// # Errors
    /// 写入失败时返回错误。
    pub fn set_distill_templates(&self, templates: &[luna_core::UserTemplate]) -> Result<()> {
        crate::meta::write_meta(&self.root, "distill_templates", templates)
    }

    /// 用户自定义 AI 指令预设（PRD-06-024），随库存于 `.luna/ai_presets.json`。
    pub fn ai_presets(&self) -> Result<Vec<crate::AiPreset>> {
        crate::presets::read_ai_presets(&self.root)
    }

    pub fn set_ai_presets(&self, presets: &[crate::AiPreset]) -> Result<()> {
        crate::presets::write_ai_presets(&self.root, presets)
    }

    /// 范围文件夹的合法性口径（盘点与排除共用）：非空、不指向 Vault 根、不越界；
    /// 归一化为无首尾 `/` 的库相对路径。
    pub(crate) fn validate_scope_folder(folder: &str) -> Result<String> {
        let normalized = folder.trim().trim_matches('/');
        if normalized.is_empty() {
            // 根即全库——那是总开关（`vault.graph_ai_tags`）的语义，不是一条范围规则。
            return Err(VaultError::InvalidScope(folder.to_owned()));
        }
        let rel = Path::new(normalized);
        for comp in rel.components() {
            match comp {
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(VaultError::InvalidScope(folder.to_owned()));
                }
                _ => {}
            }
        }
        Ok(normalized.to_owned())
    }

    /// 移动/重命名笔记、附件或文件夹，并改写全库因此失效或改了名的链接（PRD-06-030）。
    pub fn move_(&self, from: &str, to: &str) -> Result<()> {
        self.move_with_report(from, to).map(|_| ())
    }

    /// 同 [`Vault::move_`]，另返回回执（改写了哪些笔记、几处链接、撤销凭据）。
    ///
    /// **先查后写**（TDD-06 §3 改名重构 ⑤）：要搬哪些文件、每篇笔记改成什么，在动磁盘之前
    /// 全部算好；有一篇写不进去就整次不做（[`VaultError::NotWritable`]）。执行中途失败时把已
    /// 写的篇与已搬的文件尽量还原，还原不了的如实列出（[`VaultError::RelocationPartial`]）——
    /// 跨文件严格原子做不到，不承诺。
    pub fn move_with_report(&self, from: &str, to: &str) -> Result<RelocationReceipt> {
        let from_abs = self.resolve(from)?;
        let to_abs = self.resolve(to)?;
        // 只改大小写（`foo` → `Foo`）：大小写不敏感的文件系统（macOS 默认）上「目标已存在」
        // 指的就是源本身，按占用拒绝会让这类改名永远做不成（PRD-06-006 ②）。
        let case_only = is_case_only_rename(&from_abs, &to_abs);
        if to_abs.exists() && !case_only {
            return Err(VaultError::AlreadyExists(to_abs));
        }
        if to_abs.starts_with(&from_abs) {
            return Err(VaultError::InvalidMove(format!(
                "不能把「{from}」移动到其自身内部"
            )));
        }
        if !from_abs.exists() {
            return Err(VaultError::NotFound(from_abs));
        }
        let from_is_dir = from_abs.is_dir() && !from.ends_with(".md");
        let is_note = !from_is_dir && from.ends_with(".md");
        let from_norm = normalize_rel(from);
        let to_norm = normalize_rel(to);

        // ① 规划（只读）：移动前每条链接指向谁、要搬哪些文件、每篇改成什么。
        let before = self.link_world()?;
        let attachments = if is_note {
            self.plan_exclusive_attachments(&from_norm, &to_norm)?
        } else {
            Vec::new()
        };
        let pairs: Vec<(String, String)> = if from_is_dir {
            let prefix = format!("{from_norm}/");
            self.walk_files()?
                .into_iter()
                .filter_map(|old| {
                    let suffix = old.strip_prefix(&prefix)?.to_owned();
                    Some((old, format!("{to_norm}/{suffix}")))
                })
                .collect()
        } else {
            std::iter::once((from_norm.clone(), to_norm.clone()))
                .chain(attachments.iter().cloned())
                .collect()
        };
        let moves = crate::relink::Moves::new(pairs.clone());
        let plan = self.plan_relink(&before, &moves)?;

        // ② 预检：有一篇写不进去就整次不做。
        self.ensure_writable(plan.iter().map(|edit| edit.old_rel.as_str()))?;

        // ③ 搬文件：本体，再附件；附件搬失败就把已搬的连同本体搬回去。
        if let Some(parent) = to_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rename_entry(&from_abs, &to_abs, case_only)?;
        let mut moved_attachments: Vec<(String, String)> = Vec::new();
        for (old, new) in &attachments {
            if let Err(error) = self.move_file_rel(old, new) {
                let mut unrestored = self.move_back(&moved_attachments);
                if rename_entry(&to_abs, &from_abs, case_only).is_err() {
                    unrestored.push(to_norm.clone());
                }
                return Err(rollback_error(error, unrestored));
            }
            moved_attachments.push((old.clone(), new.clone()));
        }
        if let Ok(mut g) = self.guard.lock() {
            if from_is_dir {
                g.forget_dir(&from_abs);
            } else {
                g.forget(&from_abs);
            }
        }
        // 版本历史随笔记走（V-7）。快照失败不阻断（§4.7 降级）。
        for (old, new) in pairs.iter().filter(|(old, _)| old.ends_with(".md")) {
            let _ = self.snapshots.rename_chain(old, new);
        }

        // ④ 写改写计划；中途失败按 before 逐篇写回，再把文件搬回去。
        let mut written: Vec<&crate::relink::PlannedRewrite> = Vec::new();
        for edit in &plan {
            if let Err(error) = self.write_planned(edit) {
                let mut unrestored = Vec::new();
                for done in written.iter().rev() {
                    if write_atomic(&self.root.join(&done.new_rel), done.before.as_bytes()).is_err()
                    {
                        unrestored.push(done.new_rel.clone());
                    }
                }
                for (old, new) in pairs.iter().filter(|(old, _)| old.ends_with(".md")) {
                    let _ = self.snapshots.rename_chain(new, old);
                }
                unrestored.extend(self.move_back(&moved_attachments));
                if rename_entry(&to_abs, &from_abs, case_only).is_err() {
                    unrestored.push(to_norm.clone());
                }
                if !unrestored.is_empty() {
                    // 回滚不完整：索引按磁盘现状重建这几篇，至少不撒谎。
                    let _ = self.sync_external_note(&to_norm);
                }
                return Err(rollback_error(error, unrestored));
            }
            written.push(edit);
        }

        // ⑤ 收尾：媒体、蒸馏指针、索引。
        if from_is_dir {
            self.index.remove_directory(&from_norm)?;
            for (old_rel, new_rel) in pairs.iter().filter(|(_, new)| new.ends_with(".md")) {
                // 录音音频/边车已随整树 rename 迁移 → 仅改写 frontmatter 库相对路径（前缀换）。
                self.relocate_note_media(old_rel, new_rel, true)?;
                let dir_remap = |path: &str| remap_prefix(path, &from_norm, &to_norm);
                self.relocate_distill_pointers(new_rel, &dir_remap)?;
                let abs = self.root.join(new_rel);
                if let Ok(content) = std::fs::read_to_string(&abs) {
                    self.index
                        .index_note(new_rel, &content, file_mtime_ms(&abs))?;
                }
            }
        } else if is_note {
            // 录音音频/边车作为附件随笔记物理搬迁（frontmatter `media[]`，PRD-04-030 收口）。
            self.relocate_note_media(&from_norm, &to_norm, false)?;
            // 蒸馏指针跟随（PRD-06-044）：单文件移动只有本篇换了位置，对方路径不变。
            self.relocate_distill_pointers(&to_norm, &|path: &str| path.to_owned())?;
            // 移动 = 旧路径**真的不在了**，故连归入建议一并忘掉（它以路径为键；新路径的建议由
            // 下一次触发重新算出来，搬过去的那份没人保证目标页还是那几页）。
            self.index.forget_note(&from_norm)?;
            if let Ok(content) = std::fs::read_to_string(&to_abs) {
                self.index
                    .index_note(&to_norm, &content, file_mtime_ms(&to_abs))?;
            }
        }
        // 被搬动的是一份资产（非 md）：正文链接已在上面改好，frontmatter `media[]` 与转写边车
        // 在这里跟上（真机上表现过「给 wav 改个名，回放就打不开了」，2026-09-02）。
        let mut rewritten: Vec<String> = plan.iter().map(|edit| edit.new_rel.clone()).collect();
        if !from_is_dir && !is_note {
            for rel in self.relocate_asset_references(&from_norm, &to_norm)? {
                if !rewritten.contains(&rel) {
                    rewritten.push(rel);
                }
            }
        }

        // 整批写完再全量刷一次盘（改写走的是批量轻量落盘，见 `write_atomic_batched`）。
        crate::atomic::flush_durable(&self.root)?;

        // ⑥ 回执：撤销凭据记的是「写之前 / 现在」的指纹，撤销时逐篇核对。
        let notes = plan
            .iter()
            .filter_map(|edit| {
                let now = std::fs::read(self.root.join(&edit.new_rel)).ok()?;
                Some(RelocationNoteChange {
                    path: edit.new_rel.clone(),
                    before_hash: ContentHash::of(edit.before.as_bytes()).to_string(),
                    after_hash: ContentHash::of(&now).to_string(),
                })
            })
            .collect();
        // 书签跟着内容走（V5b）；书签是次要状态，写不进去不让这次移动失败。
        let _ = self.remap_bookmarks(&from_norm, &to_norm);
        Ok(RelocationReceipt {
            links: plan.iter().map(|edit| edit.links).sum(),
            rewritten,
            undo: RelocationUndo {
                from: from_norm,
                to: to_norm,
                attachments,
                notes,
            },
        })
    }

    /// 撤销一次改名 / 移动（PRD-06-030 ③）：只在**之后没有任何新写入**时可做——被改写的笔记
    /// 指纹仍是改完时那样、搬走的文件还在新位置、旧位置还空着；任一不符整次拒绝
    /// （[`VaultError::RelocationStale`]），不做半个撤销。被改写的笔记按快照逐字节还原，
    /// 文件搬回原处，媒体与蒸馏指针按同一套跟随规则反向走一遍。返回还原了的笔记（原路径）。
    pub fn undo_relocation(&self, undo: &RelocationUndo) -> Result<Vec<String>> {
        let from_abs = self.resolve(&undo.from)?;
        let to_abs = self.resolve(&undo.to)?;
        let case_only = is_case_only_rename(&to_abs, &from_abs);
        let mut stale = Vec::new();
        if !to_abs.exists() {
            stale.push(undo.to.clone());
        }
        if from_abs.exists() && !case_only {
            stale.push(undo.from.clone());
        }
        for (old, new) in &undo.attachments {
            if !self.root.join(new).is_file() || self.root.join(old).exists() {
                stale.push(new.clone());
            }
        }
        let mut befores = Vec::new();
        for note in &undo.notes {
            let current = std::fs::read(self.root.join(&note.path)).ok();
            let unchanged = current
                .as_deref()
                .is_some_and(|bytes| ContentHash::of(bytes).as_str() == note.after_hash);
            match unchanged
                .then(|| self.snapshot_text_with_hash(&note.path, &note.before_hash))
                .flatten()
            {
                Some(before) => befores.push((note.path.clone(), before)),
                None => stale.push(note.path.clone()),
            }
        }
        if !stale.is_empty() {
            return Err(VaultError::RelocationStale(stale.join("、")));
        }

        let from_is_dir = to_abs.is_dir() && !undo.to.ends_with(".md");
        let is_note = !from_is_dir && undo.to.ends_with(".md");
        let md_pairs: Vec<(String, String)> = if from_is_dir {
            let prefix = format!("{}/", undo.to);
            self.walk_files()?
                .into_iter()
                .filter_map(|new| {
                    let suffix = new.strip_prefix(&prefix)?.to_owned();
                    new.ends_with(".md")
                        .then(|| (format!("{}/{suffix}", undo.from), new.clone()))
                })
                .collect()
        } else if is_note {
            vec![(undo.from.clone(), undo.to.clone())]
        } else {
            Vec::new()
        };

        if let Some(parent) = from_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rename_entry(&to_abs, &from_abs, case_only)?;
        let back: Vec<(String, String)> = undo.attachments.clone();
        let unrestored = self.move_back(&back);
        if !unrestored.is_empty() {
            return Err(VaultError::RelocationPartial(unrestored.join("、")));
        }
        if let Ok(mut g) = self.guard.lock() {
            if from_is_dir {
                g.forget_dir(&to_abs);
            } else {
                g.forget(&to_abs);
            }
        }
        for (old, new) in &md_pairs {
            let _ = self.snapshots.rename_chain(new, old);
        }

        let map_back = |path: &str| -> String {
            if path == undo.to {
                return undo.from.clone();
            }
            if let Some(suffix) = path.strip_prefix(&format!("{}/", undo.to)) {
                return format!("{}/{suffix}", undo.from);
            }
            path.to_owned()
        };
        let mut restored = Vec::new();
        for (path, before) in &befores {
            let rel = map_back(path);
            let abs = self.root.join(&rel);
            if let Ok(current) = std::fs::read(&abs) {
                let _ = self.snapshots.snapshot(&rel, &current);
            }
            write_atomic(&abs, before.as_bytes())?;
            if let Ok(mut g) = self.guard.lock() {
                g.record_write(&abs, ContentHash::of(before.as_bytes()));
            }
            restored.push(rel);
        }

        if from_is_dir {
            self.index.remove_directory(&undo.to)?;
            for (old_rel, new_rel) in &md_pairs {
                self.relocate_note_media(new_rel, old_rel, true)?;
                let remap = |path: &str| remap_prefix(path, &undo.to, &undo.from);
                self.relocate_distill_pointers(old_rel, &remap)?;
                let abs = self.root.join(old_rel);
                if let Ok(content) = std::fs::read_to_string(&abs) {
                    self.index
                        .index_note(old_rel, &content, file_mtime_ms(&abs))?;
                }
            }
        } else if is_note {
            self.relocate_note_media(&undo.to, &undo.from, false)?;
            self.relocate_distill_pointers(&undo.from, &|path: &str| path.to_owned())?;
            self.index.forget_note(&undo.to)?;
            if let Ok(content) = std::fs::read_to_string(&from_abs) {
                self.index
                    .index_note(&undo.from, &content, file_mtime_ms(&from_abs))?;
            }
        } else {
            for rel in self.relocate_asset_references(&undo.to, &undo.from)? {
                if !restored.contains(&rel) {
                    restored.push(rel);
                }
            }
        }
        for rel in &restored {
            let abs = self.root.join(rel);
            if let Ok(content) = std::fs::read_to_string(&abs) {
                self.index.index_note(rel, &content, file_mtime_ms(&abs))?;
            }
        }
        let _ = self.remap_bookmarks(&undo.to, &undo.from);
        Ok(restored)
    }

    /// 把一份文件从 `old` 搬到 `new`（库相对路径），必要时建目录。
    fn move_file_rel(&self, old: &str, new: &str) -> Result<()> {
        let new_abs = self.root.join(new);
        if let Some(parent) = new_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(self.root.join(old), new_abs)?;
        Ok(())
    }

    /// 把一批已搬的文件（旧, 新）搬回去，返回没能搬回的新路径。
    fn move_back(&self, moved: &[(String, String)]) -> Vec<String> {
        moved
            .iter()
            .rev()
            .filter(|(old, new)| self.move_file_rel(new, old).is_err())
            .map(|(_, new)| new.clone())
            .collect()
    }

    /// 这些笔记（库里当前的正文）的链接各指向哪个文件——与改名重构、嵌入、孤儿检测同一个
    /// 解析器（[`crate::relink::resolved_targets`]）。附件跟随移动 / 删除据此判「独占」：
    /// 此前按「源目录 + 写法」拼路径，看不见 `![[图.png]]` 这种按文件名找的引用——另一篇
    /// 这样引用着的共享图会被当成独占，随删除进回收站、随移动被搬走，那一篇的图就断了。
    pub(crate) fn link_targets_of<'a>(
        &self,
        world: &crate::relink::LinkWorld,
        notes: impl IntoIterator<Item = &'a str>,
    ) -> std::collections::HashSet<String> {
        let contents: Vec<(&str, String)> = notes
            .into_iter()
            .filter_map(|rel| {
                std::fs::read_to_string(self.root.join(rel))
                    .ok()
                    .map(|content| (rel, content))
            })
            .collect();
        crate::relink::resolved_targets(
            world,
            contents
                .iter()
                .map(|(rel, content)| (*rel, content.as_str())),
        )
    }

    /// **除 `exclude_note` 外**所有笔记引用到的文件（判某附件是否被移动的笔记独占）。
    fn collect_other_attachment_refs(
        &self,
        world: &crate::relink::LinkWorld,
        exclude_note: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let notes: Vec<String> = self
            .walk_markdown()?
            .into_iter()
            .filter(|rel| rel != exclude_note)
            .collect();
        Ok(self.link_targets_of(world, notes.iter().map(String::as_str)))
    }

    /// 附件跟随笔记移动（PRD-06-012）：本笔记**独占**且在旧目录子树内的本地附件，各自要搬到
    /// 新目录的哪里（落点被占则取唯一名）。**只算不搬**——在动磁盘之前与改写计划一起定下，
    /// 链接交给改名重构一次算清。同目录移动（纯重命名）不搬。
    fn plan_exclusive_attachments(&self, from: &str, to: &str) -> Result<Vec<(String, String)>> {
        let from_dir = luna_sync_core::links::parent_dir(from).to_owned();
        let to_dir = luna_sync_core::links::parent_dir(to).to_owned();
        if from_dir == to_dir {
            return Ok(Vec::new());
        }
        let world = self.link_world()?;
        let others = self.collect_other_attachment_refs(&world, from)?;
        let mut targets: Vec<String> = self.link_targets_of(&world, [from]).into_iter().collect();
        targets.sort();
        let mut planned: Vec<(String, String)> = Vec::new();
        for old_vrel in targets {
            // 笔记不当附件搬；只搬存在的非 md 文件，且同一份只搬一次。
            if old_vrel.ends_with(".md") || planned.iter().any(|(old, _)| old == &old_vrel) {
                continue;
            }
            if !self.root.join(&old_vrel).is_file() {
                continue;
            }
            let in_subtree = from_dir.is_empty() || old_vrel.starts_with(&format!("{from_dir}/"));
            if !in_subtree || others.contains(&old_vrel) {
                continue;
            }
            let suffix = old_vrel
                .strip_prefix(&format!("{from_dir}/"))
                .unwrap_or(&old_vrel);
            let wanted = luna_sync_core::attachment::join_vault_rel(&to_dir, suffix);
            let mut new_vrel = wanted.clone();
            let mut n = 1u32;
            while self.root.join(&new_vrel).exists()
                || planned.iter().any(|(_, new)| new == &new_vrel)
            {
                new_vrel = match wanted.rsplit_once('.') {
                    Some((stem, ext)) => format!("{stem}-{n}.{ext}"),
                    None => format!("{wanted}-{n}"),
                };
                n += 1;
            }
            planned.push((old_vrel, new_vrel));
        }
        Ok(planned)
    }

    /// 一份**资产**被改名/移动之后，把 frontmatter 里引用它的笔记同步过去（PRD-06-012）；
    /// 返回改写了的笔记。正文链接不在这里改——那是改名重构（[`crate::relink`]）的事，两边
    /// 各认一遍链接写法正是 `![[图.png]]` 改名后断掉的由来。这里管另外两条引用面：
    /// - **frontmatter `media[]`**：录音/转写笔记的音频写在这里，回放、二次转写、导出全靠它；
    /// - **转写边车**：按媒体路径 hash 键控（`sidecar_path_for`），路径一变 hash 就变，
    ///   不重键的表现是"文件都在、转写却空了"。
    fn relocate_asset_references(&self, from: &str, to: &str) -> Result<Vec<String>> {
        // 边车随媒体重键（先做：下面改写 frontmatter 时要写入新的边车路径）。
        let old_sidecar = normalize_rel(&sidecar_path_for(from));
        let new_sidecar = if self.root.join(&old_sidecar).is_file() {
            Some(self.move_asset_file(&old_sidecar, &sidecar_path_for(to))?)
        } else {
            None
        };

        let mut rewritten = Vec::new();
        for rel in self.walk_markdown()? {
            let abs = self.root.join(&rel);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let mut updated = content.clone();

            // frontmatter `media[]`：路径与边车一起改（诚实元数据：绝不指向不存在的目标）。
            // 元数据从**已经读到的那份正文**解析，不再 `self.read()` 一遍：那是整库扫描
            // 里每篇多一次文件 IO，而内容就在手上。
            let mut meta = read_luna(&content);
            let mut media_changed = false;
            for media in &mut meta.media {
                if normalize_rel(&media.path) != from {
                    continue;
                }
                media.path = to.to_owned();
                if media.segments.is_some() {
                    if let Some(sidecar) = new_sidecar.clone() {
                        media.segments = Some(sidecar);
                    }
                }
                media_changed = true;
            }
            if media_changed {
                updated = crate::frontmatter::update_luna(&updated, &meta)?;
            }

            if updated == content {
                continue;
            }
            if let Ok(old_bytes) = std::fs::read(&abs) {
                let _ = self.snapshots.snapshot(&rel, &old_bytes);
            }
            write_atomic(&abs, updated.as_bytes())?;
            if let Ok(mut g) = self.guard.lock() {
                g.record_write(&abs, ContentHash::of(updated.as_bytes()));
            }
            self.index.index_note(&rel, &updated, file_mtime_ms(&abs))?;
            rewritten.push(rel);
        }
        Ok(rewritten)
    }

    /// 给定库内相对路径，若已存在则在文件名后追加 `-1`/`-2`… 直至唯一。
    ///
    /// 名字里没有「asset」：附件搬迁与**回收站还原为副本**（[`Vault::restore`] 的
    /// `KeepBoth`）要的是同一件事——「给我一个此刻空着的落点」。两处各写一份，迟早在
    /// 后缀形态上分叉（`-1` 还是 ` (1)`），而用户会在同一个库里看到两种命名。
    pub(crate) fn unique_vault_path(&self, vrel: &str) -> String {
        if !self.root.join(vrel).exists() {
            return vrel.to_owned();
        }
        let (stem, ext) = match vrel.rsplit_once('.') {
            Some((s, e)) => (s.to_owned(), format!(".{e}")),
            None => (vrel.to_owned(), String::new()),
        };
        let mut n = 1u32;
        loop {
            let candidate = format!("{stem}-{n}{ext}");
            if !self.root.join(&candidate).exists() {
                return candidate;
            }
            n += 1;
        }
    }

    /// 物理搬迁一个库内附件文件到目标路径（重名取唯一名）。返回**实际**落点 Vault 相对路径。
    /// 源文件缺失时诚实降级——返回目标候选路径不阻断移动（frontmatter 仍改指新址，避免残留旧链）。
    fn move_asset_file(&self, old_vrel: &str, new_vrel: &str) -> Result<String> {
        let old_abs = self.root.join(old_vrel);
        if !old_abs.is_file() {
            return Ok(new_vrel.to_owned());
        }
        let target = if self.root.join(new_vrel).exists() {
            self.unique_vault_path(new_vrel)
        } else {
            new_vrel.to_owned()
        };
        let new_abs = self.root.join(&target);
        if let Some(parent) = new_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_abs, &new_abs)?;
        Ok(target)
    }

    /// 录音音频作为录音笔记的**附件跟随移动**（PRD-06-012 生命周期扩展 / PRD-04-030）。
    ///
    /// frontmatter `luna.media[]` 的音频分片与其边车随笔记搬到**新笔记目录同后缀处**——保持
    /// **笔记相对结构**不变（即附件位置策略：`录音/assets/x.wav` 移动后仍为 `<新目录>/assets/x.wav`），
    /// 并原地改写 `media[].path` / `media[].segments` 的**库相对**路径。
    ///
    /// 仅搬迁**旧笔记子树内**的媒体（`vault_root`/指定目录集中存放的附件留原地，库相对路径本就
    /// 全局有效，无需改写）——与 [`Self::relocate_note_attachments`] 的正文附件同一判据（DRY）。
    ///
    /// `from`/`to` 为归一化笔记相对路径。`files_moved`=true（目录移动：上层已整树 `rename`）只改写
    /// frontmatter 不搬文件；false（单文件移动）则物理搬迁音频 + 边车。
    /// 蒸馏双向指针跟随移动（PRD-06-044）：笔记换了位置，`distill_source` / `distill_note`
    /// 必须跟着换，否则指针悬空 → 状态回落「未生成」→ 用户重新生成一篇，旧的那篇成孤儿。
    ///
    /// 与 wikilink 的 rename 重构（`rewrite_references_to`）同一心智：**移动不该破坏关系**。
    /// 两步都做：① 用 `remap` 修正**本篇指向对方**的出向指针（对方也在被移动的子树内时，
    /// 它的路径同样变了）；② 修正**对方指向本篇**的入向指针为 `new_rel`。
    ///
    /// 对方不存在（用户删过）时静默跳过——悬空指针本就等价于「未生成」，不是故障。
    fn relocate_distill_pointers(
        &self,
        new_rel: &str,
        remap: &dyn Fn(&str) -> String,
    ) -> Result<()> {
        let Ok(doc) = self.read(new_rel) else {
            return Ok(());
        };
        let mut meta = doc.meta.clone();
        let mut self_changed = false;

        // ① 出向指针：对方若随同一次移动改了位置，本篇记的路径要跟着换。
        for slot in [&mut meta.distill_source, &mut meta.distill_note] {
            if let Some(path) = slot.as_deref() {
                let remapped = remap(path);
                if remapped != path {
                    *slot = Some(remapped);
                    self_changed = true;
                }
            }
        }
        if self_changed {
            let content = update_luna(&doc.content, &meta)?;
            self.write(new_rel, &content)?;
        }

        // ② 入向指针：对方记的"我在哪儿"要改成新位置。资料侧与蒸馏侧互为反向，
        //    故按本篇持有的是哪一头，去改对方对应的另一头。
        let counterparts = [
            (meta.distill_source.as_deref(), true), // 本篇是蒸馏笔记 → 改源的 distill_note
            (meta.distill_note.as_deref(), false),  // 本篇是资料 → 改蒸馏笔记的 distill_source
        ];
        for (target, fix_note_side) in counterparts {
            let Some(target) = target else { continue };
            if target == new_rel {
                continue; // 自引（不该出现）：不写，也不递归。
            }
            let Ok(other) = self.read(target) else {
                continue; // 对方已被删除：悬空即「未生成」，无事可做。
            };
            let mut other_meta = other.meta.clone();
            let slot = if fix_note_side {
                &mut other_meta.distill_note
            } else {
                &mut other_meta.distill_source
            };
            if slot.as_deref() == Some(new_rel) {
                continue; // 已是新路径（对方那一轮先跑过）：不重复写盘。
            }
            *slot = Some(new_rel.to_owned());
            let content = update_luna(&other.content, &other_meta)?;
            self.write(target, &content)?;
        }
        Ok(())
    }

    fn relocate_note_media(&self, from: &str, to: &str, files_moved: bool) -> Result<()> {
        let from_dir = luna_sync_core::links::parent_dir(from).to_owned();
        let to_dir = luna_sync_core::links::parent_dir(to).to_owned();
        if from_dir == to_dir {
            return Ok(()); // 同目录（纯重命名）：附件相对结构不变。
        }
        let doc = self.read(to)?;
        let mut meta = doc.meta.clone();
        let mut changed = false;

        for media in &mut meta.media {
            let old_vrel = normalize_rel(&media.path);
            let in_subtree = from_dir.is_empty() || old_vrel.starts_with(&format!("{from_dir}/"));
            if !in_subtree {
                continue; // 集中存放的附件不随笔记搬，库相对路径仍有效。
            }
            let suffix = old_vrel
                .strip_prefix(&format!("{from_dir}/"))
                .unwrap_or(&old_vrel);
            let new_vrel = luna_sync_core::attachment::join_vault_rel(&to_dir, suffix);
            if new_vrel == old_vrel {
                continue;
            }
            let had_sidecar = media.segments.is_some();
            // 音频物理搬迁：目录移动已随整树 rename 迁移（files_moved=true 跳过）；单文件移动须搬。
            // 音频先搬定名（可能取唯一名），边车再按最终音频名派生。
            let final_audio = if files_moved {
                new_vrel
            } else {
                self.move_asset_file(&old_vrel, &new_vrel)?
            };
            // 边车按媒体路径 hash 键控存于中央 `.luna/sidecars/`（与音频/笔记物理位置无关）：
            // 媒体路径一变 hash 即变，故无论目录还是文件移动都须**重键**（rename old→new hash）。
            if had_sidecar {
                let old_sidecar = normalize_rel(&sidecar_path_for(&old_vrel));
                // 取搬迁**实际**落点（极端重名冲突下 move_asset_file 会另取唯一名）→ frontmatter
                // 与磁盘恒一致，绝不指向不存在的目标（诚实元数据本 crate 规约）。
                let new_sidecar =
                    self.move_asset_file(&old_sidecar, &sidecar_path_for(&final_audio))?;
                media.segments = Some(new_sidecar);
            }
            media.path = final_audio;
            changed = true;
        }

        if changed {
            let final_content = update_luna(&doc.content, &meta)?;
            self.write(to, &final_content)?;
        }
        Ok(())
    }

    /// 库内对象是否存在（越界路径一律视为不存在，判定不外泄根外的文件系统状态）。
    ///
    /// 供批量移动的**落点预检**用：整批先看清楚落点是否已被占用，再决定动不动手——
    /// 逐项撞上 [`VaultError::AlreadyExists`] 才停，会留下"前三项已经搬走、后两项还在
    /// 原处"的半途状态，而用户是把这一批当一件事发起的。
    #[must_use]
    pub fn exists(&self, rel: &str) -> bool {
        self.resolve(rel).is_ok_and(|abs| abs.exists())
    }

    /// 库内对象是不是目录。
    ///
    /// **只有它答得了重命名要不要保留扩展名**：从路径字面量猜（"没有点就是目录"）
    /// 会把 `v1.0` 这样的目录当成扩展名为 `0` 的文件。`.md` 恒为文件的判据与
    /// [`Vault::move_`] / [`Vault::delete`] 逐字一致。
    pub fn is_dir(&self, rel: &str) -> Result<bool> {
        let abs = self.resolve(rel)?;
        if !abs.exists() {
            return Err(VaultError::NotFound(rel.into()));
        }
        Ok(abs.is_dir() && !rel.ends_with(".md"))
    }

    /// 新建空文件夹（PRD-06-006「新建文件夹」控制项）。已存在则报错；索引无需变更
    ///（目录树直接来自文件系统遍历）。
    /// 递归列举某个目录下的**全部子目录**（Vault 相对路径，深度优先、升序）。
    ///
    /// 服务于归档清单的文件夹分组（PRD-07-062）：那棵树本来是从**已加载条目的路径**折出来
    /// 的，于是没有条目的目录压根不生成节点——用户刚新建的那个文件夹因此永远看不见，
    /// 而「新建文件夹」按钮就摆在清单顶上（真机 2026-09-08 上报）。空目录是真实存在的
    /// 磁盘事实，这条命令把它当事实读出来，而不是让前端去猜。
    ///
    /// 点开头的目录一律跳过（`.luna` / `.trash` 等，同 [`walk_md`] 的口径）；`rel` 不存在
    /// 时返回空表而不是报错——归档目录可能还没被创建过，那不是错误。
    pub fn list_dirs(&self, rel: &str) -> Result<Vec<String>> {
        let base = normalize_rel(rel);
        let abs = self.resolve(&base)?;
        if !abs.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        walk_dirs(&self.root, &abs, &mut out)?;
        out.sort();
        Ok(out)
    }

    pub fn create_dir(&self, rel: &str) -> Result<()> {
        let abs = self.resolve(rel)?;
        if abs.exists() {
            return Err(VaultError::AlreadyExists(rel.into()));
        }
        std::fs::create_dir_all(&abs)?;
        Ok(())
    }

    /// 改名重构能不能改这一篇：未知证据版本、损坏的 frontmatter 与校验不过的知识页原样保留，
    /// 来源已移动后的失效 / 修复出口由证据状态投影处理。
    pub(crate) fn relink_allowed(content: &str) -> bool {
        match crate::frontmatter::read_luna_checked(content) {
            Ok(meta) if meta.knowledge.is_some() => crate::validate_topic_document(content).is_ok(),
            Err(_) if content.contains("<!-- luna:claim:") => false,
            _ => true,
        }
    }

    /// 改名重构给出的新全文，补上系统底版指纹：改名前仍与底版一致的 AI 维护页须把新正文
    /// 登记为新底版，否则一次纯路径移动就会永久进入人工复核态；被人工改过的页保留旧指纹，
    /// 绝不借重命名把人工改动洗成系统内容。
    pub(crate) fn finalize_relinked(content: &str, updated: String) -> Result<String> {
        if !matches!(
            crate::topic::ownership_of(content),
            crate::topic::TopicOwnership::AiOwned
        ) {
            return Ok(updated);
        }
        let mut meta = crate::frontmatter::read_luna(&updated);
        meta.synth_body =
            Some(ContentHash::of(crate::frontmatter::body_of(&updated).as_bytes()).to_string());
        crate::frontmatter::update_luna(&updated, &meta)
    }

    /// 把一份改写计划写到新位置：覆盖前留快照（§4.7），原子写，自写登记，重索引。
    /// 改写前的预检：有一篇写不进去就整次不做（[`VaultError::NotWritable`]，列出是哪几篇）。
    /// 原子写要在同目录建临时文件再换名：文件可写而目录只读，一样写不进去。
    pub(crate) fn ensure_writable<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let dir_writable = |abs: &Path| {
            abs.parent()
                .and_then(|dir| std::fs::metadata(dir).ok())
                .is_some_and(|meta| !meta.permissions().readonly())
        };
        let blocked: Vec<String> = paths
            .into_iter()
            .filter(|rel| {
                let abs = self.root.join(rel);
                std::fs::OpenOptions::new().write(true).open(&abs).is_err() || !dir_writable(&abs)
            })
            .map(str::to_owned)
            .collect();
        if blocked.is_empty() {
            Ok(())
        } else {
            Err(VaultError::NotWritable(blocked.join("、")))
        }
    }

    /// 版本快照库（crate 内：库外改名修复要让版本链随改名迁移）。
    pub(crate) fn snapshot_store(&self) -> &SnapshotStore {
        &self.snapshots
    }

    pub(crate) fn write_planned(&self, edit: &crate::relink::PlannedRewrite) -> Result<()> {
        #[cfg(test)]
        if FAIL_PLANNED_WRITE.with(|fail| fail.borrow().as_deref() == Some(edit.new_rel.as_str())) {
            return Err(VaultError::Io(std::io::Error::other(
                "injected write failure",
            )));
        }
        let abs = self.root.join(&edit.new_rel);
        let _ = self
            .snapshots
            .snapshot(&edit.new_rel, edit.before.as_bytes());
        crate::atomic::write_atomic_batched(&abs, edit.after.as_bytes())?;
        if let Ok(mut g) = self.guard.lock() {
            g.record_write(&abs, ContentHash::of(edit.after.as_bytes()));
        }
        self.index
            .index_note(&edit.new_rel, &edit.after, file_mtime_ms(&abs))?;
        Ok(())
    }

    /// 删除一篇笔记或文件夹（§4.6）。文件操作成功后再移除派生索引；失败时保留索引，避免 UI 提前消失。
    pub fn delete(&self, rel: &str, mode: DeleteMode) -> Result<()> {
        let abs = self.resolve(rel)?;
        if !abs.exists() {
            return Err(VaultError::NotFound(rel.into()));
        }
        let is_dir = abs.is_dir() && !rel.ends_with(".md");
        // **先算清"这一篇带走什么"，再动手**：算的时候笔记还在，它的 frontmatter 与正文
        // 链接才读得到（PRD-06-012 生命周期）。删完再算就只剩一堆没人认领的文件——那正是
        // 用户看到的那一屏孤儿音频（2026-09-02 真机）。
        let owned = self
            .owned_assets_for_delete(rel, is_dir)
            .unwrap_or_default();
        let owner = self.remove_entry(rel, &abs, is_dir, mode, None)?;
        // 附件随笔记走，并**认领同一条归属**：回收站里只出现"这篇笔记"一行，还原它时
        // 附件一起回来。删除失败的那一个只是留下一个孤儿（与本条落地前的状态一样），
        // 不该让"笔记已经删掉了"这件事回滚成一个错误。
        for asset in owned {
            let asset_abs = self.root.join(&asset);
            if asset_abs.is_file() {
                let _ = self.remove_entry(&asset, &asset_abs, false, mode, owner.as_deref());
            }
        }
        if let Ok(mut g) = self.guard.lock() {
            if is_dir {
                g.forget_dir(&abs);
            } else {
                g.forget(&abs);
            }
        }
        if is_dir {
            self.index.remove_directory(&normalize_rel(rel))?;
        } else {
            self.index.forget_note(&normalize_rel(rel))?;
        }
        Ok(())
    }

    pub(crate) fn vault_trash_dir(&self) -> PathBuf {
        self.root.join(".trash")
    }

    fn trash_index_path(&self) -> PathBuf {
        self.vault_trash_dir().join(".index.json")
    }

    /**
     * 读回收站索引。
     *
     * **索引损坏时不能静默当成空的**：`.index.json` 一旦解析不了，此前这里直接
     * `unwrap_or_default()` 返回空表——回收站看起来空空如也，而文件还整整齐齐躺在
     * `.trash/` 里；更糟的是**下一次删除会把这份空表写回磁盘**，那些条目就此永久失去
     * 可还原性，也永远不再出现在任何列表里。一个安全网最不该有的失效方式，就是它坏了
     * 却装作一切正常。
     *
     * 现在坏文件先**留档**（`.index.corrupt-<时刻>.json`），再扫 `.trash/` 把还在的对象
     * 重建成条目。重建出来的条目**原路径未知**（那份信息只存在于坏掉的索引里），以空
     * `original_path` 标记：界面据此说清"不知道它原来在哪儿"，并**不给还原**——还不知道
     * 目的地就往库里写，比让用户自己去访达里取回糟得多（见 [`Vault::restore`] 的守卫）。
     */
    pub(crate) fn load_trash_index(&self) -> Vec<TrashEntry> {
        let path = self.trash_index_path();
        let Ok(bytes) = std::fs::read(&path) else {
            // 没有这个文件 = 还没删过任何东西，这是正常状态，不是损坏。
            return Vec::new();
        };
        if let Ok(entries) = serde_json::from_slice::<Vec<TrashEntry>>(&bytes) {
            return entries;
        }
        let archived = self
            .vault_trash_dir()
            .join(format!(".index.corrupt-{}.json", now_ms()));
        let _ = std::fs::rename(&path, &archived);
        let rebuilt = self.rebuild_trash_index_from_disk();
        let _ = self.save_trash_index(&rebuilt);
        rebuilt
    }

    /// 扫 `.trash/` 重建条目（索引损坏后的降级恢复）。
    ///
    /// 点开头的一律跳过：索引本身、留档的坏索引，以及任何将来加进来的内务文件。
    /// 删除时刻取文件 mtime——搬进回收站是一次 `rename`，mtime 就是那一刻。
    fn rebuild_trash_index_from_disk(&self) -> Vec<TrashEntry> {
        let dir = self.vault_trash_dir();
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut entries: Vec<TrashEntry> = read
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().to_string_lossy().into_owned();
                if id.starts_with('.') {
                    return None;
                }
                let mtime = file_mtime_ms(&dir.join(&id));
                Some(TrashEntry {
                    id,
                    // 空 = 原路径未知；界面据此不给「还原」（`Vault::restore` 也会拒绝）。
                    original_path: String::new(),
                    deleted_ms: if mtime > 0 { mtime as u64 } else { now_ms() },
                    owner: None,
                })
            })
            .collect();
        entries.sort_by_key(|entry| entry.deleted_ms);
        entries
    }

    fn save_trash_index(&self, entries: &[TrashEntry]) -> Result<()> {
        std::fs::create_dir_all(self.vault_trash_dir())?;
        let bytes =
            serde_json::to_vec_pretty(entries).map_err(|e| VaultError::Decode(e.to_string()))?;
        write_atomic(&self.trash_index_path(), &bytes)
    }

    /// 按删除策略移除一个对象（笔记 / 文件夹 / 附件共用这一处）。
    ///
    /// 三种策略的分支此前只写在 [`Vault::delete`] 里，于是"附件也按同一策略走"这件事
    /// 无处可依——附件要么被永久删（与用户设置不符），要么各写一份分支。
    /// 返回库内回收站的条目 id（仅 `VaultTrash` 策略有；另两种没有可还原的条目）。
    fn remove_entry(
        &self,
        rel: &str,
        abs: &Path,
        is_dir: bool,
        mode: DeleteMode,
        owner: Option<&str>,
    ) -> Result<Option<String>> {
        match mode {
            DeleteMode::Trash => {
                // 系统回收站按文件逐个收，还原归 Finder——附件与笔记都在那儿，不需要归属。
                trash::delete(abs)
                    .map_err(|e| VaultError::Decode(format!("系统回收站失败：{e}")))?;
            }
            DeleteMode::VaultTrash => {
                return self.move_to_vault_trash(rel, abs, owner).map(Some);
            }
            DeleteMode::Permanent => {
                if is_dir {
                    std::fs::remove_dir_all(abs)?;
                } else {
                    std::fs::remove_file(abs)?;
                }
            }
        }
        Ok(None)
    }

    /// 删除 `rel` 时**随它一起走**的本地资产（PRD-06-012 附件生命周期）。
    ///
    /// 两类，判据各自明确：
    /// - **frontmatter `media[]` 的音频与它的边车**：录音/转写笔记本就拥有它们（那是这篇
    ///   笔记的录音，不是共享素材），故无条件跟随；
    /// - **正文链接到的本地非 md 文件，且没有第二篇笔记引用它**（`collect_other_attachment_refs`
    ///   的同一判据）。共享的留下——把别人还在用的图片一并删掉，比留下一个孤儿糟得多。
    ///
    /// **与 [`Vault::move_`] 的"独占才跟随"逐字同义**：移动会带走的那些，删除也带走。
    /// 两边判据一旦分叉，用户会遇到"移动带走了、删除却留下"这种解释不了的差别。
    ///
    /// 已经在被删子树里的资产不重复列出（它们随目录一起没了）。
    fn owned_assets_for_delete(&self, rel: &str, is_dir: bool) -> Result<Vec<String>> {
        let root_rel = normalize_rel(rel);
        let notes: Vec<String> = if is_dir {
            let prefix = format!("{root_rel}/");
            self.walk_markdown()?
                .into_iter()
                .filter(|path| path.starts_with(&prefix))
                .collect()
        } else if root_rel.ends_with(".md") {
            vec![root_rel.clone()]
        } else {
            // 附件自己被删：它不拥有任何东西。
            return Ok(Vec::new());
        };
        if notes.is_empty() {
            return Ok(Vec::new());
        }

        let deleted: std::collections::HashSet<&String> = notes.iter().collect();
        let world = self.link_world()?;
        let survivors: Vec<String> = self
            .walk_markdown()?
            .into_iter()
            .filter(|path| !deleted.contains(path))
            .collect();
        let others = self.link_targets_of(&world, survivors.iter().map(String::as_str));

        let inside_deleted =
            |path: &str| is_dir && (path == root_rel || path.starts_with(&format!("{root_rel}/")));
        let mut owned: Vec<String> = Vec::new();
        let push = |path: String, owned: &mut Vec<String>| {
            if !inside_deleted(&path) && !owned.contains(&path) {
                owned.push(path);
            }
        };
        for note in &notes {
            let Ok(doc) = self.read(note) else {
                continue;
            };
            for media in &doc.meta.media {
                let audio = normalize_rel(&media.path);
                if let Some(sidecar) = media.segments.as_deref() {
                    push(normalize_rel(sidecar), &mut owned);
                }
                push(audio, &mut owned);
            }
            let mut targets: Vec<String> = self
                .link_targets_of(&world, [note.as_str()])
                .into_iter()
                .collect();
            targets.sort();
            for target in targets {
                // 笔记之间的链接不是附件；共享附件留给还在引用它的那几篇。
                if target.ends_with(".md") || others.contains(&target) {
                    continue;
                }
                if self.root.join(&target).is_file() {
                    push(target, &mut owned);
                }
            }
        }
        Ok(owned)
    }

    /// 把一个对象移进库内回收站，返回它的条目 id（附件用它认领自己的归属）。
    fn move_to_vault_trash(&self, rel: &str, abs: &Path, owner: Option<&str>) -> Result<String> {
        let id = uuid::Uuid::now_v7().to_string();
        let dir = self.vault_trash_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::rename(abs, dir.join(&id))?;
        let mut entries = self.load_trash_index();
        entries.push(TrashEntry {
            id: id.clone(),
            original_path: normalize_rel(rel),
            deleted_ms: now_ms(),
            owner: owner.map(ToOwned::to_owned),
        });
        self.save_trash_index(&entries)?;
        Ok(id)
    }

    /// 列出 `.trash/` 回收站条目（VaultTrash 策略，§4.6 / PRD-06-009）。
    ///
    /// **只列用户自己删掉的那些**：随笔记一起进来的附件（`owner` 有值）不单独出现——
    /// 否则删一篇录音之后，回收站里会多出一行认不出的 uuid 音频，而用户从没删过"它"。
    /// 还原/永久删除那一条时它们一并跟着走（见 [`Vault::restore`] / [`Vault::purge`]）。
    ///
    /// **最近删除的排在最前**。索引是按删除顺序追加的，直接列出来就是"最早删的在最
    /// 上面"——与访达、Notion、Drive 全部相反，而用户来回收站找的几乎总是刚刚那一条。
    ///
    /// 每条顺带量出体积与"是不是文件夹"（见 [`TrashListEntry`]）。
    #[must_use]
    pub fn list_trash(&self) -> Vec<TrashListEntry> {
        let dir = self.vault_trash_dir();
        let mut listed: Vec<TrashListEntry> = self
            .load_trash_index()
            .into_iter()
            .filter(|entry| entry.owner.is_none())
            .map(|entry| {
                let abs = dir.join(&entry.id);
                TrashListEntry {
                    is_dir: abs.is_dir(),
                    byte_size: crate::diskusage::path_bytes(&abs, &[]),
                    id: entry.id,
                    original_path: entry.original_path,
                    deleted_ms: entry.deleted_ms,
                }
            })
            .collect();
        listed.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms).then(b.id.cmp(&a.id)));
        listed
    }

    /// 回收站条目预览（PRD-06-009：永久删除前判断内容，避免仅凭文件名误删）。
    ///
    /// 文本笔记读正文（按上限截断至字符边界）、文件夹列子项、其余按二进制只给体积。
    ///
    /// # Errors
    /// 条目不存在或元数据读取失败时返回错误。
    pub fn trash_preview(&self, id: &str) -> Result<TrashPreview> {
        let entries = self.load_trash_index();
        let entry = entries
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| VaultError::NotFound(self.vault_trash_dir().join(id)))?;
        let abs = self.vault_trash_dir().join(id);
        let meta = std::fs::metadata(&abs)?;

        if meta.is_dir() {
            let mut children = Vec::new();
            collect_trash_children(&abs, &abs, &mut children);
            let truncated = children.len() >= TRASH_PREVIEW_MAX_CHILDREN;
            children.sort();
            return Ok(TrashPreview {
                id: entry.id.clone(),
                original_path: entry.original_path.clone(),
                kind: TrashPreviewKind::Folder,
                content: None,
                children,
                byte_size: 0,
                truncated,
            });
        }

        let byte_size = meta.len();
        if is_previewable_text(&entry.original_path) {
            // read_to_string 失败（非 UTF-8）即当二进制降级，不 error。
            if let Ok(full) = std::fs::read_to_string(&abs) {
                let truncated = full.len() > TRASH_PREVIEW_MAX_BYTES;
                let content = if truncated {
                    let mut end = TRASH_PREVIEW_MAX_BYTES;
                    while !full.is_char_boundary(end) {
                        end -= 1;
                    }
                    full[..end].to_string()
                } else {
                    full
                };
                return Ok(TrashPreview {
                    id: entry.id.clone(),
                    original_path: entry.original_path.clone(),
                    kind: TrashPreviewKind::Note,
                    content: Some(content),
                    children: Vec::new(),
                    byte_size,
                    truncated,
                });
            }
        }

        Ok(TrashPreview {
            id: entry.id.clone(),
            original_path: entry.original_path.clone(),
            kind: TrashPreviewKind::Binary,
            content: None,
            children: Vec::new(),
            byte_size,
            truncated: false,
        })
    }

    /// 还原一条回收站条目到原路径（连带索引）。
    ///
    /// `conflict` 决定落点被占时怎么办（[`RestoreConflict`]）：`Fail` 什么都不动并回
    /// `AlreadyExists`（界面据此去问用户），`KeepBoth` 取一个唯一名还原为副本、原地那个
    /// 文件一根手指都不碰。返回**实际**落点的库内相对路径——副本形态下它与原路径不同，
    /// 调用方要按它去打开笔记。
    ///
    /// **随它进来的附件不跟着改名**：附件落点被占时仍留在回收站（原有行为）。给附件
    /// 也取个唯一名，等于让笔记正文里的链接指向一个不存在的路径——那是把"还原"办成了
    /// "还回来一篇打不开的笔记"。
    pub fn restore(&self, id: &str, conflict: RestoreConflict) -> Result<String> {
        let mut entries = self.load_trash_index();
        let pos = entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| VaultError::NotFound(self.vault_trash_dir().join(id)))?;
        let mut entry = entries.remove(pos);
        // 原路径未知（索引损坏后重建出来的条目）**一律拒绝还原**：空路径 `resolve`
        // 出来就是库根，往那儿 rename 等于把一个 uuid 文件甩进库里、甚至撞上库根本身。
        // 界面上这类条目不给「还原」，这里是最后一道闸。
        if entry.original_path.is_empty() {
            entries.insert(pos, entry);
            return Err(VaultError::NotFound(self.vault_trash_dir().join(id)));
        }
        let mut to_abs = self.resolve(&entry.original_path)?;
        if to_abs.exists() {
            match conflict {
                RestoreConflict::Fail => {
                    entries.insert(pos, entry);
                    self.save_trash_index(&entries)?;
                    return Err(VaultError::AlreadyExists(to_abs));
                }
                RestoreConflict::KeepBoth => {
                    entry.original_path = self.unique_vault_path(&entry.original_path);
                    to_abs = self.resolve(&entry.original_path)?;
                }
            }
        }
        if let Some(parent) = to_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(self.vault_trash_dir().join(id), &to_abs)?;
        // 随它一起进来的附件一起回去（PRD-06-012）：还原一篇录音却把音频留在回收站，
        // 等于还给用户一篇打不开的笔记。落点被占的那一个留在回收站，不覆盖任何东西。
        entries.retain(|entry| {
            if entry.owner.as_deref() != Some(id) {
                return true;
            }
            let Ok(asset_abs) = self.resolve(&entry.original_path) else {
                return true;
            };
            if asset_abs.exists() {
                return true;
            }
            if let Some(parent) = asset_abs.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    return true;
                }
            }
            std::fs::rename(self.vault_trash_dir().join(&entry.id), &asset_abs).is_err()
        });
        self.save_trash_index(&entries)?;

        let is_dir = to_abs.is_dir() && !entry.original_path.ends_with(".md");
        if is_dir {
            // 文件夹还原：递归扫描并重建所有子 Markdown 笔记的派生索引
            let mut md_files = Vec::new();
            if walk_md(&self.root, &to_abs, &mut md_files).is_ok() {
                for rel in &md_files {
                    let abs_path = self.root.join(rel);
                    if let Ok(content) = std::fs::read_to_string(&abs_path) {
                        let _ = self
                            .index
                            .index_note(rel, &content, file_mtime_ms(&abs_path));
                    }
                }
            }
        } else if let Ok(content) = std::fs::read_to_string(&to_abs) {
            self.index
                .index_note(&entry.original_path, &content, file_mtime_ms(&to_abs))?;
        }
        Ok(entry.original_path)
    }

    /// 从 `.trash/` 里真正抹掉一个条目——**文件与目录分派**。
    ///
    /// 这一处此前只有 `std::fs::remove_file`，而**文件夹同样会被搬进回收站**
    /// （`move_to_vault_trash` 对目录一样是 `rename`，`trash_preview` 也明确处理
    /// `is_dir`）：删一个文件夹之后点「永久删除」，`remove_file` 对目录必然失败、
    /// 错误被 `let _ =` 吞掉、索引条目照样移除——那个目录**从此不出现在任何列表里，
    /// 也再也删不掉，永久占着磁盘**，而界面报的是成功。
    ///
    /// 用 `symlink_metadata` 而非 `metadata`：落在回收站里的符号链接要删的是链接本身，
    /// 跟着它去 `remove_dir_all` 目标目录会删掉库外的东西。
    ///
    /// 已经不在了按**已达成**处理（与 `vault_delete` 对"对象已不存在"的口径逐字一致）。
    fn remove_trash_object(&self, id: &str) -> std::io::Result<()> {
        let path = self.vault_trash_dir().join(id);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 按保留期裁掉过期条目（PRD-06-009 保留策略）。`max_age_ms = None` = 永不自动删除。
    ///
    /// **只看用户自己删的那一条的时间**：随笔记进来的附件跟着主条目走（`owner` 有值），
    /// 它们的去留由 [`Vault::purge`] 一并处理——按各自的时间单独裁，会把一篇还在保留期内
    /// 的笔记的音频先删掉，还回来一篇打不开的录音。
    ///
    /// 惰性执行：列一次回收站顺带裁一次。没有后台定时器——一台几个月没开的设备，回收站
    /// 该在用户下次打开它的时候就是干净的，而不是等某个循环转到。
    pub fn prune_trash(&self, max_age_ms: Option<i64>, now_ms: i64) -> Result<PurgeOutcome> {
        let mut outcome = PurgeOutcome::default();
        let Some(max_age_ms) = max_age_ms else {
            return Ok(outcome);
        };
        let cutoff = now_ms.saturating_sub(max_age_ms);
        let expired: Vec<String> = self
            .load_trash_index()
            .into_iter()
            .filter(|entry| entry.owner.is_none() && (entry.deleted_ms as i64) < cutoff)
            .map(|entry| entry.id)
            .collect();
        for id in expired {
            let one = self.purge(Some(&id))?;
            outcome.purged += one.purged;
            outcome.failed.extend(one.failed);
        }
        Ok(outcome)
    }

    /// 这一条还有几天到期（`max_age_ms = None` → `None`，即不会自动删除）。
    ///
    /// 向上取整：还剩 0.2 天时说「还有 1 天」而不是「还有 0 天」——后者读起来像"已经到期
    /// 了却还在这儿"。已过期（尚未被裁到）给 0。
    #[must_use]
    pub fn trash_days_left(deleted_ms: u64, max_age_ms: Option<i64>, now_ms: i64) -> Option<u32> {
        let max_age_ms = max_age_ms?;
        let elapsed = now_ms.saturating_sub(deleted_ms as i64);
        let remaining = max_age_ms.saturating_sub(elapsed);
        if remaining <= 0 {
            return Some(0);
        }
        let day = 24 * 60 * 60 * 1_000i64;
        // 手写向上取整：`i64::div_ceil` 在本仓的工具链上还是 unstable。
        Some(((remaining + day - 1) / day) as u32)
    }

    /// 清空回收站（`None`）或清除单条（永久删除）。
    ///
    /// **失败不再被吞掉，失败项也不从索引里除名**：删不掉却把它从索引里划掉，等于
    /// 亲手制造一个既看不见、又删不掉的幽灵。留在索引里的那一条界面上照旧可见、可重试，
    /// 而调用方拿到 `failed` 就能如实说「N 项未能删除」，不再把失败报成成功。
    pub fn purge(&self, id: Option<&str>) -> Result<PurgeOutcome> {
        let mut entries = self.load_trash_index();
        let mut outcome = PurgeOutcome::default();
        // 随它进来的附件一并清掉：留下的话，它们再也不会出现在任何列表里
        // （`list_trash` 按归属过滤），只是永远占着磁盘。
        let targeted = |entry: &TrashEntry| match id {
            Some(id) => entry.id == id || entry.owner.as_deref() == Some(id),
            None => true,
        };
        let mut failed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in entries.iter().filter(|entry| targeted(entry)) {
            match self.remove_trash_object(&entry.id) {
                Ok(()) => outcome.purged += 1,
                Err(_) => {
                    outcome.failed.push(entry.original_path.clone());
                    failed_ids.insert(entry.id.clone());
                }
            }
        }
        entries.retain(|entry| !targeted(entry) || failed_ids.contains(&entry.id));
        self.save_trash_index(&entries)?;
        Ok(outcome)
    }

    /// 搜索（委派派生索引，§3）。纯本地、不耗 Token。排除路径里的笔记不出现（V4a）。
    pub fn search(&self, q: &Query) -> Result<Vec<SearchHit>> {
        let excluded = self.exclusions();
        let mut hits = self.index.search(q)?;
        hits.retain(|hit| !excluded.covers(&hit.path));
        Ok(hits)
    }

    /// 列出所有标签及其出现次数（委派派生索引）。
    /// 按类型列出笔记（modified 倒序，派生索引路径；如对话历史会话列表，PRD-03-013）。
    pub fn list_notes_of_type(
        &self,
        note_type: crate::frontmatter::NoteType,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::index::NoteListEntry>, Option<String>)> {
        self.index.list_by_type(note_type, limit, cursor)
    }

    /// 某一类型的笔记总数（委派派生索引；对话历史徽标用，PRD-03-013）。
    pub fn count_notes_of_type(&self, note_type: crate::frontmatter::NoteType) -> Result<u32> {
        self.index.count_by_type(note_type)
    }

    /// 速记流（PRD-06-058 A；委派派生索引）：全部 `type: quick-note` + 总数。
    ///
    /// 总数与列表**同一次调用取齐**：徽标与列表出自同一快照，不会出现「徽标说 7 篇、
    /// 点进去只有 6 行」这种两次查询之间被写盘插队的错位。
    pub fn draft_inbox(
        &self,
        query: &crate::index::DraftInboxQuery<'_>,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::index::DraftInboxEntry>, Option<String>, u32, u32)> {
        let (entries, next) = self.index.draft_inbox(query, limit, cursor)?;
        // 两个数各有各的用处，**都不能省**：`matched` 是"这份筛选下有几条"（清单页头），
        // `total` 是"全库一共有几条速记"（活动栏角标，PRD-07-066 要求它是全库权威总数，
        // 不随侧栏此刻筛成什么样而变）。只留一个的表现是：一筛选，角标就开始说谎。
        let matched = self.index.draft_count(query)?;
        let total = self.index.draft_count(&crate::index::DraftInboxQuery {
            text: None,
            filter: crate::index::DraftFilter::All,
            now_ms: query.now_ms,
        })?;
        Ok((entries, next, matched, total))
    }

    pub fn list_tags(&self) -> Result<Vec<TagCount>> {
        self.index.list_tags()
    }

    /// 标签浏览器的层级去重摘要（PRD-06-033）。
    pub fn tag_summaries(&self) -> Result<Vec<TagSummary>> {
        self.index.tag_summaries()
    }

    /// 按 `luna.type` 分组的笔记计数（PRD-07-024a ①）。分组不在此合成，见索引层注释。
    pub fn note_counts_by_type(&self) -> Result<Vec<(String, u32)>> {
        self.index.note_counts_by_type()
    }

    /// 近期活跃标签（PRD-07-024a ①）：`since_ms` 之后被改过的知识笔记里出现最多的标签。
    pub fn active_tags(&self, since_ms: i64, limit: u32) -> Result<Vec<(String, u32)>> {
        self.index.active_tags(since_ms, limit)
    }

    /// 一条值得回看的旧笔记（PRD-07-024a ①）：最久没动过、且确实够旧的那一篇。
    pub fn revisit_candidate(&self, older_than_ms: i64) -> Result<Option<(String, String)>> {
        self.index.revisit_candidate(older_than_ms)
    }

    /// Phase 11 的叶子/链接/标签簇同拍事实；调用方只可在 HMAC 前短暂持有这些字符串。
    pub fn knowledge_metrics_facts(&self) -> Result<crate::KnowledgeMetricsFacts> {
        self.index.knowledge_metrics_facts()
    }

    // ── 语义检索（PRD-16-028 / ADR-027）────────────────────────────────────────

    /// 一篇笔记此刻**该有**的分片（纯函数 + 一次读盘）。空 = 无可嵌入内容。
    pub fn chunks_of(
        &self,
        rel: &str,
        policy: crate::chunk::EmbeddingChunkPolicy,
    ) -> Result<Vec<crate::chunk::Chunk>> {
        Ok(crate::chunk::chunk_note(&self.read_note_text(rel)?, policy))
    }

    /// 这篇笔记的向量是否**已经是最新的**（增量建库的幂等门控）。
    ///
    /// 正文指纹与完整向量空间身份都要对得上；只比模型名无法识别权重、维度或策略变化。
    pub fn embeddings_fresh(
        &self,
        rel: &str,
        identity: &crate::index::EmbeddingIndexIdentity,
    ) -> Result<bool> {
        let Some(hash) = self.body_fingerprint(rel) else {
            return Ok(false);
        };
        Ok(self
            .index
            .embedding_state(rel)?
            .is_some_and(|(stored_hash, stored_identity)| {
                stored_hash == hash && stored_identity == *identity
            }))
    }

    /// 写入一篇笔记的全部向量（先删后插，见 `Index::put_embeddings` 的纪律说明）。
    pub fn put_embeddings(
        &self,
        rel: &str,
        identity: &crate::index::EmbeddingIndexIdentity,
        chunks: &[crate::chunk::Chunk],
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        // 分片与向量必须一一对应：对不上号就整篇不写。少一条会让后面每一片的向量都
        // 指向别人的内容，而那种错误在检索结果里看不出来（只是"搜得不准"）。
        if chunks.len() != vectors.len() {
            return Err(VaultError::InvalidReplace(format!(
                "向量条数（{}）与分片数（{}）不符，本篇不写入",
                vectors.len(),
                chunks.len()
            )));
        }
        let Some(hash) = self.body_fingerprint(rel) else {
            return Err(VaultError::NotFound(self.root.join(rel)));
        };
        let rows: Vec<crate::index::EmbeddingRow> = chunks
            .iter()
            .zip(vectors)
            .map(|(chunk, vector)| crate::index::EmbeddingRow {
                chunk_seq: chunk.seq,
                line_start: chunk.line_start,
                vector: vector.clone(),
                src_hash: hash.clone(),
                chunk_hash: chunk_fingerprint(&chunk.text),
                identity: identity.clone(),
            })
            .collect();
        self.index.put_embeddings(rel, &rows)
    }

    /// 一篇笔记**已在库里且仍可复用**的分片向量（键 = 分片文本指纹）。
    ///
    /// 改一行只有那一两片变了，其余原样可用。`embed_note` 据此只把缺的那几片送去嵌入。
    pub fn reusable_vectors(
        &self,
        rel: &str,
        identity: &crate::index::EmbeddingIndexIdentity,
    ) -> Result<std::collections::HashMap<String, Vec<f32>>> {
        self.index.reusable_vectors(rel, identity)
    }

    /// 建内存矩阵：读出指定模型的全部向量（切库/换模型后由调用方重建，ADR-025）。
    pub fn vector_matrix(
        &self,
        identity: &crate::index::EmbeddingIndexIdentity,
    ) -> Result<crate::semantic::VectorMatrix> {
        // 排除之前已嵌入的向量仍在库里；不滤掉的话，语义检索与「可能重复」还会把它们找出来（V4a）。
        let excluded = self.exclusions();
        let entries = self
            .index
            .all_embeddings(identity)?
            .into_iter()
            .filter(|(_, path)| !excluded.covers(path))
            .map(|(row, path)| {
                (
                    crate::semantic::VectorRow {
                        path,
                        chunk_seq: row.chunk_seq,
                        line_start: row.line_start,
                    },
                    row.vector,
                )
            })
            .collect();
        Ok(crate::semantic::VectorMatrix::build(entries))
    }

    /// 已嵌入的笔记数与分片数（设置页与建库预检如实报数）。
    pub fn embedding_counts(
        &self,
        identity: &crate::index::EmbeddingIndexIdentity,
    ) -> Result<(u32, u32)> {
        self.index.embedding_counts(identity)
    }

    /// 清掉非当前向量空间的向量。返回删掉的行数。
    pub fn prune_stale_embeddings(
        &self,
        keep: &crate::index::EmbeddingIndexIdentity,
    ) -> Result<usize> {
        self.index.prune_embeddings_except(keep)
    }

    /// 语义建库的预检盘点（全量建库确认卡据此**在点开始之前**陈述代价）。
    ///
    /// 与批量蒸馏预检同一形态：只读、不写、不跑模型。`pending` 是待嵌入的笔记路径清单——
    /// 报数字之外还要能逐篇入队，故返回路径而不只是计数。
    pub fn embed_scan(
        &self,
        identity: &crate::index::EmbeddingIndexIdentity,
        policy: crate::chunk::EmbeddingChunkPolicy,
    ) -> Result<EmbedScan> {
        let mut scan = EmbedScan::default();
        let excluded = self.exclusions();
        for rel in self.walk_markdown()? {
            if excluded.covers(&rel) {
                continue; // 排除路径（V4a）不进语义索引，也不算「待嵌入」。
            }
            let Ok(chunks) = self.chunks_of(&rel, policy) else {
                continue;
            };
            if chunks.is_empty() {
                // 无可嵌入内容（空笔记 / 只有标题 / 全是脚手架行）——既不可嵌入也不算缺失。
                scan.empty += 1;
                continue;
            }
            if self.embeddings_fresh(&rel, identity).unwrap_or(false) {
                scan.fresh += 1;
                scan.fresh_chunks += chunks.len() as u32;
            } else {
                scan.pending_chunks += chunks.len() as u32;
                scan.pending.push(rel);
            }
        }
        Ok(scan)
    }

    // ── 知识体检（PRD-16 §3.7 / `PRD-16-030/031/032`）────────────────────────
    //
    // 分层同 ADR-026 ⑦：**发现是派生的**（T0 随手重算、T1 的判定落 `index.sqlite`），
    // **忽略是权威的**（随库存 `.luna/checkup_ignores.json`）。三个方法在此收口，
    // 调用方不必知道哪一半住在哪儿。

    /// T0 即时体检（`PRD-16-030`）：纯 Rust、零 LLM，已剔除用户 ✕ 过的条目。
    ///
    /// `ai_tags_enabled` 由调用方读设置传入（`vault.graph_ai_tags`）：关着时不报
    /// 「还没有标签」——那是用户明确不要这个能力，报它就是唠叨。设置读不到 Vault 层，
    /// 故显式作为参数，比在这里假设一个默认值诚实。
    ///
    /// `now_ms` 同理由传入：待沉淀积压的陈化判据要它（PRD-06-055），而判定内核是纯函数、
    /// 不读时钟。
    pub fn checkup_t0(
        &self,
        ai_tags_enabled: bool,
        now_ms: i64,
    ) -> Result<Vec<crate::checkup::Finding>> {
        let mut rows = self.index.checkup_rows()?;
        rows.ai_tags_enabled = ai_tags_enabled;
        rows.now_ms = now_ms;
        let ignored = self.checkup_ignores()?;
        let mut findings = crate::checkup::t0_findings(&rows);
        findings.extend(crate::checkup::unreadable_folder_findings(
            &self.unreadable_folders(),
        ));
        Ok(crate::checkup::without_ignored(findings, &ignored))
    }

    /// 换上最近一次全库遍历读不了的子目录（启动扫描与 [`Self::scan`] 顺手刷新）。
    pub fn set_unreadable_folders(&self, dirs: impl IntoIterator<Item = String>) {
        if let Ok(mut current) = self.unreadable_dirs.lock() {
            *current = dirs.into_iter().collect();
            current.sort();
        }
    }

    /// 最近一次全库遍历读不了的子目录。
    #[must_use]
    pub fn unreadable_folders(&self) -> Vec<String> {
        self.unreadable_dirs
            .lock()
            .map(|dirs| dirs.clone())
            .unwrap_or_default()
    }

    /// 「多少篇知识笔记还没有任何关系」（PRD-07-024a ①「连通度」）。
    ///
    /// **与体检的孤儿页同源同数**：走 `checkup::unlinked_count`，那里与 T0 的孤儿那一趟
    /// 共用同一个判据函数与同一张图。刻意**不**在索引层另写一条「没有 links 也没有 tags」
    /// 的 SQL——那会得到一个看起来是同一件事、数字却对不上的值（空页归属、资料区排除、
    /// 全图度数三处口径都会各自漂）。
    ///
    /// **不经 `checkup_t0`**：那条路还要算另外十档发现、读忽略清单与待归入计划，
    /// 而概览只要这一个数。
    pub fn unlinked_knowledge_notes(&self) -> Result<u32> {
        let rows = self.index.checkup_rows()?;
        Ok(crate::checkup::unlinked_count(&rows))
    }

    /// 「这一篇是什么」——右侧上下文面板要的**对象级事实**（PRD-07-031）。
    ///
    /// # 两个事实源各答各的那一半，不混
    ///
    /// - **类型 / 成熟度 / 创建时间**取自**文件本身**的 frontmatter：笔记即文件，它是
    ///   唯一事实源；索引里同名的那几列是派生副本，重建索引期间可能落后。
    /// - **修改时间 / 标签**取自**索引**：前者是索引写入那一刻记下的文件 mtime，与搜索
    ///   结果、速记列表显示的是同一个数——直接读一次文件系统会更"新"，但那样同一篇笔记
    ///   在两处会显示不同的时间，正是 [PRD-07-065] 那类「同一事实说两遍且不一致」的缺陷。
    ///   后者是用户标签与 AI 标签的同一个池（`Index::note_tags` 的口径），本方法不另算。
    ///
    /// 索引里没有这一篇（刚建、尚未入索引）时，修改时间为 `None`、标签为空——**不猜**。
    ///
    /// # Errors
    /// 读不到该笔记（不存在 / 越界 / 非 UTF-8）或索引查询失败。
    pub fn note_facts(&self, rel_path: &str) -> Result<NoteFacts> {
        let doc = self.read(rel_path)?;
        let entry = self.index.note_entry(rel_path)?;
        Ok(NoteFacts {
            source_material: doc.meta.note_type.is_source_material(),
            note_type: doc.meta.note_type,
            draft: doc.meta.stage().is_draft(),
            created: doc.meta.created.clone(),
            modified_ms: entry.map(|entry| entry.modified_ms),
            tags: self.index.note_tags(rel_path)?,
            removable_tags: luna_sync_core::tags::frontmatter_tags(&doc.content),
        })
    }

    /// 深度体检的**候选声明**（`PRD-16-031` 矛盾/过时检测的输入）。
    ///
    /// 只看主题页——那是 AI 代写、带源链接的断言所在（ADR-026 ②a）；判据与 T0 失效引用
    /// 同源。每页最多取 `per_page` 条：一页三十条声明全送去判，等于把一次体检变成一小时
    /// 的模型占用，而前几条已足以回答"这一页还站得住吗"。
    pub fn topic_claims(&self, per_page: usize) -> Result<Vec<(String, crate::checkup::Claim)>> {
        let rows = self.index.checkup_rows()?;
        let pairs: Vec<(&str, &str)> = rows
            .notes
            .iter()
            .map(|n| (n.path.as_str(), n.title.as_str()))
            .collect();
        let index = luna_sync_core::links::LinkIndex::build(pairs).with_aliases(
            rows.aliases
                .iter()
                .map(|(path, alias)| (path.as_str(), alias.as_str())),
        );
        let mut out = Vec::new();
        for note in rows.notes.iter().filter(|n| n.is_topic) {
            let Ok(doc) = self.read(&note.path) else {
                continue;
            };
            let offset = crate::frontmatter::body_base_line(&doc.content) as u32 + 1;
            for claim in
                crate::checkup::claims_of(&note.path, body_of(&doc.content), offset, &index)
                    .into_iter()
                    .take(per_page)
            {
                out.push((note.path.clone(), claim));
            }
        }
        Ok(out)
    }

    /// 近重复页（`PRD-16-031`）：把语义内核算出的分片级近重复对，筛成**知识笔记之间**的
    /// 发现。资料区不参与——同一场会议的两份转写高度相似是它们的正常形态，不是问题。
    #[must_use]
    pub fn near_duplicate_findings(
        &self,
        pairs: &[crate::semantic::NearDuplicate],
    ) -> Vec<crate::checkup::Finding> {
        let is_knowledge = |rel: &str| {
            self.index
                .note_entry(rel)
                .ok()
                .flatten()
                .and_then(|entry| NoteType::from_key(&entry.note_type))
                .is_some_and(|t| !t.is_source_material())
        };
        pairs
            .iter()
            .filter(|pair| is_knowledge(&pair.left) && is_knowledge(&pair.right))
            .map(|pair| crate::checkup::Finding {
                id: crate::checkup::Finding::make_id(
                    crate::checkup::FindingKind::NearDuplicate,
                    &[&pair.left, &pair.right],
                ),
                kind: crate::checkup::FindingKind::NearDuplicate,
                paths: vec![pair.left.clone(), pair.right.clone()],
                line: None,
                subject: String::new(),
                reason: String::new(),
                count: 0,
            })
            .collect()
    }

    /// 深度体检报告：读回落盘的发现，**逐条现场校验它是否仍成立**，不成立的当场删掉。
    ///
    /// 校验不能省：矛盾/过时指认的是主题页上的**某一条声明**，而用户看完报告的第一件事
    /// 多半就是去把它改掉——改完还继续报警，等于让体检自己制造待办。判据是"那条原文还在
    /// 不在这一页里"（`subject`），近重复则是"两篇都还在不在"。
    pub fn checkup_deep_findings(&self) -> Result<Vec<crate::checkup::Finding>> {
        let ignored = self.checkup_ignores()?;
        let mut out = Vec::new();
        for finding in self.index.checkup_findings()? {
            let alive = match finding.kind {
                crate::checkup::FindingKind::NearDuplicate => {
                    finding.paths.iter().all(|rel| self.read(rel).is_ok())
                }
                _ => finding.paths.first().is_some_and(|topic| {
                    self.read(topic).is_ok_and(|doc| {
                        finding.subject.is_empty()
                            || body_of(&doc.content).contains(finding.subject.trim())
                    })
                }),
            };
            if alive {
                out.push(finding);
            } else {
                self.index.delete_checkup_finding(&finding.id)?;
            }
        }
        let mut out = crate::checkup::without_ignored(out, &ignored);
        crate::checkup::sort_findings(&mut out);
        Ok(out)
    }

    /// 一次新的深度体检开跑：翻篇（报告的语义是「最近一次看到的」）。
    pub fn clear_checkup_findings(&self) -> Result<()> {
        self.index.clear_checkup_findings()
    }

    /// 落一条深度体检发现（边跑边落，中途取消也不丢已判出来的）。
    ///
    /// **只收 T1 的种类**（[`crate::checkup::FindingKind::is_deep`]）：T0 的五类是索引上的
    /// 几条 SQL，存下来只会多一份会过期的副本——而过期副本与现算结果并列摆在同一张报告里，
    /// 用户无从知道哪条是此刻为真的。这条不变量在这里显式拒绝，而不是靠调用方自觉。
    pub fn put_checkup_finding(
        &self,
        finding: &crate::checkup::Finding,
        created_ms: i64,
    ) -> Result<()> {
        if !finding.kind.is_deep() {
            return Err(VaultError::InvalidReplace(format!(
                "{} 是即时体检的发现，随手可算，不落派生层",
                finding.kind.key()
            )));
        }
        self.index.put_checkup_finding(finding, created_ms)
    }

    /// 用户 ✕ 过的发现 id（**权威数据**，随库存 `.luna/`——随派生索引重建而复活的
    /// 「我明明关掉了它」就是真 bug，与归入/主题页忽略同一条分层判据）。
    pub fn checkup_ignores(&self) -> Result<Vec<String>> {
        crate::meta::read_meta(&self.root, "checkup_ignores")
    }

    /// 忽略一条发现（幂等）。**同时删掉派生行**——留着它只会让下一次读报告时再筛一遍。
    pub fn ignore_checkup_finding(&self, id: &str) -> Result<()> {
        let mut list = self.checkup_ignores()?;
        if !list.iter().any(|x| x == id) {
            list.push(id.to_owned());
            crate::meta::write_meta(&self.root, "checkup_ignores", &list)?;
        }
        self.index.delete_checkup_finding(id)
    }

    /// 体检的权威状态（上次跑完 / 上次提醒过）。
    ///
    /// 随库存 `.luna/`：这是**发生过的事实**，不是可从笔记重算的派生值——落派生索引会
    /// 在一次重建后凭空"从没体检过"，于是提醒当天就再响一次。
    pub fn checkup_state(&self) -> Result<crate::checkup::CheckupState> {
        Ok(
            crate::meta::read_meta::<crate::checkup::CheckupState>(&self.root, "checkup_state")?
                .into_iter()
                .next()
                .unwrap_or_default(),
        )
    }

    /// 覆盖体检的权威状态。
    pub fn set_checkup_state(&self, state: crate::checkup::CheckupState) -> Result<()> {
        crate::meta::write_meta(&self.root, "checkup_state", &[state])
    }

    /// 上一次**完整跑完**的深度体检时刻（`None` = 从未）。
    pub fn checkup_last_run(&self) -> Result<Option<i64>> {
        Ok(Some(self.checkup_state()?.last_run_ms).filter(|ms| *ms > 0))
    }

    /// 记下这次深度体检的完成时刻（保留"上次提醒过"那一半，两者互不覆盖）。
    pub fn set_checkup_last_run(&self, at_ms: i64) -> Result<()> {
        let mut state = self.checkup_state()?;
        state.last_run_ms = at_ms;
        self.set_checkup_state(state)
    }

    /// `@` 提及的候选（PRD-16-009）：笔记 / 文件夹 / 标签三类，按 `query` 子串过滤。
    ///
    /// 三类**都从派生索引推出来**，不另建目录扫描：文件夹 = 笔记路径的各级前缀（库里
    /// 空文件夹对检索没有意义，`@` 一个空文件夹只会得到空结果）。匹配大小写不敏感，
    /// 空 `query` 即"还没打字"，返回各类前若干条供直接挑。
    ///
    /// 判定在这里而不是前端（ADR-003）：「什么算一个可提及的范围」与
    /// [`crate::retrieve::in_scope`] 是同一个问题的两半，分家必然漂移。
    pub fn mention_candidates(&self, query: &str, per_kind: usize) -> Result<Vec<MentionScope>> {
        let needle = query.trim().to_lowercase();
        let matches = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);

        let notes = self.index.all_titles(MENTION_SCAN_LIMIT)?;
        let mut folders: Vec<String> = Vec::new();
        for entry in &notes {
            // 逐级前缀：`工作/子/甲.md` → 「工作」「工作/子」，与用户在文件树上看到的一致。
            let parts: Vec<&str> = entry.path.split('/').collect();
            for depth in 1..parts.len() {
                let prefix = parts[..depth].join("/");
                if !folders.iter().any(|f| f == &prefix) {
                    folders.push(prefix);
                }
            }
        }
        folders.sort();

        let mut out: Vec<MentionScope> = Vec::new();
        out.extend(
            notes
                .iter()
                .filter(|entry| matches(&entry.title) || matches(&entry.path))
                .take(per_kind)
                .map(|entry| MentionScope {
                    kind: MentionKind::Note,
                    value: entry.path.clone(),
                    label: entry.title.clone(),
                }),
        );
        out.extend(
            folders
                .iter()
                .filter(|dir| matches(dir))
                .take(per_kind)
                .map(|dir| MentionScope {
                    kind: MentionKind::Folder,
                    value: dir.clone(),
                    label: dir.rsplit('/').next().unwrap_or(dir).to_owned(),
                }),
        );
        out.extend(
            self.list_tags()?
                .iter()
                .filter(|tag| matches(&tag.tag))
                .take(per_kind)
                .map(|tag| MentionScope {
                    kind: MentionKind::Tag,
                    value: tag.tag.clone(),
                    label: tag.tag.clone(),
                }),
        );
        Ok(out)
    }

    /// 知识产物路径（主题页 + 知识地图，ADR-096，委派派生索引）：前端路由闸口用。
    pub fn knowledge_product_paths(&self) -> Result<Vec<String>> {
        self.index.knowledge_product_paths()
    }

    /// 出链：`src_path` 引用的目标笔记路径列表（委派派生索引）。
    pub fn outgoing_links(&self, src_path: &str) -> Result<Vec<String>> {
        self.index.outgoing_links(src_path)
    }

    /// 库里此刻有这一篇吗（走派生索引，不读盘）。
    ///
    /// 与图谱的幽灵节点判定同源：链接目标解析不到就是「未创建」。外链面板据此把断链呈现成
    /// 不可点的「未创建」，而不是给一个点下去只会报错的按钮。
    #[must_use]
    pub fn note_exists(&self, rel: &str) -> bool {
        matches!(self.index.note_entry(rel), Ok(Some(_)))
    }

    /// 反向链接：指向 `rel_path` 的笔记路径列表（委派派生索引）。
    pub fn backlinks(&self, rel_path: &str) -> Result<Vec<String>> {
        self.index.backlinks(rel_path)
    }

    /// 派生索引（crate 内其它模块用；外部经 Vault 的方法取数）。
    pub(crate) fn index(&self) -> &Index {
        &self.index
    }

    /// [`luna_sync_core::links::LinkIndex`] 的两张输入表：`(路径, 标题)` 与 `(路径, 别名)`。
    pub(crate) fn link_rows(&self) -> Result<(PathPairs, PathPairs)> {
        let mut notes = self.index.note_identities()?;
        // iCloud 未下载的笔记不在索引里（读不到正文），但它是在的：指向它的链接不是断链。
        for path in self.cloud_placeholders() {
            if path.to_lowercase().ends_with(".md")
                && !notes.iter().any(|(known, _)| known == &path)
            {
                let title = luna_sync_core::links::title_of(&path);
                notes.push((path, title));
            }
        }
        Ok((notes, self.index.note_aliases()?))
    }

    /// 一批 wikilink 目标（`|` 之前的部分，可带 `#片段`）各自指向哪一篇；指不到为 `None`。
    ///
    /// 编辑器与阅读视图据此给链接着色、决定点击是打开还是新建。「指向哪一篇」与图谱、
    /// 反链、体检同一份 [`luna_sync_core::links::LinkIndex`]：前端按文件名比对一遍就是第二个答案，
    /// 而它的表现正是子目录里的笔记点不开（2026-10-03）。只有片段的 `[[#标题]]` 指向本篇。
    pub fn resolve_wikilinks(&self, src: &str, targets: &[String]) -> Result<Vec<Option<String>>> {
        let (notes, aliases) = self.link_rows()?;
        let index = luna_sync_core::links::LinkIndex::from_rows(&notes, &aliases);
        Ok(targets
            .iter()
            .map(
                |target| match luna_sync_core::links::split_wikilink_target(target).0 {
                    "" => Some(src.to_owned()),
                    note => index.resolve(src, note).map(str::to_owned),
                },
            )
            .collect())
    }

    /// `[[` 补全候选：排序、别名与同名消歧的规则只在 `luna_sync_core::links::wikilink_candidates`
    /// 一处（网页版共用，ADR-177 ⑥）；这里只把索引里的标题与别名两张表交给它。按别名命中的候选选中后
    /// 写成 `[[名字|别名]]`（Obsidian 同一写法）。
    pub fn wikilink_candidates(
        &self,
        src: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<luna_sync_core::links::WikiLinkCandidate>> {
        let (notes, aliases) = self.link_rows()?;
        Ok(luna_sync_core::links::wikilink_candidates(
            &notes, &aliases, src, query, limit,
        ))
    }

    /// 点击一条 wikilink 的落点：指向的笔记、第几行（1 起，含 frontmatter）、正文第几个
    /// 标题（0 起）。指不到时 `exists = false`，`path` 是该新建在哪（`[[目录/名字]]` 建在
    /// 那个目录，裸名字建在库根）——新建位置也是 Rust 的判断，前端不拼路径。
    ///
    /// `#标题` 按标题文字匹配（忽略大小写与首尾空白），`#^块` 按行尾的 ` ^块` 标记匹配；
    /// 片段找不到时仍打开那一篇，只是不跳行——笔记在，就不该因为标题改了名而点不开。
    pub fn resolve_wikilink(&self, src: &str, target: &str) -> Result<WikiLinkHit> {
        let (note, fragment) = luna_sync_core::links::split_wikilink_target(target);
        let resolved = self
            .resolve_wikilinks(src, &[target.to_owned()])?
            .into_iter()
            .next()
            .flatten();
        let Some(path) = resolved else {
            let bare = note.trim_start_matches(['/', '.']).trim();
            let path = if bare.to_ascii_lowercase().ends_with(".md") {
                bare.to_owned()
            } else {
                format!("{bare}.md")
            };
            return Ok(WikiLinkHit {
                path,
                exists: false,
                line: None,
                heading_index: None,
            });
        };
        let (line, heading_index) = match fragment {
            Some(fragment) => {
                let doc = self.read(&path)?;
                crate::parse::locate_fragment(&doc.content, fragment)
            }
            None => (None, None),
        };
        Ok(WikiLinkHit {
            path,
            exists: true,
            line,
            heading_index,
        })
    }

    /// 清理可重建缓存（§4.7）：只清派生索引与 `.luna/cache/`，保留 snapshots/sidecars/usage/settings。
    pub fn clear_cache(&self) -> Result<()> {
        self.index.clear()?;
        // 删行之后回收文件空间：否则 index.sqlite 体积不变，「清缓存」在数字上看不出发生过。
        self.index.vacuum()?;
        let cache = self.root.join(".luna").join("cache");
        match std::fs::remove_dir_all(&cache) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(VaultError::Io(e)),
        }
        std::fs::create_dir_all(&cache)?;
        Ok(())
    }

    /// 目录树（「笔记」视图，§5）。跳过 `.luna/` / `.trash/` 及点目录。
    pub fn tree(&self) -> Result<NoteTree> {
        self.tree_showing_hidden(false)
    }

    /// 文件树（`show_hidden` = 设备设置「显示隐藏文件」，V4b）。只有文件树这一个呈现面会
    /// 传 `true`；切换器等其余用到树的地方一律走 [`Self::tree`]，点文件不出现。
    pub fn tree_showing_hidden(&self, show_hidden: bool) -> Result<NoteTree> {
        // 主题页集合一次查齐（一条 SQL），树上逐个文件只做一次哈希查表——绝不逐篇读
        // frontmatter（那是 `notes.is_topic` 这一列存在的理由，schema v6）。
        let topics: std::collections::HashSet<String> =
            self.index.topic_pages()?.into_iter().collect();
        let knowledge_maps: std::collections::HashSet<String> =
            self.index.knowledge_maps()?.into_iter().collect();
        // 记录态同理一条 SQL 查齐（`notes.stage` 派生列，ADR-086）——树上不读 frontmatter。
        let drafts: std::collections::HashSet<String> =
            self.index.draft_paths()?.into_iter().collect();
        // 类型同理一条 SQL 查齐（`notes.type`，ADR-024）：来源轴与行内类型徽标都从它派生，
        // 而这一条与上面三条一样必须由 Rust 判完再下发（PRD-06-058 / PRD-06-048①）。
        let types = self.index.note_types()?;
        let tree = build_tree(
            &self.root,
            &self.root,
            &TreeFacts {
                topics: &topics,
                knowledge_maps: &knowledge_maps,
                drafts: &drafts,
                types: &types,
                show_hidden,
            },
            false,
        )?;
        // 建树顺手刷新 iCloud 占位清单（链接解析据此把它们当作在，[`crate::icloud`]）。
        let mut placeholders = Vec::new();
        collect_placeholders(&tree, &mut placeholders);
        self.set_cloud_placeholders(placeholders);
        let mut tree = tree;
        self.insert_sync_cloud_only(&mut tree);
        mark_excluded(&mut tree, &self.exclusions());
        Ok(tree)
    }

    /// 索引里的全部笔记，按路径排序（`luna note list`）。排除路径（V4a）照样列出——排除是
    /// 降噪，不是把文件藏起来；调用方用 [`Self::exclusions`] 自行标注。
    pub fn indexed_notes(&self) -> Result<Vec<crate::index::NoteListEntry>> {
        self.index.notes_by_path()
    }

    /// 搜索视图检索（PRD-06-031）：索引粗筛候选 → 逐篇读文件精确定位命中行。
    ///
    /// 为什么不直接用索引里的 FTS `snippet`：那份内容是**派生的**（剥了 frontmatter 与
    /// 时间码、还可能滞后于磁盘），拿它当命中区间会让替换写错位置。索引负责把候选从
    /// 一万篇收敛到几百篇，"命中在第几行第几列"必须由此刻的文件字节说了算。
    pub fn search_matches(&self, query: &SearchQuery) -> Result<SearchResults> {
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        let candidates = self.index.search_candidates(query, CANDIDATE_LIMIT)?;
        let mut results = SearchResults {
            truncated: candidates.len() >= CANDIDATE_LIMIT,
            ..Default::default()
        };
        let excluded = self.exclusions();
        // 读不出的候选（权限、IO）：不存在的照旧跳过（索引落后于磁盘，不算错），其余记下第一个——
        // 一篇都没读出来时整次报错，而不是说「没有匹配」：库读不了不等于没有（真机 2026-10-06，
        // 库目录没有读权限时搜索显示「没有匹配的笔记」）。
        let mut unreadable: Option<VaultError> = None;
        for candidate in candidates {
            if excluded.covers(&candidate.path) {
                continue; // 排除路径（V4a）：不出现在搜索里。
            }
            if results.notes.len() >= MAX_RESULT_NOTES {
                results.truncated = true;
                break;
            }
            let content = match self.read_note_text(&candidate.path) {
                Ok(content) => content,
                Err(VaultError::NotFound(_)) => continue,
                Err(error) => {
                    unreadable.get_or_insert(error);
                    continue;
                }
            };
            let Some(body) = match_note(&content, query, MAX_LINES_PER_NOTE) else {
                continue;
            };
            results.total_matches += body.match_count;
            results.notes.push(NoteMatch {
                path: candidate.path,
                title: candidate.title,
                modified_ms: candidate.modified_ms,
                note_type: candidate.note_type,
                draft: candidate.draft,
                match_count: body.match_count,
                lines: body.lines,
                lines_truncated: body.lines_truncated,
            });
        }
        if results.notes.is_empty() {
            if let Some(error) = unreadable {
                return Err(error);
            }
        }
        Ok(results)
    }

    /// 库内检索（PRD-16 §3.2）：一个自然语言问题 → 若干带锚的摘录。
    ///
    /// 编排三步（判定全在 [`crate::retrieve`] 纯函数里，本函数只做 IO 与串联）：
    /// ① 探测词逐个走索引粗筛取候选并集；② 逐篇读**此刻磁盘上的字节**精算并抽摘录；
    /// ③ 按相关度与预算装箱。`scanned` 报的是**真正读盘打过分的篇数**——界面上那句
    /// 「已检索 N 篇」必须对得上真实工作量，不能拿候选数充数。
    pub fn retrieve(&self, question: &str, options: &RetrieveOptions) -> Result<Retrieval> {
        let terms = question_terms(question);
        if terms.is_empty() {
            return Ok(Retrieval::default());
        }

        // ① 候选并集：每个探测词一次索引查询，去重后封顶。
        let mut seen = std::collections::HashSet::new();
        let mut candidates: Vec<crate::index::NoteListEntry> = Vec::new();
        let mut truncated = false;
        // `@` 点名的**具体某一篇**先落座（PRD-16-009）：用户手指着说"就看这篇"，
        // 它是否恰好含探测词无关紧要——探测词是用来"找"的，而这一篇已经被找到了。
        // 文件夹/标签不做同样的种子（那可能是几百篇，等于绕开检索把整个范围塞进上下文）。
        for entry in &options.scope {
            if let crate::retrieve::RetrieveScope::Note(path) = entry {
                if let Some(hit) = self.index.note_entry(path)? {
                    if seen.insert(hit.path.clone()) {
                        candidates.push(hit);
                    }
                }
            }
        }
        // 探测词找来的候选不含排除路径（V4a）；上面 `@` 点名的那一篇不受排除约束——点名即意图。
        let excluded = self.exclusions();
        for term in &terms.probe {
            if candidates.len() >= MAX_CANDIDATES {
                truncated = true;
                break;
            }
            let query = SearchQuery::parse(&quote_term(term), false);
            for hit in self.index.search_candidates(&query, CANDIDATES_PER_TERM)? {
                if candidates.len() >= MAX_CANDIDATES {
                    truncated = true;
                    break;
                }
                if excluded.covers(&hit.path) {
                    continue;
                }
                if seen.insert(hit.path.clone()) {
                    candidates.push(hit);
                }
            }
        }

        // ② 逐篇精算。类型不参与检索的（默认含对话笔记）与被点名剔除的先滤掉，
        //    它们不算「已检索」——报一个用户不曾受益的数字是虚报。
        let mut passages = Vec::new();
        let mut map_routes: Vec<(String, f64)> = Vec::new();
        let mut scanned = 0u32;
        // 已真正读过盘的路径（语义路补摘录时据此去重计数：读过但打分为 None 的笔记
        // 被语义命中时会再读一次，但「已检索 N 篇」只能算它一次——双计同样是虚报）。
        let mut scanned_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
        for candidate in candidates {
            let note_type = NoteType::from_key(&candidate.note_type).unwrap_or(NoteType::Note);
            // 成熟度事实取自索引列（写索引那一刻由 `NoteStage::resolve` 算过），不重判。
            let stage = NoteStage::from_index_row(note_type, candidate.draft);
            if !allows_grounding(note_type, stage, options)
                || options.excluded.iter().any(|p| p == &candidate.path)
            {
                continue;
            }
            // `@` 框定的范围（PRD-16-009）：判定在 Rust，标签逐篇现查——只对**已进候选集**
            // 的那几十篇查，不是全库扫描。范围外的不算「已检索」（它们从未被读过）。
            if !options.scope.is_empty()
                && !crate::retrieve::in_scope(
                    &candidate.path,
                    &self.index.note_tags(&candidate.path)?,
                    &options.scope,
                )
            {
                continue;
            }
            let Ok(content) = self.read_note_text(&candidate.path) else {
                continue;
            };
            // 地图命中只把真实链接目标扩展进候选；自身永不成为 passage。
            if read_luna(&content).knowledge_map {
                if let Some(map_hit) = score_note(
                    &candidate.path,
                    &candidate.title,
                    note_type,
                    stage,
                    &content,
                    &terms.score,
                ) {
                    for target in self.index.outgoing_links(&candidate.path)? {
                        map_routes.push((target, map_hit.score));
                    }
                }
                continue;
            }
            scanned += 1;
            scanned_paths.insert(candidate.path.clone());
            if let Some(passage) = score_note(
                &candidate.path,
                &candidate.title,
                note_type,
                stage,
                &content,
                &terms.score,
            ) {
                passages.push(passage);
            }
        }

        // 地图扩展沿用原命中分数；真实主题页若也被关键词路命中则只提升既有分数，不重复。
        for (target, route_score) in map_routes {
            if options.excluded.iter().any(|path| path == &target) {
                continue;
            }
            let Ok(doc) = self.read(&target) else {
                continue;
            };
            if doc.meta.knowledge_map || !doc.meta.topic {
                continue;
            }
            if !options.scope.is_empty()
                && !crate::retrieve::in_scope(
                    &target,
                    &self.index.note_tags(&target)?,
                    &options.scope,
                )
            {
                continue;
            }
            if let Some(existing) = passages.iter_mut().find(|passage| passage.path == target) {
                existing.score = existing.score.max(route_score);
                continue;
            }
            if scanned_paths.insert(target.clone()) {
                scanned += 1;
            }
            if let Some(mut passage) = score_note(
                &target,
                &luna_sync_core::links::title_of(&target),
                doc.meta.note_type,
                doc.meta.stage(),
                &doc.content,
                &terms.score,
            )
            .or_else(|| {
                crate::retrieve::passage_at_line(
                    &target,
                    &luna_sync_core::links::title_of(&target),
                    doc.meta.note_type,
                    doc.meta.stage(),
                    &doc.content,
                    1,
                    route_score as f32,
                )
            }) {
                passage.score = passage.score.max(route_score);
                passages.push(passage);
            }
        }

        // ③ 语义路（ADR-027）：调用方已算好的命中，此处只做「补齐摘录 + RRF 融合」。
        //    空 = 纯关键词，整段短路 → 行为与批次 1 逐字节一致（PRD-16-029 的降级承诺）。
        if !options.semantic.is_empty() {
            let (extra, fused_order) =
                self.fuse_semantic(&passages, options, &mut scanned, &scanned_paths)?;
            passages.extend(extra);
            let rank: std::collections::HashMap<&str, usize> = fused_order
                .iter()
                .enumerate()
                .map(|(i, p)| (p.as_str(), i))
                .collect();
            // 融合名次即最终名次；未进融合名单的（被上限截掉的尾巴）排在其后。
            passages.sort_by_key(|p| rank.get(p.path.as_str()).copied().unwrap_or(usize::MAX));
            // `pack_passages` 按 `score` 降序装箱，而两路的分数不可比——故融合之后把名次
            // **回写成分数**（名次越前分越高），让装箱沿用融合次序而不是又按各路原始分重排
            // 一次（那等于把 RRF 的结论丢掉）。
            let total = passages.len() as f64;
            for (i, passage) in passages.iter_mut().enumerate() {
                passage.score = total - i as f64;
            }
        }

        let packed = pack_passages(self.drop_superseded_material(passages), options);
        Ok(Retrieval {
            passages: self.mark_claim_notices(packed, &options.claim_notices),
            scanned,
            truncated,
        })
    }

    /// 问答读到证据格式主题页时，出处已不存在或来源已变的声明在摘录里**行内**加注
    /// （K4：问答不能把这类旧声明当作当前已核实的事实）。只改摘录文本、不增删行，段落的
    /// 锚点与行号不受影响；主题页本身一个字节不动。说明文案由调用方按界面语言给，空即不加注。
    fn mark_claim_notices(
        &self,
        passages: Vec<crate::retrieve::Passage>,
        notices: &crate::retrieve::ClaimNotices,
    ) -> Vec<crate::retrieve::Passage> {
        use crate::knowledge_evidence::EvidenceState;
        if notices.is_empty() {
            return passages;
        }
        passages
            .into_iter()
            .map(|mut passage| {
                let Ok(doc) = self.read(&passage.path) else {
                    return passage;
                };
                if !doc.meta.topic {
                    return passage;
                }
                let Ok(states) = self.evidence_states_of(&passage.path, &doc.content) else {
                    return passage;
                };
                let Ok(knowledge) = crate::validate_topic_document(&doc.content) else {
                    return passage;
                };
                // 每条声明取最严重的出处状态：不存在 > 来源已变。
                let mut stale: std::collections::HashMap<String, &str> =
                    std::collections::HashMap::new();
                for entry in &states {
                    let claim = entry
                        .id
                        .rsplit_once(':')
                        .map_or(entry.id.as_str(), |(id, _)| id);
                    let notice = match entry.state {
                        EvidenceState::Missing => notices.missing.as_str(),
                        EvidenceState::SourceChanged => notices.source_changed.as_str(),
                        _ => continue,
                    };
                    let slot = stale.entry(claim.to_owned()).or_insert(notice);
                    if entry.state == EvidenceState::Missing {
                        *slot = notice;
                    }
                }
                // 分歧两边都标：只标分歧那条，原说法在摘录里就像没人反对过。
                let mut disputed: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for (id, claim) in &knowledge.claims {
                    if let Some(peer) = &claim.conflicts_with {
                        disputed.insert(id);
                        disputed.insert(peer);
                    } else if claim.kind == crate::ClaimKind::Conflict {
                        disputed.insert(id);
                    }
                }
                // 两种说明可以同时成立（出处失效且有分歧），拼在一起，谁也不盖掉谁。
                let mut flagged: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for id in knowledge.claims.keys() {
                    let mut notice = stale.get(id).copied().unwrap_or_default().to_owned();
                    if disputed.contains(id.as_str()) {
                        notice.push_str(&notices.conflict);
                    }
                    if !notice.is_empty() {
                        flagged.insert(id.clone(), notice);
                    }
                }
                if flagged.is_empty() {
                    return passage;
                }
                // 声明块内第一行非空正文就是那条声明；摘录保留原行（只剥时间码），可逐字比对。
                let mut marked: std::collections::HashMap<String, &str> =
                    std::collections::HashMap::new();
                let mut current: Option<&str> = None;
                for line in crate::body_of(&doc.content).lines() {
                    let trimmed = line.trim();
                    if let Some(id) = trimmed
                        .strip_prefix("<!-- luna:claim:")
                        .and_then(|rest| rest.strip_suffix(" -->"))
                    {
                        current = flagged.get(id).map(String::as_str);
                        continue;
                    }
                    if trimmed == "<!-- /luna:claim -->" {
                        current = None;
                        continue;
                    }
                    if let Some(notice) = current {
                        if !trimmed.is_empty() {
                            marked.insert(line.trim_end().to_owned(), notice);
                            current = None;
                        }
                    }
                }
                passage.text = passage
                    .text
                    .lines()
                    .map(|line| match marked.get(line.trim_end()) {
                        Some(notice) => match line.strip_prefix("- ") {
                            Some(rest) => format!("- {notice}{rest}"),
                            None => format!("{notice}{line}"),
                        },
                        None => line.to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                passage
            })
            .collect()
    }

    /// **同一内容的两个层次不同时进上下文**（真机第一轮：一次问答的出处里，一篇转写与
    /// 它的蒸馏纪要各占一条，同一份内容占了两份预算、出了两条看起来重复的出处）。
    ///
    /// 判据用**既有的双向指针**（`luna.distill_note`，ADR-024）而不是猜标题：一篇资料的
    /// 蒸馏笔记也在结果里时，丢掉**资料**那一份。知识层优先是 ADR-024 的分层在检索层的
    /// 自然延伸——纪要是这份材料被读懂之后的样子，逐字稿只在"要核对原话"时才更有用，
    /// 而那条路是引用跳转（PRD-16-012），不是问答上下文。
    ///
    /// 反过来**绝不**丢：只有资料在场时它照常进上下文（那是唯一的材料来源）。
    fn drop_superseded_material(
        &self,
        passages: Vec<crate::retrieve::Passage>,
    ) -> Vec<crate::retrieve::Passage> {
        let present: std::collections::HashSet<&str> =
            passages.iter().map(|p| p.path.as_str()).collect();
        let superseded: std::collections::HashSet<String> = passages
            .iter()
            .filter(|p| NoteType::from_key(&p.note_type).is_some_and(NoteType::is_source_material))
            .filter(|p| {
                self.read(&p.path)
                    .ok()
                    .and_then(|doc| doc.meta.distill_note)
                    .is_some_and(|target| present.contains(target.as_str()))
            })
            .map(|p| p.path.clone())
            .collect();
        if superseded.is_empty() {
            return passages;
        }
        passages
            .into_iter()
            .filter(|p| !superseded.contains(&p.path))
            .collect()
    }

    /// 语义路的补齐与融合（[`Vault::retrieve`] 的第三步；拆出来只为让那个函数保持可读）。
    ///
    /// 返回「关键词路没覆盖到的那些语义命中的摘录」+「RRF 融合后的路径次序」。
    fn fuse_semantic(
        &self,
        keyword: &[crate::retrieve::Passage],
        options: &RetrieveOptions,
        scanned: &mut u32,
        already_scanned: &std::collections::HashSet<String>,
    ) -> Result<(Vec<crate::retrieve::Passage>, Vec<String>)> {
        let mut extra: Vec<crate::retrieve::Passage> = Vec::new();
        let mut semantic_order: Vec<String> = Vec::new();
        // **相对落差门槛**（`SEMANTIC_RELATIVE_FLOOR`）：本轮最高余弦的某个比例以下不参与融合。
        // 这不是 ADR-027 明令禁止的**绝对**阈值——它逐查询自适应（同 Weaviate `autocut` /
        // Vespa 的 score-drop 那一族做法）。理由是真机第一轮的现象：库里没有答案时，语义路
        // 照样把"同语言、同领域但无关"的笔记按 0.4 上下的余弦排进来，融合后进了上下文，
        // 于是回答被无关材料带偏。
        let ceiling = options
            .semantic
            .iter()
            .map(|hit| hit.score)
            .fold(f32::MIN, f32::max);
        let floor = if ceiling > 0.0 {
            ceiling * SEMANTIC_RELATIVE_FLOOR
        } else {
            f32::MIN
        };
        // 每路只让**前 W 名**参与融合（`rank_window_size` 的语义，Elasticsearch 的 `rrf`
        // retriever 同构）：RRF 是**重排**方法而不是召回过滤器——不设窗的话，融合会把某一路
        // 的长尾捞回来当出处，而它在那一路本就排在二十名开外。
        let window = rank_window(options.max_notes);
        for hit in &options.semantic {
            if semantic_order.len() >= window {
                break;
            }
            if hit.score < floor {
                continue;
            }
            if semantic_order.iter().any(|p| p == &hit.path) {
                continue; // 同一篇的多个分片：只保留它最好的那一次（RRF 的口径）
            }
            // 类型/剔除/范围三道闸门与关键词路**完全同源**——语义路不是绕过它们的后门。
            let Ok(doc) = self.read(&hit.path) else {
                continue;
            };
            let note_type = doc.meta.note_type;
            let stage = doc.meta.stage();
            if doc.meta.knowledge_map {
                // 语义命中地图时同样只扩展真实 topic links；地图自身不进次序、不占出处位。
                for target in self.index.outgoing_links(&hit.path)? {
                    if semantic_order.len() >= window
                        || semantic_order.iter().any(|path| path == &target)
                        || options.excluded.iter().any(|path| path == &target)
                    {
                        continue;
                    }
                    let Ok(target_doc) = self.read(&target) else {
                        continue;
                    };
                    if target_doc.meta.knowledge_map || !target_doc.meta.topic {
                        continue;
                    }
                    if !options.scope.is_empty()
                        && !crate::retrieve::in_scope(
                            &target,
                            &self.index.note_tags(&target)?,
                            &options.scope,
                        )
                    {
                        continue;
                    }
                    semantic_order.push(target.clone());
                    if keyword.iter().any(|passage| passage.path == target)
                        || extra.iter().any(|passage| passage.path == target)
                    {
                        continue;
                    }
                    if !already_scanned.contains(&target) {
                        *scanned += 1;
                    }
                    if let Some(passage) = crate::retrieve::passage_at_line(
                        &target,
                        &luna_sync_core::links::title_of(&target),
                        target_doc.meta.note_type,
                        target_doc.meta.stage(),
                        &target_doc.content,
                        1,
                        hit.score,
                    ) {
                        extra.push(passage);
                    }
                }
                continue;
            }
            if !allows_grounding(note_type, stage, options)
                || options.excluded.iter().any(|p| p == &hit.path)
            {
                continue;
            }
            if !options.scope.is_empty()
                && !crate::retrieve::in_scope(
                    &hit.path,
                    &self.index.note_tags(&hit.path)?,
                    &options.scope,
                )
            {
                continue;
            }
            semantic_order.push(hit.path.clone());
            // 关键词路已经给出这一篇的摘录 → 不重复抽（用它的，那一段是词命中中心）。
            if keyword.iter().any(|p| p.path == hit.path) {
                continue;
            }
            let Ok(content) = self.read_note_text(&hit.path) else {
                continue;
            };
            // 「已检索 N 篇」只对**首次**读盘计数：关键词路读过但打分为 None 的笔记
            // 走到这里会被再读一次抽摘录，但它已经被算过了——双计是虚报。
            if !already_scanned.contains(&hit.path) {
                *scanned += 1;
            }
            if let Some(passage) = crate::retrieve::passage_at_line(
                &hit.path,
                &luna_sync_core::links::title_of(&hit.path),
                note_type,
                stage,
                &content,
                hit.line_start,
                hit.score,
            ) {
                extra.push(passage);
            }
        }

        // 关键词路同样只取前 W 名进融合（同一条 `rank_window_size` 纪律；`keyword` 已按
        // 词命中分降序）。
        let keyword_order: Vec<String> = keyword
            .iter()
            .take(window)
            .map(|p| p.path.clone())
            .collect();
        let fused = crate::semantic::rrf_fuse(
            &[keyword_order, semantic_order],
            options.max_notes.max(1) * 4,
        );
        Ok((extra, fused))
    }

    /// 按复合锚在**此刻的笔记内容**里定位（PRD-16-012）：读盘 → 纯函数解析。
    pub fn resolve_citation(&self, rel: &str, anchor: &Anchor) -> Result<AnchorHit> {
        let content = self.read_note_text(rel)?;
        Ok(resolve_anchor(&content, anchor))
    }

    /// 「相关」面板数据（PRD-16-013）：未链接提及 + 相关笔记。
    ///
    /// 两者都是**只读发现**，不写任何东西——信任阶梯的第一级。
    pub fn related_notes(&self, rel: &str) -> Result<RelatedNotes> {
        let content = self.read_note_text(rel)?;
        let linked: std::collections::HashSet<String> = self
            .index
            .outgoing_links(rel)?
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        // 排除路径里的笔记不当成「可以链接过去的提及」、也不算「相关」（V4a）。
        let excluded = self.exclusions();
        let mut titles = self.index.all_titles(RELATED_TITLE_SCAN_LIMIT)?;
        titles.retain(|entry| !excluded.covers(&entry.path));
        let mentions = find_unlinked_mentions(&content, rel, &titles, &linked);

        // 相关笔记 = 标签共现（现成信号），排除自己与已链接的。
        let related = self
            .index
            .tag_neighbours(rel, RELATED_NOTES_LIMIT * 2)?
            .into_iter()
            .filter(|entry| entry.path != rel && !linked.contains(&entry.path.to_lowercase()))
            .filter(|entry| !excluded.covers(&entry.path))
            .take(RELATED_NOTES_LIMIT)
            .collect();

        Ok(RelatedNotes { mentions, related })
    }

    /// 把一处未链接提及改写为正式链接（PRD-16-013/014）。
    ///
    /// **只对知识笔记开放**：往逐字稿里插链接会污染原文（Zotero 早年「注释写进 PDF」
    /// 就是因此被移除的）。判定在此处而非前端隐藏按钮——前端隐藏只是不显示，命令仍可达。
    /// 落盘走 [`Vault::write`]，故**自动留版本快照**，改错了能回滚。
    ///
    /// 写入前重新校验目标区间此刻仍是那段文字：面板数据可能已陈旧（用户在别处改过），
    /// 按陈旧坐标改写会毁掉无关内容。
    pub fn link_mention(&self, rel: &str, mention: &UnlinkedMention) -> Result<()> {
        let doc = self.read(rel)?;
        let note_type = read_luna(&doc.content).note_type;
        if note_type.is_source_material() {
            return Err(VaultError::InvalidReplace(
                "资料笔记（录音 / 转写 / 对话）不写入链接，以免污染原文".into(),
            ));
        }
        let content = doc.content;
        let Some(line_start) = line_start_offset(&content, mention.line) else {
            return Err(VaultError::InvalidReplace("提及位置已失效".into()));
        };
        let start = line_start + mention.start;
        let end = line_start + mention.end;
        if end > content.len() || !content.is_char_boundary(start) || !content.is_char_boundary(end)
        {
            return Err(VaultError::InvalidReplace("提及位置已失效".into()));
        }
        if content[start..end] != mention.text {
            return Err(VaultError::InvalidReplace(
                "提及位置已失效：这段文字已被修改".into(),
            ));
        }
        let link = markdown_link(rel, &mention.target_path, &mention.text);
        let mut updated = String::with_capacity(content.len() + link.len());
        updated.push_str(&content[..start]);
        updated.push_str(&link);
        updated.push_str(&content[end..]);
        self.write(&doc.path, &updated)?;
        Ok(())
    }

    /// 跨库查找替换（PRD-06-014）：`dry_run` 只算计划，`false` 才逐篇写盘。
    ///
    /// `targets` 为 `None` 时作用于全部命中笔记，否则只作用于其中被点名的路径（"只替换这一篇"）。
    /// 写盘走 [`Vault::write`]，故**每篇替换前自动留快照**（PRD-06-040 协同），可逐篇回滚。
    /// 一篇写失败不中断整批——把失败如实记进 `failures`，比整批回滚更符合"已经改好的别丢"。
    pub fn replace_matches(
        &self,
        query: &SearchQuery,
        replacement: &str,
        targets: Option<&[String]>,
        dry_run: bool,
    ) -> Result<ReplacePlan> {
        let Some(needle) = query.sole_positive_term() else {
            return Err(VaultError::InvalidReplace(
                "替换需要恰好一个检索词（过滤器可照常限定范围）".into(),
            ));
        };
        let candidates = self.index.search_candidates(query, CANDIDATE_LIMIT)?;
        // 知识产物不进替换范围（ADR-096 ③）：产物只读，批量替换写它等于绕开只读语义
        // 制造一次「人工改动」，把一页系统维护的内容推进复核态。
        let products: std::collections::HashSet<String> =
            self.knowledge_product_paths()?.into_iter().collect();
        let mut plan = ReplacePlan {
            applied: !dry_run,
            truncated: candidates.len() >= CANDIDATE_LIMIT,
            ..Default::default()
        };
        for candidate in candidates {
            if products.contains(&candidate.path) {
                continue;
            }
            if let Some(list) = targets {
                if !list.iter().any(|p| p == &candidate.path) {
                    continue;
                }
            }
            if plan.notes.len() >= MAX_RESULT_NOTES {
                plan.truncated = true;
                break;
            }
            let Ok(content) = self.read_note_text(&candidate.path) else {
                continue;
            };
            // 先过完整查询（排除词 / 其它过滤器已在 SQL 侧），确保只改真正命中的笔记。
            if match_note(&content, query, 1).is_none() {
                continue;
            }
            let Some(applied) = replace_in_note(
                &content,
                needle,
                replacement,
                query.match_case,
                MAX_LINES_PER_NOTE,
            ) else {
                continue;
            };
            plan.total_matches += applied.match_count;
            if !dry_run {
                if let Err(err) = self.write(&candidate.path, &applied.content) {
                    plan.failures
                        .push((candidate.path.clone(), err.to_string()));
                    continue;
                }
            }
            plan.notes.push(ReplaceNote {
                path: candidate.path,
                title: candidate.title,
                replacement: applied,
            });
        }
        Ok(plan)
    }

    /// 读一篇笔记的原始文本（搜索/替换用；不解析 frontmatter，失败即跳过该篇）。
    fn read_note_text(&self, rel: &str) -> Result<String> {
        let abs = self.resolve(rel)?;
        // 「这篇笔记不存在」与「读盘出错」必须是两种错误，且口径与 [`Vault::read`] 同源：
        // 全归 `Io` 会让 `open_citation` 的 `SourceMissing` 终态**永远不可达**——来源被删掉时
        // 用户读到的是「暂时无法打开，请稍后重试」，一个永远不会好的"暂时"（真机第七轮实测）。
        std::fs::read_to_string(&abs).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VaultError::NotFound(rel.into())
            } else {
                VaultError::Io(e)
            }
        })
    }

    /// 版本快照列表（§4.7）。
    #[must_use]
    pub fn snapshots(&self, rel: &str) -> Vec<SnapshotMeta> {
        self.snapshots.list(&normalize_rel(rel))
    }

    /// 历史版本与**当前磁盘内容**的行级差异（PRD-06-041）。
    pub fn snapshot_diff(&self, rel: &str, id: &str) -> Result<LineDiff> {
        let old = self.read_snapshot(rel, id)?;
        let current = self.read_note_text(rel).unwrap_or_default();
        Ok(diff_lines(&old, &current))
    }

    /// 读取指定历史版本内容（只读预览，不写回文件）。
    pub fn read_snapshot(&self, rel: &str, id: &str) -> Result<String> {
        let old = self.snapshots.read_snapshot(&normalize_rel(rel), id)?;
        String::from_utf8(old)
            .map_err(|_| VaultError::NotUtf8(self.resolve(rel).unwrap_or_default()))
    }

    /// 还原到某历史版本：先对当前内容留快照（可回退本次还原），再写回历史内容（§4.7）。
    pub fn restore_snapshot(&self, rel: &str, id: &str) -> Result<()> {
        let content = self.read_snapshot(rel, id)?;
        self.write(rel, &content)
    }

    /// 自写指纹查询（供监听层判定，§4）。
    #[must_use]
    pub fn last_written_hash(&self, rel: &str) -> Option<ContentHash> {
        let abs = self.resolve(rel).ok()?;
        self.guard.lock().ok()?.last_written(&abs).cloned()
    }

    /// 解析相对路径为绝对路径，**拒绝越出 Vault 根**（`..` / 绝对路径，路径穿越防护）。
    fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let rel_path = Path::new(rel);
        for comp in rel_path.components() {
            match comp {
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(VaultError::PathEscape(rel_path.to_path_buf()));
                }
                _ => {}
            }
        }
        Ok(self.root.join(rel_path))
    }

    /// 列出全部 `.md` 相对路径（跳过点目录），供 [`Vault::scan`]。
    pub(crate) fn walk_markdown(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        walk_md(&self.root, &self.root, &mut out)?;
        Ok(out)
    }
}

/// 一篇资料笔记的蒸馏状态（PRD-06-044 / ADR-024），由 [`Vault::distill_state`] 判定。
///
/// 「已生成」与「源已更新」不是并列的两态而是**一态两标志**：一篇蒸馏笔记可以同时
/// 是「已生成」且「源已更新」且「用户编辑过」——把它们摊成互斥枚举会丢失信息，
/// 而恰恰是 `stale && edited` 这个组合决定了**绝不自动覆盖、只亮提示**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DistillState {
    /// 尚未生成（含指针悬空 = 用户删掉了蒸馏笔记）。
    Absent,
    Present {
        /// 蒸馏笔记相对路径。
        path: String,
        /// 源正文已变（蒸馏内容不再反映此刻的源）。
        stale: bool,
        /// 蒸馏笔记被用户编辑过——它已是用户资产，自动路径一律止步。
        edited: bool,
        /// **产物不完整时是哪一道闸到顶**（[`luna_core::TruncatedBy::key`]）；
        /// `None` = 正文自然写完（判据 = `luna.distill_output_truncated` 在场与否）。
        ///
        /// 它**不是** `stale` 的一种：`stale` 说的是「源变了、产物过时了」，
        /// 而这一条说的是「产物从出生那天起就缺了一截」。混进 `stale` 会让「源没变」
        /// 的那批不完整产物一个都提示不出来——而那正是现网这个缺陷的形状。
        ///
        /// 它**也不是一个布尔**：三档触顶各对应一个不同的用户动作（ADR-089 ⑧ /
        /// PRD-13 §1.5 E），把它压成 `incomplete: bool` 等于让所有人都去猜该动哪一个。
        /// 归属早就落在 frontmatter 里了，这里只是不再把它扔掉。
        output_truncated: Option<luna_core::TruncatedBy>,
        /// 生成时所用的模板键（`None` = 存量产物，调用方回退建议规则）。
        ///
        /// 它是「用户显式选择」的记录：一切重生成路径都必须沿用它，否则用户手选的
        /// 模板会被规则建议静默换掉（PRD-06-051）。
        template: Option<String>,
        /// 生成时是否附了整理稿（`None` = 存量产物，回退模板默认值）。
        full_text: Option<bool>,
    },
}

/// 触发主题页建议的最少笔记数（PRD-16-016）。三篇是「不止一次巧合」的最小样本——
/// 两篇常常只是同一件事的两种说法，四篇又太晚（那时用户已经开始手动找了）。
pub const TOPIC_SUGGESTION_MIN_NOTES: usize = 3;
/// 一次呈现的主题页建议上限（脉搏一次只给一条，多留几条供「还有 N 个」）。
/// 语义命中的**相对**落差门槛：低于「本轮最高余弦 × 这个比例」的命中不参与融合。
///
/// **刻意是相对而非绝对**：ADR-027 明令不设绝对余弦阈值（S16 负对照 top1 = 0.415，
/// 而真实次位答案 0.538，只差 0.12，任何硬阈值都会在某类查询上误杀）。相对门槛逐查询
/// 自适应，是同一族做法里唯一躲开那个问题的（Weaviate `autocut` / Vespa score-drop）。
///
/// 取 0.80 的算术依据：S16 那条真实次位答案与它同轮 top1 的比值约 0.87 —— 0.80 留了余量
/// 保住它，同时把"同语言同领域但无关"的那批（真机第一轮出处里混进来的正是这类）挡在外面。
/// 它**挡不住"整轮都没有答案"**（那一轮的 top1 本身就低）——那件事由「模型如实说
/// 摘录里没有」负责（PRD-16-007），不该靠阈值假装解决。
const SEMANTIC_RELATIVE_FLOOR: f32 = 0.80;

/// 每路参与融合的**排名窗**（`rank_window_size` 的语义，Elasticsearch 的 `rrf` retriever 同构）。
///
/// RRF 是**重排**方法，不是召回过滤器：不设窗的话，融合会把某一路的长尾捞回来当出处，
/// 而它在那一路本就排在二十名开外——真机第一轮"出处里混进无关笔记"有一半是这么来的。
/// 窗取 `max_notes` 的两倍（下限 8）：给融合留出"两路各自的前几名互相补位"的空间，
/// 又不至于让第二十名有机会进上下文。
fn rank_window(max_notes: usize) -> usize {
    (max_notes.max(1) * 2).max(8)
}

const TOPIC_SUGGESTION_LIMIT: usize = 5;

/// 一个分片文本的指纹（分片级复用的键）。与其余指纹同一口径（`ContentHash` over 字节），
/// 故全仓「内容变没变」只有一个答案。
#[must_use]
pub fn chunk_fingerprint(text: &str) -> String {
    ContentHash::of(text.as_bytes()).to_string()
}

/// 语义建库的预检盘点（PRD-16-028；与批量蒸馏预检同一形态）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmbedScan {
    /// 待嵌入的笔记（路径清单——确认卡报数、worker 逐篇入队，同一份数据）。
    pub pending: Vec<String>,
    /// 待嵌入笔记合计多少个分片（**耗时按分片算而不是按篇算**：S16 实测约 0.25 秒/片，
    /// 一篇长录音纪要能切出十几片，按篇估会把 40 分钟说成 4 分钟）。
    pub pending_chunks: u32,
    /// 向量已是最新、跳过的篇数。
    pub fresh: u32,
    /// 已最新的篇数对应的分片数（设置页「已索引 N 篇 / M 段」）。
    pub fresh_chunks: u32,
    /// 无可嵌入内容（空笔记 / 只有标题 / 整篇是脚手架行）的篇数。
    pub empty: u32,
}

/// 一条主题页建议（PRD-16-016）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicSuggestion {
    /// 出身标签。
    pub tag: String,
    /// 该标签下的知识笔记（截在门槛四倍；建议身份令牌与「看看是哪几篇」用）。
    pub notes: Vec<String>,
    /// 该标签下知识笔记的精确篇数（建议文案用）。
    pub note_count: u32,
}

/// 「未链接提及」扫描时读取的标题上限（一次面板查询的工作量天花板）。
const RELATED_TITLE_SCAN_LIMIT: usize = 5_000;
/// `@` 提及候选扫描的笔记上限（PRD-16-009）：`@` 是逐键重查的交互，与「未链接提及」那种
/// 一次性面板查询不同——上限更紧，让每次按键都稳定在毫秒级。
const MENTION_SCAN_LIMIT: usize = 2_000;

/// `@` 提及的一个候选范围（PRD-16-009）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionScope {
    /// 范围形态。
    pub kind: MentionKind,
    /// 范围值（笔记/文件夹相对路径，或标签名）——原样回传给 [`crate::RetrieveScope`]。
    pub value: String,
    /// 展示名（笔记与文件夹取末级，标签即自身）。
    pub label: String,
}

/// `@` 可提及的三类对象（PRD-16-009：库里看得见的东西，不发明第四种）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MentionKind {
    Note,
    Folder,
    Tag,
}

impl MentionKind {
    /// 稳定序列化名（IPC 契约；`Debug` 格式不得成为契约）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Folder => "folder",
            Self::Tag => "tag",
        }
    }
}
/// 「相关笔记」呈现条数上限。
const RELATED_NOTES_LIMIT: usize = 10;
/// 单篇正文中呈现的未链接提及上限（同一个名字提十遍不比提一遍更有信息）。
const MAX_MENTIONS: usize = 20;
/// 参与未链接提及匹配的最短标题长度——太短的标题（如 `a`）会在正文里遍地假命中。
const MIN_MENTION_TITLE_CHARS: usize = 2;

/// 一处未链接提及（PRD-16-013）：正文里出现了某篇笔记的标题，但没做成链接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnlinkedMention {
    /// 行号（1-based，相对整篇文件——与搜索命中行、编辑器定位同一口径）。
    pub line: u32,
    /// 命中在**该行内**的字节起点。
    pub start: usize,
    /// 命中在该行内的字节终点。
    pub end: usize,
    /// 命中的原文（= 目标笔记标题；改写时逐字节校验它还在，防按陈旧坐标改坏内容）。
    pub text: String,
    /// 目标笔记相对路径。
    pub target_path: String,
    /// 该行的完整文本（面板呈现上下文用）。
    pub line_text: String,
}

/// 「相关」面板的数据（PRD-16-013）：两者都是只读发现，不写任何东西。
#[derive(Clone, Debug, Default)]
pub struct RelatedNotes {
    /// 未链接提及（可一键转正式链接——仅知识笔记，见 [`Vault::link_mention`]）。
    pub mentions: Vec<UnlinkedMention>,
    /// 相关笔记（批次 1 = 标签共现）。
    pub related: Vec<crate::index::NoteListEntry>,
}

/// 在正文里找出「提到了别的笔记标题却没做成链接」的位置。
///
/// 三条纪律：① **跳过禁区**（时间码标记、脚手架行、已有链接与代码区间）——与索引侧同源，
/// 在这些位置改写会毁掉回放锚点或既有链接；② **最长优先**（`titles` 已按标题长度降序），
/// 「项目 X 周报」不会被「项目 X」吃掉半截；③ **同一区间只归一篇**，重叠命中先到先得。
fn find_unlinked_mentions(
    content: &str,
    self_path: &str,
    titles: &[crate::index::NoteListEntry],
    linked: &std::collections::HashSet<String>,
) -> Vec<UnlinkedMention> {
    let body = body_of(content);
    let base_line = crate::frontmatter::body_base_line(content);
    let self_title = luna_sync_core::links::title_of(self_path).to_lowercase();

    let mut out = Vec::new();
    for (offset, raw) in body.split('\n').enumerate() {
        if out.len() >= MAX_MENTIONS {
            break;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if luna_sync_core::scaffold::is_scaffold_line(line) {
            continue;
        }
        // 已经是链接的区间不再提议（含 wikilink、md 链接与行内代码）。
        let mut taken: Vec<(usize, usize)> = crate::parse::link_spans(line);
        taken.extend(luna_sync_core::scaffold::timecode_spans(line));
        let lowered = line.to_lowercase();

        for entry in titles {
            if out.len() >= MAX_MENTIONS {
                break;
            }
            let title = &entry.title;
            if entry.path == self_path
                || title.chars().count() < MIN_MENTION_TITLE_CHARS
                || title.to_lowercase() == self_title
                || linked.contains(&entry.path.to_lowercase())
            {
                continue;
            }
            let needle = title.to_lowercase();
            let mut cursor = 0usize;
            while let Some(found) = lowered[cursor..].find(&needle) {
                let start = cursor + found;
                let end = start + needle.len();
                cursor = end;
                if taken.iter().any(|(s, e)| start < *e && end > *s) {
                    continue;
                }
                taken.push((start, end));
                out.push(UnlinkedMention {
                    line: (base_line + offset + 1) as u32,
                    start,
                    end,
                    text: line[start..end].to_string(),
                    target_path: entry.path.clone(),
                    line_text: line.to_string(),
                });
                break; // 同一行同一目标只提一次
            }
        }
    }
    out
}

/// 某行（1-based，相对整篇文件）在内容中的字节起点。
fn line_start_offset(content: &str, line: u32) -> Option<usize> {
    if line == 0 {
        return None;
    }
    let mut remaining = line - 1;
    let mut offset = 0usize;
    for segment in content.split_inclusive('\n') {
        if remaining == 0 {
            return Some(offset);
        }
        offset += segment.len();
        remaining -= 1;
    }
    (remaining == 0 && offset <= content.len()).then_some(offset)
}

/// 单个检索词 → 搜索语法的短语形式（词内可能含空格/冒号，不加引号会被当过滤器解析）。
fn quote_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', ""))
}

/// [`Vault::distill_scan`] 的盘点结果：范围内资料区笔记按"要不要整理"分类（PRD-06-044）。
#[derive(Clone, Debug, Default)]
pub struct DistillScan {
    /// 待整理：从未生成，或**产物不完整**，或源已更新——且用户没编辑过蒸馏笔记。
    pub pending: Vec<String>,
    /// 已最新（跳过）。
    pub fresh: u32,
    /// 蒸馏笔记已被用户编辑（跳过——批量绝不覆盖用户资产）。
    pub edited: u32,
    /// 正文为空、无从整理（跳过）。
    pub empty: u32,
}

/// [`Vault::graph_tag_scan`] 的盘点结果：范围内笔记按 AI 关系标签的新鲜度分类。
#[derive(Clone, Debug, Default)]
pub struct GraphTagScan {
    /// 正文非空且指纹与 `luna.ai_tags_src` 不一致（含从未生成）——「只补缺口」扫这些。
    pub stale: Vec<String>,
    /// 指纹仍最新——只有「全部重新生成」才会重扫这些。
    pub fresh: Vec<String>,
    /// 正文为空的笔记数（无从提炼标签，不参与重扫）。
    pub empty: usize,
    /// **类型白名单外**的笔记数（ADR-024：资料区本就不打标）。这一档是设计，无须处置。
    pub excluded_type: usize,
    /// 落在**用户停用的文件夹**内的笔记数（PRD-06-035c）。与上一档分开计：这一档是
    /// **用户的一个设置**，而合成一个数字会让"我某次误点停用了整个 Notes"永远不可见。
    pub excluded_folder: usize,
}

/// 是否为 CJK 表意文字（含扩展 A 与兼容区）——[`tag_fits_language`] 的
/// 语言归属判据。只认表意文字：假名与谚文不在本产品的两种界面语言里，混进来只会让
/// 判据在"第三种语言"上给出一个谁都解释不了的答案。
fn is_cjk_char(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'      // CJK 统一表意文字
        | '\u{3400}'..='\u{4DBF}'    // 扩展 A
        | '\u{F900}'..='\u{FAFF}'    // 兼容表意文字
    )
}

/// 这条标签放进目标语言的库里**站不站得住**（PRD-06-035d ⑦）。
///
/// 语言盘点（[`Vault::graph_tag_language_scan`]）与注入词表的过滤用的是**同一份**判定：
/// 分家的后果是一边把 `#Datadog` 滤出词表、另一边把挂着它的笔记数成「待收敛」，两处
/// 都"看起来对"。逐段判定（层级标签的每一段独立过关，`ops/计划` 因父段不过而整条出局）：
///
/// - 段里含 CJK 表意文字 → 属中文；
/// - 段里无 CJK 但含 ASCII 大写或数字 → **专名形状**（`Datadog`、`I2X`、`MIO-One`）：
///   语言中性，两种库都站得住。产品名与缩写在中文库里保留原样是提示词明写的规则，把
///   它们判成"错语言"会两头受伤：词表把复用锚滤掉（真机第四轮坐实的代价——失去"优先
///   复用"的提醒后每篇各自发挥，标签总数 8 → 22，单篇碎标签横生），语言盘点把
///   `#Datadog #AWS` 的笔记永远数成"待收敛"，而重扫根本不会改它们。
/// - 全小写 ASCII（含 `-`/`_`）→ 可翻译的普通词：中文库里就是错语言（`ops`、
///   `architecture`——真机第三轮坐实的分裂形态恰是这一种）。
///
/// **英文库不对称**：CJK 不携带大小写，无从按形状识别专名，故含 CJK 的段在英文库
/// 一律算错语言。不对称是刻意的——硬凑对称只能靠词典白名单，而那是禁止的硬编码。
pub fn tag_fits_language(tag: &str, zh: bool) -> bool {
    tag.split('/').filter(|seg| !seg.is_empty()).all(|seg| {
        if seg.chars().any(is_cjk_char) {
            zh
        } else if zh {
            seg.chars()
                .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        } else {
            true
        }
    })
}

/// [`Vault::graph_tag_language_scan`] 的盘点结果（PRD-06-035d ②）。
#[derive(Clone, Debug, Default)]
pub struct GraphTagLanguageScan {
    /// AI 标签明确属于**另一种**语言、且会被全库重扫覆盖到的笔记数。
    pub mismatched: usize,
    /// AI 标签与目标语言一致的笔记数。用来分辨「库里语言分裂」与「库里根本还没有
    /// 标签」——后者该由「缺标签」那张卡回答，不该被说成语言不一致。
    pub matched: usize,
    /// 出现最多的几条「另一种语言」的标签（按出现篇数降序、同数按字典序，至多
    /// [`TAG_LANGUAGE_SAMPLE_LIMIT`] 条）。卡片据此**列出受影响的标签**，而不是用一段
    /// 机制说明解释"为什么要统一"（2026-10-08 C16）。
    pub sample_tags: Vec<String>,
}

/// 标签语言卡最多列几条受影响的标签：够认出问题，不把卡片撑成清单。
pub const TAG_LANGUAGE_SAMPLE_LIMIT: usize = 6;

/// 计数表里出现最多的前 `limit` 个键（同数按字典序，**纯函数**）。
fn most_frequent(counts: std::collections::BTreeMap<String, usize>, limit: usize) -> Vec<String> {
    let mut entries: Vec<(String, usize)> = counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
        .into_iter()
        .take(limit)
        .map(|(tag, _)| tag)
        .collect()
}

/// 整篇转写编辑的一条 cue 文本改动（[`Vault::update_playback_transcript`] 入参）：按
/// `(start_ms, end_ms)` 定位既有段，仅改 `text`；时间/译文/说话人结构不由本类型触碰。
#[derive(Clone, Debug)]
pub struct CueTextEdit {
    /// 段起点（会话绝对毫秒，匹配键）。
    pub start_ms: u64,
    /// 段止点（会话绝对毫秒，匹配键）。
    pub end_ms: u64,
    /// 新文本（写盘前 trim；为空即拒绝整批）。
    pub text: String,
}

/// 转写段人工校正的一次操作（[`Vault::edit_transcript_segment`] 入参，
/// PRD-04-031 / PRD-04-056）。段一律以 `(start_ms, end_ms)` 会话绝对毫秒定位
/// （与整篇编辑同一匹配键）。
#[derive(Clone, Debug)]
pub enum SegmentEdit {
    /// 重指派说话人（按 `scope` 决定作用到一条 / 连续一轮 / 全篇同说话人）。
    Assign {
        /// 段起点（匹配键）。
        start_ms: u64,
        /// 段止点（匹配键）。
        end_ms: u64,
        /// 目标 speaker id。
        speaker: String,
        /// 作用范围。
        scope: luna_core::AssignScope,
    },
    /// 在文本第 `char_offset` 个 UTF-16 码元处拆成两段（右片可同时指派说话人）。
    Split {
        /// 段起点（匹配键）。
        start_ms: u64,
        /// 段止点（匹配键）。
        end_ms: u64,
        /// 拆分点（UTF-16 码元偏移，与前端 `selectionStart` 同口径）。
        char_offset: usize,
        /// 右片说话人（`None` = 沿用左片）。
        speaker: Option<String>,
    },
    /// 与上一条合并（拆分的逆操作）。
    Merge {
        /// 段起点（匹配键）。
        start_ms: u64,
        /// 段止点（匹配键）。
        end_ms: u64,
    },
    /// 整条删除（片头广告 / 纯口水 / 静音段的凭空识别）。
    Delete {
        /// 段起点（匹配键）。
        start_ms: u64,
        /// 段止点（匹配键）。
        end_ms: u64,
    },
}

/// 一次转写写回对「权威转写」标记（`luna.transcript_authoritative`）的声明（PRD-16-047）。
///
/// 权威性只能由调用方说：同一个写回函数既服务停录后的批量重转写与导入首转（权威），也服务
/// 人工校对、重新翻译、说话人分离（不改变），录音时的实时预览则明确不是权威。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptAuthority {
    /// 不改变现有标记。
    Unchanged,
    /// 录音时的实时预览：不是权威转写。
    Preview,
    /// 批量转写已完整写回：权威转写。
    Authoritative,
}

impl TranscriptAuthority {
    fn apply(self, meta: &mut LunaMeta) {
        match self {
            Self::Unchanged => {}
            Self::Preview => meta.transcript_authoritative = false,
            Self::Authoritative => meta.transcript_authoritative = true,
        }
    }
}

/// 录音会话中的一个媒体分片（WAV rolling-split 产物，TDD-06 §2.2）。
///
/// 单次录音不做滚动分割时，`parts` 只含一项；
/// `start_ms = 0`、`end_ms = total_duration_ms` 即可。
#[derive(Clone, Debug)]
pub struct RecordingPart {
    /// WAV 相对路径（相对 Vault 根）。
    pub path: String,
    /// 此分片在录音会话中的绝对起点（毫秒）。
    pub start_ms: u64,
    /// 此分片在录音会话中的绝对止点（毫秒，不含）。
    pub end_ms: u64,
    /// 时长（秒，写入 `MediaRef.duration`）。
    pub duration: Option<f64>,
    /// 转写模型（写入 `MediaRef.model`）。
    pub model: Option<String>,
    /// 转写语种（写入 `MediaRef.lang`；会话级 lang_hint 复制到每个分片）。
    pub lang: Option<String>,
    /// 轨结构标记（写入 `MediaRef.tracks`；`"mic_system"` = 双轨立体声，PRD-04-052）。
    pub tracks: Option<String>,
}

#[cfg(test)]
thread_local! {
    /// 测试缝：让改名重构写到这一篇时失败，验证回滚（真实失败无从在单线程里稳定造出）。
    static FAIL_PLANNED_WRITE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// 一次改名 / 移动的回执（PRD-06-030 ③）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelocationReceipt {
    /// 被改写了链接（或 frontmatter 引用）的笔记，新路径。
    pub rewritten: Vec<String>,
    /// 改写了几处链接。
    pub links: usize,
    /// 撤销凭据（[`Vault::undo_relocation`]）。
    pub undo: RelocationUndo,
}

/// 撤销一次改名 / 移动所需的全部事实：不存后端状态，凭据交给调用方保管（同 ADR-135 标签撤销）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelocationUndo {
    pub from: String,
    pub to: String,
    /// 随笔记一起搬走的独占附件（旧, 新）。
    pub attachments: Vec<(String, String)>,
    /// 被改写的笔记（新路径）与改写前 / 改完时的内容指纹。
    pub notes: Vec<RelocationNoteChange>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelocationNoteChange {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
}

/// 搬一个文件或目录；只改大小写时走两段式。
fn rename_entry(from: &Path, to: &Path, case_only: bool) -> Result<()> {
    if case_only {
        rename_case_only(from, to)
    } else {
        std::fs::rename(from, to)?;
        Ok(())
    }
}

/// 回滚之后的错误：全部还原了就报原因，有没还原的就把它们列出来。
pub(crate) fn rollback_error(error: VaultError, unrestored: Vec<String>) -> VaultError {
    if unrestored.is_empty() {
        error
    } else {
        VaultError::RelocationPartial(unrestored.join("、"))
    }
}

/// 路径若在 `from` 子树内就换成 `to` 子树里的同一位置。
fn remap_prefix(path: &str, from: &str, to: &str) -> String {
    match path.strip_prefix(&format!("{from}/")) {
        Some(suffix) => format!("{to}/{suffix}"),
        None => path.to_owned(),
    }
}

/// `to` 与 `from` 同目录、名字只差大小写，且目录里并不存在另一个**逐字节同名**的条目。
///
/// 后一条区分两种文件系统：大小写不敏感时 `to.exists()` 为真只因它就是 `from`；大小写
/// 敏感时若真有另一个 `Foo.md`，那是占用，照常拒绝。
fn is_case_only_rename(from: &Path, to: &Path) -> bool {
    let (Some(from_name), Some(to_name)) = (from.file_name(), to.file_name()) else {
        return false;
    };
    if from == to || from.parent() != to.parent() {
        return false;
    }
    let (from_name, to_name) = (from_name.to_string_lossy(), to_name.to_string_lossy());
    if from_name.to_lowercase() != to_name.to_lowercase() {
        return false;
    }
    let Some(parent) = to.parent() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    !entries
        .filter_map(std::result::Result::ok)
        .any(|entry| entry.file_name().to_string_lossy() == to_name)
}

/// 只改大小写的改名经同目录临时名两段完成：部分文件系统（网络卷、FAT）把直接 rename
/// 当作空操作或报目标已存在。第二段失败时把名字还回去，不留临时名。
fn rename_case_only(from: &Path, to: &Path) -> Result<()> {
    let parent = to.parent().unwrap_or_else(|| Path::new(""));
    let temp = parent.join(format!(
        ".luna-rename-{}-{}",
        std::process::id(),
        from.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    ));
    std::fs::rename(from, &temp)?;
    if let Err(error) = std::fs::rename(&temp, to) {
        let _ = std::fs::rename(&temp, from);
        return Err(error.into());
    }
    Ok(())
}

pub(crate) fn walk_md(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // .luna / .trash / 其它点目录均跳过。
        }
        let path = entry.path();
        // 不跟随符号链接（指回祖先的链接会让递归永不结束）；读不了的子目录跳过而不是让整次
        // 遍历失败——与增量扫描（`crate::scan::walk`）同一口径。
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let _ = walk_md(root, &path, out);
        } else if kind.is_file() && name.to_lowercase().ends_with(".md") {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

/// 递归收集子目录（Vault 相对路径）。点开头的目录整棵跳过——它们不是用户内容。
fn walk_dirs(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `is_dir()` 跟随符号链接；这里刻意不跟随，避免把 Vault 外的目录树列进来。
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
        // 读不了的子目录本身照列（它确实在），只是不往下走——不让一个目录拖垮整份清单。
        let _ = walk_dirs(root, &path, out);
    }
    Ok(())
}

/// 递归列举模板 Markdown。模板目录内允许嵌套分类，但不跟随符号链接，防止把 Vault
/// 外文件暴露给模板选择器。
fn walk_user_templates(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk_user_templates(root, &path, out)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}

/// 建树时逐个文件要查的**已判定事实**（各自一条 SQL 查齐，树上只做哈希查表）。
///
/// 收成一个结构体而不是四个平行参数：它们生生死死在一起，而四个同类型的
/// `&HashSet<String>` 排在参数表里，传错顺序编译器一个字都不会说。
struct TreeFacts<'a> {
    topics: &'a std::collections::HashSet<String>,
    knowledge_maps: &'a std::collections::HashSet<String>,
    drafts: &'a std::collections::HashSet<String>,
    /// 全部笔记的 `path → 类型`：来源轴（`is_source_material`）与行内类型徽标都从它派生。
    types: &'a std::collections::HashMap<String, crate::frontmatter::NoteType>,
    /// 列不列点文件（「显示隐藏文件」，V4b）。
    show_hidden: bool,
}

fn collect_placeholders(node: &NoteTree, out: &mut Vec<String>) {
    for child in &node.children {
        if child.cloud_placeholder {
            out.push(child.path.clone());
        }
        collect_placeholders(child, out);
    }
}

/// 给树上每个节点标出覆盖它的排除规则（V4a）。
fn mark_excluded(node: &mut NoteTree, exclusions: &crate::Exclusions) {
    if exclusions.is_empty() {
        return;
    }
    node.excluded_by = exclusions.covering(&node.path).map(str::to_owned);
    for child in &mut node.children {
        mark_excluded(child, exclusions);
    }
}

/// `in_hidden`：`dir` 自己是不是隐藏项（它或某一级上级以 `.` 开头）。
fn build_tree(root: &Path, dir: &Path, facts: &TreeFacts<'_>, in_hidden: bool) -> Result<NoteTree> {
    let mut children = Vec::new();
    // 读不了的子目录显示为空文件夹，不让整棵树失败——此前一个没权限的子目录就让文件树
    // 永远停在「加载中」、状态栏报错（笔记库对标轮 V1 真机）。只有库根读不了才是错误。
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(read) => read.filter_map(|e| e.ok()).collect(),
        Err(error) if dir == root => return Err(error.into()),
        Err(_) => Vec::new(),
    };
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let dotted = name.starts_with('.');
        if dotted {
            // iCloud 未下载占位（TDD-06 §4.1 ②）：按真名列出、标成未下载；真文件已在就不重复列。
            if let Some(real) = crate::icloud::placeholder_real_name(&name) {
                let real_abs = dir.join(real);
                if !real_abs.exists() {
                    let placeholder = entry.path();
                    children.push(NoteTree {
                        name: real.to_owned(),
                        path: real_abs
                            .strip_prefix(root)
                            .unwrap_or(&real_abs)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        is_dir: false,
                        draft: false,
                        markable_as_draft: false,
                        distillable: false,
                        is_source_material: false,
                        note_type: None,
                        can_merge: false,
                        mtime_ms: file_mtime_ms(&placeholder),
                        created_ms: file_created_ms(&placeholder),
                        cloud_placeholder: true,
                        sync_cloud_only: false,
                        excluded_by: None,
                        hidden: false,
                        children: Vec::new(),
                    });
                }
                continue;
            }
            // 其余点文件只在「显示隐藏文件」时列出（V4b），Lunarca 自己的数据与 git 本体永远不列。
            if !facts.show_hidden || crate::pathutil::ALWAYS_HIDDEN.contains(&name.as_str()) {
                continue;
            }
        }
        let hidden = in_hidden || dotted;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            children.push(build_tree(root, &path, facts, hidden)?);
        } else if kind.is_symlink() && path.is_dir() {
            // 指向目录的符号链接不跟随：指回祖先的链接会让建树无限递归；与增量扫描
            // （`crate::scan::walk`）同一口径——那里面的笔记本来就不进索引。
            continue;
        } else {
            // 显示**所有**文件（Obsidian 文件浏览器对标，PRD-06-001/007）：`.md` 是笔记，
            // 其余（图片/音频/pdf…）是附件，点击进 AssetView 预览（点号前缀内部文件已在上方跳过）。
            let is_topic = facts.topics.contains(&rel);
            let is_knowledge_map = facts.knowledge_maps.contains(&rel);
            // 知识产物不进笔记树（ADR-096 ②）：主题页与知识地图是系统提炼的只读内容，
            // 应用内归「知识库」空间呈现；文件仍在磁盘原处、外部工具照常可见。过滤在
            // Rust（这里）而不是前端——身份是 frontmatter 事实，前端零业务（ADR-003）。
            if is_topic || is_knowledge_map {
                continue;
            }
            let draft = facts.drafts.contains(&rel);
            let is_markdown = rel.to_lowercase().ends_with(".md");
            let note_type = facts.types.get(&rel).copied();
            let is_source_material = note_type.is_some_and(|t| t.is_source_material());
            // 成熟度由**唯一那处判据**还原，下面两位能力都问它——手拼一次就是一处会漂的
            // 第二定义（`NoteStage::from_flags` 的说明）。
            let stage = crate::frontmatter::NoteStage::from_flags(is_source_material, draft);
            children.push(NoteTree {
                name,
                draft,
                // 「标记为记录」（PRD-06-058）：知识态的 `.md` 才给。附件没有 frontmatter、
                // 资料上这个键会被忽略——两类都不给（知识产物在上方已整体出树）。
                markable_as_draft: is_markdown && stage.allows_mark_as_draft(),
                // 「生成知识笔记」（PRD-06-044/054）：**资料 ∪ 记录**，与入队前置、worker
                // 门槛、差异预览问的是同一个函数（`NoteStage::allows_distill`）。
                distillable: is_markdown && stage.allows_distill(),
                is_source_material,
                note_type,
                // 「汇总成一篇」资格同理走共享谓词（`merge_block`，PRD-06-047 修订后只挡
                // 聚合页）：入参恒 `false` 是因为产物在上方已整体出树，不是因为这里另有
                // 一套判断。
                can_merge: is_markdown && crate::merge_block(false).is_none(),
                path: rel,
                is_dir: false,
                mtime_ms: file_mtime_ms(&path),
                created_ms: file_created_ms(&path),
                cloud_placeholder: false,
                sync_cloud_only: false,
                excluded_by: None,
                hidden,
                children: Vec::new(),
            });
        }
    }
    // 文件夹置顶 + 名称大小写不敏感（Obsidian 文件浏览器默认序，PRD-06-006）。
    children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let rel = dir
        .strip_prefix(root)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    Ok(NoteTree {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: rel,
        is_dir: true,
        draft: false,              // 成熟度是一篇笔记的属性，目录没有
        markable_as_draft: false,  // 目录没有 frontmatter 可写
        distillable: false,        // 目录没有正文可整理（整个文件夹的批量整理是另一条入口）
        is_source_material: false, // 来源轴是一篇笔记的属性
        note_type: None,
        can_merge: false, // 目录没有正文，谈不上当碎片
        mtime_ms: file_mtime_ms(dir),
        created_ms: file_created_ms(dir),
        cloud_placeholder: false,
        sync_cloud_only: false,
        excluded_by: None,
        hidden: in_hidden,
        children,
    })
}

/// 幂等确保 `.gitignore` 含 Lunarca 块（§4.1/§4.7）：无文件则新建；有则仅在缺标记时**追加**带标记块，
/// **绝不覆盖/重排用户既有内容**。`settings.toml` 属随同步项，故意不忽略。
fn ensure_gitignore(root: &Path) -> Result<()> {
    const MARKER: &str = "# >>> Lunarca >>>";
    const END_MARKER: &str = "# <<< Lunarca <<<";
    const BLOCK: &str = "\
# >>> Lunarca >>> (managed, do not edit between markers)
# 可重建缓存：不跨设备同步
.luna/index.sqlite
.luna/index.sqlite-journal
.luna/cache/
# durable·设备本地：随 Vault 但不跨设备同步（历史/用量，不可重建）
.luna/snapshots/
.luna/usage.sqlite
.luna/sidecars/
# 库身份只用来在本机查设备状态（ADR-140 ②）：进 git 的话 clone 出来的两份共用一个 id，
# 同机打开第二份时改写它就成了一处 git 改动
.luna/vault-id
# 注：.luna/settings.toml 属随同步项，故意不在此忽略
# <<< Lunarca <<<
";
    let path = root.join(".gitignore");
    match std::fs::read_to_string(&path) {
        Ok(existing) => {
            // 已有托管块：块内是我们的（标记行写明 managed, do not edit），内容变了就**只换
            // 块内**，块外用户的每一行原样保留；一样就不写。否则新规则永远到不了老库。
            if let (Some(start), Some(end)) = (existing.find(MARKER), existing.find(END_MARKER)) {
                if start < end {
                    let tail_start = existing[end..]
                        .find('\n')
                        .map_or(existing.len(), |offset| end + offset + 1);
                    if existing[start..tail_start] == *BLOCK {
                        return Ok(());
                    }
                    let out = format!("{}{BLOCK}{}", &existing[..start], &existing[tail_start..]);
                    return write_atomic(&path, out.as_bytes());
                }
            }
            if existing.contains(MARKER) {
                return Ok(()); // 标记残缺（用户删了结束行）：不猜边界，不动它。
            }
            let mut out = existing;
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(BLOCK);
            write_atomic(&path, out.as_bytes())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => write_atomic(&path, BLOCK.as_bytes()),
        Err(e) => Err(VaultError::Io(e)),
    }
}

fn file_mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// 文件出生时间；取不到（部分文件系统不记录）时回落修改时间，与 `file_mtime_ms` 同单位。
fn file_created_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(|| file_mtime_ms(path), |d| d.as_millis() as i64)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn normalize_rel(rel: &str) -> String {
    rel.replace('\\', "/")
}

fn replace_timed_text_line(content: &str, start_ms: u64, next_text: &str) -> String {
    let marker = format_timecode(std::time::Duration::from_millis(start_ms));
    let marker_prefix = format!("{marker} ");
    let body = body_of(content);
    let frontmatter_len = content.len() - body.len();
    let mut out = String::with_capacity(content.len() + next_text.len());
    out.push_str(&content[..frontmatter_len]);

    let mut replaced = false;
    for chunk in body.split_inclusive('\n') {
        let (line, newline) = if let Some(line) = chunk.strip_suffix("\r\n") {
            (line, "\r\n")
        } else if let Some(line) = chunk.strip_suffix('\n') {
            (line, "\n")
        } else {
            (chunk, "")
        };

        if !replaced && (line == marker || line.starts_with(&marker_prefix)) {
            out.push_str(&marker);
            out.push(' ');
            out.push_str(next_text);
            out.push_str(newline);
            replaced = true;
        } else {
            out.push_str(chunk);
        }
    }

    out
}

// ─── 录音转写正文块（哨兵定界，可整块替换）─────────────────────────────────────

// 转写块的起止哨兵（`rewrite_recording_transcript` 据此整块替换）与搜索、索引共用一份定义。
use luna_sync_core::scaffold::{TRANSCRIPT_BEGIN, TRANSCRIPT_END};

/// 把正文切成「便签可编辑的那一段」与「机器段」：机器段从第一个转写块哨兵开始（它前面的
/// 尾部空白一并归机器段），到正文结束。没有转写块时整段都可编辑、机器段为空。
///
/// 两段相接即原文（`text + tail == body`），写回时据此逐字节复原。
#[must_use]
pub fn sticky_text(body: &str) -> (&str, &str) {
    match body.find(TRANSCRIPT_BEGIN) {
        Some(index) => {
            let end = body[..index].trim_end().len();
            (&body[..end], &body[end..])
        }
        None => (body, ""),
    }
}

/// 由段 + 书签构建 `[HH:MM:SS.mmm]` 行标记（转写行 + 紧随的译文引用行 + 书签锚点行，
/// 全部按会话绝对 ms 稳定排序）。finalize 与 rewrite 共用，避免两处格式漂移。
/// 录音书签（PRD-04-005）：会话绝对毫秒 + 可选短备注。精确层（frontmatter `luna.bookmarks`）
/// 只存毫秒；备注只活在可读层的 `[HH:MM:SS.mmm] 🔖 备注` 行里（TDD-06 §2.1 末注）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// 会话绝对时刻（毫秒）。
    pub ms: u64,
    /// 短备注；空串按无备注处理。
    pub note: Option<String>,
}

/// 备注一行化：换行折成空格、首尾去空；空 → `None`。
fn normalize_note(note: Option<&str>) -> Option<String> {
    let text = note?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// 升序 + 按毫秒去重（同一时刻打了两次：留有备注的那一条，都有则留后者）。
fn normalize_bookmarks(bookmarks: &[Bookmark]) -> Vec<Bookmark> {
    let mut out: Vec<Bookmark> = Vec::with_capacity(bookmarks.len());
    let mut sorted: Vec<Bookmark> = bookmarks
        .iter()
        .map(|b| Bookmark {
            ms: b.ms,
            note: normalize_note(b.note.as_deref()),
        })
        .collect();
    sorted.sort_by_key(|b| b.ms);
    for b in sorted {
        match out.last_mut() {
            Some(last) if last.ms == b.ms => {
                if b.note.is_some() {
                    last.note = b.note;
                }
            }
            _ => out.push(b),
        }
    }
    out
}

/// 从正文可读层读回书签行 `[HH:MM:SS.mmm] 🔖 备注`（备注可缺）。
#[must_use]
pub fn parse_bookmark_lines(body: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some((at, rest)) = crate::timecode::parse_timecode(line.trim_end()) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix('\u{1F516}') else {
            continue;
        };
        out.push(Bookmark {
            ms: at.as_millis() as u64,
            note: normalize_note(Some(after)),
        });
    }
    out
}

/// 一篇笔记的书签全集：精确层的毫秒（事实源）+ 可读层的备注按毫秒对上；可读层里精确层
/// 没有的时刻（用户手写的 🔖 行）也算——它是用户明确写下的锚点。
#[must_use]
pub fn bookmarks_of(precise_ms: &[u64], body: &str) -> Vec<Bookmark> {
    let mut all: Vec<Bookmark> = precise_ms
        .iter()
        .map(|&ms| Bookmark { ms, note: None })
        .collect();
    all.extend(parse_bookmark_lines(body));
    normalize_bookmarks(&all)
}

fn build_timed_marker_lines(segments: &[Segment], sorted_bookmarks: &[Bookmark]) -> String {
    let mut timed_lines: Vec<(u64, String)> = Vec::new();
    for seg in segments {
        let text = seg.text.trim();
        if !text.is_empty() {
            let ms = seg.start.as_millis() as u64;
            timed_lines.push((ms, format!("{} {text}\n", format_timecode(seg.start))));
            // 译文引用行紧随源文行（同 ms + 稳定排序 → 顺序保持；时间锚点唯一归属源文行）。
            if let Some(translation) = seg.translation.as_deref().map(str::trim) {
                if !translation.is_empty() {
                    timed_lines.push((ms, format!("> {translation}\n")));
                }
            }
        }
    }
    for bookmark in sorted_bookmarks {
        let stamp = format_timecode(std::time::Duration::from_millis(bookmark.ms));
        let line = match bookmark.note.as_deref() {
            Some(note) => format!("{stamp} \u{1F516} {note}\n"),
            None => format!("{stamp} \u{1F516}\n"),
        };
        timed_lines.push((bookmark.ms, line));
    }
    timed_lines.sort_by_key(|(ms, _)| *ms);
    timed_lines.into_iter().map(|(_, line)| line).collect()
}

// ─── 语音速记的产物结构（PRD-02-033）────────────────────────────────────────────

/// 速记正文里「原始转录」那一节的标题（zh / en；`voice_quick_note_body` 写、本文件读，同一份）。
pub const QUICK_NOTE_RAW_HEADINGS: &[&str] = &["原始转录", "Raw transcript"];
/// 「优化结果 / 译文」那一节的标题（zh / en 各两种输出形态）。
pub const QUICK_NOTE_REFINED_HEADINGS: &[&str] =
    &["优化结果", "Polished text", "译文", "Translation"];

/// 速记的采用版本（frontmatter `luna.adopted`；无键 = 没表态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptedVersion {
    /// 原始转录。
    Raw,
    /// 优化 / 翻译结果。
    Refined,
    /// 二次转写（正文转写块）。
    SecondPass,
}

impl AdoptedVersion {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Refined => "refined",
            Self::SecondPass => "second_pass",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "raw" => Some(Self::Raw),
            "refined" => Some(Self::Refined),
            "second_pass" => Some(Self::SecondPass),
            _ => None,
        }
    }
}

/// 一篇速记此刻拥有的三种产物文本（缺哪种就是 `None`，不编造）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickNoteProducts {
    pub raw: Option<String>,
    pub refined: Option<String>,
    pub second_pass: Option<String>,
}

/// 从正文读回三种产物：`## 原始转录` / `## 优化结果` 两节按标题认（zh / en 两套都认），
/// 二次转写取哨兵块里的源文行（剥时间码；跳过 🔖 书签行与 `> ` 译文引用行）；
/// 正文没有任何二级标题时（纯原文落盘），**只有登记了录音的速记**整段正文才是原始转录。
///
/// 无分节正文本身说不出它是说出来的还是敲进来的：文字速记（捕获框 / 文字速记面板 /
/// 「服务」菜单）与「输出即原始转写」的语音速记落盘形状一字不差。能证明正文出自 ASR 的
/// 文件事实只有 `luna.media`（语音速记留了录音）；没有它就不认原始转录——否则每篇文字
/// 速记顶上都挂一枚「原始转写 N 字」（真机 2026-10-05）。没留录音、也没分节的语音速记
/// 因此同样不出这一枚：它只有一个版本，没有可对照、可采用的东西（PRD-02-033）。
#[must_use]
pub fn quick_note_products(content: &str) -> QuickNoteProducts {
    let body = crate::frontmatter::body_of(content);
    let (rest, block) = split_transcript_block(body);
    let second_pass = block.and_then(|block| {
        let lines: Vec<String> = block
            .lines()
            .filter(|line| !line.starts_with(TRANSCRIPT_BEGIN) && !line.starts_with(TRANSCRIPT_END))
            .filter_map(|line| {
                let text = match crate::timecode::parse_timecode(line) {
                    Some((_, rest)) => rest.trim(),
                    None => line.trim(),
                };
                if text.is_empty() || text.starts_with('\u{1F516}') || text.starts_with("> ") {
                    None
                } else {
                    Some(text.to_owned())
                }
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    });

    let mut products = QuickNoteProducts {
        second_pass,
        ..QuickNoteProducts::default()
    };
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut preamble = String::new();
    for line in rest.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.push((heading.trim().to_owned(), String::new()));
        } else if let Some((_, text)) = sections.last_mut() {
            text.push_str(line);
            text.push('\n');
        } else {
            preamble.push_str(line);
            preamble.push('\n');
        }
    }
    if sections.is_empty() {
        let text = preamble.trim();
        let spoken = !crate::frontmatter::read_luna(content).media.is_empty();
        products.raw = (spoken && !text.is_empty()).then(|| text.to_owned());
        return products;
    }
    for (heading, text) in sections {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if QUICK_NOTE_RAW_HEADINGS.contains(&heading.as_str()) {
            products.raw.get_or_insert_with(|| text.to_owned());
        } else if QUICK_NOTE_REFINED_HEADINGS.contains(&heading.as_str()) {
            products.refined.get_or_insert_with(|| text.to_owned());
        }
    }
    products
}

/// 把正文拆成「块外部分」与「转写块」（块含哨兵；无块返回 `None`）。
fn split_transcript_block(body: &str) -> (String, Option<String>) {
    match (body.find(TRANSCRIPT_BEGIN), body.find(TRANSCRIPT_END)) {
        (Some(begin), Some(end)) if end >= begin => {
            let end_idx = end + TRANSCRIPT_END.len();
            let mut rest = String::with_capacity(body.len());
            rest.push_str(&body[..begin]);
            rest.push_str(&body[end_idx..]);
            (rest, Some(body[begin..end_idx].to_owned()))
        }
        _ => (body.to_owned(), None),
    }
}

impl Vault {
    /// 写 / 清速记的采用标记（PRD-02-033）：只动 frontmatter，正文一个字不碰。
    /// 只替换正文，frontmatter 逐字节保留（桌面便签的写回路径，ADR-119 ④）。
    ///
    /// 便签里编辑的是原始 Markdown 正文，而 frontmatter 是 Lunarca 的账本——把整篇内容
    /// 交给一个 260×220 的文本框，等于让用户在便签里误删自己的 `luna:` 块。故拆合归这里，
    /// 前端只收发一段正文。
    ///
    /// 保留 frontmatter 与正文之间那一段原样的分隔（`body_of` 的切点即前缀末尾），
    /// 不重新拼一个空行——重拼会让每次保存都产生一次无意义的字节差异。
    ///
    /// # 写回必须带基线
    ///
    /// `base` 是调用方**以为磁盘上此刻是什么**。对不上就一个字节都不写并返回 `false`。
    ///
    /// 这不是防并发的优化，是一条数据安全护栏（真机 2026-09-09：钉到桌面之后整篇正文被
    /// 清空）。没有基线时，"把客户端手里的字符串盖上去"与"把一个客户端从没见过的状态盖
    /// 掉"在类型上长得一模一样——前端只要有一帧拿着陈旧的空串，用户的笔记就没了。有了
    /// 基线，这种写入在架构上不可能发生：客户端没读过的内容，它也说不出正确的基线。
    ///
    /// 顺带挡住第二种丢数据：便签与主编辑器同时开着同一篇时，后写的那一方会把对方的
    /// 改动整段盖掉——现在它会被拒绝，由调用方重读。
    ///
    /// # 只收发「人写的那一段」
    ///
    /// `base` / `body` 指的是 [`sticky_text`] 切出来的可编辑段：正文里第一个转写块之前的部分。
    /// 转写块（及其后的内容）是机器账本，便签不显示、写回时逐字节接在后面——真机 2026-10-02：
    /// 语音速记钉到桌面，便签里露出 `<!-- luna:transcript -->` 两行哨兵，用户一删，二次转写
    /// 就再也找不到落点。
    pub fn replace_body(&self, rel: &str, base: &str, body: &str) -> Result<bool> {
        let doc = self.read(rel)?;
        let full = crate::frontmatter::body_of(&doc.content);
        let (existing, tail) = sticky_text(full);
        if existing != base {
            return Ok(false);
        }
        if existing == body {
            return Ok(true);
        }
        let prefix_len = doc.content.len() - full.len();
        let mut next = String::with_capacity(prefix_len + body.len() + tail.len());
        next.push_str(&doc.content[..prefix_len]);
        next.push_str(body);
        next.push_str(tail);
        self.write(rel, &next)?;
        Ok(true)
    }

    /// 钉住 / 取消钉住一张桌面便签，或更新它的位置、尺寸与配色（ADR-119）。
    ///
    /// `None` = 取消钉住：只删掉 `luna.desktop_note` 这一个键，笔记本身一个字不动。
    /// 走 [`Self::rewrite_luna`] 这条唯一的 frontmatter 咽喉，正文与第三方键逐字节保留。
    ///
    /// 值没变就不写盘：便签拖动会连着发很多次坐标，每一次都写盘等于把用户的笔记历史
    /// 塞满无意义的快照。
    pub fn set_desktop_note(
        &self,
        rel: &str,
        desktop: Option<crate::frontmatter::DesktopNote>,
    ) -> Result<()> {
        self.rewrite_luna(rel, |meta| {
            if meta.desktop_note == desktop {
                return false;
            }
            meta.desktop_note = desktop;
            true
        })?;
        Ok(())
    }

    /// 此刻**钉在桌面上**的那些便签（启动时按它恢复窗口）。
    ///
    /// 走索引而不是扫盘：这条在每次启动都要跑一遍，而 `luna.desktop_note` 不进索引列，
    /// 故按速记清单逐篇读 frontmatter——便签是用户手工钉的，数量在个位数量级，
    /// 与"全库扫一遍"不是一回事。
    pub fn desktop_notes(&self) -> Result<Vec<(String, crate::frontmatter::DesktopNote)>> {
        let (entries, _) = self.index.draft_inbox(
            &crate::index::DraftInboxQuery {
                text: None,
                filter: crate::index::DraftFilter::All,
                now_ms: 0,
            },
            usize::from(u16::MAX),
            None,
        )?;
        let mut out = Vec::new();
        for entry in entries {
            // 读不出来的那一篇跳过而不是整体失败：一篇坏笔记不该让所有便签都恢复不了。
            let Ok(doc) = self.read(&entry.path) else {
                continue;
            };
            // 只收**此刻钉着的**：取消钉住之后那个键仍在（它记着位置、尺寸与颜色，
            // 见 `DesktopNote::pinned`），但它不该在下次启动时又自己冒出来。
            if let Some(desktop) = doc.meta.desktop_note.filter(|d| d.pinned) {
                out.push((entry.path, desktop));
            }
        }
        Ok(out)
    }

    pub fn set_quick_note_adopted(&self, rel: &str, adopted: Option<AdoptedVersion>) -> Result<()> {
        self.rewrite_luna(rel, |meta| {
            let next = adopted.map(|a| a.as_str().to_owned());
            if meta.adopted == next {
                return false;
            }
            meta.adopted = next;
            true
        })?;
        Ok(())
    }
}

/// 用哨兵包裹行标记，产出可定位替换的转写块（空转写仍产 `BEGIN\nEND\n`，供后续重写定位）。
fn build_transcript_block(segments: &[Segment], sorted_bookmarks: &[Bookmark]) -> String {
    let marker_lines = build_timed_marker_lines(segments, sorted_bookmarks);
    format!("{TRANSCRIPT_BEGIN}\n{marker_lines}{TRANSCRIPT_END}\n")
}

/// 在正文中替换哨兵块；无块则追加到正文末尾（首次归档路径）。块外内容原字节保留。
fn replace_or_append_transcript_block(body: &str, block: &str) -> String {
    if let (Some(begin), Some(end)) = (body.find(TRANSCRIPT_BEGIN), body.find(TRANSCRIPT_END)) {
        let end_idx = end + TRANSCRIPT_END.len();
        let mut out = String::with_capacity(body.len() + block.len());
        out.push_str(&body[..begin]);
        out.push_str(block);
        // 跳过旧块结束哨兵及其后紧邻的一个换行（block 自带尾换行）。
        let rest = &body[end_idx..];
        out.push_str(rest.strip_prefix('\n').unwrap_or(rest));
        out
    } else {
        let mut out = body.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block);
        out
    }
}

#[cfg(test)]
mod tests {
    /// 测试夹具：不带任何筛选的取数请求（`now_ms` 取此刻，陈化判定按真实时钟）。
    fn all_drafts() -> crate::DraftInboxQuery<'static> {
        crate::DraftInboxQuery {
            text: None,
            filter: crate::DraftFilter::All,
            now_ms: chrono::Local::now().timestamp_millis(),
        }
    }

    use super::*;
    use crate::frontmatter::NoteType;
    use crate::sidecar::{read_sidecar, SidecarLoad};
    use luna_core::{Segment, SegmentId};
    use std::time::Duration;

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.into(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    /// **SQLite 查询和 Rust 文件判定必须逐形状一致**（批次 H，真机 2026-08-31）。
    ///
    /// 这条用例存在的理由是一次真实的漏格：判据有四条轴，而它此前在四处各写各的
    /// ——归入候选的 SQL、`revisit_candidate` 的 SQL、「立即归入」检索路的 Rust
    /// `filter`，以及标签合成源集的 `Query`。最后那一份只设了三条轴（漏了产物轴），
    /// 于是三个 `type='note', is_topic=1` 的主题页通过了筛、被当成源笔记喂进合成，
    /// `Knowledge/计划.md` 的「来源」里因此列着另外两个主题页。当时代码里已经写着一句
    /// 「这三条与 SQL 判据**必须同义**」——纪律在、判定不在。
    ///
    /// 判据落在**语料**上而不是读代码：每一种形状各造一篇，SQLite 那一路（`search` 的
    /// `ingest_candidates_only`）通过连接函数调用同一 Rust 判据；另一条逐篇读文件。
    /// 索引事实、连接注册或 IPC 传参任一侧漂移，这里就会变红。
    #[test]
    fn the_sql_and_the_rust_predicate_agree_on_every_shape() {
        let (_dir, vault) = open_vault();
        // (路径, 正文, 该不该被挑成候选) —— 第三格是人读的期望，供报错时定位。
        let corpus: &[(&str, &str, bool)] = &[
            ("Notes/普通知识笔记.md", "---\ntags: [T]\n---\n正文", true),
            // 空笔记没有可提取证据，不能进入候选数或处理集。
            ("Notes/空白笔记.md", "---\ntags: [T]\n---\n", false),
            // 速记缺省为 draft；显式完成整理时应转成普通笔记。
            (
                "Notes/未整理速记.md",
                "---\ntags: [T]\nluna:\n  type: quick-note\n---\n正文",
                false,
            ),
            // 资料：先蒸馏成知识笔记再谈（ADR-024）。
            (
                "Transcripts/转写.md",
                "---\ntags: [T]\nluna:\n  type: transcript\n---\n正文",
                false,
            ),
            (
                "Recordings/录音.md",
                "---\ntags: [T]\nluna:\n  type: recording\n---\n正文",
                false,
            ),
            (
                "Chats/对话.md",
                "---\ntags: [T]\nluna:\n  type: chat\n---\n正文",
                false,
            ),
            // 待整理：还没定稿，我们不替他挑（ADR-086）。他自己点归入仍然可以。
            (
                "Notes/草稿.md",
                "---\ntags: [T]\nluna:\n  stage: draft\n---\n正文",
                false,
            ),
            (
                "Notes/未知成熟度.md",
                "---\ntags: [T]\nluna:\n  stage: unknown\n---\n正文",
                false,
            ),
            // 出事的那一格：主题页。`type` 仍是 note，只有产物轴分得开它。
            (
                "Knowledge/主题页.md",
                "---\ntags: [T]\nluna:\n  topic: true\n  topic_src: T\n---\n正文",
                false,
            ),
            (
                "Knowledge/知识地图.md",
                "---\ntags: [T]\nluna:\n  knowledge_map: true\n---\n正文",
                false,
            ),
        ];
        for (rel, content, _) in corpus {
            vault.write(rel, content).unwrap();
        }

        let by_sql: std::collections::BTreeSet<String> = vault
            .index
            .search(&Query {
                tag: Some("T".to_owned()),
                ingest_candidates_only: true,
                limit: 100,
                ..Query::default()
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.path)
            .collect();

        let by_rust: std::collections::BTreeSet<String> = corpus
            .iter()
            .filter(|(rel, ..)| {
                let doc = vault.read(rel).unwrap();
                crate::is_ingest_candidate(
                    rel.to_lowercase().ends_with(".md"),
                    doc.meta.note_type,
                    doc.meta.stage(),
                    crate::ownership_of(&doc.content).is_topic(),
                    !crate::body_of(&doc.content).trim().is_empty(),
                )
            })
            .map(|(rel, ..)| (*rel).to_owned())
            .collect();

        let expected: std::collections::BTreeSet<String> = corpus
            .iter()
            .filter(|(.., want)| *want)
            .map(|(rel, ..)| (*rel).to_owned())
            .collect();

        assert_eq!(by_rust, expected, "Rust 判据与语料的期望对不上");
        assert_eq!(by_sql, expected, "SQL 判据与语料的期望对不上");
        // 两条真实路径和语料期望三方一致，避免索引事实或连接函数接线漂移。
        assert_eq!(by_sql, by_rust);
    }

    /// **主题页建议所数的源集里，不许有 AI 产物页**（真机 2026-08-31）。
    ///
    /// 上一条守的是索引与逐文件读取一致；这一条守的是 `tag_knowledge_notes` 这条真实路径
    /// 确实用了它——建议的「够不够格」与标签页的「创建主题页」都按它数。
    #[test]
    fn the_compose_source_set_never_contains_a_knowledge_product() {
        let (_dir, vault) = open_vault();
        vault
            .write("Notes/会议纪要.md", "---\ntags: [运维/计划]\n---\n正文")
            .unwrap();
        vault
            .write(
                "Knowledge/计划.md",
                "---\ntags: [运维/计划]\nluna:\n  topic: true\n  topic_src: 运维/计划\n---\n正文",
            )
            .unwrap();
        vault
            .write(
                "Knowledge/知识地图.md",
                "---\ntags: [运维/计划]\nluna:\n  knowledge_map: true\n---\n正文",
            )
            .unwrap();

        let sources = vault.tag_knowledge_notes("运维/计划").unwrap();
        assert_eq!(
            sources,
            vec!["Notes/会议纪要.md".to_owned()],
            "产物页进了合成源集——它会被写进新页的「来源」区，AI 产物回流进 AI 产物"
        );
    }

    /// **树行的能力位由 Rust 判定下发，且用的是唯一那处判据**（ADR-003）。
    ///
    /// 「生成知识笔记」= **资料 ∪ 记录**（`NoteStage::allows_distill`）。把它写成"记录态
    /// 才给"的代价是：同一篇录音在录音清单里有这一项、在文件树里没有——2026-09-02 真机
    /// 上报「批量生成知识笔记没有入口」，选中的正是三篇录音。
    #[test]
    fn tree_rows_carry_the_capabilities_rust_already_decided() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Recordings")).unwrap();
        std::fs::create_dir_all(dir.path().join("Notes")).unwrap();
        std::fs::write(
            dir.path().join("Recordings/会议.md"),
            "---\nluna:\n  type: recording\n---\n逐字稿\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Notes/草稿.md"),
            "---\nluna:\n  stage: draft\n---\n想法\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("Notes/定稿.md"), "---\n---\n知识\n").unwrap();
        std::fs::write(dir.path().join("Notes/图.png"), b"PNG").unwrap();
        vault.scan().unwrap();

        let tree = vault.tree().unwrap();
        let find = |path: &str| -> NoteTree {
            fn walk(node: &NoteTree, path: &str) -> Option<NoteTree> {
                if node.path == path {
                    return Some(node.clone());
                }
                node.children.iter().find_map(|child| walk(child, path))
            }
            walk(&tree, path).unwrap_or_else(|| panic!("树里没有 {path}"))
        };

        // 资料与记录都能整理；知识态不能（它已经是产物那一侧）。
        assert!(
            find("Recordings/会议.md").distillable,
            "录音（资料）该给「生成知识笔记」"
        );
        assert!(
            find("Notes/草稿.md").distillable,
            "记录态该给「生成知识笔记」"
        );
        assert!(!find("Notes/定稿.md").distillable);
        // 附件与目录没有正文可整理。
        assert!(!find("Notes/图.png").distillable);
        assert!(!find("Notes").distillable);
        // 「汇总成一篇」的资格只挡聚合页（PRD-06-047 修订）：三种成熟度都能当碎片，
        // 附件与目录不能（没有正文）。
        assert!(find("Recordings/会议.md").can_merge, "资料现在也能参与汇总");
        assert!(find("Notes/草稿.md").can_merge);
        assert!(find("Notes/定稿.md").can_merge);
        assert!(!find("Notes/图.png").can_merge);
        assert!(!find("Notes").can_merge);
        // 「标记为记录」是另一位事实：只有知识态给（两位不能互相顶替）。
        assert!(find("Notes/定稿.md").markable_as_draft);
        assert!(!find("Recordings/会议.md").markable_as_draft);
        assert!(!find("Notes/草稿.md").markable_as_draft);
    }

    /// **给资产改名/移动之后，引用它的笔记必须跟着改**（PRD-06-012，2026-09-02 真机：
    /// 「改 wav 文件名称后，无法回放」）。
    ///
    /// 三条引用面各自独立，缺一条就是一种具体的坏：frontmatter `media[]` 不改 → 回放
    /// 打不开；边车不重键 → 文件都在、转写却空了；正文链接不改 → 图片变破图。
    #[test]
    fn renaming_an_asset_carries_every_reference_to_it() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Recordings/assets")).unwrap();
        std::fs::write(dir.path().join("Recordings/assets/旧名.wav"), b"RIFF").unwrap();
        std::fs::write(dir.path().join("Recordings/assets/图.png"), b"PNG").unwrap();
        let old_sidecar = crate::sidecar::sidecar_path_for("Recordings/assets/旧名.wav");
        std::fs::create_dir_all(dir.path().join(&old_sidecar).parent().unwrap()).unwrap();
        std::fs::write(dir.path().join(&old_sidecar), b"[]").unwrap();
        std::fs::write(
            dir.path().join("Recordings/会议.md"),
            format!(
                "---\nluna:\n  type: recording\n  media:\n    - path: Recordings/assets/旧名.wav\n      segments: {old_sidecar}\n---\n![](assets/图.png)\n"
            ),
        )
        .unwrap();

        vault
            .move_(
                "Recordings/assets/旧名.wav",
                "Recordings/assets/会议录音.wav",
            )
            .unwrap();

        let doc = vault.read("Recordings/会议.md").unwrap();
        assert_eq!(
            doc.meta.media[0].path, "Recordings/assets/会议录音.wav",
            "frontmatter 还指着旧路径——回放会打不开"
        );
        let new_sidecar = crate::sidecar::sidecar_path_for("Recordings/assets/会议录音.wav");
        assert_eq!(
            doc.meta.media[0].segments.as_deref(),
            Some(new_sidecar.as_str()),
            "边车没跟着重键——文件都在，转写却空了"
        );
        assert!(dir.path().join(&new_sidecar).is_file());
        assert!(!dir.path().join(&old_sidecar).exists());

        // 正文里的图片链接是**笔记相对**的：给图片改名后同样要跟着走。
        vault
            .move_("Recordings/assets/图.png", "Recordings/assets/配图.png")
            .unwrap();
        let doc = vault.read("Recordings/会议.md").unwrap();
        assert!(
            doc.content.contains("![](assets/配图.png)"),
            "图片链接没跟着改，正文里留下一张破图：{}",
            doc.content
        );
    }

    /// **删除必须带走这一篇独占的附件**（PRD-06-012 生命周期，2026-09-02 真机上报：
    /// 删掉一批录音笔记之后，`Recordings/assets/` 里躺着一屏没人认领的音频）。
    ///
    /// 判据与 [`Vault::move_`] 的"独占才跟随"逐字同义——移动会带走的，删除也带走；
    /// 还有第二篇笔记引用的共享附件留下（把别人还在用的图片一并删掉更糟）。
    /// 另一篇用 `![[图.png]]`（按文件名全库找）引用着的图是共享的：删除或移动这一篇时
    /// 不得随它走——此前按「源目录 + 写法」拼路径看不见这种引用，那一篇的图会断（V2）。
    #[test]
    fn an_image_shared_through_a_wiki_embed_stays_put_on_delete_and_move() {
        let (dir, v) = open_vault();
        let root = dir.path();
        std::fs::create_dir_all(root.join("甲/assets")).unwrap();
        std::fs::write(root.join("甲/assets/图.png"), b"png").unwrap();
        v.write("甲/a.md", "![](assets/图.png)").unwrap();
        v.write("乙/b.md", "![[图.png]]").unwrap();
        v.move_("甲/a.md", "丙/a.md").unwrap();
        assert!(root.join("甲/assets/图.png").exists(), "移动不得带走共享图");
        v.delete("丙/a.md", DeleteMode::VaultTrash).unwrap();
        assert!(root.join("甲/assets/图.png").exists(), "删除不得带走共享图");
    }

    /// 反过来：只有这一篇用 `![[图.png]]` 引用它时，它是独占的，随删除一起走。
    #[test]
    fn an_image_owned_through_a_wiki_embed_follows_its_note_on_delete() {
        let (dir, v) = open_vault();
        let root = dir.path();
        std::fs::create_dir_all(root.join("甲/assets")).unwrap();
        std::fs::write(root.join("甲/assets/图.png"), b"png").unwrap();
        v.write("甲/a.md", "![[图.png]]").unwrap();
        v.delete("甲/a.md", DeleteMode::VaultTrash).unwrap();
        assert!(!root.join("甲/assets/图.png").exists());
    }

    #[test]
    fn deleting_a_note_takes_its_own_attachments_and_leaves_the_shared_ones() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Recordings/assets")).unwrap();
        std::fs::create_dir_all(dir.path().join(".luna/sidecars")).unwrap();
        std::fs::write(dir.path().join("Recordings/assets/录音.wav"), b"RIFF").unwrap();
        std::fs::write(dir.path().join("Recordings/assets/独占图.png"), b"PNG").unwrap();
        std::fs::write(dir.path().join("Recordings/assets/共享图.png"), b"PNG").unwrap();
        let sidecar = crate::sidecar::sidecar_path_for("Recordings/assets/录音.wav");
        std::fs::create_dir_all(dir.path().join(&sidecar).parent().unwrap()).unwrap();
        std::fs::write(dir.path().join(&sidecar), b"[]").unwrap();
        std::fs::write(
            dir.path().join("Recordings/会议.md"),
            format!(
                "---\nluna:\n  type: recording\n  media:\n    - path: Recordings/assets/录音.wav\n      segments: {sidecar}\n---\n![](assets/独占图.png)\n![](assets/共享图.png)\n"
            ),
        )
        .unwrap();
        // 第二篇也引用「共享图」——它不属于任何一篇独占。
        std::fs::write(
            dir.path().join("Recordings/另一篇.md"),
            "---\n---\n![](assets/共享图.png)\n",
        )
        .unwrap();

        vault
            .delete("Recordings/会议.md", DeleteMode::Permanent)
            .unwrap();

        assert!(!dir.path().join("Recordings/会议.md").exists());
        assert!(
            !dir.path().join("Recordings/assets/录音.wav").exists(),
            "录音音频没跟着笔记走——它就是这一篇的录音，不是共享素材"
        );
        assert!(
            !dir.path().join(&sidecar).exists(),
            "转写边车留下了：笔记没了，它再也没有归属"
        );
        assert!(
            !dir.path().join("Recordings/assets/独占图.png").exists(),
            "只有这一篇引用的图片没跟着走"
        );
        assert!(
            dir.path().join("Recordings/assets/共享图.png").exists(),
            "把另一篇还在引用的图片一并删了——比留下一个孤儿糟得多"
        );
        assert!(dir.path().join("Recordings/另一篇.md").exists());
    }

    /// **回收站里只出现"用户删掉的那一条"，还原它时附件一起回来**。
    ///
    /// 附件跟着笔记进回收站之后，如果它们各自成一行：用户会看到一列认不出的 uuid 音频
    /// （他从没删过"它们"），还原笔记则拿回一篇打不开的录音——两件事都得靠归属解决。
    #[test]
    fn vault_trash_hides_owned_assets_and_restores_them_with_their_note() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Recordings/assets")).unwrap();
        std::fs::write(dir.path().join("Recordings/assets/录音.wav"), b"RIFF").unwrap();
        std::fs::write(
            dir.path().join("Recordings/会议.md"),
            "---\nluna:\n  type: recording\n  media:\n    - path: Recordings/assets/录音.wav\n---\n正文\n",
        )
        .unwrap();

        vault
            .delete("Recordings/会议.md", DeleteMode::VaultTrash)
            .unwrap();
        assert!(!dir.path().join("Recordings/assets/录音.wav").exists());

        let listed = vault.list_trash();
        assert_eq!(listed.len(), 1, "回收站里多出了一行用户从没删过的音频");
        assert_eq!(listed[0].original_path, "Recordings/会议.md");

        vault.restore(&listed[0].id, RestoreConflict::Fail).unwrap();
        assert!(dir.path().join("Recordings/会议.md").exists());
        assert!(
            dir.path().join("Recordings/assets/录音.wav").exists(),
            "还原回来的是一篇打不开的录音——音频还留在回收站里"
        );
        assert!(vault.list_trash().is_empty());
    }

    /// **文件夹条目的「永久删除」必须真的删掉它**。
    ///
    /// 这一处此前只有 `remove_file`：对目录必然失败、错误被吞、索引条目照样移除——那个
    /// 目录从此不出现在任何列表里、也再也删不掉，永久占着磁盘，而界面报的是成功。
    #[test]
    fn purging_a_folder_entry_removes_the_directory_not_just_the_index_row() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("项目/子目录")).unwrap();
        std::fs::write(dir.path().join("项目/子目录/一.md"), "# 一\n").unwrap();
        vault.delete("项目", DeleteMode::VaultTrash).unwrap();

        let id = vault.list_trash()[0].id.clone();
        let trash_dir = dir.path().join(".trash");
        assert!(trash_dir.join(&id).is_dir(), "文件夹本该整棵搬进回收站");

        let outcome = vault.purge(Some(&id)).unwrap();
        assert_eq!(outcome.purged, 1);
        assert!(outcome.failed.is_empty());
        assert!(
            !trash_dir.join(&id).exists(),
            "目录还躺在 `.trash/` 里：它既不出现在任何列表中，也再也删不掉"
        );
        assert!(vault.list_trash().is_empty());
    }

    /// 清空回收站同样要对目录分派——一次清空里混着文件与文件夹是常态。
    #[test]
    fn purging_everything_removes_folder_entries_too() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("项目")).unwrap();
        std::fs::write(dir.path().join("项目/一.md"), "# 一\n").unwrap();
        std::fs::write(dir.path().join("散记.md"), "# 散记\n").unwrap();
        vault.delete("项目", DeleteMode::VaultTrash).unwrap();
        vault.delete("散记.md", DeleteMode::VaultTrash).unwrap();

        let outcome = vault.purge(None).unwrap();
        assert_eq!(outcome.purged, 2);
        assert!(outcome.failed.is_empty());

        let trash_dir = dir.path().join(".trash");
        let left = std::fs::read_dir(&trash_dir)
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.file_name() != ".index.json")
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(left, 0, "清空之后 `.trash/` 里还剩下东西");
        assert!(vault.list_trash().is_empty());
    }

    /// **索引损坏不能静默当成空的**：文件还在 `.trash/` 里，而空表一旦写回磁盘，
    /// 它们就永远失去可还原性、也永远不再出现在任何列表里。
    #[test]
    fn corrupt_trash_index_is_archived_and_rebuilt_from_disk() {
        let (dir, vault) = open_vault();
        std::fs::write(dir.path().join("一.md"), "一\n").unwrap();
        std::fs::write(dir.path().join("二.md"), "二\n").unwrap();
        vault.delete("一.md", DeleteMode::VaultTrash).unwrap();
        vault.delete("二.md", DeleteMode::VaultTrash).unwrap();

        let index = dir.path().join(".trash/.index.json");
        std::fs::write(&index, b"{ this is not json").unwrap();

        let listed = vault.list_trash();
        assert_eq!(listed.len(), 2, "损坏之后条目被当成不存在了");
        // 原路径只存在于坏掉的那份索引里，重建不出来——如实标成未知，不猜一个。
        assert!(listed.iter().all(|entry| entry.original_path.is_empty()));

        // 坏文件留档，没有被直接丢掉。
        let archived = std::fs::read_dir(dir.path().join(".trash"))
            .unwrap()
            .flatten()
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".index.corrupt-")
            });
        assert!(archived, "坏索引没有留档");

        // 重建后的索引是好的：再读一次不会又触发一次重建。
        let again = vault.list_trash();
        assert_eq!(again.len(), 2);
    }

    /// 原路径未知的条目**不许还原**：空路径 `resolve` 出来就是库根。
    #[test]
    fn recovered_entries_refuse_to_restore() {
        let (dir, vault) = open_vault();
        std::fs::write(dir.path().join("一.md"), "一\n").unwrap();
        vault.delete("一.md", DeleteMode::VaultTrash).unwrap();
        std::fs::write(dir.path().join(".trash/.index.json"), b"[[[").unwrap();

        let id = vault.list_trash()[0].id.clone();
        assert!(vault.restore(&id, RestoreConflict::Fail).is_err());
        assert!(vault.restore(&id, RestoreConflict::KeepBoth).is_err());
        // 拒绝之后条目仍在，用户还能预览、在访达里取回、或永久删除。
        assert_eq!(vault.list_trash().len(), 1);
        // 永久删除照样有效——这是它唯一还能被清掉的路。
        assert_eq!(vault.purge(Some(&id)).unwrap().purged, 1);
    }

    /// 索引文件**不存在**是正常状态（还没删过任何东西），不该触发"损坏恢复"。
    #[test]
    fn missing_trash_index_is_not_treated_as_corruption() {
        let (dir, vault) = open_vault();
        assert!(vault.list_trash().is_empty());
        let archived = std::fs::read_dir(dir.path().join(".trash"))
            .map(|read| {
                read.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".index.corrupt-")
                })
            })
            .unwrap_or(false);
        assert!(!archived, "没有索引却留了一份坏索引档案");
    }

    /// 保留期裁剪：过期的走人，没到期的一根手指都不碰。
    #[test]
    fn prune_trash_removes_only_expired_entries() {
        let (dir, vault) = open_vault();
        std::fs::write(dir.path().join("旧.md"), "旧\n").unwrap();
        std::fs::write(dir.path().join("新.md"), "新\n").unwrap();
        vault.delete("旧.md", DeleteMode::VaultTrash).unwrap();
        vault.delete("新.md", DeleteMode::VaultTrash).unwrap();

        let thirty_days = 30 * 24 * 60 * 60 * 1000i64;
        let now = now_ms() as i64;
        // 「旧」那条把时间往回拨到 31 天前（直接改索引，等同于它是一个月前删的）。
        let path = dir.path().join(".trash/.index.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut entries: Vec<TrashEntry> = serde_json::from_str(&raw).unwrap();
        for entry in &mut entries {
            if entry.original_path == "旧.md" {
                entry.deleted_ms = (now - thirty_days - 24 * 60 * 60 * 1000) as u64;
            }
        }
        std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();

        let outcome = vault.prune_trash(Some(thirty_days), now).unwrap();
        assert_eq!(outcome.purged, 1);
        let left = vault.list_trash();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].original_path, "新.md");
    }

    /// 保留期设为「永不」时**一条都不许动**——自动删数据必须能被关掉。
    #[test]
    fn prune_trash_does_nothing_when_retention_is_never() {
        let (dir, vault) = open_vault();
        std::fs::write(dir.path().join("旧.md"), "旧\n").unwrap();
        vault.delete("旧.md", DeleteMode::VaultTrash).unwrap();
        let far_future = now_ms() as i64 + 3650 * 24 * 60 * 60 * 1000;
        assert_eq!(vault.prune_trash(None, far_future).unwrap().purged, 0);
        assert_eq!(vault.list_trash().len(), 1);
    }

    /// 剩余天数向上取整；到期与永不各有各的答案。
    #[test]
    fn trash_days_left_rounds_up_and_honours_never() {
        let day = 24 * 60 * 60 * 1000i64;
        let now = 1_000 * day;
        let thirty = 30 * day;
        // 刚删掉：整整 30 天。
        assert_eq!(
            Vault::trash_days_left(now as u64, Some(thirty), now),
            Some(30)
        );
        // 还剩 0.2 天 → 说「还有 1 天」，而不是「还有 0 天」（后者读起来像已经到期了）。
        let almost = now - thirty + day / 5;
        assert_eq!(
            Vault::trash_days_left(almost as u64, Some(thirty), now),
            Some(1)
        );
        // 已过期（还没被裁到）给 0，不给负数。
        assert_eq!(
            Vault::trash_days_left((now - thirty - day) as u64, Some(thirty), now),
            Some(0)
        );
        // 永不 = 没有"还剩几天"这回事。
        assert_eq!(Vault::trash_days_left(now as u64, None, now), None);
    }

    /// **落点被占时，可恢复性不该被一次重名卡死**。
    ///
    /// `Fail` 什么都不动（界面据此去问用户），`KeepBoth` 取唯一名还原为副本，且**原地
    /// 那个文件一根手指都不碰**。
    #[test]
    fn restoring_onto_an_occupied_path_either_reports_or_keeps_both() {
        let (dir, vault) = open_vault();
        std::fs::write(dir.path().join("笔记.md"), "旧的\n").unwrap();
        vault.delete("笔记.md", DeleteMode::VaultTrash).unwrap();
        // 同一个位置此刻被另一个文件占着（外部编辑器新建、或用户又写了一篇同名的）。
        std::fs::write(dir.path().join("笔记.md"), "新的\n").unwrap();

        let id = vault.list_trash()[0].id.clone();
        let err = vault.restore(&id, RestoreConflict::Fail).unwrap_err();
        assert!(matches!(err, VaultError::AlreadyExists(_)));
        assert_eq!(vault.list_trash().len(), 1, "报错那一支不该把条目弄丢");

        let restored = vault.restore(&id, RestoreConflict::KeepBoth).unwrap();
        assert_ne!(restored, "笔记.md", "副本不该盖在原路径上");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("笔记.md")).unwrap(),
            "新的\n",
            "占位的那个文件被覆盖了"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&restored)).unwrap(),
            "旧的\n"
        );
        assert!(vault.list_trash().is_empty());
    }

    /// 永久删除那一条时，跟着它进来的附件也必须清掉——否则它们再也不会出现在任何
    /// 列表里（回收站按归属过滤），只是永远占着磁盘。
    #[test]
    fn purging_a_note_also_purges_the_assets_that_came_with_it() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Recordings/assets")).unwrap();
        std::fs::write(dir.path().join("Recordings/assets/录音.wav"), b"RIFF").unwrap();
        std::fs::write(
            dir.path().join("Recordings/会议.md"),
            "---\nluna:\n  type: recording\n  media:\n    - path: Recordings/assets/录音.wav\n---\n正文\n",
        )
        .unwrap();
        vault
            .delete("Recordings/会议.md", DeleteMode::VaultTrash)
            .unwrap();

        let id = vault.list_trash()[0].id.clone();
        vault.purge(Some(&id)).unwrap();

        let trash_dir = dir.path().join(".trash");
        let left = std::fs::read_dir(&trash_dir)
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.file_name() != ".index.json")
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            left, 0,
            "音频还躺在 `.trash/` 里，而它再也不会出现在任何列表中"
        );
    }

    /// 删整个文件夹时，**子树之外**的独占附件同样要跟着走（附件目录可以配在库根）。
    #[test]
    fn deleting_a_folder_takes_the_owned_attachments_that_live_outside_it() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Notes/项目")).unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/图.png"), b"PNG").unwrap();
        std::fs::write(
            dir.path().join("Notes/项目/纪要.md"),
            "---\n---\n![](../../assets/图.png)\n",
        )
        .unwrap();

        vault.delete("Notes/项目", DeleteMode::Permanent).unwrap();

        assert!(!dir.path().join("Notes/项目").exists());
        assert!(
            !dir.path().join("assets/图.png").exists(),
            "集中存放的独占附件没跟着整个文件夹走"
        );
    }

    /// 重命名要不要保留扩展名，只有"它是不是目录"答得了——而从路径字面量猜（"没有点
    /// 就是目录"）会把 `v1.0` 这样的目录当成扩展名为 `0` 的文件。
    #[test]
    fn is_dir_and_exists_answer_from_disk_not_from_the_path_string() {
        let (dir, vault) = open_vault();
        std::fs::create_dir_all(dir.path().join("Notes/v1.0")).unwrap();
        std::fs::write(dir.path().join("Notes/v1.0/a.md"), "正文").unwrap();
        std::fs::write(dir.path().join("Notes/录音.wav"), b"RIFF").unwrap();

        assert!(vault.is_dir("Notes/v1.0").unwrap());
        assert!(!vault.is_dir("Notes/录音.wav").unwrap());
        assert!(!vault.is_dir("Notes/v1.0/a.md").unwrap());
        // 不存在的对象是错误而不是 `false`——"它不是目录"与"它不在"是两回事。
        assert!(vault.is_dir("Notes/不存在.md").is_err());

        assert!(vault.exists("Notes/v1.0"));
        assert!(vault.exists("Notes/录音.wav"));
        assert!(!vault.exists("Notes/不存在.md"));
        // 越界路径一律视作不存在，不外泄库根之外的文件系统状态。
        assert!(!vault.exists("../库外.md"));
    }

    #[test]
    fn hidden_entries_show_only_on_request_and_never_enter_the_index() {
        // V4b：点文件只在「显示隐藏文件」时列出、标成隐藏（其下的子项同样），Lunarca 自己的
        // 数据与 git 本体永远不列；在那里写一篇笔记也不进索引（与启动扫描同一口径）。
        let (dir, v) = open_vault();
        v.write("可见.md", "正文\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        std::fs::write(dir.path().join(".obsidian/app.json"), "{}").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        v.write("草稿/.私密.md", "藏起来的字\n").unwrap();

        let names = |tree: &NoteTree| -> Vec<String> {
            let mut out = Vec::new();
            fn walk(node: &NoteTree, out: &mut Vec<String>) {
                for child in &node.children {
                    out.push(format!(
                        "{}{}",
                        child.path,
                        if child.hidden { " (隐藏)" } else { "" }
                    ));
                    walk(child, out);
                }
            }
            walk(tree, &mut out);
            out
        };
        let plain = names(&v.tree().unwrap());
        assert!(
            plain.iter().all(|n| !n.contains('.') || n.ends_with(".md")),
            "{plain:?}"
        );
        assert!(!plain.iter().any(|n| n.contains(".私密")), "{plain:?}");

        let shown = names(&v.tree_showing_hidden(true).unwrap());
        assert!(shown.contains(&".obsidian (隐藏)".to_owned()), "{shown:?}");
        assert!(
            shown.contains(&".obsidian/app.json (隐藏)".to_owned()),
            "{shown:?}"
        );
        assert!(
            shown.contains(&"草稿/.私密.md (隐藏)".to_owned()),
            "{shown:?}"
        );
        assert!(shown.contains(&"可见.md".to_owned()), "{shown:?}");
        assert!(
            shown.contains(&".gitignore (隐藏)".to_owned()),
            "用户自己的点文件照列：{shown:?}"
        );
        assert!(
            !shown.iter().any(|n| n.starts_with(".luna")
                || n.starts_with(".git ")
                || n.starts_with(".git/")),
            "{shown:?}"
        );

        let hits = v
            .search_matches(&SearchQuery::parse("藏起来", false))
            .unwrap();
        assert!(hits.notes.is_empty(), "隐藏笔记不该进索引");
    }

    fn open_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn editor_save_rejects_external_update_and_deleted_file() {
        let (dir, vault) = open_vault();
        vault.write("note.md", "baseline").unwrap();
        std::fs::write(dir.path().join("note.md"), "external").unwrap();
        assert_eq!(
            vault.save_note("note.md", "baseline", "local").unwrap(),
            NoteSave::Conflict {
                current_content: Some("external".to_owned()),
            }
        );
        assert_eq!(vault.read("note.md").unwrap().content, "external");
        std::fs::remove_file(dir.path().join("note.md")).unwrap();
        assert_eq!(
            vault.save_note("note.md", "external", "local").unwrap(),
            NoteSave::Conflict {
                current_content: None
            }
        );
        assert!(!dir.path().join("note.md").exists());
    }

    #[test]
    fn editor_save_returns_persistence_receipt_and_retains_snapshot() {
        let (_dir, vault) = open_vault();
        vault.write("note.md", "baseline").unwrap();
        assert_eq!(
            vault.save_note("note.md", "baseline", "local").unwrap(),
            NoteSave::Saved {
                index_updated: true
            }
        );
        assert_eq!(vault.read("note.md").unwrap().content, "local");
        assert_eq!(vault.snapshots("note.md").len(), 1);
        assert!(matches!(
            vault.save_note("note.md", "baseline", "second"),
            Ok(NoteSave::Conflict { .. })
        ));
    }

    #[test]
    fn editor_save_does_not_report_file_failure_when_only_index_fails() {
        let (dir, vault) = open_vault();
        vault.write("note.md", "baseline").unwrap();
        let connection = rusqlite::Connection::open(dir.path().join(".luna/index.sqlite")).unwrap();
        connection.execute_batch("DROP TABLE notes;").unwrap();
        assert_eq!(
            vault.save_note("note.md", "baseline", "persisted").unwrap(),
            NoteSave::Saved {
                index_updated: false
            }
        );
        assert_eq!(vault.read("note.md").unwrap().content, "persisted");
        assert_eq!(vault.snapshots("note.md").len(), 1);
    }

    fn embedding_identity(
        target_id: &str,
        runtime_digest: &str,
        dimension: u32,
    ) -> crate::EmbeddingIndexIdentity {
        crate::EmbeddingIndexIdentity {
            target_id: target_id.to_owned(),
            runtime_digest: runtime_digest.to_owned(),
            dimension,
            profile_version: 1,
            normalization: "l2".to_owned(),
            chunk_policy_id: crate::BGE_M3_CHUNK_POLICY.id.to_owned(),
        }
    }

    /// 建一篇笔记（`Vault::write` 顺带重索引，检索与相关面板都走索引粗筛）。
    fn seed_note(v: &Vault, stem: &str, note_type: NoteType, body: &str) -> String {
        let rel = format!("{stem}.md");
        let content = format!(
            "---\nluna:\n  schema: 1\n  type: {}\n---\n{body}\n",
            note_type.key()
        );
        v.write(&rel, &content).unwrap();
        rel
    }

    /// 读不了的子目录（V1b）：遍历时记下，体检列成一张卡；权限恢复后的下一次遍历就消失。
    #[cfg(unix)]
    #[test]
    fn unreadable_folders_surface_in_the_checkup_until_the_next_walk() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, v) = open_vault();
        std::fs::create_dir(v.root().join("锁住")).unwrap();
        std::fs::write(v.root().join("锁住/c.md"), "y").unwrap();
        std::fs::set_permissions(
            v.root().join("锁住"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        v.scan().unwrap();
        std::fs::set_permissions(
            v.root().join("锁住"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let unreadable: Vec<_> = v
            .checkup_t0(false, 0)
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == crate::checkup::FindingKind::UnreadableFolder)
            .collect();
        assert_eq!(unreadable.len(), 1);
        assert_eq!(unreadable[0].paths, vec!["锁住".to_owned()]);
        assert_eq!(unreadable[0].id, "unreadable-folder:锁住");

        v.scan().unwrap();
        assert!(v
            .checkup_t0(false, 0)
            .unwrap()
            .iter()
            .all(|f| f.kind != crate::checkup::FindingKind::UnreadableFolder));
    }

    /// 搜索：命中的篇全都读不出（权限）时报错，不说「没有匹配」；文件不在（索引落后）照旧跳过。
    #[cfg(unix)]
    #[test]
    fn search_reports_unreadable_hits_instead_of_no_matches() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, v) = open_vault();
        v.write("锁住/甲.md", "季度复盘的要点").unwrap();
        v.write("乙.md", "季度复盘的另一篇").unwrap();
        let query = SearchQuery::parse("季度复盘", false);
        std::fs::remove_file(v.root().join("乙.md")).unwrap();
        std::fs::set_permissions(
            v.root().join("锁住"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let result = v.search_matches(&query);
        std::fs::set_permissions(
            v.root().join("锁住"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(matches!(result, Err(VaultError::Io(_))), "{result:?}");
        // 只剩索引落后的那篇（文件已不在）：照旧是空结果，不报错。
        std::fs::remove_dir_all(v.root().join("锁住")).unwrap();
        assert!(v.search_matches(&query).unwrap().notes.is_empty());
    }

    /// 知识体检 T0 走真实文件与真实索引（PRD-16-030）：三列新派生字段（`is_topic` /
    /// `body_chars` / `distill_note`）要真的被索引写对，否则纯内核测得再全也白搭。
    #[test]
    fn checkup_t0_runs_on_the_real_index() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/孤零零", NoteType::Note, "一篇谁也没链的笔记。");
        seed_note(&v, "Notes/空的", NoteType::Note, "");
        seed_note(&v, "Recordings/还没转写", NoteType::Recording, "");
        seed_note(&v, "Notes/源", NoteType::Note, "限流策略已确认。");
        v.write_topic(
            "Notes/主题.md",
            "# 主题\n\n## 进展\n\n- 限流策略已确认。（[源](%E6%BA%90.md)）\n- 指向虚空。（[没这一篇](%E6%B2%A1%E8%BF%99%E4%B8%80%E7%AF%87.md)）\n",
            None,
        )
        .unwrap();

        let findings = v.checkup_t0(true, 0).unwrap();
        let of = |kind: crate::checkup::FindingKind| -> Vec<crate::checkup::Finding> {
            findings
                .iter()
                .filter(|f| f.kind == kind)
                .cloned()
                .collect()
        };
        assert_eq!(
            of(crate::checkup::FindingKind::Orphan)
                .iter()
                .map(|f| f.paths[0].clone())
                .collect::<Vec<_>>(),
            vec!["Notes/孤零零.md"],
            "被主题页引用的源不是孤儿；资料区不参与：{findings:?}"
        );
        assert_eq!(
            of(crate::checkup::FindingKind::EmptyNote)
                .iter()
                .map(|f| f.paths[0].clone())
                .collect::<Vec<_>>(),
            vec!["Notes/空的.md"],
            "空的录音笔记是它的正常状态，不报"
        );
        // 转义 + 同目录的自动链接必须解析得回来：解析不回来的话「源」会变成一个幽灵
        // 节点，于是这里会冒出**两条**断链，其中一条是凭空的（graph::LinkIndex 守的正是它）。
        let broken = of(crate::checkup::FindingKind::BrokenLink);
        assert_eq!(broken.len(), 1, "只有真断链：{broken:?}");
        assert_eq!(broken[0].paths, vec!["Notes/主题.md"]);

        // 深度体检的候选：主题页上带源链接的列表项，行号相对整篇文件。
        let claims = v.topic_claims(8).unwrap();
        let claim = claims
            .iter()
            .find(|(_, c)| c.source == "Notes/源.md")
            .expect("带源链接的声明就是候选");
        let doc = v.read("Notes/主题.md").unwrap();
        let line_text = doc
            .content
            .lines()
            .nth(claim.1.line as usize - 1)
            .expect("行号必须落在文件里");
        assert!(
            line_text.contains("限流策略已确认。"),
            "行号指向的必须就是那条声明：{line_text}"
        );

        // T0 的发现绝不落派生层（存下来就是一份会过期的副本，与现算结果并列摆在同
        // 一张报告里，用户无从知道哪条此刻为真）——这条不变量由 Vault 显式拒绝。
        let t0 = findings
            .iter()
            .find(|f| f.kind == crate::checkup::FindingKind::Orphan)
            .expect("孤儿卡在场");
        assert!(
            v.put_checkup_finding(t0, 0).is_err(),
            "即时体检的发现不该被落盘"
        );

        // ✕ 是权威决定：清空整个派生索引再重建，它也必须还在（ADR-026 ⑦）。
        let orphan_id = of(crate::checkup::FindingKind::Orphan)[0].id.clone();
        v.ignore_checkup_finding(&orphan_id).unwrap();
        v.clear_cache().unwrap();
        v.scan().unwrap();
        assert!(
            v.checkup_t0(true, 0)
                .unwrap()
                .iter()
                .all(|f| f.id != orphan_id),
            "忽略过的发现绝不能随索引重建复活"
        );
    }

    /// 标签簇的大小写归并（2026-07-29 真机第一轮）：模型给两篇笔记写了
    /// `Data_Security` 与 `data_security`，本该 2 篇的簇裂成两个 1 篇——标签树重复呈现、
    /// 主题页建议（≥3 篇同标签）永远凑不够。分组与过滤必须同一口径。
    /// 写一篇记录态笔记（显式 `luna.stage: draft`）。
    fn seed_draft(v: &Vault, stem: &str, body: &str) -> String {
        let rel = format!("{stem}.md");
        v.write(
            &rel,
            &format!("---\nluna:\n  schema: 1\n  type: note\n  stage: draft\n---\n{body}\n"),
        )
        .unwrap();
        rel
    }

    /// 主题页合成建议的「≥3 篇」判据排除记录态（PRD-16-016 修订 / ADR-086）：
    /// 零散速记凑满一个标签不该触发合成建议。反向验证：去掉 `knowledge_only` 即红。
    #[test]
    fn topic_suggestions_do_not_count_drafts() {
        let (_dir, v) = open_vault();
        let tagged = |body: &str| format!("---\ntags:\n  - 项目\n---\n{body}");
        // 两篇知识 + 两篇记录态：知识只有 2 篇，不到门槛。
        for (i, stem) in ["Notes/知一", "Notes/知二"].iter().enumerate() {
            v.write(
                &format!("{stem}.md"),
                &format!(
                    "---\ntags:\n  - 项目\nluna:\n  schema: 1\n  type: note\n---\n知识第 {i} 篇。\n"
                ),
            )
            .unwrap();
        }
        for stem in ["Notes/草一", "Notes/草二"] {
            v.write(
                &format!("{stem}.md"),
                "---\ntags:\n  - 项目\nluna:\n  schema: 1\n  type: note\n  stage: draft\n---\n草稿。\n",
            )
            .unwrap();
        }
        assert!(
            v.topic_suggestions(&[]).unwrap().is_empty(),
            "两篇知识 + 两篇记录不该触发建议"
        );

        // 再补一篇知识 → 够 3 篇，建议出现，且候选里不含记录态。
        v.write("Notes/知三.md", &tagged("知识第三篇。")).unwrap();
        let out = v.topic_suggestions(&[]).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].notes.len(), 3, "候选只数知识态：{:?}", out[0].notes);
        assert!(
            out[0].notes.iter().all(|p| !p.contains("草")),
            "记录态不得进入合成候选：{:?}",
            out[0].notes
        );
    }

    /// 打标白名单收窄为「`note` 且非记录态」（PRD-06-052）：重扫盘点把记录态并入
    /// **同一个** `excluded_type` 计数（不第二处散写、不新开第三个数）。
    #[test]
    fn graph_tag_scan_excludes_drafts_through_the_same_whitelist() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/知识", NoteType::Note, "值得打标的正文。");
        seed_draft(&v, "Notes/草稿", "还没定稿的正文。");
        seed_note(&v, "Notes/速记", NoteType::QuickNote, "随口说的一句。");
        seed_note(&v, "Recordings/录音", NoteType::Recording, "转写全文。");

        let scan = v.graph_tag_scan(None).unwrap();
        assert_eq!(scan.stale, vec!["Notes/知识.md".to_string()], "{scan:?}");
        assert_eq!(
            scan.excluded_type, 3,
            "记录态 + 速记 + 录音同属「设计如此」这一档：{scan:?}"
        );

        // 沉淀（去掉 stage 键）后立刻恢复打标资格——不需要任何迁移动作。
        v.write(
            "Notes/草稿.md",
            "---\nluna:\n  schema: 1\n  type: note\n---\n还没定稿的正文。\n",
        )
        .unwrap();
        let after = v.graph_tag_scan(None).unwrap();
        assert_eq!(after.excluded_type, 2, "{after:?}");
        assert!(
            after.stale.contains(&"Notes/草稿.md".to_string()),
            "{after:?}"
        );
    }

    /// 树行的记录态事实由 Rust 下发（PRD-06-052 的淡显 + 徽标 + 过滤开关据此渲染）。
    #[test]
    fn tree_carries_the_draft_fact_like_topic_and_map() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/知识", NoteType::Note, "正文。");
        seed_draft(&v, "Notes/草稿", "正文。");
        seed_note(&v, "Notes/速记", NoteType::QuickNote, "正文。");
        let tree = v.tree().unwrap();
        let notes = tree.children.iter().find(|n| n.name == "Notes").unwrap();
        let by_name = |name: &str| notes.children.iter().find(|n| n.name == name).unwrap();
        assert!(by_name("草稿.md").draft);
        assert!(by_name("速记.md").draft, "速记缺省记录态（PRD-02-055）");
        assert!(!by_name("知识.md").draft);
        assert!(!notes.draft, "目录谈不上成熟度");
    }

    /// 右侧面板那两格问的是**同一个对象**，故一次取齐；而两半各有各的事实源。
    ///
    /// 类型 / 成熟度 / 创建时间来自**文件**（笔记即文件，索引里的同名列是派生副本）；
    /// 修改时间与标签来自**索引**——后者尤其要紧：直接读一次文件系统会更"新"，但同一篇
    /// 笔记在搜索列表与本面板上就会显示不同的时间，正是 PRD-07-065 那类缺陷。
    #[test]
    fn note_facts_reads_identity_from_the_file_and_tags_from_the_index() {
        let (_dir, v) = open_vault();
        v.write(
            "Notes/甲.md",
            "---\ntags:\n  - 架构\n  - ops\nluna:\n  schema: 1\n  type: note\n  created: 2026-08-31T10:00:00+08:00\n---\n正文。\n",
        )
        .unwrap();

        let facts = v.note_facts("Notes/甲.md").unwrap();
        assert_eq!(facts.note_type, NoteType::Note);
        assert!(!facts.draft);
        assert_eq!(facts.created.as_deref(), Some("2026-08-31T10:00:00+08:00"));
        assert!(
            facts.modified_ms.is_some_and(|ms| ms > 0),
            "写完即入索引，修改时间该有值"
        );
        let mut tags = facts.tags.clone();
        tags.sort();
        assert_eq!(
            tags,
            vec!["ops", "架构"],
            "标签取自索引（用户标签与 AI 标签同池），本方法不另算一份"
        );
    }

    /// 速记缺省即记录态（ADR-086），且没写 `created` 就如实是 `None`——**不猜**。
    #[test]
    fn note_facts_reports_draft_by_the_single_resolver_and_never_invents_a_created_time() {
        let (_dir, v) = open_vault();
        let rel = seed_note(&v, "Notes/速记", NoteType::QuickNote, "正文。");
        let facts = v.note_facts(&rel).unwrap();
        assert!(facts.draft, "速记无 stage 键即记录态（缺省规则只有一处）");
        assert_eq!(facts.created, None);
        assert!(facts.tags.is_empty());
        assert!(
            !facts.source_material,
            "速记不是来源层——上下文面板不该给它挂「所在层」那一行"
        );
    }

    #[test]
    fn create_stamps_created_for_typed_notes_and_keeps_a_caller_supplied_one() {
        // 录音 / 转写 / 对话 / 速记的「创建时间」在建档那一处盖章；给了就不覆盖；普通笔记
        // 仍是 lazy 骨架（不注入 `luna:` 块）。
        let (_dir, v) = open_vault();
        let doc = v
            .create(
                "Notes/录音.md",
                NoteType::Recording,
                &TemplateVars::default(),
            )
            .unwrap();
        let created = doc.meta.created.expect("录音建档必须写 created");
        chrono::DateTime::parse_from_rfc3339(&created).expect("RFC3339");
        assert_eq!(
            v.note_facts("Notes/录音.md").unwrap().created.as_deref(),
            Some(created.as_str())
        );

        let doc = v
            .create(
                "Notes/导入.md",
                NoteType::Transcript,
                &TemplateVars {
                    created: Some("2026-05-28T10:00:00+08:00".into()),
                    ..TemplateVars::default()
                },
            )
            .unwrap();
        assert_eq!(
            doc.meta.created.as_deref(),
            Some("2026-05-28T10:00:00+08:00")
        );

        let doc = v
            .create("Notes/普通.md", NoteType::Note, &TemplateVars::default())
            .unwrap();
        assert_eq!(doc.meta.created, None, "普通笔记不注入 luna 块");
    }

    #[test]
    fn note_facts_reports_source_material_by_note_type_so_the_panel_owns_no_type_list() {
        // PRD-06-048②：「所在层」那一行的判定在 Rust（`NoteType::is_source_material`），
        // 前端只看这一位布尔——它不自带"录音 / 转写 / 对话算来源"的清单。
        let (_dir, v) = open_vault();
        for (stem, ty) in [
            ("Notes/录音", NoteType::Recording),
            ("Notes/转写", NoteType::Transcript),
            ("Notes/对话", NoteType::Chat),
        ] {
            let rel = seed_note(&v, stem, ty, "正文。");
            assert!(
                v.note_facts(&rel).unwrap().source_material,
                "{stem} 应是来源层"
            );
        }
        let rel = seed_note(&v, "Notes/普通", NoteType::Note, "正文。");
        assert!(!v.note_facts(&rel).unwrap().source_material);
    }

    #[test]
    fn tags_group_case_insensitively_and_filter_agrees() {
        let (_dir, v) = open_vault();
        for (stem, tag) in [
            ("Notes/甲", "Data_Security"),
            ("Notes/乙", "data_security"),
            ("Notes/丙", "DATA_SECURITY"),
        ] {
            v.write(
                &format!("{stem}.md"),
                &format!("---\ntags:\n  - {tag}\nluna:\n  schema: 1\n  type: note\n---\n正文。\n"),
            )
            .unwrap();
        }
        let tags = v.list_tags().unwrap();
        let security: Vec<&crate::index::TagCount> = tags
            .iter()
            .filter(|t| t.tag.eq_ignore_ascii_case("data_security"))
            .collect();
        assert_eq!(security.len(), 1, "三种写法必须并成一条：{tags:?}");
        assert_eq!(security[0].count, 3);

        // 过滤跟着分组走：点开那一条要能看到全部三篇，否则数字与内容当场对不上。
        let hits = v
            .search(&Query {
                tag: Some("data_security".into()),
                limit: 10,
                ..Query::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 3, "过滤与分组同口径：{hits:?}");
    }

    /// 归入的候选**不能只靠相似度**：字面零重叠的笔记与主题页，要靠共享标签连上。
    ///
    /// 第五轮真机的现场：一篇繁体笔记与那唯一一页（简体）主题页字面几乎不重叠，检索的
    /// top-N 里全是别的笔记，于是"归入主题页"永远回答"没有合适的主题页"——而那一页
    /// 正是凭这条标签被建议出来的。这条测试把两条路的**分工**钉住：检索找不到（①），
    /// 标签必须找到，且出身标签那一页排最前（②）。
    #[test]
    fn tags_connect_a_note_to_its_topic_page_when_wording_shares_nothing() {
        let (_dir, v) = open_vault();
        let note = seed_note(
            &v,
            "Notes/雙均線心得",
            NoteType::Note,
            "以兩條移動平均線判斷多空轉折，慢線走平即減碼。",
        );
        let doc = v.read(&note).unwrap();
        let tagged = doc
            .content
            .replace("---\nluna:", "---\ntags:\n  - 趋势分析\nluna:");
        v.write(&note, &tagged).unwrap();
        // 主题页：简体、用词与上面那篇不重叠，但出身标签正是它的标签。
        v.write_topic(
            "Notes/趋势分析.md",
            "# 趋势分析\n\n## 要点\n\n- 顺势而为。\n",
            Some("趋势分析"),
        )
        .unwrap();
        // 另一页主题（无共享标签）用来证明邻居不是"所有主题页"。
        v.write_topic(
            "Notes/账务流程.md",
            "# 账务流程\n\n## 要点\n\n- 月结。\n",
            Some("账务"),
        )
        .unwrap();

        // ① 相似度路捞不到它——这正是缺陷现场，也是标签路存在的理由。
        let body = body_of(&v.read(&note).unwrap().content).to_owned();
        let hits = v
            .retrieve(
                &format!("{} {body}", luna_sync_core::links::title_of(&note)),
                &RetrieveOptions::default(),
            )
            .unwrap();
        assert!(
            !hits.passages.iter().any(|p| p.path == "Notes/趋势分析.md"),
            "字面不重叠时相似度本就找不到它，这条前提变了就该重看候选口径"
        );

        // ② 标签路找到了，且出身标签那一页排最前。
        let neighbors = v.tag_neighbors(&note).unwrap();
        let topics: Vec<&str> = neighbors
            .iter()
            .filter(|n| n.is_topic)
            .map(|n| n.path.as_str())
            .collect();
        assert_eq!(
            topics,
            vec!["Notes/趋势分析.md"],
            "共享标签的那一页才是候选"
        );
        assert!(neighbors[0].birth, "出身标签命中的页必须排最前");

        // ③ 反方向同一口径：从页出发也能找回这篇（「立即归入」走的就是它）。
        let back = v.tag_neighbors("Notes/趋势分析.md").unwrap();
        assert!(
            back.iter().any(|n| !n.is_topic && n.path == note),
            "从页出发要能收回它的标签同伴"
        );
    }

    /// 主题页建议的生命周期：成簇即建议 → 建页后安静 → 忽略后安静（PRD-16-016）。
    #[test]
    fn topic_page_lifecycle_from_suggestion_to_page() {
        let (_dir, v) = open_vault();
        // 三篇带同一标签的知识笔记 → 触发建议。
        for i in 1..=3 {
            seed_note(
                &v,
                &format!("Notes/项目笔记{i}"),
                NoteType::Note,
                &format!("---\n\n项目 X 的第 {i} 条记录。"),
            );
        }
        // 标签写在 frontmatter 顶层（用户标签口径）。
        for i in 1..=3 {
            let rel = format!("Notes/项目笔记{i}.md");
            let doc = v.read(&rel).unwrap();
            let with_tag = doc
                .content
                .replace("---\nluna:", "---\ntags:\n  - 项目X\nluna:");
            v.write(&rel, &with_tag).unwrap();
        }

        let suggestions = v.topic_suggestions(&[]).unwrap();
        let hit = suggestions
            .iter()
            .find(|s| s.tag == "项目X")
            .expect("三篇成簇即建议");
        assert_eq!(hit.notes.len(), 3);

        // 建页后同一标签不再建议（否则用户会被同一条提示反复打扰）。
        v.write_topic(
            "Notes/项目 X.md",
            "# 项目 X\n\n## 进展\n\n- 起步。\n",
            Some("项目X"),
        )
        .unwrap();
        assert!(
            !v.topic_suggestions(&[])
                .unwrap()
                .iter()
                .any(|s| s.tag == "项目X"),
            "已有主题页的标签不该再被建议"
        );
        // 忽略清单同样让它安静。
        assert!(v
            .topic_suggestions(&["项目X".into()])
            .unwrap()
            .iter()
            .all(|s| s.tag != "项目X"));

        // 刚写完是「AI 所有」。
        assert_eq!(
            v.topic_ownership("Notes/项目 X.md"),
            TopicOwnership::AiOwned
        );
    }

    /// 用户一编辑主题页，所有权即翻转，自动路径此后一律止步（ADR-026 ①）。
    #[test]
    fn user_edit_flips_topic_ownership_and_blocks_direct_writes() {
        let (_dir, v) = open_vault();
        v.write_topic("Notes/主题.md", "# 主题\n\n## 进展\n\n- 起步。\n", None)
            .unwrap();
        assert!(v.topic_ownership("Notes/主题.md").allows_direct_write());

        // 用户手改一个字（走普通 write，不经 write_topic）。
        let doc = v.read("Notes/主题.md").unwrap();
        v.write(
            "Notes/主题.md",
            &doc.content.replace("- 起步。", "- 起步（我补的）。"),
        )
        .unwrap();

        assert_eq!(
            v.topic_ownership("Notes/主题.md"),
            TopicOwnership::UserOwned
        );
        assert!(
            !v.topic_ownership("Notes/主题.md").allows_direct_write(),
            "接管之后一切更新恒走差异预览"
        );
    }

    /// 「舍弃改动」的恢复来源（ADR-096 ④）：底版 = 快照链里正文指纹与此刻
    /// `synth_body` 一致的最近一版；找不到即 `None`（调用方如实拒绝、引向「保留」）。
    #[test]
    fn product_baseline_snapshot_finds_the_system_written_version() {
        let (_dir, v) = open_vault();
        let rel = "Notes/主题.md";
        v.write_topic(rel, "# 主题\n\n## 进展\n\n- 起步。\n", Some("项目X"))
            .unwrap();
        let system_body = body_of(&v.read(rel).unwrap().content).to_owned();

        // 人工改动（外部编辑器同构）：`write` 留下改动前的快照——那一版就是底版。
        let doc = v.read(rel).unwrap();
        v.write(rel, &doc.content.replace("- 起步。", "- 我改的。"))
            .unwrap();
        assert_eq!(v.topic_ownership(rel), TopicOwnership::UserOwned);

        let baseline = v
            .product_baseline_snapshot(rel)
            .unwrap()
            .expect("底版应当在快照链里");
        assert_eq!(
            body_of(&baseline),
            system_body,
            "底版必须是系统写入的那一版"
        );

        // 恢复 = 原样写回底版（写盘再留一版快照，后悔可再回）；判据当场回到系统维护。
        v.write(rel, &baseline).unwrap();
        assert_eq!(v.topic_ownership(rel), TopicOwnership::AiOwned);

        // 普通笔记没有 `synth_body`，恒 `None`。
        seed_note(&v, "Notes/普通", NoteType::Note, "内容");
        assert!(v
            .product_baseline_snapshot("Notes/普通.md")
            .unwrap()
            .is_none());
    }

    /// 基线重登记（ADR-096 ④，「保留我的改动」的收口半步）。
    ///
    /// 三条一起守：① 登记后**自动维护重新可写**（判据回到系统维护）；② **正文一个
    /// 字节都不改**（登记的是基线，不是内容——把用户写的东西一并改掉才是真事故）；
    /// ③ 登记是**显式且有前提的**：不是产物页、或此刻并未被改动，一律如实拒绝而不是
    /// 静默成功（静默成功会让用户以为刚才那一下起了别的作用）。
    #[test]
    fn reregister_baseline_resumes_maintenance_without_touching_the_body() {
        let (_dir, v) = open_vault();
        let rel = "Notes/主题.md";
        v.write_topic(rel, "# 主题\n\n## 进展\n\n- 起步。\n", Some("项目X"))
            .unwrap();
        // 已是 AI 所有 → 交还无事可做，如实拒绝。
        assert!(
            v.reregister_product_baseline(rel).is_err(),
            "未被改动的页无基线可重登记"
        );

        let doc = v.read(rel).unwrap();
        v.write(rel, &doc.content.replace("- 起步。", "- 起步（我补的）。"))
            .unwrap();
        assert_eq!(v.topic_ownership(rel), TopicOwnership::UserOwned);
        let before = body_of(&v.read(rel).unwrap().content).to_owned();
        let versions_before = v.snapshots(rel).len();

        v.reregister_product_baseline(rel).unwrap();

        assert_eq!(
            v.topic_ownership(rel),
            TopicOwnership::AiOwned,
            "登记之后判据必须回到系统维护——它就是 synth_body 那一行，不是第二套状态"
        );
        assert_eq!(
            body_of(&v.read(rel).unwrap().content),
            before,
            "登记的是基线，正文一个字节都不该动"
        );
        assert!(
            v.snapshots(rel).len() > versions_before,
            "登记错了要能回滚：写盘必须留快照"
        );
        // 自动维护重新可写（这正是重登记的全部意义）。
        assert!(v.topic_ownership(rel).allows_direct_write());

        // 普通笔记不存在基线。
        seed_note(&v, "Notes/普通", NoteType::Note, "内容");
        assert!(v.reregister_product_baseline("Notes/普通.md").is_err());
    }

    #[test]
    fn retrieve_finds_notes_and_downranks_source_material() {
        let (_dir, v) = open_vault();
        seed_note(
            &v,
            "Notes/项目进展",
            NoteType::Note,
            "项目 X 的部署流程已经跑通。",
        );
        seed_note(
            &v,
            "Transcripts/会议逐字稿",
            NoteType::Transcript,
            "我们聊了项目 X 的部署流程。",
        );

        let hit = v
            .retrieve("项目 X 的部署流程是什么", &RetrieveOptions::default())
            .unwrap();
        assert_eq!(hit.passages.len(), 2, "资料降权但绝不排除");
        assert!(
            hit.passages[0].path.starts_with("Notes/"),
            "同等相关时知识笔记在前：{:?}",
            hit.passages.iter().map(|p| &p.path).collect::<Vec<_>>()
        );
        assert!(hit.scanned >= 2, "「已检索 N 篇」报的是真读盘篇数");
    }

    #[test]
    fn knowledge_map_routes_to_real_topic_but_never_becomes_evidence() {
        let (_dir, v) = open_vault();
        v.write_topic(
            "Notes/限流.md",
            "# 限流\n\n令牌桶用于吸收短时突发流量。",
            Some("架构"),
        )
        .unwrap();
        let body = v.knowledge_map_body("Notes/知识地图.md", true).unwrap();
        v.write_knowledge_map("Notes/知识地图.md", &body).unwrap();

        // 只让语义路命中地图，验证它扩展真实链接而非把地图摘要当 passage。
        let found = v
            .retrieve(
                "突发流量怎么处理",
                &RetrieveOptions {
                    semantic: vec![crate::semantic::SemanticHit {
                        path: "Notes/知识地图.md".into(),
                        chunk_seq: 0,
                        line_start: 1,
                        score: 0.9,
                    }],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(found
            .passages
            .iter()
            .any(|passage| passage.path == "Notes/限流.md"));
        assert!(found
            .passages
            .iter()
            .all(|passage| passage.path != "Notes/知识地图.md"));
    }

    #[test]
    fn retrieve_excludes_chat_notes_until_explicitly_included() {
        // 反投毒（ADR-026 ②b）：对话笔记默认不作事实源，但排除必须可翻转。
        let (_dir, v) = open_vault();
        seed_note(
            &v,
            "Chats/某次对话",
            NoteType::Chat,
            "## Assistant\n\n量子退火的原理如下。",
        );

        let default_run = v
            .retrieve("量子退火的原理", &RetrieveOptions::default())
            .unwrap();
        assert!(default_run.passages.is_empty(), "默认排除对话笔记");
        assert_eq!(default_run.scanned, 0, "没读的篇数不能算进「已检索」");

        let included = v
            .retrieve(
                "量子退火的原理",
                &RetrieveOptions {
                    include_chat: true,
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert_eq!(included.passages.len(), 1, "显式纳入即照常参与");
    }

    /// 库内问答对记录态的完整口径（PRD-16-041），走真实文件与真实索引：
    /// ① 默认**参与**；② 同一事实同时存在时，回答优先引用知识态（排名让位）；
    /// ③ 命中随身带「来自记录」的事实；④ 透明行一键整组排除后不再出现；
    /// ⑤ 沉淀（去掉 `stage` 键）后立刻按知识态待遇参与、标注消失。
    #[test]
    fn retrieve_down_ranks_drafts_without_excluding_them() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/知识", NoteType::Note, "预算审批流程：先报后批。");
        let draft = seed_draft(&v, "Notes/草稿", "预算审批流程：先报后批。");

        let found = v
            .retrieve("预算审批流程", &RetrieveOptions::default())
            .unwrap();
        let paths: Vec<&str> = found.passages.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths.len(), 2, "记录态默认参与：{paths:?}");
        assert_eq!(paths[0], "Notes/知识.md", "知识态排在前面：{paths:?}");
        let draft_hit = found
            .passages
            .iter()
            .find(|p| p.path == draft)
            .expect("记录态命中在场");
        assert!(draft_hit.draft, "命中须带「来自记录」的事实");
        assert!(
            !found
                .passages
                .iter()
                .any(|p| p.path.contains("知识") && p.draft),
            "知识态不得被误标"
        );

        // 透明行的一键整组排除（一次查询有效、不改设置）。
        let filtered = v
            .retrieve(
                "预算审批流程",
                &RetrieveOptions {
                    exclude_drafts: true,
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            filtered
                .passages
                .iter()
                .map(|p| p.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Notes/知识.md"]
        );

        // 沉淀：去掉 stage 键 → 同一篇立刻按知识态参与，标注消失。
        v.write(
            &draft,
            "---\nluna:\n  schema: 1\n  type: note\n---\n预算审批流程：先报后批。\n",
        )
        .unwrap();
        let after = v
            .retrieve("预算审批流程", &RetrieveOptions::default())
            .unwrap();
        assert!(
            after.passages.iter().all(|p| !p.draft),
            "沉淀后不该再有任何「来自记录」的标注"
        );
        assert_eq!(after.passages.len(), 2);
    }

    /// `@` 定范围（PRD-16-009）：三种形态各自把这一次检索限定其中，判定全在 Rust。
    #[test]
    fn retrieve_honours_at_mention_scope() {
        use crate::retrieve::RetrieveScope;
        let (_dir, v) = open_vault();
        let inside = seed_note(
            &v,
            "工作/甲",
            NoteType::Note,
            "预算审批流程说明。\n\n#项目/luna\n",
        );
        let outside = seed_note(&v, "私人/乙", NoteType::Note, "预算审批流程补充。");

        // 无范围 = 全库（既有口径不变）。
        assert_eq!(
            v.retrieve("预算审批流程", &RetrieveOptions::default())
                .unwrap()
                .passages
                .len(),
            2
        );

        for scope in [
            RetrieveScope::Folder("工作".into()),
            RetrieveScope::Note(inside.clone()),
            // 父标签覆盖子标签：`@项目` 认得 `#项目/luna`。
            RetrieveScope::Tag("项目".into()),
        ] {
            let hit = v
                .retrieve(
                    "预算审批流程",
                    &RetrieveOptions {
                        scope: vec![scope.clone()],
                        ..RetrieveOptions::default()
                    },
                )
                .unwrap();
            let paths: Vec<&str> = hit.passages.iter().map(|p| p.path.as_str()).collect();
            assert_eq!(paths, vec![inside.as_str()], "范围 {scope:?} 未收窄");
            // 范围外的那篇从未被读过，故不能算进「已检索 N 篇」——那个数字是真实工作量。
            assert_eq!(hit.scanned, 1, "范围 {scope:?} 的已检索数虚报");
        }

        // 多条提及取并集（`@工作 @私人` 读作"在这两处里找"，取交集会清空结果）。
        let union = v
            .retrieve(
                "预算审批流程",
                &RetrieveOptions {
                    scope: vec![
                        RetrieveScope::Folder("工作".into()),
                        RetrieveScope::Folder("私人".into()),
                    ],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert_eq!(union.passages.len(), 2, "多条提及是并集不是交集");
        assert!(union.passages.iter().any(|p| p.path == outside));
    }

    /// **语义路能捞回「字面一个词都没命中」的笔记**——这是批次 4 的全部存在理由。
    ///
    /// S16 实测三条跨语言查询的字面命中率为 0。若语义命中仍要过 `score_note` 才能拿到摘录，
    /// 它们会在最后一步被系统性丢掉：检索做了、结果一条也进不了上下文。
    /// **语义路的相对落差门槛**（`SEMANTIC_RELATIVE_FLOOR`，真机第一轮）：库里没有答案时，
    /// 语义照样把"同语言同领域但无关"的笔记按 0.4 上下的余弦排进来，融合后进了上下文，
    /// 回答被无关材料带偏。门槛是**相对本轮最高分**的（ADR-027 明令不设绝对阈值）。
    #[test]
    fn semantic_tail_below_the_relative_floor_never_enters_context() {
        use crate::semantic::SemanticHit;
        let (_dir, v) = open_vault();
        let good = seed_note(
            &v,
            "Notes/限流",
            NoteType::Note,
            "限流策略最终定为令牌桶，每秒 200 次。",
        );
        let weak = seed_note(&v, "Notes/无关", NoteType::Note, "周末去爬山的装备清单。");
        let hit = |path: &str, score: f32| SemanticHit {
            path: path.to_owned(),
            chunk_seq: 0,
            line_start: 6,
            score,
        };
        let found = v
            .retrieve(
                "限流那件事最后怎么定的",
                &RetrieveOptions {
                    // 0.62 是本轮最高；0.41 只有它的 66%——低于 0.80 的相对门槛。
                    semantic: vec![hit(&good, 0.62), hit(&weak, 0.41)],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        let paths: Vec<&str> = found.passages.iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&good.as_str()), "真答案必须在：{paths:?}");
        assert!(
            !paths.contains(&weak.as_str()),
            "远低于本轮最高分的长尾不该进上下文：{paths:?}"
        );

        // 同一篇在**另一轮**里若是最高分，它照常进——门槛是相对的，不是给某篇判死刑。
        let alone = v
            .retrieve(
                "爬山装备",
                &RetrieveOptions {
                    semantic: vec![hit(&weak, 0.41)],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(
            alone.passages.iter().any(|p| p.path == weak),
            "相对门槛不该把一篇笔记永久打死"
        );
    }

    /// **同一内容的两个层次不同时进上下文**（真机第一轮：一次问答的出处里，一篇转写与
    /// 它的蒸馏纪要各占一条，同一份内容占了两份预算、看起来还像重复）。
    #[test]
    fn a_transcript_gives_way_to_its_own_distilled_note() {
        let (_dir, v) = open_vault();
        let transcript = seed_note(
            &v,
            "Transcripts/会议",
            NoteType::Transcript,
            "我们讨论了限流策略，最后定为令牌桶。",
        );
        v.write_distilled(
            &transcript,
            "Notes/会议 · 纪要.md",
            "限流策略定为令牌桶。",
            "meeting",
            false,
            None,
        )
        .unwrap();

        let found = v.retrieve("限流策略", &RetrieveOptions::default()).unwrap();
        let paths: Vec<&str> = found.passages.iter().map(|p| p.path.as_str()).collect();
        assert!(
            paths.contains(&"Notes/会议 · 纪要.md"),
            "知识层必须在：{paths:?}"
        );
        assert!(
            !paths.contains(&transcript.as_str()),
            "它的蒸馏纪要在场时，转写不该再占一份预算：{paths:?}"
        );

        // 反过来：只有资料在场时它照常进（那是唯一的材料来源）。
        let orphan = seed_note(
            &v,
            "Transcripts/没整理过",
            NoteType::Transcript,
            "另一场会也聊了限流策略。",
        );
        let found = v.retrieve("限流策略", &RetrieveOptions::default()).unwrap();
        assert!(
            found.passages.iter().any(|p| p.path == orphan),
            "没有纪要的资料是唯一来源，必须留下"
        );
    }

    #[test]
    fn semantic_hits_survive_even_with_zero_keyword_overlap() {
        use crate::semantic::SemanticHit;
        let (_dir, v) = open_vault();
        let target = seed_note(
            &v,
            "Notes/gateway",
            NoteType::Note,
            "The rate limiting policy uses a token bucket at 200 rps.",
        );
        // 中文问句与英文笔记**字面零交集**：纯关键词路必然一条都不出。
        let question = "限流策略定了吗";
        let keyword_only = v.retrieve(question, &RetrieveOptions::default()).unwrap();
        assert!(
            keyword_only.passages.is_empty(),
            "前提不成立就测不出东西：{:?}",
            keyword_only.passages
        );

        let hybrid = v
            .retrieve(
                question,
                &RetrieveOptions {
                    semantic: vec![SemanticHit {
                        path: target.clone(),
                        chunk_seq: 0,
                        line_start: 6,
                        score: 0.59,
                    }],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert_eq!(hybrid.passages.len(), 1, "语义命中必须能进上下文");
        assert_eq!(hybrid.passages[0].path, target);
        assert!(
            hybrid.passages[0].text.contains("token bucket"),
            "摘录要真的抽到那段内容：{:?}",
            hybrid.passages[0].text
        );
        // 语义路多读了一次盘，故「已检索 N 篇」如实 +1（不能白报也不能漏报）。
        assert_eq!(hybrid.scanned, keyword_only.scanned + 1);
    }

    /// **降级路径与批次 1 的纯内核逐字节一致**：`semantic` 为空时融合必须整段短路。
    ///
    /// 缺模型 / 用户关掉语义 / 矩阵还没建好都会走这条路。PRD-16-029 承诺的是"界面零变化"，
    /// 而那要求降级不是"另一套排序"，是**同一个结果**。
    ///
    /// 基线**必须独立算**（直接跑 `score_note` + `pack_passages` 这两个批次 1 的纯函数），
    /// 不能拿 `retrieve` 的另一次调用来比——那样两边走的是同一条代码路径，融合逻辑一旦
    /// 误伤空语义的情形，两边会**一起错**而测试照绿。这条测试的第一版正是那样写的，
    /// 反向验证时（把短路条件改成 `if true`）它没有变红，故重写成现在这样。
    #[test]
    fn empty_semantic_matches_the_batch_one_kernel_exactly() {
        use crate::retrieve::{pack_passages, question_terms, score_note};
        let (_dir, v) = open_vault();
        let paths = [
            seed_note(&v, "Notes/甲", NoteType::Note, "预算审批流程走三级签字。"),
            seed_note(&v, "Notes/乙", NoteType::Note, "预算审批流程的补充说明。"),
            seed_note(
                &v,
                "Transcripts/丙",
                NoteType::Transcript,
                "我们聊了预算审批流程。",
            ),
        ];
        let question = "预算审批流程";
        let options = RetrieveOptions::default();

        // 独立基线：批次 1 的两个纯函数直接算一遍（不经 `retrieve`）。
        let terms = question_terms(question);
        let mut expected: Vec<crate::retrieve::Passage> = Vec::new();
        for path in &paths {
            let content = v.read_note_text(path).unwrap();
            let meta = v.read(path).unwrap().meta;
            if let Some(passage) = score_note(
                path,
                &luna_sync_core::links::title_of(path),
                meta.note_type,
                meta.stage(),
                &content,
                &terms.score,
            ) {
                expected.push(passage);
            }
        }
        let expected = pack_passages(expected, &options);
        assert!(!expected.is_empty(), "前提不成立就测不出东西");

        let actual = v.retrieve(question, &options).unwrap();
        assert_eq!(
            actual.passages, expected,
            "空语义必须是纯粹的短路：融合与名次回写一步都不该发生"
        );
    }

    /// 「已检索 N 篇」不双计（验收轮缺陷）：关键词路读过但打分为 None 的笔记，
    /// 被语义命中时会再读一次抽摘录——但它只被"检索"了一次。
    ///
    /// 构造：让 FTS 索引陈旧（绕开 Vault 直接改文件），关键词路按旧索引把它当候选、
    /// 读盘后一个词都对不上 → scanned+1、无 passage；语义路再命中它 → 修复前会再 +1。
    #[test]
    fn semantic_rescan_of_a_scored_none_note_is_not_double_counted() {
        use crate::semantic::SemanticHit;
        let (_dir, v) = open_vault();
        let rel = seed_note(&v, "Notes/甲", NoteType::Note, "预算审批流程走三级签字。");
        // 直接改磁盘文件（不经 Vault::write → 索引不更新）：FTS 里还留着「预算审批」。
        std::fs::write(
            v.root().join(&rel),
            "---\nluna:\n  schema: 1\n  type: note\n---\n内容已经完全换掉了。\n",
        )
        .unwrap();

        let hit = v
            .retrieve(
                "预算审批流程",
                &RetrieveOptions {
                    semantic: vec![SemanticHit {
                        path: rel.clone(),
                        chunk_seq: 0,
                        line_start: 6,
                        score: 0.7,
                    }],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            hit.scanned, 1,
            "同一篇只读作一次「已检索」，关键词路与语义路各读一遍不等于检索了两篇"
        );
        // 语义路照常给出摘录（它存在的意义正是捞回字面不命中的笔记）。
        assert_eq!(hit.passages.len(), 1);
    }

    /// 语义路**不是绕过三道闸门的后门**：类型排除 / 剔除 / `@` 范围照样生效。
    #[test]
    fn semantic_path_honours_type_exclusion_and_scope() {
        use crate::retrieve::RetrieveScope;
        use crate::semantic::SemanticHit;
        let (_dir, v) = open_vault();
        let chat = seed_note(&v, "Chats/某次对话", NoteType::Chat, "量子退火的原理如下。");
        let note = seed_note(&v, "工作/甲", NoteType::Note, "量子退火的工程实践。");
        let hits = vec![
            SemanticHit {
                path: chat.clone(),
                chunk_seq: 0,
                line_start: 6,
                score: 0.9,
            },
            SemanticHit {
                path: note.clone(),
                chunk_seq: 0,
                line_start: 6,
                score: 0.8,
            },
        ];

        // 对话笔记默认排除（ADR-026 ②b）——语义分再高也不进。
        let default_run = v
            .retrieve(
                "量子退火",
                &RetrieveOptions {
                    semantic: hits.clone(),
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(
            default_run.passages.iter().all(|p| p.path != chat),
            "语义路绕过了类型排除"
        );
        assert!(default_run.passages.iter().any(|p| p.path == note));

        // 显式剔除照样生效。
        let excluded = v
            .retrieve(
                "量子退火",
                &RetrieveOptions {
                    semantic: hits.clone(),
                    excluded: vec![note.clone()],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(
            excluded.passages.is_empty(),
            "语义路绕过了剔除：{:?}",
            excluded.passages
        );

        // `@` 范围照样生效（PRD-16-009）。
        let scoped = v
            .retrieve(
                "量子退火",
                &RetrieveOptions {
                    semantic: hits,
                    scope: vec![RetrieveScope::Folder("私人".into())],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(
            scoped.passages.is_empty(),
            "语义路绕过了 @ 范围：{:?}",
            scoped.passages
        );
    }

    /// 建库的指纹幂等：正文没变即「已最新」；正文或向量空间任一字段变化即过期。
    #[test]
    fn embedding_freshness_gates_on_body_hash_and_full_index_identity() {
        let (_dir, v) = open_vault();
        let rel = seed_note(
            &v,
            "Notes/甲",
            NoteType::Note,
            "这一段内容足够长以切出一个分片来做嵌入。",
        );
        let identity = embedding_identity("bge-m3", "sha256:one", 2);
        let chunks = v.chunks_of(&rel, crate::BGE_M3_CHUNK_POLICY).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(!v.embeddings_fresh(&rel, &identity).unwrap(), "从未嵌入过");

        v.put_embeddings(&rel, &identity, &chunks, &[vec![1.0, 0.0]])
            .unwrap();
        assert!(v.embeddings_fresh(&rel, &identity).unwrap());
        for changed in [
            embedding_identity("qwen3-embedding:0.6b", "sha256:one", 2),
            embedding_identity("bge-m3", "sha256:two", 2),
            embedding_identity("bge-m3", "sha256:one", 3),
        ] {
            assert!(
                !v.embeddings_fresh(&rel, &changed).unwrap(),
                "目标、权重摘要或维度变化都必须使旧空间过期：{changed:?}"
            );
        }

        // 改一个字 → 指纹破 → 过期。
        let doc = v.read(&rel).unwrap();
        v.write(&rel, &doc.content.replace("足够长", "很长"))
            .unwrap();
        assert!(!v.embeddings_fresh(&rel, &identity).unwrap());

        // 条数不符即整篇不写（对不上号的向量比没有更糟）。
        assert!(v.put_embeddings(&rel, &identity, &chunks, &[]).is_err());
    }

    /// **分片级复用**：改一行只有那一两片需要重嵌，其余向量原样可用。
    ///
    /// 编辑器每 ~800ms 自动保存一次、每次**正文**指纹都变——只按正文指纹判定的话，一篇
    /// 40 片的长笔记会在每个编辑停顿处整篇重嵌（S16：~0.25 秒/片 ≈ 10 秒 GPU）。
    /// 这条断言锁住"未改动分片的向量能按分片指纹被认回来"这一件事。
    #[test]
    fn unchanged_chunks_keep_their_vectors_across_edits() {
        let (_dir, v) = open_vault();
        // 两个小节 → 两片；只改第二片的内容。
        let body_a = "## 甲\n\n甲的内容足够长以越过最短分片门槛，这里再补一些字。\n\n## 乙\n\n乙的内容原样。\n";
        let rel = seed_note(&v, "Notes/两节", NoteType::Note, body_a);
        let identity = embedding_identity("bge-m3", "sha256:one", 2);
        let chunks = v.chunks_of(&rel, crate::BGE_M3_CHUNK_POLICY).unwrap();
        assert_eq!(chunks.len(), 2, "前提：两个小节切成两片");
        v.put_embeddings(&rel, &identity, &chunks, &[vec![1.0, 0.0], vec![0.0, 1.0]])
            .unwrap();

        // 改第二片。
        let doc = v.read(&rel).unwrap();
        v.write(
            &rel,
            &doc.content.replace("乙的内容原样。", "乙的内容改过了。"),
        )
        .unwrap();
        let after = v.chunks_of(&rel, crate::BGE_M3_CHUNK_POLICY).unwrap();
        let reusable = v.reusable_vectors(&rel, &identity).unwrap();

        // 第一片的文本没变 → 它的向量必须能按分片指纹认回来。
        let first = crate::chunk_fingerprint(&after[0].text);
        assert_eq!(
            reusable.get(&first),
            Some(&vec![1.0, 0.0]),
            "未改动的分片必须复用旧向量，否则改一行就是整篇重嵌"
        );
        // 第二片变了 → 认不回来，必须重嵌。
        let second = crate::chunk_fingerprint(&after[1].text);
        assert!(
            !reusable.contains_key(&second),
            "改过的分片绝不能复用旧向量（那是拿旧内容的向量代表新内容）"
        );
        // 换模型 → 一片都不复用（不同模型的向量空间不可比）。
        let other = embedding_identity("other-embedding", "sha256:other", 2);
        assert!(v.reusable_vectors(&rel, &other).unwrap().is_empty());
    }

    /// 全库重扫**不得**清掉向量：`scan()` 每次开库都跑，清了就是每次启动重跑 40 分钟。
    #[test]
    fn rescan_keeps_embeddings_but_deleting_the_note_drops_them() {
        let (_dir, v) = open_vault();
        let rel = seed_note(
            &v,
            "Notes/甲",
            NoteType::Note,
            "一段足够长的内容用于嵌入测试。",
        );
        let identity = embedding_identity("bge-m3", "sha256:one", 2);
        let chunks = v.chunks_of(&rel, crate::BGE_M3_CHUNK_POLICY).unwrap();
        v.put_embeddings(&rel, &identity, &chunks, &[vec![1.0, 0.0]])
            .unwrap();
        assert_eq!(v.embedding_counts(&identity).unwrap(), (1, 1));

        v.scan().unwrap();
        assert_eq!(
            v.embedding_counts(&identity).unwrap(),
            (1, 1),
            "重扫是「从文件重算投影」，而向量不是从文件算得出来的（要跑模型）"
        );

        // 笔记真的没了 → 向量一并忘掉（否则矩阵里留着指向虚空的行）。
        v.index.forget_note(&rel).unwrap();
        assert_eq!(v.embedding_counts(&identity).unwrap(), (0, 0));
    }

    /// 预检盘点按**分片**报耗时依据（按篇报会把 40 分钟说成 4 分钟）。
    #[test]
    fn embed_scan_counts_chunks_not_just_notes() {
        let (_dir, v) = open_vault();
        let long = seed_note(
            &v,
            "Notes/长",
            NoteType::Note,
            &"这是一段很长的内容需要切成多个分片来嵌入。".repeat(200),
        );
        seed_note(&v, "Notes/短", NoteType::Note, "短");

        let identity = embedding_identity("bge-m3", "sha256:one", 2);
        let scan = v.embed_scan(&identity, crate::BGE_M3_CHUNK_POLICY).unwrap();
        assert!(scan.pending.contains(&long), "长笔记待嵌入");
        assert!(
            scan.pending_chunks > 1,
            "长笔记应切出多片：{}",
            scan.pending_chunks
        );
        assert_eq!(scan.empty, 1, "无可嵌入内容的那篇既不待办也不算已完成");
        assert_eq!(scan.fresh, 0);

        // 嵌完即转入 fresh 桶。
        let chunks = v.chunks_of(&long, crate::BGE_M3_CHUNK_POLICY).unwrap();
        let vectors: Vec<Vec<f32>> = chunks.iter().map(|_| vec![1.0, 0.0]).collect();
        v.put_embeddings(&long, &identity, &chunks, &vectors)
            .unwrap();
        let after = v.embed_scan(&identity, crate::BGE_M3_CHUNK_POLICY).unwrap();
        assert!(after.pending.is_empty());
        assert_eq!(after.fresh, 1);
        assert_eq!(after.fresh_chunks, chunks.len() as u32);
    }

    /// 向量 BLOB 往返无损，且矩阵按模型隔离（换模型后旧向量绝不混进来）。
    #[test]
    fn vector_matrix_round_trips_and_isolates_models() {
        let (_dir, v) = open_vault();
        let rel = seed_note(
            &v,
            "Notes/甲",
            NoteType::Note,
            "一段足够长的内容用于嵌入测试。",
        );
        let identity = embedding_identity("bge-m3", "sha256:one", 3);
        let chunks = v.chunks_of(&rel, crate::BGE_M3_CHUNK_POLICY).unwrap();
        let vector = vec![0.5, -0.25, 0.125];
        v.put_embeddings(&rel, &identity, &chunks, std::slice::from_ref(&vector))
            .unwrap();

        let matrix = v.vector_matrix(&identity).unwrap();
        assert_eq!(matrix.len(), 1);
        assert_eq!(matrix.dim(), 3);
        let hits = matrix.search(&vector, 1);
        // f32 往返必须逐位无损（小端字节序），否则相似度会莫名其妙地漂。
        assert!((hits[0].score - crate::semantic::dot(&vector, &vector)).abs() < f32::EPSILON);
        assert_eq!(hits[0].line_start, chunks[0].line_start);

        // 另一个模型的矩阵是空的——绝不把 bge-m3 的向量拿去与别的模型的查询比。
        let other = embedding_identity("qwen3-embedding:0.6b", "sha256:qwen", 3);
        assert!(v.vector_matrix(&other).unwrap().is_empty());
        // 清场只删别人的。
        assert_eq!(v.prune_stale_embeddings(&identity).unwrap(), 0);
        assert_eq!(v.prune_stale_embeddings(&other).unwrap(), 1);
        assert!(v.vector_matrix(&identity).unwrap().is_empty());
    }

    #[test]
    fn retrieve_honours_per_query_exclusions() {
        let (_dir, v) = open_vault();
        let a = seed_note(&v, "Notes/甲", NoteType::Note, "预算审批流程说明。");
        seed_note(&v, "Notes/乙", NoteType::Note, "预算审批流程补充。");

        let all = v
            .retrieve("预算审批流程", &RetrieveOptions::default())
            .unwrap();
        assert_eq!(all.passages.len(), 2);

        let filtered = v
            .retrieve(
                "预算审批流程",
                &RetrieveOptions {
                    excluded: vec![a.clone()],
                    ..RetrieveOptions::default()
                },
            )
            .unwrap();
        assert!(
            filtered.passages.iter().all(|p| p.path != a),
            "剔除即不出现"
        );
    }

    #[test]
    fn retrieve_citation_anchor_survives_edits_and_degrades_honestly() {
        let (_dir, v) = open_vault();
        let path = seed_note(
            &v,
            "Notes/结论",
            NoteType::Note,
            "## 本周结论\n\n压缩比提升到百分之四十是本次的关键成果。",
        );
        let hit = v
            .retrieve("压缩比提升到多少", &RetrieveOptions::default())
            .unwrap();
        let anchor = hit.passages[0].anchor.clone();

        // 用户在上方插入内容 → 行号漂移，引文仍在 → 跳转照常准确。
        let doc = v.read(&path).unwrap();
        let shifted = doc
            .content
            .replace("## 本周结论", "## 新增一节\n\n补充说明。\n\n## 本周结论");
        v.write(&path, &shifted).unwrap();
        let moved = v.resolve_citation(&path, &anchor).unwrap();
        assert!(moved.is_exact(), "找到引文即准确跳转：{moved:?}");

        // 引文被删 → 必须如实降级，绝不猜一个行号冒充精确命中。
        let doc = v.read(&path).unwrap();
        let gutted = doc
            .content
            .replace("压缩比提升到百分之四十是本次的关键成果。", "内容已重写。");
        v.write(&path, &gutted).unwrap();
        let degraded = v.resolve_citation(&path, &anchor).unwrap();
        assert!(
            !degraded.is_exact(),
            "退化命中必须标记为不精确：{degraded:?}"
        );
    }

    #[test]
    fn related_panel_finds_unlinked_mentions_but_skips_existing_links() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/知识图谱", NoteType::Note, "定义。");
        let host = seed_note(
            &v,
            "Notes/读书笔记",
            NoteType::Note,
            "今天读到知识图谱这一章，很有启发。",
        );

        let found = v.related_notes(&host).unwrap();
        assert_eq!(found.mentions.len(), 1, "{:?}", found.mentions);
        assert_eq!(found.mentions[0].text, "知识图谱");

        // 转成正式链接后不再重复提议（否则面板会一直催用户做已经做过的事）。
        v.link_mention(&host, &found.mentions[0]).unwrap();
        let body = v.read(&host).unwrap().content;
        assert!(body.contains("[知识图谱](知识图谱.md)"), "{body}");
        assert!(v.related_notes(&host).unwrap().mentions.is_empty());
    }

    #[test]
    fn link_mention_refuses_source_material_and_stale_positions() {
        let (_dir, v) = open_vault();
        seed_note(&v, "Notes/知识图谱", NoteType::Note, "定义。");
        let transcript = seed_note(
            &v,
            "Transcripts/讲座",
            NoteType::Transcript,
            "他提到知识图谱的构建方法。",
        );

        // 资料笔记只读：往逐字稿里插链接会污染原文，判定必须在 Rust。
        let found = v.related_notes(&transcript).unwrap();
        assert_eq!(found.mentions.len(), 1, "发现照常显示");
        assert!(
            v.link_mention(&transcript, &found.mentions[0]).is_err(),
            "但写入必须被拒"
        );

        // 面板数据陈旧时按坐标盲改会毁掉无关内容 → 必须逐字节复核后拒绝。
        let host = seed_note(&v, "Notes/宿主", NoteType::Note, "提到知识图谱这件事。");
        let mention = v.related_notes(&host).unwrap().mentions.remove(0);
        v.write(
            &host,
            "---\nluna:\n  schema: 1\n  type: note\n---\n完全换了内容。\n",
        )
        .unwrap();
        assert!(
            v.link_mention(&host, &mention).is_err(),
            "陈旧坐标必须被拒绝，绝不按旧偏移改写"
        );
    }

    #[test]
    fn distill_state_tracks_absent_fresh_stale_and_edited() {
        // PRD-06-044 / ADR-024 四态判定：全部依据都在文件里（指针 + 两个指纹），
        // 故重启/换设备/外部编辑器改动都自然成立。
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n原始转写")
            .unwrap();
        assert_eq!(v.distill_state("会议.md").unwrap(), DistillState::Absent);

        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 决议\n\n- 上线延后一周",
            "meeting",
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            v.distill_state("会议.md").unwrap(),
            DistillState::Present {
                path: "Notes/会议 · 纪要.md".into(),
                stale: false,
                edited: false,
                output_truncated: None,
                template: Some("meeting".into()),
                full_text: Some(false),
            },
            "刚生成：既不陈旧也未被编辑，且模板与整理稿开关随之落盘"
        );

        // 源被二次转写改写 → stale，但用户没动过纪要 → 可安全重生成。
        // 按真实写回方式**保留 frontmatter 只换正文**（`rewrite_recording_transcript`
        // 与整篇校对都是这么写的）——整体覆写会连 `distill_note` 指针一起抹掉。
        let meta = v.read("会议.md").unwrap().meta;
        let rewritten = crate::frontmatter::update_luna("权威重转写文本", &meta).unwrap();
        v.write("会议.md", &rewritten).unwrap();
        let state = v.distill_state("会议.md").unwrap();
        assert!(
            matches!(
                state,
                DistillState::Present {
                    stale: true,
                    edited: false,
                    ..
                }
            ),
            "源变更后应为 stale 且未编辑：{state:?}"
        );

        // 用户动手改了纪要 → edited，此后自动路径必须止步。
        let current = v.read("Notes/会议 · 纪要.md").unwrap().content;
        v.write(
            "Notes/会议 · 纪要.md",
            &format!("{current}\n\n补一条我自己的判断。\n"),
        )
        .unwrap();
        let state = v.distill_state("会议.md").unwrap();
        assert!(
            matches!(state, DistillState::Present { edited: true, .. }),
            "用户编辑后必须判为 edited（这是「绝不静默覆盖」的唯一依据）：{state:?}"
        );
    }

    #[test]
    fn set_note_type_keeps_body_and_third_party_keys_byte_for_byte() {
        // 速记「升级为笔记」（PRD-02-055）只该动 `luna.type` 这一个键——用户的正文与
        // 第三方 frontmatter 键（Obsidian 互操作命门 §4.4）必须逐字节保留。
        let (_dir, v) = open_vault();
        let original =
            "---\ntags: [灵感]\ncustom: 保留我\nluna:\n  type: quick-note\n---\n\n随口说的一句话\n";
        v.write("速记.md", original).unwrap();
        v.set_note_type("速记.md", crate::frontmatter::NoteType::Note)
            .unwrap();

        let doc = v.read("速记.md").unwrap();
        assert_eq!(doc.meta.note_type, crate::frontmatter::NoteType::Note);
        assert_eq!(body_of(&doc.content), body_of(original), "正文不得改动");
        assert!(doc.content.contains("custom: 保留我"), "第三方键须保留");
        assert!(doc.content.contains("tags: [灵感]"), "用户标签须保留");
    }

    #[test]
    fn upgrading_a_quick_note_also_drops_the_explicit_stage_key() {
        // PRD-06-053：速记的沉淀**就是**「升级为笔记」——type 改写已含毕业语义。
        // 若显式 `stage` 被留在原处，升级完的那一篇仍判记录态（`NoteStage::resolve`
        // 对任何取值都判 Draft），"毕业"就只改了个类型名而已。
        let (_dir, v) = open_vault();
        v.write(
            "速记.md",
            "---\ncustom: 保留我\nluna:\n  type: quick-note\n  stage: draft\n---\n\n随口说的\n",
        )
        .unwrap();
        v.set_note_type("速记.md", crate::frontmatter::NoteType::Note)
            .unwrap();

        let doc = v.read("速记.md").unwrap();
        assert_eq!(doc.meta.stage(), NoteStage::Knowledge, "升级后即知识态");
        assert!(!doc.content.contains("stage:"), "显式 stage 键须被去除");
        assert!(doc.content.contains("custom: 保留我"), "第三方键须保留");
    }

    #[test]
    fn promote_to_knowledge_removes_only_the_stage_line() {
        // PRD-06-053 验收：文件 diff **仅少 `stage` 一行**（round-trip §4.4）。
        let (_dir, v) = open_vault();
        let original = "---\ntags: [会议]\ncustom: 保留我\nluna:\n  schema: 1\n  type: note\n  stage: draft\n---\n\n草稿正文\n";
        v.write("草稿.md", original).unwrap();

        let written = v.promote_to_knowledge("草稿.md").unwrap();
        let after = v.read("草稿.md").unwrap();
        assert_eq!(written.as_deref(), Some(after.content.as_str()));
        assert_eq!(after.meta.stage(), NoteStage::Knowledge);
        assert_eq!(
            after.content,
            original.replace("  stage: draft\n", ""),
            "除 stage 那一行外逐字节不变"
        );

        // 已是知识态 → 不写盘（`None`），避免无谓快照与重索引。
        assert_eq!(v.promote_to_knowledge("草稿.md").unwrap(), None);
    }

    #[test]
    fn promote_to_knowledge_does_not_graduate_a_default_draft_quick_note() {
        // 缺省记录态的速记没有 `stage` 键可删——它的沉淀是「升级为笔记」那条路
        // （PRD-06-053）。本方法在这里必须**什么都不做**，而不是假装成功。
        let (_dir, v) = open_vault();
        v.write("速记.md", "---\nluna:\n  type: quick-note\n---\n\n碎片\n")
            .unwrap();
        assert_eq!(v.promote_to_knowledge("速记.md").unwrap(), None);
        assert_eq!(
            v.read("速记.md").unwrap().meta.stage(),
            NoteStage::Draft,
            "仍是记录态"
        );
    }

    #[test]
    fn word_goal_round_trips_without_touching_body_or_other_keys() {
        let (_dir, v) = open_vault();
        let original =
            "---\ntags: [写作]\ncustom: 保留我\nluna:\n  schema: 1\n  type: note\n---\n\n正文\n";
        v.write("稿.md", original).unwrap();

        let written = v.set_word_goal("稿.md", Some(2000)).unwrap();
        let after = v.read("稿.md").unwrap();
        assert_eq!(written.as_deref(), Some(after.content.as_str()));
        assert_eq!(after.meta.word_goal, Some(2000));
        assert!(after.content.contains("custom: 保留我"));
        assert_eq!(body_of(&after.content), body_of(original));

        // 同值不写盘；清掉逐字节回到原样。
        assert_eq!(v.set_word_goal("稿.md", Some(2000)).unwrap(), None);
        v.set_word_goal("稿.md", None).unwrap();
        assert_eq!(v.read("稿.md").unwrap().content, original);
    }

    #[test]
    fn mark_as_draft_adds_only_the_stage_line_and_round_trips_with_settling() {
        // PRD-06-058：「标记为记录」是 `promote_to_knowledge` 的反向动作，走同一个咽喉。
        // 判据是**往返之后逐字节回到原样**——成熟度双向可改若留下痕迹，用户下次打开
        // 看到的就是一个被 Lunarca 改过格式的文件（§4.4 round-trip 的同一条承诺）。
        let (_dir, v) = open_vault();
        let original =
            "---\ntags: [会议]\ncustom: 保留我\nluna:\n  schema: 1\n  type: note\n---\n\n知识正文\n";
        v.write("知识.md", original).unwrap();

        let written = v.mark_as_draft("知识.md").unwrap();
        let after = v.read("知识.md").unwrap();
        assert_eq!(written.as_deref(), Some(after.content.as_str()));
        assert_eq!(after.meta.stage(), NoteStage::Draft, "标记后即记录态");
        assert!(after.content.contains("custom: 保留我"), "第三方键须保留");
        assert_eq!(body_of(&after.content), body_of(original), "正文不得改动");

        // 已是记录态 → 不写盘（重复动作是无操作，不是又一次快照）。
        assert_eq!(v.mark_as_draft("知识.md").unwrap(), None);

        // 往返：沉淀回去必须逐字节回到原样。
        v.promote_to_knowledge("知识.md").unwrap();
        assert_eq!(v.read("知识.md").unwrap().content, original);
    }

    #[test]
    fn mark_as_draft_refuses_material_topic_pages_and_the_knowledge_map() {
        // PRD-06-058 的三类拒绝，各有各的理由（都不是"以防万一"）：
        // 资料上 `stage` 被 `NoteStage::resolve` 忽略，写下去就是一个谁都不读的键；
        // 主题页与知识地图是 AI 维护页，成熟度由所有权指纹表达，两套语义会打架。
        let (_dir, v) = open_vault();
        v.write("录音.md", "---\nluna:\n  type: recording\n---\n\n转写\n")
            .unwrap();
        v.write(
            "主题页.md",
            "---\nluna:\n  type: note\n  topic: true\n---\n\n综述\n",
        )
        .unwrap();
        v.write(
            "地图.md",
            "---\nluna:\n  type: note\n  knowledge_map: true\n---\n\n路由\n",
        )
        .unwrap();

        for rel in ["录音.md", "主题页.md", "地图.md"] {
            let err = v.mark_as_draft(rel).unwrap_err();
            assert_eq!(err.code(), "VAULT_NOT_MARKABLE_AS_DRAFT", "{rel} 应被拒绝");
            assert_eq!(
                v.read(rel).unwrap().meta.stage_raw,
                None,
                "{rel} 被拒后不得留下半截写入"
            );
        }
    }

    #[test]
    fn tree_marks_only_knowledge_markdown_as_markable() {
        // PRD-06-058：树行右键「标记为记录」的判据由 Rust 给出（ADR-003）——前端不自己
        // 拼「知识态且非主题页且非资料且是 .md」，否则编辑器右键那一份必然与它分叉。
        let (_dir, v) = open_vault();
        v.write("知识.md", "---\nluna:\n  type: note\n---\n\n正文\n")
            .unwrap();
        v.write(
            "草稿.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n\n正文\n",
        )
        .unwrap();
        v.write("录音.md", "---\nluna:\n  type: recording\n---\n\n转写\n")
            .unwrap();
        v.write(
            "主题页.md",
            "---\nluna:\n  type: note\n  topic: true\n---\n\n综述\n",
        )
        .unwrap();
        std::fs::write(v.root().join("图片.png"), b"not markdown").unwrap();

        let tree = v.tree().unwrap();
        let markable = |name: &str| {
            tree.children
                .iter()
                .find(|n| n.name == name)
                .unwrap_or_else(|| panic!("树上找不到 {name}"))
                .markable_as_draft
        };
        assert!(markable("知识.md"), "知识笔记可被标记为记录");
        assert!(!markable("草稿.md"), "已是记录态，不重复给");
        assert!(!markable("录音.md"), "资料上这个键会被忽略");
        assert!(!markable("图片.png"), "附件没有 frontmatter 可写");
        // 主题页不参与本判定——知识产物根本不进树（ADR-096 ②）。
        assert!(
            !tree.children.iter().any(|n| n.name == "主题页.md"),
            "产物不该出现在树上"
        );
    }

    #[test]
    fn tree_ships_typed_source_and_ingest_facts_instead_of_letting_the_ui_guess() {
        // PRD-06-048 ⑥ / ADR-003：「归入主题页」的资格由 Rust 判定后随对象下发。前端此前
        // 拿「资料归档目录」比路径猜——目录被改名、笔记被移出归档目录、主题页躺在
        // `Notes/` 里，三种情形都能让它给出一个点下去必被拒的动作。
        let (_dir, v) = open_vault();
        v.write("知识.md", "---\nluna:\n  type: note\n---\n\n正文\n")
            .unwrap();
        v.write("录音.md", "---\nluna:\n  type: recording\n---\n\n转写\n")
            .unwrap();
        // 资料上带 `stage: draft` 也仍然只是资料（来源轴优先，ADR-086）——树行据此
        // 各挂各的徽标，两枚**不可能同时出现**；若它们能同时为真，同一行会既说"资料"
        // 又说"待整理"，而这两句话承诺的待遇互相矛盾。
        v.write(
            "转写.md",
            "---\nluna:\n  type: transcript\n  stage: draft\n---\n\n逐字稿\n",
        )
        .unwrap();
        v.write(
            "主题页.md",
            "---\nluna:\n  type: note\n  topic: true\n---\n\n综述\n",
        )
        .unwrap();
        v.write(
            "地图.md",
            "---\nluna:\n  type: note\n  knowledge_map: true\n---\n\n路由\n",
        )
        .unwrap();
        std::fs::create_dir_all(v.root().join("归档")).unwrap();
        v.write(
            "归档/搬过去的.md",
            "---\nluna:\n  type: note\n---\n\n正文\n",
        )
        .unwrap();
        std::fs::write(v.root().join("图片.png"), b"not markdown").unwrap();

        let tree = v.tree().unwrap();
        let node = |name: &str| {
            tree.children
                .iter()
                .find(|n| n.name == name)
                .unwrap_or_else(|| panic!("树上找不到 {name}"))
        };

        // 来源轴按 frontmatter 判，与目录无关。
        assert!(node("录音.md").is_source_material);
        assert!(node("转写.md").is_source_material);
        assert!(!node("知识.md").is_source_material);
        assert!(!node("图片.png").is_source_material, "附件不是资料笔记");

        // 主题页与知识地图连树都不进（ADR-096 ②）。
        for product in ["主题页.md", "地图.md"] {
            assert!(
                !tree.children.iter().any(|n| n.name == product),
                "{product} 不该出现在树上"
            );
        }

        // 一篇被搬进任意目录的知识笔记照旧不是资料——判据是笔记自己的类型，不是它此刻
        // 躺在哪个文件夹里。
        let archived = tree.children.iter().find(|n| n.name == "归档").unwrap();
        let moved = archived.children.first().unwrap();
        assert_eq!(moved.path, "归档/搬过去的.md");
        assert!(!moved.is_source_material);

        // 目录本身两项恒假：来源与成熟度都是**一篇笔记**的属性。
        assert!(!archived.is_source_material);

        // 两枚身份徽标的判据互斥（来源轴优先）：树行分别据 `is_source_material` 与
        // `draft` 挂「资料」「待整理」，两枚同时为真就会在同一行说两句互相矛盾的话。
        assert!(!node("转写.md").draft, "资料上的 stage 键被忽略");
        for name in ["知识.md", "录音.md", "转写.md"] {
            let n = node(name);
            assert!(
                !(n.is_source_material && n.draft),
                "{name} 同时是资料与记录——树行会同时挂两枚互相矛盾的徽标"
            );
        }
    }

    #[test]
    fn draft_inbox_collects_quick_notes_only_and_marked_notes_stay_in_the_tree() {
        // PRD-06-058 A / PRD-07-066：速记流是 **type 驱动**的捕获流——只收速记。被「标记为
        // 待整理」的普通笔记留在「笔记」空间，由文件树的淡显与徽标（`draft_paths`，stage 驱动）
        // 表达；资料不进（来源轴优先），知识不进（它已经处理完了）。
        let (_dir, v) = open_vault();
        v.write(
            "速记.md",
            "---\nluna:\n  type: quick-note\n---\n\n# 标题行\n\n正文\t换行\n",
        )
        .unwrap();
        v.write(
            "标记过的.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n\n正文\n",
        )
        .unwrap();
        v.write("知识.md", "---\nluna:\n  type: note\n---\n\n已定稿\n")
            .unwrap();
        v.write(
            "转写.md",
            "---\nluna:\n  type: transcript\n  stage: draft\n---\n\n逐字稿\n",
        )
        .unwrap();

        let (entries, _, _, total) = v.draft_inbox(&all_drafts(), 100, None).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["速记.md"], "速记流只收 quick-note");
        assert_eq!(total, 1, "徽标计数与列表同源");

        let quick = &entries[0];
        assert!(quick.quick_note);
        assert!(
            !quick.preview.contains('\n') && !quick.preview.contains('\t'),
            "预览必须是一行：{:?}",
            quick.preview
        );
        assert!(quick.preview.contains("标题行"), "预览取正文首段");
        assert!(quick.mergeable, "速记都可参与汇总");

        // 反向一半：被标记的普通笔记仍是记录态——文件树那条 stage 驱动的口径照常认得它。
        let dimmed = v.index.draft_paths().unwrap();
        assert!(dimmed.contains(&"标记过的.md".to_owned()));
        assert!(dimmed.contains(&"速记.md".to_owned()), "速记缺省即记录态");
        assert!(!dimmed.contains(&"知识.md".to_owned()));
    }

    #[test]
    fn draft_inbox_never_lists_aggregate_pages_even_if_hand_flagged() {
        // 主题页与知识地图是 `type: note` 的聚合层，给它们手写 `stage: draft` 只会让它们
        // 在文件树里淡显，**不会**落进速记流——速记流按类型收，而聚合页从来不是速记
        // （PRD-06-047 / PRD-06-058 A）。把一张聚合页当碎片再合一次是让它吞自己，故它们
        // 连进入候选的机会都没有。
        let (_dir, v) = open_vault();
        v.write("碎片.md", "---\nluna:\n  type: quick-note\n---\n\n一句话\n")
            .unwrap();
        v.write(
            "主题页.md",
            "---\nluna:\n  type: note\n  topic: true\n  stage: draft\n---\n\n综述\n",
        )
        .unwrap();
        v.write(
            "地图.md",
            "---\nluna:\n  type: note\n  knowledge_map: true\n  stage: draft\n---\n\n路由\n",
        )
        .unwrap();

        let (entries, _, _, total) = v.draft_inbox(&all_drafts(), 100, None).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["碎片.md"]);
        assert_eq!(total, 1);
        assert!(entries[0].mergeable);
    }

    #[test]
    fn draft_count_is_not_truncated_by_the_list_limit() {
        // 徽标上的数字是承诺给用户的「还剩几篇」——被列表上限截断就是谎报，
        // 故它是独立的一条 `COUNT(*)`（PRD-07-063）。
        let (_dir, v) = open_vault();
        for i in 0..5 {
            v.write(
                &format!("速记{i}.md"),
                "---\nluna:\n  type: quick-note\n---\n\n碎片\n",
            )
            .unwrap();
        }

        let (entries, _, _, total) = v.draft_inbox(&all_drafts(), 2, None).unwrap();
        assert_eq!(entries.len(), 2, "列表按上限截断");
        assert_eq!(total, 5, "总数不受上限影响");
    }

    #[test]
    fn chat_history_count_is_not_truncated_and_the_cursor_reaches_the_tail() {
        // 真机 2026-09-09 复盘：对话历史是写死 100 条的 top-N，而侧栏徽标显示的是已加载
        // 条数——第 101 场对话既不在列表里，也没被计进那个数字。这里同时钉两件事：
        // 总数是独立的 `COUNT(*)`，且游标能把全部会话穷尽。
        let (_dir, v) = open_vault();
        for i in 0..5 {
            v.write(
                &format!("对话{i}.md"),
                "---\nluna:\n  type: chat\n---\n\n聊了点什么\n",
            )
            .unwrap();
        }

        assert_eq!(
            v.count_notes_of_type(crate::frontmatter::NoteType::Chat)
                .unwrap(),
            5,
            "总数不受列表上限影响"
        );

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, next) = v
                .list_notes_of_type(crate::frontmatter::NoteType::Chat, 2, cursor.as_deref())
                .unwrap();
            assert!(page.len() <= 2, "这一页超过了请求的上限");
            seen.extend(page.into_iter().map(|entry| entry.path));
            match next {
                Some(token) => cursor = Some(token),
                None => break,
            }
        }
        seen.sort();
        assert_eq!(seen.len(), 5, "游标没能把全部会话遍历完");
    }

    #[test]
    fn replace_body_refuses_to_write_over_a_base_it_was_not_given() {
        // 真机 2026-09-09：钉到桌面之后整篇正文被清空。根因是写回没有基线——前端有一帧
        // 拿着陈旧的空串，那一下就把用户的笔记盖没了。基线让这种写入在架构上不可能：
        // 客户端没读过的内容，它说不出正确的基线。
        let (_dir, v) = open_vault();
        v.write(
            "便签.md",
            "---\nmy_own_key: 保留我\nluna:\n  type: quick-note\n---\n\n买牛奶\n",
        )
        .unwrap();
        let disk = crate::frontmatter::body_of(&v.read("便签.md").unwrap().content).to_owned();

        // 基线对不上（这里模拟"从没读过、手里是空串"）：一个字节都不许写。
        assert!(!v.replace_body("便签.md", "", "").unwrap());
        assert!(
            v.read("便签.md").unwrap().content.contains("买牛奶"),
            "正文必须原封不动"
        );

        // 基线对得上才写，且只动正文。
        assert!(v
            .replace_body("便签.md", &disk, "\n买牛奶\n买梨\n")
            .unwrap());
        let after = v.read("便签.md").unwrap();
        assert!(after.content.contains("买梨"));
        assert!(
            after.content.contains("my_own_key: 保留我"),
            "第三方键逐字节保留"
        );
        assert_eq!(after.meta.note_type, crate::NoteType::QuickNote);

        // 同值不写盘（拖动/失焦会连着发很多次保存）。
        let base = crate::frontmatter::body_of(&after.content).to_owned();
        let before = v.read("便签.md").unwrap().content;
        assert!(v.replace_body("便签.md", &base, &base).unwrap());
        assert_eq!(v.read("便签.md").unwrap().content, before);
    }

    #[test]
    fn a_sticky_edits_only_the_human_text_and_keeps_the_transcript_block() {
        // 真机 2026-10-02：语音速记钉到桌面，便签里露出转写块哨兵。便签只收发哨兵之前那段，
        // 转写块逐字节接回去。
        let (_dir, v) = open_vault();
        let machine = "\n\n<!-- luna:transcript -->\n[00:00:01.000] 今天下午去超市\n<!-- /luna:transcript -->\n";
        v.write(
            "速记.md",
            &format!(
                "---\nluna:\n  type: quick-note\n---\n\n## 优化结果\n\n今天下午去超市{machine}"
            ),
        )
        .unwrap();
        let full = crate::frontmatter::body_of(&v.read("速记.md").unwrap().content).to_owned();
        let (text, tail) = sticky_text(&full);
        assert!(!text.contains("luna:transcript"), "便签里不出现机器哨兵");
        assert!(text.ends_with("今天下午去超市"));
        assert_eq!(format!("{text}{tail}"), full, "两段相接即原文");

        assert!(v
            .replace_body("速记.md", text, "## 优化结果\n\n今天下午去超市，买牛奶")
            .unwrap());
        let after = v.read("速记.md").unwrap().content;
        assert!(after.contains("买牛奶"));
        assert!(after.ends_with(machine), "转写块逐字节保留");
    }

    #[test]
    fn pinning_a_desktop_note_only_touches_that_one_key() {
        // ADR-119 的要害：便签不是第二个对象。钉住只加一个 frontmatter 键——正文、type、
        // stage 与用户自己写的第三方键都必须逐字节不动。
        //
        // 取消钉住**不删这个键**（2026-09-09 真机修订）：它同时记着位置、尺寸与颜色，
        // 那些是笔记的属性，与"此刻在不在桌面上"是两件事。
        let (_dir, v) = open_vault();
        v.write(
            "便签.md",
            "---\nmy_own_key: 保留我\nluna:\n  type: quick-note\n---\n\n买牛奶\n",
        )
        .unwrap();

        v.set_desktop_note(
            "便签.md",
            Some(crate::DesktopNote {
                x: Some(120),
                y: Some(80),
                w: Some(240),
                h: Some(200),
                color: Some("green".to_owned()),
                pinned: true,
            }),
        )
        .unwrap();
        let pinned = v.read("便签.md").unwrap();
        assert_eq!(pinned.meta.desktop_note.as_ref().unwrap().x, Some(120));
        assert_eq!(pinned.meta.note_type, crate::NoteType::QuickNote);
        assert!(
            pinned.content.contains("my_own_key: 保留我"),
            "第三方键必须原样保留"
        );
        assert!(pinned.content.contains("买牛奶"), "正文一个字都不该动");

        assert_eq!(
            v.desktop_notes().unwrap().len(),
            1,
            "启动恢复按这条清单；钉住的便签必须在里面"
        );

        // 取消钉住 = 把 `pinned` 放倒，外观留着。
        let mut off = pinned.meta.desktop_note.clone().unwrap();
        off.pinned = false;
        v.set_desktop_note("便签.md", Some(off)).unwrap();
        let unpinned = v.read("便签.md").unwrap();
        let kept = unpinned.meta.desktop_note.as_ref().unwrap();
        assert!(!kept.pinned);
        assert_eq!(kept.x, Some(120), "位置要留着——再钉回来还贴在原处");
        assert_eq!(
            kept.color.as_deref(),
            Some("green"),
            "颜色要留着（真机 2026-09-09：取消钉住再钉回来变回默认色）"
        );
        assert!(unpinned.content.contains("买牛奶"), "取消钉住不动笔记本身");
        assert!(
            v.desktop_notes().unwrap().is_empty(),
            "键还在，但它不该在下次启动时又自己冒出来"
        );
    }

    #[test]
    fn a_desktop_note_key_written_before_the_pinned_flag_existed_still_counts_as_pinned() {
        // 零迁移：升级前落下的键没有 `pinned` 字段，而它们当时的语义正是"钉着"。
        // 缺省判成 `false` 会让所有已钉住的便签在升级后集体消失。
        let (_dir, v) = open_vault();
        v.write(
            "旧便签.md",
            "---
luna:
  type: quick-note
  desktop_note:
    x: 10
    y: 20
---

旧的
",
        )
        .unwrap();
        let doc = v.read("旧便签.md").unwrap();
        assert!(doc.meta.desktop_note.as_ref().unwrap().pinned);
        assert_eq!(v.desktop_notes().unwrap().len(), 1);

        // 且回写之后仍然不写出这个字段——老文件逐字节不变。
        v.set_desktop_note("旧便签.md", doc.meta.desktop_note.clone())
            .unwrap();
        assert!(
            !v.read("旧便签.md").unwrap().content.contains("pinned"),
            "为 true 时跳过序列化，老文件不该因为一次读写长出新字段"
        );
    }

    #[test]
    fn desktop_note_write_is_skipped_when_nothing_changed() {
        // 拖动一张便签会连着发很多次坐标；每一次都写盘等于把用户的笔记历史塞满无意义的
        // 快照（`Vault::write` 每次都留快照）。
        let (_dir, v) = open_vault();
        v.write("便签.md", "---\nluna:\n  type: quick-note\n---\n\n买牛奶\n")
            .unwrap();
        let desktop = crate::DesktopNote {
            x: Some(10),
            y: Some(10),
            ..Default::default()
        };
        v.set_desktop_note("便签.md", Some(desktop.clone()))
            .unwrap();
        let before = v.read("便签.md").unwrap().content;
        v.set_desktop_note("便签.md", Some(desktop)).unwrap();
        assert_eq!(v.read("便签.md").unwrap().content, before);
    }

    #[test]
    fn draft_filters_narrow_the_list_and_the_matched_count_together() {
        // 清单与「筛出来有几条」必须是**同一个谓词**：各写一遍的表现是"头上说 3 条、
        // 列表里 5 行"。同时锁住 `total` **不随筛选变化**——活动栏角标承诺的是全库还剩
        // 几条（PRD-07-066），一筛选就跟着变的角标是在说谎。
        let (_dir, v) = open_vault();
        v.write(
            "只有文字.md",
            "---\nluna:\n  type: quick-note\n---\n\n改一下登录页的按钮\n",
        )
        .unwrap();
        v.write(
            "跑过提炼.md",
            "---\nluna:\n  type: quick-note\n  refined_at: 2026-09-09T10:00:00Z\n---\n\n周会要点\n",
        )
        .unwrap();

        let refined = crate::DraftInboxQuery {
            text: None,
            filter: crate::DraftFilter::Refined,
            now_ms: chrono::Local::now().timestamp_millis(),
        };
        let (entries, _, matched, total) = v.draft_inbox(&refined, 100, None).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["跑过提炼.md"]
        );
        assert_eq!(matched, 1, "筛出来的条数与清单同源");
        assert_eq!(total, 2, "全库总数不随筛选变化");

        // 就地查找是**子串**匹配，正文与标题都算数（全库检索归「搜索」空间）。
        let by_text = crate::DraftInboxQuery {
            text: Some("登录页"),
            filter: crate::DraftFilter::All,
            now_ms: refined.now_ms,
        };
        let (hits, _, matched, _) = v.draft_inbox(&by_text, 100, None).unwrap();
        assert_eq!(
            hits.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["只有文字.md"]
        );
        assert_eq!(matched, 1);

        // `LIKE` 的元字符要转义：搜一个下划线不该命中"任意单字符"。
        let underscore = crate::DraftInboxQuery {
            text: Some("_"),
            filter: crate::DraftFilter::All,
            now_ms: refined.now_ms,
        };
        let (none, _, matched, _) = v.draft_inbox(&underscore, 100, None).unwrap();
        assert!(none.is_empty(), "未转义的 `_` 会把所有行都捞回来");
        assert_eq!(matched, 0);
    }

    #[test]
    fn distill_scan_buckets_pending_fresh_edited_and_empty() {
        // PRD-06-044 批量入口：确认卡上的四个数字必须与单篇判定同源，且 `edited`
        // 单独成桶——"已最新"与"你写过东西我不敢动"是两个完全不同的跳过理由。
        let (_dir, v) = open_vault();
        let material = |ty: &str, body: &str| format!("---\nluna:\n  type: {ty}\n---\n\n{body}");

        v.write("从未整理.md", &material("recording", "转写正文"))
            .unwrap();
        v.write("空的.md", &material("chat", "")).unwrap();
        v.write("已最新.md", &material("transcript", "字幕"))
            .unwrap();
        v.write_distilled(
            "已最新.md",
            "Notes/已最新 · 纪要.md",
            "## 概要\n\n甲",
            "meeting",
            false,
            None,
        )
        .unwrap();
        v.write("被编辑.md", &material("recording", "转写"))
            .unwrap();
        v.write_distilled(
            "被编辑.md",
            "Notes/被编辑 · 纪要.md",
            "## 概要\n\n乙",
            "meeting",
            false,
            None,
        )
        .unwrap();
        let edited_doc = v.read("Notes/被编辑 · 纪要.md").unwrap();
        v.write(
            "Notes/被编辑 · 纪要.md",
            &format!("{}\n\n我补的一句。\n", edited_doc.content),
        )
        .unwrap();
        // 普通笔记与速记根本不是蒸馏对象，一个桶都不该进。
        v.write("普通.md", "普通正文").unwrap();
        v.write("速记.md", &material("quick-note", "随口一句"))
            .unwrap();

        let scan = v.distill_scan(None).unwrap();
        assert_eq!(scan.pending, vec!["从未整理.md".to_string()]);
        assert_eq!(scan.fresh, 1, "已最新的资料计入 fresh");
        assert_eq!(scan.edited, 1, "被编辑过的单独成桶（绝不覆盖）");
        assert_eq!(scan.empty, 1);
    }

    #[test]
    fn distill_scan_stale_but_unedited_goes_back_into_pending() {
        // 源被二次转写改写、而用户没动过纪要 → 应重新排进待整理（这正是批量的价值）。
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n初版")
            .unwrap();
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 概要\n\n初版纪要",
            "meeting",
            false,
            None,
        )
        .unwrap();
        assert!(v.distill_scan(None).unwrap().pending.is_empty());

        let meta = v.read("会议.md").unwrap().meta;
        let rewritten = crate::frontmatter::update_luna("权威重转写文本", &meta).unwrap();
        v.write("会议.md", &rewritten).unwrap();

        let scan = v.distill_scan(None).unwrap();
        assert_eq!(scan.pending, vec!["会议.md".to_string()]);
        assert_eq!(scan.fresh, 0);
    }

    /// **截断标注必须落在磁盘上**，不能只活在渲染层（D9′ ①）。
    ///
    /// 产物是写进 Vault 的一篇普通 Markdown：重开笔记、导出、同步到另一台设备之后，
    /// 只在 DOM 上标注的话它与一篇完整笔记**一模一样**。故判据只有一条——把文件按字节
    /// 读回来，那个事实还在不在。
    #[test]
    fn a_truncated_distillation_carries_its_mark_on_disk() {
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n原始转写")
            .unwrap();
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 决议\n\n- 只写到一半就",
            "meeting",
            false,
            Some(luna_core::TruncatedBy::Upstream.key()),
        )
        .unwrap();

        // ① 磁盘上的字节里有这个事实（不经任何内存态）。
        let on_disk = std::fs::read_to_string(v.root().join("Notes/会议 · 纪要.md")).unwrap();
        assert!(
            on_disk.contains("distill_output_truncated: upstream"),
            "截断归属没落进 frontmatter：{on_disk}"
        );

        // ② 重新读出来即「不完整」态，且**不是** stale / edited（三件事互不替代）。
        assert_eq!(
            v.distill_state("会议.md").unwrap(),
            DistillState::Present {
                path: "Notes/会议 · 纪要.md".into(),
                stale: false,
                edited: false,
                // 断言的是**归属**而不是一个布尔：三档各对应一个不同的用户动作。
                output_truncated: Some(luna_core::TruncatedBy::Upstream),
                template: Some("meeting".into()),
                full_text: Some(false),
            }
        );

        // ③ 补齐成功后标注必须**消失**——否则补了也像没补。
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 决议\n\n- 这一次说完了",
            "meeting",
            false,
            None,
        )
        .unwrap();
        let on_disk = std::fs::read_to_string(v.root().join("Notes/会议 · 纪要.md")).unwrap();
        assert!(
            !on_disk.contains("distill_output_truncated"),
            "补齐后旧标注仍留在文件里：{on_disk}"
        );
        assert!(matches!(
            v.distill_state("会议.md").unwrap(),
            DistillState::Present {
                output_truncated: None,
                ..
            }
        ));
    }

    /// **不完整产物落 `pending` 桶，绝不落 `fresh`**（D9′ ②）。
    ///
    /// 落进 `fresh` 的代价不是"少一个提示"，而是**永久排除**：界面写着「已是最新（跳过）」，
    /// 于是用户再也不会被提醒去补那一篇——现网这个缺陷的形状正是如此。
    #[test]
    fn an_incomplete_distillation_is_pending_not_fresh() {
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n原始转写")
            .unwrap();
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 概要\n\n只写到一半就",
            "meeting",
            false,
            Some(luna_core::TruncatedBy::Caller.key()),
        )
        .unwrap();

        // 源没变、用户没编辑——旧口径下它是标准的「已最新」。
        let scan = v.distill_scan(None).unwrap();
        assert_eq!(
            scan.pending,
            vec!["会议.md".to_string()],
            "不完整产物必须留在待办里"
        );
        assert_eq!(scan.fresh, 0, "它一旦进 fresh 桶就再也没人提醒了");
        assert_eq!(scan.edited, 0);

        // 对照组：同一篇补齐之后才回到 fresh。
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 概要\n\n这一次说完了",
            "meeting",
            false,
            None,
        )
        .unwrap();
        let scan = v.distill_scan(None).unwrap();
        assert!(scan.pending.is_empty());
        assert_eq!(scan.fresh, 1);
    }

    /// 用户编辑过的不完整产物仍归 `edited` 桶——**绝不覆盖用户资产**这条排在前面。
    #[test]
    fn an_edited_incomplete_distillation_still_counts_as_edited() {
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n原始转写")
            .unwrap();
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 概要\n\n只写到一半就",
            "meeting",
            false,
            Some(luna_core::TruncatedBy::Absolute.key()),
        )
        .unwrap();
        let current = v.read("Notes/会议 · 纪要.md").unwrap().content;
        v.write(
            "Notes/会议 · 纪要.md",
            &format!("{current}\n\n我自己补的结尾。\n"),
        )
        .unwrap();

        let scan = v.distill_scan(None).unwrap();
        assert!(scan.pending.is_empty(), "用户已接管的不该被排进批量重跑");
        assert_eq!(scan.edited, 1);
    }

    #[test]
    fn user_distill_templates_round_trip_and_live_in_the_vault() {
        // 模板**随库**：它是「这个库里的知识长什么样」的一部分，且蒸馏笔记的
        // frontmatter 存着模板键——存设备级的话，换库后那些键会指向不存在的模板。
        let (dir, v) = open_vault();
        assert!(
            v.distill_templates().unwrap().is_empty(),
            "新库无自定义模板"
        );

        let templates = vec![luna_core::UserTemplate {
            id: "abc".into(),
            name: "读书笔记".into(),
            sections: vec![luna_core::UserSection {
                heading: "金句摘抄".into(),
                guidance: "quotes worth keeping".into(),
            }],
            full_text_default: true,
        }];
        v.set_distill_templates(&templates).unwrap();
        assert_eq!(v.distill_templates().unwrap(), templates);

        // 落在库内 `.luna/` 而非设备级配置目录（换库即换模板集）。
        assert!(dir.path().join(".luna/distill_templates.json").is_file());

        // 整体替换（读改写一次，没有"改到一半"的中间态）。
        v.set_distill_templates(&[]).unwrap();
        assert!(v.distill_templates().unwrap().is_empty());
    }

    #[test]
    fn distill_records_template_and_full_text_as_a_pair() {
        // 用户显式选的模板与「附整理稿」必须落进 frontmatter：不落盘的话，
        // 「源已更新」重生成 / 自动档 / 批量整理都会按规则重新建议一个模板，把用户手选的
        // 那个静默换掉；只记模板不记整理稿开关，则整理稿会按模板默认值回落而静默消失。
        let (_dir, v) = open_vault();
        v.write("讲座.md", "---\nluna:\n  type: transcript\n---\n\n讲座转写")
            .unwrap();
        v.write_distilled(
            "讲座.md",
            "Notes/讲座 · 笔记.md",
            "## 核心观点\n\n甲",
            "interview",
            true,
            None,
        )
        .unwrap();

        // 两项都回读得到，且是**成对**的。
        let meta = v.read("Notes/讲座 · 笔记.md").unwrap().meta;
        assert_eq!(meta.distill_template.as_deref(), Some("interview"));
        assert_eq!(meta.distill_full_text, Some(true));

        // 状态查询一并下发（目标文件本就已读，零额外 IO）——重生成据此沿用。
        match v.distill_state("讲座.md").unwrap() {
            DistillState::Present {
                template,
                full_text,
                ..
            } => {
                assert_eq!(template.as_deref(), Some("interview"));
                assert_eq!(full_text, Some(true));
            }
            other => panic!("应为已生成态：{other:?}"),
        }

        // 刚写完不该被判成「用户编辑过」——新增两个 frontmatter 键不得干扰正文指纹口径。
        assert!(matches!(
            v.distill_state("讲座.md").unwrap(),
            DistillState::Present { edited: false, .. }
        ));
    }

    #[test]
    fn case_only_rename_succeeds_and_links_follow() {
        // PRD-06-006 ②：macOS 默认不区分大小写，`foo` → `Foo` 曾被判成「目标已存在」。
        let (_dir, v) = open_vault();
        v.write("foo.md", "正文").unwrap();
        v.write("引用.md", "见 [[foo]] 与 [链接](foo.md)\n")
            .unwrap();
        v.create_dir("notes").unwrap();
        v.write("notes/子.md", "子").unwrap();

        v.move_("foo.md", "Foo.md").unwrap();
        let names: Vec<String> = std::fs::read_dir(v.root())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n == "Foo.md"),
            "磁盘上的名字应是新的大小写：{names:?}"
        );
        assert!(!names
            .iter()
            .any(|n| n == "foo.md" || n.starts_with(".luna-rename-")));
        assert_eq!(
            std::fs::read_to_string(v.root().join("引用.md")).unwrap(),
            "见 [[Foo]] 与 [链接](Foo.md)\n"
        );

        v.move_("notes", "Notes").unwrap();
        assert!(v.root().join("Notes/子.md").is_file());
        let dirs: Vec<String> = std::fs::read_dir(v.root())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(dirs.iter().any(|n| n == "Notes"), "{dirs:?}");
    }

    #[test]
    fn rename_rewrites_only_links_that_point_at_the_moved_note() {
        // V-1 端到端：两篇同名 `foo`，改其中一篇，另一篇的引用者一个字节都不动；
        // 返回值列出被改写的笔记（IPC 据此通知打开着它们的编辑器，V-11）。
        let (_dir, v) = open_vault();
        v.create_dir("a").unwrap();
        v.create_dir("b").unwrap();
        v.write("a/foo.md", "甲").unwrap();
        v.write("b/foo.md", "乙").unwrap();
        v.write("a/引用.md", "见 [[foo]] 与 [链接](foo.md#节)\n")
            .unwrap();
        v.write("b/引用.md", "见 [[foo]]\n").unwrap();
        v.write("代码.md", "```\n[[a/foo]]\n```\n").unwrap();

        let rewritten = v.move_with_report("a/foo.md", "a/bar.md").unwrap();
        assert_eq!(rewritten.rewritten, vec!["a/引用.md".to_owned()]);
        assert_eq!(rewritten.links, 2, "[[foo]] 与 (foo.md#节) 两处");
        assert_eq!(
            std::fs::read_to_string(v.root().join("a/引用.md")).unwrap(),
            "见 [[bar]] 与 [链接](bar.md#节)\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("b/引用.md")).unwrap(),
            "见 [[foo]]\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("代码.md")).unwrap(),
            "```\n[[a/foo]]\n```\n"
        );
    }

    #[test]
    fn moving_a_note_keeps_its_attachments_and_outgoing_links_working() {
        // 独占附件随笔记搬、共享附件留原地；两种情况下本篇链接都还指得到，别人的相对链接也跟上。
        let (_dir, v) = open_vault();
        v.create_dir("a/assets").unwrap();
        std::fs::write(v.root().join("a/assets/独占.png"), b"1").unwrap();
        std::fs::write(v.root().join("a/assets/共享.png"), b"2").unwrap();
        v.write("a/邻居.md", "![](assets/共享.png)\n").unwrap();
        v.write(
            "a/x.md",
            "![](assets/独占.png) ![](assets/共享.png) [邻](邻居.md)\n",
        )
        .unwrap();
        v.write("外.md", "[x](a/x.md)\n").unwrap();

        v.move_("a/x.md", "b/x.md").unwrap();
        assert!(
            v.root().join("b/assets/独占.png").is_file(),
            "独占附件随笔记搬"
        );
        assert!(
            v.root().join("a/assets/共享.png").is_file(),
            "共享附件留原地"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("b/x.md")).unwrap(),
            "![](assets/独占.png) ![](../a/assets/共享.png) [邻](../a/邻居.md)\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("外.md")).unwrap(),
            "[x](b/x.md)\n"
        );
    }

    #[test]
    fn version_history_follows_a_rename() {
        // V-7：版本链按路径为键，改名后「版本历史」此前是空的。
        let (_dir, v) = open_vault();
        v.write("周报.md", "第一版").unwrap();
        v.write("周报.md", "第二版").unwrap();
        assert!(!v.snapshots("周报.md").is_empty());
        v.move_("周报.md", "归档/周报-0901.md").unwrap();
        assert!(v.snapshots("周报.md").is_empty());
        let history = v.snapshots("归档/周报-0901.md");
        assert!(!history.is_empty(), "改名后仍看得到改名前的版本");
        // 只改大小写同样迁（路径指纹区分大小写）。
        v.move_("归档/周报-0901.md", "归档/周报-0901.MD.md")
            .unwrap();
        assert_eq!(v.snapshots("归档/周报-0901.MD.md").len(), history.len());
    }

    #[test]
    fn candidates_found_through_links_stored_relative_to_their_source() {
        // `links.dst_path` 按源目录归一化：`乙/x` 里的 `[[甲/foo]]` 存成 `乙/甲/foo`，
        // 文件名嵌入存成「源目录/文件名」。候选篇按末段名字找，不能漏掉它们。
        let (_dir, v) = open_vault();
        v.create_dir("甲").unwrap();
        v.create_dir("乙").unwrap();
        v.create_dir("资料/assets").unwrap();
        v.write("甲/foo.md", "甲").unwrap();
        std::fs::write(v.root().join("资料/assets/图.png"), b"1").unwrap();
        v.write("乙/x.md", "[[甲/foo]] ![[图.png]]\n").unwrap();
        v.write("乙/无关.md", "[[别的]]\n").unwrap();

        let rewritten = v.move_with_report("甲/foo.md", "甲/bar.md").unwrap();
        assert_eq!(rewritten.rewritten, vec!["乙/x.md".to_owned()]);
        let rewritten = v
            .move_with_report("资料/assets/图.png", "资料/assets/封面.png")
            .unwrap();
        assert_eq!(rewritten.rewritten, vec!["乙/x.md".to_owned()]);
        assert_eq!(
            std::fs::read_to_string(v.root().join("乙/x.md")).unwrap(),
            "[[甲/bar]] ![[封面.png]]\n"
        );
    }

    // 「目录 0o555 即不可在其中写」是 Unix 权限语义；Windows 目录的只读属性不挡写入。
    #[cfg(unix)]
    #[test]
    fn a_rename_that_cannot_write_every_note_changes_nothing() {
        // 预检：被引用方所在目录只读 → 整次不做，文件不挪、引用不改。
        use std::os::unix::fs::PermissionsExt;
        let (_dir, v) = open_vault();
        v.create_dir("只读").unwrap();
        v.write("a.md", "甲").unwrap();
        v.write("只读/引用.md", "[[a]]\n").unwrap();
        let locked = v.root().join("只读");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = v.move_with_report("a.md", "b.md");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(result, Err(VaultError::NotWritable(ref list)) if list.contains("只读/引用.md"))
        );
        assert!(v.root().join("a.md").is_file() && !v.root().join("b.md").exists());
        assert_eq!(
            std::fs::read_to_string(v.root().join("只读/引用.md")).unwrap(),
            "[[a]]\n"
        );
    }

    #[test]
    fn a_write_failing_halfway_rolls_everything_back() {
        let (_dir, v) = open_vault();
        v.write("a.md", "甲").unwrap();
        v.write("一.md", "[[a]]\n").unwrap();
        v.write("二.md", "[[a]]\n").unwrap();
        FAIL_PLANNED_WRITE.with(|fail| *fail.borrow_mut() = Some("二.md".to_owned()));
        let result = v.move_with_report("a.md", "b.md");
        FAIL_PLANNED_WRITE.with(|fail| *fail.borrow_mut() = None);
        assert!(matches!(result, Err(VaultError::Io(_))), "{result:?}");
        assert!(v.root().join("a.md").is_file() && !v.root().join("b.md").exists());
        assert_eq!(
            std::fs::read_to_string(v.root().join("一.md")).unwrap(),
            "[[a]]\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("二.md")).unwrap(),
            "[[a]]\n"
        );
    }

    #[test]
    fn undo_restores_every_byte_while_nothing_changed_since() {
        let (_dir, v) = open_vault();
        v.create_dir("a/assets").unwrap();
        std::fs::write(v.root().join("a/assets/图.png"), b"1").unwrap();
        v.write("a/x.md", "![](assets/图.png) [[y]]\n").unwrap();
        v.write("y.md", "y").unwrap();
        v.write("引用.md", "见 [[x]]，又见 [x](a/x.md)\n").unwrap();
        let receipt = v.move_with_report("a/x.md", "b/新名.md").unwrap();
        assert_eq!(receipt.links, 2);
        assert!(
            v.root().join("b/assets/图.png").is_file(),
            "独占附件随笔记搬"
        );

        let restored = v.undo_relocation(&receipt.undo).unwrap();
        assert!(restored.contains(&"引用.md".to_owned()));
        assert!(v.root().join("a/x.md").is_file() && !v.root().join("b/新名.md").exists());
        assert!(v.root().join("a/assets/图.png").is_file(), "附件搬回原处");
        assert_eq!(
            std::fs::read_to_string(v.root().join("引用.md")).unwrap(),
            "见 [[x]]，又见 [x](a/x.md)\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("a/x.md")).unwrap(),
            "![](assets/图.png) [[y]]\n"
        );
    }

    #[test]
    fn undo_is_refused_once_anything_changed_afterwards() {
        let (_dir, v) = open_vault();
        v.write("a.md", "甲").unwrap();
        v.write("引用.md", "[[a]]\n").unwrap();
        let receipt = v.move_with_report("a.md", "b.md").unwrap();
        v.write("引用.md", "[[b]] 之后又写了一句\n").unwrap();
        assert!(matches!(
            v.undo_relocation(&receipt.undo),
            Err(VaultError::RelocationStale(ref list)) if list.contains("引用.md")
        ));
        assert!(
            v.root().join("b.md").is_file() && !v.root().join("a.md").exists(),
            "拒绝时一个字节都不动"
        );
    }

    #[test]
    fn undo_of_a_folder_move_puts_the_tree_back() {
        let (_dir, v) = open_vault();
        v.create_dir("项目").unwrap();
        v.write("项目/说明.md", "[根](../foo.md)\n").unwrap();
        v.write("foo.md", "根").unwrap();
        v.write("指向.md", "[说明](项目/说明.md)\n").unwrap();
        let receipt = v.move_with_report("项目", "归档/项目").unwrap();
        assert_eq!(
            std::fs::read_to_string(v.root().join("归档/项目/说明.md")).unwrap(),
            "[根](../../foo.md)\n"
        );
        v.undo_relocation(&receipt.undo).unwrap();
        assert_eq!(
            std::fs::read_to_string(v.root().join("项目/说明.md")).unwrap(),
            "[根](../foo.md)\n"
        );
        assert_eq!(
            std::fs::read_to_string(v.root().join("指向.md")).unwrap(),
            "[说明](项目/说明.md)\n"
        );
        assert!(!v.root().join("归档/项目").exists());
    }

    #[test]
    fn rename_onto_a_different_existing_name_is_still_refused() {
        let (_dir, v) = open_vault();
        v.write("甲.md", "1").unwrap();
        v.write("乙.md", "2").unwrap();
        assert!(matches!(
            v.move_("甲.md", "乙.md"),
            Err(VaultError::AlreadyExists(_))
        ));
        assert_eq!(
            std::fs::read_to_string(v.root().join("乙.md")).unwrap(),
            "2"
        );
    }

    #[test]
    fn distill_pointers_follow_moves_on_both_sides() {
        // PRD-06-044：移动不该破坏关系（与 wikilink rename 重构同一心智）。
        let (_dir, v) = open_vault();
        v.write("会议.md", "---\nluna:\n  type: recording\n---\n\n转写")
            .unwrap();
        v.write_distilled(
            "会议.md",
            "Notes/会议 · 纪要.md",
            "## 概要\n\n甲",
            "meeting",
            false,
            None,
        )
        .unwrap();

        // ① 移动**资料**：蒸馏笔记的 distill_source 要跟着换。
        v.move_("会议.md", "归档/会议.md").unwrap();
        assert_eq!(
            v.read("Notes/会议 · 纪要.md")
                .unwrap()
                .meta
                .distill_source
                .as_deref(),
            Some("归档/会议.md")
        );
        assert!(
            matches!(
                v.distill_state("归档/会议.md").unwrap(),
                DistillState::Present { edited: false, .. }
            ),
            "移动后状态仍是「已生成且未编辑」，不该回落未生成"
        );

        // ② 移动**蒸馏笔记**：资料侧的 distill_note 要跟着换。
        v.move_("Notes/会议 · 纪要.md", "Notes/知识/会议纪要.md")
            .unwrap();
        assert_eq!(
            v.read("归档/会议.md").unwrap().meta.distill_note.as_deref(),
            Some("Notes/知识/会议纪要.md")
        );
        let state = v.distill_state("归档/会议.md").unwrap();
        assert!(
            matches!(&state, DistillState::Present { path, edited: false, stale: false, .. } if path == "Notes/知识/会议纪要.md"),
            "两侧都移动过之后关系仍完整：{state:?}"
        );
    }

    #[test]
    fn distill_pointers_survive_moving_the_whole_folder() {
        // 目录移动：源与蒸馏笔记**同在**被移动的子树内——两侧路径都变了，
        // 出向与入向指针都必须重映射，否则一次拖拽就把关系全断了。
        let (_dir, v) = open_vault();
        v.write("工作/会议.md", "---\nluna:\n  type: recording\n---\n\n转写")
            .unwrap();
        v.write_distilled(
            "工作/会议.md",
            "工作/会议 · 纪要.md",
            "## 概要\n\n甲",
            "meeting",
            false,
            None,
        )
        .unwrap();

        v.move_("工作", "已归档/2026").unwrap();

        let source = v.read("已归档/2026/会议.md").unwrap();
        let distilled = v.read("已归档/2026/会议 · 纪要.md").unwrap();
        assert_eq!(
            source.meta.distill_note.as_deref(),
            Some("已归档/2026/会议 · 纪要.md")
        );
        assert_eq!(
            distilled.meta.distill_source.as_deref(),
            Some("已归档/2026/会议.md")
        );
        assert!(matches!(
            v.distill_state("已归档/2026/会议.md").unwrap(),
            DistillState::Present {
                stale: false,
                edited: false,
                ..
            }
        ));
    }

    #[test]
    fn distill_pointer_dangling_falls_back_to_absent_and_never_resurrects() {
        // 用户删掉蒸馏笔记 = 显式意图：状态回「未生成」，绝不复活旧文件。
        let (_dir, v) = open_vault();
        v.write(
            "对话.md",
            "---\nluna:\n  type: chat\n---\n\n## User\n\n问题",
        )
        .unwrap();
        v.write_distilled(
            "对话.md",
            "Notes/对话 · 笔记.md",
            "## 结论\n\n用 A 方案",
            "meeting",
            false,
            None,
        )
        .unwrap();
        v.delete("Notes/对话 · 笔记.md", DeleteMode::Permanent)
            .unwrap();
        assert_eq!(v.distill_state("对话.md").unwrap(), DistillState::Absent);
        // 源笔记里的指针仍在（不主动清理），但判定以**实体是否存在**为准。
        assert_eq!(
            v.read("对话.md").unwrap().meta.distill_note.as_deref(),
            Some("Notes/对话 · 笔记.md")
        );
    }

    #[test]
    fn distilled_note_is_plain_note_with_both_pointers() {
        // 蒸馏产物恒为 type: note——它生来就是知识，全量参与图谱/标签/搜索（ADR-024）。
        let (_dir, v) = open_vault();
        v.write("视频.md", "---\nluna:\n  type: transcript\n---\n\n字幕全文")
            .unwrap();
        v.write_distilled(
            "视频.md",
            "Notes/视频 · 纪要.md",
            "## 概要\n\n讲了三件事",
            "meeting",
            false,
            None,
        )
        .unwrap();

        let distilled = v.read("Notes/视频 · 纪要.md").unwrap();
        assert_eq!(distilled.meta.note_type, crate::frontmatter::NoteType::Note);
        assert_eq!(distilled.meta.distill_source.as_deref(), Some("视频.md"));
        assert!(distilled.content.contains("## 概要"));
        // 源侧反向指针
        assert_eq!(
            v.read("视频.md").unwrap().meta.distill_note.as_deref(),
            Some("Notes/视频 · 纪要.md")
        );
        // 源笔记类型不被蒸馏动作改动
        assert_eq!(
            v.read("视频.md").unwrap().meta.note_type,
            crate::frontmatter::NoteType::Transcript
        );
    }

    /// 真机 2026-09-27：用户编辑时删掉了闭合 `---` 后的换行（`---# 周会`），后台 AI 标签
    /// 写回照「无 frontmatter」在前面又新建一份，叠出两段、把用户的 tags 挤进正文。
    /// frontmatter 没闭合时一律拒写，文件一个字节不动；用户修好后照常写。
    #[test]
    fn ai_tags_never_stack_a_second_frontmatter_on_an_unclosed_one() {
        let (_dir, v) = open_vault();
        let broken = "---\ntags: [项目/A]\n---# 周会\n\n## 项目 A\n\n发布推迟到下月。\n";
        v.write("周会.md", broken).unwrap();
        let tags = vec!["项目/A".to_string()];
        assert!(v.set_ai_tags("周会.md", &tags, "h").is_err());
        assert_eq!(v.read("周会.md").unwrap().content, broken, "一个字节不动");

        let fixed = broken.replacen("---# 周会", "---\n# 周会", 1);
        v.write("周会.md", &fixed).unwrap();
        let written = v
            .set_ai_tags("周会.md", &tags, "h")
            .unwrap()
            .expect("修好后照常写");
        assert!(
            written.starts_with("---\ntags: [项目/A]\nluna:") && !written.contains("---\n---"),
            "luna 块并入既有的那一段 frontmatter，不另起一段：{written}"
        );
    }

    #[test]
    fn set_ai_tags_round_trip_and_graph_reflects_them() {
        // PRD-06-034/035：AI 标签写入 luna 块（用户 tags/正文不动）→ 索引合并 → 图谱
        // 经标签节点连通两篇笔记；重复写同内容不产生新快照写盘（返回 None）。
        let (_dir, v) = open_vault();
        v.write("a.md", "---\ntags: [用户]\n---\n正文 A 关于产品评审")
            .unwrap();
        v.write("b.md", "正文 B 也关于产品评审").unwrap();
        let tags = vec!["产品/评审".to_string()];
        let hash = luna_sync_core::hashing::ContentHash::of("正文".as_bytes());
        let written = v.set_ai_tags("a.md", &tags, hash.as_str()).unwrap();
        let content = written.expect("首次写入应发生");
        assert!(content.contains("tags: [用户]"), "用户 tags 逐字保留");
        assert!(content.contains("ai_tags:"), "AI 标签入 luna 块");
        // 幂等：同标签同指纹再写 → 不写盘。
        assert!(v
            .set_ai_tags("a.md", &tags, hash.as_str())
            .unwrap()
            .is_none());
        v.set_ai_tags("b.md", &tags, hash.as_str()).unwrap();
        let g = v.graph().unwrap();
        let tag_node = g
            .nodes
            .iter()
            .find(|n| n.id == "tag:产品/评审")
            .expect("AI 标签成为图谱节点");
        assert_eq!(tag_node.degree, 2, "两篇笔记经标签节点相连");
        // 索引标签面板同源受益。
        let counts = v.list_tags().unwrap();
        assert!(counts.iter().any(|t| t.tag == "产品/评审" && t.count == 2));
    }

    #[test]
    fn graph_tag_scan_classifies_stale_fresh_and_empty() {
        // PRD-06-035b 重扫入口：盘点口径必须与 set_ai_tags 写入的指纹同源——
        // 指纹一致 = fresh、不一致或从未生成 = stale、正文为空 = empty（不参与）。
        let (_dir, v) = open_vault();
        v.write("fresh.md", "正文未再改动").unwrap();
        let hash = luna_sync_core::hashing::ContentHash::of(body_of("正文未再改动").as_bytes());
        v.set_ai_tags("fresh.md", &["主题".to_string()], hash.as_str())
            .unwrap();
        v.write("never.md", "从未生成过标签").unwrap();
        v.write("outdated.md", "旧正文").unwrap();
        v.set_ai_tags("outdated.md", &["旧".to_string()], "stale-hash")
            .unwrap();
        v.write("blank.md", "---\ntags: [x]\n---\n\n").unwrap();

        let scan = v.graph_tag_scan(None).unwrap();
        assert_eq!(scan.fresh, vec!["fresh.md".to_string()]);
        let mut stale = scan.stale.clone();
        stale.sort();
        assert_eq!(
            stale,
            vec!["never.md".to_string(), "outdated.md".to_string()]
        );
        assert_eq!(scan.empty, 1, "仅 frontmatter 无正文的笔记不参与重扫");
        assert_eq!(scan.excluded_type, 0);
        assert_eq!(scan.excluded_folder, 0);
    }

    #[test]
    fn revisit_candidate_only_surfaces_notes_that_are_genuinely_old() {
        // 库里最久没动的那一篇，在刚建的库里可能是昨天写的——端出来说「值得回看」
        // 是无中生有。现状块的纪律是没东西说就安静，不是凑一条。
        let (_dir, v) = open_vault();
        v.write("old.md", "很久以前写的东西").unwrap();

        // 门槛推到过去（比任何一篇都早）→ 一篇都不够旧。
        assert!(v.revisit_candidate(0).unwrap().is_none());
        // 门槛推到未来 → 现有的那篇够旧了。
        let hit = v.revisit_candidate(i64::MAX).unwrap();
        assert_eq!(hit.map(|(path, _)| path), Some("old.md".to_string()));
    }

    #[test]
    fn revisit_candidate_skips_empty_pages_and_materials() {
        // 空页没内容可回看；资料区不是「你当初的思考」。
        let (_dir, v) = open_vault();
        v.write("blank.md", "---\ntags: []\n---\n\n").unwrap();
        v.write(
            "Recordings/r.md",
            "---\nluna:\n  type: recording\n---\n\n转写正文",
        )
        .unwrap();
        assert!(v.revisit_candidate(i64::MAX).unwrap().is_none());
    }

    #[test]
    fn unlinked_count_matches_the_checkup_orphan_judgment() {
        // PRD-07-024a ① 要求这个数与图谱/体检**同源同数**。本用例直接把两边摆在一起
        // 比：概览的计数必须恰好等于体检报出的孤儿页篇数——不是"差不多"。
        let (_dir, v) = open_vault();
        v.write("lonely.md", "有正文但没有任何链接与标签").unwrap();
        v.write("tagged.md", "---\ntags: [x]\n---\n\n有标签就有边")
            .unwrap();
        v.write("blank.md", "---\ntags: []\n---\n\n").unwrap();
        v.write(
            "Recordings/r.md",
            "---\nluna:\n  type: recording\n---\n\n资料区不参与这一维",
        )
        .unwrap();

        let count = v.unlinked_knowledge_notes().unwrap();
        let orphans = v
            .checkup_t0(true, 0)
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == crate::checkup::FindingKind::Orphan)
            .count();
        assert_eq!(count as usize, orphans, "两处必须逐个对上，不是差不多");
        assert_eq!(count, 1, "只有 lonely.md 算孤儿");
    }

    #[test]
    fn unlinked_count_excludes_empty_pages_and_materials() {
        // 空页有自己的那一档（用户的下一步是补内容，不是建链接）；资料区本就不进知识网络。
        let (_dir, v) = open_vault();
        v.write("blank.md", "---\ntags: []\n---\n\n").unwrap();
        v.write(
            "Recordings/r.md",
            "---\nluna:\n  type: recording\n---\n\n转写正文",
        )
        .unwrap();
        assert_eq!(v.unlinked_knowledge_notes().unwrap(), 0);
    }

    #[test]
    fn note_counts_by_type_returns_raw_groups_not_a_knowledge_vs_material_split() {
        // 索引层刻意只回原始分组：「知识 vs 资料」那条线的判据是
        // `NoteType::is_source_material`，在 SQL 里再写一次 `type IN (…)` 就是把同一条线
        // 画到第二个地方，而它必然漏掉将来新增的类型。
        let (_dir, v) = open_vault();
        v.write("a.md", "普通笔记一").unwrap();
        v.write("b.md", "普通笔记二").unwrap();
        v.write(
            "Recordings/r.md",
            "---\nluna:\n  type: recording\n---\n\n转写正文",
        )
        .unwrap();

        let counts: std::collections::BTreeMap<String, u32> =
            v.note_counts_by_type().unwrap().into_iter().collect();
        assert_eq!(counts.get("note").copied(), Some(2));
        assert_eq!(counts.get("recording").copied(), Some(1));
        assert!(!counts.contains_key("knowledge"), "不得在 SQL 里合成分组");
    }

    #[test]
    fn active_tags_only_counts_recently_touched_knowledge_notes() {
        let (_dir, v) = open_vault();
        v.write("recent.md", "最近改过").unwrap();
        v.set_ai_tags("recent.md", &["架构".to_string()], "h1")
            .unwrap();
        v.write(
            "Recordings/r.md",
            "---\nluna:\n  type: recording\n  ai_tags: [架构]\n---\n\n转写正文",
        )
        .unwrap();

        // 时间下界取 0 = 全都算「近期」，此时只应看到知识笔记那一篇。
        let tags = v.active_tags(0, 10).unwrap();
        assert_eq!(tags, vec![("架构".to_string(), 1)], "资料区笔记不参与统计");

        // 下界推到未来：一篇都不在窗口内。
        let future = i64::MAX;
        assert!(v.active_tags(future, 10).unwrap().is_empty());
    }

    #[test]
    fn graph_tag_language_scan_judges_per_tag_with_proper_nouns_neutral() {
        // PRD-06-035d ②/⑦：判据逐条标签走 `tag_fits_language`，与注入词表的过滤同一份。
        let (_dir, v) = open_vault();
        v.write("zh.md", "中文笔记").unwrap();
        v.set_ai_tags("zh.md", &["架构".to_string()], "h1").unwrap();
        v.write("en.md", "english note").unwrap();
        v.set_ai_tags("en.md", &["architecture".to_string()], "h2")
            .unwrap();
        // 中文模式下技术缩写**故意保留原样**（提示词就这么要求的），故这一篇必须
        // 算中文——专名形状（大写/数字）语言中性，两种库都站得住。
        v.write("mixed.md", "带缩写的中文笔记").unwrap();
        v.set_ai_tags(
            "mixed.md",
            &["架构".to_string(), "AWS".to_string(), "Datadog".to_string()],
            "h3",
        )
        .unwrap();
        // 一篇之内就分裂的组合（`#运维 #ops`）必须算不一致——旧的笔记级判据
        // （"有一个含 CJK 就算中文"）恰好漏掉这种分裂最常见的形态。
        v.write("split.md", "分裂笔记").unwrap();
        v.set_ai_tags("split.md", &["运维".to_string(), "ops".to_string()], "h4")
            .unwrap();
        // 全是专名的笔记两头都一致：重扫不会改它们，数成"待收敛"就是消不掉的数字。
        v.write("product.md", "产品对照").unwrap();
        v.set_ai_tags(
            "product.md",
            &["Datadog".to_string(), "I2X".to_string()],
            "h5",
        )
        .unwrap();
        // 还没有标签的笔记归「缺标签」那张卡，不该被说成语言不一致。
        v.write("untagged.md", "还没打过标签").unwrap();

        let zh = v.graph_tag_language_scan(true).unwrap();
        assert_eq!(zh.mismatched, 2, "纯英文那篇 + 一篇之内分裂那篇");
        assert_eq!(zh.matched, 3, "含缩写的中文笔记与纯专名笔记都算一致");
        assert_eq!(
            zh.sample_tags,
            vec!["architecture".to_string(), "ops".to_string()],
            "列出的是站不住的那几条标签本身，专名不在其中"
        );

        let en = v.graph_tag_language_scan(false).unwrap();
        assert_eq!(en.mismatched, 3, "英文界面下含 CJK 标签的三篇都算不一致");
        assert_eq!(en.matched, 2, "纯英文那篇 + 纯专名那篇");
    }

    #[test]
    fn tag_fits_language_is_segment_wise_and_neutral_on_proper_nouns() {
        // 中文库：中文段 ✓、专名形状（大写/数字）✓、全小写普通词 ✗、层级逐段判。
        for tag in ["架构", "架构/三域", "Datadog", "I2X", "MIO-One", "AWS"] {
            assert!(tag_fits_language(tag, true), "{tag} 在中文库该站得住");
        }
        for tag in [
            "ops",
            "architecture",
            "ops/计划",
            "data-integration",
            "datadog",
        ] {
            assert!(!tag_fits_language(tag, true), "{tag} 在中文库站不住");
        }
        // 英文库：CJK 不携带大小写、无从按形状识别专名，含 CJK 的段一律算错语言。
        for tag in ["ops", "Datadog", "I2X", "architecture/three-tier"] {
            assert!(tag_fits_language(tag, false), "{tag} 在英文库该站得住");
        }
        for tag in ["架构", "ops/计划", "架构/三域"] {
            assert!(!tag_fits_language(tag, false), "{tag} 在英文库站不住");
        }
    }

    /// 受影响标签按出现篇数降序列出、同数按字典序、至多上限条（C16）。
    #[test]
    fn most_frequent_tags_are_ranked_and_capped() {
        let counts: std::collections::BTreeMap<String, usize> = [
            ("ops", 3),
            ("architecture", 3),
            ("design", 1),
            ("a", 1),
            ("b", 1),
            ("c", 1),
            ("d", 1),
        ]
        .into_iter()
        .map(|(tag, n)| (tag.to_owned(), n))
        .collect();
        assert_eq!(
            most_frequent(counts, 4),
            vec!["architecture", "ops", "a", "b"]
        );
    }

    #[test]
    fn graph_tag_language_scan_ignores_what_a_rescan_would_not_touch() {
        // 判定面必须与「全部重新生成」对齐：类型白名单外的笔记不会被重扫覆盖，
        // 把它们算进「受影响篇数」就是给用户一个按下去也消不掉的数字。
        let (_dir, v) = open_vault();
        v.write(
            "Recordings/rec.md",
            "---\nluna:\n  type: recording\n  ai_tags: [architecture]\n---\n\n转写正文",
        )
        .unwrap();
        // 知识地图同理（PRD-16-023 修订）：即便误打上过标签，重扫也不会碰它。
        v.write(
            "知识地图.md",
            "---\nluna:\n  type: note\n  knowledge_map: true\n  ai_tags: [architecture]\n---\n\n路由",
        )
        .unwrap();
        v.write("note.md", "普通笔记").unwrap();
        v.set_ai_tags("note.md", &["架构".to_string()], "h1")
            .unwrap();

        let scan = v.graph_tag_language_scan(true).unwrap();
        assert_eq!(
            scan.mismatched, 0,
            "资料区笔记与知识地图不参与自动打标，也就不参与本盘点"
        );
        assert_eq!(scan.matched, 1);
    }

    /// 生成侧「地图不打标」（`origin == "knowledge_map"` 跳过 Tag 步）与重扫必须
    /// 同一个结论——否则就是「生成不打、重扫补上」的自相矛盾（真机第五轮：地图被
    /// 打上 `#cutover-迁移` 混进标签页的「共 6 篇」）。写盘同时清存量，理由见
    /// `write_knowledge_map` 注释。
    #[test]
    fn knowledge_map_is_exempt_from_tag_rescan_and_write_clears_stale_tags() {
        let (_dir, v) = open_vault();
        v.write(
            "知识地图.md",
            "---\nluna:\n  type: note\n  knowledge_map: true\n  ai_tags: [cutover-迁移]\n---\n\n路由",
        )
        .unwrap();
        v.write("普通.md", "一篇普通笔记").unwrap();

        let scan = v.graph_tag_scan(None).unwrap();
        assert!(
            !scan.stale.contains(&"知识地图.md".to_string())
                && !scan.fresh.contains(&"知识地图.md".to_string()),
            "地图不得进入任何一档重扫清单"
        );
        assert_eq!(scan.excluded_type, 1, "地图计入「设计豁免」那一档");

        // 下一次地图维护写回即清掉误打的存量标签。
        v.write_knowledge_map("知识地图.md", "- 空目录\n").unwrap();
        let doc = v.read("知识地图.md").unwrap();
        assert!(doc.meta.ai_tags.is_empty(), "写回后存量 ai_tags 应被清空");
        assert!(doc.meta.ai_tags_src.is_none());
    }

    #[test]
    fn graph_tag_scan_excludes_non_note_types() {
        // ADR-024：AI 标签自动生成白名单 = {note}——资料区（录音/转写/对话）与速记都
        // 不该被批量打标，且必须计入 excluded（被挡在外面的篇数要可见，不能凭空消失）。
        let (_dir, v) = open_vault();
        v.write("普通.md", "一篇普通笔记的正文").unwrap();
        for (path, ty) in [
            ("会议.md", "recording"),
            ("视频.md", "transcript"),
            ("对话.md", "chat"),
            ("速记.md", "quick-note"),
        ] {
            v.write(path, &format!("---\nluna:\n  type: {ty}\n---\n\n有正文"))
                .unwrap();
        }

        let scan = v.graph_tag_scan(None).unwrap();
        assert_eq!(
            scan.stale,
            vec!["普通.md".to_string()],
            "只有普通笔记进候选"
        );
        assert_eq!(
            scan.excluded_type, 4,
            "四类非 note 笔记计入**类型**那一档（这是设计，用户无须处置）"
        );
        assert_eq!(scan.excluded_folder, 0, "没停用任何文件夹");
        assert_eq!(scan.empty, 0);
    }

    #[test]
    fn graph_tag_scan_honors_scope_and_exclusions() {
        // PRD-06-035c：scope 只圈定文件夹（含子孙）；排除文件夹内的笔记计入
        // `excluded` 而非静默消失——被规则挡在外面的篇数必须可见。
        let (_dir, v) = open_vault();
        v.write("Chats/对话.md", "AI 对话记录").unwrap();
        v.write("Projects/方案.md", "项目方案").unwrap();
        v.write("Projects/Sub/细节.md", "细节").unwrap();
        v.write("随手记.md", "散记").unwrap();
        v.set_graph_tag_exclusion("Chats", true).unwrap();

        // 全库：Chats 那篇进 excluded，其余进 stale。
        let scan = v.graph_tag_scan(None).unwrap();
        // 两个数分开（D7）：这一篇是**用户停用了文件夹**，不是类型白名单挡的——
        // 合成一个数字时，用户无从知道"我某次误点停用了整个文件夹"。
        assert_eq!(scan.excluded_folder, 1);
        assert_eq!(scan.excluded_type, 0);
        assert_eq!(scan.stale.len(), 3);
        assert!(!scan.stale.iter().any(|p| p.starts_with("Chats/")));

        // 定向 Projects：含子孙、不含库内其它笔记。
        let scoped = v.graph_tag_scan(Some("Projects")).unwrap();
        let mut stale = scoped.stale.clone();
        stale.sort();
        assert_eq!(
            stale,
            vec![
                "Projects/Sub/细节.md".to_string(),
                "Projects/方案.md".to_string()
            ]
        );
        assert_eq!(scoped.excluded_folder, 0);

        // 解除排除后 Chats 回到候选。
        v.set_graph_tag_exclusion("Chats", false).unwrap();
        let after = v.graph_tag_scan(None).unwrap();
        assert_eq!(after.excluded_folder, 0);
        assert_eq!(after.stale.len(), 4);
    }

    #[test]
    fn graph_tag_exclusion_round_trips_and_rejects_invalid_scope() {
        let (_dir, v) = open_vault();
        // 归一化：首尾斜杠剥离；重复添加幂等；返回值即最新清单。
        let list = v.set_graph_tag_exclusion("/Chats/", true).unwrap();
        assert_eq!(list, vec!["Chats".to_string()]);
        assert_eq!(
            v.set_graph_tag_exclusion("Chats", true).unwrap(),
            vec!["Chats".to_string()]
        );
        assert_eq!(v.graph_tag_exclusions().unwrap(), vec!["Chats".to_string()]);
        assert!(v
            .set_graph_tag_exclusion("Chats", false)
            .unwrap()
            .is_empty());
        // 空（= Vault 根）与越界一律拒绝：根是总开关的语义，不是一条范围规则。
        assert!(v.set_graph_tag_exclusion("", true).is_err());
        assert!(v.set_graph_tag_exclusion("/", true).is_err());
        assert!(v.set_graph_tag_exclusion("../evil", true).is_err());
        assert!(v.graph_tag_scan(Some("../evil")).is_err());
    }

    #[test]
    fn open_creates_luna_and_gitignore() {
        let (dir, _v) = open_vault();
        assert!(dir.path().join(".luna").is_dir());
        let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains(".luna/index.sqlite"));
        assert!(gi.contains(".luna/snapshots/"));
        // settings.toml 不被忽略。
        assert!(!gi.contains(".luna/settings.toml\n"));
    }

    #[test]
    fn gitignore_idempotent_append_preserves_user_lines() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        std::fs::write(&gi, "node_modules/\n*.log\n").unwrap();
        let _v1 = Vault::open(dir.path()).unwrap();
        let after1 = std::fs::read_to_string(&gi).unwrap();
        assert!(after1.starts_with("node_modules/\n*.log\n"), "用户行须保留");
        assert!(after1.contains("# >>> Lunarca >>>"));
        // 再次打开不重复追加。
        drop(_v1);
        let _v2 = Vault::open(dir.path()).unwrap();
        let after2 = std::fs::read_to_string(&gi).unwrap();
        assert_eq!(after1, after2, "重复打开不应重复追加 Lunarca 块");
    }

    /// 老库的托管块缺新规则（如 `.luna/vault-id`）时只换块内，块外用户的每一行原样保留。
    #[test]
    fn gitignore_refreshes_an_outdated_managed_block_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        let before = "node_modules/\n\n# >>> Lunarca >>> (managed, do not edit between markers)\n.luna/index.sqlite\n# <<< Lunarca <<<\n*.log\n";
        std::fs::write(&gi, before).unwrap();
        drop(Vault::open(dir.path()).unwrap());
        let after = std::fs::read_to_string(&gi).unwrap();
        assert!(after.starts_with("node_modules/\n\n# >>> Lunarca >>>"));
        assert!(
            after.ends_with("# <<< Lunarca <<<\n*.log\n"),
            "块后的用户行原样保留"
        );
        assert!(after.contains(".luna/vault-id\n"));
        assert_eq!(after.matches("# >>> Lunarca >>>").count(), 1);
    }

    /// 库身份（ADR-140 ②）：重开不变；让出后写回磁盘；旧布局文件开库即删（ADR-140 ③）。
    #[test]
    fn vault_id_is_stable_reassignable_and_the_legacy_layout_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        std::fs::write(dir.path().join(".luna/workspace.json"), "{}").unwrap();
        let first = Vault::open(dir.path()).unwrap().id();
        assert!(!dir.path().join(".luna/workspace.json").exists());
        let mut vault = Vault::open(dir.path()).unwrap();
        assert_eq!(vault.id(), first);
        vault.reassign_id().unwrap();
        let second = vault.id();
        assert_ne!(second, first);
        drop(vault);
        assert_eq!(Vault::open(dir.path()).unwrap().id(), second);
    }

    #[test]
    fn write_read_round_trip_and_index() {
        let (_d, v) = open_vault();
        v.write("日常/会议.md", "# 会议\n讨论产品路线图").unwrap();
        let doc = v.read("日常/会议.md").unwrap();
        assert!(doc.content.contains("产品路线图"));
        assert_eq!(doc.meta.note_type, NoteType::Note);
        // 已进索引：搜得到。
        let hits = v.search(&Query::text("产品路线图")).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn overwrite_takes_snapshot_first() {
        let (_d, v) = open_vault();
        v.write("a.md", "第一版").unwrap();
        v.write("a.md", "第二版").unwrap();
        let snaps = v.snapshots("a.md");
        assert_eq!(snaps.len(), 1, "覆盖前应留旧版本快照");
        // 还原到历史版本。
        v.restore_snapshot("a.md", &snaps[0].id).unwrap();
        assert_eq!(v.read("a.md").unwrap().content, "第一版");
    }

    #[test]
    fn self_write_hash_is_recorded() {
        let (_d, v) = open_vault();
        v.write("a.md", "内容X").unwrap();
        let h = v.last_written_hash("a.md").unwrap();
        assert_eq!(h, ContentHash::of("内容X".as_bytes()));
    }

    #[test]
    fn create_from_template_and_no_overwrite() {
        let (_d, v) = open_vault();
        let doc = v
            .create(
                "录音/会议.md",
                NoteType::Recording,
                &TemplateVars {
                    body: "# 会议\n正文".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(doc.meta.note_type, NoteType::Recording);
        // 重复 create 报已存在。
        assert!(v
            .create(
                "录音/会议.md",
                NoteType::Recording,
                &TemplateVars::default()
            )
            .is_err());
    }

    #[test]
    fn user_templates_are_nested_sorted_and_cannot_escape_their_root() {
        let (dir, v) = open_vault();
        let root = dir.path().join(".luna/templates");
        std::fs::create_dir_all(root.join("会议")).unwrap();
        std::fs::write(root.join("每日.md"), "# {{date}}").unwrap();
        std::fs::write(root.join("会议/周会.md"), "# {{title}}").unwrap();
        std::fs::write(root.join("忽略.txt"), "no").unwrap();

        let templates = v.list_user_templates(".luna/templates").unwrap();
        assert_eq!(
            templates
                .iter()
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>(),
            vec!["会议/周会.md", "每日.md"]
        );
        assert_eq!(
            v.read_user_template(".luna/templates", "会议/周会.md")
                .unwrap(),
            "# {{title}}"
        );
        assert!(v
            .read_user_template(".luna/templates", "../outside.md")
            .is_err());
    }

    /// 「复制到库」：目录不存在先建，同名不覆盖、往后加序号，名字里不许带路径。
    #[test]
    fn adding_a_user_template_never_overwrites_and_never_leaves_the_template_root() {
        let (dir, v) = open_vault();
        assert_eq!(
            v.add_user_template("Templates", "周报", "# 一").unwrap(),
            "周报.md"
        );
        assert_eq!(
            v.add_user_template("Templates", "周报", "# 二").unwrap(),
            "周报-1.md"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Templates/周报.md")).unwrap(),
            "# 一"
        );
        for bad in ["", "a/b", "..", ".hidden", "a\\b"] {
            assert!(v.add_user_template("Templates", bad, "x").is_err(), "{bad}");
        }
        assert!(v.add_user_template("../outside", "周报", "x").is_err());
    }

    #[test]
    fn attach_media_roundtrips_luna() {
        let (_d, v) = open_vault();
        v.write("a.md", "---\ntitle: 我的\ntags: [x]\n---\n正文")
            .unwrap();
        v.attach_media(
            "a.md",
            MediaRef {
                path: "Recordings/a.wav".into(),
                transcript: true,
                ..Default::default()
            },
        )
        .unwrap();
        let doc = v.read("a.md").unwrap();
        assert!(doc.content.contains("title: 我的"), "用户键须保留");
        assert!(doc.content.contains("tags: [x]"));
        assert_eq!(doc.meta.media.len(), 1);
    }

    #[test]
    fn vault_trash_delete_restore_purge() {
        let (_d, v) = open_vault();
        v.write("a.md", "内容").unwrap();
        v.delete("a.md", DeleteMode::VaultTrash).unwrap();
        assert!(v.read("a.md").is_err(), "删除后原路径不存在");
        let entries = v.list_trash();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original_path, "a.md");
        // 还原。
        v.restore(&entries[0].id, RestoreConflict::Fail).unwrap();
        assert_eq!(v.read("a.md").unwrap().content, "内容");
        // 再删 + purge。
        v.delete("a.md", DeleteMode::VaultTrash).unwrap();
        v.purge(None).unwrap();
        assert!(v.list_trash().is_empty());
    }

    #[test]
    fn trash_preview_note_folder_and_binary() {
        let (dir, v) = open_vault();

        // 文本笔记 → Note，正文可读。
        v.write("note.md", "# 标题\n正文内容").unwrap();
        v.delete("note.md", DeleteMode::VaultTrash).unwrap();
        let note_id = &v.list_trash()[0].id.clone();
        let note = v.trash_preview(note_id).unwrap();
        assert_eq!(note.kind, TrashPreviewKind::Note);
        assert_eq!(note.content.as_deref(), Some("# 标题\n正文内容"));
        assert!(!note.truncated);

        // 二进制文件 → Binary，无正文但有体积。
        std::fs::write(dir.path().join("clip.bin"), [0u8, 159, 146, 150]).unwrap();
        v.delete("clip.bin", DeleteMode::VaultTrash).unwrap();
        let bin_id = v
            .list_trash()
            .into_iter()
            .find(|e| e.original_path == "clip.bin")
            .unwrap()
            .id;
        let bin = v.trash_preview(&bin_id).unwrap();
        assert_eq!(bin.kind, TrashPreviewKind::Binary);
        assert!(bin.content.is_none());
        assert_eq!(bin.byte_size, 4);

        // 文件夹 → Folder，列子项相对路径。
        std::fs::create_dir_all(dir.path().join("proj/sub")).unwrap();
        v.write("proj/a.md", "甲").unwrap();
        v.write("proj/sub/b.md", "乙").unwrap();
        v.delete("proj", DeleteMode::VaultTrash).unwrap();
        let folder_id = v
            .list_trash()
            .into_iter()
            .find(|e| e.original_path == "proj")
            .unwrap()
            .id;
        let folder = v.trash_preview(&folder_id).unwrap();
        assert_eq!(folder.kind, TrashPreviewKind::Folder);
        assert_eq!(
            folder.children,
            vec!["a.md".to_string(), "sub/b.md".to_string()]
        );

        // 不存在的 id → 错误。
        assert!(v.trash_preview("nope").is_err());
    }

    #[test]
    fn permanent_delete_removes_file_and_index() {
        let (_d, v) = open_vault();
        v.write("a.md", "可搜索内容").unwrap();
        v.delete("a.md", DeleteMode::Permanent).unwrap();
        assert!(v.read("a.md").is_err());
        assert!(v.search(&Query::text("可搜索内容")).unwrap().is_empty());
    }

    #[test]
    fn external_change_syncs_content_links_and_deletion_into_one_index() {
        let (dir, v) = open_vault();
        v.write("topic.md", "---\nluna:\n  topic: true\n---\n主题")
            .unwrap();
        v.write("leaf.md", "旧正文").unwrap();

        std::fs::write(dir.path().join("leaf.md"), "手工新正文 [[topic]]").unwrap();
        v.sync_external_note("leaf.md").unwrap();
        assert_eq!(v.search(&Query::text("手工新正文")).unwrap().len(), 1);
        assert!(v
            .knowledge_metrics_facts()
            .unwrap()
            .linked_leaf_paths
            .contains(&"leaf.md".to_owned()));

        std::fs::remove_file(dir.path().join("leaf.md")).unwrap();
        v.sync_external_note("leaf.md").unwrap();
        assert!(v.search(&Query::text("手工新正文")).unwrap().is_empty());
    }

    #[test]
    fn failed_delete_keeps_index_until_file_operation_succeeds() {
        let (dir, v) = open_vault();
        v.write("a.md", "失败删除仍应可搜索").unwrap();
        let abs = dir.path().join("a.md");
        std::fs::remove_file(&abs).unwrap();
        std::fs::create_dir(&abs).unwrap();

        let err = v.delete("a.md", DeleteMode::Permanent).unwrap_err();

        assert!(matches!(err, VaultError::Io(_)));
        let hits = v.search(&Query::text("失败删除")).unwrap();
        assert_eq!(hits.len(), 1, "文件操作失败前不应提前移除索引");
    }

    #[test]
    fn clear_cache_preserves_durable_vault_state() {
        let (dir, v) = open_vault();
        v.write("a.md", "第一版缓存回归").unwrap();
        v.write("a.md", "第二版缓存回归").unwrap();

        let luna = dir.path().join(".luna");
        std::fs::write(luna.join("cache").join("thumb.bin"), b"cache").unwrap();
        std::fs::write(luna.join("sidecars").join("note.segments.json"), b"{}").unwrap();
        std::fs::write(luna.join("usage.sqlite"), b"usage").unwrap();
        std::fs::write(luna.join("settings.toml"), b"[vault]\n").unwrap();

        assert_eq!(v.search(&Query::text("第二版缓存")).unwrap().len(), 1);
        assert_eq!(v.snapshots("a.md").len(), 1);

        v.clear_cache().unwrap();

        assert!(luna.join("cache").is_dir(), "cache 目录应重建为空目录");
        assert!(
            std::fs::read_dir(luna.join("cache"))
                .unwrap()
                .next()
                .is_none(),
            "cache 内容应被清除"
        );
        assert!(luna.join("sidecars").join("note.segments.json").exists());
        assert!(luna.join("usage.sqlite").exists());
        assert!(luna.join("settings.toml").exists());
        assert_eq!(v.snapshots("a.md").len(), 1, "快照不是缓存");
        assert!(
            v.search(&Query::text("第二版缓存")).unwrap().is_empty(),
            "派生索引应被清空，等待 scan 重建"
        );

        v.scan().unwrap();
        assert_eq!(v.search(&Query::text("第二版缓存")).unwrap().len(), 1);
        assert_eq!(v.snapshots("a.md").len(), 1, "重建索引不得影响快照");
    }

    #[test]
    fn move_updates_index() {
        let (_d, v) = open_vault();
        v.write("a.md", "搬家内容讨论").unwrap();
        v.move_("a.md", "归档/a.md").unwrap();
        assert!(v.read("a.md").is_err());
        assert_eq!(v.read("归档/a.md").unwrap().content, "搬家内容讨论");
        let hits = v.search(&Query::text("搬家内容讨论")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "归档/a.md");
    }

    #[test]
    fn move_directory_rewrites_child_references_and_index() {
        let (_d, v) = open_vault();
        v.write("项目/计划.md", "目录移动内容").unwrap();
        v.write("首页.md", "见 [[计划]] 与 [直链](项目/计划.md)")
            .unwrap();
        v.move_("项目", "归档/项目").unwrap();
        assert_eq!(v.read("归档/项目/计划.md").unwrap().content, "目录移动内容");
        // 标准 md 链接更新到新路径；wikilink stem 未变仍解析。
        let home = v.read("首页.md").unwrap().content;
        assert!(
            home.contains("(归档/项目/计划.md)"),
            "标准链接应改写：{home}"
        );
        let hits = v.search(&Query::text("目录移动内容")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "归档/项目/计划.md");
        // 反链跟随新路径。
        assert_eq!(v.backlinks("归档/项目/计划.md").unwrap(), vec!["首页.md"]);
    }

    #[test]
    fn move_relocates_exclusive_attachment_and_keeps_link() {
        // 独占附件随笔记搬到新目录，相对链接不变（PRD-06-012）。
        let (d, v) = open_vault();
        std::fs::create_dir_all(d.path().join("Chats/assets")).unwrap();
        std::fs::write(d.path().join("Chats/assets/pic.png"), b"png").unwrap();
        v.write("Chats/a.md", "见图 ![](assets/pic.png)").unwrap();
        v.move_("Chats/a.md", "Notes/a.md").unwrap();
        // 附件搬到新目录同后缀处。
        assert!(d.path().join("Notes/assets/pic.png").is_file());
        assert!(!d.path().join("Chats/assets/pic.png").exists());
        // 链接不变（同后缀，从新目录仍解析）。
        assert!(v
            .read("Notes/a.md")
            .unwrap()
            .content
            .contains("![](assets/pic.png)"));
    }

    #[test]
    fn move_keeps_shared_attachment_and_rewrites_link() {
        // 共享附件（他人也引用）留原地，本笔记链接改写为可解析的 ../ 相对路径。
        let (d, v) = open_vault();
        std::fs::create_dir_all(d.path().join("Chats/assets")).unwrap();
        std::fs::write(d.path().join("Chats/assets/shared.png"), b"png").unwrap();
        v.write("Chats/a.md", "![](assets/shared.png)").unwrap();
        v.write("Chats/b.md", "![](assets/shared.png)").unwrap();
        v.move_("Chats/a.md", "Notes/a.md").unwrap();
        // 附件不动（b.md 仍引用）。
        assert!(d.path().join("Chats/assets/shared.png").is_file());
        // a.md 链接改写为从 Notes/ 可解析。
        assert!(
            v.read("Notes/a.md")
                .unwrap()
                .content
                .contains("![](../Chats/assets/shared.png)"),
            "共享附件链接应改写为 ../ 相对路径"
        );
    }

    #[test]
    fn rename_within_same_dir_leaves_attachments_untouched() {
        // 同目录纯重命名：附件不动、链接不变。
        let (d, v) = open_vault();
        std::fs::create_dir_all(d.path().join("Chats/assets")).unwrap();
        std::fs::write(d.path().join("Chats/assets/pic.png"), b"png").unwrap();
        v.write("Chats/a.md", "![](assets/pic.png)").unwrap();
        v.move_("Chats/a.md", "Chats/renamed.md").unwrap();
        assert!(d.path().join("Chats/assets/pic.png").is_file());
        assert!(v
            .read("Chats/renamed.md")
            .unwrap()
            .content
            .contains("![](assets/pic.png)"));
    }

    /// 录音音频（frontmatter `media[]` 附件）随笔记单文件移动：物理搬迁音频 + 边车到新笔记
    /// 附件目录同后缀处，`media[].path`/`segments` 库相对路径原地改写（PRD-04-030 / PRD-06-012）。
    #[test]
    fn move_relocates_recording_media_and_sidecar() {
        let (d, v) = open_vault();
        v.create(
            "录音/会议.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 会议\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // 音频落在笔记附件目录（note_subfolder 默认 = 录音/assets/）。
        std::fs::create_dir_all(d.path().join("录音/assets")).unwrap();
        std::fs::write(d.path().join("录音/assets/会议.wav"), b"wav").unwrap();
        let parts = vec![RecordingPart {
            path: "录音/assets/会议.wav".into(),
            start_ms: 0,
            end_ms: 3000,
            duration: Some(3.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/会议.md",
            &parts,
            &[seg(0, 1000, "你好")],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        v.move_("录音/会议.md", "会议归档/会议.md").unwrap();

        // 音频 + 边车物理搬到新笔记附件目录同后缀处，旧址清空。
        assert!(d.path().join("会议归档/assets/会议.wav").is_file());
        assert!(!d.path().join("录音/assets/会议.wav").exists());
        let doc = v.read("会议归档/会议.md").unwrap();
        let mr = &doc.meta.media[0];
        assert_eq!(mr.path, "会议归档/assets/会议.wav");
        let sidecar_rel = mr.segments.as_ref().expect("边车路径应存在");
        assert_eq!(sidecar_rel, &sidecar_path_for("会议归档/assets/会议.wav"));
        assert!(d.path().join(sidecar_rel).is_file(), "边车应随音频搬迁");
        assert!(matches!(
            read_sidecar_for_media(d.path(), &mr.path),
            SidecarLoad::Ok(_)
        ));
    }

    /// 录音笔记随**目录移动**：整树 rename 已迁移音频/边车文件，仅 frontmatter 库相对
    /// 路径前缀改写（不重复搬迁），且路径仍指向真实文件（PRD-06-006 + PRD-04-030）。
    #[test]
    fn move_directory_rewrites_recording_media_paths() {
        let (d, v) = open_vault();
        v.create(
            "录音/周会.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 周会\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("录音/assets")).unwrap();
        std::fs::write(d.path().join("录音/assets/周会.wav"), b"wav").unwrap();
        let parts = vec![RecordingPart {
            path: "录音/assets/周会.wav".into(),
            start_ms: 0,
            end_ms: 2000,
            duration: Some(2.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/周会.md",
            &parts,
            &[seg(0, 800, "开会")],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        v.move_("录音", "归档录音").unwrap();

        // 目录 rename 已把 assets 子树一并迁移；frontmatter 前缀改写指向真实新址。
        assert!(d.path().join("归档录音/assets/周会.wav").is_file());
        let doc = v.read("归档录音/周会.md").unwrap();
        let mr = &doc.meta.media[0];
        assert_eq!(mr.path, "归档录音/assets/周会.wav");
        assert!(matches!(
            read_sidecar_for_media(d.path(), &mr.path),
            SidecarLoad::Ok(_)
        ));
    }

    #[test]
    fn move_into_own_descendant_is_rejected() {
        let (_d, v) = open_vault();
        v.write("项目/计划.md", "x").unwrap();
        assert!(matches!(
            v.move_("项目", "项目/子目录"),
            Err(VaultError::InvalidMove(_))
        ));
        assert!(v.read("项目/计划.md").is_ok(), "拒绝后原文件不动");
    }

    #[test]
    fn create_dir_creates_and_rejects_duplicates() {
        let (_d, v) = open_vault();
        v.create_dir("新目录/嵌套").unwrap();
        assert!(v
            .tree()
            .unwrap()
            .children
            .iter()
            .any(|c| c.name == "新目录" && c.is_dir));
        assert!(matches!(
            v.create_dir("新目录/嵌套"),
            Err(VaultError::AlreadyExists(_))
        ));
    }

    #[test]
    fn list_dirs_returns_empty_dirs_so_a_freshly_created_folder_is_visible() {
        let (_d, v) = open_vault();
        v.create_dir("录音/空文件夹").unwrap();
        v.create_dir("录音/有东西/更深").unwrap();
        v.write("录音/有东西/一.md", "x").unwrap();

        let dirs = v.list_dirs("录音").unwrap();
        // 空目录必须在里面：归档清单的文件夹树只折条目路径的话它永远不生成节点，
        // 而「新建文件夹」按钮就在清单顶上（真机 2026-09-08）。
        assert!(dirs.contains(&"录音/空文件夹".to_string()), "{dirs:?}");
        assert!(dirs.contains(&"录音/有东西".to_string()), "{dirs:?}");
        assert!(
            dirs.contains(&"录音/有东西/更深".to_string()),
            "递归到每一层：{dirs:?}"
        );
        assert!(
            !dirs.iter().any(|d| d.ends_with(".md")),
            "只列目录：{dirs:?}"
        );
    }

    #[test]
    fn list_dirs_skips_dot_dirs_and_missing_root() {
        let (_d, v) = open_vault();
        v.create_dir("录音/正常").unwrap();
        std::fs::create_dir_all(v.root().join("录音").join(".隐藏")).unwrap();

        let dirs = v.list_dirs("录音").unwrap();
        assert_eq!(dirs, vec!["录音/正常".to_string()]);
        // 归档目录还没被创建过不是错误——那是"这一类还一件都没有"的正常状态。
        assert_eq!(v.list_dirs("从未存在过").unwrap(), Vec::<String>::new());
    }

    /// 一个没权限的子目录、一条指回祖先的符号链接都不能拖垮整棵文件树与文件夹清单
    /// （笔记库对标轮 V1 真机：子目录 0o000 时文件树永远「加载中」、状态栏报错）。
    #[cfg(unix)]
    #[test]
    fn tree_and_dir_list_survive_unreadable_folders_and_symlink_loops() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, v) = open_vault();
        v.write("甲/a.md", "x").unwrap();
        v.write("锁住/b.md", "y").unwrap();
        std::os::unix::fs::symlink(dir.path().join("甲"), dir.path().join("甲/回环")).unwrap();
        let locked = dir.path().join("锁住");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let tree = v.tree();
        let dirs = v.list_dirs("");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tree = tree.unwrap();
        let names: Vec<_> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["甲", "锁住"]);
        let jia = &tree.children[0];
        assert_eq!(
            jia.children
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.md"],
            "指向目录的链接不跟随、不列出"
        );
        assert!(
            tree.children[1].children.is_empty(),
            "读不了的目录显示为空文件夹"
        );
        assert_eq!(dirs.unwrap(), vec!["甲".to_owned(), "锁住".to_owned()]);
    }

    #[test]
    fn tree_sorts_dirs_first_case_insensitive() {
        let (_d, v) = open_vault();
        v.write("b.md", "x").unwrap();
        v.write("Alpha.md", "x").unwrap();
        v.write("目录/内.md", "x").unwrap();
        let tree = v.tree().unwrap();
        let names: Vec<_> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["目录", "Alpha.md", "b.md"]);
        assert!(tree.children[1].mtime_ms > 0, "文件应带 mtime");
    }

    /// `topic_ingest_pending_of` 用的必须是**传进来的那份正文**，不是重新读盘。
    ///
    /// 目录那一行的摘要、所有权与待归入数是一次读取的四个投影；若这一项偷偷再读一次，
    /// 两次读之间的一次保存就会让同一行里"这一页写了什么"和"它还差几篇"来自两个版本，
    /// 而界面上看不出这种错位。
    ///
    /// 夹具让盘上与传入两份正文**在同一条候选上给出相反答案**：盘上那份已经引用了源，
    /// 传入那份没有。读盘就得到空，用传入正文才得到那一条。
    #[test]
    fn topic_ingest_pending_of_reads_the_content_it_was_given() {
        let (_d, v) = open_vault();
        v.write(
            "Notes/源.md",
            "---\nluna:\n  schema: 1\n  type: note\ntags:\n  - 主题A\n---\n正文\n",
        )
        .unwrap();
        v.write_topic(
            "Notes/主题 A.md",
            "# 主题 A\n\n## 进展\n\n- 见 [源](源.md)。\n",
            Some("主题A"),
        )
        .unwrap();

        // 盘上那份已经引用了 `源.md`，故按盘上算是"没有待归入"。
        assert!(
            v.topic_ingest_pending("Notes/主题 A.md")
                .unwrap()
                .is_empty(),
            "夹具前提：盘上那份确实已引用该候选"
        );

        // 传入一份**没有**引用它的正文——契约要求按传入的算，于是那一条重新成为待归入。
        let without_link = v
            .topic_content(
                "Notes/主题 A.md",
                "# 主题 A\n\n## 进展\n\n- 起步。\n",
                Some("主题A"),
            )
            .unwrap();
        assert_eq!(
            v.topic_ingest_pending_of("Notes/主题 A.md", &without_link)
                .unwrap(),
            vec!["Notes/源.md".to_string()],
            "拿到空说明它把盘上那份读回来了，没用传进去的正文"
        );
    }

    /// 知识产物不进笔记树（ADR-096 ②）：主题页与知识地图是系统提炼的只读内容，
    /// 应用内归知识库空间；文件仍在磁盘原处（`read` 照常可达，外部工具照常可见）。
    /// 本测试取代原「树上给主题页标图标」——身份标识随产物一起迁去了知识库目录。
    #[test]
    fn tree_excludes_knowledge_products() {
        let (_d, v) = open_vault();
        seed_note(&v, "Notes/普通", NoteType::Note, "正文");
        v.write_topic(
            "Notes/项目 X.md",
            "# 项目 X\n\n## 进展\n\n- 起步。\n",
            Some("项目X"),
        )
        .unwrap();
        v.write_knowledge_map("Notes/知识地图.md", "- 目录\n")
            .unwrap();
        let tree = v.tree().unwrap();
        let notes = tree
            .children
            .iter()
            .find(|c| c.name == "Notes")
            .expect("Notes 目录");
        assert!(
            notes.children.iter().any(|c| c.name == "普通.md"),
            "普通笔记照常在树上"
        );
        for product in ["项目 X.md", "知识地图.md"] {
            assert!(
                !notes.children.iter().any(|c| c.name == product),
                "{product} 是知识产物，不该出现在笔记树里"
            );
        }
        // 文件真身仍在磁盘：出树不是出库。
        assert!(v.read("Notes/项目 X.md").is_ok());
        // 产物清单由派生索引一次给全（前端路由闸口的取数面）。
        let products = v.knowledge_product_paths().unwrap();
        assert_eq!(
            products,
            vec![
                "Notes/知识地图.md".to_string(),
                "Notes/项目 X.md".to_string()
            ]
        );
    }

    #[test]
    fn path_escape_is_rejected() {
        let (_d, v) = open_vault();
        assert!(matches!(
            v.read("../outside.md"),
            Err(VaultError::PathEscape(_))
        ));
        assert!(v.write("../evil.md", "x").is_err());
    }

    // ── finalize_recording 测试 ──────────────────────────────────────────────

    #[test]
    fn finalize_recording_single_part_writes_sidecar_body_and_media() {
        let (_d, v) = open_vault();
        // 先建录音笔记骨架。
        v.create(
            "录音/会议.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 会议\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let segments = vec![
            seg(0, 1200, "大家好"),
            seg(1200, 3400, "开始评审"),
            seg(3400, 5000, "下面请发言"),
        ];
        let parts = vec![RecordingPart {
            path: "Recordings/meeting-001.wav".into(),
            start_ms: 0,
            end_ms: 5001,
            duration: Some(5.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/会议.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let doc = v.read("录音/会议.md").unwrap();

        // 正文含行标记。
        assert!(
            doc.content.contains("[00:00:00.000] 大家好"),
            "缺第一行标记"
        );
        assert!(
            doc.content.contains("[00:00:01.200] 开始评审"),
            "缺第二行标记"
        );
        assert!(
            doc.content.contains("[00:00:03.400] 下面请发言"),
            "缺第三行标记"
        );

        // frontmatter 已登记媒体（transcript=true，segments 路径存在）。
        let meta = &doc.meta;
        assert_eq!(meta.media.len(), 1);
        let mr = &meta.media[0];
        assert_eq!(mr.path, "Recordings/meeting-001.wav");
        assert!(mr.transcript);
        assert!(mr.segments.is_some(), "segments 路径应存在");

        // 边车文件已落盘且可读。
        let sidecar_abs = _d.path().join(mr.segments.as_ref().unwrap());
        match read_sidecar(&sidecar_abs) {
            SidecarLoad::Ok(sc) => assert_eq!(sc.segments.len(), 3),
            other => panic!("边车读取失败：{other:?}"),
        }

        // 单 part 不写区间字段。
        assert!(mr.part_start_ms.is_none(), "单 part 不写 part_start_ms");
        assert!(mr.part_end_ms.is_none(), "单 part 不写 part_end_ms");
    }

    /// 书签两层落盘（PRD-04-005）：frontmatter `luna.bookmarks` 精确层（升序去重）+
    /// 正文 `[HH:MM:SS.mmm] 🔖` 可读锚点行按时间序穿插在转写行之间。
    #[test]
    fn finalize_recording_writes_bookmarks_two_layers() {
        let (_d, v) = open_vault();
        v.create(
            "录音/书签.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 书签\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let segments = vec![seg(0, 1200, "大家好"), seg(3400, 5000, "下面请发言")];
        let parts = vec![RecordingPart {
            path: "Recordings/bm-001.wav".into(),
            start_ms: 0,
            end_ms: 5001,
            duration: Some(5.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        // 乱序 + 重复书签：finalize 应升序去重。
        v.finalize_recording(
            "录音/书签.md",
            &parts,
            &segments,
            &[
                Bookmark {
                    ms: 2000,
                    note: None,
                },
                Bookmark {
                    ms: 500,
                    note: Some("  重点\n结论 ".into()),
                },
                Bookmark {
                    ms: 2000,
                    note: Some("再议".into()),
                },
            ],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let doc = v.read("录音/书签.md").unwrap();

        // 精确层：frontmatter 升序去重。
        assert_eq!(doc.meta.bookmarks, vec![500, 2000]);

        // 可读层：锚点行存在，且按时间序穿插（500ms 在两条转写行之间）。
        let body = body_of(&doc.content);
        // 备注一行化（换行折空格、去首尾），同一时刻两条留有备注的后者。
        assert!(
            body.contains("[00:00:00.500] \u{1F516} 重点 结论\n"),
            "缺 500ms 锚点行或备注未归一化：\n{body}"
        );
        assert!(
            body.contains("[00:00:02.000] \u{1F516} 再议\n"),
            "缺 2000ms 锚点行或同刻去重丢了备注：\n{body}"
        );
        // 读回：精确层毫秒 + 可读层备注对上。
        assert_eq!(
            bookmarks_of(&doc.meta.bookmarks, &doc.content),
            vec![
                Bookmark {
                    ms: 500,
                    note: Some("重点 结论".into())
                },
                Bookmark {
                    ms: 2000,
                    note: Some("再议".into())
                },
            ]
        );
        // 用户手写的 🔖 行（精确层没有）也算书签；无备注的 🔖 行备注为空。
        assert_eq!(
            parse_bookmark_lines("[00:00:09.000] \u{1F516}\n[00:00:10.000] \u{1F516}   手记 \n"),
            vec![
                Bookmark {
                    ms: 9000,
                    note: None
                },
                Bookmark {
                    ms: 10_000,
                    note: Some("手记".into())
                },
            ]
        );
        let pos_seg1 = body.find("[00:00:00.000] 大家好").unwrap();
        let pos_bm1 = body.find("[00:00:00.500]").unwrap();
        let pos_seg2 = body.find("[00:00:03.400] 下面请发言").unwrap();
        assert!(
            pos_seg1 < pos_bm1 && pos_bm1 < pos_seg2,
            "书签行应按时间序穿插在转写行之间"
        );
    }

    #[test]
    fn quick_note_products_reads_sections_block_and_plain_bodies() {
        let content = "---\nluna:\n  type: quick-note\n---\n## 优化结果\n\n我今天要记录。\n\n## 原始转录\n\n我 这个 今天 要 记录\n\n## 待办\n- [ ] 记录\n\n<!-- luna:transcript -->\n[00:00:00.000] 我今天要记录\n[00:00:00.500] \u{1F516} 备注\n> translation line\n<!-- /luna:transcript -->\n";
        let products = quick_note_products(content);
        assert_eq!(products.refined.as_deref(), Some("我今天要记录。"));
        assert_eq!(products.raw.as_deref(), Some("我 这个 今天 要 记录"));
        assert_eq!(products.second_pass.as_deref(), Some("我今天要记录"));

        let english = "## Polished text\n\nDone.\n\n## Raw transcript\n\ndone done\n";
        let products = quick_note_products(english);
        assert_eq!(products.refined.as_deref(), Some("Done."));
        assert_eq!(products.raw.as_deref(), Some("done done"));

        let spoken = "---\nluna:\n  type: quick-note\n  media:\n    - path: QuickNotes/assets/v.wav\n---\njust words\n";
        let products = quick_note_products(spoken);
        assert_eq!(products.raw.as_deref(), Some("just words"));
        assert!(products.refined.is_none() && products.second_pass.is_none());

        let empty_block =
            "## 优化结果\n\nx\n\n<!-- luna:transcript -->\n<!-- /luna:transcript -->\n";
        assert!(quick_note_products(empty_block).second_pass.is_none());
    }

    /// 文字速记（捕获框 / 文字速记面板 / 「服务」菜单）的正文不是原始转写：没有分节、
    /// 也没有登记录音时一个产物都不认（真机 2026-10-05：敲进去的「duizhao」顶上挂着
    /// 「原始转写 7 字」）。
    #[test]
    fn typed_quick_note_has_no_raw_transcript() {
        let typed = "---\nluna:\n  schema: 1\n  type: quick-note\n  created: 2026-10-05T10:00:00+08:00\n---\nduizhao\n";
        assert_eq!(quick_note_products(typed), QuickNoteProducts::default());

        let (_d, v) = open_vault();
        v.create(
            "速记/typed.md",
            NoteType::QuickNote,
            &TemplateVars {
                body: "从「服务」菜单存进来的一段话".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let doc = v.read("速记/typed.md").unwrap();
        assert_eq!(
            quick_note_products(&doc.content),
            QuickNoteProducts::default()
        );
    }

    #[test]
    fn adopted_version_round_trips_through_frontmatter() {
        let (_d, v) = open_vault();
        v.create(
            "速记/a.md",
            NoteType::QuickNote,
            &TemplateVars {
                body: "## 优化结果\n\nx\n\n## 原始转录\n\ny\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        v.set_quick_note_adopted("速记/a.md", Some(AdoptedVersion::Raw))
            .unwrap();
        let doc = v.read("速记/a.md").unwrap();
        assert_eq!(doc.meta.adopted.as_deref(), Some("raw"));
        assert!(doc.content.contains("## 原始转录\n\ny"), "正文不动");
        v.set_quick_note_adopted("速记/a.md", None).unwrap();
        assert!(v.read("速记/a.md").unwrap().meta.adopted.is_none());
        assert_eq!(
            AdoptedVersion::parse("second_pass"),
            Some(AdoptedVersion::SecondPass)
        );
        assert_eq!(AdoptedVersion::parse("nope"), None);
    }

    /// 重写转写块（二次转写 / 导入转写）必须保留书签备注：备注只活在可读层，重写前不读回
    /// 就会被冲掉——用户在会议里敲的那句话不能因为换了个模型就没了。
    #[test]
    fn rewriting_the_transcript_keeps_bookmark_notes() {
        let (_d, v) = open_vault();
        v.create(
            "录音/备注.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 备注\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let parts = vec![RecordingPart {
            path: "Recordings/n.wav".into(),
            start_ms: 0,
            end_ms: 5000,
            duration: Some(5.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/备注.md",
            &parts,
            &[seg(0, 1000, "第一版")],
            &[Bookmark {
                ms: 700,
                note: Some("关键决定".into()),
            }],
            TranscriptAuthority::Preview,
        )
        .unwrap();
        v.rewrite_recording_transcript(
            "录音/备注.md",
            &[seg(0, 1000, "第二版")],
            None,
            TranscriptAuthority::Unchanged,
        )
        .unwrap();
        let doc = v.read("录音/备注.md").unwrap();
        assert!(doc.content.contains("[00:00:00.000] 第二版"));
        assert!(
            doc.content.contains("[00:00:00.700] \u{1F516} 关键决定"),
            "重写后备注丢了：\n{}",
            doc.content
        );
        assert_eq!(doc.meta.bookmarks, vec![700]);
    }

    /// 权威转写标记（PRD-16-047）：实时预览不是权威，批量写回才是；人工编辑不改变它，
    /// 再次以预览归档（重新录一段）会如实降回。
    #[test]
    fn transcript_authority_follows_the_writer_not_the_function() {
        let (_d, v) = open_vault();
        v.create(
            "录音/权威.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 权威\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let parts = vec![RecordingPart {
            path: "Recordings/qw-001.wav".into(),
            start_ms: 0,
            end_ms: 2000,
            duration: Some(2.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/权威.md",
            &parts,
            &[seg(0, 1000, "预览")],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();
        assert!(
            !v.read("录音/权威.md")
                .unwrap()
                .meta
                .transcript_authoritative
        );
        v.rewrite_recording_transcript(
            "录音/权威.md",
            &[seg(0, 1000, "批量")],
            Some("whisper".into()),
            TranscriptAuthority::Authoritative,
        )
        .unwrap();
        assert!(
            v.read("录音/权威.md")
                .unwrap()
                .meta
                .transcript_authoritative
        );
        v.update_playback_transcript(
            "录音/权威.md",
            &[CueTextEdit {
                start_ms: 0,
                end_ms: 1000,
                text: "人工校对".into(),
            }],
        )
        .unwrap();
        assert!(
            v.read("录音/权威.md")
                .unwrap()
                .meta
                .transcript_authoritative,
            "人工校对不改变权威性"
        );
    }

    /// 二次转写写回（PRD-04-030）：重写边车 + 翻正 transcript/回填 model，**正文不动**。
    #[test]
    fn apply_second_pass_rewrites_sidecar_and_metadata_without_touching_body() {
        let (_d, v) = open_vault();
        v.create(
            "录音/二转.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 二转\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // 首次归档：零段纯录音（transcript=false、无边车）。
        let parts = vec![RecordingPart {
            path: "Recordings/sp-001.wav".into(),
            start_ms: 0,
            end_ms: 4000,
            duration: Some(4.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/二转.md",
            &parts,
            &[],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        // 权威重转写：整体替换转写（正文哨兵块 + 边车 + 标记一致更新）。
        let segments = vec![seg(0, 1500, "重转第一段"), seg(1500, 3000, "重转第二段")];
        v.rewrite_recording_transcript(
            "录音/二转.md",
            &segments,
            Some("ggml-large-v3".into()),
            TranscriptAuthority::Unchanged,
        )
        .unwrap();

        let doc = v.read("录音/二转.md").unwrap();
        let mr = &doc.meta.media[0];
        assert!(mr.transcript, "transcript 应翻正");
        assert_eq!(mr.model.as_deref(), Some("ggml-large-v3"));
        let sidecar_abs = _d.path().join(mr.segments.as_ref().unwrap());
        match read_sidecar(&sidecar_abs) {
            SidecarLoad::Ok(sc) => assert_eq!(sc.segments.len(), 2),
            other => panic!("边车读取失败：{other:?}"),
        }
        // 正文↔边车一致：转写块被整块重写为新段。
        assert!(doc.content.contains("[00:00:00.000] 重转第一段"));
        assert!(doc.content.contains("[00:00:01.500] 重转第二段"));
        assert!(doc.content.contains(TRANSCRIPT_BEGIN) && doc.content.contains(TRANSCRIPT_END));

        // 幂等：再次重转写为空 → transcript 翻回 false、去边车引用（诚实元数据）。
        v.rewrite_recording_transcript("录音/二转.md", &[], None, TranscriptAuthority::Unchanged)
            .unwrap();
        let doc2 = v.read("录音/二转.md").unwrap();
        assert!(!doc2.meta.media[0].transcript);
        assert!(doc2.meta.media[0].segments.is_none());
    }

    #[test]
    fn update_playback_cue_updates_sidecar_and_markdown_line() {
        let (_d, v) = open_vault();
        v.create(
            "录音/修订.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 修订\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let segments = vec![seg(0, 1500, "原始第一段"), seg(1500, 3000, "原始第二段")];
        let parts = vec![RecordingPart {
            path: "Recordings/edit-001.wav".into(),
            start_ms: 0,
            end_ms: 3000,
            duration: Some(3.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/修订.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        v.update_playback_cue("录音/修订.md", 1500, 3000, "修订后的第二段", None)
            .unwrap();

        let doc = v.read("录音/修订.md").unwrap();
        assert!(doc.content.contains("[00:00:01.500] 修订后的第二段"));
        assert!(
            !doc.content.contains("[00:00:01.500] 原始第二段"),
            "同一时间戳正文行应被替换"
        );

        let sidecar_abs = _d.path().join(doc.meta.media[0].segments.as_ref().unwrap());
        match read_sidecar(&sidecar_abs) {
            SidecarLoad::Ok(sc) => assert_eq!(sc.segments[1].text, "修订后的第二段"),
            other => panic!("边车读取失败：{other:?}"),
        }
    }

    /// 整篇编辑批量写回（PRD-04-031）：多条 cue 文本一次落盘、保留译文、空文本拒绝、
    /// 无改动幂等 no-op。
    #[test]
    fn update_playback_transcript_batch_preserves_translation() {
        let (_d, v) = open_vault();
        v.create(
            "录音/整篇.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 整篇\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // 第二段带译文：批量改文本后译文须保留（整篇编辑只碰源文）。
        let mut second = seg(1500, 3000, "原始第二段");
        second.translation = Some("original second".into());
        let segments = vec![seg(0, 1500, "原始第一段"), second];
        let parts = vec![RecordingPart {
            path: "Recordings/whole-001.wav".into(),
            start_ms: 0,
            end_ms: 3000,
            duration: Some(3.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/整篇.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let edits = vec![
            CueTextEdit {
                start_ms: 0,
                end_ms: 1500,
                text: "  校对第一段  ".into(), // 前后空白应被 trim
            },
            CueTextEdit {
                start_ms: 1500,
                end_ms: 3000,
                text: "校对第二段".into(),
            },
        ];
        v.update_playback_transcript("录音/整篇.md", &edits)
            .unwrap();

        let doc = v.read("录音/整篇.md").unwrap();
        assert!(doc.content.contains("[00:00:00.000] 校对第一段"));
        assert!(doc.content.contains("[00:00:01.500] 校对第二段"));
        let sidecar_abs = _d.path().join(doc.meta.media[0].segments.as_ref().unwrap());
        match read_sidecar(&sidecar_abs) {
            SidecarLoad::Ok(sc) => {
                assert_eq!(sc.segments[0].text, "校对第一段");
                assert_eq!(sc.segments[1].text, "校对第二段");
                assert_eq!(
                    sc.segments[1].translation.as_deref(),
                    Some("original second"),
                    "整篇编辑源文不应丢失既有译文"
                );
            }
            other => panic!("边车读取失败：{other:?}"),
        }

        // 空文本拒绝整批。
        assert!(v
            .update_playback_transcript(
                "录音/整篇.md",
                &[CueTextEdit {
                    start_ms: 0,
                    end_ms: 1500,
                    text: "   ".into(),
                }]
            )
            .is_err());

        // 无匹配/无改动 = 幂等 no-op（Ok，不写盘）。
        v.update_playback_transcript(
            "录音/整篇.md",
            &[CueTextEdit {
                start_ms: 9_000,
                end_ms: 9_500,
                text: "不存在的段".into(),
            }],
        )
        .unwrap();
        let doc2 = v.read("录音/整篇.md").unwrap();
        assert!(
            doc2.content.contains("[00:00:00.000] 校对第一段"),
            "no-op 不改动"
        );
    }

    /// 转写段人工校正（PRD-04-031 / PRD-04-056）：重指派按范围生效、拆分改段界与段数并同步
    /// 正文行标记、合并为拆分的逆操作、删除整条丢弃且不动邻段时间，四者都经权威写回保持
    /// 「正文 ↔ 边车」一致。
    #[test]
    fn edit_transcript_segment_assign_split_merge_delete_round_trip() {
        let (_d, v) = open_vault();
        v.create(
            "录音/校正.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 校正\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut segments = vec![
            seg(0, 1_500, "第一句话"),
            seg(1_500, 4_500, "第二句话第三句话"),
        ];
        segments[0].speaker = Some("remote:1".into());
        segments[1].speaker = Some("remote:1".into());
        let parts = vec![RecordingPart {
            path: "Recordings/fix-001.wav".into(),
            start_ms: 0,
            end_ms: 4500,
            duration: Some(4.5),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/校正.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let sidecar_of = |v: &Vault| {
            let doc = v.read("录音/校正.md").unwrap();
            let abs = _d.path().join(doc.meta.media[0].segments.as_ref().unwrap());
            match read_sidecar(&abs) {
                SidecarLoad::Ok(sc) => sc.segments,
                other => panic!("边车读取失败：{other:?}"),
            }
        };

        // 1. 重指派（仅此条）：只改首段。
        v.edit_transcript_segment(
            "录音/校正.md",
            &SegmentEdit::Assign {
                start_ms: 0,
                end_ms: 1_500,
                speaker: "remote:2".into(),
                scope: luna_core::AssignScope::Single,
            },
        )
        .unwrap();
        let segs = sidecar_of(&v);
        assert_eq!(segs[0].speaker.as_deref(), Some("remote:2"));
        assert_eq!(segs[1].speaker.as_deref(), Some("remote:1"));

        // 2. 拆分第二段（无 words → 按文本长度内插），右片指派另一位说话人。
        v.edit_transcript_segment(
            "录音/校正.md",
            &SegmentEdit::Split {
                start_ms: 1_500,
                end_ms: 4_500,
                char_offset: 4,
                speaker: Some("remote:2".into()),
            },
        )
        .unwrap();
        let segs = sidecar_of(&v);
        assert_eq!(segs.len(), 3, "拆分后段数 +1");
        assert_eq!(segs[1].text, "第二句话");
        assert_eq!(segs[2].text, "第三句话");
        assert_eq!(segs[2].speaker.as_deref(), Some("remote:2"));
        let doc = v.read("录音/校正.md").unwrap();
        assert!(
            doc.content.contains("[00:00:03.000] 第三句话"),
            "正文行标记随新段界整块重建：{}",
            doc.content
        );

        // 3. 合并回去（拆分的逆操作）：段数复原、文本重新拼接。
        v.edit_transcript_segment(
            "录音/校正.md",
            &SegmentEdit::Merge {
                start_ms: 3_000,
                end_ms: 4_500,
            },
        )
        .unwrap();
        let segs = sidecar_of(&v);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].text, "第二句话第三句话");
        assert_eq!(segs[1].end, Duration::from_millis(4_500));

        // 4. 删除首条：段数 -1，剩余段时间原地不动，正文行标记随之消失。
        v.edit_transcript_segment(
            "录音/校正.md",
            &SegmentEdit::Delete {
                start_ms: 0,
                end_ms: 1_500,
            },
        )
        .unwrap();
        let segs = sidecar_of(&v);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "第二句话第三句话");
        assert_eq!(
            segs[0].start,
            Duration::from_millis(1_500),
            "邻段时间不重排"
        );
        let doc = v.read("录音/校正.md").unwrap();
        assert!(
            !doc.content.contains("第一句话"),
            "正文哨兵块整块重建，被删的行不再出现：{}",
            doc.content
        );

        // 5. 错误路径（均针对删除后的实际段序列）：删已删的段 / 首段无上一条 / 切出空片 /
        //    指派未匹配段。
        assert!(v
            .edit_transcript_segment(
                "录音/校正.md",
                &SegmentEdit::Delete {
                    start_ms: 0,
                    end_ms: 1_500
                }
            )
            .is_err());
        assert!(v
            .edit_transcript_segment(
                "录音/校正.md",
                &SegmentEdit::Merge {
                    start_ms: 1_500,
                    end_ms: 4_500
                }
            )
            .is_err());
        assert!(v
            .edit_transcript_segment(
                "录音/校正.md",
                &SegmentEdit::Split {
                    start_ms: 1_500,
                    end_ms: 4_500,
                    char_offset: 0,
                    speaker: None
                }
            )
            .is_err());
        assert!(v
            .edit_transcript_segment(
                "录音/校正.md",
                &SegmentEdit::Assign {
                    start_ms: 8_000,
                    end_ms: 9_000,
                    speaker: "self".into(),
                    scope: luna_core::AssignScope::Single,
                }
            )
            .is_err());
    }

    /// 删到一条不剩（整篇都是噪声的极端情形）：媒体标记诚实翻回「无转写」并去掉边车引用，
    /// 不留下一个指向空边车的假 `transcript: true`（TDD-06 §2.2 诚实元数据）。
    #[test]
    fn delete_last_segment_marks_media_without_transcript() {
        let (_d, v) = open_vault();
        v.create(
            "录音/删空.md",
            NoteType::Recording,
            &TemplateVars::default(),
        )
        .unwrap();
        let parts = vec![RecordingPart {
            path: "Recordings/empty-001.wav".into(),
            start_ms: 0,
            end_ms: 1_000,
            duration: Some(1.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/删空.md",
            &parts,
            &[seg(0, 1_000, "唯一一条")],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        v.edit_transcript_segment(
            "录音/删空.md",
            &SegmentEdit::Delete {
                start_ms: 0,
                end_ms: 1_000,
            },
        )
        .unwrap();

        let doc = v.read("录音/删空.md").unwrap();
        assert!(!doc.meta.media[0].transcript);
        assert!(doc.meta.media[0].segments.is_none());
        assert!(!doc.content.contains("唯一一条"));
    }

    /// 说话人改名持久化（PRD-04-054）：写 frontmatter `luna.speakers`，重开笔记仍在；
    /// 空名 = 删除映射回退默认标签；改名不动正文与其他 frontmatter。
    #[test]
    fn set_speaker_name_persists_in_frontmatter_and_empty_removes() {
        let (_d, v) = open_vault();
        v.create(
            "录音/改名.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 改名\n\n用户散文保留。\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        v.set_speaker_name("录音/改名.md", "remote:1", "张三")
            .unwrap();
        v.set_speaker_name("录音/改名.md", "self", "我自己")
            .unwrap();

        // 重开笔记：映射仍在（笔记即文件）。
        let doc = v.read("录音/改名.md").unwrap();
        assert_eq!(
            doc.meta.speakers.get("remote:1").map(String::as_str),
            Some("张三")
        );
        assert_eq!(
            doc.meta.speakers.get("self").map(String::as_str),
            Some("我自己")
        );
        assert!(doc.content.contains("用户散文保留。"), "正文不受影响");

        // 空名 = 删除映射。
        v.set_speaker_name("录音/改名.md", "remote:1", "  ")
            .unwrap();
        let doc2 = v.read("录音/改名.md").unwrap();
        assert!(!doc2.meta.speakers.contains_key("remote:1"));
        assert!(doc2.meta.speakers.contains_key("self"));

        // 空 speaker id 拒绝。
        assert!(v.set_speaker_name("录音/改名.md", " ", "x").is_err());
    }

    /// 「仅存链接」的转写（PRD-05-006）：**没有媒体也要能存段落**，且**取回媒体后段落
    /// 必须跟着迁到媒体槽**——挂载槽的判定随 `meta.media` 是否为空翻转，若不迁移，
    /// 回放会读一个空边车，用户看到的是"文稿凭空消失了"。
    #[test]
    fn transcript_survives_from_link_only_slot_to_media_slot() {
        let (_d, v) = open_vault();
        let note = "Transcripts/仅链接.md";
        v.create(
            note,
            NoteType::Transcript,
            &TemplateVars {
                body: "# 仅链接

"
                .into(),
                ..Default::default()
            },
        )
        .unwrap();
        let url = "https://www.youtube.com/watch?v=abc";
        v.set_source_url(note, url).unwrap();

        // 无媒体时写段落 → 落到「来源地址」那个槽。
        let segments = vec![seg(0, 2_000, "第一句"), seg(2_000, 4_000, "第二句")];
        v.rewrite_recording_transcript(
            note,
            &segments,
            Some("whisper".into()),
            TranscriptAuthority::Unchanged,
        )
        .unwrap();
        let meta = v.read(note).unwrap().meta;
        assert!(meta.media.is_empty(), "仅存链接不登记媒体分片");
        let slots = crate::transcript_slots(&meta);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key, url, "段落挂在来源地址上");
        let loaded = match crate::read_sidecar_for_media(v.root(), url) {
            crate::SidecarLoad::Ok(sc) => sc.segments,
            other => panic!("读不到来源槽的边车：{other:?}"),
        };
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, "第一句");

        // 取回媒体：登记分片后**原样写回**，段落应落到媒体槽。
        let media_rel = "Transcripts/assets/仅链接.wav";
        v.finalize_recording(
            note,
            &[RecordingPart {
                path: media_rel.into(),
                start_ms: 0,
                end_ms: 4_000,
                duration: Some(4.0),
                model: None,
                lang: None,
                tracks: None,
            }],
            &[],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();
        v.rewrite_recording_transcript(note, &segments, None, TranscriptAuthority::Unchanged)
            .unwrap();

        let meta = v.read(note).unwrap().meta;
        let slots = crate::transcript_slots(&meta);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key, media_rel, "有媒体后槽切到媒体路径");
        let migrated = match crate::read_sidecar_for_media(v.root(), media_rel) {
            crate::SidecarLoad::Ok(sc) => sc.segments,
            other => panic!("段落没有迁到媒体槽：{other:?}"),
        };
        assert_eq!(migrated.len(), 2, "文稿必须完整跟过来，不能凭空消失");
        assert_eq!(migrated[1].text, "第二句");
        assert!(meta.media[0].transcript, "媒体标记随之翻正");
    }

    /// **边车文件与段落一一对应**：删光段落必须**物理删除**边车文件，否则下次加载会把
    /// 删掉的段落读回来——读取端只认"文件在不在"（无媒体槽没有 `media.segments` 可查），
    /// 只把标记置空是"逻辑删了、物理还在"的二义状态。
    #[test]
    fn clearing_segments_removes_the_sidecar_file() {
        let (_d, v) = open_vault();
        let note = "Transcripts/删空.md";
        v.create(
            note,
            NoteType::Transcript,
            &TemplateVars {
                body: "# 删空

"
                .into(),
                ..Default::default()
            },
        )
        .unwrap();
        let url = "https://a.com/v";
        v.set_source_url(note, url).unwrap();

        v.rewrite_recording_transcript(
            note,
            &[seg(0, 1_000, "一句话")],
            None,
            TranscriptAuthority::Unchanged,
        )
        .unwrap();
        let sidecar = v.root().join(crate::sidecar_path_for(url));
        assert!(sidecar.is_file(), "有段落时边车在场");

        // 删光段落（用户在校对里把最后一条也删了）。
        v.rewrite_recording_transcript(note, &[], None, TranscriptAuthority::Unchanged)
            .unwrap();
        assert!(!sidecar.exists(), "段落没了，边车文件必须一并消失");
        // 再读一次：读取端只认文件，故得到空——而不是把删掉的那句读回来。
        let meta = v.read(note).unwrap().meta;
        let slots = crate::transcript_slots(&meta);
        assert_eq!(slots.len(), 1);
        assert!(matches!(
            crate::read_sidecar_for_media(v.root(), &slots[0].key),
            crate::SidecarLoad::Missing
        ));
    }

    /// 抓取来源落笔记 frontmatter（PRD-05-006）：跨重开仍在、正文不受影响、空串清除。
    /// 溯源写在文件里而不是索引里——索引可重建，而"这篇来自哪个视频"重建不出来。
    /// 导入文档的去重只看同一层文件夹（ADR-164 ④），且只认 `document` 与指纹都对得上的那篇。
    #[test]
    fn find_document_matches_sha_within_one_folder_only() {
        let (_d, v) = open_vault();
        let create = |rel: &str, note_type: NoteType, sha: Option<&str>| {
            v.create(rel, note_type, &TemplateVars::default()).unwrap();
            if let Some(sha) = sha {
                let doc = v.read(rel).unwrap();
                let mut meta = doc.meta.clone();
                meta.original = Some(crate::OriginalFile {
                    path: format!("{rel}.docx"),
                    sha256: sha.to_owned(),
                    format: "docx".to_owned(),
                });
                v.write(rel, &update_luna(&doc.content, &meta).unwrap())
                    .unwrap();
            }
        };
        create("P/素材/提纲.md", NoteType::Document, Some("aa"));
        create("P/素材/笔记.md", NoteType::Note, None);
        create("P/素材/子/深处.md", NoteType::Document, Some("bb"));
        create("Q/素材/提纲.md", NoteType::Document, Some("cc"));

        assert_eq!(
            v.find_document("P/素材", "aa").unwrap().as_deref(),
            Some("P/素材/提纲.md")
        );
        assert_eq!(
            v.find_document("P/素材", "bb").unwrap(),
            None,
            "子文件夹不算"
        );
        assert_eq!(
            v.find_document("P/素材", "cc").unwrap(),
            None,
            "别的项目不算"
        );
        assert_eq!(v.find_document("不存在", "aa").unwrap(), None);
        let meta = v.read("P/素材/提纲.md").unwrap().meta;
        assert_eq!(meta.original.unwrap().format, "docx", "luna.original 往返");
    }

    #[test]
    fn set_source_url_persists_in_frontmatter_and_empty_clears() {
        let (_d, v) = open_vault();
        v.create(
            "Transcripts/分享.md",
            NoteType::Transcript,
            &TemplateVars {
                body: "# 分享\n\n转写正文。\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        v.set_source_url(
            "Transcripts/分享.md",
            "  https://www.youtube.com/watch?v=abc  ",
        )
        .unwrap();
        let doc = v.read("Transcripts/分享.md").unwrap();
        assert_eq!(
            doc.meta.source_url.as_deref(),
            Some("https://www.youtube.com/watch?v=abc"),
            "去空白后原样保留（查询串是视频身份）"
        );
        assert!(doc.content.contains("转写正文。"), "正文不受影响");

        // 空串 = 清除（本地文件导入的笔记恒无此字段，故必须能回到"没有来源"）。
        v.set_source_url("Transcripts/分享.md", "   ").unwrap();
        assert!(v
            .read("Transcripts/分享.md")
            .unwrap()
            .meta
            .source_url
            .is_none());
    }

    #[test]
    fn update_playback_cue_rejects_empty_or_unknown_cue() {
        let (_d, v) = open_vault();
        v.create(
            "录音/拒绝.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 拒绝\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let segments = vec![seg(0, 1500, "原始第一段")];
        let parts = vec![RecordingPart {
            path: "Recordings/reject-001.wav".into(),
            start_ms: 0,
            end_ms: 1500,
            duration: Some(1.5),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/拒绝.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        assert!(matches!(
            v.update_playback_cue("录音/拒绝.md", 0, 1500, " ", None),
            Err(VaultError::InvalidCue(_))
        ));
        assert!(matches!(
            v.update_playback_cue("录音/拒绝.md", 1500, 3000, "不存在", None),
            Err(VaultError::InvalidCue(_))
        ));
    }

    #[test]
    fn finalize_recording_zero_segment_part_writes_honest_metadata() {
        // 纯录音/无语音分片：不写空边车，transcript=false、segments=None（诚实元数据）。
        let (_d, v) = open_vault();
        v.create(
            "录音/无语音.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 无语音\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let parts = vec![RecordingPart {
            path: "Recordings/silent-001.wav".into(),
            start_ms: 0,
            end_ms: 3000,
            duration: Some(3.0),
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/无语音.md",
            &parts,
            &[],
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let doc = v.read("录音/无语音.md").unwrap();
        let mr = &doc.meta.media[0];
        assert!(!mr.transcript, "零段分片 transcript 应为 false");
        assert!(mr.segments.is_none(), "零段分片不应登记边车路径");
        // 边车文件不应存在。
        let sidecar_abs = _d.path().join(".luna/sidecars/silent-001.segments.json");
        assert!(!sidecar_abs.exists(), "零段分片不应写空边车文件");
    }

    #[test]
    fn finalize_recording_multi_part_intervals_consistent_with_sidecars() {
        let (_d, v) = open_vault();
        v.create(
            "录音/长会.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 长会\n".into(),
                ..Default::default()
            },
        )
        .unwrap();

        // 6 段分布在两个分片：[0,5000) 和 [5000,10000)。
        let segments = vec![
            seg(0, 1000, "A1"),
            seg(1000, 2500, "A2"),
            seg(2500, 4800, "A3"),
            seg(5000, 6200, "B1"),
            seg(6200, 8000, "B2"),
            seg(8000, 9500, "B3"),
        ];
        let parts = vec![
            RecordingPart {
                path: "Recordings/long-001.wav".into(),
                start_ms: 0,
                end_ms: 5000,
                duration: Some(5.0),
                model: None,
                lang: None,
                tracks: None,
            },
            RecordingPart {
                path: "Recordings/long-002.wav".into(),
                start_ms: 5000,
                end_ms: 10000,
                duration: Some(5.0),
                model: None,
                lang: None,
                tracks: None,
            },
        ];
        v.finalize_recording(
            "录音/长会.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let meta = v.read("录音/长会.md").unwrap().meta;
        assert_eq!(meta.media.len(), 2);

        // 分片 1。
        let p1 = &meta.media[0];
        assert_eq!(p1.part_start_ms, Some(0), "分片1起点");
        assert_eq!(p1.part_end_ms, Some(5000), "分片1终点");
        let sc1_abs = _d.path().join(p1.segments.as_ref().unwrap());
        match read_sidecar(&sc1_abs) {
            SidecarLoad::Ok(sc) => {
                assert_eq!(sc.segments.len(), 3, "分片1应含3段");
                for s in &sc.segments {
                    assert!(
                        s.start.as_millis() < 5000,
                        "分片1段应在 [0,5000) 内：{:?}",
                        s.start
                    );
                }
            }
            other => panic!("分片1边车读取失败：{other:?}"),
        }

        // 分片 2。
        let p2 = &meta.media[1];
        assert_eq!(p2.part_start_ms, Some(5000), "分片2起点");
        assert_eq!(p2.part_end_ms, Some(10000), "分片2终点");
        let sc2_abs = _d.path().join(p2.segments.as_ref().unwrap());
        match read_sidecar(&sc2_abs) {
            SidecarLoad::Ok(sc) => {
                assert_eq!(sc.segments.len(), 3, "分片2应含3段");
                for s in &sc.segments {
                    assert!(
                        s.start.as_millis() >= 5000,
                        "分片2段应在 [5000,10000) 内：{:?}",
                        s.start
                    );
                }
            }
            other => panic!("分片2边车读取失败：{other:?}"),
        }
    }

    #[test]
    fn finalize_recording_sidecar_survives_clear_cache() {
        // 回归：清缓存不得删耐久边车（TDD-06 §2.1 durable 约定）。
        let (_d, v) = open_vault();
        v.create(
            "录音/回归.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 回归".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let segments = vec![seg(0, 500, "你好")];
        let parts = vec![RecordingPart {
            path: "Recordings/regression.wav".into(),
            start_ms: 0,
            end_ms: 1000,
            duration: None,
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/回归.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let sidecar_rel = v.read("录音/回归.md").unwrap().meta.media[0]
            .segments
            .clone()
            .expect("边车路径应存在");
        let sidecar_abs = _d.path().join(&sidecar_rel);
        assert!(sidecar_abs.exists(), "清缓存前边车应存在");

        v.clear_cache().unwrap();

        assert!(
            sidecar_abs.exists(),
            "清缓存后边车应仍存在（durable，不可重建）"
        );
        // 边车内容完好。
        assert!(matches!(read_sidecar(&sidecar_abs), SidecarLoad::Ok(_)));
    }

    #[test]
    fn finalize_recording_writes_bilingual_body_and_sidecar_translation() {
        // 阶段②（TDD-08）：有译文段 → 正文源行下写 `> 译文` 引用行（无时间标记）、
        // 边车段携带 translation、MediaRef.translation 翻正。
        let (_d, v) = open_vault();
        v.create(
            "录音/双语.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 双语".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut translated = seg(0, 1000, "大家好。");
        translated.translation = Some("Hello everyone.".into());
        let segments = vec![translated, seg(1000, 2000, "未译尾句")];
        let parts = vec![RecordingPart {
            path: "Recordings/bilingual.wav".into(),
            start_ms: 0,
            end_ms: 2000,
            duration: None,
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/双语.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let doc = v.read("录音/双语.md").unwrap();
        assert!(
            doc.content.contains("大家好。\n> Hello everyone.\n"),
            "译文引用行应紧随源文行：{}",
            doc.content
        );
        assert!(
            !doc.content.contains("> \n"),
            "未译段不得产生空引用行：{}",
            doc.content
        );
        assert!(doc.meta.media[0].translation, "MediaRef.translation 应翻正");

        let sidecar_abs = _d.path().join(doc.meta.media[0].segments.as_ref().unwrap());
        match read_sidecar(&sidecar_abs) {
            SidecarLoad::Ok(sc) => {
                assert_eq!(
                    sc.segments[0].translation.as_deref(),
                    Some("Hello everyone."),
                    "边车应携带译文（双语回放精确层）"
                );
                assert_eq!(sc.segments[1].translation, None, "未译段边车留 None");
            }
            other => panic!("边车读取失败：{other:?}"),
        }
    }

    #[test]
    fn finalize_recording_noop_translation_never_writes_pending() {
        // 诚实元数据：translation=None 时绝不写 "translation: pending" 或假译文。
        let (_d, v) = open_vault();
        v.create(
            "录音/翻译.md",
            NoteType::Recording,
            &TemplateVars {
                body: "# 翻译测试".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let segments = vec![seg(0, 1000, "Hello World")];
        let parts = vec![RecordingPart {
            path: "Recordings/trans.wav".into(),
            start_ms: 0,
            end_ms: 2000,
            duration: None,
            model: None,
            lang: None,
            tracks: None,
        }];
        v.finalize_recording(
            "录音/翻译.md",
            &parts,
            &segments,
            &[],
            TranscriptAuthority::Preview,
        )
        .unwrap();

        let content = v.read("录音/翻译.md").unwrap().content;
        assert!(
            !content.contains("pending"),
            "禁止写入 translation: pending：{content}"
        );
        assert!(
            !content.contains("translation: "),
            "禁止写假译文字段：{content}"
        );
    }

    #[test]
    fn scan_indexes_existing_files_without_modifying_them() {
        // §4.1 只读接入：扫描既有文件后字节不变。
        let dir = tempfile::tempdir().unwrap();
        let original = "---\ntitle: 既有\ntags: [obsidian]\n---\n既有正文内容讨论";
        std::fs::write(dir.path().join("existing.md"), original).unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let n = v.scan().unwrap();
        assert_eq!(n, 1);
        // 字节逐字不变（互操作安全契约）。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("existing.md")).unwrap(),
            original
        );
        assert_eq!(v.search(&Query::text("既有正文内容")).unwrap().len(), 1);
    }

    #[test]
    fn wikilinks_resolve_into_subfolders_and_land_on_the_fragment() {
        let (_d, v) = open_vault();
        v.write("项目/周报.md", "# 总览\n\n## 下周计划\n\n要点 ^k1\n")
            .unwrap();
        v.write("会议.md", "见 [[周报#下周计划]]").unwrap();
        let resolved = v
            .resolve_wikilinks(
                "会议.md",
                &[
                    "周报#下周计划".into(),
                    "项目/周报".into(),
                    "#总览".into(),
                    "缺".into(),
                ],
            )
            .unwrap();
        assert_eq!(
            resolved,
            vec![
                Some("项目/周报.md".to_string()),
                Some("项目/周报.md".to_string()),
                Some("会议.md".to_string()),
                None
            ]
        );
        let hit = v.resolve_wikilink("会议.md", "周报#下周计划").unwrap();
        assert_eq!(
            hit,
            WikiLinkHit {
                path: "项目/周报.md".into(),
                exists: true,
                line: Some(3),
                heading_index: Some(1),
            }
        );
        assert_eq!(
            v.resolve_wikilink("会议.md", "周报#^k1").unwrap().line,
            Some(5)
        );
        let missing = v.resolve_wikilink("会议.md", "新目录/想法#一").unwrap();
        assert_eq!(
            (missing.path.as_str(), missing.exists),
            ("新目录/想法.md", false)
        );
        v.write("周报.md", "根目录那篇").unwrap();
        let candidates = v.wikilink_candidates("会议.md", "周", 10).unwrap();
        let inserts: Vec<_> = candidates
            .iter()
            .map(|c| (c.path.as_str(), c.insert.as_str()))
            .collect();
        assert_eq!(
            inserts,
            vec![("周报.md", "周报"), ("项目/周报.md", "项目/周报")]
        );
        assert_eq!(candidates[1].folder, "项目");
        // 反链与图谱用的是同一份解析：带片段的链接也算指向那一篇。
        assert_eq!(
            v.backlinks("项目/周报.md").unwrap(),
            vec!["会议.md".to_string()]
        );
    }

    /// macOS 列目录给的是文件落盘时的形态：一篇以 NFD 存下的「Café 계획」，用户用输入法敲出的
    /// 是 NFC——两者按字节不同。解析、反链、补全与图谱都要认出它（ADR-148 ⑥）。
    #[test]
    fn nfd_file_names_are_found_by_nfc_links_backlinks_completion_and_graph() {
        let (_d, v) = open_vault();
        let nfd_title = icu_normalizer::DecomposingNormalizerBorrowed::new_nfd()
            .normalize("Café 계획")
            .into_owned();
        let nfd_path = format!("项目/{nfd_title}.md");
        assert_ne!(nfd_title, "Café 계획", "夹具必须真的是 NFD");
        v.write(&nfd_path, "# 计划\n").unwrap();
        v.write("日记.md", "今天看了[[Café 계획]]。").unwrap();

        assert_eq!(
            v.resolve_wikilinks("日记.md", &["café 계획".into()])
                .unwrap(),
            vec![Some(nfd_path.clone())]
        );
        assert_eq!(v.backlinks(&nfd_path).unwrap(), vec!["日记.md".to_string()]);
        let candidates = v.wikilink_candidates("日记.md", "Café", 10).unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            vec![nfd_path.as_str()]
        );
        let graph = v.graph().unwrap();
        assert!(
            graph
                .nodes
                .iter()
                .all(|node| !node.id.starts_with("ghost:")),
            "NFC 链接不应把 NFD 文件画成幽灵节点"
        );
    }

    #[test]
    fn aliases_take_part_in_resolution_backlinks_graph_and_completion() {
        let (_d, v) = open_vault();
        v.write(
            "项目/周会纪要.md",
            "---\naliases: [例会, Weekly]\n---\n# 周会纪要\n\n## 结论\n",
        )
        .unwrap();
        v.write("日记.md", "今天开了[[例会#结论]]。").unwrap();
        assert_eq!(
            v.resolve_wikilinks("日记.md", &["例会".into(), "weekly".into()])
                .unwrap(),
            vec![
                Some("项目/周会纪要.md".to_string()),
                Some("项目/周会纪要.md".to_string())
            ]
        );
        assert_eq!(
            v.resolve_wikilink("日记.md", "例会#结论")
                .unwrap()
                .heading_index,
            Some(1)
        );
        assert_eq!(
            v.backlinks("项目/周会纪要.md").unwrap(),
            vec!["日记.md".to_string()]
        );
        let graph = v.graph().unwrap();
        assert!(
            graph
                .nodes
                .iter()
                .all(|n| n.kind != crate::graph::GraphNodeKind::Ghost),
            "按别名写的链接不是幽灵：{:?}",
            graph.nodes
        );
        let candidates = v.wikilink_candidates("日记.md", "例", 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].insert, "周会纪要");
        assert_eq!(candidates[0].alias.as_deref(), Some("例会"));
        // 改掉别名，旧链接随即解析不到（索引随写盘更新）。
        v.write("项目/周会纪要.md", "# 周会纪要\n").unwrap();
        assert_eq!(
            v.resolve_wikilinks("日记.md", &["例会".into()]).unwrap(),
            vec![None]
        );
    }

    #[test]
    fn embeds_resolve_notes_sections_and_files() {
        use crate::embed::EmbedHit;
        let (_d, v) = open_vault();
        v.write(
            "项目/周会.md",
            "---\naliases: [例会]\n---\n# 周会\n\n## 结论\n\n![图](assets/图.png)\n\n## 下周\n",
        )
        .unwrap();
        std::fs::create_dir_all(v.root().join("项目/assets")).unwrap();
        std::fs::write(v.root().join("项目/assets/图.png"), b"png").unwrap();
        std::fs::write(v.root().join("项目/报告.pdf"), b"pdf").unwrap();
        v.write("日记/今天.md", "![[例会#结论]]").unwrap();
        let hits = v
            .resolve_embeds(
                "日记/今天.md",
                &[
                    "例会#结论".into(),
                    "图.png".into(),
                    "报告.pdf".into(),
                    "没有".into(),
                ],
            )
            .unwrap();
        assert_eq!(
            hits,
            vec![
                EmbedHit::Note {
                    path: "项目/周会.md".into(),
                    content: "## 结论\n\n![图](../项目/assets/图.png)".into(),
                },
                EmbedHit::File {
                    path: "项目/assets/图.png".into(),
                    link: "../项目/assets/图.png".into(),
                    image: true,
                },
                EmbedHit::File {
                    path: "项目/报告.pdf".into(),
                    link: "../项目/报告.pdf".into(),
                    image: false,
                },
                EmbedHit::Missing,
            ]
        );
        // 嵌入也是一条指向那一篇的链接：反链认得。
        assert_eq!(
            v.backlinks("项目/周会.md").unwrap(),
            vec!["日记/今天.md".to_string()]
        );
    }

    #[test]
    fn tree_skips_dot_dirs() {
        let (_d, v) = open_vault();
        v.write("a.md", "x").unwrap();
        v.write("子目录/b.md", "y").unwrap();
        let tree = v.tree().unwrap();
        // 根含 a.md 与 子目录，不含 .luna/.gitignore。
        let names: Vec<_> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"a.md"));
        assert!(names.contains(&"子目录"));
        assert!(!names.iter().any(|n| n.starts_with('.')));
    }

    #[test]
    fn tree_includes_non_md_attachments() {
        // 附件（图片等非 md 文件）须出现在文件树（Obsidian 对标，PRD-06-007）——
        // 否则 assets 文件夹显示为空、点不到附件预览。
        let (d, v) = open_vault();
        v.write("Chats/a.md", "x").unwrap();
        std::fs::create_dir_all(d.path().join("Chats/assets")).unwrap();
        std::fs::write(d.path().join("Chats/assets/pic.png"), b"png").unwrap();
        let tree = v.tree().unwrap();
        let chats = tree.children.iter().find(|c| c.name == "Chats").unwrap();
        let assets = chats.children.iter().find(|c| c.name == "assets").unwrap();
        assert!(
            assets
                .children
                .iter()
                .any(|c| c.name == "pic.png" && !c.is_dir),
            "assets 目录应含附件 pic.png"
        );
    }

    #[test]
    fn move_rewrites_references_two_link_types() {
        let (_d, v) = open_vault();
        v.write("a.md", "see [[b]] [x](b.md)").unwrap();
        v.write("b.md", "old b body").unwrap();
        v.move_("b.md", "b2.md").unwrap();
        let a_after = v.read("a.md").unwrap().content;
        assert!(
            a_after.contains("[[b2]]"),
            "wikilink should be rewritten: {a_after}"
        );
        assert!(
            a_after.contains("[x](b2.md)"),
            "md link should be rewritten: {a_after}"
        );
        assert!(!a_after.contains("[[b]]"), "old wikilink should not remain");
    }

    #[test]
    fn moving_a_claim_source_preserves_clean_topic_baseline_but_not_user_edits() {
        let (_dir, vault) = open_vault();
        let source_path = "Notes/会议.md";
        vault
            .write(source_path, "# 会议\n\n已完成原型。\n")
            .unwrap();
        let claim_id = uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap();
        let locator = crate::EvidenceLocator {
            heading_path: vec!["会议".into()],
            quote: "已完成原型".into(),
            context: String::new(),
            line_hint: Some(3),
            time_start_ms: None,
            time_end_ms: None,
        };
        let (block, claim, citations) = crate::render_generated_claim(
            claim_id,
            "原型已完成",
            crate::ClaimKind::Synthesis,
            "Knowledge/项目.md",
            &[crate::ClaimSource {
                path: source_path,
                title: "会议",
                source_hash: "source-v1",
                locator,
                complete_input: true,
            }],
        )
        .unwrap();
        let body = format!("# 项目\n\n{block}\n");
        let mut knowledge = crate::KnowledgeMeta::new(crate::TopicScope::from_tag("项目"));
        knowledge.claims.insert(claim_id.to_string(), claim);
        knowledge.citations = citations;
        for path in ["Knowledge/项目.md", "Knowledge/人工修改.md"] {
            let content = vault.topic_content(path, &body, Some("项目")).unwrap();
            let mut meta = crate::read_luna(&content);
            meta.knowledge = Some(serde_yaml_ng::to_value(&knowledge).unwrap());
            vault
                .write(path, &crate::update_luna(&content, &meta).unwrap())
                .unwrap();
        }
        let before_legacy_write = vault.read("Knowledge/项目.md").unwrap().content;
        assert!(vault
            .topic_content("Knowledge/项目.md", "# 替换整页", None)
            .is_err());
        assert_eq!(
            vault.read("Knowledge/项目.md").unwrap().content,
            before_legacy_write
        );
        let edited = vault.read("Knowledge/人工修改.md").unwrap().content;
        vault
            .write(
                "Knowledge/人工修改.md",
                &edited.replace("原型已完成", "原型仍在进行"),
            )
            .unwrap();
        assert_eq!(
            vault.topic_ownership("Knowledge/人工修改.md"),
            crate::TopicOwnership::UserOwned
        );

        vault.move_(source_path, "Archive/会议.md").unwrap();
        assert!(vault
            .index
            .topic_citation_dependents(source_path, None, 100)
            .unwrap()
            .is_empty());
        assert_eq!(
            vault
                .index
                .topic_citation_dependents("Archive/会议.md", None, 100)
                .unwrap()
                .len(),
            2,
            "来源移动重写两篇主题的链接后，依赖投影必须跟随新路径"
        );
        let clean = vault.read("Knowledge/项目.md").unwrap().content;
        assert!(clean.contains("../Archive/会议.md"), "{clean}");
        assert!(crate::validate_topic_document(&clean).is_ok());
        assert_eq!(
            vault.topic_ownership("Knowledge/项目.md"),
            crate::TopicOwnership::AiOwned
        );
        let edited = vault.read("Knowledge/人工修改.md").unwrap().content;
        assert!(edited.contains("../Archive/会议.md"));
        assert_eq!(
            vault.topic_ownership("Knowledge/人工修改.md"),
            crate::TopicOwnership::UserOwned
        );
    }

    #[test]
    fn scoped_topic_creation_publishes_one_valid_file_without_overwriting_a_target() {
        let (dir, vault) = open_vault();
        vault
            .write("Notes/会议.md", "# 会议\n\n已完成原型。\n")
            .unwrap();
        let source_hash = vault.body_fingerprint("Notes/会议.md").unwrap();
        let source = crate::ClaimSource {
            path: "Notes/会议.md",
            title: "会议",
            source_hash: &source_hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["会议".into()],
                quote: "已完成原型".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        };
        let id = uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap();
        let sources = [source];
        let claim = crate::NewClaim {
            id,
            text: "原型已完成",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let rel = "Knowledge/项目.md";
        let content = vault
            .scoped_topic_content(
                rel,
                Some("项目"),
                crate::TopicScope::from_tag("项目"),
                &[claim],
            )
            .unwrap();
        assert_eq!(
            crate::validate_topic_document(&content)
                .unwrap()
                .claims
                .len(),
            1
        );
        vault.publish_scoped_topic(rel, &content).unwrap();
        assert_eq!(vault.read(rel).unwrap().content, content);
        assert_eq!(vault.topic_ownership(rel), crate::TopicOwnership::AiOwned);
        assert_eq!(
            vault.topic_maintenance_state(rel).unwrap(),
            crate::knowledge_evidence::TopicMaintenanceState::Ready
        );
        assert_eq!(vault.backlinks("Notes/会议.md").unwrap(), vec![rel]);
        assert!(matches!(
            vault.publish_scoped_topic(rel, &content),
            Err(VaultError::AlreadyExists(_))
        ));
        assert_eq!(vault.read(rel).unwrap().content, content);
        // 真实文件外部编辑：格式和正文指纹都必须由下一次读盘发现，不依赖索引刷新。
        let edited = content.replace("[会议](../Notes/会议.md)", "会议");
        std::fs::write(dir.path().join(rel), &edited).unwrap();
        assert_eq!(vault.read(rel).unwrap().content, edited);
        assert_eq!(
            crate::validate_topic_document(&edited),
            Err(crate::KnowledgeError::InvalidClaims)
        );
        assert_eq!(vault.topic_ownership(rel), crate::TopicOwnership::UserOwned);
    }

    #[test]
    fn scoped_topic_projection_recovers_after_derived_index_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault
            .write(
                "Notes/来源.md",
                "---\ntags: [项目]\n---\n# 来源\n\n原型完成。\n",
            )
            .unwrap();
        vault
            .write("Notes/空白.md", "---\ntags: [项目]\n---\n")
            .unwrap();
        let source_hash = vault.body_fingerprint("Notes/来源.md").unwrap();
        let sources = [crate::ClaimSource {
            path: "Notes/来源.md",
            title: "来源",
            source_hash: &source_hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["来源".into()],
                quote: "原型完成".into(),
                context: String::new(),
                line_hint: Some(6),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = crate::NewClaim {
            id: uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap(),
            text: "原型完成",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let claim_id = claim.id.to_string();
        let topic_rel = "Knowledge/项目.md";
        let topic_bytes = vault
            .scoped_topic_content(
                topic_rel,
                Some("项目"),
                crate::TopicScope::from_tag("项目"),
                &[claim],
            )
            .unwrap();
        vault.publish_scoped_topic(topic_rel, &topic_bytes).unwrap();
        let dependencies = vault
            .topic_citation_dependents("Notes/来源.md", None, 100)
            .unwrap();
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].topic_path, topic_rel);
        assert_eq!(dependencies[0].citation_id, claim_id + ":0");
        assert_eq!(
            vault.index.topic_tag_candidates(topic_rel).unwrap(),
            ["Notes/来源.md"]
        );
        drop(vault);

        std::fs::remove_file(root.join(".luna/index.sqlite")).unwrap();
        let reopened = Vault::open(root).unwrap();
        assert_eq!(reopened.scan().unwrap(), 3);
        assert_eq!(reopened.read(topic_rel).unwrap().content, topic_bytes);
        assert_eq!(
            reopened.topic_maintenance_state(topic_rel).unwrap(),
            crate::TopicMaintenanceState::Ready
        );
        let evidence = reopened.topic_evidence_states(topic_rel).unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].state, crate::EvidenceState::Current);
        assert_eq!(
            reopened
                .topic_citation_dependents("Notes/来源.md", None, 100)
                .unwrap(),
            dependencies,
            "逐条依赖必须由 Markdown 重建，不依赖旧 index.sqlite"
        );
        assert_eq!(
            reopened.index.topic_tag_candidates(topic_rel).unwrap(),
            ["Notes/来源.md"]
        );
    }

    #[test]
    fn topic_maintenance_blocks_metadata_edits_and_missing_baselines() {
        use crate::knowledge_evidence::{KnowledgeMeta, TopicMaintenanceState as State};
        let (dir, vault) = open_vault();
        vault
            .write("Notes/来源.md", "# 来源\n\n原型完成。\n")
            .unwrap();
        let hash = vault.body_fingerprint("Notes/来源.md").unwrap();
        let sources = [crate::ClaimSource {
            path: "Notes/来源.md",
            title: "来源",
            source_hash: &hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["来源".into()],
                quote: "原型完成".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = crate::NewClaim {
            id: uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap(),
            text: "原型完成",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let rel = "Knowledge/项目.md";
        let content = vault
            .scoped_topic_content(rel, None, crate::TopicScope::from_tag("项目"), &[claim])
            .unwrap();
        vault.publish_scoped_topic(rel, &content).unwrap();
        assert_eq!(vault.topic_maintenance_state(rel).unwrap(), State::Ready);

        let mut frontmatter = crate::read_luna(&content);
        let mut knowledge: KnowledgeMeta =
            serde_yaml_ng::from_value(frontmatter.knowledge.clone().unwrap()).unwrap();
        knowledge.scope.title = "用户修改的主题".into();
        frontmatter.knowledge = Some(serde_yaml_ng::to_value(knowledge).unwrap());
        let changed = crate::frontmatter::update_luna(&content, &frontmatter).unwrap();
        std::fs::write(dir.path().join(rel), &changed).unwrap();
        assert_eq!(vault.topic_ownership(rel), crate::TopicOwnership::AiOwned);
        assert_eq!(
            vault.topic_maintenance_state(rel).unwrap(),
            State::MetadataChanged
        );
        // 普通保存会把用户改过的 frontmatter 也留成历史快照；它不能被按正文 hash
        // 误选为新的系统底版，否则维护闸门会错误放行。
        vault.write(rel, &changed).unwrap();
        assert_eq!(
            vault.topic_maintenance_state(rel).unwrap(),
            State::MetadataChanged
        );

        std::fs::write(
            dir.path().join(rel),
            content.replace("原型完成", "用户正文修改"),
        )
        .unwrap();
        assert_eq!(
            vault.topic_maintenance_state(rel).unwrap(),
            State::BodyChanged
        );
        std::fs::write(dir.path().join(rel), &content).unwrap();
        assert_eq!(vault.topic_maintenance_state(rel).unwrap(), State::Ready);

        vault.write("Knowledge/无底版.md", &content).unwrap();
        assert_eq!(
            vault
                .topic_maintenance_state("Knowledge/无底版.md")
                .unwrap(),
            State::MissingBaseline
        );
        vault
            .write_topic("Knowledge/旧主题.md", "# 旧主题\n", None)
            .unwrap();
        assert_eq!(
            vault
                .topic_maintenance_state("Knowledge/旧主题.md")
                .unwrap(),
            State::NeedsUpgrade
        );
    }

    #[test]
    fn scoped_topic_rejects_changed_or_missing_source_before_rendering() {
        let (_dir, vault) = open_vault();
        vault
            .write("Notes/来源.md", "# 来源\n\n旧说法。\n")
            .unwrap();
        let old_hash = vault.body_fingerprint("Notes/来源.md").unwrap();
        let sources = [crate::ClaimSource {
            path: "Notes/来源.md",
            title: "来源",
            source_hash: &old_hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["来源".into()],
                quote: "旧说法".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = crate::NewClaim {
            id: uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap(),
            text: "旧说法",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let target = "Knowledge/主题.md";
        vault
            .write("Notes/来源.md", "# 来源\n\n新说法。\n")
            .unwrap();
        assert!(matches!(
            vault.scoped_topic_content(
                target,
                None,
                crate::TopicScope::from_tag("主题"),
                std::slice::from_ref(&claim),
            ),
            Err(VaultError::InvalidReplace(_))
        ));
        assert!(!vault.resolve(target).unwrap().exists());
        std::fs::remove_file(vault.resolve("Notes/来源.md").unwrap()).unwrap();
        assert!(matches!(
            vault.scoped_topic_content(
                target,
                None,
                crate::TopicScope::from_tag("主题"),
                std::slice::from_ref(&claim),
            ),
            Err(VaultError::NotFound(_))
        ));
        assert!(!vault.resolve(target).unwrap().exists());
    }

    #[test]
    fn evidence_states_follow_current_partial_changed_missing_and_ambiguous_sources() {
        use crate::knowledge_evidence::EvidenceState;
        let (dir, vault) = open_vault();
        let source_rel = "Notes/会议.md";
        let topic_rel = "Knowledge/项目.md";
        vault
            .write(source_rel, "# 会议\n\n原型已完成。\n\n预算已批准。\n")
            .unwrap();
        let hash = vault.body_fingerprint(source_rel).unwrap();
        let sources = [
            crate::ClaimSource {
                path: source_rel,
                title: "会议",
                source_hash: &hash,
                locator: crate::EvidenceLocator {
                    heading_path: vec!["会议".into()],
                    quote: "原型已完成".into(),
                    context: String::new(),
                    line_hint: Some(3),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: true,
            },
            crate::ClaimSource {
                path: source_rel,
                title: "会议",
                source_hash: &hash,
                locator: crate::EvidenceLocator {
                    heading_path: vec!["会议".into()],
                    quote: "预算已批准".into(),
                    context: String::new(),
                    line_hint: Some(5),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: false,
            },
        ];
        let id = uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap();
        let claim = crate::NewClaim {
            id,
            text: "原型与预算已确认",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let content = vault
            .scoped_topic_content(
                topic_rel,
                Some("项目"),
                crate::TopicScope::from_tag("项目"),
                &[claim],
            )
            .unwrap();
        assert_eq!(
            crate::knowledge_evidence::topic_citations(topic_rel, &content)
                .unwrap()
                .len(),
            2,
            "同源两条引用不能被图谱的链接去重折叠"
        );
        vault.publish_scoped_topic(topic_rel, &content).unwrap();
        let states = vault.topic_evidence_states(topic_rel).unwrap();
        assert_eq!(states[0].state, EvidenceState::Current);
        assert_eq!(states[1].state, EvidenceState::Partial);
        assert_eq!(states[0].source_path.as_deref(), Some(source_rel));
        let first = vault
            .index
            .topic_citation_dependents(source_rel, None, 1)
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = vault
            .index
            .topic_citation_dependents(
                source_rel,
                Some((&first[0].topic_path, &first[0].citation_id)),
                1,
            )
            .unwrap();
        assert_eq!(second.len(), 1, "同源重复出处不能在依赖索引中折叠");
        assert_ne!(first[0].citation_id, second[0].citation_id);
        assert!(vault
            .index
            .topic_citation_dependents(
                source_rel,
                Some((&second[0].topic_path, &second[0].citation_id)),
                1
            )
            .unwrap()
            .is_empty());

        std::fs::write(dir.path().join(source_rel), "# 会议\n\n结论已撤回。\n").unwrap();
        assert!(vault
            .topic_evidence_states(topic_rel)
            .unwrap()
            .iter()
            .all(|entry| entry.state == EvidenceState::SourceChanged));
        std::fs::remove_file(dir.path().join(source_rel)).unwrap();
        assert!(vault
            .topic_evidence_states(topic_rel)
            .unwrap()
            .iter()
            .all(|entry| entry.state == EvidenceState::Missing));
        vault.scan().unwrap();
        assert_eq!(
            vault
                .index
                .topic_citation_dependents(source_rel, None, 100)
                .unwrap()
                .len(),
            2,
            "来源删除后仍须定位受影响的声明"
        );

        let damaged = content.replace("../Notes/会议.md", "../../../Notes/会议.md");
        std::fs::write(dir.path().join(topic_rel), damaged).unwrap();
        assert!(vault
            .topic_evidence_states(topic_rel)
            .unwrap()
            .iter()
            .all(|entry| entry.state == EvidenceState::Ambiguous));
        vault.scan().unwrap();
        assert!(
            vault
                .index
                .topic_citation_dependents(source_rel, None, 100)
                .unwrap()
                .is_empty(),
            "坏出处不能在重扫后留下看似可用的依赖"
        );
    }

    #[test]
    fn scoped_topic_publish_rechecks_source_after_preview() {
        let (_dir, vault) = open_vault();
        vault.write("Notes/源.md", "# 源\n\n最初结论。\n").unwrap();
        let hash = vault.body_fingerprint("Notes/源.md").unwrap();
        let sources = [crate::ClaimSource {
            path: "Notes/源.md",
            title: "源",
            source_hash: &hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["源".into()],
                quote: "最初结论".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = crate::NewClaim {
            id: uuid::Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap(),
            text: "最初结论",
            kind: crate::ClaimKind::Synthesis,
            sources: &sources,
        };
        let target = "Knowledge/项目.md";
        let preview = vault
            .scoped_topic_content(target, None, crate::TopicScope::from_tag("项目"), &[claim])
            .unwrap();
        vault.write("Notes/源.md", "# 源\n\n结论已变。\n").unwrap();
        assert!(matches!(
            vault.publish_scoped_topic(target, &preview),
            Err(VaultError::InvalidReplace(_))
        ));
        assert!(!vault.resolve(target).unwrap().exists());
    }

    #[test]
    fn legacy_topic_writer_does_not_overwrite_unreadable_metadata() {
        let (_dir, vault) = open_vault();
        let malformed = "---\nluna:\n  schema: 1\n  topic: true\n  knowledge: [\n---\n# 用户内容\n";
        vault.write("Knowledge/损坏.md", malformed).unwrap();
        assert!(vault
            .write_topic("Knowledge/损坏.md", "# 系统覆盖", None)
            .is_err());
        assert_eq!(vault.read("Knowledge/损坏.md").unwrap().content, malformed);
        // 明确的旧版转主题入口仍可将有效普通笔记转为主题（既有行为）。
        vault.write("Notes/普通.md", "# 用户笔记\n").unwrap();
        vault
            .write_topic("Notes/普通.md", "# 用户笔记\n", Some("项目"))
            .unwrap();
        assert!(vault.read("Notes/普通.md").unwrap().meta.topic);
    }

    #[test]
    fn source_move_does_not_rewrite_unknown_knowledge_schema() {
        let (_dir, vault) = open_vault();
        vault.write("Notes/来源.md", "原文").unwrap();
        let future = "---\nluna:\n  schema: 1\n  type: note\n  topic: true\n  knowledge:\n    schema: 99\n    scope:\n      title: 主题\n      description: ''\n      includes: [主题]\n      excludes: []\n    claims: {}\n    citations: {}\n---\n# 主题\n\n<!-- luna:claim:019d1a08-7cf3-72d7-90fa-382ef03524ad -->\n- 结论（[来源](../Notes/来源.md)）\n<!-- /luna:claim -->\n";
        vault.write("Knowledge/未来版本.md", future).unwrap();
        vault.move_("Notes/来源.md", "Archive/来源.md").unwrap();
        assert_eq!(vault.read("Knowledge/未来版本.md").unwrap().content, future);
        assert_eq!(
            crate::validate_topic_document(future),
            Err(crate::KnowledgeError::UnknownSchema(99))
        );
        assert_eq!(
            vault
                .topic_maintenance_state("Knowledge/未来版本.md")
                .unwrap(),
            crate::TopicMaintenanceState::UnknownSchema
        );
    }

    #[test]
    fn sidecar_hash_named_no_collision_for_same_stem_different_dirs() {
        let (_d, v) = open_vault();
        // Two media files with same stem "meeting.wav" but in different directories
        let p1 = vec![RecordingPart {
            path: "Recordings/2024/meeting.wav".into(),
            start_ms: 0,
            end_ms: 1000,
            duration: None,
            model: None,
            lang: None,
            tracks: None,
        }];
        let p2 = vec![RecordingPart {
            path: "另一/Recordings/meeting.wav".into(),
            start_ms: 0,
            end_ms: 1000,
            duration: None,
            model: None,
            lang: None,
            tracks: None,
        }];
        let seg = vec![seg(0, 500, "hi")];

        v.create("a.md", NoteType::Recording, &TemplateVars::default())
            .unwrap();
        v.create("b.md", NoteType::Recording, &TemplateVars::default())
            .unwrap();
        v.finalize_recording("a.md", &p1, &seg, &[], TranscriptAuthority::Preview)
            .unwrap();
        v.finalize_recording("b.md", &p2, &seg, &[], TranscriptAuthority::Preview)
            .unwrap();

        let m1 = v.read("a.md").unwrap().meta.media[0]
            .segments
            .as_ref()
            .unwrap()
            .clone();
        let m2 = v.read("b.md").unwrap().meta.media[0]
            .segments
            .as_ref()
            .unwrap()
            .clone();
        assert_ne!(
            m1, m2,
            "same stem different dirs must produce different sidecar paths"
        );
    }

    #[test]
    fn search_matches_locates_lines_and_honours_filters() {
        // PRD-06-031：索引粗筛 → 读文件精确定位；`path:`/`-` 过滤真的收敛结果集。
        let (_dir, v) = open_vault();
        v.write(
            "项目/计划.md",
            "---\ntags:\n  - 工作\n---\n季度目标待定\n再提一次季度目标\n",
        )
        .unwrap();
        v.write("归档/旧计划.md", "季度目标已完成\n").unwrap();

        let all = v
            .search_matches(&SearchQuery::parse("季度目标", false))
            .unwrap();
        assert_eq!(all.notes.len(), 2);
        assert_eq!(all.total_matches, 3);

        let scoped = v
            .search_matches(&SearchQuery::parse("季度目标 path:项目", false))
            .unwrap();
        assert_eq!(scoped.notes.len(), 1);
        assert_eq!(scoped.notes[0].path, "项目/计划.md");
        assert_eq!(scoped.notes[0].match_count, 2);
        assert_eq!(
            scoped.notes[0]
                .lines
                .iter()
                .map(|l| l.line)
                .collect::<Vec<_>>(),
            vec![5, 6],
            "行号相对整篇文件（含 frontmatter 三行）"
        );

        let tagged = v
            .search_matches(&SearchQuery::parse("季度目标 tag:工作", false))
            .unwrap();
        assert_eq!(tagged.notes.len(), 1);

        let excluded = v
            .search_matches(&SearchQuery::parse("季度目标 -已完成", false))
            .unwrap();
        assert_eq!(excluded.notes.len(), 1);
        assert_eq!(excluded.notes[0].path, "项目/计划.md");
    }

    /// 保存与速记捕获一落盘就可检索、可打开（K3 / PRD-16 §4 E01/E02）：关键词检索只走
    /// Vault 自己的索引，不经嵌入或任何模型——它们不可用时这条路照样通。速记标为待整理。
    #[test]
    fn saved_notes_and_quick_captures_are_searchable_without_any_ai() {
        let (_dir, v) = open_vault();
        v.write("Notes/周会.md", "# 周会\n\n批量导入排到十二月。\n")
            .unwrap();
        v.write(
            "QuickNotes/随手记.md",
            "---\nluna:\n  schema: 1\n  type: quick-note\n---\n客户要批量导入\n",
        )
        .unwrap();
        let hits = v
            .search_matches(&SearchQuery::parse("批量导入", false))
            .unwrap();
        let mut paths: Vec<(&str, bool)> = hits
            .notes
            .iter()
            .map(|note| (note.path.as_str(), note.draft))
            .collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![("Notes/周会.md", false), ("QuickNotes/随手记.md", true)],
            "两篇都立刻可检索，速记标为待整理"
        );
        assert!(v
            .read("QuickNotes/随手记.md")
            .unwrap()
            .content
            .contains("客户要批量导入"));
    }

    /// AI 关系标签全库忽略与恢复（PRD-16-051）：只影响派生索引——纯 AI 打上的那篇失去这个
    /// 标签，用户自己写的同名标签不受影响；笔记文件一字不动；重开库、删索引重建后仍被忽略；
    /// 恢复后原样回来。
    #[test]
    fn ignoring_an_ai_tag_hides_it_everywhere_without_touching_files() {
        let (dir, v) = open_vault();
        let ai_only = "---\nluna:\n  schema: 1\n  ai_tags: [网页端, 性能]\n---\n# 甲\n\n正文。\n";
        let user_too =
            "---\ntags: [网页端]\nluna:\n  schema: 1\n  ai_tags: [网页端]\n---\n# 乙\n\n正文。\n";
        v.write("Notes/甲.md", ai_only).unwrap();
        v.write("Notes/乙.md", user_too).unwrap();
        let tags_of = |v: &Vault, path: &str| v.index.note_tags(path).unwrap();
        assert!(tags_of(&v, "Notes/甲.md").iter().any(|t| t == "网页端"));

        v.set_ai_tag_ignored("网页端", true).unwrap();
        assert!(
            !tags_of(&v, "Notes/甲.md").iter().any(|t| t == "网页端"),
            "纯 AI 的被忽略"
        );
        assert!(
            tags_of(&v, "Notes/甲.md").iter().any(|t| t == "性能"),
            "别的 AI 标签不受影响"
        );
        assert!(
            tags_of(&v, "Notes/乙.md").iter().any(|t| t == "网页端"),
            "用户写的同名标签不受影响"
        );
        assert_eq!(
            v.read("Notes/甲.md").unwrap().content,
            ai_only,
            "文件一字不动"
        );
        assert_eq!(v.ignored_ai_tags().unwrap(), vec![("网页端".to_owned(), 1)]);

        // 重开库、删掉派生索引重建：忽略是随库的权威决定，不复活。
        drop(v);
        std::fs::remove_file(dir.path().join(".luna/index.sqlite")).unwrap();
        let reopened = Vault::open(dir.path()).unwrap();
        reopened.scan().unwrap();
        assert!(!tags_of(&reopened, "Notes/甲.md")
            .iter()
            .any(|t| t == "网页端"));

        reopened.set_ai_tag_ignored("网页端", false).unwrap();
        assert!(
            tags_of(&reopened, "Notes/甲.md")
                .iter()
                .any(|t| t == "网页端"),
            "恢复后回来"
        );
        assert!(reopened.ignored_ai_tags().unwrap().is_empty());
    }

    /// 主题建议报精确的知识笔记篇数（PRD-16-051「计数注明统计对象」）：截断清单只到门槛
    /// 四倍，不能当篇数说；速记与待整理不算知识笔记。
    #[test]
    fn topic_suggestions_report_the_exact_knowledge_note_count() {
        let (_dir, v) = open_vault();
        for index in 0..15 {
            v.write(
                &format!("Notes/项目{index}.md"),
                &format!("---\ntags: [项目]\n---\n# 第 {index} 篇\n\n进展。\n"),
            )
            .unwrap();
        }
        v.write(
            "QuickNotes/速记.md",
            "---\ntags: [项目]\nluna:\n  schema: 1\n  type: quick-note\n---\n随手一句。\n",
        )
        .unwrap();
        v.write(
            "Notes/待整理.md",
            "---\ntags: [项目]\nluna:\n  schema: 1\n  stage: draft\n---\n草稿。\n",
        )
        .unwrap();
        let suggestion = v
            .topic_suggestions(&[])
            .unwrap()
            .into_iter()
            .find(|s| s.tag == "项目")
            .unwrap();
        assert_eq!(suggestion.note_count, 15, "精确篇数，不含速记与待整理");
        assert_eq!(
            suggestion.notes.len(),
            TOPIC_SUGGESTION_MIN_NOTES * 4,
            "清单仍截断"
        );
    }

    #[test]
    fn replace_matches_previews_then_writes_and_snapshots() {
        // PRD-06-014：dry_run 不落盘；正式替换经 Vault::write 故自动留快照（PRD-06-040 协同）。
        let (_dir, v) = open_vault();
        v.write("a.md", "旧称呼在此\n").unwrap();
        v.write("b.md", "另一处旧称呼\n").unwrap();
        let query = SearchQuery::parse("旧称呼", false);

        let preview = v.replace_matches(&query, "新称呼", None, true).unwrap();
        assert!(!preview.applied);
        assert_eq!(preview.total_matches, 2);
        assert_eq!(
            v.read("a.md").unwrap().content,
            "旧称呼在此\n",
            "预览不写盘"
        );

        // 点名单篇：另一篇原样不动。
        let one = v
            .replace_matches(&query, "新称呼", Some(&["a.md".to_string()]), false)
            .unwrap();
        assert!(one.applied);
        assert_eq!(one.notes.len(), 1);
        assert_eq!(v.read("a.md").unwrap().content, "新称呼在此\n");
        assert_eq!(v.read("b.md").unwrap().content, "另一处旧称呼\n");
        assert!(
            !v.snapshots("a.md").is_empty(),
            "替换前必须留下可回滚的版本"
        );
    }

    #[test]
    fn replace_requires_exactly_one_search_term() {
        let (_dir, v) = open_vault();
        v.write("a.md", "甲 乙\n").unwrap();
        let err = v
            .replace_matches(&SearchQuery::parse("甲 乙", false), "丙", None, true)
            .unwrap_err();
        assert_eq!(err.code(), "VAULT_INVALID_REPLACE");
    }

    #[test]
    fn knowledge_intent_hash_survives_every_skeleton_writer() {
        // TDD-06 §3.7.1：知识意图在**写盘前**用候选正文算指纹，落盘校验读的却是
        // `body_of`（含 frontmatter 之后那个分隔空行）。两侧不归一到同一基准，每一个
        // 知识产物的生命周期任务都会当场判「正文已变」而整条链路一次都跑不起来。
        // 三个骨架写入面各验一次；反向验证：把 `knowledge_body_hash` 换回
        // `ContentHash::of(body)` 会让这三条断言全部变红。
        let (_dir, v) = open_vault();
        let body = "## 主题归纳\n\n第一段。\n\n第二段。";

        v.write_topic("topic.md", body, Some("交易计划")).unwrap();
        assert_eq!(
            v.knowledge_body_fingerprint("topic.md"),
            Some(crate::knowledge_body_hash(body)),
            "主题页初稿的意图指纹必须与落盘后一致"
        );

        v.write_knowledge_map("map.md", body).unwrap();
        assert_eq!(
            v.knowledge_body_fingerprint("map.md"),
            Some(crate::knowledge_body_hash(body)),
            "知识地图的意图指纹必须与落盘后一致"
        );

        seed_note(&v, "source", NoteType::Transcript, "原始逐字稿");
        v.write_distilled("source.md", "distilled.md", body, "meeting", false, None)
            .unwrap();
        assert_eq!(
            v.knowledge_body_fingerprint("distilled.md"),
            Some(crate::knowledge_body_hash(body)),
            "蒸馏产物的意图指纹必须与落盘后一致"
        );

        // 正文真的变了仍必须变指纹——归一化只吃首尾换行，不能顺手把守卫一起弱化。
        assert_ne!(
            crate::knowledge_body_hash(body),
            crate::knowledge_body_hash("## 主题归纳\n\n第一段。\n\n第三段。")
        );
        assert_eq!(
            crate::knowledge_body_hash(body),
            crate::knowledge_body_hash(&format!("\n{body}\n"))
        );
    }

    #[test]
    fn knowledge_map_refresh_is_a_no_op_once_written() {
        // 地图「已经是这份内容」必须成立，否则每次去抖刷新都重写同样的字节，
        // 而写入事件又排下一次刷新——真机上表现为地图每秒被重写一次的自激循环。
        let (_dir, v) = open_vault();
        seed_note(&v, "topic-a", NoteType::Note, "甲页正文。");
        v.write_topic("topic-a.md", "甲页正文。", Some("甲"))
            .unwrap();

        let body = v.knowledge_map_body("map.md", true).unwrap();
        v.write_knowledge_map("map.md", &body).unwrap();
        let after_first = std::fs::read_to_string(v.resolve("map.md").unwrap()).unwrap();

        let again = v.knowledge_map_body("map.md", true).unwrap();
        assert_eq!(
            v.knowledge_body_fingerprint("map.md"),
            Some(crate::knowledge_body_hash(&again)),
            "相同主题集合的第二次刷新必须被判为无变化"
        );
        v.write_knowledge_map("map.md", &again).unwrap();
        assert_eq!(
            std::fs::read_to_string(v.resolve("map.md").unwrap()).unwrap(),
            after_first,
            "相同输入的刷新必须字节稳定"
        );
    }

    #[test]
    fn snapshot_diff_compares_history_against_disk() {
        // PRD-06-041：版本对比拿的是"历史版本 vs 此刻磁盘"，不是两次快照。
        let (_dir, v) = open_vault();
        v.write("a.md", "第一行\n第二行\n").unwrap();
        v.write("a.md", "第一行\n改过的第二行\n").unwrap();
        let snap = v.snapshots("a.md");
        let diff = v.snapshot_diff("a.md", &snap[0].id).unwrap();
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert!(!diff.degraded);
    }
}
