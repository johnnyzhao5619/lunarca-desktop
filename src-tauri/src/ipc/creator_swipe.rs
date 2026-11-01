//! 灵感库（ADR-162 ⑬，PRD-19-005）：按套路分组列出灵感，「归类套路」由 AI 提案、用户确认后
//! 写回顶层标签 `<套路>/<类名>`。
//!
//! 分组、摘句、标签枝的判定与改写全在 Rust（`luna_vault::swipe`）；模型只看得到序号，给不出
//! 不存在的笔记；前端只渲染与发命令（ADR-003）。

use super::*;
use luna_vault::swipe::{pattern_tag, set_pattern_tag, SwipeEntry, SwipeNames};
use luna_vault::tag_edit::NoteTagEdit;

/// 一次归类最多读多少条灵感（新的在前）。
const PATTERN_MAX_ENTRIES: usize = 200;
/// 少于这么多条不值得归类。
const PATTERN_MIN_ENTRIES: usize = 3;
/// 最多几类。
const PATTERN_MAX_GROUPS: usize = 8;
/// 每条喂给模型的片段 / 为什么好各取多少字。
const PATTERN_FIELD_CHARS: usize = 120;
/// 归类 Job 的输出预算。
const PATTERN_OUTPUT_TOKENS: u32 = 1024;

/// 灵感条目里各节的名字与套路前缀（随界面语言）。
struct SwipeCopy {
    snippet: String,
    why: String,
    source: String,
    prefix: String,
}

fn swipe_copy(copy: BackendCopy<'_>) -> SwipeCopy {
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    SwipeCopy {
        snippet: text("creator-swipe-snippet", "片段"),
        why: text("creator-swipe-why", "为什么好"),
        source: text("creator-swipe-source", "来源"),
        prefix: text("creator-swipe-pattern-prefix", "套路"),
    }
}

impl SwipeCopy {
    fn names(&self) -> SwipeNames<'_> {
        SwipeNames {
            snippet: &self.snippet,
            why: &self.why,
            prefix: &self.prefix,
        }
    }
}

/// 新建灵感条目的正文骨架：片段 / 为什么好 / 来源三节（ADR-162 ⑬）。
pub(crate) fn swipe_skeleton(copy: BackendCopy<'_>) -> String {
    let names = swipe_copy(copy);
    format!(
        "## {}\n\n\n\n## {}\n\n\n\n## {}\n\n",
        names.snippet, names.why, names.source
    )
}

/// 灵感库里的一条。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeItemDto {
    pub path: String,
    pub title: String,
    /// 修改时间（Unix 毫秒）。
    pub modified_ms: f64,
    /// 「为什么好」的第一句。
    pub why: Option<String>,
}

impl From<&SwipeEntry> for SwipeItemDto {
    fn from(entry: &SwipeEntry) -> Self {
        Self {
            path: entry.path.clone(),
            title: entry.title.clone(),
            #[allow(clippy::cast_precision_loss)]
            modified_ms: entry.modified_ms as f64,
            why: entry.why.clone(),
        }
    }
}

/// 一组：一个套路名下的灵感；`name` 为 `None` = 还没归类的。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeGroupDto {
    pub name: Option<String>,
    pub items: Vec<SwipeItemDto>,
}

/// 灵感库：已归类的按套路分组（条目多的在前），没归类的在最后；一条有几个套路就出现在几组。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeLibraryDto {
    pub groups: Vec<SwipeGroupDto>,
    pub total: u32,
}

fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 按套路分组（纯函数）：条目多的组在前，同样多按名字；没有套路的放最后一组。
fn group_by_pattern(entries: &[SwipeEntry]) -> Vec<SwipeGroupDto> {
    let mut named: Vec<(String, Vec<SwipeItemDto>)> = Vec::new();
    let mut loose: Vec<SwipeItemDto> = Vec::new();
    for entry in entries {
        if entry.patterns.is_empty() {
            loose.push(entry.into());
            continue;
        }
        for pattern in &entry.patterns {
            match named
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(pattern))
            {
                Some((_, items)) => items.push(entry.into()),
                None => named.push((pattern.clone(), vec![entry.into()])),
            }
        }
    }
    named.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut groups: Vec<SwipeGroupDto> = named
        .into_iter()
        .map(|(name, items)| SwipeGroupDto {
            name: Some(name),
            items,
        })
        .collect();
    if !loose.is_empty() {
        groups.push(SwipeGroupDto {
            name: None,
            items: loose,
        });
    }
    groups
}

fn load_swipes(
    app: &tauri::AppHandle,
    window_label: &str,
) -> Result<(Vec<SwipeEntry>, SwipeCopy), String> {
    let i18n = backend_copy_arc(app);
    let copy = swipe_copy(i18n.as_deref());
    let state = app.state::<AppState>();
    let entries = state.with_vault(window_label, |vault| {
        vault.swipe_entries(copy.names()).map_err(vault_err)
    })?;
    Ok((entries, copy))
}

/// 灵感库（PRD-19-005）：全部灵感按套路分组。
///
/// # Errors
/// 库未打开或索引查询失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_swipe_library(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<SwipeLibraryDto, String> {
    let (entries, _) = load_swipes(&app, window.label())?;
    Ok(SwipeLibraryDto {
        total: count_u32(entries.len()),
        groups: group_by_pattern(&entries),
    })
}

/// 喂给模型的一条：`[序号] 标题 | 片段 | 为什么好`（各字段截短；只给序号不给路径）。
fn pattern_line(number: usize, entry: &SwipeEntry) -> String {
    let cut = |text: &Option<String>| {
        text.as_deref()
            .unwrap_or("")
            .chars()
            .take(PATTERN_FIELD_CHARS)
            .collect::<String>()
    };
    format!(
        "[{number}] {} | {} | {}",
        entry.title,
        cut(&entry.snippet),
        cut(&entry.why)
    )
}

/// 在输入上限内能放下的前几条（新的在前），以及拼好的输入。
fn pattern_source(entries: &[SwipeEntry], limit_chars: usize) -> (usize, String) {
    let mut text = String::new();
    let mut taken = 0;
    for (index, entry) in entries.iter().take(PATTERN_MAX_ENTRIES).enumerate() {
        let line = pattern_line(index + 1, entry);
        if taken > 0 && text.chars().count() + line.chars().count() + 1 > limit_chars {
            break;
        }
        if taken > 0 {
            text.push('\n');
        }
        text.push_str(&line);
        taken += 1;
    }
    (taken, text)
}

/// 模型回的归类（ResultSchema）。
#[derive(Debug, serde::Deserialize)]
struct ModelGroups {
    #[serde(default)]
    groups: Vec<ModelGroup>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    members: Vec<serde_json::Value>,
}

/// 读模型输出：类名只取一行；序号越界、重复（一条只归一类，先到先得）的丢掉；空类丢掉；
/// 最多 [`PATTERN_MAX_GROUPS`] 类。返回（类名，成员下标从 0 起）。
fn parse_model_groups(raw: &str, count: usize) -> Option<Vec<(String, Vec<usize>)>> {
    let begin = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= begin {
        return None;
    }
    let parsed: ModelGroups = serde_json::from_str(&raw[begin..=end]).ok()?;
    let mut seen = vec![false; count];
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for group in parsed.groups {
        let name = group.name.lines().next().unwrap_or("").trim().to_owned();
        if name.is_empty() {
            continue;
        }
        let mut members = Vec::new();
        for member in group.members {
            let number = member
                .as_u64()
                .or_else(|| member.as_str().and_then(|s| s.trim().parse().ok()));
            let Some(index) = number
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|n| n.checked_sub(1))
            else {
                continue;
            };
            if index < count && !seen[index] {
                seen[index] = true;
                members.push(index);
            }
        }
        if members.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => existing.extend(members),
            None => groups.push((name, members)),
        }
        if groups.len() >= PATTERN_MAX_GROUPS {
            break;
        }
    }
    (!groups.is_empty()).then_some(groups)
}

/// 归类提案里的一类。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeProposalGroupDto {
    pub name: String,
    pub members: Vec<SwipeItemDto>,
}

/// 归类提案：先给用户改，确认后再写。`considered` 条参与了归类（最多 200 条、且放得进模型）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeProposalDto {
    pub groups: Vec<SwipeProposalGroupDto>,
    pub considered: u32,
    pub total: u32,
}

/// 「归类套路」（PRD-19-005）：AI 把灵感归成几类，返回提案，**不写任何文件**。
///
/// # Errors
/// 灵感少于三条、模型不可用或回得不像归类时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_suggest_swipe_patterns(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<SwipeProposalDto, String> {
    let (entries, _) = load_swipes(&app, window.label())?;
    if entries.len() < PATTERN_MIN_ENTRIES {
        return Err(coded_error_args(
            &app,
            "CREATOR_SWIPE_TOO_FEW",
            "creator-swipe-too-few",
            &[("min", PATTERN_MIN_ENTRIES.to_string())],
        ));
    }
    super::chat::ensure_chat_context_window(&app).await;
    let limit = floating_input_limit(app.state::<AppState>().chat_ctx_window());
    let (considered, text) = pattern_source(&entries, limit);
    let output = super::creator_publish::run_summarize_job(
        &app,
        "creator.swipe.patterns",
        text,
        PATTERN_OUTPUT_TOKENS,
    )
    .await?;
    let groups = parse_model_groups(&output, considered)
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;
    Ok(SwipeProposalDto {
        groups: groups
            .into_iter()
            .map(|(name, members)| SwipeProposalGroupDto {
                name,
                members: members.iter().map(|&i| (&entries[i]).into()).collect(),
            })
            .collect(),
        considered: count_u32(considered),
        total: count_u32(entries.len()),
    })
}

/// 用户确认后的一类：类名与要归进去的灵感。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct SwipeAssignmentDto {
    pub name: String,
    pub paths: Vec<String>,
}

/// 写回结果：写了几条；没写成的（写法改不了、写回期间被改过、已不是灵感）列出标题。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwipeApplyDto {
    pub written: u32,
    pub skipped: Vec<String>,
}

/// 把确认过的归类写回：每条灵感的套路标签换成 `<套路>/<类名>`，其它标签不动（ADR-162 ⑬）。
///
/// # Errors
/// 某个类名写不成标签（空或全是标点）时整批不写、返回错误；库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_apply_swipe_patterns(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    groups: Vec<SwipeAssignmentDto>,
) -> Result<SwipeApplyDto, String> {
    let i18n = backend_copy_arc(&app);
    let copy = swipe_copy(i18n.as_deref());
    // 先把类名都变成标签：有一个写不成就整批不动，让用户改名再来。
    let mut plan: Vec<(String, Vec<String>)> = Vec::new();
    for group in &groups {
        let Some(tag) = pattern_tag(&copy.prefix, &group.name) else {
            return Err(coded_error_args(
                &app,
                "CREATOR_SWIPE_BAD_NAME",
                "creator-swipe-bad-name",
                &[("name", group.name.clone())],
            ));
        };
        plan.push((tag, group.paths.clone()));
    }
    let mut written = 0u32;
    let mut skipped = Vec::new();
    for (tag, paths) in plan {
        for path in paths {
            let outcome = state.with_vault(window.label(), |vault| {
                let Ok(doc) = vault.read(&path) else {
                    return Ok(None);
                };
                if doc.meta.note_type != luna_vault::NoteType::Swipe {
                    return Ok(None);
                }
                match set_pattern_tag(&doc.content, &copy.prefix, &tag) {
                    NoteTagEdit::Written(next) => {
                        match vault
                            .save_note(&path, &doc.content, &next)
                            .map_err(vault_err)?
                        {
                            luna_vault::NoteSave::Saved { .. } => Ok(Some(Some(next))),
                            luna_vault::NoteSave::Conflict { .. } => Ok(None),
                        }
                    }
                    NoteTagEdit::Unchanged => Ok(Some(None)),
                    _ => Ok(None),
                }
            })?;
            match outcome {
                Some(Some(content)) => {
                    written += 1;
                    broadcast_note_write(&app, window.label(), &path, content);
                }
                Some(None) => {}
                None => skipped.push(note_stem_of(&path)),
            }
        }
    }
    if written > 0 {
        emit_vault_changed_in_scope(
            &app,
            "",
            false,
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(SwipeApplyDto { written, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, patterns: &[&str]) -> SwipeEntry {
        SwipeEntry {
            path: format!("Creator/Swipe/{title}.md"),
            title: title.into(),
            modified_ms: 0,
            snippet: Some(format!("{title}的片段")),
            why: Some("因为好。".into()),
            patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn the_library_groups_by_pattern_bigger_groups_first_loose_ones_last() {
        let entries = [
            entry("甲", &["悬念"]),
            entry("乙", &["清单", "悬念"]),
            entry("丙", &[]),
            entry("丁", &["清单"]),
            entry("戊", &["反常识"]),
        ];
        let groups = group_by_pattern(&entries);
        let shape: Vec<(Option<&str>, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.name.as_deref(),
                    g.items.iter().map(|i| i.title.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("悬念"), vec!["甲", "乙"]),
                (Some("清单"), vec!["乙", "丁"]),
                (Some("反常识"), vec!["戊"]),
                (None, vec!["丙"]),
            ]
        );
    }

    #[test]
    fn the_model_sees_numbers_only_and_bad_numbers_are_dropped() {
        let entries = [entry("甲", &[]), entry("乙", &[])];
        let (taken, text) = pattern_source(&entries, 10_000);
        assert_eq!(taken, 2);
        assert_eq!(
            text,
            "[1] 甲 | 甲的片段 | 因为好。\n[2] 乙 | 乙的片段 | 因为好。"
        );
        assert!(!text.contains("Creator/"));
        // 放不下第二条：只取第一条（第一条总会放）。
        assert_eq!(pattern_source(&entries, 5).0, 1);

        let raw = "好的\n{\"groups\":[{\"name\":\"悬念\\n多余\",\"members\":[1,\"3\",1,9]},{\"name\":\"清单\",\"members\":[3,2]},{\"name\":\"空\",\"members\":[9]},{\"name\":\"\",\"members\":[4]}]}";
        assert_eq!(
            parse_model_groups(raw, 4),
            Some(vec![("悬念".into(), vec![0, 2]), ("清单".into(), vec![1])])
        );
        assert_eq!(parse_model_groups("{\"groups\":[]}", 4), None);
        assert_eq!(parse_model_groups("没有", 4), None);
    }

    #[test]
    fn new_swipes_start_with_three_sections() {
        assert_eq!(
            swipe_skeleton(None),
            "## 片段\n\n\n\n## 为什么好\n\n\n\n## 来源\n\n"
        );
    }
}
