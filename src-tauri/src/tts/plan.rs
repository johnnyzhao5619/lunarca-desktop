//! 朗读文本规划（纯逻辑，零 IO，可单测）。
//!
//! 把「要朗读的文本」切成一串**句子级发声单元**（[`SpeechSegment`]），每个携带其在**源字符串
//! 中的 UTF-16 区间** `[start, end)`——与 CodeMirror 6 文档位置**同口径**（JS 字符串按 UTF-16
//! 码元计数），供前端逐句高亮与跟随滚动。
//!
//! Markdown 源会先剥离不宜朗读的语法：frontmatter、围栏代码块、标题井号、列表/引用/任务符号、
//! 强调符、行内代码反引号、链接/图片/wiki/嵌入的装饰、HTML 标签、**内联 `#tag`**、`==高亮==`、
//! 脚注标记、公式定界符、转义反斜杠，并**整段丢弃 `%%注释%%`**（阅读视图本就不渲染）；整行跳过
//! **纯排版行**（水平分隔线 / setext 下划线 / 表格分隔行——念出 `====`、`|---|` 是噪声）；表格
//! 竖线化为停顿、标识符内下划线与标签内斜杠化为分词空格（`some_var` 读作「some var」而非粘成
//! 一个词）。**刻意不误伤的对照**：`#1` 编号、`C#` 词内井号、`$100` 金额、`a == b` 比较运算、
//! `a < b` 比较号——它们是正文语义，须原样读出（逐条有回归单测）。
//! 但**发声单元的区间始终指向源文本原句**
//! （高亮覆盖整句原文，含其行内标记），仅 `text` 是清洗后的可朗读文本——二者职责分离，前端拿区间
//! 高亮、系统合成器拿 `text` 出声。
//!
//! 断句在**行边界**与**句末标点**（`. ! ? 。！？…；;`）双重切分；宁可多切（多一次自然停顿）也不
//! 误合，绝不跨行合并（Markdown 段落多为整行，跟随高亮以行/句为单位最直观）。

/// 一个发声单元：清洗后的可朗读文本 + 其在源串中的 UTF-16 区间 `[start_utf16, end_utf16)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechSegment {
    /// 清洗后要交给系统合成器朗读的文本（已 `trim`、非空）。
    pub text: String,
    /// 源串 UTF-16 起（含）——与 CodeMirror 文档位置同口径。
    pub start_utf16: u32,
    /// 源串 UTF-16 止（不含）。
    pub end_utf16: u32,
}

/// 语言检测样本的最大字符数。lingua 是 CPU 密集且 `tts_speak` 为同步命令——整篇长文（数万字）
/// 直接送检会明显拖慢「点击即读」的响应；而语言不会在一篇文档中途改变，取开头若干句足以判定。
const DETECT_SAMPLE_MAX_CHARS: usize = 600;

/// 从发声单元取语言检测样本（前若干句拼接并截断到 [`DETECT_SAMPLE_MAX_CHARS`]）。
///
/// 用**清洗后的可朗读文本**而非原始 Markdown：frontmatter/代码块/链接 URL 已被剥离，
/// 不会把 YAML 键名或代码里的英文误判成正文语言。
#[must_use]
pub fn detect_sample(segments: &[SpeechSegment]) -> String {
    let mut sample = String::new();
    for segment in segments {
        if sample.chars().count() >= DETECT_SAMPLE_MAX_CHARS {
            break;
        }
        if !sample.is_empty() {
            sample.push(' ');
        }
        sample.push_str(&segment.text);
    }
    sample.chars().take(DETECT_SAMPLE_MAX_CHARS).collect()
}

/// 是否句子边界。CJK 全角标点无空格惯例 → 恒切；拉丁标点须其后为空白/行末/收尾符号才切
/// （否则 `example.com`、`e.g.`、`3.14` 会被误切；URL 里的 `.` 尤甚）。
fn is_sentence_boundary(ch: char, next: Option<char>) -> bool {
    match ch {
        // CJK 全角句末标点：恒为边界。
        '。' | '！' | '？' | '；' => true,
        // 拉丁句末标点 + 省略号：仅当其后为空白、行末或收尾括号/引号时才是边界。
        '.' | '!' | '?' | ';' | '…' => match next {
            None => true,
            Some(n) => {
                n.is_whitespace()
                    || matches!(n, ')' | ']' | '}' | '"' | '\'' | '”' | '’' | '」' | '』')
            }
        },
        _ => false,
    }
}

/// 把「要朗读的文本」规划为发声单元序列。
///
/// - `markdown = true`：按 Markdown 处理（剥 frontmatter/代码块/块级与行内标记）。
/// - `markdown = false`：纯文本（HUD 原文/译文、编辑器选区可能含少量标记但按纯文本处理更稳）。
#[must_use]
pub fn plan_speech(source: &str, markdown: bool) -> Vec<SpeechSegment> {
    let mut out = Vec::new();
    let mut cur_utf16: u32 = 0;
    let mut line_start_utf16: u32 = 0;
    let mut line = String::new();
    let mut state = LineState::new();

    // 逐字符扫描，精确维护每行起始的 UTF-16 偏移（`\r\n` 中 `\r` 计入行内容、`\n` 单独 +1）。
    for ch in source.chars() {
        if ch == '\n' {
            process_line(&line, line_start_utf16, markdown, &mut state, &mut out);
            cur_utf16 += 1;
            line.clear();
            line_start_utf16 = cur_utf16;
        } else {
            line.push(ch);
            cur_utf16 += u32::try_from(ch.len_utf16()).unwrap_or(1);
        }
    }
    if !line.is_empty() {
        process_line(&line, line_start_utf16, markdown, &mut state, &mut out);
    }
    out
}

/// 跨行扫描状态（frontmatter / 围栏代码块 / 是否首行）。
struct LineState {
    first_line: bool,
    in_frontmatter: bool,
    in_code_fence: bool,
}

impl LineState {
    fn new() -> Self {
        Self {
            first_line: true,
            in_frontmatter: false,
            in_code_fence: false,
        }
    }
}

/// 处理一行（可能产出零或多个发声单元）。
fn process_line(
    line: &str,
    line_start_utf16: u32,
    markdown: bool,
    state: &mut LineState,
    out: &mut Vec<SpeechSegment>,
) {
    let was_first = state.first_line;
    state.first_line = false;

    if markdown {
        let trimmed = line.trim();
        // YAML frontmatter：仅当整个文档首行是 `---` 才进入，直到下一条 `---` 收尾（都不朗读）。
        if was_first && trimmed == "---" {
            state.in_frontmatter = true;
            return;
        }
        if state.in_frontmatter {
            if trimmed == "---" {
                state.in_frontmatter = false;
            }
            return;
        }
        // 围栏代码块：``` 或 ~~~ 切换，围栏内与围栏行都不朗读。
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            state.in_code_fence = !state.in_code_fence;
            return;
        }
        if state.in_code_fence {
            return;
        }
    }

    // 纯排版行（水平分隔线 / setext 标题下划线 / 表格分隔行）不含可朗读信息，整行跳过。
    if markdown && is_layout_only_line(line) {
        return;
    }

    // 计算「块级前缀」消耗的字符数（缩进 + 标题/引用/列表/任务符号），得到正文起点。
    let prefix_chars = if markdown { block_prefix_len(line) } else { 0 };
    // 前缀之外若整行空白，跳过。
    let content: String = line.chars().skip(prefix_chars).collect();
    if content.trim().is_empty() {
        return;
    }

    // 正文起点的 UTF-16 偏移。
    let content_start_utf16 =
        line_start_utf16 + utf16_len(&line.chars().take(prefix_chars).collect::<String>());

    // 在正文内按句边界切句，每句的源区间 = 该句在源行的 UTF-16 跨度（含前导空白，保持连续无缝）。
    let content_chars: Vec<char> = content.chars().collect();
    let mut sentence_start_off: u32 = 0; // 相对 content 的 UTF-16 偏移
    let mut sentence_src = String::new();
    let mut acc_off: u32 = 0; // content 内当前 UTF-16 偏移
    for (idx, &ch) in content_chars.iter().enumerate() {
        sentence_src.push(ch);
        acc_off += u32::try_from(ch.len_utf16()).unwrap_or(1);
        if is_sentence_boundary(ch, content_chars.get(idx + 1).copied()) {
            emit_sentence(
                &sentence_src,
                content_start_utf16 + sentence_start_off,
                content_start_utf16 + acc_off,
                markdown,
                out,
            );
            sentence_src.clear();
            sentence_start_off = acc_off;
        }
    }
    if !sentence_src.is_empty() {
        emit_sentence(
            &sentence_src,
            content_start_utf16 + sentence_start_off,
            content_start_utf16 + acc_off,
            markdown,
            out,
        );
    }
}

/// 清洗一句源文本并（非空则）产出发声单元。区间指向源，`text` 为清洗后可朗读文本。
fn emit_sentence(
    src_sentence: &str,
    start_utf16: u32,
    end_utf16: u32,
    markdown: bool,
    out: &mut Vec<SpeechSegment>,
) {
    let spoken = if markdown {
        strip_inline(src_sentence)
    } else {
        src_sentence.to_string()
    };
    let spoken = spoken.trim();
    if spoken.is_empty() {
        return;
    }
    out.push(SpeechSegment {
        text: spoken.to_string(),
        start_utf16,
        end_utf16,
    });
}

/// 字符串的 UTF-16 码元长度。
fn utf16_len(s: &str) -> u32 {
    s.chars()
        .map(|c| u32::try_from(c.len_utf16()).unwrap_or(1))
        .sum()
}

/// 块级前缀的**字符数**（缩进空白 + 标题/引用/列表/任务符号），用于定位正文起点。
///
/// 引用 `>` **逐层递归剥净**（`>>` 嵌套、以及引用内再嵌标题/列表如 `> - x`），其余块符号各吃一层
/// 即够定位正文。
fn block_prefix_len(line: &str) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    // 前导空白。
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    let rest: String = chars[i..].iter().collect();
    // ATX 标题：`#`..`######` + 至少一个空格。
    if let Some(after) = atx_heading_prefix(&rest) {
        return i + after;
    }
    // 引用：**逐层**剥 `>`（含 `>>` 嵌套；每层可跟一个空格），剥净后可能还是列表/标题，故继续判定。
    if rest.starts_with('>') {
        let mut consumed = 0;
        let chars_rest: Vec<char> = rest.chars().collect();
        while consumed < chars_rest.len() && chars_rest[consumed] == '>' {
            consumed += 1;
            if consumed < chars_rest.len() && chars_rest[consumed] == ' ' {
                consumed += 1;
            }
        }
        let inner: String = chars_rest[consumed..].iter().collect();
        return i + consumed + block_prefix_len(&inner);
    }
    // 任务列表：`- [ ] ` / `* [x] ` 等。
    if let Some(n) = task_list_prefix(&rest) {
        return i + n;
    }
    // 无序列表：`- ` / `* ` / `+ `。
    if let Some(first) = rest.chars().next() {
        if matches!(first, '-' | '*' | '+') && rest.chars().nth(1) == Some(' ') {
            return i + 2;
        }
    }
    // 有序列表：`1. ` / `12) `。
    if let Some(n) = ordered_list_prefix(&rest) {
        return i + n;
    }
    i
}

/// ATX 标题前缀字符数（`#{1,6}` + 空格），非标题返回 `None`。
fn atx_heading_prefix(s: &str) -> Option<usize> {
    let mut hashes = 0;
    for ch in s.chars() {
        if ch == '#' {
            hashes += 1;
        } else {
            break;
        }
    }
    if (1..=6).contains(&hashes) && s.chars().nth(hashes) == Some(' ') {
        Some(hashes + 1)
    } else {
        None
    }
}

/// 任务列表前缀字符数（`[-*+] [ [ xX] ] `），非任务返回 `None`。
fn task_list_prefix(s: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    // `[-*+]` + ' ' + '[' + (' '|'x'|'X') + ']' + ' '
    if chars.len() >= 6
        && matches!(chars[0], '-' | '*' | '+')
        && chars[1] == ' '
        && chars[2] == '['
        && matches!(chars[3], ' ' | 'x' | 'X')
        && chars[4] == ']'
        && chars[5] == ' '
    {
        Some(6)
    } else {
        None
    }
}

/// 有序列表前缀字符数（`\d+[.)]` + 空格），非有序返回 `None`。
fn ordered_list_prefix(s: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    if i < chars.len() && matches!(chars[i], '.' | ')') && chars.get(i + 1) == Some(&' ') {
        Some(i + 2)
    } else {
        None
    }
}

/// 纯排版行：**不含任何可朗读信息**，整行跳过（否则 TTS 会念出 `====` / `|---|---|` 之类噪声）。
///
/// 覆盖三类：① 水平分隔线（`---`/`***`/`___`，≥3）；② setext 标题下划线（整行 `=` 或 `-`，≥2——
/// 标题文本本身在上一行已朗读，下划线只是排版）；③ 表格分隔行（仅由 `|`、`-`、`:`、空白组成且
/// 至少各含一个 `|` 与 `-`）。
fn is_layout_only_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // ①② 整行同一符号：`-`/`*`/`_` ≥3 为分隔线，`=`/`-` ≥2 为 setext 下划线。
    for (marker, min) in [('-', 2), ('*', 3), ('_', 3), ('=', 2)] {
        if t.chars().count() >= min && t.chars().all(|c| c == marker) {
            return true;
        }
    }
    // ③ 表格分隔行 `|---|:--:|`。
    let table_charset = t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
    if table_charset && t.contains('|') && t.contains('-') {
        return true;
    }
    false
}

/// 剥离**行内** Markdown 装饰，保留可读文本（链接/图片/wiki 取显示文字，代码取内容，强调符去除）。
fn strip_inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            // 图片 `![alt](url)` → alt；**嵌入 `![[note]]`**（Obsidian transclusion）→ 显示名。
            '!' if chars.get(i + 1) == Some(&'[') => {
                if chars.get(i + 2) == Some(&'[') {
                    if let Some((text, next)) = parse_wiki_link(&chars, i + 1) {
                        out.push_str(&text);
                        i = next;
                        continue;
                    }
                }
                if let Some((text, next)) = parse_link(&chars, i + 1) {
                    out.push_str(&text);
                    i = next;
                    continue;
                }
                out.push(c);
                i += 1;
            }
            '[' => {
                // wiki 链接 `[[target|alias]]` → alias（无别名取 target）。
                if chars.get(i + 1) == Some(&'[') {
                    if let Some((text, next)) = parse_wiki_link(&chars, i) {
                        out.push_str(&text);
                        i = next;
                        continue;
                    }
                }
                // 脚注引用 `[^1]`（及定义行的 `[^1]:` 前缀）：标记打断语流，不朗读。
                if chars.get(i + 1) == Some(&'^') {
                    if let Some(close) = (i + 2..chars.len()).find(|&j| chars[j] == ']') {
                        i = close + 1;
                        if chars.get(i) == Some(&':') {
                            i += 1;
                        }
                        continue;
                    }
                }
                if let Some((text, next)) = parse_link(&chars, i) {
                    out.push_str(&text);
                    i = next;
                    continue;
                }
                out.push(c);
                i += 1;
            }
            // 内联标签 `#工作/项目`：读标签文字、不念井号；嵌套斜杠化为停顿。
            // 判据与项目内联 `#tag` 解析同构，故 `#1`（编号）与 `C#`（词内）不误伤。
            '#' if is_inline_tag_start(&chars, i) => {
                let mut j = i + 1;
                while j < chars.len() && is_tag_char(chars[j]) {
                    out.push(if chars[j] == '/' { ' ' } else { chars[j] });
                    j += 1;
                }
                i = j;
            }
            // Obsidian 注释 `%%…%%`：**连内容一并丢弃**——它在阅读视图本就不渲染，更不该被读出。
            '%' if chars.get(i + 1) == Some(&'%') => {
                if let Some(close) = find_pair(&chars, i + 2, '%', true) {
                    i = close + 2;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            // 高亮 `==重点==`：剥标记留文本；`a == b` 是比较运算（两侧皆空白）须原样保留。
            '=' if chars.get(i + 1) == Some(&'=') => {
                let prev_space = i == 0 || chars[i - 1].is_whitespace();
                let next_space = chars.get(i + 2).is_none_or(|n| n.is_whitespace());
                if prev_space && next_space {
                    out.push_str("==");
                }
                i += 2;
            }
            // 公式定界符 `$…$` / `$$…$$` 不朗读（内容保留）；但 `$100` 是金额，须原样读出。
            '$' => {
                let double = chars.get(i + 1) == Some(&'$');
                let content_start = if double { i + 2 } else { i + 1 };
                let is_amount = !double
                    && chars
                        .get(content_start)
                        .is_some_and(|n| n.is_ascii_digit() || n.is_whitespace());
                if !is_amount {
                    if let Some(close) = find_pair(&chars, content_start, '$', double) {
                        let inner: String = chars[content_start..close].iter().collect();
                        out.push(' ');
                        out.push_str(inner.trim());
                        out.push(' ');
                        i = close + if double { 2 } else { 1 };
                        continue;
                    }
                }
                out.push(c);
                i += 1;
            }
            // 转义 `\*` `\_`：读被转义的字面字符，绝不念反斜杠。
            '\\' => {
                if let Some(&next) = chars.get(i + 1) {
                    if next.is_ascii_punctuation() {
                        out.push(next);
                        i += 2;
                        continue;
                    }
                }
                out.push(c);
                i += 1;
            }
            // 行内代码：反引号成对，内部原样保留。
            '`' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '`' {
                    out.push(chars[j]);
                    j += 1;
                }
                i = if j < chars.len() { j + 1 } else { j };
            }
            // HTML 标签（`<br>` / `<span …>`）：整体剥离，绝不逐字念出尖括号。
            // 仅当 `<` 后紧跟字母或 `/` 且能在本句内闭合才算标签（`a < b` 的比较号不误伤）。
            '<' => {
                if let Some(next) = html_tag_end(&chars, i) {
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            // 表格竖线：换成空格作停顿，避免念出「竖线」或把相邻单元格粘连成一个词。
            '|' => {
                out.push(' ');
                i += 1;
            }
            // 下划线：标识符内部（两侧均为字母/数字，如 `some_var_name`）**换空格**分词朗读；
            // 否则视为强调标记去除（`__bold__`）。直接删会把标识符粘成 `somevarname` 读不成词。
            '_' => {
                let prev_alnum = i
                    .checked_sub(1)
                    .and_then(|p| chars.get(p))
                    .is_some_and(|p| p.is_alphanumeric());
                let next_alnum = chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
                if prev_alnum && next_alnum {
                    out.push(' ');
                }
                i += 1;
            }
            // 强调/删除线标记：去符号留文本。
            '*' | '~' => {
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    // 折叠多余空白。
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 内联标签 `#tag` 的起始判据（与项目内联标签解析同构）：`#` 位于**词首**（行首或空白之后），
/// 且其后紧跟至少一个非空白、非数字字符——故 `#1`（编号）与 `C#`（词内）都不会被当作标签。
fn is_inline_tag_start(chars: &[char], at: usize) -> bool {
    let word_start = at == 0 || chars[at - 1].is_whitespace();
    let next_taggable = chars
        .get(at + 1)
        .is_some_and(|n| !n.is_whitespace() && !n.is_ascii_digit() && *n != '#');
    word_start && next_taggable
}

/// 标签可含字符：字母/数字/CJK + 嵌套斜杠 + 连接符。
fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '-' | '_')
}

/// 从 `from` 起在**同句内**查找成对定界符（`double` 时为双字符如 `$$`/`%%`），返回其起始下标。
fn find_pair(chars: &[char], from: usize, marker: char, double: bool) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == marker && (!double || chars.get(j + 1) == Some(&marker)) {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// 若 `open` 处起是一个 HTML 标签（`<br>`、`</div>`、`<a href="…">`），返回其 `>` 之后的下一位置。
///
/// 判据保守：`<` 后须紧跟字母或 `/`，且在 60 字符内闭合——`a < b` 这类比较运算不会被误当标签剥掉。
fn html_tag_end(chars: &[char], open: usize) -> Option<usize> {
    let first = *chars.get(open + 1)?;
    if !(first.is_ascii_alphabetic() || first == '/') {
        return None;
    }
    let limit = (open + 60).min(chars.len());
    for (offset, ch) in chars.iter().enumerate().take(limit).skip(open + 1) {
        match ch {
            '>' => return Some(offset + 1),
            '<' => return None, // 未闭合就遇到下一个 `<`：不是标签
            _ => {}
        }
    }
    None
}

/// 解析 `[text](url)`，从 `[` 位置起；成功返回（显示文本, 结束后下一位置）。
fn parse_link(chars: &[char], open: usize) -> Option<(String, usize)> {
    if chars.get(open) != Some(&'[') {
        return None;
    }
    let mut i = open + 1;
    let mut text = String::new();
    while i < chars.len() && chars[i] != ']' {
        text.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars.get(i) != Some(&']') {
        return None;
    }
    i += 1; // 跳过 ']'
    if chars.get(i) != Some(&'(') {
        return None;
    }
    // 跳过 (url)
    while i < chars.len() && chars[i] != ')' {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    i += 1; // 跳过 ')'
    Some((strip_inline(&text), i))
}

/// 解析 `[[target|alias]]`，从首个 `[` 位置起；成功返回（显示文本, 结束后下一位置）。
fn parse_wiki_link(chars: &[char], open: usize) -> Option<(String, usize)> {
    if chars.get(open) != Some(&'[') || chars.get(open + 1) != Some(&'[') {
        return None;
    }
    let mut i = open + 2;
    let mut inner = String::new();
    while i + 1 < chars.len() && !(chars[i] == ']' && chars[i + 1] == ']') {
        inner.push(chars[i]);
        i += 1;
    }
    if i + 1 >= chars.len() {
        return None;
    }
    i += 2; // 跳过 ']]'
            // `target|alias` → alias（显示名）。
    let display = inner
        .split_once('|')
        .map_or(inner.clone(), |(_, alias)| alias.to_string());
    Some((display, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(segs: &[SpeechSegment]) -> Vec<&str> {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn plain_text_splits_by_sentence() {
        let segs = plan_speech("Hello world. How are you? Fine!", false);
        assert_eq!(texts(&segs), vec!["Hello world.", "How are you?", "Fine!"]);
    }

    #[test]
    fn plain_single_short_stays_one_segment() {
        let segs = plan_speech("demos", false);
        assert_eq!(texts(&segs), vec!["demos"]);
        assert_eq!(segs[0].start_utf16, 0);
        assert_eq!(segs[0].end_utf16, 5);
    }

    #[test]
    fn cjk_sentences_split() {
        let segs = plan_speech("你好世界。今天天气不错！", false);
        assert_eq!(texts(&segs), vec!["你好世界。", "今天天气不错！"]);
        // CJK 每字 1 个 UTF-16 码元，句号亦然。
        assert_eq!(segs[0].start_utf16, 0);
        assert_eq!(segs[0].end_utf16, 5);
        assert_eq!(segs[1].start_utf16, 5);
    }

    #[test]
    fn markdown_frontmatter_skipped() {
        let src = "---\ntitle: Test\ntags: [a]\n---\nHello there.";
        let segs = plan_speech(src, true);
        assert_eq!(texts(&segs), vec!["Hello there."]);
        // 区间应指向正文 "Hello there." 而非 frontmatter。
        let body_start = src.find("Hello").unwrap() as u32;
        assert_eq!(segs[0].start_utf16, body_start);
    }

    #[test]
    fn markdown_heading_and_emphasis_stripped() {
        let segs = plan_speech("## **Bold** heading here", true);
        assert_eq!(texts(&segs), vec!["Bold heading here"]);
        // 区间从 "**Bold**..." 起（跳过 "## "），覆盖整句原文含标记。
        assert_eq!(segs[0].start_utf16, 3);
    }

    #[test]
    fn markdown_code_fence_skipped() {
        let src = "Before.\n```rust\nlet x = 1;\n```\nAfter.";
        let segs = plan_speech(src, true);
        assert_eq!(texts(&segs), vec!["Before.", "After."]);
    }

    #[test]
    fn markdown_link_reads_display_text() {
        let segs = plan_speech("See [the docs](https://example.com) now.", true);
        assert_eq!(texts(&segs), vec!["See the docs now."]);
    }

    #[test]
    fn markdown_wiki_link_reads_alias() {
        let segs = plan_speech("Refer to [[note-id|the note]].", true);
        assert_eq!(texts(&segs), vec!["Refer to the note."]);
    }

    #[test]
    fn markdown_list_markers_stripped() {
        let src = "- first item\n- second item";
        let segs = plan_speech(src, true);
        assert_eq!(texts(&segs), vec!["first item", "second item"]);
    }

    #[test]
    fn markdown_task_and_ordered_lists() {
        let src = "- [ ] todo one\n1. ordered two";
        let segs = plan_speech(src, true);
        assert_eq!(texts(&segs), vec!["todo one", "ordered two"]);
    }

    #[test]
    fn markdown_inline_code_content_kept() {
        let segs = plan_speech("Run `cargo test` please.", true);
        assert_eq!(texts(&segs), vec!["Run cargo test please."]);
    }

    #[test]
    fn thematic_break_skipped() {
        let src = "Above.\n---\nBelow.";
        let segs = plan_speech(src, true);
        assert_eq!(texts(&segs), vec!["Above.", "Below."]);
    }

    #[test]
    fn empty_and_whitespace_yield_nothing() {
        assert!(plan_speech("", false).is_empty());
        assert!(plan_speech("   \n\t\n", true).is_empty());
        assert!(plan_speech("```\n\n```", true).is_empty());
    }

    #[test]
    fn utf16_offsets_account_for_astral_chars() {
        // 😀 (U+1F600) 是星际字符：1 个 char / 2 个 UTF-16 码元。
        let segs = plan_speech("😀 hi. bye.", false);
        assert_eq!(texts(&segs), vec!["😀 hi.", "bye."]);
        // 区间含前导空白（连续无缝）：😀(2)+空格(1)+"hi."(3) = 6 为第二句起点（含其前导空格）。
        assert_eq!(segs[1].start_utf16, 6);
        assert_eq!(segs[1].end_utf16, 11);
    }

    #[test]
    fn blockquote_marker_stripped() {
        let segs = plan_speech("> quoted line here.", true);
        assert_eq!(texts(&segs), vec!["quoted line here."]);
        assert_eq!(segs[0].start_utf16, 2);
    }

    // —— 朗读质量回归（验收发现，2026-07-24）：这些若回归，用户会听到排版噪声 ——

    #[test]
    fn setext_heading_underline_not_spoken() {
        // `====` / `---` 是排版，标题文本已在上一行朗读；念出等号是噪声。
        let segs = plan_speech("标题文字\n====\n正文。", true);
        assert_eq!(texts(&segs), vec!["标题文字", "正文。"]);
        let segs = plan_speech("标题文字\n---\n正文。", true);
        assert_eq!(texts(&segs), vec!["标题文字", "正文。"]);
    }

    #[test]
    fn table_reads_cells_without_pipes_or_separator() {
        let src = "| 姓名 | 年龄 |\n|------|------|\n| 张三 | 30 |";
        let segs = plan_speech(src, true);
        // 分隔行整行跳过；内容行竖线化为停顿，不念「竖线」也不粘连单元格。
        assert_eq!(texts(&segs), vec!["姓名 年龄", "张三 30"]);
    }

    #[test]
    fn table_alignment_separator_variants_skipped() {
        let segs = plan_speech("| a |\n|:---:|\n| b |", true);
        assert_eq!(texts(&segs), vec!["a", "b"]);
    }

    #[test]
    fn snake_case_identifier_reads_as_words() {
        // 直接删下划线会粘成 "somevariablename"，TTS 读不成词。
        let segs = plan_speech("调用 some_variable_name 这个函数。", true);
        assert_eq!(texts(&segs), vec!["调用 some variable name 这个函数。"]);
    }

    #[test]
    fn emphasis_underscores_still_stripped() {
        let segs = plan_speech("这是 __重点__ 内容。", true);
        assert_eq!(texts(&segs), vec!["这是 重点 内容。"]);
    }

    #[test]
    fn html_tags_stripped_but_comparison_kept() {
        let segs = plan_speech("换行<br>之后。", true);
        assert_eq!(texts(&segs), vec!["换行之后。"]);
        let segs = plan_speech("已知 a < b 成立。", true);
        assert_eq!(texts(&segs), vec!["已知 a < b 成立。"]);
    }

    #[test]
    fn nested_blockquote_fully_stripped() {
        let segs = plan_speech(">> 二级引用内容。", true);
        assert_eq!(texts(&segs), vec!["二级引用内容。"]);
        // 引用内的列表/标题也要剥净。
        let segs = plan_speech("> - 引用里的列表项", true);
        assert_eq!(texts(&segs), vec!["引用里的列表项"]);
        let segs = plan_speech("> ## 引用里的标题", true);
        assert_eq!(texts(&segs), vec!["引用里的标题"]);
    }

    #[test]
    fn inline_tag_reads_text_without_hash() {
        // Lunarca 自有内联标签，笔记中极常见；念「井号工作斜杠项目」是明显噪声。
        let segs = plan_speech("今天完成了 #工作/项目 的部分。", true);
        assert_eq!(texts(&segs), vec!["今天完成了 工作 项目 的部分。"]);
    }

    #[test]
    fn hash_not_treated_as_tag_when_number_or_word_internal() {
        // `#1` 是编号、`C#` 是词内井号——都不是标签，须原样读出。
        let segs = plan_speech("见 issue #1 与 C# 语言。", true);
        assert_eq!(texts(&segs), vec!["见 issue #1 与 C# 语言。"]);
    }

    #[test]
    fn highlight_marks_stripped_but_comparison_kept() {
        let segs = plan_speech("这是 ==重点内容== 需注意。", true);
        assert_eq!(texts(&segs), vec!["这是 重点内容 需注意。"]);
        // `a == b` 两侧皆空白 = 比较运算，保留。
        let segs = plan_speech("若 a == b 则成立。", true);
        assert_eq!(texts(&segs), vec!["若 a == b 则成立。"]);
    }

    #[test]
    fn embed_reads_display_name() {
        let segs = plan_speech("见 ![[架构图]] 所示。", true);
        assert_eq!(texts(&segs), vec!["见 架构图 所示。"]);
    }

    #[test]
    fn obsidian_comment_content_discarded() {
        // `%%…%%` 在阅读视图本就不渲染，更不该被读出——连内容一并丢弃。
        let segs = plan_speech("正文 %%这是隐藏注释%% 继续。", true);
        assert_eq!(texts(&segs), vec!["正文 继续。"]);
    }

    #[test]
    fn footnote_marker_dropped() {
        let segs = plan_speech("论点[^1] 成立。", true);
        assert_eq!(texts(&segs), vec!["论点 成立。"]);
    }

    #[test]
    fn math_delimiters_dropped_but_currency_kept() {
        let segs = plan_speech("$$E = mc^2$$", true);
        assert_eq!(texts(&segs), vec!["E = mc^2"]);
        let segs = plan_speech("已知 $x = 1$ 成立。", true);
        assert_eq!(texts(&segs), vec!["已知 x = 1 成立。"]);
        // 金额不是公式：`$` 后紧跟数字须原样读出。
        let segs = plan_speech("花费 $100 元。", true);
        assert_eq!(texts(&segs), vec!["花费 $100 元。"]);
    }

    #[test]
    fn escaped_punctuation_reads_literal_char() {
        // `\*` 是用户要显示的字面星号：读出该字符，但绝不念反斜杠。
        let segs = plan_speech("字面 \\*星号\\* 显示。", true);
        assert_eq!(texts(&segs), vec!["字面 *星号* 显示。"]);
    }

    #[test]
    fn detect_sample_is_bounded_and_skips_non_prose() {
        // frontmatter / 代码块已在规划期剥离，样本只含正文——不会把 YAML 键名误判成语言。
        let src = "---\ntitle: My Note\ntags: [a]\n---\n中文正文第一句。中文正文第二句。";
        let sample = detect_sample(&plan_speech(src, true));
        assert!(
            !sample.contains("title"),
            "样本不应含 frontmatter：{sample}"
        );
        assert!(sample.starts_with("中文正文第一句。"));

        // 超长文档只取开头，绝不整篇送检（同步命令的响应上限）。
        let long = "这是一个很长的句子。".repeat(500);
        let sample = detect_sample(&plan_speech(&long, false));
        assert!(sample.chars().count() <= DETECT_SAMPLE_MAX_CHARS);

        assert!(detect_sample(&[]).is_empty());
    }

    #[test]
    fn multiple_sentences_ranges_are_contiguous() {
        let segs = plan_speech("One. Two.", false);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_utf16, 0);
        assert_eq!(segs[0].end_utf16, 4); // "One."
        assert_eq!(segs[1].start_utf16, 4); // 含前导空格
        assert_eq!(segs[1].end_utf16, 9);
    }
}
