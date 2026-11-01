//! Cue 类型与 Segments → Cues 重切分（TDD-10 §2）。
//!
//! 纯函数（零 IO），供导出（SRT/VTT/ASS/LRC）与同步回放视图共同消费，保证
//! "屏幕断行 = 导出断行"（DRY；TDD-01 §2.2 注，TDD-07 §3.1）。
//!
//! 事实源：[`TDD-10`] §2/§3；归属说明见 TDD-01 §2.2 注。

use crate::result::Segment;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ─── CueId ────────────────────────────────────────────────────────────────────

/// 可读字幕条目 id（重切分派生，按调用顺序从 0 递增；同一输入 segments 集合的调用
/// 结果保证稳定——纯函数。）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CueId(pub u32);

impl std::fmt::Display for CueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ─── Cue ──────────────────────────────────────────────────────────────────────

/// 面向阅读的字幕条目（`Segment` 重切分派生）。
///
/// 供导出（SRT/VTT/ASS/LRC）与同步回放视图消费——两者复用同一份，
/// 保证"屏幕上看到的断行 = 导出文件里的断行"（DRY）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// 顺序 id。
    pub id: CueId,
    /// 起点（绝对时间）。
    pub start: Duration,
    /// 止点（绝对时间）。
    pub end: Duration,
    /// 正文（已按 `max_line_len` / `max_lines` 折行，`\n` 分隔）。
    pub text: String,
    /// 译文（实时转译填入；无则 `None`）。
    pub translation: Option<String>,
    /// 说话人（组内共享——重切分**绝不跨说话人合并**，见 [`segments_to_cues`]；
    /// 导出说话人前缀 / 回放徽标消费）。
    pub speaker: Option<String>,
}

// ─── CueConstraints ───────────────────────────────────────────────────────────

/// `segments_to_cues` 重切分约束（TDD-10 §2/§3 ExportOptions 子集）。
///
/// **同时是字幕规范校验（[`subtitle_issues`]）的阈值来源**——切分与校验必须同一套数，
/// 否则会出现"校验说超标、导出却已自动修好"这种自相矛盾的提示（真机反馈根因）。
#[derive(Clone, Debug)]
pub struct CueConstraints {
    /// 每行最大字符数（CJK 按 `.chars().count()` 计，不按字节）。`0` = 无限制。
    pub max_line_len: usize,
    /// 每条最大行数（默认 2）。
    pub max_lines: u8,
    /// 每秒字符上限（CPS）；超过则拆分保证可读性。`0.0` = 无限制。
    pub max_cps: f32,
    /// 最短持续时长（ms）；过短相邻段强制合并。
    pub min_dur_ms: u32,
    /// 最长持续时长（ms）；多段合并时不超此上限。
    pub max_dur_ms: u32,
}

impl Default for CueConstraints {
    fn default() -> Self {
        Self::LATIN
    }
}

impl CueConstraints {
    /// 拉丁语系档（Netflix TTSG / BBC 口径）。
    pub const LATIN: Self = Self {
        max_line_len: 42, // SRT/VTT 工业标准推荐（约两行视角舒适宽度）
        max_lines: 2,
        max_cps: 20.0, // 字幕可读性工业标准
        min_dur_ms: 500,
        max_dur_ms: 7_000,
    };

    /// CJK 档（Netflix TTSG 中日韩口径）。
    ///
    /// **一个汉字承载的信息远多于一个字母**，故按字符数算时上限显著更低：行长 20（而非
    /// 42）、阅读速度 9 字/秒（而非 20）。用拉丁档量中文，等于默认"一行 42 个汉字、每秒
    /// 读 20 个汉字"——那是没人读得完的字幕。
    pub const CJK: Self = Self {
        max_line_len: 20,
        max_lines: 2,
        max_cps: 9.0,
        min_dur_ms: 500,
        max_dur_ms: 7_000,
    };

    /// 按文本语系选档。语系判定用**字符占比**而非语言标签——同一条字幕可能中英混排。
    #[must_use]
    pub fn for_text(text: &str) -> Self {
        if is_cjk_dominant(text) {
            Self::CJK
        } else {
            Self::LATIN
        }
    }

    /// 按整篇转写选档（整篇同档，避免逐条切换行宽导致断行忽宽忽窄）。
    #[must_use]
    pub fn for_segments(segments: &[Segment]) -> Self {
        let joined: String = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        Self::for_text(&joined)
    }

    /// 单条 Cue 的字符容量（`max_line_len × max_lines`）。`0` = 无限制。
    fn char_capacity(&self) -> usize {
        if self.max_line_len == 0 {
            usize::MAX
        } else {
            self.max_line_len.saturating_mul(self.max_lines as usize)
        }
    }
}

/// CJK（含日文假名、韩文谚文）字符占比是否过半——据此选阈值档。
#[must_use]
pub fn is_cjk_dominant(text: &str) -> bool {
    let mut total = 0usize;
    let mut cjk = 0usize;
    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        if matches!(c,
            '\u{3040}'..='\u{30ff}'   // 假名
            | '\u{3400}'..='\u{4dbf}' // 扩展 A
            | '\u{4e00}'..='\u{9fff}' // 统一表意文字
            | '\u{ac00}'..='\u{d7af}' // 谚文
            | '\u{f900}'..='\u{faff}' // 兼容表意文字
        ) {
            cjk += 1;
        }
    }
    total > 0 && cjk * 2 > total
}

// ─── 字幕规范校验 ─────────────────────────────────────────────────────────────

/// 一条字幕的规范问题（PRD-05-033）。
///
/// **只收重切分修不掉、必须人工处理的项**——这是本类型存在的全部意义。行长超限与时长
/// 过短都由 [`segments_to_cues`] 自动修（拆分 / 合并），把它们也报出来只会制造"我改不掉"
/// 的噪声（真机反馈：309 条里 14 条常亮，二次转写也消不掉，因为那根本不是转写的问题）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubtitleIssue {
    /// 阅读速度超限：这段时间内的字太多，观众读不完。
    /// **重切分不改变 CPS**（时间与文本同比例拆分），只能靠精简文本或让它显示更久。
    Cps,
    /// 单条显示过久：一条字幕挂满屏太长时间。用户可在该处拆分为两条。
    TooLong,
}

/// 校验一条字幕（PRD-05-033）：返回**重切分修不掉**的问题。
///
/// 阈值取自 [`CueConstraints`]（与导出重切分同一套数，DRY）。`duration` 为该条显示时长。
#[must_use]
pub fn subtitle_issues(text: &str, duration: Duration, c: &CueConstraints) -> Vec<SubtitleIssue> {
    let mut issues = Vec::new();
    let dur_secs = duration.as_secs_f32();
    // 计入阅读量的字符不含空白——空格不需要阅读时间，含空格会让拉丁语系被系统性高估。
    let visible = text.chars().filter(|c| !c.is_whitespace()).count();
    if c.max_cps > 0.0 && dur_secs > 0.0 && visible as f32 / dur_secs > c.max_cps {
        issues.push(SubtitleIssue::Cps);
    }
    if c.max_dur_ms > 0 && duration.as_millis() as u32 > c.max_dur_ms {
        issues.push(SubtitleIssue::TooLong);
    }
    issues
}

/// 该条的阅读速度（字符/秒）；时长为 0 时 `None`（无法计算而非 0）。
#[must_use]
pub fn subtitle_cps(text: &str, duration: Duration) -> Option<f32> {
    let secs = duration.as_secs_f32();
    if secs <= 0.0 {
        return None;
    }
    let visible = text.chars().filter(|c| !c.is_whitespace()).count();
    Some(visible as f32 / secs)
}

// ─── 辅助函数 ──────────────────────────────────────────────────────────────────

/// 字符数（CJK 按 `.chars().count()`，不按字节数）。
fn count_chars(s: &str) -> usize {
    s.chars().count()
}

/// 贪心折行（CJK/Latin 统一按 `.chars().count()`）。
///
/// **超出 `max_lines` 时把剩余内容留在末行，绝不丢弃**——宁可一行超宽，也不能让导出的
/// 字幕悄悄少半句话。容量超限本应由 [`group_to_cues`] 先拆成多条 cue（见那里的
/// `n_by_capacity`），本函数的兜底只在极端输入下生效。
///
/// > 假设：完整 CJK 避头尾（kinsoku）属 P1；MVP 仅保证字符数约束，
/// > 不含标点避行首逻辑。
fn wrap_text(text: &str, max_line_len: usize, max_lines: u8) -> String {
    if max_line_len == 0 || max_lines == 0 {
        return text.to_string();
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }

    let max_lines = max_lines as usize;
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    // 待放置片段队列：超宽单词（CJK 整段无空格）切出一行后，余下回填队首继续放置。
    let mut queue: std::collections::VecDeque<String> =
        words.iter().map(|w| (*w).to_string()).collect();

    while let Some(word) = queue.pop_front() {
        // 已用满 max_lines-1 行 → 当前就是末行：剩余内容一律留在这里（宁可超宽，不丢字）。
        let on_last_line = lines.len() + 1 >= max_lines;
        let wc = count_chars(&word);
        if current.is_empty() {
            if wc > max_line_len && !on_last_line {
                lines.push(word.chars().take(max_line_len).collect());
                queue.push_front(word.chars().skip(max_line_len).collect());
            } else {
                current = word;
            }
            continue;
        }
        if on_last_line || count_chars(&current) + 1 + wc <= max_line_len {
            current.push(' ');
            current.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut current));
            queue.push_front(word);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines.join("\n")
}

/// 把文本按词界均匀分成 `n` 份（尽量等字符数）。
/// 当词数不足（包括无空格的 CJK/单长词场景）时退化为字符级切分。
fn split_text_n(text: &str, n: usize) -> Vec<String> {
    if n <= 1 {
        return vec![text.to_string()];
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new(); n];
    }

    // 词数不足以按词界分 n 份 → 字符级切分
    if words.len() < n {
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len();
        let target = total.div_ceil(n);
        return (0..n)
            .map(|i| {
                let start = (i * target).min(total);
                let end = ((i + 1) * target).min(total);
                chars[start..end].iter().collect()
            })
            .collect();
    }

    // 目标：每份 ≈ total_chars / n（含空格估算）
    let total_chars: usize =
        words.iter().map(|w| count_chars(w)).sum::<usize>() + words.len().saturating_sub(1);
    let target = total_chars.div_ceil(n);

    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for (idx, word) in words.iter().enumerate() {
        let wc = count_chars(word);
        if !current.is_empty() {
            current.push(' ');
            current_chars += 1;
        }
        current.push_str(word);
        current_chars += wc;

        let parts_remaining = n.saturating_sub(parts.len());
        let words_remaining = words.len() - idx - 1;

        // 达到目标且还有足够剩余词 → 提交当前份
        if parts_remaining > 1 && current_chars >= target && words_remaining >= parts_remaining - 1
        {
            parts.push(std::mem::take(&mut current));
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// 把一组相邻 Segment 转为 Cue（s）——必要时按 CPS 拆分。
fn group_to_cues(group: &[Segment], c: &CueConstraints, next_id: &mut u32) -> Vec<Cue> {
    let start = group[0].start;
    let end = group.last().unwrap().end;

    // 合并文本（相邻段间加空格）
    let text = group
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // 译文：合并组内全部非空译文（与源文同序拼接）——只取首段会丢合并段的译文，
    // 双语三态在 cue 化后语义不丢（TDD-10 §3）。
    let translations: Vec<&str> = group
        .iter()
        .filter_map(|s| s.translation.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let translation: Option<String> = (!translations.is_empty()).then(|| translations.join(" "));
    // 说话人：组内共享（合并阶段保证不跨说话人）。
    let speaker = group[0].speaker.clone();

    let dur_secs = end.saturating_sub(start).as_secs_f32();
    let nchars = count_chars(&text);

    // CPS 检查
    let cps = if c.max_cps > 0.0 && dur_secs > 0.01 {
        nchars as f32 / dur_secs
    } else {
        0.0
    };

    // 拆分份数 = max(读速拆分, **容量拆分**)。
    // 容量拆分不可省：一条塞不下的 cue 若不拆，折行时多出的内容就只能被截断——那是**导出
    // 悄悄丢字**（真机把 CJK 行长收紧到 20 后触发概率陡增）。拆开后每条都装得下。
    let cap = c.char_capacity();
    let n_by_cps = if c.max_cps > 0.0 && cps > c.max_cps {
        ((cps / c.max_cps).ceil() as usize).max(2)
    } else {
        1
    };
    let n_by_cap = if cap > 0 && nchars > cap {
        nchars.div_ceil(cap).max(2)
    } else {
        1
    };
    let n = n_by_cps.max(n_by_cap);

    if n > 1 {
        // 拆分为 n 个时间均等的子 Cue
        let text_parts = split_text_n(&text, n);
        let total_ms = end.saturating_sub(start).as_millis() as u64;

        let mut result = Vec::with_capacity(n);
        for (i, part) in text_parts.iter().enumerate() {
            let part_start = start + Duration::from_millis(total_ms * i as u64 / n as u64);
            let part_end = if i + 1 == n {
                end
            } else {
                start + Duration::from_millis(total_ms * (i + 1) as u64 / n as u64)
            };
            let wrapped = wrap_text(part, c.max_line_len, c.max_lines);
            let id = CueId(*next_id);
            *next_id += 1;
            result.push(Cue {
                id,
                start: part_start,
                end: part_end,
                text: wrapped,
                translation: translation.clone(),
                speaker: speaker.clone(),
            });
        }
        result
    } else {
        let wrapped = wrap_text(&text, c.max_line_len, c.max_lines);
        let id = CueId(*next_id);
        *next_id += 1;
        vec![Cue {
            id,
            start,
            end,
            text: wrapped,
            translation,
            speaker,
        }]
    }
}

// ─── 公共 API ─────────────────────────────────────────────────────────────────

/// Segments → Cues 重切分（TDD-10 §2 纯函数）。
///
/// 算法（两阶段）：
/// 1. **贪心合并**：在 `max_dur_ms` / 字符容量约束内合并相邻段；当前组时长不足
///    `min_dur_ms` 时强制延伸。
/// 2. **CPS 拆分**：对超过 `max_cps` 的组进一步按比例拆分为子 Cue，每个子 Cue
///    折行后返回。
///
/// CJK 字符按 `.chars().count()` 计（不按字节；TDD-10 §2）。
///
/// 纯函数：零 IO，同一输入给出同一输出，可被导出（SRT/VTT/ASS/LRC）与同步回放视图
/// 复用（DRY；TDD-07 §3.1）。
pub fn segments_to_cues(segments: &[Segment], c: &CueConstraints) -> Vec<Cue> {
    if segments.is_empty() {
        return Vec::new();
    }

    let cap = c.char_capacity();

    // ── 阶段 1：贪心合并 ────────────────────────────────────────────────────
    let mut groups: Vec<&[Segment]> = Vec::new();
    let mut group_start = 0usize;
    let mut group_chars = count_chars(&segments[0].text);

    for i in 1..segments.len() {
        let first_start = segments[group_start].start;
        let current_dur_ms = segments[i - 1].end.saturating_sub(first_start).as_millis() as u32;
        let new_dur_ms = segments[i].end.saturating_sub(first_start).as_millis() as u32;
        let next_chars = count_chars(&segments[i].text);
        let merged_chars = group_chars + next_chars; // 粗估（不含段间空格）

        // **跨说话人绝不合并**（TDD-10 §2 硬约束）：说话人边界即切——否则导出前缀/
        // 回放徽标会把两人话拼进一条 cue（归属语义丢失）。
        let speaker_boundary = segments[i].speaker != segments[group_start].speaker;
        // 当前组时长不足 min_dur_ms → 必须继续合并（不超 max_dur_ms）
        let must_merge = current_dur_ms < c.min_dur_ms;
        // 合并后满足约束
        let can_merge = new_dur_ms <= c.max_dur_ms && merged_chars <= cap;

        if !speaker_boundary && (must_merge || can_merge) && new_dur_ms <= c.max_dur_ms {
            group_chars = merged_chars;
        } else {
            groups.push(&segments[group_start..i]);
            group_start = i;
            group_chars = next_chars;
        }
    }
    groups.push(&segments[group_start..]);

    // ── 阶段 2：组 → Cue（含 CPS 拆分与折行）───────────────────────────────
    let mut next_id = 0u32;
    let mut cues = Vec::new();
    for group in groups {
        cues.extend(group_to_cues(group, c, &mut next_id));
    }
    cues
}

// ─── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SegmentId;

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    // ── 基础 ──────────────────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        assert!(segments_to_cues(&[], &CueConstraints::default()).is_empty());
    }

    #[test]
    fn single_segment_becomes_single_cue() {
        let segs = vec![seg(0, 2_000, "hello world")];
        let c = CueConstraints {
            max_cps: 100.0,
            ..CueConstraints::default()
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, Duration::ZERO);
        assert_eq!(cues[0].end, Duration::from_millis(2_000));
        assert_eq!(cues[0].text, "hello world");
        assert_eq!(cues[0].id.0, 0);
    }

    // ── 说话人边界与双语语义（TDD-10 §2/§3）────────────────────────────────────

    #[test]
    fn speaker_boundary_never_merges() {
        // 两段各 200ms 本应因 min_dur 合并——但说话人不同 → 边界即切（硬约束）。
        let mut a = seg(0, 200, "你好");
        a.speaker = Some("张三".into());
        let mut b = seg(200, 400, "在吗");
        b.speaker = Some("李四".into());
        let cues = segments_to_cues(&[a, b], &CueConstraints::default());
        assert_eq!(cues.len(), 2, "跨说话人绝不合并");
        assert_eq!(cues[0].speaker.as_deref(), Some("张三"));
        assert_eq!(cues[1].speaker.as_deref(), Some("李四"));
    }

    #[test]
    fn merged_group_joins_all_translations() {
        // 同说话人两短段合并：双方译文都保留（只取首段会丢第二段译文）。
        let mut a = seg(0, 200, "你好");
        a.translation = Some("Hello".into());
        let mut b = seg(200, 400, "世界");
        b.translation = Some("world".into());
        let cues = segments_to_cues(&[a, b], &CueConstraints::default());
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].translation.as_deref(), Some("Hello world"));
    }

    // ── 合并 ──────────────────────────────────────────────────────────────────

    #[test]
    fn short_segments_are_merged() {
        // 两段各 200ms < min_dur_ms=500ms → 合并为一条
        let segs = vec![seg(0, 200, "hello"), seg(200, 400, "world")];
        let c = CueConstraints {
            min_dur_ms: 500,
            max_dur_ms: 10_000,
            max_cps: 100.0,
            max_line_len: 42,
            max_lines: 2,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 1, "两短段应合并");
        assert_eq!(cues[0].start, Duration::ZERO);
        assert_eq!(cues[0].end, Duration::from_millis(400));
    }

    #[test]
    fn three_short_segments_merge_until_min_dur() {
        // 三段各 200ms（总 600ms >= min_dur_ms=500ms）：前两段合并达 400ms 仍 <500ms，
        // 第三段加入后达 600ms ≥ 500ms → 全部合并为一条
        let segs = vec![seg(0, 200, "a"), seg(200, 400, "b"), seg(400, 600, "c")];
        let c = CueConstraints {
            min_dur_ms: 500,
            max_dur_ms: 10_000,
            max_cps: 100.0,
            max_line_len: 42,
            max_lines: 2,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 1, "三短段应全部合并");
    }

    // ── 不合并（超 max_dur_ms）────────────────────────────────────────────────

    #[test]
    fn segments_beyond_max_dur_are_not_merged() {
        // 3000 + 2001 ms = 5001ms > max_dur_ms=5000ms → 不合并
        let segs = vec![seg(0, 3_000, "first"), seg(3_000, 5_001, "second")];
        let c = CueConstraints {
            max_dur_ms: 5_000,
            min_dur_ms: 100,
            max_cps: 100.0,
            max_line_len: 100,
            max_lines: 2,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 2, "超 max_dur_ms 不应合并");
        assert_eq!(cues[0].end, Duration::from_millis(3_000));
        assert_eq!(cues[1].start, Duration::from_millis(3_000));
    }

    // ── CPS 拆分 ──────────────────────────────────────────────────────────────

    #[test]
    fn high_cps_triggers_split() {
        // 3000ms 内 150 字符 → CPS = 50 > max_cps=20 → ceil(50/20)=3 个子 cue
        let text = "a".repeat(150);
        let segs = vec![seg(0, 3_000, &text)];
        let c = CueConstraints {
            max_cps: 20.0,
            min_dur_ms: 0,
            max_dur_ms: 60_000,
            max_line_len: 200,
            max_lines: 10,
        };
        let cues = segments_to_cues(&segs, &c);
        assert!(cues.len() >= 2, "高 CPS 段应被拆分（got {}）", cues.len());
        // 时间覆盖完整
        assert_eq!(cues[0].start, Duration::ZERO);
        assert_eq!(cues.last().unwrap().end, Duration::from_millis(3_000));
        // 各 cue 文本非空
        for cue in &cues {
            assert!(
                !cue.text.is_empty(),
                "cue {:?} text should not be empty",
                cue.id
            );
        }
    }

    #[test]
    fn low_cps_does_not_split() {
        // 10s 内 40 字符 → CPS=4 < max_cps=20 → 不拆分
        let segs = vec![seg(0, 10_000, "hello world foo bar baz qux abc def ghij")];
        let c = CueConstraints {
            max_cps: 20.0,
            min_dur_ms: 0,
            max_dur_ms: 60_000,
            max_line_len: 100,
            max_lines: 2,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 1, "低 CPS 段不应被 CPS 逻辑拆分");
    }

    // ── CJK 字符计数 ──────────────────────────────────────────────────────────

    #[test]
    fn cjk_chars_counted_not_bytes() {
        // "你好世界" = 4 字符，不是 12 字节
        assert_eq!(count_chars("你好世界"), 4);
        assert_eq!("你好世界".len(), 12); // 字节数（供对比）
    }

    #[test]
    fn cjk_text_hard_breaks_at_max_line_len() {
        // "你好世界" 4 字符，max_line_len=3 → 硬切为 "你好世\n界"
        let segs = vec![seg(0, 2_000, "你好世界")];
        let c = CueConstraints {
            max_line_len: 3,
            max_lines: 2,
            max_cps: 100.0,
            min_dur_ms: 0,
            max_dur_ms: 60_000,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "你好世\n界", "CJK 超长词应硬切");
    }

    // ── 折行 ──────────────────────────────────────────────────────────────────

    #[test]
    fn over_capacity_group_is_split_instead_of_truncated() {
        // 48 字符、容量 15×2=30 → 拆成多条 cue，**每条** ≤ max_lines 行，且全文一字不丢。
        // 旧实现只按 CPS 拆分，容量超限时靠折行截断——导出会悄悄丢字。
        let text = "one two three four five six seven eight nine ten";
        let segs = vec![seg(0, 15_000, text)];
        let c = CueConstraints {
            max_line_len: 15,
            max_lines: 2,
            max_cps: 100.0,
            min_dur_ms: 0,
            max_dur_ms: 60_000,
        };
        let cues = segments_to_cues(&segs, &c);
        assert!(cues.len() >= 2, "容量超限应拆条，而不是截断");
        for cue in &cues {
            assert!(
                cue.text.lines().count() <= 2,
                "每条折行数应 ≤ max_lines（{:?}）",
                cue.text
            );
        }
        let restored: String = cues
            .iter()
            .flat_map(|c| c.text.split_whitespace())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(restored, text, "拆分 + 折行 不得丢失任何词");
    }

    #[test]
    fn wrap_text_never_drops_words_on_last_line() {
        // "hello"(5) 满行 → 末行承载剩余全部（宁可超宽，不得丢字）。
        let result = wrap_text("hello world foo", 8, 2);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world foo");
    }

    #[test]
    fn wrap_text_hard_splits_cjk_without_losing_chars() {
        // CJK 无空格：整段是一个“词”，需按字硬切；末行否则会把尾巴吐掉。
        let text = "一二三四五六七八九十拾百千万亿";
        let result = wrap_text(text, 4, 2);
        assert_eq!(result.lines().count(), 2);
        assert_eq!(result.replace('\n', ""), text, "硬切不得丢字");
    }

    // ── 语系档与字幕规范校验（PRD-05-033）────────────────────────────────────

    #[test]
    fn constraints_pick_tier_by_dominant_script() {
        assert!(is_cjk_dominant("今天天气不错"));
        assert!(!is_cjk_dominant("hello world"));
        // 中英混排按字符占比判定，不看语言标签。
        assert!(is_cjk_dominant("今天用 GPT 做了个 demo 演示"));
        assert!(!is_cjk_dominant("Deploy the 服务 now"));

        assert_eq!(CueConstraints::for_text("今天天气不错").max_line_len, 20);
        assert_eq!(CueConstraints::for_text("hello world").max_line_len, 42);
        let segs = vec![seg(0, 1_000, "今天"), seg(1_000, 2_000, "天气不错")];
        assert_eq!(CueConstraints::for_segments(&segs).max_cps, 9.0);
    }

    #[test]
    fn subtitle_issues_only_reports_what_resegmentation_cannot_fix() {
        let cjk = CueConstraints::CJK;
        // 25 个汉字 / 7 秒：容量超 20 但 ≤ 40（两行装得下）、CPS 3.6 → **无问题**。
        // 这正是真机上被误标 14 条的形态：它既不超容量也读得完，旧实现却按“单行 20”判超标。
        let text = "应该要按照美国的四月九号凌晨来算也就是还没有到时间";
        assert_eq!(text.chars().count(), 25);
        assert!(subtitle_issues(text, Duration::from_secs(7), &cjk).is_empty());

        // 同一句只给 2 秒 → CPS 12.5 > 9 → 读不完（重切分改不了，必须人工处理）。
        assert_eq!(
            subtitle_issues(text, Duration::from_secs(2), &cjk),
            vec![SubtitleIssue::Cps]
        );

        // 一条挂 9 秒 → 超 max_dur（用户可在该处拆分）。
        assert_eq!(
            subtitle_issues("短句", Duration::from_secs(9), &cjk),
            vec![SubtitleIssue::TooLong]
        );

        // 过短由重切分自动合并 → 不报（报了用户也只能手动合并，纯噪声）。
        assert!(subtitle_issues("嗯", Duration::from_millis(200), &cjk).is_empty());
    }

    #[test]
    fn subtitle_cps_excludes_whitespace_and_guards_zero_duration() {
        // 空白不占阅读时间：含空格会让拉丁语系被系统性高估。
        let cps = subtitle_cps("a b c d", Duration::from_secs(1)).unwrap();
        assert!((cps - 4.0).abs() < f32::EPSILON);
        assert!(subtitle_cps("x", Duration::ZERO).is_none());
    }

    // ── CueId 单调 ────────────────────────────────────────────────────────────

    #[test]
    fn cue_ids_are_sequential_from_zero() {
        let segs = vec![
            seg(0, 1_000, "a"),
            seg(1_000, 2_000, "b"),
            seg(2_000, 3_000, "c"),
        ];
        let c = CueConstraints {
            min_dur_ms: 0,
            max_dur_ms: 999, // 每段单独 cue（1000ms > 999ms 不合并）
            max_cps: 100.0,
            max_line_len: 100,
            max_lines: 2,
        };
        let cues = segments_to_cues(&segs, &c);
        assert_eq!(cues.len(), 3);
        for (i, cue) in cues.iter().enumerate() {
            assert_eq!(cue.id.0, i as u32, "CueId 应从 0 连续递增");
        }
    }

    // ── 译文透传 ──────────────────────────────────────────────────────────────

    #[test]
    fn translation_from_first_segment_in_group() {
        let mut s = seg(0, 300, "hello");
        s.translation = Some("你好".to_string());
        let s2 = seg(300, 600, "world");
        // 两段合并（min_dur_ms=500，合计 600ms）
        let c = CueConstraints {
            min_dur_ms: 500,
            max_dur_ms: 10_000,
            max_cps: 100.0,
            max_line_len: 42,
            max_lines: 2,
        };
        let cues = segments_to_cues(&[s, s2], &c);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].translation, Some("你好".to_string()));
    }
}
