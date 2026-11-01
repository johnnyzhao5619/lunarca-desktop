//! 转写段人工校正的**纯逻辑**（PRD-04-031 / PRD-04-056 / TDD-03 §10.4）。零 IO，headless 单测。
//!
//! 机器产出的转写永远会错：自动细分（[`crate::result::Segment::speaker`] 由
//! `Capability::Diarize` 归属）把相近音色并簇、把抢话段整段归给主导者；ASR 把一句切碎、
//! 又把两个人的话并成一条；片头广告/口水/静音段更是被凭空识别出文本。行业形态
//! （Otter / Descript / Sonix）一致给出**四件人工兜底**——重指派、拆分、合并、删除；
//! 本模块是这四件的唯一算法来源，Vault 层只负责读段/写回（单一写回点
//! `rewrite_recording_transcript`）。
//!
//! 三个不变量：
//! 1. **段序列恒按 `start` 升序、区间不倒挂**——所有算子输入输出都维持该形态；
//! 2. **改文本即弃词级时间戳**——`words` 描述的是「这段文本」的时间，文本一变即失效；
//! 3. **改段界即弃译文**——译文按旧段界对位，跨界后不再对应（与 `assign_speakers`
//!    骑跨切开同一政策，DRY）。

use crate::ids::SegmentId;
use crate::result::{Segment, WordTiming};
use std::time::Duration;

/// 重指派作用范围（对标 Otter「this segment / all following / all by this speaker」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignScope {
    /// 仅这一条。
    Single,
    /// 这一条 + 其后**连续**同原说话人的段（到下一次说话人变化为止）。
    Following,
    /// 全篇所有原说话人相同的段（= 把 A 并进 B，说话人合并）。
    All,
}

/// 一位说话人在本录音中的统计（回放说话人条 / 指派菜单的数据源）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeakerStat {
    /// speaker id（`self` / `remote` / `remote:N` / `speaker:N`）。
    pub id: String,
    /// 段数。
    pub segments: u32,
    /// 累计时长。
    pub duration: Duration,
}

/// 手动校正失败原因（调用方转为用户可读提示；不含正文，可安全入日志）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentEditError {
    /// 给定 `(start_ms, end_ms)` 未匹配到任何段（前端拿的是陈旧快照）。
    SegmentNotFound,
    /// 拆分点落在段首/段尾，切出空文本片。
    EmptySplitPiece,
    /// 已是首段，没有「上一条」可合并。
    NoPreviousSegment,
}

impl std::fmt::Display for SegmentEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::SegmentNotFound => "SEGMENT_NOT_FOUND",
            Self::EmptySplitPiece => "EMPTY_SPLIT_PIECE",
            Self::NoPreviousSegment => "NO_PREVIOUS_SEGMENT",
        };
        f.write_str(code)
    }
}

/// 拆分后两片各自的最短时长（ms）：低于此的片没有可听内容，进度条上也点不中。
const MIN_PIECE_MS: u64 = 40;

/// 按 `(start_ms, end_ms)` 定位段下标（与 `update_playback_transcript` 同一匹配键：
/// 边车时间即会话绝对毫秒，前端只回传区间不回传 id）。
fn index_of(segments: &[Segment], start_ms: u64, end_ms: u64) -> Option<usize> {
    segments.iter().position(|seg| {
        seg.start.as_millis() as u64 == start_ms && seg.end.as_millis() as u64 == end_ms
    })
}

/// 说话人名册：按**首次出现的时间顺序**列出（列表顺序即配色槽位顺序，稳定不跳色）。
/// 无说话人标签的段不入册（单麦克风未细分的录音 = 空名册）。
#[must_use]
pub fn speaker_roster(segments: &[Segment]) -> Vec<SpeakerStat> {
    let mut out: Vec<SpeakerStat> = Vec::new();
    for seg in segments {
        let Some(id) = seg.speaker.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let dur = seg.end.saturating_sub(seg.start);
        match out.iter_mut().find(|stat| stat.id == id) {
            Some(stat) => {
                stat.segments += 1;
                stat.duration += dur;
            }
            None => out.push(SpeakerStat {
                id: id.to_owned(),
                segments: 1,
                duration: dur,
            }),
        }
    }
    out
}

/// 下一个可用的说话人 id（「新建说话人」用）：沿用本录音已有的命名家族——
/// 出现过 `remote:N`（线上会议对方侧）则续 `remote:{max+1}`，否则 `speaker:{max+1}`
/// （现场麦克风）。前端零业务：id 由此单点派生，界面只呈现返回值。
#[must_use]
pub fn next_speaker_id(segments: &[Segment]) -> String {
    let mut max_remote = 0u32;
    let mut max_plain = 0u32;
    let mut has_remote_family = false;
    for seg in segments {
        let Some(id) = seg.speaker.as_deref() else {
            continue;
        };
        if id == "remote" {
            has_remote_family = true;
        } else if let Some(n) = id
            .strip_prefix("remote:")
            .and_then(|n| n.parse::<u32>().ok())
        {
            has_remote_family = true;
            max_remote = max_remote.max(n);
        } else if let Some(n) = id
            .strip_prefix("speaker:")
            .and_then(|n| n.parse::<u32>().ok())
        {
            max_plain = max_plain.max(n);
        }
    }
    if has_remote_family {
        format!("remote:{}", max_remote + 1)
    } else {
        format!("speaker:{}", max_plain + 1)
    }
}

/// 重指派说话人。返回实际改动的段数（0 = 目标与原值相同的幂等调用）。
///
/// # Errors
/// [`SegmentEditError::SegmentNotFound`]：`(start_ms, end_ms)` 未命中任何段。
pub fn assign_speaker(
    segments: &mut [Segment],
    start_ms: u64,
    end_ms: u64,
    speaker: &str,
    scope: AssignScope,
) -> Result<u32, SegmentEditError> {
    let index = index_of(segments, start_ms, end_ms).ok_or(SegmentEditError::SegmentNotFound)?;
    let original = segments[index].speaker.clone();
    let targets: Vec<usize> = match scope {
        AssignScope::Single => vec![index],
        // 连续同原说话人的一段（到下一次说话人变化为止）——用户校正的通常是「这一整轮发言」。
        AssignScope::Following => segments
            .iter()
            .enumerate()
            .skip(index)
            .take_while(|(_, seg)| seg.speaker == original)
            .map(|(i, _)| i)
            .collect(),
        AssignScope::All => segments
            .iter()
            .enumerate()
            .filter(|(_, seg)| seg.speaker == original)
            .map(|(i, _)| i)
            .collect(),
    };
    let mut changed = 0u32;
    for i in targets {
        if segments[i].speaker.as_deref() != Some(speaker) {
            segments[i].speaker = Some(speaker.to_owned());
            changed += 1;
        }
    }
    Ok(changed)
}

/// 文本的 UTF-16 码元长度（与前端 `textarea.selectionStart` 同口径，见 [`split_segment`]）。
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// UTF-16 偏移 → 字节偏移（越界即钳到文本长度；绝不切在字符中间）。
fn byte_index_at_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        if units >= offset {
            return byte_idx;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// 拆分点对应的时间：**有词级时间戳走词界**（精确），否则按 UTF-16 长度线性内插
/// （诚实降级——实时段/旧边车无 `words`；CJK 每字时长接近均匀，内插可用）。
fn split_time_at(seg: &Segment, offset: usize, total_units: usize) -> Duration {
    let span = seg.end.saturating_sub(seg.start);
    if let Some(words) = seg.words.as_ref().filter(|w| !w.is_empty()) {
        let mut units = 0usize;
        for word in words {
            let next = units + utf16_len(&word.text);
            // 拆分点落在本词之内/之前 → 以本词起点为界（不切碎一个词）。
            if offset <= next {
                return if offset * 2 <= units + next {
                    word.start
                } else {
                    word.end
                };
            }
            units = next;
        }
        return words.last().map_or(seg.end, |w| w.end);
    }
    if total_units == 0 {
        return seg.start + span / 2;
    }
    seg.start + Duration::from_secs_f64(span.as_secs_f64() * offset as f64 / total_units as f64)
}

/// 在文本第 `char_offset` 个 **UTF-16 码元**处把一段拆成两段（PRD-04-056）。
///
/// - 时间界：见 [`split_time_at`]（有 `words` 走词界、否则线性内插），并夹到两片各留
///   [`MIN_PIECE_MS`] 的可播区间内；
/// - 文本：左片 `trim_end`、右片 `trim_start`，任一为空即拒绝（用户把光标放在了段首/段尾）；
/// - 译文：**两片都清空**——译文按旧段界对位，拆开后不再对应，重新翻译才诚实；
/// - `words`：按时间界分给两片（无则两片皆 `None`）；
/// - `speaker_after`：右片的说话人（`None` = 沿用左片，即纯粹的断句拆分）。
///
/// `char_offset` 用 UTF-16 码元而非字符数，是为了与前端 `textarea.selectionStart` 严格同口径
/// （JS 字符串索引即 UTF-16），emoji 等代理对不会错位。
///
/// # Errors
/// [`SegmentEditError::SegmentNotFound`] / [`SegmentEditError::EmptySplitPiece`]。
pub fn split_segment(
    segments: &mut Vec<Segment>,
    start_ms: u64,
    end_ms: u64,
    char_offset: usize,
    speaker_after: Option<&str>,
) -> Result<(), SegmentEditError> {
    let index = index_of(segments, start_ms, end_ms).ok_or(SegmentEditError::SegmentNotFound)?;
    let seg = &segments[index];
    let total_units = utf16_len(&seg.text);
    let offset = char_offset.min(total_units);
    let byte_idx = byte_index_at_utf16(&seg.text, offset);
    let left_text = seg.text[..byte_idx].trim_end().to_string();
    let right_text = seg.text[byte_idx..].trim_start().to_string();
    if left_text.is_empty() || right_text.is_empty() {
        return Err(SegmentEditError::EmptySplitPiece);
    }

    let span = seg.end.saturating_sub(seg.start);
    let min_piece = Duration::from_millis(MIN_PIECE_MS).min(span / 3);
    let boundary = split_time_at(seg, offset, total_units)
        .max(seg.start + min_piece)
        .min(seg.end.saturating_sub(min_piece));

    // 词级时间戳按时间界分派（保留者仍与各自文本对位）。
    let (left_words, right_words) = match seg.words.as_ref() {
        Some(words) => {
            let (l, r): (Vec<WordTiming>, Vec<WordTiming>) =
                words.iter().cloned().partition(|w| w.start < boundary);
            ((!l.is_empty()).then_some(l), (!r.is_empty()).then_some(r))
        }
        None => (None, None),
    };

    let right = Segment {
        id: SegmentId::new(),
        start: boundary,
        end: seg.end,
        text: right_text,
        translation: None,
        speaker: speaker_after
            .map(str::to_owned)
            .or_else(|| seg.speaker.clone()),
        confidence: seg.confidence,
        words: right_words,
    };
    let left = &mut segments[index];
    left.end = boundary;
    left.text = left_text;
    left.translation = None;
    left.words = left_words;
    segments.insert(index + 1, right);
    Ok(())
}

/// 与**上一条**合并（拆分的逆操作，也用于把 ASR 切碎的半句还原成一句）。
///
/// 合并后：时间取 `[prev.start, max(prev.end, cur.end)]`、说话人取上一条（用户从上往下校对，
/// 上一条即已确认的那条）、文本按 [`join_transcript_text`] 拼接、译文同法拼接（两条都有才拼，
/// 只有一条时保留那条——译文与整段仍对位）、`words` 两条都有才顺接（缺一即整体作废，
/// 半截词表比没有更危险）。
///
/// # Errors
/// [`SegmentEditError::SegmentNotFound`] / [`SegmentEditError::NoPreviousSegment`]。
pub fn merge_with_previous(
    segments: &mut Vec<Segment>,
    start_ms: u64,
    end_ms: u64,
) -> Result<(), SegmentEditError> {
    let index = index_of(segments, start_ms, end_ms).ok_or(SegmentEditError::SegmentNotFound)?;
    if index == 0 {
        return Err(SegmentEditError::NoPreviousSegment);
    }
    let cur = segments.remove(index);
    let prev = &mut segments[index - 1];
    prev.end = prev.end.max(cur.end);
    prev.text = join_transcript_text(&prev.text, &cur.text);
    prev.translation = match (prev.translation.take(), cur.translation) {
        (Some(a), Some(b)) => Some(join_transcript_text(&a, &b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    prev.words = match (prev.words.take(), cur.words) {
        (Some(mut a), Some(b)) => {
            a.extend(b);
            Some(a)
        }
        _ => None,
    };
    Ok(())
}

/// 整条删除（PRD-04-031：片头广告 / 纯口水 / 静音段被凭空识别出的文本）。
///
/// 删除是**丢弃一条**而非把文本清空：空文本段在字幕里等于一条时长内什么都不显示的空轨，
/// 导出后仍占一个序号（SRT/VTT 都会渲染成空白条），因此文本编辑一律拒绝空文本、
/// 「不要这条」只能走本算子。剩余段的时间**不做重排**——每条字幕的时间锚定的是音轨上
/// 那一刻的声音，删掉一条不会让后面的话提前发生（与剪辑软件的「波纹删除」是两回事）。
///
/// # Errors
/// [`SegmentEditError::SegmentNotFound`]：`(start_ms, end_ms)` 未命中任何段。
pub fn delete_segment(
    segments: &mut Vec<Segment>,
    start_ms: u64,
    end_ms: u64,
) -> Result<(), SegmentEditError> {
    let index = index_of(segments, start_ms, end_ms).ok_or(SegmentEditError::SegmentNotFound)?;
    segments.remove(index);
    Ok(())
}

/// 转写文本拼接：拉丁文之间补一个空格，CJK 之间不补（中文本就无词间空格）。
#[must_use]
pub fn join_transcript_text(left: &str, right: &str) -> String {
    let left = left.trim_end();
    let right = right.trim_start();
    if left.is_empty() {
        return right.to_owned();
    }
    if right.is_empty() {
        return left.to_owned();
    }
    let need_space = !is_cjk(left.chars().next_back().unwrap_or(' '))
        && !is_cjk(right.chars().next().unwrap_or(' '));
    if need_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

/// 是否 CJK 字符（含中日韩统一表意文字、假名、全角标点）——决定拼接处补不补空格。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303f}'   // CJK 标点
        | '\u{3040}'..='\u{30ff}' // 假名
        | '\u{3400}'..='\u{4dbf}' // 扩展 A
        | '\u{4e00}'..='\u{9fff}' // 统一表意文字
        | '\u{ac00}'..='\u{d7af}' // 谚文
        | '\u{f900}'..='\u{faff}' // 兼容表意文字
        | '\u{ff00}'..='\u{ffef}' // 全角形式
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_ms: u64, end_ms: u64, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.into(),
            translation: None,
            speaker: speaker.map(str::to_owned),
            confidence: None,
            words: None,
        }
    }

    fn words(items: &[(&str, u64, u64)]) -> Option<Vec<WordTiming>> {
        Some(
            items
                .iter()
                .map(|(t, a, b)| WordTiming {
                    text: (*t).to_string(),
                    start: Duration::from_millis(*a),
                    end: Duration::from_millis(*b),
                })
                .collect(),
        )
    }

    // ── 名册 / 新 id ────────────────────────────────────────────────────────

    #[test]
    fn roster_orders_by_first_appearance_and_aggregates() {
        let segs = vec![
            seg(0, 1_000, "a", Some("remote:2")),
            seg(1_000, 3_000, "b", Some("self")),
            seg(3_000, 4_000, "c", Some("remote:2")),
            seg(4_000, 5_000, "d", None),
        ];
        let roster = speaker_roster(&segs);
        assert_eq!(roster.len(), 2, "无标签段不入册");
        assert_eq!(roster[0].id, "remote:2");
        assert_eq!(roster[0].segments, 2);
        assert_eq!(roster[0].duration, Duration::from_millis(2_000));
        assert_eq!(roster[1].id, "self");
    }

    #[test]
    fn next_speaker_id_follows_existing_family() {
        // 线上会议家族（remote / remote:N）→ 续 remote:N+1。
        let online = vec![
            seg(0, 1, "a", Some("self")),
            seg(1, 2, "b", Some("remote:3")),
        ];
        assert_eq!(next_speaker_id(&online), "remote:4");
        // 粗轨 remote 也属该家族（尚未细分的单系统源）。
        assert_eq!(
            next_speaker_id(&[seg(0, 1, "a", Some("remote"))]),
            "remote:1"
        );
        // 现场麦克风家族 → speaker:N+1；空名册 → speaker:1。
        let onsite = vec![seg(0, 1, "a", Some("speaker:2"))];
        assert_eq!(next_speaker_id(&onsite), "speaker:3");
        assert_eq!(next_speaker_id(&[seg(0, 1, "a", None)]), "speaker:1");
    }

    // ── 重指派 ──────────────────────────────────────────────────────────────

    #[test]
    fn assign_single_touches_only_that_segment() {
        let mut segs = vec![
            seg(0, 1_000, "a", Some("remote:1")),
            seg(1_000, 2_000, "b", Some("remote:1")),
        ];
        assert_eq!(
            assign_speaker(&mut segs, 0, 1_000, "remote:2", AssignScope::Single),
            Ok(1)
        );
        assert_eq!(segs[0].speaker.as_deref(), Some("remote:2"));
        assert_eq!(segs[1].speaker.as_deref(), Some("remote:1"));
    }

    #[test]
    fn assign_following_stops_at_speaker_change() {
        let mut segs = vec![
            seg(0, 1_000, "a", Some("remote:1")),
            seg(1_000, 2_000, "b", Some("remote:1")),
            seg(2_000, 3_000, "c", Some("self")),
            seg(3_000, 4_000, "d", Some("remote:1")),
        ];
        assert_eq!(
            assign_speaker(&mut segs, 0, 1_000, "remote:2", AssignScope::Following),
            Ok(2)
        );
        let ids: Vec<Option<&str>> = segs.iter().map(|s| s.speaker.as_deref()).collect();
        assert_eq!(
            ids,
            vec![
                Some("remote:2"),
                Some("remote:2"),
                Some("self"),
                Some("remote:1")
            ],
            "只到下一次说话人变化为止，后面同名段不受影响"
        );
    }

    #[test]
    fn assign_all_merges_two_speakers() {
        // 说话人合并：把全篇 remote:2 并进 remote:1（自动细分把同一人拆成两簇的典型修正）。
        let mut segs = vec![
            seg(0, 1_000, "a", Some("remote:1")),
            seg(1_000, 2_000, "b", Some("remote:2")),
            seg(2_000, 3_000, "c", Some("self")),
            seg(3_000, 4_000, "d", Some("remote:2")),
        ];
        assert_eq!(
            assign_speaker(&mut segs, 1_000, 2_000, "remote:1", AssignScope::All),
            Ok(2)
        );
        assert!(segs
            .iter()
            .all(|s| s.speaker.as_deref() != Some("remote:2")));
    }

    #[test]
    fn assign_is_idempotent_and_reports_missing_segment() {
        let mut segs = vec![seg(0, 1_000, "a", Some("self"))];
        assert_eq!(
            assign_speaker(&mut segs, 0, 1_000, "self", AssignScope::Single),
            Ok(0),
            "目标与原值相同 → 0 改动（调用方据此免写盘）"
        );
        assert_eq!(
            assign_speaker(&mut segs, 5, 9, "self", AssignScope::Single),
            Err(SegmentEditError::SegmentNotFound)
        );
    }

    #[test]
    fn assign_all_can_label_unlabelled_segments() {
        // 单麦克风未细分（speaker=None）：All = 给全部无标签段打同一标签。
        let mut segs = vec![seg(0, 1_000, "a", None), seg(1_000, 2_000, "b", None)];
        assert_eq!(
            assign_speaker(&mut segs, 0, 1_000, "speaker:1", AssignScope::All),
            Ok(2)
        );
        assert_eq!(segs[1].speaker.as_deref(), Some("speaker:1"));
    }

    // ── 拆分 ────────────────────────────────────────────────────────────────

    #[test]
    fn split_uses_word_boundary_when_words_available() {
        let mut s = seg(0, 3_000, "hello there yes indeed", Some("remote:1"));
        s.translation = Some("旧译文".into());
        s.words = words(&[
            ("hello", 0, 500),
            (" there", 500, 1_100),
            (" yes", 1_300, 2_000),
            (" indeed", 2_000, 2_900),
        ]);
        let mut segs = vec![s];
        // 光标落在 "hello there| yes indeed"（UTF-16 偏移 11）。
        split_segment(&mut segs, 0, 3_000, 11, Some("remote:2")).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "hello there");
        assert_eq!(segs[0].end, Duration::from_millis(1_100), "取词界不取内插");
        assert_eq!(segs[1].text, "yes indeed");
        assert_eq!(segs[1].start, Duration::from_millis(1_100));
        assert_eq!(segs[1].end, Duration::from_millis(3_000));
        assert_eq!(segs[0].speaker.as_deref(), Some("remote:1"));
        assert_eq!(segs[1].speaker.as_deref(), Some("remote:2"));
        assert_eq!(segs[0].translation, None, "段界已变，译文不再对位");
        assert_eq!(segs[1].translation, None);
        assert_eq!(segs[0].words.as_ref().map(Vec::len), Some(2));
        assert_eq!(segs[1].words.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn split_without_words_interpolates_by_text_length() {
        // 无词级时间戳（实时段/旧边车）：按 UTF-16 长度线性内插，CJK 每字时长近似均匀。
        let mut segs = vec![seg(0, 4_000, "你好世界再见世界", Some("speaker:1"))];
        split_segment(&mut segs, 0, 4_000, 4, None).unwrap();
        assert_eq!(segs[0].text, "你好世界");
        assert_eq!(segs[0].end, Duration::from_millis(2_000));
        assert_eq!(segs[1].text, "再见世界");
        assert_eq!(
            segs[1].speaker.as_deref(),
            Some("speaker:1"),
            "未指定说话人 → 沿用左片（纯断句拆分）"
        );
    }

    #[test]
    fn split_rejects_empty_piece_and_missing_segment() {
        let mut segs = vec![seg(0, 1_000, "只有一句", None)];
        assert_eq!(
            split_segment(&mut segs, 0, 1_000, 0, None),
            Err(SegmentEditError::EmptySplitPiece)
        );
        assert_eq!(
            split_segment(&mut segs, 0, 1_000, 99, None),
            Err(SegmentEditError::EmptySplitPiece),
            "越界偏移钳到末尾 → 右片为空 → 拒绝"
        );
        assert_eq!(
            split_segment(&mut segs, 7, 8, 2, None),
            Err(SegmentEditError::SegmentNotFound)
        );
        assert_eq!(segs.len(), 1, "失败不得改动段序列");
    }

    #[test]
    fn split_keeps_both_pieces_playable_on_short_segment() {
        // 极短段（90ms）：词界/内插都可能贴到端点，两片仍须各留可播区间。
        let mut segs = vec![seg(0, 90, "ab cd", None)];
        split_segment(&mut segs, 0, 90, 2, None).unwrap();
        assert!(segs[0].end > segs[0].start && segs[1].end > segs[1].start);
        assert_eq!(segs[0].end, segs[1].start, "两片首尾相接，时间轴无缝");
    }

    #[test]
    fn split_offset_is_utf16_so_surrogate_pairs_stay_intact() {
        // emoji 占 2 个 UTF-16 码元（与前端 selectionStart 同口径）。
        let mut segs = vec![seg(0, 2_000, "🎉 好的 收到", None)];
        split_segment(&mut segs, 0, 2_000, 5, None).unwrap();
        assert_eq!(segs[0].text, "🎉 好的");
        assert_eq!(segs[1].text, "收到");
    }

    // ── 合并 ────────────────────────────────────────────────────────────────

    #[test]
    fn merge_joins_text_time_and_keeps_previous_speaker() {
        let mut a = seg(0, 1_000, "你好", Some("remote:1"));
        a.translation = Some("Hello".into());
        let mut b = seg(1_000, 2_500, "世界", Some("remote:2"));
        b.translation = Some("world".into());
        let mut segs = vec![a, b];
        merge_with_previous(&mut segs, 1_000, 2_500).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "你好世界", "CJK 之间不补空格");
        assert_eq!(segs[0].translation.as_deref(), Some("Hello world"));
        assert_eq!(segs[0].end, Duration::from_millis(2_500));
        assert_eq!(segs[0].speaker.as_deref(), Some("remote:1"));
    }

    #[test]
    fn merge_drops_words_when_either_side_lacks_them() {
        let mut a = seg(0, 1_000, "hello", None);
        a.words = words(&[("hello", 0, 900)]);
        let segs_b = seg(1_000, 2_000, "world", None);
        let mut segs = vec![a, segs_b];
        merge_with_previous(&mut segs, 1_000, 2_000).unwrap();
        assert_eq!(segs[0].text, "hello world", "拉丁之间补空格");
        assert_eq!(segs[0].words, None, "半截词表比没有更危险");
    }

    #[test]
    fn merge_rejects_first_segment_and_missing_segment() {
        let mut segs = vec![seg(0, 1_000, "a", None), seg(1_000, 2_000, "b", None)];
        assert_eq!(
            merge_with_previous(&mut segs, 0, 1_000),
            Err(SegmentEditError::NoPreviousSegment)
        );
        assert_eq!(
            merge_with_previous(&mut segs, 9, 9),
            Err(SegmentEditError::SegmentNotFound)
        );
        assert_eq!(segs.len(), 2);
    }

    // ── 删除 ────────────────────────────────────────────────────────────────

    #[test]
    fn delete_removes_only_that_segment_and_keeps_neighbour_times() {
        // 片头广告一条：删掉它，后面两条的时间**原地不动**（时间锚的是音轨，不是序号）。
        let mut segs = vec![
            seg(0, 1_000, "本节目由某某赞助播出", None),
            seg(1_000, 2_000, "正片开始", Some("self")),
            seg(2_000, 3_000, "第二句", Some("self")),
        ];
        delete_segment(&mut segs, 0, 1_000).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "正片开始");
        assert_eq!(segs[0].start, Duration::from_millis(1_000));
        assert_eq!(segs[1].start, Duration::from_millis(2_000));
    }

    #[test]
    fn delete_reports_missing_segment_and_can_empty_the_transcript() {
        let mut segs = vec![seg(0, 1_000, "唯一一条", None)];
        assert_eq!(
            delete_segment(&mut segs, 5, 9),
            Err(SegmentEditError::SegmentNotFound)
        );
        assert_eq!(segs.len(), 1, "失败不得改动段序列");
        // 删到一条不剩是合法终态（整篇都是噪声的极端情形），调用方据此翻 transcript:false。
        delete_segment(&mut segs, 0, 1_000).unwrap();
        assert!(segs.is_empty());
    }

    #[test]
    fn split_then_merge_restores_text() {
        // 拆分/合并互为逆操作（文本层面），给用户「改错了能退回去」的确定性。
        let mut segs = vec![seg(0, 3_000, "前半句 后半句", Some("self"))];
        split_segment(&mut segs, 0, 3_000, 3, Some("remote:1")).unwrap();
        assert_eq!(segs.len(), 2);
        let (s, e) = (
            segs[1].start.as_millis() as u64,
            segs[1].end.as_millis() as u64,
        );
        merge_with_previous(&mut segs, s, e).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "前半句后半句");
        assert_eq!(segs[0].start, Duration::ZERO);
        assert_eq!(segs[0].end, Duration::from_millis(3_000));
    }
}
