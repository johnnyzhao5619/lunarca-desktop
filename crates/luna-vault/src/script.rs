//! 视频脚本的分段与口播时长估算（ADR-162 ③④，PRD-19-010 / 011 / 012）。
//!
//! 脚本是普通 Markdown，结构只是约定：以**二级标题**分段（开头 / 各节拍 / 结尾），
//! `broll` 等代码块标注画面需求、不念出来，行首 `[口播]` / `[出镜]`（英文 `[VO]` / `[CAM]`）
//! 只标形式、不念出来。本模块从全文算出每段要念多少字词、约念多久——判定在 Rust，
//! 前端只渲染（ADR-003）。
//!
//! 时长 = 中文字数 ÷ 每分钟字数 + 英文词数 ÷ 每分钟词数。语速默认值写在
//! [`SpeakingRate::DEFAULT`]，界面一律标「约」；按用户本人录音校准是后续一批（PRD-19-012）。

use crate::frontmatter::body_of;

/// 每分钟念多少中文字、多少英文词。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeakingRate {
    /// 中文：每分钟字数。
    pub cjk_per_minute: f32,
    /// 英文：每分钟词数。
    pub words_per_minute: f32,
    /// 是否来自用户本人的录音（`false` = 默认值）。
    pub calibrated: bool,
}

impl SpeakingRate {
    /// 默认语速：中文 240 字 / 分、英文 150 词 / 分。
    ///
    /// > 假设：取口播常见区间的中段（理由：英文旁白常见 140–160 词 / 分；中文没有可靠的公开
    /// > 口径，取常见口播 200–280 字 / 分的中段。这两个数只是起点——PRD-19-012 要求按本人
    /// > 实录校准，未校准时界面标「约」）。
    pub const DEFAULT: Self = Self {
        cjk_per_minute: 240.0,
        words_per_minute: 150.0,
        calibrated: false,
    };
}

/// 脚本里的一段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSection {
    /// 段标题（二级标题的文字）；`None` = 第一个二级标题之前的那段开场。
    pub heading: Option<String>,
    /// 段标题所在行（1 起，**含 frontmatter**，与编辑器大纲同一口径）；开场段为正文首行。
    pub line: u32,
    /// 要念的中文字数。
    pub cjk_chars: u32,
    /// 要念的英文词数。
    pub words: u32,
    /// 本段里 `broll` 代码块的个数（画面需求，不念）。
    pub broll_blocks: u32,
    /// 约念多少秒。
    pub seconds: u32,
}

/// 一篇脚本的分段与总时长。
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptOutline {
    pub sections: Vec<ScriptSection>,
    /// 全篇约念多少秒（各段之和）。
    pub total_seconds: u32,
    /// 估算用的语速。
    pub rate: SpeakingRate,
}

/// 只标形式、不念出来的行首标记。
const DELIVERY_MARKERS: &[&str] = &["[口播]", "[出镜]", "[VO]", "[CAM]"];

/// 中文字：统一表意文字（含扩展 A 与兼容区），与编辑器状态栏字数同一口径。
fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}')
}

/// 一行里要念的中文字数与英文词数。链接地址、行首形式标记不念。
pub(crate) fn spoken_counts(line: &str) -> (u32, u32) {
    let mut text = line.trim_start();
    for marker in DELIVERY_MARKERS {
        if let Some(rest) = text.strip_prefix(marker) {
            text = rest;
            break;
        }
    }
    // 口播记号（注释、停顿号、强调星号）不念（ADR-176 ⑦）。
    let (spoken, _, _) = parse_cues(text);
    let mut cjk = 0u32;
    let mut words = 0u32;
    for token in spoken.split_whitespace() {
        // 裸链接与 Markdown 链接的地址部分不念（`[文字](地址)` 只念文字）。
        if token.starts_with("http://") || token.starts_with("https://") {
            continue;
        }
        let token = match token.find("](") {
            Some(at) => &token[..at],
            None => token,
        };
        let mut in_word = false;
        for ch in token.chars() {
            if is_cjk(ch) {
                cjk += 1;
                in_word = false;
            } else if ch.is_ascii_alphanumeric() {
                if !in_word {
                    words += 1;
                    in_word = true;
                }
            } else if ch == '\'' || ch == '-' {
                // 词中的撇号与连字符不断词（don't、co-host）。
            } else {
                in_word = false;
            }
        }
    }
    (cjk, words)
}

/// 送进提词器的外来文字（粘贴、剪贴板、选中文字、转换后的文件）的清洗（ADR-176 ⑨，PRD-20-020）：
/// 统一换行、去掉零宽字符与行尾空白、不换行空格改普通空格、去掉图片（提词器只念文字）、把连续空行
/// 收成一行；段落保留，文字一字不改。结果以换行结尾（空文字为空串）。
#[must_use]
pub fn normalize_pasted_script(raw: &str) -> String {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let cleaned: String = line
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
                )
            })
            .map(|c| {
                if matches!(c, '\u{00A0}' | '\u{202F}') {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let cleaned = cleaned.trim_end();
        // 单独一行的图片（转换出的 `![…](…)`，地址可能为空）不念。
        let is_image = cleaned.trim_start().starts_with("![") && cleaned.ends_with(')');
        if is_image {
            continue;
        }
        if cleaned.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        // 动作提示行后面紧跟正文时补一个空行：Markdown 的引用块会把紧跟的下一行并进去，编辑器与
        // Obsidian 里就成了提示框的一部分（真机 2026-11-04）。提词器本来就只把那一行当提示。
        if !cleaned.is_empty() && out.last().is_some_and(|prev| cue_label(prev).is_some()) {
            out.push(String::new());
        }
        out.push(cleaned.to_owned());
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\n") + "\n"
    }
}

/// 提词稿的名字（PRD-20-020）：第一行非空文字，去掉标题井号、列表符与形式标记，最多
/// [`SCRIPT_TITLE_MAX_CHARS`] 个字（按字符截，不切坏多字节字）。没有可用文字时为 `None`。
#[must_use]
pub fn script_title_of(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut line = first.trim_start_matches('#').trim_start();
    for prefix in ["- ", "* ", "> "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            line = rest.trim_start();
        }
    }
    for marker in DELIVERY_MARKERS {
        if let Some(rest) = line.strip_prefix(marker) {
            line = rest.trim_start();
        }
    }
    let title: String = line.chars().take(SCRIPT_TITLE_MAX_CHARS).collect();
    let title = title.trim().to_owned();
    (!title.is_empty()).then_some(title)
}

/// 提词稿名字最多几个字。
pub const SCRIPT_TITLE_MAX_CHARS: usize = 24;

/// 稿子的主语言（ADR-176 ④）：跟读的识别语言设成「自动」时，按它给识别器一个语言提示——短窗里
/// 的自动语种检测常在中英之间乱跳。按要念的字数比：每两个中文字算一个英文词的分量，中文多就是
/// `"zh"`，否则 `"en"`；一个字都没有为 `None`。中英混读的中文稿判为中文（Whisper 的中文模式
/// 能带出英文词，反过来不行）。
#[must_use]
pub fn script_language(content: &str) -> Option<&'static str> {
    let (cjk, words) = prompter_blocks(content)
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| spoken_counts(&line.text))
        .fold((0u32, 0u32), |(c, w), (lc, lw)| (c + lc, w + lw));
    match (cjk, words) {
        (0, 0) => None,
        (cjk, words) if cjk / 2 >= words => Some("zh"),
        _ => Some("en"),
    }
}

/// 一行要念多少毫秒（与脚本时长估算同一口径、同一语速；提词器的「预计剩余」按它累加）。
#[must_use]
pub fn spoken_millis(line: &str, rate: SpeakingRate) -> u32 {
    let (cjk, words) = spoken_counts(line);
    let minutes = f64::from(cjk) / f64::from(rate.cjk_per_minute.max(1.0))
        + f64::from(words) / f64::from(rate.words_per_minute.max(1.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let millis = (minutes * 60_000.0).round() as u32;
    millis
}

fn seconds_for(cjk: u32, words: u32, rate: SpeakingRate) -> u32 {
    let minutes = f64::from(cjk) / f64::from(rate.cjk_per_minute.max(1.0))
        + f64::from(words) / f64::from(rate.words_per_minute.max(1.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let seconds = (minutes * 60.0).round() as u32;
    seconds
}

/// 代码围栏的开头：返回围栏字符序列（```` ``` ```` 或 `~~~`）与信息串。
fn fence_open(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    for fence in ["```", "~~~"] {
        if let Some(rest) = trimmed.strip_prefix(fence) {
            let marker_len =
                fence.len() + rest.chars().take_while(|c| fence.starts_with(*c)).count();
            return Some((&trimmed[..marker_len], trimmed[marker_len..].trim()));
        }
    }
    None
}

/// 是不是二级标题行（`## 标题`）；返回标题文字。
fn h2_text(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    let text = rest.trim().trim_end_matches('#').trim();
    Some(text)
}

/// 脚本里一行的身份：分段与搬段共用同一套判定，保证「大纲里看到的段」就是「能搬的段」。
enum LineKind<'a> {
    /// 代码块内部（含收尾的围栏行）。
    Fenced,
    /// 代码块的开头围栏；`broll` = 信息串是 `broll`（画面需求）。
    FenceOpen {
        broll: bool,
    },
    /// 二级标题（段名）。
    Heading2(&'a str),
    /// 录制标记标注块（`> [!retake] …` 及其后续引用行）：给剪辑看的，不念、不计时、不对齐。
    Retake,
    Other,
}

/// 只有一条嵌入的行（`![[研究#^q-…]]`，插进脚本的引文卡片等，ADR-162 ⑭）：内容在别处，不念、
/// 不计时、不上提词器。
fn is_embed_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("![[") && trimmed.ends_with("]]") && trimmed.matches("![[").count() == 1
}

/// 录制标记标注块的类型词（Obsidian 兼容标注块，ADR-162 ⑤ C4c）。
pub const RETAKE_CALLOUT: &str = "[!retake]";

/// 是不是录制标记标注块的头一行（`> [!retake] …`，类型词不分大小写）。
fn is_retake_head(line: &str) -> bool {
    line.trim_start()
        .strip_prefix('>')
        .and_then(|rest| rest.trim_start().get(..RETAKE_CALLOUT.len()))
        .is_some_and(|head| head.eq_ignore_ascii_case(RETAKE_CALLOUT))
}

/// 逐行扫描时记住「是否在代码块里」——代码块里的 `## ` 不是段标题——以及「是否在录制标记
/// 标注块里」。
#[derive(Default)]
struct LineScanner {
    fence: Option<String>,
    retake: bool,
}

impl LineScanner {
    fn classify<'a>(&mut self, line: &'a str) -> LineKind<'a> {
        if let Some(open) = self.fence.as_deref() {
            if line.trim_start().starts_with(open)
                && line
                    .trim()
                    .trim_start_matches(open.chars().next().unwrap_or('`'))
                    .is_empty()
            {
                self.fence = None;
            }
            return LineKind::Fenced;
        }
        if self.retake {
            if line.trim_start().starts_with('>') {
                return LineKind::Retake;
            }
            self.retake = false;
        }
        if is_retake_head(line) {
            self.retake = true;
            return LineKind::Retake;
        }
        if let Some((marker, info)) = fence_open(line) {
            let broll = info.split_whitespace().next() == Some("broll");
            self.fence = Some(marker.to_owned());
            return LineKind::FenceOpen { broll };
        }
        match h2_text(line) {
            Some(text) => LineKind::Heading2(text),
            None => LineKind::Other,
        }
    }
}

/// 按二级标题分段并估算时长。
///
/// - 第一个二级标题之前有要念的内容时，单独成一段「开场」（`heading: None`）；没有就不出现。
/// - 标题本身不念（它是段名）；其余级别的标题同样只当标签。
/// - 代码块里的字一律不念；信息串为 `broll` 的代码块计入本段的画面需求数。
#[must_use]
pub fn script_outline(content: &str, rate: SpeakingRate) -> ScriptOutline {
    let body = body_of(content);
    let offset = content[..content.len() - body.len()].matches('\n').count();
    let mut sections: Vec<ScriptSection> = Vec::new();
    let mut current = ScriptSection {
        heading: None,
        line: u32::try_from(offset + 1).unwrap_or(u32::MAX),
        cjk_chars: 0,
        words: 0,
        broll_blocks: 0,
        seconds: 0,
    };
    let mut scanner = LineScanner::default();
    // 每段的停顿数（ADR-176 ⑦：时长估算计入停顿），与 `sections` 一一对应。
    let mut pauses_in_section = 0u32;
    let mut kept_pauses: Vec<u32> = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let line_no = u32::try_from(offset + index + 1).unwrap_or(u32::MAX);
        let text = match scanner.classify(line) {
            LineKind::Fenced | LineKind::Retake => continue,
            LineKind::FenceOpen { broll } => {
                current.broll_blocks += u32::from(broll);
                continue;
            }
            LineKind::Heading2(text) => text,
            LineKind::Other => {
                if line.trim_start().starts_with('#')
                    || is_embed_line(line)
                    || cue_label(line).is_some()
                {
                    continue;
                }
                let (cjk, words) = spoken_counts(line);
                current.cjk_chars += cjk;
                current.words += words;
                pauses_in_section += parse_cues(line).2;
                continue;
            }
        };
        {
            let pauses = std::mem::take(&mut pauses_in_section);
            let finished = std::mem::replace(
                &mut current,
                ScriptSection {
                    heading: Some(text.to_owned()),
                    line: line_no,
                    cjk_chars: 0,
                    words: 0,
                    broll_blocks: 0,
                    seconds: 0,
                },
            );
            if finished.heading.is_some()
                || finished.cjk_chars + finished.words + finished.broll_blocks > 0
            {
                sections.push(finished);
                kept_pauses.push(pauses);
            }
        }
    }
    if current.heading.is_some() || current.cjk_chars + current.words + current.broll_blocks > 0 {
        sections.push(current);
        kept_pauses.push(pauses_in_section);
    }
    for (section, pauses) in sections.iter_mut().zip(kept_pauses) {
        section.seconds = seconds_for(section.cjk_chars, section.words, rate)
            + (pauses * PAUSE_MILLIS + 500) / 1000;
    }
    let total_seconds = sections.iter().map(|s| s.seconds).sum();
    ScriptOutline {
        sections,
        total_seconds,
        rate,
    }
}

/// `broll` 代码块里的一个镜头（PRD-19-026 B-roll 镜头清单）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrollShot {
    /// 所在段的段名；开场为 `None`。
    pub section: Option<String>,
    /// 开头围栏所在行（1 起，含 frontmatter，与 [`ScriptSection::line`] 同一口径）。
    pub line: u32,
    /// 代码块里的文字（去掉首尾空行）。
    pub text: String,
    /// 已拍 / 已找到：围栏信息串里带 `done`（`` ```broll done ``）。状态写在脚本里，随文件走。
    pub done: bool,
}

/// 镜头完成的标记词（围栏信息串里 `broll` 之后的一个词）。
pub const BROLL_DONE: &str = "done";

/// 脚本里全部 `broll` 代码块，按出现顺序（与 [`script_outline`] 同一套分段判定）。
#[must_use]
pub fn broll_shots(content: &str) -> Vec<BrollShot> {
    let body = body_of(content);
    let offset = content[..content.len() - body.len()].matches('\n').count();
    let mut shots: Vec<BrollShot> = Vec::new();
    let mut section: Option<String> = None;
    let mut open: Option<BrollShot> = None;
    let mut scanner = LineScanner::default();
    for (index, line) in body.lines().enumerate() {
        let line_no = u32::try_from(offset + index + 1).unwrap_or(u32::MAX);
        match scanner.classify(line) {
            LineKind::FenceOpen { broll: true } => {
                let done = fence_open(line).is_some_and(|(_, info)| {
                    info.split_whitespace()
                        .skip(1)
                        .any(|word| word == BROLL_DONE)
                });
                open = Some(BrollShot {
                    section: section.clone(),
                    line: line_no,
                    text: String::new(),
                    done,
                });
            }
            LineKind::Fenced => {
                if let Some(shot) = open.as_mut() {
                    if scanner.fence.is_none() {
                        // 收尾围栏：这个镜头读完了。
                        shot.text = shot.text.trim().to_owned();
                        shots.extend(open.take());
                    } else {
                        shot.text.push_str(line);
                        shot.text.push('\n');
                    }
                }
            }
            LineKind::Heading2(text) => section = Some(text.to_owned()),
            _ => {}
        }
    }
    // 没收尾的代码块（写到一半）也算一个镜头。
    if let Some(mut shot) = open {
        shot.text = shot.text.trim().to_owned();
        shots.push(shot);
    }
    shots
}

/// 把开头围栏在 `line` 的那个镜头标成完成 / 未完成（改它的信息串：`` ```broll `` ⇄
/// `` ```broll done ``，其余词不动）。返回新全文；那一行不是 `broll` 围栏（大纲过期了）或本来就是
/// 这个状态时为 `None`。
#[must_use]
pub fn set_broll_done(content: &str, line: u32, done: bool) -> Option<String> {
    let index = usize::try_from(line).ok()?.checked_sub(1)?;
    let target = content.lines().nth(index)?;
    let (marker, info) = fence_open(target)?;
    let mut words: Vec<&str> = info.split_whitespace().collect();
    if words.first() != Some(&"broll") {
        return None;
    }
    let has = words.iter().skip(1).any(|word| *word == BROLL_DONE);
    if has == done {
        return None;
    }
    if done {
        words.insert(1, BROLL_DONE);
    } else {
        let mut first = true;
        words.retain(|word| {
            if *word == BROLL_DONE && first {
                first = false;
                return false;
            }
            true
        });
    }
    let indent = &target[..target.len() - target.trim_start().len()];
    let rewritten = format!("{indent}{marker}{}", words.join(" "));
    let mut out = String::with_capacity(content.len() + 5);
    for (i, original) in content.split_inclusive('\n').enumerate() {
        if i == index {
            out.push_str(&rewritten);
            if original.ends_with("\r\n") {
                out.push_str("\r\n");
            } else if original.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(original);
        }
    }
    Some(out)
}

/// 搬一段（PRD-19-011 大纲拖动调段序）：把标题在 `from_line` 的那一段整段搬到标题在
/// `before_line` 的那一段之前；`before_line` 为 `None` = 搬到最后。
///
/// 一段 = 从它的二级标题行起、到下一个二级标题行之前（或全文末尾），段后的空行、`broll`
/// 代码块随段走；第一个二级标题之前的开场不动。行号与 [`ScriptSection::line`] 同一口径
/// （含 frontmatter，1 起）。返回新全文与被搬那段标题的新行号；行号对不上任何一段（大纲
/// 过期了）或搬完位置不变时返回 `None`——调用方什么都不做，而不是猜一个位置。
#[must_use]
pub fn move_section(
    content: &str,
    from_line: u32,
    before_line: Option<u32>,
) -> Option<(String, u32)> {
    let body = body_of(content);
    let body_start = content.len() - body.len();
    let offset = content[..body_start].matches('\n').count();
    let mut scanner = LineScanner::default();
    // 各段标题行：(行号, 字节起点)。
    let mut starts: Vec<(u32, usize)> = Vec::new();
    let mut at = body_start;
    for (index, raw) in body.split_inclusive('\n').enumerate() {
        if matches!(
            scanner.classify(raw.trim_end_matches(['\n', '\r'])),
            LineKind::Heading2(_)
        ) {
            starts.push((u32::try_from(offset + index + 1).unwrap_or(u32::MAX), at));
        }
        at += raw.len();
    }
    let find = |line: u32| starts.iter().position(|(l, _)| *l == line);
    let from = find(from_line)?;
    let before = match before_line {
        Some(line) => find(line)?,
        None => starts.len(),
    };
    if before == from || before == from + 1 {
        return None;
    }
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    // 末段可能没有收尾换行；搬到中间时补上，免得与下一段的标题粘成一行。
    let mut chunks: Vec<String> = starts
        .iter()
        .enumerate()
        .map(|(i, (_, start))| {
            let end = starts.get(i + 1).map_or(content.len(), |(_, next)| *next);
            let mut chunk = content[*start..end].to_owned();
            if !chunk.ends_with('\n') {
                chunk.push_str(newline);
            }
            chunk
        })
        .collect();
    // 段与段之间原来空几行（按第一段的段尾）：末段本来没有段后空行，搬到中间后要补到与别的段
    // 一样，否则它会与下一段的标题紧贴在一起（真机 2026-10-19：「我们下期见。」后面直接是「## 开头」）。
    let separator = if chunks.len() > 1 {
        trailing_newlines(&chunks[0], newline).max(1)
    } else {
        1
    };
    let moved = chunks.remove(from);
    let target = if before > from { before - 1 } else { before };
    chunks.insert(target, moved);
    let last = chunks.len() - 1;
    for chunk in &mut chunks[..last] {
        for _ in trailing_newlines(chunk, newline)..separator {
            chunk.push_str(newline);
        }
    }
    let region_start = starts[0].1;
    let mut out = String::with_capacity(content.len() + newline.len());
    out.push_str(&content[..region_start]);
    for chunk in &chunks {
        out.push_str(chunk);
    }
    // 文末的换行保持原文的样子：新的末段自带的段后空行搬到文末没有意义，原文没有收尾换行也不凭空多一个。
    let trailing = &content[content.trim_end_matches(['\n', '\r']).len()..];
    out.truncate(out.trim_end_matches(['\n', '\r']).len());
    out.push_str(trailing);
    let lines_before = content[..region_start].matches('\n').count()
        + chunks[..target]
            .iter()
            .map(|chunk| chunk.matches('\n').count())
            .sum::<usize>();
    Some((out, u32::try_from(lines_before + 1).unwrap_or(u32::MAX)))
}

/// 脚本里已写回的一处录制标记（ADR-165 ⑧，录制页签列出来；C4c 写回的 `> [!retake] …`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetakeNote {
    /// 被标的那一行（1 起，含 frontmatter，与编辑器行号同一口径）。
    pub line: u32,
    /// 被标的那一行的文字（去掉首尾空白）。
    pub text: String,
    /// 标注块里写的说明（如「重来 ×2 · 20:41、20:43」）。
    pub note: String,
}

/// 读出脚本里已写回的录制标记：每个 `> [!retake]` 标注块对应它上方最近的一行非空正文。
/// 代码块里的不算；标在文首（上方没有正文）的也不算。
#[must_use]
pub fn retake_notes(content: &str) -> Vec<RetakeNote> {
    let mut notes = Vec::new();
    let mut fence: Option<String> = None;
    let mut last_text: Option<(u32, String)> = None;
    for (index, raw) in content.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str()) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker: String = trimmed
                .chars()
                .take_while(|c| *c == '`' || *c == '~')
                .collect();
            fence = Some(marker);
            continue;
        }
        if is_retake_head(line) {
            if let Some((target, text)) = &last_text {
                let after = trimmed
                    .strip_prefix('>')
                    .map(str::trim_start)
                    .and_then(|rest| rest.get(RETAKE_CALLOUT.len()..))
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                notes.push(RetakeNote {
                    line: *target,
                    text: text.clone(),
                    note: after,
                });
            }
            continue;
        }
        // 引用与标注块的续行（`> …`）不是可被标的正文。
        if !trimmed.is_empty() && !trimmed.starts_with('>') {
            let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
            last_text = Some((number, trimmed.trim_end().to_owned()));
        }
    }
    notes
}

/// 文本末尾连续有几个换行（`newline` 为 `\n` 或 `\r\n`）。
fn trailing_newlines(text: &str, newline: &str) -> usize {
    let mut count = 0;
    let mut rest = text;
    while let Some(stripped) = rest.strip_suffix(newline) {
        count += 1;
        rest = stripped;
    }
    count
}

/// 提词器一行里的一段（口播记号，ADR-176 ⑦，PRD-20-014）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrompterSpan {
    /// 要念的文字。
    Text(String),
    /// 强调（`**词**`）：要念，显示时加粗加色。
    Emphasis(String),
    /// 停一拍（`//` 或 `[停顿]` / `[pause]`）：不念，时长估算加 [`PAUSE_MILLIS`]。
    Pause,
    /// 不念的提示（`%% … %%`，Obsidian 注释语法）：显示成灰色小字，不计时、不对齐。
    Note(String),
}

/// 一个停顿记号在时长估算里算多久（毫秒）。
pub const PAUSE_MILLIS: u32 = 600;

/// 动作提示标注块的类型（`> [!cue] 举起产品`，ADR-176 ⑦）：整行不念，提词器上显示成彩色条。
pub const CUE_CALLOUT: &str = "[!cue]";

/// 提词器上的一行（C4a，PRD-19-016）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrompterLine {
    /// 要念的文字（已去掉行首形式标记、列表符号与口播记号）：跟读对齐、录制标记、时长都用它。
    pub text: String,
    /// 行首形式标记里的字（`口播` / `出镜` / `VO` / `CAM`），作为小标签显示，不念。
    pub marker: Option<String>,
    /// 显示用的各段（口播记号拆开后的样子，ADR-176 ⑦）。
    pub spans: Vec<PrompterSpan>,
    /// 这一行有几个停顿记号（时长估算加 [`PAUSE_MILLIS`] × 这个数）。
    pub pauses: u32,
    /// 动作提示行（`> [!cue] …`）：整行不念，`spans` 里是提示的字。
    pub cue: bool,
}

/// 一行文字里的口播记号拆开（ADR-176 ⑦）：返回（要念的文字、显示用的各段、停顿数）。
///
/// - `%% … %%` 不念；`**…**` 要念（去掉星号）；`[停顿]` / `[pause]` 与前后都是空白（或行首行尾）的
///   `//` 是停顿——`https://` 这种紧贴字母的不算。没闭合的 `%%` / `**` 照原样当文字。
/// - 要念的文字里停顿处补一个空格，再把连续空白收成一个。
#[must_use]
pub fn parse_cues(text: &str) -> (String, Vec<PrompterSpan>, u32) {
    let mut spans: Vec<PrompterSpan> = Vec::new();
    let mut buf = String::new();
    let mut pauses = 0u32;
    let flush = |buf: &mut String, spans: &mut Vec<PrompterSpan>| {
        if !buf.is_empty() {
            spans.push(PrompterSpan::Text(std::mem::take(buf)));
        }
    };
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("%%") {
            if let Some(end) = after.find("%%") {
                flush(&mut buf, &mut spans);
                let note = after[..end].trim();
                if !note.is_empty() {
                    spans.push(PrompterSpan::Note(note.to_owned()));
                }
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**").filter(|end| *end > 0) {
                flush(&mut buf, &mut spans);
                spans.push(PrompterSpan::Emphasis(after[..end].to_owned()));
                rest = &after[end + 2..];
                continue;
            }
        }
        let worded = ["[停顿]", "[pause]"]
            .iter()
            .find(|word| {
                rest.get(..word.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(word))
            })
            .map(|word| word.len());
        // `//` 只在两边都是空白（或行首行尾、紧挨别的记号）时算停顿：`https://` 不算。
        let slashes = rest.starts_with("//")
            && buf.chars().last().is_none_or(char::is_whitespace)
            && rest[2..].chars().next().is_none_or(char::is_whitespace);
        if let Some(len) = worded.or_else(|| slashes.then_some(2)) {
            flush(&mut buf, &mut spans);
            spans.push(PrompterSpan::Pause);
            pauses += 1;
            rest = &rest[len..];
            continue;
        }
        buf.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut buf, &mut spans);
    let spoken: String = spans
        .iter()
        .map(|span| match span {
            PrompterSpan::Text(text) | PrompterSpan::Emphasis(text) => text.as_str(),
            PrompterSpan::Pause => " ",
            PrompterSpan::Note(_) => "",
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (spoken, spans, pauses)
}

/// 动作提示行（`> [!cue] 举起产品`）的提示文字；不是动作提示行为 `None`。
fn cue_label(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('>')?.trim_start();
    let head = rest.get(..CUE_CALLOUT.len())?;
    head.eq_ignore_ascii_case(CUE_CALLOUT)
        .then(|| rest[CUE_CALLOUT.len()..].trim())
}

/// 在新稿里找回改稿前的那一行（ADR-176 ⑦，PRD-20-015）：要念的文字一样的行里离原位置最近的
/// 那一行；一行都对不上时落在离原段号行号最近的那一行。稿子是空的为 `None`。
#[must_use]
pub fn relocate_line(
    blocks: &[PrompterBlock],
    text: &str,
    block: u32,
    line: u32,
) -> Option<(u32, u32)> {
    let flat: Vec<(u32, u32, &str)> = blocks
        .iter()
        .enumerate()
        .flat_map(|(b, blk)| {
            blk.lines.iter().enumerate().map(move |(l, ln)| {
                (
                    u32::try_from(b).unwrap_or(u32::MAX),
                    u32::try_from(l).unwrap_or(u32::MAX),
                    ln.text.as_str(),
                )
            })
        })
        .collect();
    let distance = |b: u32, l: u32| -> u64 {
        u64::from(b.abs_diff(block)) * 10_000 + u64::from(l.abs_diff(line))
    };
    let same_text = (!text.trim().is_empty())
        .then(|| {
            flat.iter()
                .filter(|(_, _, t)| *t == text)
                .min_by_key(|(b, l, _)| distance(*b, *l))
        })
        .flatten();
    same_text
        .or_else(|| flat.iter().min_by_key(|(b, l, _)| distance(*b, *l)))
        .map(|(b, l, _)| (*b, *l))
}

/// 提词器上的一段：一个二级标题下要念的各行。第一个二级标题之前有要念的内容时，单独成
/// 一段（`heading: None`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrompterBlock {
    pub heading: Option<String>,
    pub lines: Vec<PrompterLine>,
}

/// 提词器一行在源文件里落在哪（写回录制标记用）：第几段第几行、这一行（含换行）在全文里
/// 结束的字节位置。
struct SourcedLine {
    block: usize,
    line: usize,
    end: usize,
}

/// 从脚本全文抽出提词器要显示的文字（ADR-162 ⑤ C4a）：按二级标题分段；frontmatter、代码块
/// （含 `broll`）、录制标记标注块与其它级别的标题不显示；行首形式标记拆成标签；空行只是
/// 分隔。与分段、时长共用同一套判定，所以大纲里看到的段就是提词器上的段。
#[must_use]
pub fn prompter_blocks(content: &str) -> Vec<PrompterBlock> {
    walk_prompter(content).0
}

/// [`prompter_blocks`] 的本体，顺带记下每一行在源文件里的结束位置。
fn walk_prompter(content: &str) -> (Vec<PrompterBlock>, Vec<SourcedLine>) {
    let mut blocks: Vec<PrompterBlock> = Vec::new();
    let mut sourced: Vec<SourcedLine> = Vec::new();
    let mut current = PrompterBlock {
        heading: None,
        lines: Vec::new(),
    };
    let body = body_of(content);
    let mut at = content.len() - body.len();
    let mut scanner = LineScanner::default();
    for raw in body.split_inclusive('\n') {
        at += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        match scanner.classify(line) {
            LineKind::Fenced | LineKind::FenceOpen { .. } | LineKind::Retake => {}
            LineKind::Heading2(text) => {
                let finished = std::mem::replace(
                    &mut current,
                    PrompterBlock {
                        heading: Some(text.to_owned()),
                        lines: Vec::new(),
                    },
                );
                if finished.heading.is_some() || !finished.lines.is_empty() {
                    blocks.push(finished);
                }
            }
            LineKind::Other => {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || is_embed_line(trimmed) {
                    continue;
                }
                // 动作提示行（ADR-176 ⑦）：整行不念，照样占一行（录制标记、跟读的行号都按它数）。
                if let Some(label) = cue_label(trimmed) {
                    sourced.push(SourcedLine {
                        block: blocks.len(),
                        line: current.lines.len(),
                        end: at,
                    });
                    current.lines.push(PrompterLine {
                        text: String::new(),
                        marker: None,
                        spans: vec![PrompterSpan::Text(label.to_owned())],
                        pauses: 0,
                        cue: true,
                    });
                    continue;
                }
                let trimmed = trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("* "))
                    .unwrap_or(trimmed)
                    .trim_start();
                let (marker, text) = match DELIVERY_MARKERS
                    .iter()
                    .find(|marker| trimmed.starts_with(*marker))
                {
                    Some(marker) => (
                        Some(marker.trim_matches(['[', ']']).to_owned()),
                        trimmed[marker.len()..].trim(),
                    ),
                    None => (None, trimmed),
                };
                if !text.is_empty() || marker.is_some() {
                    // 有字的段一定会进 `blocks`，所以它的下标就是此刻已收下的段数。
                    sourced.push(SourcedLine {
                        block: blocks.len(),
                        line: current.lines.len(),
                        end: at,
                    });
                    let (spoken, spans, pauses) = parse_cues(text);
                    current.lines.push(PrompterLine {
                        text: spoken,
                        marker,
                        spans,
                        pauses,
                        cue: false,
                    });
                }
            }
        }
    }
    if current.heading.is_some() || !current.lines.is_empty() {
        blocks.push(current);
    }
    (blocks, sourced)
}

/// 一条要写回脚本的录制标记（ADR-162 ⑤ C4c，PRD-19-017）：标在提词器第几段第几行，那一行
/// 当时的文字，以及标注块标题（次数与时刻，由调用方按界面语言拼好）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetakeMark {
    pub block: u32,
    pub line: u32,
    pub text: String,
    pub title: String,
}

/// 把录制标记写回脚本：每条紧跟它标的那一行之后插入一个 `> [!retake] 标题` 标注块（与上下文
/// 之间留空行，免得被并进同一段）；那一行后面已有录制标记时排在它们之后，按时间先后读。
///
/// 定位先按段号行号、文字对得上才算；脚本在这期间改过、对不上时按文字找第一处相同的行；
/// 再找不到就不写这一条（不猜位置）。返回新全文与写进去的条数。
#[must_use]
pub fn insert_retake_marks(content: &str, marks: &[RetakeMark]) -> (String, usize) {
    let (blocks, sourced) = walk_prompter(content);
    let text_of = |line: &SourcedLine| blocks[line.block].lines[line.line].text.as_str();
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for mark in marks {
        let exact = sourced.iter().find(|line| {
            u32::try_from(line.block).ok() == Some(mark.block)
                && u32::try_from(line.line).ok() == Some(mark.line)
                && text_of(line) == mark.text
        });
        let Some(target) = exact.or_else(|| sourced.iter().find(|line| text_of(line) == mark.text))
        else {
            continue;
        };
        let at = after_existing_retakes(content, target.end);
        let mut block = String::new();
        if !content[..at].ends_with('\n') {
            block.push_str(newline);
        }
        block.push_str(newline);
        block.push_str("> ");
        block.push_str(RETAKE_CALLOUT);
        block.push(' ');
        block.push_str(&mark.title);
        let next_is_blank = content[at..]
            .split_inclusive('\n')
            .next()
            .is_none_or(|next| next.trim().is_empty());
        // 文末：原文以换行收尾就照样收尾，没有就不凭空多一个。
        if at < content.len() || content.ends_with('\n') {
            block.push_str(newline);
        }
        if !next_is_blank {
            block.push_str(newline);
        }
        inserts.push((at, block));
    }
    let written = inserts.len();
    // 从后往前插，前面的位置不受影响；同一处的多条后给的先插，最后读起来仍是给定的先后。
    let mut order: Vec<(usize, (usize, String))> = inserts.into_iter().enumerate().collect();
    order.sort_by_key(|(index, (at, _))| (std::cmp::Reverse(*at), std::cmp::Reverse(*index)));
    let mut out = content.to_owned();
    for (_, (at, block)) in order {
        out.insert_str(at, &block);
    }
    (out, written)
}

/// 从 `at`（某一行的结束处）往后跳过「空行 + 录制标记标注块」，返回最后一个标注块的结束处；
/// 后面没有录制标记就原样返回 `at`。
fn after_existing_retakes(content: &str, at: usize) -> usize {
    let mut end = at;
    let mut cursor = at;
    let mut in_block = false;
    for raw in content[at..].split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        if in_block && line.trim_start().starts_with('>') {
            cursor += raw.len();
            end = cursor;
            continue;
        }
        in_block = false;
        if is_retake_head(line) {
            in_block = true;
            cursor += raw.len();
            end = cursor;
        } else if line.trim().is_empty() {
            cursor += raw.len();
        } else {
            break;
        }
    }
    end
}

/// 用新正文替换 `## heading` 这一节（从这个标题到下一个二级标题之前）；没有这一节就追加到
/// 末尾。与分段同一判定：代码块里的 `##` 不算标题。发布包把章节、简介写进发布笔记时用它
/// （ADR-162 ⑪），用户在这篇笔记里写的其它节原样保留。
#[must_use]
pub fn upsert_section(content: &str, heading: &str, section: &str) -> String {
    let body = body_of(content);
    let body_start = content.len() - body.len();
    let mut scanner = LineScanner::default();
    let mut found: Option<usize> = None;
    let mut end: Option<usize> = None;
    let mut at = body_start;
    for raw in body.split_inclusive('\n') {
        if let LineKind::Heading2(text) = scanner.classify(raw.trim_end_matches(['\n', '\r'])) {
            if found.is_some() {
                end = Some(at);
                break;
            }
            if text == heading {
                found = Some(at);
            }
        }
        at += raw.len();
    }
    let block = format!("## {heading}\n\n{}\n", section.trim_end());
    match found {
        Some(start) => {
            let rest = end.map_or("", |end| &content[end..]);
            let separator = if rest.is_empty() { "" } else { "\n" };
            format!("{}{block}{separator}{rest}", &content[..start])
        }
        None => {
            let head = content.trim_end();
            if head.is_empty() {
                block
            } else {
                format!("{head}\n\n{block}")
            }
        }
    }
}

/// `## heading` 这一节的正文（不含标题行，去掉首尾空行）；没有这一节为 `None`。与
/// [`upsert_section`] 同一判定，代码块里的 `##` 不算标题。
#[must_use]
pub fn section_body(content: &str, heading: &str) -> Option<String> {
    let mut scanner = LineScanner::default();
    let mut inside = false;
    let mut body = String::new();
    for raw in body_of(content).split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        if let LineKind::Heading2(text) = scanner.classify(line) {
            if inside {
                break;
            }
            inside = text == heading;
            continue;
        }
        if inside {
            body.push_str(raw);
        }
    }
    inside.then(|| body.trim_matches(['\n', '\r']).trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retake_notes_point_at_the_line_above_and_skip_code_blocks() {
        let script = "---\nluna:\n  type: script\n---\n## 开头\n大家好。\n\n> [!retake] 重来 ×2 · 20:41、20:43\n\n今天聊内存。\n```broll\n> [!retake] 这不算\n```\n> [!RETAKE] 重来 · 20:50\n";
        let notes = retake_notes(script);
        assert_eq!(
            notes,
            [
                RetakeNote {
                    line: 6,
                    text: "大家好。".to_owned(),
                    note: "重来 ×2 · 20:41、20:43".to_owned()
                },
                RetakeNote {
                    line: 10,
                    text: "今天聊内存。".to_owned(),
                    note: "重来 · 20:50".to_owned()
                },
            ]
        );
        assert!(retake_notes("> [!retake] 文首没有正文\n").is_empty());
    }

    #[test]
    fn moving_the_last_section_up_keeps_a_blank_line_after_it() {
        // 末段没有段后空行：搬到前面后，它与下一段之间补上与别的段一样的空行。
        let content = "开场\n\n## 开头\n第一句\n\n## 结尾\n收尾\n";
        let (moved, line) = move_section(content, 6, Some(3)).expect("能搬");
        assert_eq!(moved, "开场\n\n## 结尾\n收尾\n\n## 开头\n第一句\n");
        assert_eq!(line, 3);
        // 原文段与段之间不空行，就不凭空加。
        let tight = "## 开头\n第一句\n## 结尾\n收尾";
        let (moved, _) = move_section(tight, 3, Some(1)).expect("能搬");
        assert_eq!(moved, "## 结尾\n收尾\n## 开头\n第一句");
    }

    const SCRIPT: &str = "---\nluna:\n  type: script\n---\n## 开头\n\n[口播] 你好，今天聊三件事。\n\n## 节拍 1\n\n这一段讲 Lunarca 的 local-first 设计。\n\n```broll\n屏幕录制：打开设置\n```\n\n## 结尾\n";

    #[test]
    fn sections_follow_h2_headings_and_lines_include_the_frontmatter() {
        let outline = script_outline(SCRIPT, SpeakingRate::DEFAULT);
        let headings: Vec<_> = outline
            .sections
            .iter()
            .map(|s| s.heading.as_deref())
            .collect();
        assert_eq!(headings, [Some("开头"), Some("节拍 1"), Some("结尾")]);
        // frontmatter 占 4 行，第一个二级标题在第 5 行（与编辑器大纲同一口径）。
        assert_eq!(outline.sections[0].line, 5);
        assert_eq!(outline.sections[1].line, 9);
    }

    #[test]
    fn markers_code_blocks_and_headings_are_not_spoken() {
        let outline = script_outline(SCRIPT, SpeakingRate::DEFAULT);
        // 「你好今天聊三件事」8 个字；「[口播]」不念。
        assert_eq!(outline.sections[0].cjk_chars, 8);
        assert_eq!(outline.sections[0].words, 0);
        // 「这一段讲的设计」7 个字 + Lunarca、local-first 两个词；broll 里的字不念。
        assert_eq!(outline.sections[1].cjk_chars, 7);
        assert_eq!(outline.sections[1].words, 2);
        assert_eq!(outline.sections[1].broll_blocks, 1);
        // 只有标题的空段照样列出（用户要看到「结尾」还没写），时长为 0。
        assert_eq!(outline.sections[2].seconds, 0);
    }

    #[test]
    fn duration_is_chars_over_rate_plus_words_over_rate() {
        let text = format!("## A\n{}\n", "字".repeat(240));
        let outline = script_outline(&text, SpeakingRate::DEFAULT);
        assert_eq!(outline.sections[0].seconds, 60);
        let words = format!("## B\n{}\n", "word ".repeat(150));
        assert_eq!(
            script_outline(&words, SpeakingRate::DEFAULT).total_seconds,
            60
        );
        let fast = SpeakingRate {
            cjk_per_minute: 480.0,
            words_per_minute: 300.0,
            calibrated: true,
        };
        assert_eq!(script_outline(&text, fast).total_seconds, 30);
    }

    #[test]
    fn an_opening_before_the_first_heading_is_its_own_section_only_when_it_has_words() {
        let outline = script_outline("# 标题\n先说一句。\n## 开头\n好。\n", SpeakingRate::DEFAULT);
        assert_eq!(outline.sections[0].heading, None);
        assert_eq!(outline.sections[0].cjk_chars, 4);
        assert_eq!(outline.sections[0].line, 1);
        let no_opening = script_outline("# 标题\n\n## 开头\n好。\n", SpeakingRate::DEFAULT);
        assert_eq!(no_opening.sections.len(), 1);
    }

    #[test]
    fn links_say_their_text_not_their_address() {
        let outline = script_outline(
            "## A\n看 https://example.com/x 和 [官网](https://lunarca.app)\n",
            SpeakingRate::DEFAULT,
        );
        // 「看」「和」「官网」= 4 个字；两个地址都不念。
        assert_eq!(outline.sections[0].cjk_chars, 4);
        assert_eq!(outline.sections[0].words, 0);
    }

    #[test]
    fn an_unclosed_fence_swallows_the_rest_instead_of_counting_code() {
        let outline = script_outline("## A\n```\n很多代码\n", SpeakingRate::DEFAULT);
        assert_eq!(outline.sections[0].cjk_chars, 0);
    }

    const MOVABLE: &str = "---\nluna:\n  type: script\n---\n先打个招呼。\n\n## 开头\n你好。\n\n## 节拍 1\n第一件事。\n\n```broll\n## 这不是标题\n```\n\n## 结尾\n再见。";

    fn headings(content: &str) -> Vec<String> {
        script_outline(content, SpeakingRate::DEFAULT)
            .sections
            .into_iter()
            .filter_map(|s| s.heading)
            .collect()
    }

    #[test]
    fn a_section_moves_whole_with_its_blank_lines_and_broll_blocks() {
        // 「节拍 1」（第 10 行）搬到「开头」（第 7 行）之前。
        let (moved, line) = move_section(MOVABLE, 10, Some(7)).unwrap();
        assert_eq!(headings(&moved), ["节拍 1", "开头", "结尾"]);
        assert!(moved.contains("## 节拍 1\n第一件事。\n\n```broll\n## 这不是标题\n```\n\n## 开头"));
        // 开场与 frontmatter 原样不动；被搬那段的标题落在第 7 行。
        assert!(moved.starts_with("---\nluna:\n  type: script\n---\n先打个招呼。\n\n## 节拍 1"));
        assert_eq!(line, 7);
        // 原文没有收尾换行，搬完也没有。
        assert!(moved.ends_with("再见。"));
    }

    #[test]
    fn the_last_section_gets_a_newline_when_it_moves_into_the_middle() {
        let (moved, line) = move_section(MOVABLE, 17, Some(7)).unwrap();
        assert_eq!(headings(&moved), ["结尾", "开头", "节拍 1"]);
        // 末段搬到中间：补上与别的段一样的段后空行，不与下一段的标题紧贴。
        assert!(moved.contains("## 结尾\n再见。\n\n## 开头"));
        assert_eq!(line, 7);
        assert!(!moved.ends_with('\n'));
        // 搬到最后。
        let (to_end, line) = move_section(MOVABLE, 7, None).unwrap();
        assert_eq!(headings(&to_end), ["节拍 1", "结尾", "开头"]);
        assert_eq!(
            script_outline(&to_end, SpeakingRate::DEFAULT)
                .sections
                .last()
                .unwrap()
                .line,
            line
        );
    }

    #[test]
    fn a_heading_inside_a_code_block_is_not_a_section_and_stale_lines_do_nothing() {
        // 第 14 行是 broll 代码块里的「## 这不是标题」。
        assert_eq!(move_section(MOVABLE, 14, Some(7)), None);
        assert_eq!(move_section(MOVABLE, 10, Some(99)), None);
        // 搬到自己前面或紧挨着的下一段前面 = 位置不变。
        assert_eq!(move_section(MOVABLE, 10, Some(10)), None);
        assert_eq!(move_section(MOVABLE, 10, Some(17)), None);
        assert_eq!(move_section(MOVABLE, 17, None), None);
    }

    #[test]
    fn the_end_of_the_file_keeps_its_original_newline() {
        let script = "## 开头\n你好。\n\n## 结尾\n再见。\n";
        let (moved, _) = move_section(script, 1, None).unwrap();
        assert_eq!(moved, "## 结尾\n再见。\n\n## 开头\n你好。\n");
    }

    #[test]
    fn moving_keeps_every_character_and_crlf_files_stay_crlf() {
        let crlf = MOVABLE.replace('\n', "\r\n");
        let (moved, _) = move_section(&crlf, 17, Some(10)).unwrap();
        assert!(!moved.replace("\r\n", "").contains('\n'));
        let sorted = |s: &str| {
            let mut chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
            chars.sort_unstable();
            chars
        };
        assert_eq!(sorted(&moved), sorted(&crlf));
        assert_eq!(headings(&moved), ["开头", "结尾", "节拍 1"]);
    }

    #[test]
    fn a_section_is_replaced_in_place_or_appended_and_other_sections_stay() {
        let note =
            "## 简介\n旧简介\n\n## 章节\n0:00 旧\n\n```md\n## 章节\n```\n\n## 备注\n别动我\n";
        let updated = upsert_section(note, "章节", "0:00 开场\n1:00 正文\n");
        assert_eq!(
            updated,
            "## 简介\n旧简介\n\n## 章节\n\n0:00 开场\n1:00 正文\n\n## 备注\n别动我\n"
        );
        // 没有这一节就追加；空笔记就只有这一节。
        assert_eq!(
            upsert_section("## 简介\n文字", "章节", "0:00 a"),
            "## 简介\n文字\n\n## 章节\n\n0:00 a\n"
        );
        assert_eq!(upsert_section("", "章节", "0:00 a"), "## 章节\n\n0:00 a\n");
        // 代码块里的同名标题不算。
        let fenced = "```\n## 章节\n```\n";
        assert!(upsert_section(fenced, "章节", "x").starts_with("```\n## 章节\n```\n\n## 章节"));
    }

    #[test]
    fn a_sections_body_is_read_back_and_missing_sections_are_none() {
        let note = "## 章节\n\n0:00 开场\n1:00 正文\n\n```\n## 不是标题\n```\n\n## 链接\n- 官网\n";
        assert_eq!(
            section_body(note, "章节").as_deref(),
            Some("0:00 开场\n1:00 正文\n\n```\n## 不是标题\n```")
        );
        assert_eq!(section_body(note, "链接").as_deref(), Some("- 官网"));
        assert_eq!(section_body(note, "简介"), None);
        // 写进去再读出来是同一段。
        let written = upsert_section("", "简介", "第一段\n\n第二段");
        assert_eq!(
            section_body(&written, "简介").as_deref(),
            Some("第一段\n\n第二段")
        );
    }

    #[test]
    fn the_prompter_shows_only_what_is_read_aloud_split_by_section() {
        let script = "---\nluna:\n  type: script\n---\n# 标题\n先打个招呼。\n\n## 开头\n[口播] 你好。\n- 今天聊三件事\n\n```broll\n街景空镜\n```\n### 小节\n[CAM] Look here.\n\n## 结尾\n";
        let blocks = prompter_blocks(script);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].heading, None);
        assert_eq!(blocks[0].lines[0].text, "先打个招呼。");
        assert_eq!(blocks[1].heading.as_deref(), Some("开头"));
        let plain = |text: &str, marker: Option<&str>| PrompterLine {
            text: text.into(),
            marker: marker.map(Into::into),
            spans: vec![PrompterSpan::Text(text.into())],
            pauses: 0,
            cue: false,
        };
        assert_eq!(
            blocks[1].lines,
            [
                plain("你好。", Some("口播")),
                plain("今天聊三件事", None),
                plain("Look here.", Some("CAM")),
            ]
        );
        // 只有标题的空段照样列出（念到哪一段一目了然）。
        assert_eq!(blocks[2].heading.as_deref(), Some("结尾"));
        assert!(blocks[2].lines.is_empty());
    }

    fn mark(block: u32, line: u32, text: &str, title: &str) -> RetakeMark {
        RetakeMark {
            block,
            line,
            text: text.into(),
            title: title.into(),
        }
    }

    #[test]
    fn retake_callouts_are_not_read_timed_or_shown() {
        let plain = "## 开头\n大家好。\n\n很多人写完稿子。\n";
        let marked = "## 开头\n大家好。\n\n> [!retake] 重来 ×2 · 20:41、20:43\n> 补一句备注\n\n很多人写完稿子。\n";
        assert_eq!(prompter_blocks(marked), prompter_blocks(plain));
        let rate = SpeakingRate::DEFAULT;
        assert_eq!(
            script_outline(marked, rate).sections[0].cjk_chars,
            script_outline(plain, rate).sections[0].cjk_chars
        );
        // 普通引用照念：只跳过录制标记。
        assert_eq!(
            prompter_blocks("> 引一句话\n")[0].lines[0].text,
            "> 引一句话"
        );
    }

    #[test]
    fn marks_land_right_after_their_line_and_after_earlier_marks() {
        let script =
            "## 开头\n大家好。\n\n> [!Retake] 重来 · 20:30\n\n很多人写完稿子。\n## 结尾\n下期见。";
        let (out, written) = insert_retake_marks(
            script,
            &[
                mark(0, 0, "大家好。", "重来 · 20:41"),
                mark(0, 1, "很多人写完稿子。", "重来 ×2 · 20:42、20:43"),
                mark(1, 0, "下期见。", "重来 · 20:44"),
            ],
        );
        assert_eq!(written, 3);
        assert_eq!(
            out,
            "## 开头\n大家好。\n\n> [!Retake] 重来 · 20:30\n\n> [!retake] 重来 · 20:41\n\n很多人写完稿子。\n\n> [!retake] 重来 ×2 · 20:42、20:43\n\n## 结尾\n下期见。\n\n> [!retake] 重来 · 20:44"
        );
        // 写回之后提词器上的字一行没变。
        assert_eq!(prompter_blocks(&out), prompter_blocks(script));
    }

    #[test]
    fn a_line_that_moved_is_found_by_its_text_and_a_vanished_one_is_skipped() {
        let script = "## 开头\n新加的一句。\n大家好。\r\n";
        let (out, written) = insert_retake_marks(
            script,
            &[
                mark(0, 0, "大家好。", "重来 · 20:41"),
                mark(0, 5, "删掉了的一句。", "重来 · 20:42"),
            ],
        );
        assert_eq!(written, 1);
        assert_eq!(
            out,
            "## 开头\n新加的一句。\n大家好。\r\n\r\n> [!retake] 重来 · 20:41\r\n"
        );
    }

    #[test]
    fn embedded_quotes_are_not_read_timed_or_prompted() {
        let plain = "## 开头\n大家好。\n";
        let embedded = "## 开头\n大家好。\n\n![[第一期-研究#^q-abc123]]\n";
        assert_eq!(prompter_blocks(embedded), prompter_blocks(plain));
        let rate = SpeakingRate::DEFAULT;
        assert_eq!(
            script_outline(embedded, rate).sections[0].cjk_chars,
            script_outline(plain, rate).sections[0].cjk_chars
        );
        // 行里还有别的字：照念。
        assert_eq!(
            prompter_blocks("看这句 ![[研究#^q-1]]\n")[0].lines[0].text,
            "看这句 ![[研究#^q-1]]"
        );
    }

    #[test]
    fn broll_shots_carry_their_section_text_and_done_flag() {
        let script = "---\nluna:\n  type: script\n---\n开场一句。\n\n```broll\n街景\n```\n\n## 第一段\n\n念的话。\n\n```broll done\n键盘特写\n屏幕录制\n```\n\n```python\nprint(1)\n```\n";
        let shots = broll_shots(script);
        assert_eq!(shots.len(), 2, "python 代码块不是镜头");
        assert_eq!(shots[0].section, None);
        assert_eq!(shots[0].line, 7);
        assert_eq!(shots[0].text, "街景");
        assert!(!shots[0].done);
        assert_eq!(shots[1].section.as_deref(), Some("第一段"));
        assert_eq!(shots[1].text, "键盘特写\n屏幕录制");
        assert!(shots[1].done);
    }

    #[test]
    fn toggling_a_shot_rewrites_only_its_fence_line() {
        let script = "## 段\n\n```broll wide\n街景\n```\n";
        let done = set_broll_done(script, 3, true).unwrap();
        assert_eq!(done, "## 段\n\n```broll done wide\n街景\n```\n");
        assert!(set_broll_done(&done, 3, true).is_none(), "本来就是完成");
        assert_eq!(set_broll_done(&done, 3, false).unwrap(), script);
        assert!(
            set_broll_done(script, 1, true).is_none(),
            "那一行不是 broll 围栏"
        );
        assert!(broll_shots(&done)[0].done);
    }

    #[test]
    fn pasted_text_is_cleaned_but_its_words_are_kept() {
        let raw = "第一段\u{00A0}开头\u{200B}\r\n\r\n\r\n\r\n第二段   \n![](x.png)\n![]()\n\n\n";
        assert_eq!(normalize_pasted_script(raw), "第一段 开头\n\n第二段\n");
        assert_eq!(normalize_pasted_script("  \n\n"), "");
        assert_eq!(normalize_pasted_script("one\rtwo"), "one\ntwo\n");
        // 动作提示行后补空行，免得下一行被并进提示框。
        assert_eq!(
            normalize_pasted_script("> [!cue] 举起产品\n第二件\n"),
            "> [!cue] 举起产品\n\n第二件\n"
        );
    }

    #[test]
    fn a_script_is_named_after_its_first_line() {
        assert_eq!(
            script_title_of("\n\n## 开场白\n正文").as_deref(),
            Some("开场白")
        );
        assert_eq!(script_title_of("[口播] 大家好").as_deref(), Some("大家好"));
        assert_eq!(
            script_title_of("- Hello world").as_deref(),
            Some("Hello world")
        );
        let long = "一".repeat(40);
        assert_eq!(
            script_title_of(&long).map(|t| t.chars().count()),
            Some(SCRIPT_TITLE_MAX_CHARS)
        );
        assert_eq!(script_title_of("#\n  \n"), None);
    }

    #[test]
    fn the_script_language_follows_what_is_read_aloud() {
        assert_eq!(
            script_language("## 开头\n大家好，今天聊 AI 和 local-first。\n"),
            Some("zh")
        );
        assert_eq!(
            script_language("Hello everyone, today we talk about 本地优先.\n"),
            Some("en")
        );
        assert_eq!(script_language("```broll\n空镜头\n```\n"), None);
        assert_eq!(script_language(""), None);
    }

    #[test]
    fn cue_marks_split_into_spoken_text_and_display_spans() {
        let (spoken, spans, pauses) =
            parse_cues("大家好 // 今天聊**三件事**%% 慢一点 %%[停顿]见 https://lunarca.app");
        assert_eq!(spoken, "大家好 今天聊三件事 见 https://lunarca.app");
        assert_eq!(pauses, 2, "`//` 与 `[停顿]` 各一处；网址里的 // 不算");
        assert_eq!(
            spans,
            [
                PrompterSpan::Text("大家好 ".into()),
                PrompterSpan::Pause,
                PrompterSpan::Text(" 今天聊".into()),
                PrompterSpan::Emphasis("三件事".into()),
                PrompterSpan::Note("慢一点".into()),
                PrompterSpan::Pause,
                PrompterSpan::Text("见 https://lunarca.app".into()),
            ]
        );
        // 没闭合的记号照原样念；[pause] 不分大小写。
        assert_eq!(parse_cues("a ** b %% c").0, "a ** b %% c");
        assert_eq!(parse_cues("Wait [PAUSE] go").2, 1);
    }

    #[test]
    fn cue_lines_take_a_line_but_are_not_read_and_pauses_count_toward_duration() {
        let script =
            "## 开头\n大家好 // 今天聊三件事。\n> [!cue] 举起产品\n%% 不念 %%这一句要念。\n";
        let blocks = prompter_blocks(script);
        let lines = &blocks[0].lines;
        assert_eq!(lines.len(), 3);
        assert!(lines[1].cue);
        assert_eq!(lines[1].text, "", "动作提示不念");
        assert_eq!(lines[1].spans, [PrompterSpan::Text("举起产品".into())]);
        assert_eq!(lines[0].pauses, 1);
        assert_eq!(lines[2].text, "这一句要念。");
        let rate = SpeakingRate {
            cjk_per_minute: 60.0,
            words_per_minute: 60.0,
            calibrated: true,
        };
        // 念的字：大家好今天聊三件事（9）+ 这一句要念（5）= 14 字 = 14 秒；一个停顿 0.6 秒，四舍五入加 1 秒。
        assert_eq!(script_outline(script, rate).total_seconds, 15);
    }

    #[test]
    fn an_edited_script_finds_the_line_you_were_on() {
        let before = prompter_blocks("## 甲\n一\n二\n三\n## 乙\n四\n");
        assert_eq!(relocate_line(&before, "三", 0, 2), Some((0, 2)));
        // 前面插了两行：「三」挪到第 4 行，仍找得到。
        let after = prompter_blocks("## 甲\n新一\n新二\n一\n二\n三\n## 乙\n四\n");
        assert_eq!(relocate_line(&after, "三", 0, 2), Some((0, 4)));
        // 那一行被改掉了：落在原段号行号。
        let edited = prompter_blocks("## 甲\n一\n二\n改过的三\n## 乙\n四\n");
        assert_eq!(relocate_line(&edited, "三", 0, 2), Some((0, 2)));
        assert_eq!(relocate_line(&[], "三", 0, 0), None);
    }
}
