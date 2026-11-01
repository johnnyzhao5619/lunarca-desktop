//! ASR 幻觉过滤（TDD-03 §2.3）。
//!
//! Whisper 在静音 / 低能量段会吐出训练集里的字幕署名、水印、频道口号——这些内容
//! **从未在音频中出现**（真机实测「字幕by索兰娅」）。数值阈值（no_speech / logprob /
//! entropy，见 [`crate::whisper`] `build_params`）挡不住成句幻觉，须显式短语过滤 +
//! 重复段抑制作为流式/批量两条解码路径的**统一收口**。
//!
//! 词表是**数据驱动**的唯一事实源（[`resources/asr_hallucinations.txt`]），随构建 `include_str!`
//! 内联，避免散落硬编码；用户词典（Vault glossary）是另一维度（识别增强），不在此。

use luna_core::Segment;

/// 幻觉词表资源（构建期内联；解析见 [`HallucinationFilter::builtin`]）。
const HALLUCINATION_LIST: &str = include_str!("../resources/asr_hallucinations.txt");

/// 连续同文本段的保留上限：超过即判为重复幻觉循环（whisper loop），塌缩为 1 段。
const MAX_CONSECUTIVE_REPEATS: usize = 2;

/// 段内循环检测的最少重复周期数（≥此数才判为 whisper 递增式重复自激循环）。
const MIN_LOOP_PERIODS: usize = 3;
/// 段内循环判定的 token 匹配率阈值（周期性匹配比例 ≥此值即判定为循环）。
const LOOP_MATCH_RATIO: f32 = 0.8;

/// 塌缩段内**周期性重复循环**（whisper 长音频"递增式重复"签名，问题4）：
/// 如「A and A and A and A …」——检测最小重复周期 `p`（token 序列近似 `p` 周期），命中即
/// 塌缩为单个周期实例。空格分词（英文/拉丁）与逐字（CJK 无空格）两路，join 分隔符对应。
///
/// 返回 `Some(collapsed)` 表示命中循环并已塌缩；`None` 表示非循环、原文保留。
#[must_use]
pub(crate) fn collapse_looping_text(text: &str) -> Option<String> {
    let has_space = text.contains(' ');
    let tokens: Vec<&str> = if has_space {
        text.split_whitespace().collect()
    } else {
        // CJK 逐字 token（grapheme 近似：按 char 边界切）。
        text.split("").filter(|s| !s.is_empty()).collect()
    };
    let n = tokens.len();
    if n < MIN_LOOP_PERIODS * 2 {
        return None; // 太短，不可能构成 ≥3 周期的循环
    }
    for p in 1..=n / MIN_LOOP_PERIODS {
        // 至少 MIN_LOOP_PERIODS 个完整周期才算循环。
        if p * MIN_LOOP_PERIODS > n {
            break;
        }
        let comparable = n - p;
        let matches = (0..comparable)
            .filter(|&i| tokens[i] == tokens[i + p])
            .count();
        if matches as f32 / comparable as f32 >= LOOP_MATCH_RATIO {
            let sep = if has_space { " " } else { "" };
            return Some(tokens[..p].join(sep));
        }
    }
    None
}

/// 文本规范化（小写化、去标点、折叠空白）——匹配前对段文本与词表条目统一施加，
/// 以稳健对齐大小写/标点/多空白差异。CJK 字符 `is_alphanumeric()` 为真故保留。
#[must_use]
pub(crate) fn normalize_text(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 幻觉过滤器：整段精确表 + 标记子串表（编译期从词表解析一次）。
pub struct HallucinationFilter {
    /// 整段幻觉（规范化后精确相等即命中）。
    exact: Vec<String>,
    /// 标记子串（规范化后段文本包含即命中；用于「字幕by<人名>」无限变体）。
    markers: Vec<String>,
}

impl HallucinationFilter {
    /// 从内置词表构建（构建期资源，解析零 IO）。
    #[must_use]
    pub fn builtin() -> Self {
        let mut exact = Vec::new();
        let mut markers = Vec::new();
        for raw in HALLUCINATION_LIST.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(marker) = line.strip_prefix('~') {
                let norm = normalize_text(marker);
                if !norm.is_empty() {
                    markers.push(norm);
                }
            } else {
                let norm = normalize_text(line);
                if !norm.is_empty() {
                    exact.push(norm);
                }
            }
        }
        Self { exact, markers }
    }

    /// 单段是否为幻觉：规范化后整段命中精确表，或包含任一标记子串。
    #[must_use]
    pub fn is_hallucination(&self, text: &str) -> bool {
        let norm = normalize_text(text);
        if norm.is_empty() {
            return false; // 空段交由上游 trim 逻辑处理，不在此判定
        }
        if self.exact.iter().any(|e| e == &norm) {
            return true;
        }
        self.markers.iter().any(|m| norm.contains(m.as_str()))
    }

    /// 过滤一批解码段：剔除幻觉短语 + 塌缩连续重复段（whisper loop）。
    ///
    /// 顺序无关地保持时间轴：仅删除，不重排、不改时间戳。
    #[must_use]
    pub fn filter(&self, segments: Vec<Segment>) -> Vec<Segment> {
        let mut out: Vec<Segment> = Vec::with_capacity(segments.len());
        let mut run_norm: Option<String> = None;
        let mut run_len: usize = 0;
        for mut seg in segments {
            if self.is_hallucination(&seg.text) {
                continue;
            }
            // 段内周期性重复循环塌缩（whisper 递增式重复自激，问题4）：命中即改写为单周期实例
            // （事实源仍是转写段文本，此为去噪不改时间戳）。
            if let Some(collapsed) = collapse_looping_text(&seg.text) {
                seg.text = collapsed;
            }
            let norm = normalize_text(&seg.text);
            match &run_norm {
                Some(prev) if prev == &norm => {
                    run_len += 1;
                    if run_len > MAX_CONSECUTIVE_REPEATS {
                        continue; // 重复循环超限：丢弃多余复制
                    }
                }
                _ => {
                    run_norm = Some(norm);
                    run_len = 1;
                }
            }
            out.push(seg);
        }
        out
    }
}

impl Default for HallucinationFilter {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::SegmentId;
    use std::time::Duration;

    fn seg(text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::ZERO,
            end: Duration::from_millis(500),
            text: text.to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    #[test]
    fn builtin_list_parses_both_categories() {
        let f = HallucinationFilter::builtin();
        assert!(!f.exact.is_empty(), "整段表不应为空");
        assert!(!f.markers.is_empty(), "标记表不应为空");
    }

    #[test]
    fn marker_catches_subtitle_credit_variants() {
        let f = HallucinationFilter::builtin();
        // 用户实测的真实幻觉。
        assert!(f.is_hallucination("字幕by索兰娅"));
        // 「字幕by」标记覆盖任意人名变体。
        assert!(f.is_hallucination("字幕by 张三"));
        assert!(f.is_hallucination("Subtitles by the Amara.org community"));
    }

    #[test]
    fn exact_catches_known_credits() {
        let f = HallucinationFilter::builtin();
        assert!(f.is_hallucination("Thanks for watching!"));
        assert!(f.is_hallucination("谢谢观看"));
        assert!(f.is_hallucination("ご視聴ありがとうございました"));
    }

    #[test]
    fn signoff_variants_simplified_traditional_and_combined_are_flagged() {
        // 真机流式实测：简繁 + 观看/收看 + 下集/下次 变体，及拼成同段的组合形态（substring 覆盖）。
        let f = HallucinationFilter::builtin();
        assert!(f.is_hallucination("謝謝大家收看"));
        assert!(f.is_hallucination("谢谢大家收看"));
        assert!(f.is_hallucination("下次再見"));
        assert!(f.is_hallucination("下次再见"));
        assert!(
            f.is_hallucination("謝謝大家收看 下次再見"),
            "组合段须整体命中"
        );
        assert!(f.is_hallucination("谢谢大家观看"));
        assert!(
            f.is_hallucination("So that was all for today. Thank you for watching."),
            "英文片尾语与其他句拼成一段时仍须在进入语言锁和翻译前剔除"
        );
    }

    #[test]
    fn legit_speech_is_not_flagged() {
        let f = HallucinationFilter::builtin();
        assert!(!f.is_hallucination("这艘军舰的马克思武器号在红艺水平"));
        assert!(!f.is_hallucination("将舰号的复跑系统测试相当令人赞叹"));
        assert!(!f.is_hallucination("thanks a lot for the detailed report today"));
        // 裸「谢谢」是正常礼貌用语，绝不误伤（只匹含「观看/收看」的完整签退语）。
        assert!(!f.is_hallucination("谢谢"));
        assert!(!f.is_hallucination("谢谢大家"));
    }

    #[test]
    fn filter_removes_hallucination_segments_keeps_order() {
        let f = HallucinationFilter::builtin();
        let out = f.filter(vec![
            seg("第一段真实内容"),
            seg("字幕by索兰娅"),
            seg("第二段真实内容"),
        ]);
        let texts: Vec<&str> = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["第一段真实内容", "第二段真实内容"]);
    }

    #[test]
    fn filter_collapses_repetition_loop() {
        let f = HallucinationFilter::builtin();
        let out = f.filter(vec![seg("嗯"), seg("嗯"), seg("嗯"), seg("嗯"), seg("嗯")]);
        assert_eq!(out.len(), MAX_CONSECUTIVE_REPEATS, "连续重复段应塌缩到上限");
    }

    #[test]
    fn filter_keeps_non_consecutive_repeats() {
        let f = HallucinationFilter::builtin();
        // 正常语流里分散重复的词不应被误删。
        let out = f.filter(vec![seg("对"), seg("然后呢"), seg("对"), seg("好的")]);
        assert_eq!(out.len(), 4);
    }

    // ── 段内递增式重复循环塌缩（问题4）────────────────────────────────────────

    #[test]
    fn collapse_looping_english_repeat() {
        // 真机截图签名：同短语反复自激。
        let text = "I've been working on TTS for eight years and \
                    I've been working on TTS for eight years and \
                    I've been working on TTS for eight years and";
        let collapsed = collapse_looping_text(text).expect("应检出循环");
        assert!(
            collapsed.len() < text.len() / 2,
            "循环应塌缩到单周期（got: {collapsed}）"
        );
        assert!(collapsed.contains("working on TTS"));
    }

    #[test]
    fn collapse_looping_cjk_repeat() {
        // CJK 无空格逐字循环（如「好的好的好的好的」）。
        let collapsed = collapse_looping_text("好的好的好的好的好的").expect("应检出 CJK 循环");
        assert_eq!(collapsed, "好的");
    }

    #[test]
    fn collapse_preserves_normal_speech() {
        // 正常句子不应被误判为循环。
        assert!(collapse_looping_text("今天我们讨论三个议题然后开始动手").is_none());
        assert!(collapse_looping_text("the quick brown fox jumps over the lazy dog").is_none());
    }

    #[test]
    fn filter_collapses_growing_repetition_segment() {
        let f = HallucinationFilter::builtin();
        let out = f.filter(vec![seg(
            "eight years and eight years and eight years and eight years",
        )]);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].text.len() < 20,
            "递增重复段应塌缩（got: {}）",
            out[0].text
        );
    }
}
