//! 笔记解析助手（派生索引的输入加工，TDD-06 §3）。零外部依赖的纯函数，便于 headless 重测。
//!
//! - [`title_of`]：标题 = 文件名 stem（去 `.md`）——**单一来源**（标题 = 文件名 = 链接锚点，§3）。
//! - [`clean_for_fts`]：FTS 入库前预清洗——剥 frontmatter（[`crate::frontmatter::body_of`]）+ 剥时间轴
//!   行标记 `[HH:MM:SS.mmm]`（§3 索引卫生命门），否则搜索会命中 YAML 键与时间码、`snippet` 被污染。
//! - [`extract_tags`]：读 frontmatter 顶层 `tags`（§2.3，派生索引；只读不写）。
//! - [`extract_links`]：解析 wikilink `[[t]]`/`[[t|alias]]`/`![[embed]]` 与标准 md 相对链接
//!   `[文本](rel.md)`，建 `links` 边（§3 反链图谱统一）。

use crate::frontmatter::body_of;

/// 标题 = 文件名 stem（去末尾 `.md`，大小写不敏感）。不引入独立 title 字段（§3）。
#[must_use]
pub fn title_of(rel_path: &str) -> String {
    let file = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = file
        .strip_suffix(".md")
        .or_else(|| file.strip_suffix(".MD"))
        .or_else(|| file.strip_suffix(".markdown"))
        .unwrap_or(file);
    stem.to_string()
}

/// 判断从 `bytes[i..]` 起是否为一个时间轴行标记 `[HH:MM:SS]` 或 `[HH:MM:SS.mmm]`，
/// 返回标记的字节长度（含中括号）；不是则返回 `None`。
fn timecode_len(s: &[u8]) -> Option<usize> {
    // 形如 [dd:dd:dd] 或 [dd:dd:dd.d{1,3}]
    if s.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1;
    let digit = |b: u8| b.is_ascii_digit();
    let take = |s: &[u8], i: &mut usize, n: usize| -> bool {
        for _ in 0..n {
            if s.get(*i).copied().is_some_and(digit) {
                *i += 1;
            } else {
                return false;
            }
        }
        true
    };
    // HH:MM:SS
    if !take(s, &mut i, 2) || s.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    if !take(s, &mut i, 2) || s.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    if !take(s, &mut i, 2) {
        return None;
    }
    // 可选 .mmm（1~3 位）
    if s.get(i) == Some(&b'.') {
        i += 1;
        let mut frac = 0;
        while frac < 3 && s.get(i).copied().is_some_and(digit) {
            i += 1;
            frac += 1;
        }
        if frac == 0 {
            return None;
        }
    }
    if s.get(i) == Some(&b']') {
        Some(i + 1)
    } else {
        None
    }
}

/// 一段文本中被时间轴行标记 `[HH:MM:SS(.mmm)]` 覆盖的字节区间（升序不重叠）。
///
/// 搜索与替换的**禁区**：这些标记不进 FTS（[`clean_for_fts`] 已剥掉），若命中定位阶段
/// 反而能匹配它们，就会出现「索引筛不出、逐行扫得到」的口径分裂；更要命的是跨库替换
/// 可能改写转写笔记的时间码而毁掉回放锚点。故两处共用同一判定（DRY，TDD-06 §3）。
#[must_use]
pub fn timecode_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(len) = timecode_len(&bytes[i..]) {
                out.push((i, i + len));
                i += len;
                continue;
            }
        }
        i += utf8_char_len(bytes[i]);
    }
    out
}

/// 判断一行是否为**结构脚手架**：它由 Lunarca 自己写入用于定界/分角色，不是任何人写下的内容，
/// 故不该进 FTS（ADR-024 索引卫生）。命中即整行不入索引（保留换行，不改行结构）。
///
/// 两类（判据都是**整行精确匹配**，不做前缀匹配）：
/// - 转写块哨兵 `<!-- luna:transcript -->` / `<!-- /luna:transcript -->`（[`crate::vault`] 单一来源）；
/// - 对话笔记角色标题 `## User` / `## Assistant` / `## System`（`render_chat_markdown` 固定英文写入）。
///
/// **必须整行精确匹配**：用户自己写的 `## User research` 是内容，理应可搜——只有恰好一行
/// 只有这三个词时才是机器脚手架。
///
/// `pub(crate)`：FTS 预清洗（本模块）与搜索命中定位/替换（[`crate::search`] 的整行禁区）
/// 共用**同一判定**——两处分家就是 TDD-06 §3 警告过的「索引筛不出、逐行扫得到」口径分裂，
/// 更要命的是替换会改写 `## User` 结构行、毁掉对话笔记的往返解析。
pub(crate) fn is_scaffold_line(line: &str) -> bool {
    let t = line.trim();
    t == crate::vault::TRANSCRIPT_BEGIN
        || t == crate::vault::TRANSCRIPT_END
        || t == "## User"
        || t == "## Assistant"
        || t == "## System"
}

/// FTS 预清洗：剥 frontmatter + 移除时间轴行标记 `[HH:MM:SS(.mmm)]` + 丢弃结构脚手架行，
/// 仅留可读正文。
///
/// 输出只用于 FTS 匹配与 snippet，**不承担偏移语义**（命中定位由 `Vault::search` 重读文件
/// 逐行完成，TDD-06 §3.1），故整行丢弃是安全的。
#[must_use]
pub fn clean_for_fts(content: &str) -> String {
    let body = body_of(content);
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        if is_scaffold_line(line) {
            // 行内容不入索引，但保留换行——不改变行结构，snippet 不会把上下两行黏成一句。
            if line.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        strip_timecodes_into(line, &mut out);
    }
    out
}

/// 把一行的时间轴标记剥掉后写入 `out`（其余字节原样保留）。
fn strip_timecodes_into(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(len) = timecode_len(&bytes[i..]) {
                i += len;
                // 跳过紧随标记的一个空格（常见 "[..] 正文"），避免行首留下孤立空格。
                if bytes.get(i) == Some(&b' ') {
                    i += 1;
                }
                continue;
            }
        }
        // 安全推进一个 UTF-8 字符（i 始终落在字符边界）。
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
}

fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
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
    if let Some(value) = frontmatter_yaml(content) {
        for tag in yaml_tag_list(value.get("tags")) {
            push(tag, TagSource::User);
        }
    }
    // 内联 `#tag` 排在 `ai_tags` **之前**：它同样是用户亲手写的，用户与 AI 都提到
    // 同一个标签时，事实是"用户在用这个标签"——AI 恰好也提到不该改变它的归属。
    for tag in extract_inline_tags(body_of(content)) {
        push(tag, TagSource::User);
    }
    if let Some(value) = frontmatter_yaml(content) {
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
    frontmatter_yaml(content)
        .map(|value| yaml_tag_list(value.get("tags")))
        .unwrap_or_default()
}

/// frontmatter 顶层 `aliases` 里的别名（Obsidian 同一写法；对标方案 C2）：列表逐项取，
/// 写成一个字符串时按逗号分开——别名里常有空格（「周会 纪要」），不能像标签那样按空格切。
/// 去空白、去空项，不分大小写去重，保留书写顺序。
#[must_use]
pub fn frontmatter_aliases(content: &str) -> Vec<String> {
    let Some(value) = frontmatter_yaml(content) else {
        return Vec::new();
    };
    let raw: Vec<String> = match value.get("aliases") {
        Some(serde_yaml_ng::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(serde_yaml_ng::Value::String(s)) => s.split(',').map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for alias in raw {
        let alias = alias.trim();
        if !alias.is_empty() && !out.iter().any(|a| a.to_lowercase() == alias.to_lowercase()) {
            out.push(alias.to_owned());
        }
    }
    out
}

/// 解析 frontmatter 为 YAML 值。容错：无 frontmatter / 解析失败返回 `None`。
fn frontmatter_yaml(content: &str) -> Option<serde_yaml_ng::Value> {
    let inner = frontmatter_inner(content)?;
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(inner).ok()
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
pub(crate) fn is_tag_char(ch: char) -> bool {
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

/// 取 frontmatter 内部文本（`---`…`---` 之间）。复用 §4.4 同一定位口径。
fn frontmatter_inner(content: &str) -> Option<&str> {
    // 与 frontmatter::find_frontmatter 同口径，但只需 inner；此处轻量重实现避免暴露内部类型。
    let rest = content.strip_prefix("---\n").or_else(|| {
        content
            .strip_prefix("---\r\n")
            .map(|_| &content["---\r\n".len()..])
    })?;
    let start = content.len() - rest.len();
    let mut cursor = start;
    let bytes = content.as_bytes();
    while cursor < content.len() {
        let line_end = content[cursor..]
            .find('\n')
            .map_or(content.len(), |r| cursor + r);
        let line = content[cursor..line_end]
            .strip_suffix('\r')
            .unwrap_or(&content[cursor..line_end]);
        if line == "---" {
            return Some(&content[start..cursor]);
        }
        cursor = if line_end < content.len() {
            line_end + 1
        } else {
            content.len()
        };
        let _ = bytes;
    }
    None
}

/// wikilink `|` 之前的部分拆成「指向哪一篇」与「落到哪一处」：`笔记#标题` →
/// (`笔记`, `Some("标题")`)，`笔记#^块` → (`笔记`, `Some("^块")`)。片段不参与解析——
/// 把 `笔记#标题` 整串当笔记名，结果是一条明明存在的链接被画成「未创建」。
#[must_use]
pub fn split_wikilink_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((note, fragment)) => {
            let fragment = fragment.trim();
            (note.trim(), (!fragment.is_empty()).then_some(fragment))
        }
        None => (target.trim(), None),
    }
}

/// wikilink 片段在一篇笔记里的位置：`(行号, 标题序号)`。
///
/// - `标题`：正文里第一个文字相同的 ATX 标题（忽略大小写、首尾空白与收尾 `#`）；
/// - `^块`：第一个以 ` ^块` 结尾的行（Obsidian 块标记），标题序号为 `None`。
///
/// 行号 1 起、按整个文件计（编辑器载入的是整个文件）；标题序号 0 起、只数正文
/// （阅读视图不渲染 frontmatter）。围栏代码里的 `#` 行不是标题。
#[must_use]
pub fn locate_fragment(content: &str, fragment: &str) -> (Option<u32>, Option<u32>) {
    let base = crate::frontmatter::body_base_line(content);
    let wanted = fragment.trim();
    let block = wanted.strip_prefix('^').map(str::trim);
    let wanted_heading = wanted.to_lowercase();
    let mut fence: Option<&str> = None;
    let mut heading_index = 0u32;
    for (offset, line) in body_of(content).lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        let line_no = Some((base + offset + 1) as u32);
        if let Some(id) = block {
            let tail = line.trim_end();
            if !id.is_empty()
                && tail
                    .strip_suffix(id)
                    .is_some_and(|rest| rest.ends_with(" ^") || rest == "^")
            {
                return (line_no, None);
            }
            continue;
        }
        if let Some(text) = atx_heading_text(line) {
            if text.to_lowercase() == wanted_heading {
                return (line_no, Some(heading_index));
            }
            heading_index += 1;
        }
    }
    (None, None)
}

/// ATX 标题的文字（`## 标题 ##` → `标题`）；不是标题为 `None`。缩进超过 3 格是代码块。
pub(crate) fn atx_heading_text(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let level = rest.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &rest[level..];
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with('\t')) {
        return None;
    }
    // 收尾的 `#` 串前面必须是空白才算收尾（`## C#` 的 `#` 是文字）。
    let text = after.trim();
    let stripped = text.trim_end_matches('#');
    Some(if stripped.is_empty() {
        ""
    } else if stripped.ends_with([' ', '\t']) {
        stripped.trim_end()
    } else {
        text
    })
}

/// 解析正文中的链接目标（wikilink 与标准 md 相对链接），建 `links` 边（§3）。
/// 返回去重后的目标字符串列表（保留原书写，规范化交由索引层）。
#[must_use]
pub fn extract_links(content: &str) -> Vec<String> {
    extract_links_inner(content, true)
}

/// 声明出处按出现顺序编号；同一文件的两个独立引文不能被图谱用的去重规则折叠。
#[must_use]
pub fn extract_link_occurrences(content: &str) -> Vec<String> {
    extract_links_inner(content, false)
}

fn extract_links_inner(content: &str, deduplicate: bool) -> Vec<String> {
    let body = body_of(content);
    let mut out = Vec::new();
    for link in scan_links(body) {
        let target = body[link.target.clone()].to_string();
        if deduplicate {
            push_unique(&mut out, target);
        } else {
            out.push(target);
        }
    }
    out
}

/// 一条链接是哪种写法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkKind {
    /// `[[目标]]`；`embed` = 前缀 `!` 的嵌入 `![[目标]]`。
    Wiki { embed: bool },
    /// `[文字](路径)` 与图片 `![文字](路径)`。
    Markdown,
}

/// 正文里的一条链接：只记**指向谁**那一段的字节区间，片段（`#标题`）、别名（`|显示`）、
/// md 链接的标题（`"说明"`）与尖括号都在区间之外——改写时只换这一段，其余原样留着。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkRef {
    pub kind: LinkKind,
    /// 目标文字在原文里的字节区间（wikilink：`|` 与 `#` 之前、去掉首尾空白；md：路径部分）。
    pub target: std::ops::Range<usize>,
    /// md 链接的目标写在 `<…>` 里（可含空格，改写时不转义）。
    pub angle: bool,
}

/// 正文里 HTML 媒体标签的 `src`（`<img>` / `<video>` / `<audio>` / `<source>`）。Markdown
/// 链接扫描不认这一种，而从别处粘来的笔记常这么引图——孤儿附件检测漏掉它就会把在用的图
/// 当孤儿（PRD-06-012 ②）。只认带引号的值；宁可多认（代码块里的示例也算），不可漏认。
pub(crate) fn html_media_sources(content: &str) -> Vec<&str> {
    let lower = content.to_ascii_lowercase();
    let mut out = Vec::new();
    for tag in ["<img", "<video", "<audio", "<source"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(tag) {
            let start = from + found;
            let end = lower[start..]
                .find('>')
                .map_or(lower.len(), |offset| start + offset);
            from = start + tag.len();
            let Some(attr) = lower[start..end].find("src=") else {
                continue;
            };
            let value_start = start + attr + 4;
            let Some(quote) = content[value_start..]
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
            else {
                continue;
            };
            let inner = value_start + 1;
            if let Some(close) = content[inner..end].find(quote) {
                let value = content[inner..inner + close].trim();
                if !value.is_empty() {
                    out.push(value);
                }
            }
        }
    }
    out
}

/// 全文的链接扫描——建 `links` 边与改名重构**共用这一份**（TDD-06 §3 改名重构 ③）。
///
/// 两边各认一套的代价是真实的：改名器此前只认行首 ```，`~~~` 围栏与行内代码里的示例
/// 链接会被改写；md 链接的 `#片段` 没剥，`[t](a.md#节)` 改名后不更新。这里跳过：
/// ``` / `~~~` 围栏（闭合需同字符且不短于开头）、缩进代码块（空行之后缩进 ≥4，列表里的
/// 缩进不算）、行内代码（同长度反引号配对）。外链（带 `://`、`mailto:`）与纯锚点不收。
#[must_use]
pub(crate) fn scan_links(content: &str) -> Vec<LinkRef> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut fence: Option<(u8, usize)> = None;
    let mut prev_blank = true;
    let mut in_indented_code = false;
    let mut in_list = false;
    for raw in content.split_inclusive('\n') {
        let line_start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let indent = line
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .map(|b| if b == b'\t' { 4 } else { 1 })
            .sum::<usize>();
        let trimmed = line.trim_start_matches([' ', '\t']);
        if let Some((ch, len)) = fence {
            let run = trimmed.bytes().take_while(|b| *b == ch).count();
            if indent <= 3 && run >= len && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if indent <= 3 {
            let first = trimmed.bytes().next();
            if let Some(ch @ (b'`' | b'~')) = first {
                let run = trimmed.bytes().take_while(|b| *b == ch).count();
                if run >= 3 {
                    fence = Some((ch, run));
                    continue;
                }
            }
        }
        if trimmed.is_empty() {
            prev_blank = true;
            continue;
        }
        if indent >= 4 && !in_list && (prev_blank || in_indented_code) {
            in_indented_code = true;
            prev_blank = false;
            continue;
        }
        in_indented_code = false;
        if starts_list_item(trimmed) {
            in_list = true;
        } else if indent == 0 && prev_blank {
            in_list = false;
        }
        prev_blank = false;
        scan_line_links(line, line_start, &mut out);
    }
    out
}

fn starts_list_item(trimmed: &str) -> bool {
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") "))
}

fn scan_line_links(line: &str, base: usize, out: &mut Vec<LinkRef>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // 行内代码：同长度的反引号串才闭合；配不上的反引号按字面走。
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
            let fence = &line[i..i + run];
            let mut search = i + run;
            let mut closed = None;
            while let Some(at) = line[search..].find(fence) {
                let at = search + at;
                let len = bytes[at..].iter().take_while(|b| **b == b'`').count();
                if len == run {
                    closed = Some(at + run);
                    break;
                }
                search = at + len;
            }
            i = closed.unwrap_or(i + run);
            continue;
        }
        if bytes[i] == b'[' && bytes.get(i + 1) == Some(&b'[') {
            if let Some(close) = line[i + 2..].find("]]") {
                let inner_start = i + 2;
                let inner = &line[inner_start..inner_start + close];
                let before_alias = inner.split('|').next().unwrap_or(inner);
                let note = before_alias.split('#').next().unwrap_or(before_alias);
                let lead = note.len() - note.trim_start().len();
                let trimmed = note.trim();
                if !trimmed.is_empty() {
                    let start = base + inner_start + lead;
                    out.push(LinkRef {
                        kind: LinkKind::Wiki {
                            embed: i > 0 && bytes[i - 1] == b'!',
                        },
                        target: start..start + trimmed.len(),
                        angle: false,
                    });
                }
                i = inner_start + close + 2;
                continue;
            }
        }
        if bytes[i] == b']' && bytes.get(i + 1) == Some(&b'(') && line[..i].contains('[') {
            if let Some((link, end)) = markdown_destination(line, i + 2, base) {
                out.extend(link);
                i = end;
                continue;
            }
        }
        i += utf8_char_len(bytes[i]);
    }
}

/// `](` 之后的目标：返回（链接，`)` 之后的位置）。目标里的括号需配平；`<…>` 可含空格。
fn markdown_destination(line: &str, open: usize, base: usize) -> Option<(Option<LinkRef>, usize)> {
    let bytes = line.as_bytes();
    let mut start = open;
    while bytes.get(start) == Some(&b' ') {
        start += 1;
    }
    let (path_start, path_end, angle, mut cursor) = if bytes.get(start) == Some(&b'<') {
        let close = line[start + 1..].find('>')? + start + 1;
        (start + 1, close, true, close + 1)
    } else {
        let mut depth = 0usize;
        let mut end = start;
        while end < bytes.len() {
            match bytes[end] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b' ' | b'\t' => break,
                _ => {}
            }
            end += 1;
        }
        (start, end, false, end)
    };
    // 标题（`"说明"`）与空白一直吃到收尾的 `)`。
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'(' => depth += 1,
            b')' if depth == 0 => break,
            b')' => depth -= 1,
            _ => {}
        }
        cursor += 1;
    }
    if cursor >= bytes.len() {
        return None;
    }
    // 手写的 `[x](带 空格.md)` 不合 CommonMark，但此前一直被当作链接认、建过反链：空白之后
    // 不是标题（引号或括号开头）时，整段到 `)` 都算路径，不让这些链接突然「消失」。
    let path_end = if !angle && path_end < cursor {
        let rest = line[path_end..cursor].trim_start();
        if rest.is_empty() || rest.starts_with(['"', '\'', '(']) {
            path_end
        } else {
            path_start + line[path_start..cursor].trim_end().len()
        }
    } else {
        path_end
    };
    let dest = &line[path_start..path_end];
    let path = dest.split('#').next().unwrap_or(dest);
    let link = (is_local_link(path) && !path.trim().is_empty()).then(|| LinkRef {
        kind: LinkKind::Markdown,
        target: base + path_start..base + path_start + path.len(),
        angle,
    });
    Some((link, cursor + 1))
}

/// 一行内**已是链接或代码**的字节区间（wikilink / md 链接 / 行内 code）。
///
/// 供「未链接提及」（PRD-16-013）判定改写禁区：往已有链接或行内代码里再插一层链接会
/// 毁掉它们。与 [`extract_links`] 共用同一套语法识别（此处只多返回区间、不返回目标），
/// 故两处对「什么算链接」的答案恒一致。
#[must_use]
pub fn link_spans(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(close) = line[i + 1..].find('`') {
                out.push((i, i + 1 + close + 1));
                i += 1 + close + 1;
                continue;
            }
        }
        if bytes[i] == b'[' && bytes.get(i + 1) == Some(&b'[') {
            if let Some(close) = line[i + 2..].find("]]") {
                let start = i.saturating_sub(usize::from(i > 0 && bytes[i - 1] == b'!'));
                out.push((start, i + 2 + close + 2));
                i += 2 + close + 2;
                continue;
            }
        }
        if bytes[i] == b'[' {
            if let Some(rb) = line[i..].find("](") {
                let text_end = i + rb;
                if let Some(close) = line[text_end + 2..].find(')') {
                    out.push((i, text_end + 2 + close + 1));
                    i = text_end + 2 + close + 1;
                    continue;
                }
            }
        }
        i += utf8_char_len(bytes[i]);
    }
    out
}

fn is_local_link(dest: &str) -> bool {
    !dest.is_empty()
        && !dest.starts_with('#')
        && !dest.contains("://")
        && !dest.starts_with("mailto:")
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试助手：只比对标签串（来源另有专测，避免每条断言都写一遍 `TaggedTerm{..}`）。
    fn tag_names(terms: Vec<TaggedTerm>) -> Vec<String> {
        terms.into_iter().map(|t| t.tag).collect()
    }

    #[test]
    fn timecode_spans_mark_markers_only() {
        // 搜索/替换禁区与 FTS 剥离口径同源：只圈标记本身，正文里的裸时间字符串不算。
        assert_eq!(
            timecode_spans("[00:01:23.400] 讲到 00:01:23 这里"),
            vec![(0, 14)]
        );
        assert!(timecode_spans("没有标记的一行 12:34:56").is_empty());
        assert_eq!(
            timecode_spans("[00:00:01][00:00:02]"),
            vec![(0, 10), (10, 20)]
        );
    }

    #[test]
    fn title_is_filename_stem() {
        assert_eq!(title_of("日常/会议纪要.md"), "会议纪要");
        assert_eq!(title_of("note.md"), "note");
        assert_eq!(title_of("无扩展"), "无扩展");
    }

    #[test]
    fn clean_strips_frontmatter_and_timecodes() {
        let content =
            "---\nluna:\n  type: recording\n---\n[00:00:12.480] 大家好\n[01:02:03] 第二段";
        let cleaned = clean_for_fts(content);
        assert!(!cleaned.contains("luna"), "不该含 frontmatter：{cleaned}");
        assert!(!cleaned.contains('['), "不该含时间码：{cleaned}");
        assert!(cleaned.contains("大家好"));
        assert!(cleaned.contains("第二段"));
    }

    #[test]
    fn clean_drops_scaffold_lines() {
        // 转写哨兵与对话角色标题是 Lunarca 自己写的结构脚手架，不是任何人写下的内容（ADR-024）。
        let content = "<!-- luna:transcript -->\n[00:00:01] 说了一句\n<!-- /luna:transcript -->\n";
        let cleaned = clean_for_fts(content);
        assert!(
            !cleaned.contains("luna:transcript"),
            "哨兵应剥净：{cleaned}"
        );
        assert!(cleaned.contains("说了一句"), "正文须保留：{cleaned}");

        let chat = "## User\n\n帮我看看这段代码\n\n## Assistant\n\n这里有个空指针\n";
        let cleaned = clean_for_fts(chat);
        assert!(!cleaned.contains("Assistant"), "角色标题应剥净：{cleaned}");
        assert!(!cleaned.contains("## User"), "角色标题应剥净：{cleaned}");
        assert!(cleaned.contains("帮我看看这段代码"));
        assert!(cleaned.contains("这里有个空指针"));
    }

    #[test]
    fn clean_keeps_user_written_headings_that_merely_start_with_role_words() {
        // 判据是整行精确匹配——用户自己写的标题是内容，理应可搜。
        let cleaned = clean_for_fts("## User research\n\n## Assistant 岗位职责\n");
        assert!(cleaned.contains("User research"), "{cleaned}");
        assert!(cleaned.contains("Assistant 岗位职责"), "{cleaned}");
    }

    #[test]
    fn clean_keeps_non_timecode_brackets() {
        // 普通中括号（如脚注 [1]、待办 [ ]）不应被误删。
        let cleaned = clean_for_fts("见 [1] 与 [ ] 任务");
        assert!(cleaned.contains("[1]"));
        assert!(cleaned.contains("[ ]"));
    }

    #[test]
    fn reads_frontmatter_aliases_list_or_comma_string() {
        assert_eq!(
            frontmatter_aliases(
                "---\naliases:\n  - 周会 纪要\n  - Weekly\n  - weekly\n  - ''\n---\n正文"
            ),
            vec!["周会 纪要", "Weekly"]
        );
        assert_eq!(
            frontmatter_aliases("---\naliases: 甲, 乙 丙\n---\n"),
            vec!["甲", "乙 丙"]
        );
        assert!(frontmatter_aliases("---\ntags: [a]\n---\n").is_empty());
        assert!(frontmatter_aliases("没有 frontmatter").is_empty());
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
    fn extracts_both_link_kinds() {
        let body = "见 [[会议纪要]] 与 [[项目|别名]]，嵌入 ![[图]]，外链 [文档](项目/计划.md) 与 [站](https://x.com)";
        let links = extract_links(body);
        assert!(links.contains(&"会议纪要".to_string()));
        assert!(links.contains(&"项目".to_string()));
        assert!(links.contains(&"图".to_string()));
        assert!(links.contains(&"项目/计划.md".to_string()));
        assert!(!links.iter().any(|l| l.starts_with("https")), "外链不建边");
    }

    #[test]
    fn link_scan_skips_code_and_keeps_only_the_target_part() {
        // 建边与改名重构共用这一份扫描（TDD-06 §3 改名重构 ③）。
        let content = "```\n[[围栏]]\n```\n~~~~\n[[波浪]]\n~~~~\n\n    [[缩进代码]]\n\n\
            `[[行内]]` [[真的#节|别名]] [t](a.md#节 \"标题\") ![](<带 空格.png>) [u](手写 空格.md) [w](https://x.com)\n";
        assert_eq!(
            extract_links(content),
            vec!["真的", "a.md", "带 空格.png", "手写 空格.md"]
        );
        let spans = scan_links(content);
        let embed_or_md: Vec<_> = spans.iter().map(|l| (l.kind, l.angle)).collect();
        assert_eq!(
            embed_or_md,
            vec![
                (LinkKind::Wiki { embed: false }, false),
                (LinkKind::Markdown, false),
                (LinkKind::Markdown, true),
                (LinkKind::Markdown, false),
            ]
        );
        // 双反引号包住单反引号也是一整段代码。
        assert!(extract_links("``a ` [[x]]`` 外").is_empty());
    }

    #[test]
    fn fragment_locates_headings_and_block_marks() {
        let doc = "---\ntitle: x\n---\n# 总览\n```\n# 代码里的井号\n```\n## 下周计划 ##\n要点 ^k1\n## C#\n";
        assert_eq!(locate_fragment(doc, "下周计划"), (Some(8), Some(1)));
        assert_eq!(locate_fragment(doc, " 总览 "), (Some(4), Some(0)));
        assert_eq!(locate_fragment(doc, "c#"), (Some(10), Some(2)));
        assert_eq!(locate_fragment(doc, "^k1"), (Some(9), None));
        assert_eq!(locate_fragment(doc, "代码里的井号"), (None, None));
        assert_eq!(locate_fragment(doc, "^k"), (None, None));
    }

    #[test]
    fn wikilink_fragment_is_not_part_of_the_target() {
        assert_eq!(
            split_wikilink_target(" 周报 # 下周计划 "),
            ("周报", Some("下周计划"))
        );
        assert_eq!(split_wikilink_target("周报#"), ("周报", None));
        assert_eq!(split_wikilink_target("周报"), ("周报", None));
        let links = extract_links("见 [[周报#计划]]、[[周报#^k1|块]] 与 [[#本页标题]]");
        assert_eq!(links, vec!["周报".to_string()]);
    }
}

/// 正文里的待办统计（PRD-06-058 G）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TodoStats {
    /// 还没勾的条数。
    pub open: u32,
    /// 总条数（`0` = 这篇没有待办）。
    pub total: u32,
}

/// 数一段正文里的 Markdown 复选框。
///
/// 判定在这里而不在前端（ADR-003）：待办既可能来自 AI 提炼（PRD-06-058 C′ 写的
/// `- [ ] …`），也可能是用户自己敲的——两种都是同一种正文事实，只该有一个人认识它。
///
/// 口径刻意收得很紧：**行首（可缩进）的列表标记 + 方括号 + 单个空格**。放宽会把正文里
/// 谈论待办的句子（「我写了 - [x] 这种东西」）也数进去，而列表上那个数字一旦不可信，
/// 用户就再也不看它了。
///
/// 围栏代码块内的复选框不算——那是代码示例，不是这篇笔记的待办。
#[must_use]
pub fn todo_stats(body: &str) -> TodoStats {
    let mut stats = TodoStats::default();
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        else {
            continue;
        };
        let rest = rest.trim_start();
        let done = match rest.get(..4) {
            Some("[ ] ") => false,
            Some("[x] ") | Some("[X] ") => true,
            // `- [ ]` 后面什么都没有（空待办）也算一条：用户敲了框就是打算填。
            _ => match rest.trim_end() {
                "[ ]" => false,
                "[x]" | "[X]" => true,
                _ => continue,
            },
        };
        stats.total += 1;
        if !done {
            stats.open += 1;
        }
    }
    stats
}

/// 一篇速记的到期状态（`luna.dates` 相对于「今天」）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DueState {
    /// 没有日期线索。
    #[default]
    None,
    /// 最早的那个日期就是今天。
    Today,
    /// 最早的那个日期已经过去了。
    Overdue,
    /// 都在将来。
    Upcoming,
}

/// 判定一组 ISO 日期相对于今天的状态（取**最早**的那一个说话）。
///
/// `today` 与列表里的日期都是 `YYYY-MM-DD`：同为定长的 ISO 形状，字典序即时间序，
/// 不必解析成日期类型再比一次。非法形状（模型偶尔回一个别的东西）直接跳过——
/// 提炼那一侧已经用 ResultSchema 校验过形状，这里是第二道，不该因为一个坏值就整篇失声。
#[must_use]
pub fn due_state(dates: &[String], today: &str) -> DueState {
    let mut earliest: Option<&str> = None;
    for date in dates {
        let d = date.trim();
        if d.len() != 10
            || !d.as_bytes().iter().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    *b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        {
            continue;
        }
        if earliest.is_none_or(|e| d < e) {
            earliest = Some(d);
        }
    }
    match earliest {
        None => DueState::None,
        Some(d) if d < today => DueState::Overdue,
        Some(d) if d == today => DueState::Today,
        Some(_) => DueState::Upcoming,
    }
}

#[cfg(test)]
mod todo_tests {
    use super::*;

    #[test]
    fn todo_stats_counts_checkboxes_and_ignores_prose_and_code() {
        let body = "\
# 周会
- [ ] 发周报
- [x] 订会议室
* [ ] 星号也算
+ [X] 加号也算

普通一行 - [x] 谈论待办的句子不算（不在行首）

```
- [ ] 代码示例里的不算
```
- 不是待办的普通列表项
";
        let stats = todo_stats(body);
        assert_eq!(stats.total, 4, "四个真待办");
        assert_eq!(stats.open, 2, "两个没勾");
    }

    #[test]
    fn todo_stats_is_zero_when_there_is_nothing_to_do() {
        assert_eq!(todo_stats("就是一段话\n"), TodoStats::default());
        // 列表上那个数字一旦把散文也数进去就不可信了，而不可信的数字用户再也不看。
        assert_eq!(todo_stats("聊到 [x] 这种写法\n"), TodoStats::default());
    }

    #[test]
    fn due_state_speaks_for_the_earliest_date() {
        let today = "2026-09-09";
        assert_eq!(due_state(&[], today), DueState::None);
        assert_eq!(
            due_state(&["2026-09-20".into(), "2026-09-01".into()], today),
            DueState::Overdue,
            "最早的那个已经过去了，就该按它说话"
        );
        assert_eq!(due_state(&["2026-09-09".into()], today), DueState::Today);
        assert_eq!(due_state(&["2026-12-01".into()], today), DueState::Upcoming);
    }

    #[test]
    fn due_state_skips_malformed_dates_instead_of_going_silent() {
        // 模型偶尔回一个别的形状；因为一个坏值就让整篇失声，比忽略它更糟。
        let today = "2026-09-09";
        assert_eq!(
            due_state(&["下周三".into(), "2026-09-01".into()], today),
            DueState::Overdue
        );
        assert_eq!(due_state(&["下周三".into()], today), DueState::None);
    }
}
