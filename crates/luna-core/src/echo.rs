//! 跨轨回声转写去重（TDD-02 §3.1-I 外放对策层②，PRD-04-052/053）。
//!
//! 外放场景：系统声音经扬声器进入麦克风 → mic 轨（`speaker=self`）拾到**对方**的话，
//! 被误标「我」且与 system 轨重复转写。本模块在**转写结果层**去重：self 段与任一
//! remote 轨段（`remote`/`remote:N`）**时间重叠**（含回声传播延迟容差）且**文本相似**
//! → 判为回声，弃 self 段、保 system 轨段（对方语音的干净数字源）。
//!
//! 与流式跨窗去重能力同族（时间前沿 + 文本判据）；纯函数零 IO，权威重转写与实时
//! finalize 共用（DRY）。真双讲（同时说话但内容不同）因文本不相似而保留——宁可漏删
//! 不误删用户自己的话。真 AEC（音频层，AEC3）列 P2，依赖时钟对齐地基（TDD-02 §3.1-I）。

use crate::result::Segment;
use crate::text_norm::normalize_text;
use std::time::Duration;

/// 回声判定的最小时间重叠比（重叠时长 / self 段时长）。
const ECHO_OVERLAP_MIN: f64 = 0.5;

/// 回声判定的最小文本相似度（归一化字符 bigram Jaccard）。
/// 同一音频在两轨的转写存在信噪差异（mic 侧回声质量差），阈值取宽容值。
const ECHO_SIMILARITY_MIN: f64 = 0.55;

/// 回声传播延迟容差：扬声器→空气→麦克风 + 输出缓冲，回声只会**晚于**源信号；
/// 重叠判定给 remote 段区间尾部加此容差，避免短段因延迟错开被漏判。
const ECHO_MAX_DELAY: Duration = Duration::from_millis(500);

/// 移除 mic 轨（`speaker=self`）中被判为系统外放回声的段；返回移除数。
///
/// 判据：与任一 remote 轨段时间重叠比 ≥ [`ECHO_OVERLAP_MIN`]（remote 尾部含
/// [`ECHO_MAX_DELAY`] 容差）且文本相似度 ≥ [`ECHO_SIMILARITY_MIN`]。
/// 无 remote 段（单源/单麦克风录音）为 no-op。
pub fn drop_cross_track_echo(segments: &mut Vec<Segment>) -> usize {
    let remote: Vec<(Duration, Duration, String)> = segments
        .iter()
        .filter(|s| {
            s.speaker
                .as_deref()
                .is_some_and(|sp| sp == "remote" || sp.starts_with("remote:"))
        })
        .map(|s| (s.start, s.end + ECHO_MAX_DELAY, normalize_text(&s.text)))
        .filter(|(_, _, text)| !text.is_empty())
        .collect();
    if remote.is_empty() {
        return 0;
    }
    let before = segments.len();
    segments.retain(|s| {
        if s.speaker.as_deref() != Some("self") {
            return true;
        }
        let Some(dur) = s.end.checked_sub(s.start).filter(|d| !d.is_zero()) else {
            return true;
        };
        let text = normalize_text(&s.text);
        if text.is_empty() {
            return true;
        }
        let is_echo = remote.iter().any(|(r_start, r_end, r_text)| {
            let ov_start = s.start.max(*r_start);
            let ov_end = s.end.min(*r_end);
            if ov_end <= ov_start {
                return false;
            }
            let ratio = (ov_end - ov_start).as_secs_f64() / dur.as_secs_f64();
            ratio >= ECHO_OVERLAP_MIN && text_similarity(&text, r_text) >= ECHO_SIMILARITY_MIN
        });
        !is_echo
    });
    before - segments.len()
}

/// 归一化文本相似度：字符 bigram 集合 Jaccard；超短文本（<2 字符组不成 bigram）
/// 退化为包含关系（含则 1.0，否则 0.0）。
fn text_similarity(a: &str, b: &str) -> f64 {
    let a_grams = bigrams(a);
    let b_grams = bigrams(b);
    if a_grams.is_empty() || b_grams.is_empty() {
        return if !a.is_empty() && !b.is_empty() && (a.contains(b) || b.contains(a)) {
            1.0
        } else {
            0.0
        };
    }
    let intersection = a_grams.intersection(&b_grams).count();
    let union = a_grams.union(&b_grams).count();
    intersection as f64 / union as f64
}

fn bigrams(text: &str) -> std::collections::BTreeSet<(char, char)> {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SegmentId;

    fn seg(start_ms: u64, end_ms: u64, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_string(),
            translation: None,
            speaker: speaker.map(str::to_string),
            confidence: None,
            words: None,
        }
    }

    #[test]
    fn echo_segment_on_mic_track_is_dropped() {
        // 外放回声：self 段与 remote 段时间重叠、文本相似（转写噪声级差异）→ 弃 self。
        let mut segs = vec![
            seg(0, 2_000, "You're gonna have to get a job.", Some("remote")),
            seg(200, 2_100, "you're gonna have to get a job", Some("self")),
            seg(3_000, 4_000, "我觉得可以。", Some("self")),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 1);
        assert_eq!(segs.len(), 2);
        assert!(segs
            .iter()
            .all(|s| s.speaker.as_deref() != Some("self") || s.text.contains("我觉得")));
    }

    #[test]
    fn double_talk_with_different_text_is_kept() {
        // 真双讲：时间重叠但内容不同 → 保留（宁可漏删不误删用户的话）。
        let mut segs = vec![
            seg(
                0,
                2_000,
                "The quarterly numbers look great.",
                Some("remote"),
            ),
            seg(100, 1_900, "我先记一下这个结论。", Some("self")),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 0);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn similar_text_without_overlap_is_kept() {
        // 无时间重叠（超出回声延迟容差）：即便文本相似也不是回声（如复述对方的话）。
        let mut segs = vec![
            seg(0, 1_000, "let's ship it on friday", Some("remote")),
            seg(5_000, 6_000, "let's ship it on friday", Some("self")),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 0);
    }

    #[test]
    fn delayed_echo_within_tolerance_is_dropped() {
        // 回声滞后（声学+输出延迟 <500ms）：remote 尾部容差使重叠判定仍命中。
        let mut segs = vec![
            seg(0, 800, "thank you very much", Some("remote")),
            seg(400, 1_200, "thank you very much", Some("self")),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 1);
    }

    #[test]
    fn remote_n_labels_count_as_reference_and_cjk_matches() {
        // 细分后的 remote:N 同样作为参考轨；CJK 文本相似判定生效。
        let mut segs = vec![
            seg(0, 2_000, "这是现在的最优解，按右上角看。", Some("remote:2")),
            seg(100, 2_050, "这是现在的最优解 按右上角看", Some("self")),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 1);
    }

    #[test]
    fn no_remote_reference_is_noop() {
        // 单麦克风/无系统轨：没有参考轨，绝不误删。
        let mut segs = vec![
            seg(0, 1_000, "hello", Some("self")),
            seg(1_000, 2_000, "hello", None),
        ];
        assert_eq!(drop_cross_track_echo(&mut segs), 0);
        assert_eq!(segs.len(), 2);
    }
}
