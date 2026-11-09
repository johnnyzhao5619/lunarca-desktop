//! 笔记的标签怎么读（PRD-06-033 / 035 / 046；纯逻辑，无 IO）：frontmatter 顶层 `tags`、`luna.ai_tags` 与正文
//! 内联 `#标签`。桌面的派生索引、标签改名与网页版的标签列表（PRD-21-042，ADR-177 ⑥）用的是这同一份读法——
//! 两套判定的结局是「桌面上有的标签网页上没有」，或改名改到了索引不认的地方。

/// frontmatter 之后的正文（无 frontmatter 即全文）：区段定位与写入器同一口径。
fn body_of(content: &str) -> &str {
    crate::frontmatter::find_frontmatter(content)
        .map_or(content, |span| &content[span.body_start..])
}

/// 读笔记全部标签（去重保序）：frontmatter 顶层 `tags` ∪ `luna.ai_tags`（AI 关系标签，
/// PRD-06-035）∪ 正文内联 `#tag`（Obsidian 兼容，PRD-06-033）。frontmatter 顶层 `tags`
/// 只读不写（§2.3）；内联解析跳过代码块。
#[must_use]
pub fn extract_tags(content: &str) -> Vec<TaggedTerm> {
    let mut out: Vec<TaggedTerm> = Vec::new();
    let mut push = |tag: String, source: TagSource| {
        // 同一标签重复出现只记一次，且**先到的来源胜出**——下面的顺序保证"用户来源优先"。
        if !out.iter().any(|t| t.tag.eq_ignore_ascii_case(&tag)) {
            out.push(TaggedTerm { tag, source });
        }
    };
    if let Some(value) = frontmatter_value(content) {
        for tag in yaml_tag_list(value.get("tags")) {
            push(tag, TagSource::User);
        }
    }
    // 内联 `#tag` 排在 `ai_tags` **之前**：它同样是用户亲手写的，用户与 AI 都提到
    // 同一个标签时，事实是"用户在用这个标签"——AI 恰好也提到不该改变它的归属。
    for tag in extract_inline_tags(body_of(content)) {
        push(tag, TagSource::User);
    }
    if let Some(value) = frontmatter_value(content) {
        // AI 关系标签与用户标签**同池**入索引（标签面板/过滤/关系图谱同源，PRD-06-035）；
        // 来源只作区分呈现，不改变"同池"这一契约。
        for tag in yaml_tag_list(value.get("luna").and_then(|l| l.get("ai_tags"))) {
            push(tag, TagSource::Ai);
        }
    }
    out
}

/// 一条标签及其**来源**（PRD-06-046）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaggedTerm {
    pub tag: String,
    pub source: TagSource,
}

/// 标签来源。**刻意只有两档**：产品诉求是"哪些是机器给的"，而 frontmatter 顶层 `tags`
/// 与正文内联 `#tag` 在这个问题上是同一答案（都是用户亲手写的）——存三档而只用两档，
/// 等于给未来挖一个"第三档在界面上该长什么样"没人回答得了的坑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagSource {
    /// 用户写的（frontmatter 顶层 `tags` ∪ 正文内联 `#tag`）。
    User,
    /// LLM 提炼的（`luna.ai_tags`，PRD-06-035）。
    Ai,
}

impl TagSource {
    /// 派生索引列的字面值（单一事实源）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Ai => "ai",
        }
    }
}

/// 写在 frontmatter 顶层 `tags` 里的标签（与 [`extract_tags`] 同一读法）——只有这些能被
/// 单篇「移除标签」改掉；正文 `#标签` 与 AI 标签各有各的出口（ADR-135）。
#[must_use]
pub fn frontmatter_tags(content: &str) -> Vec<String> {
    frontmatter_value(content)
        .map(|value| yaml_tag_list(value.get("tags")))
        .unwrap_or_default()
}

/// 解析 frontmatter 为 YAML 值（只读，供标签与别名的读法用）。容错：无 frontmatter（含未闭合）/ 解析失败
/// 返回 `None`。区段定位与写入器同一口径（[`crate::frontmatter::frontmatter_inner_range`]）。
#[must_use]
pub fn frontmatter_value(content: &str) -> Option<serde_yaml_ng::Value> {
    let inner = crate::frontmatter::frontmatter_inner_range(content)?;
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content[inner]).ok()
}

/// 把一个 YAML 标签值（list 或逗号串）读成标签列表。缺失/类型不符返回空。
fn yaml_tag_list(value: Option<&serde_yaml_ng::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml_ng::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(serde_yaml_ng::Value::String(s)) => s
            .split([',', ' '])
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// 判断字符是否可出现在内联标签体内（Obsidian 规则：字母/数字/`_`/`-`/`/`；
/// 含 CJK 等非 ASCII 字母）。
pub fn is_tag_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '/'
}

/// 解析正文内联 `#tag`（PRD-06-033 可扩展项）。规则对齐 Obsidian：
/// `#` 前必须是行首或空白；标签体至少含一个非数字字符（排除 `#123` 式锚点/序号）；
/// 跳过 fenced code block 与行内 code span；`#` 后紧跟空白（Markdown 标题）自然不匹配。
#[must_use]
pub fn extract_inline_tags(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in inline_tag_spans(body) {
        let tag = &body[span];
        if !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// 正文内联 `#tag` 的**位置**：每个标签体（不含 `#`、不含尾部 `/`）在 `body` 里的字节区间，
/// 按出现顺序。索引读标签与改名改写标签用的是**同一个**扫描（ADR-135）——两套判定的结局是
/// 改名改到了索引不认的地方，或漏改了索引认的地方。跳过围栏代码块与行内代码。
#[must_use]
pub fn inline_tag_spans(body: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut offset = 0usize;
    for raw in body.split_inclusive('\n') {
        let line_start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut prev: Option<char> = None;
        let mut in_code_span = false;
        let mut chars = line.char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if ch == '`' {
                in_code_span = !in_code_span;
                prev = Some(ch);
                continue;
            }
            if in_code_span {
                prev = Some(ch);
                continue;
            }
            if ch == '#' && prev.is_none_or(char::is_whitespace) {
                let rest = &line[idx + ch.len_utf8()..];
                let end = rest.find(|c: char| !is_tag_char(c)).unwrap_or(rest.len());
                // 尾部 `/` 不计入标签体；首字符为 `/` 视为非标签（与前端 scanLineForTags 同构）。
                let tag = rest[..end].trim_end_matches('/');
                if !tag.is_empty()
                    && !tag.starts_with('/')
                    && tag.chars().any(|c| !c.is_ascii_digit())
                {
                    let tag_start = line_start + idx + ch.len_utf8();
                    out.push(tag_start..tag_start + tag.len());
                    // 跳过已消费的标签体，避免 `#a/b` 内部再触发匹配。
                    while chars
                        .peek()
                        .is_some_and(|(i, _)| *i < idx + ch.len_utf8() + end)
                    {
                        chars.next();
                    }
                    prev = Some('x');
                    continue;
                }
            }
            prev = Some(ch);
        }
    }
    out
}

/// 一个标签过滤器盖不盖得住这个标签：同名，或是它的子孙（`a` 盖住 `a` 与 `a/b`）；不分 ASCII 大小写，与
/// [`extract_tags`] 的去重同一口径（桌面搜索的 `tag:` 过滤器在 SQL 里是同一语义）。
#[must_use]
pub fn tag_covers(filter: &str, tag: &str) -> bool {
    let filter = filter.trim_start_matches('#');
    tag.len() >= filter.len()
        && tag.is_char_boundary(filter.len())
        && tag[..filter.len()].eq_ignore_ascii_case(filter)
        && (tag.len() == filter.len() || tag[filter.len()..].starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试助手：只比对标签串（来源另有专测，避免每条断言都写一遍 `TaggedTerm{..}`）。
    fn tag_names(terms: Vec<TaggedTerm>) -> Vec<String> {
        terms.into_iter().map(|t| t.tag).collect()
    }

    #[test]
    fn extracts_frontmatter_tags() {
        let content = "---\ntitle: x\ntags:\n  - 工作\n  - 重要\n---\n正文";
        assert_eq!(tag_names(extract_tags(content)), vec!["工作", "重要"]);
        let inline = "---\ntags: 工作, 重要\n---\n正文";
        assert_eq!(tag_names(extract_tags(inline)), vec!["工作", "重要"]);
        assert!(extract_tags("无 frontmatter").is_empty());
    }

    // ── 内联 #tag 解析（PRD-06-033）─────────────────────────────────────────

    #[test]
    fn extracts_inline_tags_obsidian_rules() {
        let body = "开头 #工作 中间 #project/luna 结尾\n下一行 #re-check_2 ok";
        assert_eq!(
            extract_inline_tags(body),
            vec!["工作", "project/luna", "re-check_2"]
        );
    }

    #[test]
    fn inline_tags_reject_headings_numbers_and_midword() {
        // 标题（# 后空白）、纯数字（#123）、词中 #（a#b）都不是标签。
        assert!(extract_inline_tags("# 标题\n序号 #123 词中a#b").is_empty());
    }

    #[test]
    fn inline_tags_skip_code_regions() {
        let body = "```\n#in-fence\n```\n行内 `#in-span` 之外 #real";
        assert_eq!(extract_inline_tags(body), vec!["real"]);
    }

    #[test]
    fn inline_tags_dedupe_and_merge_with_frontmatter() {
        let content = "---\ntags: [工作]\n---\n正文 #工作 #复盘 再次 #复盘";
        assert_eq!(tag_names(extract_tags(content)), vec!["工作", "复盘"]);
    }

    #[test]
    fn extract_tags_merges_luna_ai_tags() {
        // PRD-06-035：`luna.ai_tags` 与用户顶层 tags / 内联 #tag 同池去重入索引。
        // 顺序 = 顶层 tags → 内联 #tag → ai_tags（PRD-06-046：用户来源优先，见下一条测试）。
        let content =
            "---\ntags: [工作]\nluna:\n  schema: 1\n  ai_tags:\n    - 会议\n    - 工作\n---\n正文 #复盘";
        assert_eq!(
            tag_names(extract_tags(content)),
            vec!["工作", "复盘", "会议"]
        );
        // 仅 ai_tags、无用户 tags 也可独立成立。
        let only_ai = "---\nluna:\n  ai_tags: [luna/图谱]\n---\n正文";
        assert_eq!(tag_names(extract_tags(only_ai)), vec!["luna/图谱"]);
    }

    #[test]
    fn tag_source_prefers_user_when_both_sides_use_the_same_tag() {
        // PRD-06-046：用户与 AI 都提到同一个标签时，事实是「用户在用它」——
        // AI 恰好也提到不该把它降格为机器标签（否则「只看用户标签」会把它藏起来）。
        let content = "---\ntags: [工作]\nluna:\n  ai_tags: [工作, 会议]\n---\n正文 #复盘";
        let terms = extract_tags(content);
        let by = |name: &str| {
            terms
                .iter()
                .find(|t| t.tag == name)
                .unwrap_or_else(|| panic!("缺标签 {name}"))
                .source
        };
        assert_eq!(
            by("工作"),
            TagSource::User,
            "顶层 tags 与 ai_tags 都有 → 归用户"
        );
        assert_eq!(by("复盘"), TagSource::User, "正文内联 #tag 是用户亲手写的");
        assert_eq!(by("会议"), TagSource::Ai, "只有 AI 提到 → 归 AI");
    }

    #[test]
    fn tag_filters_cover_the_tag_and_its_descendants() {
        assert!(tag_covers("工作", "工作"));
        assert!(tag_covers("工作", "工作/周报"));
        assert!(tag_covers("#Project", "project/luna"));
        assert!(!tag_covers("工作", "工作室"));
        assert!(!tag_covers("工作/周报", "工作"));
        assert!(tag_covers("한국어", "한국어"));
    }

    #[test]
    fn korean_and_mixed_inline_tags_are_read() {
        let content = "---\ntags: [회의]\n---\n안녕하세요 #한국어 그리고 #中文/标签 ok";
        assert_eq!(
            tag_names(extract_tags(content)),
            vec!["회의", "한국어", "中文/标签"]
        );
    }
}
