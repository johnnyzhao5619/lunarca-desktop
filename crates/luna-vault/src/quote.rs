//! 引文卡片（ADR-162 ⑭，PRD-19-007）：从转写稿或笔记里摘一段，记成项目研究笔记「引文」一节里
//! 的一行 `> 引文——[来源 · m:ss](相对路径#t=毫秒) ^q-xxxxxx`。
//!
//! 卡片不是单独的笔记：一行即一张，行尾的块标记让脚本可以 `![[研究#^q-…]]` 嵌入这一行（D5 的
//! 嵌入只取带标记的那一行）。时刻取自原文的时间码行；链接片段 `#t=` / `#L` 由
//! [`link_location`] 解析回「到哪一行、从哪一刻播放」。

use crate::creator::ProjectCard;
use crate::error::Result;
use crate::frontmatter::{body_base_line, body_of};
use crate::timecode::parse_timecode;
use crate::vault::Vault;

/// 块标记前缀：引文卡片的标记一律是 `q-` + 6 位小写字母数字。
pub const QUOTE_ID_PREFIX: &str = "q-";

/// 选区 → 一行引文：每行去掉行首时间码与引用符号，空行丢掉，并成一行、压掉多余空白。
#[must_use]
pub fn quote_text(selection: &str) -> String {
    let parts: Vec<String> = selection
        .lines()
        .map(|line| {
            let line = line.trim();
            let line = parse_timecode(line).map_or(line, |(_, rest)| rest).trim();
            line.trim_start_matches('>').trim().to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect();
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 整篇文件里第 `line` 行（1 起，含 frontmatter）或它之前最近的时间码行的时刻（毫秒）。
#[must_use]
pub fn time_at_line(content: &str, line: u32) -> Option<u64> {
    let lines: Vec<&str> = content.lines().collect();
    let last = usize::try_from(line).ok()?.checked_sub(1)?;
    lines
        .get(..=last.min(lines.len().checked_sub(1)?))?
        .iter()
        .rev()
        .find_map(|line| parse_timecode(line.trim_start()))
        .and_then(|(at, _)| u64::try_from(at.as_millis()).ok())
}

/// 整篇文件里第 `line` 行（1 起，含 frontmatter）或它之前最近一个 PDF 页标的页码（ADR-164 ⑧）。
/// 页标认的是链接锚 `#page=N)`，与页标文字的语言无关；没有页标为 `None`。
#[must_use]
pub fn page_at_line(content: &str, line: u32) -> Option<u32> {
    let lines: Vec<&str> = content.lines().collect();
    let last = usize::try_from(line).ok()?.checked_sub(1)?;
    lines
        .get(..=last.min(lines.len().checked_sub(1)?))?
        .iter()
        .rev()
        .find_map(|line| {
            let line = line.trim();
            if !line.starts_with('[') {
                return None;
            }
            let (_, rest) = line.rsplit_once("#page=")?;
            rest.strip_suffix(')')?.parse().ok()
        })
}

/// 时刻 `ms` 落在哪一行：起点不晚于它的最后一个时间码行（1 起，含 frontmatter）。没有时间码行
/// 为 `None`。
#[must_use]
pub fn line_at_time(content: &str, ms: u64) -> Option<u32> {
    let base = body_base_line(content);
    let mut found = None;
    for (offset, line) in body_of(content).lines().enumerate() {
        if let Some((at, _)) = parse_timecode(line.trim_start()) {
            if u64::try_from(at.as_millis()).unwrap_or(u64::MAX) <= ms {
                found = u32::try_from(base + offset + 1).ok();
            } else if found.is_some() {
                break;
            }
        }
    }
    found
}

/// 引文在原文里落在哪一行：先看编辑器报来的行（`hint`，1 起）里有没有引文开头那几个字，
/// 没有就找第一处有的行；都没有为 `None`（文件在这期间改过）。开头只取 8 个字：选区常常
/// 跨行，取多了会跨过换行处，哪一行都对不上。
#[must_use]
pub fn locate_quote(content: &str, hint: u32, quote: &str) -> Option<u32> {
    let probe: String = quote.chars().take(8).collect();
    if probe.is_empty() {
        return None;
    }
    let lines: Vec<&str> = content.lines().collect();
    let contains = |line: &str| quote_text(line).contains(&probe);
    if let Some(line) = usize::try_from(hint)
        .ok()
        .and_then(|h| h.checked_sub(1))
        .and_then(|i| lines.get(i))
    {
        if contains(line) {
            return Some(hint);
        }
    }
    lines
        .iter()
        .position(|line| contains(line))
        .and_then(|i| u32::try_from(i + 1).ok())
}

/// `m:ss` / `h:mm:ss`（与章节时间戳同一写法）。
#[must_use]
pub fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 一张卡片的那一行。`href` 已是相对路径（含片段），`dash` 是引文与出处之间的连接（随界面语言）。
#[must_use]
pub fn quote_line(quote: &str, label: &str, href: &str, dash: &str, id: &str) -> String {
    format!("> {quote}{dash}[{label}]({href}) ^{id}")
}

/// 一篇笔记里已有的引文卡片。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteCard {
    /// 块标记（不含 `^`）。
    pub id: String,
    /// 给人看的那一行：引文 + 出处的文字（链接只留标题，不含 `> `、链接地址与块标记）。
    pub text: String,
}

/// `[文字](地址)` → `文字`（卡片列表给人看，地址是给点的）。
fn without_link_targets(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let after = &rest[open..];
        let Some(close) = after.find("](") else {
            break;
        };
        let Some(end) = after[close + 2..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&after[1..close]);
        rest = &after[close + 3 + end..];
    }
    out.push_str(rest);
    out
}

/// 读出一篇笔记里的引文卡片（`> …… ^q-xxxxxx` 行），按出现顺序。
#[must_use]
pub fn quote_cards(content: &str) -> Vec<QuoteCard> {
    body_of(content)
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let rest = line.strip_prefix('>')?;
            let (text, id) = rest.rsplit_once(" ^")?;
            (id.starts_with(QUOTE_ID_PREFIX)
                && id.len() > QUOTE_ID_PREFIX.len()
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .then(|| QuoteCard {
                id: id.to_owned(),
                text: without_link_targets(text.trim()),
            })
        })
        .collect()
}

/// 一个在 `content` 里还没用过的块标记（`seed` 给随机性，调用方传时间或 UUID 的数字）。
#[must_use]
pub fn new_quote_id(content: &str, seed: u128) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut n = seed;
    loop {
        let mut id = String::from(QUOTE_ID_PREFIX);
        let mut m = n;
        for _ in 0..6 {
            id.push(ALPHABET[usize::try_from(m % 36).unwrap_or(0)] as char);
            m /= 36;
        }
        if !content.contains(&format!("^{id}")) {
            return id;
        }
        n = n.wrapping_mul(31).wrapping_add(7);
    }
}

/// 标准 md 链接片段指向的位置：`#L<行>` 或 `#t=<毫秒>`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkLocation {
    /// 到哪一行（1 起，含 frontmatter）。
    pub line: Option<u32>,
    /// 从哪一刻播放（毫秒）。
    pub seek_ms: Option<u64>,
}

/// 解析链接片段（不带 `#`）：`L12` → 第 12 行；`t=83456` → 时间码落在的那一行并从那一刻播放。
/// 不认识的片段两项都是 `None`（只打开笔记）。
#[must_use]
pub fn link_location(content: &str, fragment: &str) -> LinkLocation {
    if let Some(line) = fragment
        .strip_prefix('L')
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| *n > 0)
    {
        return LinkLocation {
            line: Some(line),
            seek_ms: None,
        };
    }
    if let Some(ms) = fragment
        .strip_prefix("t=")
        .and_then(|n| n.parse::<u64>().ok())
    {
        return LinkLocation {
            line: line_at_time(content, ms),
            seek_ms: Some(ms),
        };
    }
    LinkLocation {
        line: None,
        seek_ms: None,
    }
}

impl Vault {
    /// `rel` 属于哪些视频项目（ADR-162 ⑫ 同一判定：在项目文件夹里、项目主页链到它、或它链到
    /// 项目主页），最近改动的在前。
    ///
    /// # Errors
    /// 派生索引查询失败时返回错误。
    pub fn projects_of(&self, rel: &str) -> Result<Vec<ProjectCard>> {
        let outgoing = self.outgoing_links(rel)?;
        let linked_from_rel: Vec<String> = self
            .resolve_wikilinks(rel, &outgoing)?
            .into_iter()
            .flatten()
            .collect();
        let mut projects = Vec::new();
        for card in self.video_projects()? {
            if card.path == rel {
                continue;
            }
            let in_folder = card
                .path
                .rsplit_once('/')
                .is_some_and(|(folder, _)| rel.starts_with(&format!("{folder}/")));
            let home_links = || -> Result<bool> {
                let targets = self.outgoing_links(&card.path)?;
                Ok(self
                    .resolve_wikilinks(&card.path, &targets)?
                    .iter()
                    .flatten()
                    .any(|target| target == rel))
            };
            if in_folder || linked_from_rel.contains(&card.path) || home_links()? {
                projects.push(card);
            }
        }
        projects.sort_by_key(|card| std::cmp::Reverse(card.modified_ms));
        Ok(projects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = "---\nluna:\n  type: transcript\n---\n<!-- luna:transcript -->\n[00:00:01.000] 大家好。\n[00:01:23.456] 很多人写完稿子一念，\n才发现句子太长。\n[00:02:00.000] 下一段。\n<!-- /luna:transcript -->\n";

    #[test]
    fn a_selection_becomes_one_clean_line() {
        assert_eq!(
            quote_text("[00:01:23.456] 很多人写完稿子一念，\n\n> 才发现  句子太长。\n"),
            "很多人写完稿子一念， 才发现 句子太长。"
        );
    }

    #[test]
    fn time_and_line_map_both_ways_through_timecode_lines() {
        // 第 8 行（「才发现句子太长」）之前最近的时间码是 1:23.456。
        assert_eq!(time_at_line(TRANSCRIPT, 8), Some(83_456));
        assert_eq!(time_at_line(TRANSCRIPT, 6), Some(1_000));
        assert_eq!(time_at_line(TRANSCRIPT, 3), None);
        assert_eq!(line_at_time(TRANSCRIPT, 83_456), Some(7));
        assert_eq!(line_at_time(TRANSCRIPT, 90_000), Some(7));
        assert_eq!(line_at_time(TRANSCRIPT, 0), None);
        assert_eq!(clock(83_456), "1:23");
        assert_eq!(clock(3_723_000), "1:02:03");
    }

    #[test]
    fn the_quote_is_found_at_the_hint_or_searched_for() {
        assert_eq!(
            locate_quote(TRANSCRIPT, 7, "很多人写完稿子一念， 才发现"),
            Some(7)
        );
        // 编辑器报的行不对（文件改过）：按字找。
        assert_eq!(locate_quote(TRANSCRIPT, 2, "很多人写完稿子一念"), Some(7));
        assert_eq!(locate_quote(TRANSCRIPT, 7, "不存在的话"), None);
    }

    #[test]
    fn cards_round_trip_through_their_line() {
        let line = quote_line(
            "句子太长。",
            "访谈 · 1:23",
            "访谈.md#t=83456",
            "——",
            "q-abc123",
        );
        assert_eq!(
            line,
            "> 句子太长。——[访谈 · 1:23](访谈.md#t=83456) ^q-abc123"
        );
        let note = format!("## 引文\n\n{line}\n\n> 普通引用不算 ^x-1\n");
        assert_eq!(
            quote_cards(&note),
            [QuoteCard {
                id: "q-abc123".into(),
                text: "句子太长。——访谈 · 1:23".into(),
            }]
        );
        let id = new_quote_id(&note, 0);
        assert!(id.starts_with("q-") && id.len() == 8 && id != "q-abc123");
    }

    #[test]
    fn link_fragments_resolve_to_a_line_and_a_time() {
        assert_eq!(
            link_location(TRANSCRIPT, "t=83456"),
            LinkLocation {
                line: Some(7),
                seek_ms: Some(83_456)
            }
        );
        assert_eq!(
            link_location(TRANSCRIPT, "L5"),
            LinkLocation {
                line: Some(5),
                seek_ms: None
            }
        );
        assert_eq!(
            link_location(TRANSCRIPT, "某个标题"),
            LinkLocation {
                line: None,
                seek_ms: None
            }
        );
    }

    #[test]
    fn a_note_belongs_to_projects_by_folder_or_links() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "- [[外部访谈]]\n", &[])
            .unwrap();
        vault
            .create_video_project("Creator/Q", "Q", "", &[])
            .unwrap();
        vault.write("Creator/P/成片.md", "正文\n").unwrap();
        vault.write("访谈/外部访谈.md", "正文\n").unwrap();
        vault.write("访谈/提到Q.md", "给 [[Q]] 用\n").unwrap();
        vault.write("访谈/无关.md", "正文\n").unwrap();
        vault.rebuild_index().unwrap();
        let titles = |rel: &str| -> Vec<String> {
            vault
                .projects_of(rel)
                .unwrap()
                .into_iter()
                .map(|c| c.title)
                .collect()
        };
        assert_eq!(titles("Creator/P/成片.md"), ["P"]);
        assert_eq!(titles("访谈/外部访谈.md"), ["P"]);
        assert_eq!(titles("访谈/提到Q.md"), ["Q"]);
        assert!(titles("访谈/无关.md").is_empty());
    }
    #[test]
    fn page_at_line_finds_the_nearest_marker_above() {
        let content = "---\nluna:\n  type: document\n---\n# 论文\n\n[第 1 页](assets/a.pdf#page=1)\n\n第一页的话\n\n[Page 2](assets/a%20b.pdf#page=2)\n\n第二页的话\n";
        assert_eq!(page_at_line(content, 5), None, "页标之前");
        assert_eq!(page_at_line(content, 9), Some(1));
        assert_eq!(page_at_line(content, 13), Some(2), "页标文字是什么语言都认");
        assert_eq!(
            page_at_line("正文 [x](y#page=3)\n", 1),
            None,
            "不在行首的不算页标"
        );
    }
}
