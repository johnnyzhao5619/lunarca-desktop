//! Vault 检索域（PRD-06-031 搜索视图 / PRD-06-014 跨库查找替换 / PRD-06-041 版本对比）。
//!
//! 三条命令共一个心智：**语法解析、命中定位、差异计算全在 Rust**（ADR-003 前端零业务）。
//! 前端只负责把用户键入的一行字原样送过来、把回来的分段渲染成高亮——不解析 `tag:`、
//! 不算字符偏移、不做大小写折叠。这不是洁癖：偏移换算一旦分家（Rust 按字节、JS 按 UTF-16），
//! 高亮就会在 CJK 与 emoji 上错位，而替换会照着错位置改文件。
//!
//! 命中区间以**分段序列**（`text` + `hit`）下发而非下标对，正是为了让这条边界不可能被跨越。

use super::*;
use luna_sync_core::diff::{DiffKind, LineDiff};
use luna_sync_core::search::{MatchSegment, SearchQuery};

// ── DTO ─────────────────────────────────────────────────────────────────────

/// 命中行的一段。`hit=true` 即高亮段。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct MatchSegmentDto {
    pub text: String,
    pub hit: bool,
}

impl From<MatchSegment> for MatchSegmentDto {
    fn from(value: MatchSegment) -> Self {
        Self {
            text: value.text,
            hit: value.hit,
        }
    }
}

/// 一条命中行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct SearchLineDto {
    /// 行号（1-based，相对整篇文件）。
    pub line: u32,
    pub segments: Vec<MatchSegmentDto>,
}

/// 一篇笔记的命中。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct SearchNoteDto {
    pub path: String,
    pub title: String,
    pub modified_ms: f64,
    /// 笔记类型 kebab 键（`NoteType::key()`）。
    pub note_type: String,
    /// 是否属**资料区**（录音 / 转写 / 对话，ADR-024）：前端据此把命中折进「资料」分组
    /// 并降权呈现。判定在 Rust（`NoteType::is_source_material`）——前端零业务，
    /// 且与图谱、AI 标签白名单共用同一份类型语义，不在两处各写一遍「哪几种算资料」。
    pub source_material: bool,
    /// 是否**记录态**（ADR-086）：前端据此把命中折进独立的「记录」分组
    /// （组头 + 计数，与「资料」组同形态）。
    ///
    /// **不降权**——记录是用户之言，「我上周记了什么」与找知识笔记同等常见；分组只
    /// 回答「这是草稿」，不惩罚它的排序（PRD-06-052）。Rust 侧因此只下发事实、不动次序。
    pub draft: bool,
    /// 命中总数（不因 `lines` 截断而少报）。
    pub match_count: u32,
    pub lines: Vec<SearchLineDto>,
    pub lines_truncated: bool,
}

/// 一次搜索的结果集。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultSearchResultDto {
    pub notes: Vec<SearchNoteDto>,
    pub total_matches: u32,
    /// 结果不完整（候选集或结果数触顶），UI 须提示缩小范围。
    pub truncated: bool,
    /// 该查询是否可用于替换（= 恰好一个检索词）。前端据此启用替换区，不自行判语法。
    pub replaceable: bool,
}

/// 一行替换预览。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ReplaceLineDto {
    pub line: u32,
    pub before: Vec<MatchSegmentDto>,
    pub after: String,
}

/// 一篇笔记的替换计划。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ReplaceNoteDto {
    pub path: String,
    pub title: String,
    pub match_count: u32,
    pub lines: Vec<ReplaceLineDto>,
    pub lines_truncated: bool,
}

/// 一次跨库替换的计划/结果。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ReplacePlanDto {
    pub notes: Vec<ReplaceNoteDto>,
    pub total_matches: u32,
    /// 已写盘（`false` = 仅预览）。
    pub applied: bool,
    /// 写盘失败的笔记路径 + 原因（其余笔记照常完成）。
    pub failures: Vec<ReplaceFailureDto>,
    pub truncated: bool,
}

/// 一次写盘失败。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ReplaceFailureDto {
    pub path: String,
    pub error: String,
}

/// 一行差异。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct DiffLineDto {
    /// `equal` / `added` / `removed`。
    pub kind: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

/// 版本对比结果。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct LineDiffDto {
    pub lines: Vec<DiffLineDto>,
    pub added: u32,
    pub removed: u32,
    /// 差异区过大已降级为整段替换（对比粒度变粗）。
    pub degraded: bool,
}

impl From<LineDiff> for LineDiffDto {
    fn from(value: LineDiff) -> Self {
        Self {
            lines: value
                .lines
                .into_iter()
                .map(|line| DiffLineDto {
                    kind: match line.kind {
                        DiffKind::Equal => "equal",
                        DiffKind::Added => "added",
                        DiffKind::Removed => "removed",
                    }
                    .to_string(),
                    old_line: line.old_line,
                    new_line: line.new_line,
                    text: line.text,
                })
                .collect(),
            added: value.added,
            removed: value.removed,
            degraded: value.degraded,
        }
    }
}

// ── 命令 ────────────────────────────────────────────────────────────────────

/// 搜索视图检索（PRD-06-031）。`query` 是用户原样键入的一行；语法解析在 Rust。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_search(
    state: tauri::State<'_, AppState>,
    query: String,
    match_case: bool,
) -> Result<VaultSearchResultDto, String> {
    let parsed = SearchQuery::parse(&query, match_case);
    let replaceable = parsed.sole_positive_term().is_some();
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let results = vault.search_matches(&parsed).map_err(vault_err)?;
    Ok(VaultSearchResultDto {
        notes: results
            .notes
            .into_iter()
            .map(|note| SearchNoteDto {
                path: note.path,
                title: note.title,
                modified_ms: note.modified_ms as f64,
                // 未知类型串按缺省 `note` 计（与索引「无 luna.type 一律视为 note」同口径）。
                source_material: luna_vault::NoteType::from_key(&note.note_type)
                    .unwrap_or_default()
                    .is_source_material(),
                draft: note.draft,
                note_type: note.note_type,
                match_count: note.match_count,
                lines: note
                    .lines
                    .into_iter()
                    .map(|line| SearchLineDto {
                        line: line.line,
                        segments: line.segments.into_iter().map(Into::into).collect(),
                    })
                    .collect(),
                lines_truncated: note.lines_truncated,
            })
            .collect(),
        total_matches: results.total_matches,
        truncated: results.truncated,
        replaceable,
    })
}

/// 跨库查找替换（PRD-06-014）。`dry_run=true` 只出计划，`targets` 为空表示全部命中笔记。
///
/// 正式替换会广播 `vault://changed`，让已打开的编辑器/文件树按既有链路重载——替换是从
/// 应用外部改了正在编辑的文件，与外部编辑器改动同构，走同一条刷新路径（DRY）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_replace(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    query: String,
    replacement: String,
    match_case: bool,
    targets: Option<Vec<String>>,
    dry_run: bool,
) -> Result<ReplacePlanDto, String> {
    let parsed = SearchQuery::parse(&query, match_case);
    let plan = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .replace_matches(&parsed, &replacement, targets.as_deref(), dry_run)
            .map_err(vault_err)?
    };
    if plan.applied {
        for note in &plan.notes {
            emit_vault_changed(&app, &note.path, false);
        }
    }
    Ok(ReplacePlanDto {
        total_matches: plan.total_matches,
        applied: plan.applied,
        truncated: plan.truncated,
        failures: plan
            .failures
            .into_iter()
            .map(|(path, error)| ReplaceFailureDto { path, error })
            .collect(),
        notes: plan
            .notes
            .into_iter()
            .map(|note| ReplaceNoteDto {
                path: note.path,
                title: note.title,
                match_count: note.replacement.match_count,
                lines_truncated: note.replacement.lines_truncated,
                lines: note
                    .replacement
                    .lines
                    .into_iter()
                    .map(|line| ReplaceLineDto {
                        line: line.line,
                        before: line.before.into_iter().map(Into::into).collect(),
                        after: line.after,
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// 历史版本与当前磁盘内容的行级差异（PRD-06-041）。
#[tauri::command]
#[specta::specta]
pub(crate) fn snapshot_diff(
    state: tauri::State<'_, AppState>,
    path: String,
    id: String,
) -> Result<LineDiffDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .snapshot_diff(&path, &id)
        .map(Into::into)
        .map_err(vault_err)
}
