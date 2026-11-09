//! 笔记里由 Lunarca 自己写入的「机器结构」（纯逻辑，无 IO）：转写块哨兵、对话角色标题与时间轴行标记。
//!
//! 它们不是任何人写下的内容：桌面的全文索引（`luna_vault::parse::clean_for_fts`）不收、搜索命中定位与替换
//! （[`crate::search`]）不碰，网页版的全文搜索（PRD-21-041）同样不碰——三处共用这一份判定（ADR-024、ADR-177 ⑥）。

/// 转写块起始哨兵（HTML 注释，渲染不可见；桌面 `rewrite_recording_transcript` 据此整块替换）。
pub const TRANSCRIPT_BEGIN: &str = "<!-- luna:transcript -->";
/// 转写块结束哨兵。
pub const TRANSCRIPT_END: &str = "<!-- /luna:transcript -->";

/// 判断从 `bytes[i..]` 起是否为一个时间轴行标记 `[HH:MM:SS]` 或 `[HH:MM:SS.mmm]`，
/// 返回标记的字节长度（含中括号）；不是则返回 `None`。
pub fn timecode_len(s: &[u8]) -> Option<usize> {
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
/// 搜索与替换的**禁区**：这些标记不进 FTS（桌面 `clean_for_fts` 已剥掉），若命中定位阶段
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
/// - 转写块哨兵 `<!-- luna:transcript -->` / `<!-- /luna:transcript -->`（[`TRANSCRIPT_BEGIN`] / [`TRANSCRIPT_END`]）；
/// - 对话笔记角色标题 `## User` / `## Assistant` / `## System`（`render_chat_markdown` 固定英文写入）。
///
/// **必须整行精确匹配**：用户自己写的 `## User research` 是内容，理应可搜——只有恰好一行
/// 只有这三个词时才是机器脚手架。
///
/// FTS 预清洗（桌面 `luna_vault::parse::clean_for_fts`）与搜索命中定位 / 替换（[`crate::search`] 的整行禁区）
/// 共用**同一判定**——两处分家就是 TDD-06 §3 警告过的「索引筛不出、逐行扫得到」口径分裂，
/// 更要命的是替换会改写 `## User` 结构行、毁掉对话笔记的往返解析。
pub fn is_scaffold_line(line: &str) -> bool {
    let t = line.trim();
    t == TRANSCRIPT_BEGIN
        || t == TRANSCRIPT_END
        || t == "## User"
        || t == "## Assistant"
        || t == "## System"
}

fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
