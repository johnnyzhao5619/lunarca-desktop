//! 本地语言检测器（lingua-rs 实现，TDD-03 §3.5）。
//!
//! 实现 `luna-core::LanguageDetector`：纯 Rust、不调 LLM、可离线、无外发。藏在 trait 后，
//! 调用方只依赖核心接口。
//!
//! **为何 lingua 而非 whatlang**：实测 whatlang 靠字符脚本判语言，对拉丁字母诸语言（英/法/
//! 德/西…）的短文本几乎无从分辨——清晰英文曾被判为丹麦语——会漏掉「固定语言模板选同语言→
//! 润色」这个 P0 场景（PRD-01-006 US-01-6）。lingua 专为短文本高准确而生，且其置信度随脚本
//! 自适应：独占脚本短词（你好/スシ/안녕）恒 1.0，拉丁歧义短词自然偏低，无字母文本返回 None。
//!
//! **置信度即不确定信号**：`compute_language_confidence_values` 的最高值 < [`CONFIDENCE_THRESHOLD`]
//! 时置 `Detection.confident = false`，由 `decide_branch` 走 PRD-01-004 兜底（默认翻译、标 uncertain）。

use lingua::{LanguageDetector as LinguaInner, LanguageDetectorBuilder};
use luna_core::{Detection, Lang, LanguageDetector};

/// 置信阈值：最高语言置信 ≥ 此值才判"可靠"。
///
/// 依实测谱定（curated 语言子集下）：清晰句/独占脚本远高于此（英文整句 0.67–0.91、CJK 1.0、
/// 法/西/德单词 0.70–0.87），而歧义短词（"OK" 0.41、"Hallo" 0.35、"hi" 0.29）低于此 → 兜底翻译。
const CONFIDENCE_THRESHOLD: f64 = 0.60;

/// lingua 语言检测器（默认实现）。持有内部检测器（模型按需懒加载并缓存于其中）。
pub struct LinguaDetector {
    inner: LinguaInner,
}

impl LinguaDetector {
    /// 用 workspace 编译进的语言子集（`Cargo.toml` 的 lingua features）构造。
    ///
    /// `from_all_languages()` = 以"编译进二进制的全部语言"建检测器，恰为我们 per-feature
    /// 精选的子集（DRY：语言集只在 `Cargo.toml` 维护一处）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: LanguageDetectorBuilder::from_all_languages().build(),
        }
    }
}

impl Default for LinguaDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageDetector for LinguaDetector {
    fn detect(&self, text: &str) -> Detection {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Detection::unknown();
        }
        // 取最高置信项：其语言即检测结果，其置信值定可靠性（与 detect_language_of 一致，但同时拿到分值）。
        match self
            .inner
            .compute_language_confidence_values(trimmed)
            .into_iter()
            .next()
        {
            // 置信 0.0 = 无可识别字母（纯数字/符号）：lingua 仍返回占位项，按"无法判定"处理。
            Some((_, confidence)) if confidence <= 0.0 => Detection::unknown(),
            Some((lang, confidence)) => Detection {
                // lingua 直接给 ISO 639-1（大写如 "EN"）→ 归一到配置用的小写 BCP-47 主子标签。
                lang: Some(Lang::from(
                    lang.iso_code_639_1().to_string().to_lowercase().as_str(),
                )),
                confident: confidence >= CONFIDENCE_THRESHOLD,
            },
            // 完全无候选 → 无法判定。
            None => Detection::unknown(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english_prose_reliably() {
        // lingua 对真实英文散文可靠（whatlang 在此失效，正是换它的原因）。
        // 注：刻意不用 "The quick brown fox…" pangram——它用尽全字母、天然歧义，非真实输入。
        let d = LinguaDetector::new();
        let det = d.detect("Please review the attached document and reply by Friday.");
        assert_eq!(det.lang, Some(Lang::from("en")));
        assert!(det.confident);
    }

    #[test]
    fn detects_short_english_sentence_not_as_danish() {
        // whatlang 曾把这句判成丹麦语；lingua 正确判英文且可靠 → 保住 P0 的固定英文润色场景。
        let d = LinguaDetector::new();
        let det = d.detect("Hello, how are you doing today my friend?");
        assert_eq!(det.lang, Some(Lang::from("en")));
        assert!(det.confident);
    }

    #[test]
    fn detects_chinese_as_zh_even_when_short() {
        // 独占脚本：短至两字也可靠（置信随脚本自适应，无需字符数硬门）。
        let d = LinguaDetector::new();
        let det = d.detect("你好");
        assert_eq!(det.lang, Some(Lang::from("zh")));
        assert!(det.confident);
    }

    #[test]
    fn ambiguous_short_latin_is_not_confident() {
        // 歧义短词置信低于阈值 → 标不可靠 → decide_branch 走兜底翻译（不误判润色）。
        let d = LinguaDetector::new();
        let det = d.detect("OK");
        assert!(!det.confident);
    }

    /// **一个单词的「判定不确定」是正确结果，不是缺陷**（真机 2026-08-31 提出的疑问）。
    ///
    /// `Production` 在英语和法语里是**同一个词**（法语 `production` 拼写一致），检测器
    /// 手上只有这一个词，任何一侧都够不到 [`CONFIDENCE_THRESHOLD`]。此时 PRD-01-004 的
    /// 兜底是**恒翻译到有效目标语言并标不确定**——这正是那句「判定不确定」的来源。
    ///
    /// 反过来才是缺陷：硬判一个语言，等概率地把「翻译」降级成「润色」，用户拿回的是
    /// 原样的外语。宁可标不确定也不许猜，与 `ambiguous_short_latin_is_not_confident`
    /// 同一条纪律，这条只是把那条纪律钉在一个真实用户输入上。
    #[test]
    fn a_single_word_that_exists_in_several_languages_is_not_confident() {
        let d = LinguaDetector::new();
        assert!(
            !d.detect("Production").confident,
            "英法同形的单词判成「可靠」，就会有一半的时候把翻译降级成润色"
        );
    }

    #[test]
    fn empty_or_no_letters_is_unknown() {
        let d = LinguaDetector::new();
        assert_eq!(d.detect("   "), Detection::unknown());
        assert_eq!(d.detect("123 456"), Detection::unknown());
    }
}
