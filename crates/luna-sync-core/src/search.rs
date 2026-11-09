//! 全文检索的**查询语法与命中定位**（PRD-06-031，Obsidian 搜索面板对标；纯逻辑，无 IO）。桌面的搜索视图与
//! 跨库替换、网页版在浏览器里的全文搜索（PRD-21-041，ADR-177 ⑥）用的是这同一份：哪些词算命中、命中落在哪一行
//! 哪几个字、片段怎么切。
//!
//! 与桌面 `luna_vault::index::Query`（结构化列表查询：按类型/标签列出，供功能视图与快速切换器
//! 消费）分工明确——本模块面向**搜索视图**：解析一行查询串、把 Vault 的候选笔记逐行定位
//! 到**具体命中区间**，并据此支撑「跨库查找替换」（PRD-06-014）。
//!
//! ## 三条口径（都是刻意的）
//!
//! 1. **作用域 = 正文**：frontmatter 一律不搜也不改（区段定位同 [`crate::frontmatter::find_frontmatter`]）。
//!    用户顶层 `tags` 只读不写是 Vault 的既有契约（§4.4 round-trip），跨库替换若能改写
//!    YAML 头，等于从另一条路径破坏它。标签检索有 `tag:` 过滤器，不需要正文匹配代劳。
//! 2. **时间码是禁区**：`[HH:MM:SS.mmm]` 标记不进 FTS（桌面 `luna_vault::parse::clean_for_fts`），
//!    命中定位也不匹配它（[`crate::scaffold::timecode_spans`]），两处一个口径；顺带保证
//!    替换绝不会改坏转写笔记的回放锚点。
//! 3. **索引只做粗筛、文件读取才是权威**：桌面 SQLite 侧用 FTS/LIKE 取一个**超集**候选（网页是内存里的折叠副本），
//!    精确的「每个词都在 / 排除词不在 / 命中落在第几行第几列」由读文件的这一遍决定。
//!    索引是派生数据，命中区间必须对得上磁盘上此刻的字节，否则替换会错位。
//!
//! ## 语法（明示边界，不做"看起来支持"的猜测）
//!
//! - 裸词 / `"带空格的短语"`：正文包含（多个词之间是 **AND**）。
//! - `tag:` / `path:` / `file:` / `type:` 过滤器（值可加引号）。
//! - 任意一项前缀 `-` 表示排除。
//! - **不支持** `OR`、正则、`line:`/`section:`/`block:`——未识别的 `foo:bar` 一律按裸词
//!   处理（不静默丢弃），UI 侧给出语法速查，避免"写了没生效还不知道"。

use crate::scaffold::{is_scaffold_line, timecode_spans};

/// frontmatter 之后的正文（无 frontmatter 即全文）：区段定位与写入器同一口径。
fn body_of(content: &str) -> &str {
    crate::frontmatter::find_frontmatter(content)
        .map_or(content, |span| &content[span.body_start..])
}

/// 过滤器字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterField {
    /// 标签（含嵌套语义：`tag:a` 命中 `a` 与 `a/b`）。
    Tag,
    /// 相对路径子串。
    Path,
    /// 文件名（= 标题）子串。
    File,
    /// 笔记类型（`note`/`quick-note`/`recording`/`transcript`/`chat`）。
    Type,
}

impl FilterField {
    fn parse(key: &str) -> Option<Self> {
        match key.to_ascii_lowercase().as_str() {
            "tag" => Some(Self::Tag),
            "path" => Some(Self::Path),
            "file" => Some(Self::File),
            "type" => Some(Self::Type),
            _ => None,
        }
    }
}

/// 一条字段过滤器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub field: FilterField,
    pub value: String,
    pub negated: bool,
}

/// 一个正文检索词（短语与裸词在匹配上等价，只是分词方式不同）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub text: String,
    pub negated: bool,
}

/// 解析后的查询。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub terms: Vec<Term>,
    pub filters: Vec<Filter>,
    /// 区分大小写（UI 开关，不属于语法）。
    pub match_case: bool,
}

/// 结果笔记数上限（超出即 `truncated`，UI 提示缩小范围）。
pub const MAX_RESULT_NOTES: usize = 200;
/// 单篇呈现的命中行上限（命中总数照实统计，不因截断而少报）。
pub const MAX_LINES_PER_NOTE: usize = 20;
/// 索引粗筛的候选上限（读文件那一遍的工作量天花板）。
pub const CANDIDATE_LIMIT: usize = 400;

impl SearchQuery {
    /// 解析查询串。空串 / 只有空白 → 空查询（[`SearchQuery::is_empty`]）。
    #[must_use]
    pub fn parse(input: &str, match_case: bool) -> Self {
        let mut query = Self {
            match_case,
            ..Default::default()
        };
        for raw in tokenize(input) {
            let (negated, rest) = match raw.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, raw.as_str()),
            };
            match split_field(rest) {
                Some((field, value)) if !value.is_empty() => {
                    let value = if field == FilterField::Tag {
                        value.trim_start_matches('#').to_string()
                    } else {
                        value
                    };
                    if value.is_empty() {
                        continue;
                    }
                    query.filters.push(Filter {
                        field,
                        value,
                        negated,
                    });
                }
                // 未识别的 `foo:bar` 与普通词一样按正文检索（不静默丢弃整段输入）。
                _ => {
                    let text = unquote(rest);
                    if !text.is_empty() {
                        query.terms.push(Term { text, negated });
                    }
                }
            }
        }
        query
    }

    /// 无任何有效条件（不应发起搜索）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.filters.is_empty()
    }

    /// 正向检索词（笔记须**全部**包含）。
    pub fn positive_terms(&self) -> impl Iterator<Item = &str> {
        self.terms
            .iter()
            .filter(|t| !t.negated)
            .map(|t| t.text.as_str())
    }

    /// 唯一的正向检索词——**替换的前提**。
    ///
    /// 多个检索词时"把什么替换掉"没有唯一答案（`a b` → 替换 a？b？还是"a b"？），
    /// 与其猜一个，不如要求用户把范围收敛到一个词，过滤器照常可用于限定范围。
    #[must_use]
    pub fn sole_positive_term(&self) -> Option<&str> {
        let mut positives = self.positive_terms();
        let first = positives.next()?;
        if positives.next().is_some() {
            return None;
        }
        Some(first)
    }

    /// 粗筛用的检索词：取最长的正向词（正向词是 AND 关系，任取其一都是超集）。
    #[must_use]
    pub fn narrowing_term(&self) -> Option<&str> {
        self.positive_terms().max_by_key(|t| t.chars().count())
    }
}

/// 按空白切分，双引号内的空白不切；引号本身不进 token 值。
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_content = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                has_content = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_content {
                    out.push(std::mem::take(&mut current));
                    has_content = false;
                }
            }
            c => {
                current.push(c);
                has_content = true;
            }
        }
    }
    if has_content {
        out.push(current);
    }
    out.into_iter().filter(|t| !t.is_empty()).collect()
}

/// `field:value` → 已识别字段 + 值；否则 `None`。
fn split_field(token: &str) -> Option<(FilterField, String)> {
    let (key, value) = token.split_once(':')?;
    let field = FilterField::parse(key)?;
    Some((field, unquote(value)))
}

fn unquote(value: &str) -> String {
    value.replace('"', "")
}

// ---------------------------------------------------------------------------
// 命中定位
// ---------------------------------------------------------------------------

/// 命中行的一段：`hit=true` 即检索词命中区间（前端直接渲染高亮，不做偏移换算）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MatchSegment {
    pub text: String,
    pub hit: bool,
}

/// 一条命中行。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LineMatch {
    /// 行号（1-based，**相对整篇文件**，含 frontmatter 行）。
    pub line: u32,
    pub segments: Vec<MatchSegment>,
}

/// 一篇笔记的命中。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NoteMatch {
    pub path: String,
    pub title: String,
    pub modified_ms: i64,
    /// 笔记类型 kebab 键——供结果分「笔记 / 资料」两组（ADR-024）；判定在 Rust。
    pub note_type: String,
    /// 是否**记录态**（ADR-086）——供结果分出第三组「记录」；同样判定在 Rust，
    /// 且**不影响排序**（PRD-06-052：分组只回答「这是草稿」，不惩罚它）。
    pub draft: bool,
    /// 命中总数（不因 `lines` 截断而少报）。
    pub match_count: u32,
    pub lines: Vec<LineMatch>,
    /// `lines` 因上限被截断。
    pub lines_truncated: bool,
}

/// 一次搜索的结果集。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub notes: Vec<NoteMatch>,
    pub total_matches: u32,
    /// 候选集或结果数触顶——结果不完整，UI 须如实提示。
    pub truncated: bool,
}

/// 单篇笔记的命中定位结果（不含路径等索引侧信息）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteMatchBody {
    pub match_count: u32,
    pub lines: Vec<LineMatch>,
    pub lines_truncated: bool,
}

/// 在一篇笔记正文中定位命中。不满足查询（缺正向词 / 命中排除词）返回 `None`。
///
/// `max_lines` 只截断**呈现**的行，`match_count` 始终是全量统计。
#[must_use]
pub fn match_note(content: &str, query: &SearchQuery, max_lines: usize) -> Option<NoteMatchBody> {
    let positives: Vec<&str> = query.positive_terms().collect();
    let negatives: Vec<&str> = query
        .terms
        .iter()
        .filter(|t| t.negated)
        .map(|t| t.text.as_str())
        .collect();

    let body = body_of(content);
    let base_line = content[..content.len() - body.len()]
        .bytes()
        .filter(|b| *b == b'\n')
        .count();

    let mut seen = vec![false; positives.len()];
    let mut match_count: u32 = 0;
    let mut lines = Vec::new();
    let mut lines_truncated = false;

    for (offset, raw) in body.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let searchable = SearchableLine::new(line, query.match_case);

        for needle in &negatives {
            if searchable.find_all(needle).next().is_some() {
                return None;
            }
        }

        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (idx, needle) in positives.iter().enumerate() {
            let mut found = false;
            for range in searchable.find_all(needle) {
                ranges.push(range);
                found = true;
            }
            if found {
                seen[idx] = true;
            }
        }
        if ranges.is_empty() {
            continue;
        }
        let ranges = merge_ranges(ranges);
        match_count += ranges.len() as u32;
        if lines.len() < max_lines {
            lines.push(LineMatch {
                line: (base_line + offset + 1) as u32,
                segments: split_segments(line, &ranges),
            });
        } else {
            lines_truncated = true;
        }
    }

    // 过滤器只筛笔记，正向词必须**每一个**都在这篇笔记里出现过（AND 语义）。
    if seen.iter().any(|hit| !hit) {
        return None;
    }
    Some(NoteMatchBody {
        match_count,
        lines,
        lines_truncated,
    })
}

/// 一篇笔记的替换结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteReplacement {
    /// 替换后的完整文件内容（`dry_run` 时仅用于预览，不写盘）。
    pub content: String,
    pub match_count: u32,
    pub lines: Vec<ReplaceLine>,
    pub lines_truncated: bool,
}

/// 一行的替换预览：`before` 为高亮分段、`after` 为替换后的整行文本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceLine {
    pub line: u32,
    pub before: Vec<MatchSegment>,
    pub after: String,
}

/// 一篇笔记的替换计划（`ReplaceNote` = 索引侧身份 + [`NoteReplacement`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceNote {
    pub path: String,
    pub title: String,
    pub replacement: NoteReplacement,
}

/// 一次跨库替换的计划/结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplacePlan {
    pub notes: Vec<ReplaceNote>,
    pub total_matches: u32,
    /// 是否已写盘（`false` = 预览）。
    pub applied: bool,
    /// 写盘失败的笔记（路径 + 原因）；其余笔记照常完成，不整批回滚。
    pub failures: Vec<(String, String)>,
    /// 候选集触顶——计划不完整，UI 须如实提示。
    pub truncated: bool,
}

/// 在一篇笔记正文中按**唯一检索词**做替换，返回新内容与逐行预览。
///
/// 与 [`match_note`] 共用同一套区间定位（含时间码禁区），故"预览到什么就替换什么"。
#[must_use]
pub fn replace_in_note(
    content: &str,
    needle: &str,
    replacement: &str,
    match_case: bool,
    max_lines: usize,
) -> Option<NoteReplacement> {
    let body = body_of(content);
    let prefix_len = content.len() - body.len();
    let base_line = content[..prefix_len]
        .bytes()
        .filter(|b| *b == b'\n')
        .count();

    let mut out = String::with_capacity(content.len());
    out.push_str(&content[..prefix_len]);
    let mut match_count: u32 = 0;
    let mut lines = Vec::new();
    let mut lines_truncated = false;

    for (offset, raw) in body.split('\n').enumerate() {
        if offset > 0 {
            out.push('\n');
        }
        let eol = if raw.ends_with('\r') { "\r" } else { "" };
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let ranges: Vec<(usize, usize)> = SearchableLine::new(line, match_case)
            .find_all(needle)
            .collect();
        if ranges.is_empty() {
            out.push_str(raw);
            continue;
        }
        let ranges = merge_ranges(ranges);
        match_count += ranges.len() as u32;
        let after = apply_ranges(line, &ranges, replacement);
        if lines.len() < max_lines {
            lines.push(ReplaceLine {
                line: (base_line + offset + 1) as u32,
                before: split_segments(line, &ranges),
                after: after.clone(),
            });
        } else {
            lines_truncated = true;
        }
        out.push_str(&after);
        out.push_str(eol);
    }

    if match_count == 0 {
        return None;
    }
    Some(NoteReplacement {
        content: out,
        match_count,
        lines,
        lines_truncated,
    })
}

/// 把命中区间外的文本与命中文本切成有序分段（前端零业务地渲染高亮）。
fn split_segments(line: &str, ranges: &[(usize, usize)]) -> Vec<MatchSegment> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for &(from, to) in ranges {
        if from > cursor {
            out.push(MatchSegment {
                text: line[cursor..from].to_string(),
                hit: false,
            });
        }
        out.push(MatchSegment {
            text: line[from..to].to_string(),
            hit: true,
        });
        cursor = to;
    }
    if cursor < line.len() {
        out.push(MatchSegment {
            text: line[cursor..].to_string(),
            hit: false,
        });
    }
    out
}

fn apply_ranges(line: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    for &(from, to) in ranges {
        out.push_str(&line[cursor..from]);
        out.push_str(replacement);
        cursor = to;
    }
    out.push_str(&line[cursor..]);
    out
}

/// 多个检索词的命中区间可能重叠（`AB` 与 `BC` 落在 `ABC` 上），合并后再切分段，
/// 否则 `split_segments` 会切出负长度区间。
fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// 检索用的折叠：逐字符规范分解（NFD），不区分大小写时再转小写。查询词与正文过同一个函数。
#[must_use]
pub fn fold(text: &str, match_case: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        fold_char(ch, match_case, &mut out);
    }
    out
}

fn fold_char(ch: char, match_case: bool, out: &mut String) {
    // 两条快路：ASCII 与常用汉字区既不分解、也没有大小写。
    if ch.is_ascii() {
        out.push(if match_case {
            ch
        } else {
            ch.to_ascii_lowercase()
        });
        return;
    }
    if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
        out.push(ch);
        return;
    }
    let mut buf = [0u8; 4];
    let decomposed = icu_normalizer::DecomposingNormalizerBorrowed::new_nfd()
        .normalize(ch.encode_utf8(&mut buf));
    for part in decomposed.chars() {
        if match_case {
            out.push(part);
        } else {
            out.extend(part.to_lowercase());
        }
    }
}

/// 一行的可检索视图：折叠（规范分解 + 大小写）+ 回映表 + 禁区（时间码 / 脚手架行），命中区间以**原始字节
/// 下标**返回。
///
/// 折叠后的字节下标与原始下标并非一一对应（`İ`→`i̇`、`é`→`e`+`◌́`），故随折叠缓冲一起建一张回映表；只做
/// ASCII 折叠虽简单，但会让 `Café` 搜不到 `café`。**逐字符做规范分解**（NFD）让同一个字的两种写法——macOS
/// 文件名常见的分解形与键盘输入的合成形，韩文音节与它的字母序列——折成同一串字节（与名字比较键的 NFC 口径
/// 等价：两段文本 NFC 相等 ⇔ 逐字 NFD 相等）；命中的两端必须落在原文的字边界上，所以「가」不会命中「각」的
/// 前半个字。
///
/// 问答检索（桌面 `luna_vault::retrieve`）复用同一套折叠与禁区判定——问答检索与搜索视图对「哪些字节可以被
/// 命中」必须是同一个答案，各写一份必然漂移。
pub struct SearchableLine {
    folded: String,
    /// `map[i]` = 折叠缓冲第 i 字节对应的原始字节下标；长度 = `folded.len() + 1`。
    map: Vec<usize>,
    match_case: bool,
    forbidden: Vec<(usize, usize)>,
}

impl SearchableLine {
    #[must_use]
    pub fn new(line: &str, match_case: bool) -> Self {
        let mut folded = String::with_capacity(line.len());
        let mut map = Vec::with_capacity(line.len() + 1);
        for (idx, ch) in line.char_indices() {
            fold_char(ch, match_case, &mut folded);
            // 该字符折叠出的每一个字节都回映到它在原文中的起点。
            while map.len() < folded.len() {
                map.push(idx);
            }
        }
        map.push(line.len());
        // 禁区两类，判定都与索引侧同源（ADR-024 / TDD-06 §3，两处一个口径）：
        // - 时间码标记：`clean_for_fts` 剥掉的行内片段；
        // - 结构脚手架整行（转写哨兵 / 对话角色标题）：`clean_for_fts` 整行丢弃的行——
        //   命中它会让「索引筛不出、逐行扫得到」，替换它会毁掉对话笔记的往返解析。
        let forbidden = if is_scaffold_line(line) {
            vec![(0, line.len())]
        } else {
            timecode_spans(line)
        };
        Self {
            folded,
            map,
            match_case,
            forbidden,
        }
    }

    fn fold_needle(&self, needle: &str) -> String {
        fold(needle, self.match_case)
    }

    /// 全部命中的原始字节区间（不重叠推进，跳过与时间码标记相交的命中）。
    pub fn find_all<'a>(&'a self, needle: &str) -> impl Iterator<Item = (usize, usize)> + 'a {
        let folded_needle = self.fold_needle(needle);
        let mut cursor = 0usize;
        std::iter::from_fn(move || {
            if folded_needle.is_empty() {
                return None;
            }
            loop {
                let found = self.folded.get(cursor..)?.find(&folded_needle)? + cursor;
                let end = found + folded_needle.len();
                // 折叠缓冲里 needle 的边界必是字符边界，据此非重叠推进。
                cursor = end;
                // 两端都要落在原文的字边界上：折叠展开出的几个字节只命中其中一部分（`가` 对 `각`、`i` 对
                // `İ`）不算命中。
                let starts_char = found == 0 || self.map[found - 1] != self.map[found];
                let ends_char = end == self.folded.len() || self.map[end] != self.map[end - 1];
                let from = self.map[found];
                let to = self.map[end];
                if !starts_char || !ends_char || to <= from {
                    continue;
                }
                if !self.forbidden.iter().any(|&(a, b)| from < b && a < to) {
                    return Some((from, to));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms_filters_and_negation() {
        let q = SearchQuery::parse(
            "会议 tag:#工作/周报 -草稿 path:\"项目 A\" -type:chat",
            false,
        );
        assert_eq!(
            q.terms,
            vec![
                Term {
                    text: "会议".into(),
                    negated: false
                },
                Term {
                    text: "草稿".into(),
                    negated: true
                },
            ]
        );
        assert_eq!(
            q.filters,
            vec![
                Filter {
                    field: FilterField::Tag,
                    value: "工作/周报".into(),
                    negated: false
                },
                Filter {
                    field: FilterField::Path,
                    value: "项目 A".into(),
                    negated: false
                },
                Filter {
                    field: FilterField::Type,
                    value: "chat".into(),
                    negated: true
                },
            ]
        );
    }

    #[test]
    fn quoted_phrase_keeps_inner_space_and_unknown_field_is_a_term() {
        let q = SearchQuery::parse("\"产品 路线图\" section:标题", false);
        assert_eq!(
            q.terms,
            vec![
                Term {
                    text: "产品 路线图".into(),
                    negated: false
                },
                Term {
                    text: "section:标题".into(),
                    negated: false
                },
            ]
        );
        assert!(q.filters.is_empty());
    }

    #[test]
    fn sole_positive_term_gates_replacement() {
        assert_eq!(
            SearchQuery::parse("旧词 tag:工作 -废弃", false).sole_positive_term(),
            Some("旧词")
        );
        assert_eq!(
            SearchQuery::parse("旧词 新词", false).sole_positive_term(),
            None
        );
        assert_eq!(
            SearchQuery::parse("tag:工作", false).sole_positive_term(),
            None
        );
    }

    #[test]
    fn matches_lines_and_counts_all_occurrences() {
        let content = "---\ntitle: x\n---\n第一行有会议\n无关\n会议与会议\n";
        let q = SearchQuery::parse("会议", false);
        let body = match_note(content, &q, 10).expect("命中");
        assert_eq!(body.match_count, 3);
        // frontmatter 占 3 行 → 正文首行是第 4 行。
        assert_eq!(
            body.lines.iter().map(|l| l.line).collect::<Vec<_>>(),
            vec![4, 6]
        );
        assert_eq!(
            body.lines[0].segments,
            vec![
                MatchSegment {
                    text: "第一行有".into(),
                    hit: false
                },
                MatchSegment {
                    text: "会议".into(),
                    hit: true
                },
            ]
        );
    }

    #[test]
    fn frontmatter_is_never_searched() {
        let content = "---\ntags:\n  - 机密\n---\n正文\n";
        assert!(match_note(content, &SearchQuery::parse("机密", false), 10).is_none());
    }

    #[test]
    fn timecode_markers_are_not_matched() {
        let content = "[00:01:23.400] 讲到 00:01:23 这个点\n";
        let q = SearchQuery::parse("00:01:23", false);
        let body = match_note(content, &q, 10).expect("正文里的裸时间字符串仍可命中");
        // 行首标记内的那次不算，只剩正文里的一次。
        assert_eq!(body.match_count, 1);
    }

    #[test]
    fn all_positive_terms_must_appear_and_negatives_exclude() {
        let content = "第一行 甲\n第二行 乙\n";
        assert!(match_note(content, &SearchQuery::parse("甲 乙", false), 10).is_some());
        assert!(match_note(content, &SearchQuery::parse("甲 丙", false), 10).is_none());
        assert!(match_note(content, &SearchQuery::parse("甲 -乙", false), 10).is_none());
    }

    #[test]
    fn case_folding_covers_non_ascii_and_keeps_original_offsets() {
        let content = "Le Café ouvre\n";
        let body = match_note(content, &SearchQuery::parse("café", false), 10).expect("命中");
        assert_eq!(
            body.lines[0].segments,
            vec![
                MatchSegment {
                    text: "Le ".into(),
                    hit: false
                },
                MatchSegment {
                    text: "Café".into(),
                    hit: true
                },
                MatchSegment {
                    text: " ouvre".into(),
                    hit: false
                },
            ]
        );
        assert!(match_note(content, &SearchQuery::parse("café", true), 10).is_none());
    }

    #[test]
    fn overlapping_ranges_merge_instead_of_producing_broken_segments() {
        let content = "ABC\n";
        let q = SearchQuery::parse("AB BC", false);
        let body = match_note(content, &q, 10).expect("命中");
        assert_eq!(
            body.lines[0].segments,
            vec![MatchSegment {
                text: "ABC".into(),
                hit: true
            }]
        );
    }

    #[test]
    fn line_cap_truncates_display_but_not_count() {
        let content = "词\n词\n词\n";
        let body = match_note(content, &SearchQuery::parse("词", false), 2).expect("命中");
        assert_eq!(body.match_count, 3);
        assert_eq!(body.lines.len(), 2);
        assert!(body.lines_truncated);
    }

    #[test]
    fn replace_rewrites_body_only_and_previews_each_line() {
        let content = "---\ntitle: 旧词\n---\n旧词开头\n无关\n";
        let applied = replace_in_note(content, "旧词", "新词", false, 10).expect("有命中");
        assert_eq!(applied.match_count, 1);
        assert!(!applied.lines_truncated);
        // frontmatter 里的「旧词」原样保留。
        assert_eq!(applied.content, "---\ntitle: 旧词\n---\n新词开头\n无关\n");
        assert_eq!(applied.lines[0].line, 4);
        assert_eq!(applied.lines[0].after, "新词开头");
    }

    #[test]
    fn replace_skips_timecode_markers() {
        let content = "[00:01:23] 00:01:23 提到\n";
        let applied = replace_in_note(content, "00:01:23", "X", false, 10).expect("有命中");
        assert_eq!(applied.match_count, 1);
        assert_eq!(applied.content, "[00:01:23] X 提到\n");
    }

    #[test]
    fn replace_without_match_returns_none() {
        assert!(replace_in_note("正文\n", "缺失", "X", false, 10).is_none());
    }

    #[test]
    fn scaffold_lines_are_whole_line_forbidden_for_match_and_replace() {
        // ADR-024 口径闭环：`clean_for_fts` 整行丢弃的脚手架行，命中定位与替换也必须
        // 整行跳过——否则替换 "User" 会把对话笔记的 `## User` 结构行改坏，往返解析报废。
        let content = "## User\n\nUser 研究计划\n\n## Assistant\n\n结论如下\n";
        let hits = match_note(content, &SearchQuery::parse("User", false), 10).expect("正文命中");
        assert_eq!(hits.match_count, 1, "脚手架行上的 User 不计命中");
        assert_eq!(hits.lines.len(), 1);
        assert_eq!(hits.lines[0].line, 3, "命中只落在正文那一行");

        let applied = replace_in_note(content, "User", "Reader", false, 10).expect("正文命中");
        assert_eq!(applied.match_count, 1, "只替换正文里的那一处");
        assert!(applied.content.contains("## User\n"), "结构行必须原样保留");
        assert!(applied.content.contains("Reader 研究计划"));

        // 只出现在脚手架行时，整篇视为不命中——与 FTS「筛不出」保持同一答案。
        assert!(match_note(
            "## Assistant\n\n好的\n",
            &SearchQuery::parse("Assistant", false),
            10
        )
        .is_none());
    }

    #[test]
    fn narrowing_term_picks_the_longest_positive() {
        let q = SearchQuery::parse("甲 产品路线图 -乙", false);
        assert_eq!(q.narrowing_term(), Some("产品路线图"));
        assert_eq!(SearchQuery::parse("tag:工作", false).narrowing_term(), None);
    }

    #[test]
    fn canonical_spellings_match_each_other_on_character_boundaries() {
        // 分解形的「é」与韩文字母序列（macOS 文件名、某些输入法）与合成形互相搜得到，高亮落在原文的整个字上。
        let nfd = "Cafe\u{301} 회\u{1100}\u{1175}\n";
        let body = match_note(nfd, &SearchQuery::parse("café 기", false), 10).expect("命中");
        let hits: Vec<&str> = body.lines[0]
            .segments
            .iter()
            .filter(|s| s.hit)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["Cafe\u{301}", "\u{1100}\u{1175}"]);
        // 只命中一个字的前半截不算：「가」不命中「각」，「a가」也不命中「a각」。
        assert!(match_note("각\n", &SearchQuery::parse("가", false), 10).is_none());
        assert!(match_note("a각\n", &SearchQuery::parse("a가", false), 10).is_none());
        assert!(match_note("안녕하세요\n", &SearchQuery::parse("안녕", false), 10).is_some());
        assert!(match_note("İstanbul\n", &SearchQuery::parse("i", false), 10).is_none());
    }
}
