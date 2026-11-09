//! 用户标签的就地改写（ADR-135 / TDD-06 §2.3）。
//!
//! 这是 Lunarca 改写 frontmatter 顶层 `tags` 的**唯一**入口，只服务用户在标签处显式发起的
//! 改名 / 合并 / 增删。纪律：只动 `tags` 键的值，其余字节逐字保留；只认三种写法——流式列表
//! `tags: [a, b]`、块列表（`tags:` 下逐行 `- a`）、单值或逗号串 `tags: a, b`——并保持原引号与
//! 缩进；认不出的写法（锚点/别名、行内注释、多行标量、跨行流式列表……）整篇跳过、如实报原因，
//! 绝不重排 YAML。正文 `#标签` 只按索引同一个扫描器（[`luna_sync_core::tags::inline_tag_spans`]）给出
//! 的位置改写。

use crate::frontmatter::{body_of, read_luna_checked, update_luna};
use luna_sync_core::frontmatter::frontmatter_inner_range;
use luna_sync_core::frontmatter_edit::{dedupe, render_item, EditSkip};

/// 这一篇为什么没改（预览里逐篇列出）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagEditSkip {
    /// `tags` 用了锚点、别名或块标量（`&` / `*` / `|` / `>`）。
    UnsupportedForm,
    /// `tags` 的值里带行内注释，改写后无法保证注释仍在原处。
    InlineComment,
    /// 流式列表跨了多行。
    MultiLine,
    /// `luna:` 元数据读不出（未知版本或损坏），不敢改写。
    InvalidMetadata,
}

/// `tag` 若是 `from` 本身或其子标签，给出改名后的写法；否则 `None`。
///
/// 与派生索引同一比较口径：ASCII 大小写不敏感（SQLite `lower()` 只折 ASCII）。子标签只在
/// `/` 边界上匹配——`项目` 改名不会波及 `项目组`。
#[must_use]
pub fn renamed_tag(tag: &str, from: &str, to: &str) -> Option<String> {
    let head = tag.get(..from.len())?;
    if !head.eq_ignore_ascii_case(from) {
        return None;
    }
    let rest = &tag[from.len()..];
    if rest.is_empty() {
        Some(to.to_owned())
    } else if rest.starts_with('/') {
        Some(format!("{to}{rest}"))
    } else {
        None
    }
}

/// 一篇笔记里把 `from`（及其子标签）改名为 `to`：frontmatter `tags`、正文 `#标签`（主题页与
/// 知识地图的正文是系统产物，`include_body = false` 时不碰）、`luna.ai_tags`、主题页出身标签与
/// 声明范围。`Ok(None)` = 这一篇没有要改的。
pub fn rename_tag_in_note(
    content: &str,
    from: &str,
    to: &str,
    include_body: bool,
) -> Result<Option<String>, TagEditSkip> {
    let map = |values: Vec<String>| -> Vec<String> {
        values
            .into_iter()
            .map(|tag| renamed_tag(&tag, from, to).unwrap_or(tag))
            .collect()
    };
    let mut out = rewrite_frontmatter_tags(content, map)?.unwrap_or_else(|| content.to_owned());
    out = rename_in_luna(&out, from, to)?.unwrap_or(out);
    if include_body {
        out = rename_inline(&out, from, to);
    }
    Ok((out != content).then_some(out))
}

/// 就地改写 frontmatter 顶层 `tags` 的值：`transform` 拿到当前标签（去引号）、返回新标签；
/// 结果按大小写不敏感去重。没有 `tags` 键或结果不变时 `Ok(None)`。写法的认定与改写是
/// 通用的顶层键写入器（[`luna_sync_core::frontmatter_edit`]，ADR-139），这里只把它的拒绝原因
/// 归到标签动作对用户说的几种。
pub fn rewrite_frontmatter_tags(
    content: &str,
    transform: impl Fn(Vec<String>) -> Vec<String>,
) -> Result<Option<String>, TagEditSkip> {
    luna_sync_core::frontmatter_edit::rewrite_list_value(content, "tags", true, transform)
        .map_err(skip_of)
}

fn skip_of(skip: EditSkip) -> TagEditSkip {
    match skip {
        EditSkip::InlineComment => TagEditSkip::InlineComment,
        EditSkip::MultiLine => TagEditSkip::MultiLine,
        EditSkip::Unclosed => TagEditSkip::InvalidMetadata,
        EditSkip::UnsupportedForm
        | EditSkip::Nested
        | EditSkip::DuplicateKey
        | EditSkip::KeyExists
        | EditSkip::InvalidKey
        | EditSkip::NotFound
        | EditSkip::InvalidValue => TagEditSkip::UnsupportedForm,
    }
}

fn has_tags_key(content: &str) -> bool {
    luna_sync_core::frontmatter_edit::read_properties(content)
        .is_ok_and(|props| props.iter().any(|p| p.key == "tags"))
}

/// `luna:` 块里的 AI 标签、主题页出身标签与声明范围（Lunarca 自有块，整块重渲染无妨）。
fn rename_in_luna(content: &str, from: &str, to: &str) -> Result<Option<String>, TagEditSkip> {
    if frontmatter_inner_range(content).is_none() {
        return Ok(None);
    }
    let mut meta = read_luna_checked(content).map_err(|_| TagEditSkip::InvalidMetadata)?;
    let mut changed = false;
    let rename_all = |tags: &mut Vec<String>, changed: &mut bool| {
        for tag in tags.iter_mut() {
            if let Some(next) = renamed_tag(tag, from, to) {
                *tag = next;
                *changed = true;
            }
        }
        let deduped = dedupe(std::mem::take(tags));
        *tags = deduped;
    };
    rename_all(&mut meta.ai_tags, &mut changed);
    if let Some(src) = meta.topic_src.as_mut() {
        if let Some(next) = renamed_tag(src, from, to) {
            *src = next;
            changed = true;
        }
    }
    if let Some(knowledge) = meta.knowledge.as_mut() {
        if let Some(scope) = knowledge.get_mut("scope") {
            for key in ["includes", "excludes"] {
                if let Some(serde_yaml_ng::Value::Sequence(list)) = scope.get_mut(key) {
                    for entry in list.iter_mut() {
                        if let serde_yaml_ng::Value::String(tag) = entry {
                            if let Some(next) = renamed_tag(tag, from, to) {
                                *tag = next;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    if !changed {
        return Ok(None);
    }
    update_luna(content, &meta)
        .map(Some)
        .map_err(|_| TagEditSkip::InvalidMetadata)
}

/// 正文 `#标签`：只改扫描器认出的位置，从后往前替换，其余字节不动。
fn rename_inline(content: &str, from: &str, to: &str) -> String {
    let body = body_of(content);
    let body_start = content.len() - body.len();
    let mut out = content.to_owned();
    for span in luna_sync_core::tags::inline_tag_spans(body)
        .into_iter()
        .rev()
    {
        let tag = &body[span.clone()];
        if let Some(next) = renamed_tag(tag, from, to) {
            out.replace_range(body_start + span.start..body_start + span.end, &next);
        }
    }
    out
}

/// 单篇笔记就地增删一个用户标签的结局（ADR-135）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteTagEdit {
    /// 已写入（新全文）。
    Written(String),
    /// 本来就有（加）/ 本来就没有（删）：什么都不改。
    Unchanged,
    /// 要删的标签写在正文里（`#标签`），这里不动正文。
    InlineOnly,
    /// 要删的标签是 AI 关系标签（走全库忽略）。
    AiOnly,
    /// 这一篇的 `tags` 写法无法安全改写。
    Skipped(TagEditSkip),
}

/// 给一篇笔记加一个用户标签：已有 `tags` 键就追加（同写法），没有就在 frontmatter 首行插
/// `tags: [x]`，连 frontmatter 都没有就在文件最前面加一段——正文字节不变。
pub fn add_tag_to_note(content: &str, tag: &str) -> NoteTagEdit {
    if luna_sync_core::tags::extract_tags(content)
        .iter()
        .any(|term| {
            term.tag.eq_ignore_ascii_case(tag)
                && term.source == luna_sync_core::tags::TagSource::User
        })
    {
        return NoteTagEdit::Unchanged;
    }
    if has_tags_key(content) {
        return match rewrite_frontmatter_tags(content, |mut tags| {
            tags.push(tag.to_owned());
            tags
        }) {
            Ok(Some(next)) => NoteTagEdit::Written(next),
            Ok(None) => NoteTagEdit::Unchanged,
            Err(skip) => NoteTagEdit::Skipped(skip),
        };
    }
    let line = format!("tags: [{}]", render_item(tag, None));
    let written = match frontmatter_inner_range(content) {
        Some(inner) => {
            let newline = if content[..inner.start].ends_with("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            format!(
                "{}{line}{newline}{}",
                &content[..inner.start],
                &content[inner.start..]
            )
        }
        // 开头的 frontmatter 没闭合：再前置一份会叠出两段，照「元数据读不出」如实跳过。
        None if luna_sync_core::frontmatter::has_unclosed_frontmatter(content) => {
            return NoteTagEdit::Skipped(TagEditSkip::InvalidMetadata);
        }
        None => format!("---\n{line}\n---\n{content}"),
    };
    NoteTagEdit::Written(written)
}

/// 从一篇笔记移除一个用户标签：只改 frontmatter `tags`；写在正文或只来自 AI 的如实说明。
pub fn remove_tag_from_note(content: &str, tag: &str) -> NoteTagEdit {
    let in_frontmatter = luna_sync_core::tags::frontmatter_tags(content)
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tag));
    if !in_frontmatter {
        let inline = luna_sync_core::tags::extract_inline_tags(body_of(content))
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tag));
        if inline {
            return NoteTagEdit::InlineOnly;
        }
        let ai = crate::read_luna(content)
            .ai_tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tag));
        return if ai {
            NoteTagEdit::AiOnly
        } else {
            NoteTagEdit::Unchanged
        };
    }
    match rewrite_frontmatter_tags(content, |tags| {
        tags.into_iter()
            .filter(|t| !t.eq_ignore_ascii_case(tag))
            .collect()
    }) {
        Ok(Some(next)) => NoteTagEdit::Written(next),
        Ok(None) => NoteTagEdit::Unchanged,
        Err(skip) => NoteTagEdit::Skipped(skip),
    }
}

/// 标签名是否可写（与正文 `#标签` 同一字符集；不以 `/` 开头或结尾、没有空段）。
#[must_use]
pub fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && !tag.starts_with('/')
        && !tag.ends_with('/')
        && !tag.contains("//")
        && tag.chars().all(luna_sync_core::tags::is_tag_char)
        && tag.chars().any(|c| !c.is_ascii_digit())
}

/// 一篇要改的笔记与预览那一刻的整文件指纹（应用时据此条件写）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEditTarget {
    pub path: String,
    pub before_hash: String,
}

/// 改名的影响预览（ADR-135）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRenamePreview {
    /// 目标标签已存在：这次改名其实是合并。
    pub merge: bool,
    pub targets: Vec<TagEditTarget>,
    /// 写法不支持而整篇跳过的笔记。
    pub skipped: Vec<(String, TagEditSkip)>,
}

/// 一篇改好了：前后整文件指纹（整批撤销用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEditChange {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
}

/// 逐篇回执：改好的、预览之后又被改过而没动的、写法不支持的、写盘失败的。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagEditReceipt {
    pub changed: Vec<TagEditChange>,
    pub stale: Vec<String>,
    pub skipped: Vec<(String, TagEditSkip)>,
    pub failed: Vec<String>,
}

/// 整批撤销的逐篇结果：恢复了的，与之后又被改过（或写入前版本找不到）而没动的。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagUndoReceipt {
    pub restored: Vec<String>,
    pub kept: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TagEditError {
    #[error("invalid tag name")]
    InvalidName,
    #[error("rename to the same tag")]
    SameTag,
    #[error("vault: {0}")]
    Vault(String),
}

fn is_system_page(content: &str) -> bool {
    let meta = crate::read_luna(content);
    meta.topic || meta.knowledge_map
}

impl crate::Vault {
    /// 单篇就地加（`add = true`）或删一个用户标签：读当前全文、按同一份字节条件写（留版本、
    /// 重建索引）。写入期间被别处改过就当作没改（调用方重取后再试）。
    pub fn edit_note_tag(
        &self,
        rel: &str,
        tag: &str,
        add: bool,
    ) -> Result<NoteTagEdit, TagEditError> {
        let tag = tag.trim().trim_start_matches('#');
        if !is_valid_tag(tag) {
            return Err(TagEditError::InvalidName);
        }
        let vault_err = |error: crate::VaultError| {
            TagEditError::Vault(format!("{}: {}", error.code(), error.detail()))
        };
        let doc = self.read(rel).map_err(vault_err)?;
        let outcome = if add {
            add_tag_to_note(&doc.content, tag)
        } else {
            remove_tag_from_note(&doc.content, tag)
        };
        if let NoteTagEdit::Written(next) = &outcome {
            let before =
                luna_sync_core::hashing::ContentHash::of(doc.content.as_bytes()).to_string();
            if !self
                .write_if_current(rel, &before, next)
                .map_err(vault_err)?
            {
                return Ok(NoteTagEdit::Unchanged);
            }
        }
        Ok(outcome)
    }

    /// 预览把 `from`（及其子标签）改名为 `to`：影响的笔记与各自整文件指纹、是否其实是合并、
    /// 因写法不支持而跳过的笔记。只读。
    pub fn preview_tag_rename(
        &self,
        from: &str,
        to: &str,
    ) -> Result<TagRenamePreview, TagEditError> {
        let (from, to) = (from.trim(), to.trim());
        if !is_valid_tag(from) || !is_valid_tag(to) {
            return Err(TagEditError::InvalidName);
        }
        if from == to {
            return Err(TagEditError::SameTag);
        }
        let vault_err = |error: crate::VaultError| {
            TagEditError::Vault(format!("{}: {}", error.code(), error.detail()))
        };
        let mut paths = self.notes_with_tag_branch(from).map_err(vault_err)?;
        // 主题页的出身标签与声明范围不在 `tags` 表里，另行纳入。
        for page in self.topic_pages().map_err(vault_err)? {
            if !paths.contains(&page) {
                paths.push(page);
            }
        }
        paths.sort();
        let mut preview = TagRenamePreview {
            merge: !from.eq_ignore_ascii_case(to) && self.tag_exists(to).map_err(vault_err)?,
            targets: Vec::new(),
            skipped: Vec::new(),
        };
        for path in paths {
            let Ok(doc) = self.read(&path) else {
                continue;
            };
            match rename_tag_in_note(&doc.content, from, to, !is_system_page(&doc.content)) {
                Ok(Some(_)) => preview.targets.push(TagEditTarget {
                    path,
                    before_hash: luna_sync_core::hashing::ContentHash::of(doc.content.as_bytes())
                        .to_string(),
                }),
                Ok(None) => {}
                Err(skip) => preview.skipped.push((path, skip)),
            }
        }
        Ok(preview)
    }

    /// 按预览逐篇改名：每篇仍是预览时那一版才写（留版本、重建索引），否则记为「预览后被改过」；
    /// 不承诺整批原子，逐篇回执。系统维护中的主题页在只因改名变了范围时重登底版。
    pub fn apply_tag_rename(
        &self,
        from: &str,
        to: &str,
        targets: &[TagEditTarget],
    ) -> Result<TagEditReceipt, TagEditError> {
        let (from, to) = (from.trim(), to.trim());
        if !is_valid_tag(from) || !is_valid_tag(to) {
            return Err(TagEditError::InvalidName);
        }
        let mut receipt = TagEditReceipt::default();
        for target in targets {
            let Ok(doc) = self.read(&target.path) else {
                receipt.stale.push(target.path.clone());
                continue;
            };
            if luna_sync_core::hashing::ContentHash::of(doc.content.as_bytes()).as_str()
                != target.before_hash
            {
                receipt.stale.push(target.path.clone());
                continue;
            }
            let system = is_system_page(&doc.content);
            let maintained = system
                && self.topic_maintenance_state(&target.path).ok()
                    == Some(crate::TopicMaintenanceState::Ready);
            let next = match rename_tag_in_note(&doc.content, from, to, !system) {
                Ok(Some(next)) => next,
                Ok(None) => continue,
                Err(skip) => {
                    receipt.skipped.push((target.path.clone(), skip));
                    continue;
                }
            };
            match self.write_if_current(&target.path, &target.before_hash, &next) {
                Ok(true) => {
                    if maintained {
                        let _ = self.record_topic_baseline(&target.path, &next);
                    }
                    receipt.changed.push(TagEditChange {
                        path: target.path.clone(),
                        before_hash: target.before_hash.clone(),
                        after_hash: luna_sync_core::hashing::ContentHash::of(next.as_bytes())
                            .to_string(),
                    });
                }
                Ok(false) => receipt.stale.push(target.path.clone()),
                Err(_) => receipt.failed.push(target.path.clone()),
            }
        }
        Ok(receipt)
    }

    /// 整批撤销：只恢复此刻仍逐字节是改名后那一版的文件（从版本链取回改名前那一版），
    /// 之后又改过的一律不动并列出。
    pub fn undo_tag_edit(&self, changes: &[TagEditChange]) -> TagUndoReceipt {
        let mut receipt = TagUndoReceipt::default();
        for change in changes {
            let current = self.read(&change.path).ok();
            let unchanged = current.as_ref().is_some_and(|doc| {
                luna_sync_core::hashing::ContentHash::of(doc.content.as_bytes()).as_str()
                    == change.after_hash
            });
            let before = unchanged
                .then(|| self.snapshot_text_with_hash(&change.path, &change.before_hash))
                .flatten();
            let Some(before) = before else {
                receipt.kept.push(change.path.clone());
                continue;
            };
            let maintained = is_system_page(&before)
                && self.topic_maintenance_state(&change.path).ok()
                    == Some(crate::TopicMaintenanceState::Ready);
            match self.write_if_current(&change.path, &change.after_hash, &before) {
                Ok(true) => {
                    if maintained {
                        let _ = self.record_topic_baseline(&change.path, &before);
                    }
                    receipt.restored.push(change.path.clone());
                }
                _ => receipt.kept.push(change.path.clone()),
            }
        }
        receipt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(content: &str) -> Result<Option<String>, TagEditSkip> {
        rename_tag_in_note(content, "项目", "工作", true)
    }

    #[test]
    fn adding_a_tag_to_an_unclosed_frontmatter_is_skipped_not_stacked() {
        assert_eq!(
            add_tag_to_note("---\ntags: [a]\n---# 标题\n", "b"),
            NoteTagEdit::Skipped(TagEditSkip::InvalidMetadata)
        );
        assert_eq!(
            add_tag_to_note("# 标题\n", "b"),
            NoteTagEdit::Written("---\ntags: [b]\n---\n# 标题\n".into())
        );
    }

    #[test]
    fn only_whole_tags_and_their_children_are_renamed() {
        assert_eq!(renamed_tag("项目", "项目", "工作").as_deref(), Some("工作"));
        assert_eq!(
            renamed_tag("项目/C", "项目", "工作").as_deref(),
            Some("工作/C")
        );
        assert_eq!(
            renamed_tag("项目组", "项目", "工作"),
            None,
            "只在 / 边界匹配"
        );
        assert_eq!(
            renamed_tag("Project/A", "project", "Work").as_deref(),
            Some("Work/A")
        );
    }

    #[test]
    fn flow_lists_keep_everything_but_the_renamed_values() {
        let before = "---\ntitle: 周报\ntags: [项目/C, \"阅读\", 生活] # 我的注释\nluna:\n  schema: 1\n---\n正文\n";
        let after = rename(before).unwrap().unwrap();
        assert_eq!(
            after,
            "---\ntitle: 周报\ntags: [工作/C, \"阅读\", 生活] # 我的注释\nluna:\n  schema: 1\n---\n正文\n"
        );
    }

    #[test]
    fn block_lists_keep_indent_and_merge_duplicates() {
        let before = "---\ntags:\n  - 项目\n  - 工作\n  - 阅读\ncreated: 2026-09-01\n---\n正文\n";
        let after = rename(before).unwrap().unwrap();
        assert_eq!(
            after, "---\ntags:\n  - 工作\n  - 阅读\ncreated: 2026-09-01\n---\n正文\n",
            "改名撞上已有标签即合并，只留一份"
        );
    }

    #[test]
    fn scalar_and_comma_strings_are_rewritten_in_place() {
        assert_eq!(
            rename("---\ntags: 项目, 阅读\n---\n").unwrap().unwrap(),
            "---\ntags: 工作, 阅读\n---\n"
        );
        assert_eq!(
            rename("---\ntags: '项目/A'\n---\n").unwrap().unwrap(),
            "---\ntags: '工作/A'\n---\n"
        );
    }

    #[test]
    fn unsafe_forms_are_skipped_not_reformatted() {
        assert_eq!(
            rename("---\ntags: &t [项目]\n---\n"),
            Err(TagEditSkip::UnsupportedForm)
        );
        assert_eq!(
            rename("---\ntags: [项目,\n  阅读]\n---\n"),
            Err(TagEditSkip::MultiLine)
        );
        assert_eq!(
            rename("---\ntags:\n  - 项目 # 旧的\n---\n"),
            Err(TagEditSkip::InlineComment)
        );
    }

    #[test]
    fn inline_tags_are_renamed_where_the_indexer_sees_them_only() {
        let before = "---\ntags: [阅读]\n---\n今天 #项目/C 有进展，#项目组 不动。\n\n```\n#项目 在代码里\n```\n`#项目` 行内代码\n";
        let after = rename(before).unwrap().unwrap();
        assert!(
            after.contains("今天 #工作/C 有进展，#项目组 不动。"),
            "{after}"
        );
        assert!(after.contains("#项目 在代码里"), "代码块不动");
        assert!(after.contains("`#项目` 行内代码"), "行内代码不动");
        assert!(
            after.starts_with("---\ntags: [阅读]\n---\n"),
            "不相关的 tags 一字不动"
        );
    }

    #[test]
    fn topic_scope_origin_and_ai_tags_follow_the_rename() {
        let before = "---\nluna:\n  schema: 1\n  topic: true\n  topic_src: 项目/C\n  ai_tags:\n  - 项目\n  knowledge:\n    scope:\n      title: 项目 · C\n      includes:\n      - 项目/C\n      excludes: []\n---\n# C\n\n正文 #项目\n";
        let after = rename_tag_in_note(before, "项目", "工作", false)
            .unwrap()
            .unwrap();
        let meta = crate::read_luna(&after);
        assert_eq!(meta.topic_src.as_deref(), Some("工作/C"));
        assert_eq!(meta.ai_tags, vec!["工作".to_owned()]);
        assert!(after.contains("- 工作/C"), "{after}");
        assert!(
            after.ends_with("正文 #项目\n"),
            "主题页正文是系统产物，不改"
        );
    }

    fn topic_fixture(vault: &crate::Vault) {
        vault
            .write(
                "Notes/会议.md",
                "---\ntags: [项目/C]\n---\n# 会议\n\n原型已完成。\n",
            )
            .unwrap();
        let hash = vault.body_fingerprint("Notes/会议.md").unwrap();
        let source = [crate::ClaimSource {
            path: "Notes/会议.md",
            title: "会议",
            source_hash: &hash,
            locator: crate::EvidenceLocator {
                heading_path: vec!["会议".into()],
                quote: "原型已完成".into(),
                context: String::new(),
                line_hint: Some(6),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = crate::NewClaim {
            id: uuid::Uuid::now_v7(),
            text: "原型已完成",
            kind: crate::ClaimKind::Synthesis,
            sources: &source,
        };
        let content = vault
            .scoped_topic_content(
                "Knowledge/C.md",
                Some("项目/C"),
                crate::TopicScope::from_tag("项目/C"),
                &[claim],
            )
            .unwrap();
        vault
            .publish_scoped_topic("Knowledge/C.md", &content)
            .unwrap();
    }

    /// 改名批次端到端（ADR-135）：预览→逐篇条件写→回执→整批撤销。
    #[test]
    fn renaming_a_tag_across_the_vault_previews_applies_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = crate::Vault::open(dir.path()).unwrap();
        topic_fixture(&vault);
        vault
            .write(
                "Notes/甲.md",
                "---\ntags:\n  - 项目\n  - 阅读\n---\n正文 #项目/C\n",
            )
            .unwrap();
        vault
            .write("Notes/乙.md", "---\ntags: 工作\n---\n#项目 进展\n")
            .unwrap();
        vault
            .write("Notes/丙.md", "---\ntags: &t [项目]\n---\n正文\n")
            .unwrap();
        vault
            .write("Notes/丁.md", "---\ntags: [项目]\n---\n正文\n")
            .unwrap();
        assert_eq!(
            vault.topic_maintenance_state("Knowledge/C.md").unwrap(),
            crate::TopicMaintenanceState::Ready
        );

        let preview = vault.preview_tag_rename("项目", "工作").unwrap();
        assert!(preview.merge, "目标已存在即合并");
        let paths: Vec<&str> = preview.targets.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "Knowledge/C.md",
                "Notes/丁.md",
                "Notes/乙.md",
                "Notes/会议.md",
                "Notes/甲.md"
            ]
        );
        assert_eq!(
            preview.skipped,
            vec![("Notes/丙.md".to_owned(), TagEditSkip::UnsupportedForm)]
        );

        // 预览之后有一篇被改过：那一篇不写。
        vault
            .write("Notes/丁.md", "---\ntags: [项目]\n---\n改过了\n")
            .unwrap();
        let receipt = vault
            .apply_tag_rename("项目", "工作", &preview.targets)
            .unwrap();
        assert_eq!(receipt.stale, vec!["Notes/丁.md".to_owned()]);
        assert_eq!(receipt.changed.len(), 4);
        assert_eq!(
            vault.read("Notes/甲.md").unwrap().content,
            "---\ntags:\n  - 工作\n  - 阅读\n---\n正文 #工作/C\n"
        );
        assert_eq!(
            vault.read("Notes/乙.md").unwrap().content,
            "---\ntags: 工作\n---\n#工作 进展\n"
        );
        assert!(vault.tag_exists("工作/C").unwrap());
        assert!(!vault.tag_exists("项目/C").unwrap());
        assert_eq!(
            vault.topic_maintenance_state("Knowledge/C.md").unwrap(),
            crate::TopicMaintenanceState::Ready,
            "只因改名变了范围的系统页仍由系统维护"
        );
        assert_eq!(
            crate::read_luna(&vault.read("Knowledge/C.md").unwrap().content)
                .topic_src
                .as_deref(),
            Some("工作/C")
        );

        // 整批撤销：之后又改过的那篇不动。
        let edited = format!("{}补一句。\n", vault.read("Notes/乙.md").unwrap().content);
        vault.write("Notes/乙.md", &edited).unwrap();
        let undo = vault.undo_tag_edit(&receipt.changed);
        assert_eq!(undo.kept, vec!["Notes/乙.md".to_owned()]);
        assert_eq!(undo.restored.len(), 3);
        assert_eq!(
            vault.read("Notes/甲.md").unwrap().content,
            "---\ntags:\n  - 项目\n  - 阅读\n---\n正文 #项目/C\n"
        );
        assert_eq!(vault.read("Notes/乙.md").unwrap().content, edited);
        assert_eq!(
            vault.topic_maintenance_state("Knowledge/C.md").unwrap(),
            crate::TopicMaintenanceState::Ready
        );
    }

    #[test]
    fn a_single_note_gains_and_loses_a_tag_through_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let vault = crate::Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/周报.md", "---\ntags: [阅读]\n---\n正文\n")
            .unwrap();
        assert!(matches!(
            vault.edit_note_tag("Notes/周报.md", "#项目", true),
            Ok(NoteTagEdit::Written(_))
        ));
        assert!(vault.tag_exists("项目").unwrap());
        assert!(matches!(
            vault.edit_note_tag("Notes/周报.md", "阅读", false),
            Ok(NoteTagEdit::Written(_))
        ));
        assert!(!vault.tag_exists("阅读").unwrap());
        assert_eq!(
            vault.read("Notes/周报.md").unwrap().content,
            "---\ntags: [项目]\n---\n正文\n"
        );
        assert_eq!(
            vault.edit_note_tag("Notes/周报.md", "有 空格", true),
            Err(TagEditError::InvalidName)
        );
    }

    #[test]
    fn names_are_validated_before_anything_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let vault = crate::Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.preview_tag_rename("项目", "有 空格"),
            Err(TagEditError::InvalidName)
        );
        assert_eq!(
            vault.preview_tag_rename("项目", "/工作"),
            Err(TagEditError::InvalidName)
        );
        assert_eq!(
            vault.preview_tag_rename("项目", "项目"),
            Err(TagEditError::SameTag)
        );
    }

    #[test]
    fn adding_a_tag_keeps_the_note_and_its_writing_style() {
        assert_eq!(
            add_tag_to_note("---\ntags: [阅读]\n---\n正文\n", "项目"),
            NoteTagEdit::Written("---\ntags: [阅读, 项目]\n---\n正文\n".into())
        );
        assert_eq!(
            add_tag_to_note("---\ntitle: 周报\n---\n正文\n", "项目"),
            NoteTagEdit::Written("---\ntags: [项目]\ntitle: 周报\n---\n正文\n".into()),
            "没有 tags 键就在 frontmatter 首行插一行"
        );
        assert_eq!(
            add_tag_to_note("正文 #阅读\n", "项目"),
            NoteTagEdit::Written("---\ntags: [项目]\n---\n正文 #阅读\n".into()),
            "没有 frontmatter 就在最前面加一段，正文字节不变"
        );
        assert_eq!(
            add_tag_to_note("正文 #项目\n", "项目"),
            NoteTagEdit::Unchanged,
            "已经有了（正文里写的也算）"
        );
    }

    #[test]
    fn removing_a_tag_only_touches_the_frontmatter_and_explains_the_rest() {
        assert_eq!(
            remove_tag_from_note("---\ntags:\n  - 项目\n  - 阅读\n---\n正文\n", "项目"),
            NoteTagEdit::Written("---\ntags:\n  - 阅读\n---\n正文\n".into())
        );
        assert_eq!(
            remove_tag_from_note("---\ntags: [项目]\n---\n正文\n", "项目"),
            NoteTagEdit::Written("---\ntags: []\n---\n正文\n".into())
        );
        assert_eq!(
            remove_tag_from_note("正文 #项目\n", "项目"),
            NoteTagEdit::InlineOnly
        );
        assert_eq!(
            remove_tag_from_note(
                "---\nluna:\n  schema: 1\n  ai_tags: [项目]\n---\n正文\n",
                "项目"
            ),
            NoteTagEdit::AiOnly
        );
    }

    #[test]
    fn crlf_and_untouched_notes_are_respected() {
        let before = "---\r\ntags:\r\n  - 项目\r\n---\r\n正文\r\n";
        assert_eq!(
            rename(before).unwrap().unwrap(),
            "---\r\ntags:\r\n  - 工作\r\n---\r\n正文\r\n"
        );
        assert_eq!(rename("---\ntags: [阅读]\n---\n正文\n").unwrap(), None);
    }
}
