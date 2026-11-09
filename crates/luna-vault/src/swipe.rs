//! 灵感库（ADR-162 ⑬，PRD-19-005）：灵感条目的列出、「为什么好」摘句与「套路」标签。
//!
//! 灵感条目（`luna.type: swipe`）是普通 Markdown，约定三节：片段 / 为什么好 / 来源（节名随界面
//! 语言，由调用方给出）。归类结果写在用户自己的顶层 `tags` 里，统一挂在 `<前缀>/` 这一枝下
//! （如 `套路/悬念`）：重新归类时只换这一枝，用户打的其它标签不动。

use crate::error::Result;
use crate::frontmatter::NoteType;
use crate::tag_edit::{add_tag_to_note, is_valid_tag, rewrite_frontmatter_tags, NoteTagEdit};
use crate::vault::Vault;

/// 一条灵感。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwipeEntry {
    pub path: String,
    pub title: String,
    pub modified_ms: i64,
    /// 「片段」一节的第一行（没有这一节时取正文第一行字）。
    pub snippet: Option<String>,
    /// 「为什么好」一节的第一句。
    pub why: Option<String>,
    /// 套路标签里 `<前缀>/` 之后的名字（可能不止一个）。
    pub patterns: Vec<String>,
}

/// 灵感条目里各节的名字与套路标签的前缀（随界面语言，由调用方给出）。
#[derive(Clone, Copy, Debug)]
pub struct SwipeNames<'a> {
    pub snippet: &'a str,
    pub why: &'a str,
    pub prefix: &'a str,
}

/// 一段文字的第一行字（跳过空行与标题行）。
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_start_matches(['-', '*', '>']).trim().to_owned())
        .filter(|line| !line.is_empty())
}

/// 第一句：到第一个句末标点为止（含标点）；没有句末标点就是整行。
fn first_sentence(line: &str) -> String {
    match line
        .char_indices()
        .find(|(_, c)| matches!(c, '。' | '！' | '？' | '.' | '!' | '?'))
    {
        Some((at, c)) => line[..at + c.len_utf8()].to_owned(),
        None => line.to_owned(),
    }
}

/// 一篇笔记顶层 `tags` 里属于 `<前缀>/` 这一枝的名字（前缀不分大小写）。
#[must_use]
pub fn pattern_names(content: &str, prefix: &str) -> Vec<String> {
    let branch = format!("{prefix}/").to_lowercase();
    luna_sync_core::tags::frontmatter_tags(content)
        .into_iter()
        .filter_map(|tag| {
            tag.to_lowercase()
                .starts_with(&branch)
                .then(|| tag[branch.len()..].to_owned())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// 套路名 → 能写进标签的样子：去掉首尾空白与 `#`，中间的空白与 `/` 换成 `-`。写不成合法
/// 标签（如全是标点）为 `None`。
#[must_use]
pub fn pattern_tag(prefix: &str, name: &str) -> Option<String> {
    let name: String = name
        .trim()
        .trim_start_matches('#')
        .trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '/' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let tag = format!("{prefix}/{name}");
    (!name.is_empty() && is_valid_tag(&tag)).then_some(tag)
}

/// 把一篇笔记的套路标签换成 `tag`：先摘掉 `<前缀>/` 这一枝的旧标签，再加上新的；其它标签不动。
#[must_use]
pub fn set_pattern_tag(content: &str, prefix: &str, tag: &str) -> NoteTagEdit {
    let branch = format!("{prefix}/").to_lowercase();
    let has_tags = luna_sync_core::frontmatter_edit::read_properties(content)
        .is_ok_and(|props| props.iter().any(|p| p.key == "tags"));
    if !has_tags {
        return add_tag_to_note(content, tag);
    }
    match rewrite_frontmatter_tags(content, |tags| {
        let mut next: Vec<String> = tags
            .into_iter()
            .filter(|t| !t.to_lowercase().starts_with(&branch))
            .collect();
        next.push(tag.to_owned());
        next
    }) {
        Ok(Some(next)) => NoteTagEdit::Written(next),
        Ok(None) => NoteTagEdit::Unchanged,
        Err(skip) => NoteTagEdit::Skipped(skip),
    }
}

/// 列看灵感时一页取多少（只决定分几次查索引，不截断结果）。
const SWIPE_PAGE: usize = 200;

impl Vault {
    /// 全部灵感条目，新的在前（派生索引的顺序）。读不到的跳过。
    ///
    /// # Errors
    /// 派生索引查询失败时返回错误。
    pub fn swipe_entries(&self, names: SwipeNames<'_>) -> Result<Vec<SwipeEntry>> {
        let mut entries = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, next) =
                self.list_notes_of_type(NoteType::Swipe, SWIPE_PAGE, cursor.as_deref())?;
            for entry in page {
                let Ok(doc) = self.read(&entry.path) else {
                    continue;
                };
                if doc.meta.note_type != NoteType::Swipe {
                    continue;
                }
                let snippet = crate::script::section_body(&doc.content, names.snippet)
                    .and_then(|section| first_line(&section))
                    .or_else(|| first_line(crate::frontmatter::body_of(&doc.content)));
                let why = crate::script::section_body(&doc.content, names.why)
                    .and_then(|section| first_line(&section))
                    .map(|line| first_sentence(&line));
                entries.push(SwipeEntry {
                    patterns: pattern_names(&doc.content, names.prefix),
                    path: entry.path,
                    title: entry.title,
                    modified_ms: entry.modified_ms,
                    snippet,
                    why,
                });
            }
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: SwipeNames<'static> = SwipeNames {
        snippet: "片段",
        why: "为什么好",
        prefix: "套路",
    };

    #[test]
    fn pattern_tags_are_one_branch_and_only_that_branch_is_replaced() {
        let note = "---\ntags: [选题, 套路/清单]\n---\n正文\n";
        assert_eq!(pattern_names(note, "套路"), ["清单"]);
        let NoteTagEdit::Written(next) = set_pattern_tag(note, "套路", "套路/悬念") else {
            panic!("应当写入");
        };
        assert_eq!(
            luna_sync_core::tags::frontmatter_tags(&next),
            ["选题", "套路/悬念"]
        );
        assert!(next.ends_with("正文\n"));
        // 没有 tags 键：新加一行，正文不动。
        let NoteTagEdit::Written(next) = set_pattern_tag("正文\n", "套路", "套路/反常识")
        else {
            panic!("应当写入");
        };
        assert_eq!(
            luna_sync_core::tags::frontmatter_tags(&next),
            ["套路/反常识"]
        );
        assert_eq!(
            set_pattern_tag("---\ntags: [套路/悬念]\n---\n", "套路", "套路/悬念"),
            NoteTagEdit::Unchanged
        );
    }

    #[test]
    fn pattern_names_become_valid_tags_or_nothing() {
        assert_eq!(pattern_tag("套路", " #悬念 "), Some("套路/悬念".into()));
        assert_eq!(
            pattern_tag("套路", "反 常识/型"),
            Some("套路/反-常识-型".into())
        );
        assert_eq!(pattern_tag("套路", "  "), None);
        assert_eq!(pattern_tag("套路", "？！"), None);
    }

    #[test]
    fn swipe_entries_carry_the_snippet_the_first_why_sentence_and_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                "Creator/Swipe/好开头.md",
                "---\nluna:\n  type: swipe\ntags: [套路/悬念]\n---\n## 片段\n\n「你以为……其实」\n\n## 为什么好\n\n先立一个错的常识。再推翻它！\n\n## 来源\n\n- https://example.com\n",
            )
            .unwrap();
        vault
            .write(
                "Creator/Swipe/旧条目.md",
                "---\nluna:\n  type: swipe\n---\n只有一行字\n",
            )
            .unwrap();
        vault.write("Notes/普通.md", "不是灵感\n").unwrap();
        vault.rebuild_index().unwrap();
        let mut entries = vault.swipe_entries(NAMES).unwrap();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "好开头");
        assert_eq!(entries[0].snippet.as_deref(), Some("「你以为……其实」"));
        assert_eq!(entries[0].why.as_deref(), Some("先立一个错的常识。"));
        assert_eq!(entries[0].patterns, ["悬念"]);
        assert_eq!(entries[1].snippet.as_deref(), Some("只有一行字"));
        assert_eq!(entries[1].why, None);
        assert!(entries[1].patterns.is_empty());
    }
}
