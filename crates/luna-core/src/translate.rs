//! 弹窗翻译的**语言判定与"翻译/润色"分支决策**（零 IO，纯逻辑）。
//!
//! 事实源：TDD-03 §3.5（检测源语言一次 → 解析有效
//! 目标语言 → 按"源 vs 有效目标"决定 `Translate`/`Refine`）、PRD-01-004/010。
//!
//! 设计纪律：
//! - **检测不进引擎执行路径**：这里只做决策，真正的 LLM 调用仍走 `JobQueue → Provider`（ADR-001）。
//! - **检测器是注入的 trait**（[`LanguageDetector`]）：核心只持纯接口，whatlang/lingua 实现在
//!   `luna-provider`（避免把外部 crate 拖进零 IO 核心，且可换实现）。
//! - **合成在 Job 构造前**：本模块产出 [`Branch`] 与最终 [`JobKind`]，引擎只见最终 prompt（TDD-03 §3.3）。

use crate::job::{JobKind, TextRef};
use crate::lang::Lang;
use crate::prompt::{Instruction, PromptRegistry, PromptScope};
use serde::{Deserialize, Serialize};

/// 语言检测结果（源语言 + 是否可靠）。
///
/// `lang == None` 或 `confident == false` 时按 PRD-01-004 兜底（默认翻译到有效目标，不误判润色）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    /// 检测到的源语言（无法判定时 `None`）。
    pub lang: Option<Lang>,
    /// 是否可靠（短文本/混合语言/低置信度时为 `false`）。
    pub confident: bool,
}

impl Detection {
    /// 一个"无法可靠判定"的检测结果（触发兜底分支）。
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            lang: None,
            confident: false,
        }
    }

    /// 一个可靠的检测结果。
    #[must_use]
    pub fn confident(lang: Lang) -> Self {
        Self {
            lang: Some(lang),
            confident: true,
        }
    }

    /// 仅当可靠且有值时返回源语言。
    #[must_use]
    pub fn reliable_lang(&self) -> Option<&Lang> {
        if self.confident {
            self.lang.as_ref()
        } else {
            None
        }
    }
}

/// 轻量本地语言检测器（纯 Rust、不调 LLM、可离线，TDD-03 §3.5）。
///
/// 注入式抽象：核心持接口，具体实现（whatlang→可换 lingua）在 `luna-provider`。
pub trait LanguageDetector: Send + Sync {
    /// 检测一段文本的源语言。
    fn detect(&self, text: &str) -> Detection;
}

/// 模板的**目标语言模式**（PRD-01-010）。
///
/// 带 serde：作为 [`crate::preset::PromptPreset`] 的字段随预设持久化（luna-config / Vault）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLangMode {
    /// 固定语言：源 == 该语言 → 润色；≠ → 翻译。适合"产出永远某语言"（写作/润色）。
    Fixed(Lang),
    /// 智能双向（默认）：源 == 母语 → 翻成默认外语；否则 → 翻成母语。
    ///
    /// 按构造几乎只触发翻译，保留"选中母语→反向翻成外语"的便利。
    SmartBidirectional,
}

/// 全局翻译语言基准（设备级，TDD-05 §7.2）：母语 + 默认外语。
///
/// 弹窗翻译为全局功能（无活动 Vault 也要可用），故母语/默认外语定为设备级有兜底值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LangContext {
    /// 用户母语（如 `zh`）。
    pub native: Lang,
    /// 默认外语（如 `en`）。
    pub default_foreign: Lang,
}

/// 分支类型：翻译（跨语言）或润色（同语言重写）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchKind {
    /// 跨语言翻译。
    Translate,
    /// 同语言润色（仅"固定语言"模式可能触发）。
    Refine,
}

/// 语言判定的最终决策（供构造 Job 与前端结果性质标识 PRD-01-029）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// 翻译 or 润色。
    pub kind: BranchKind,
    /// 有效目标语言（翻译的目标 / 润色的语言）。
    pub target: Lang,
    /// 源语言（可靠时带上，供 `JobKind::Translate.source` 与展示；不可靠为 `None`）。
    pub source: Option<Lang>,
    /// 判定是否不确定（PRD-01-004 兜底场景 → 前端标注"判定不确定"）。
    pub uncertain: bool,
}

/// 比较两个语言标签是否"同语言"：取主子标签（`-`/`_` 前）、大小写不敏感。
///
/// 故 `en` == `EN` == `en-US`；`zh` == `zh-Hans`。译/润分支据此判等。
#[must_use]
pub fn langs_equal(a: &Lang, b: &Lang) -> bool {
    fn primary(l: &Lang) -> String {
        l.as_str()
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }
    primary(a) == primary(b)
}

/// 解析模板在给定源语言下的**有效目标语言**（PRD-01-010）。
///
/// 源语言不可靠（`None`）时，智能双向无法判定方向，按"读外语→要母语"的常见意图退回**母语**
/// （PRD-01-004 兜底；固定模式不受影响，目标本就写死）。
#[must_use]
pub fn resolve_effective_target(
    mode: &TargetLangMode,
    source: Option<&Lang>,
    ctx: &LangContext,
) -> Lang {
    match mode {
        TargetLangMode::Fixed(target) => target.clone(),
        TargetLangMode::SmartBidirectional => match source {
            Some(src) if langs_equal(src, &ctx.native) => ctx.default_foreign.clone(),
            Some(_) => ctx.native.clone(),
            // 源未知：默认翻成母语（最常见：选中外语想看母语译文）。
            None => ctx.native.clone(),
        },
    }
}

/// 由检测结果 + 模板目标语言模式 + 语言基准，决出**翻译/润色分支**（TDD-03 §3.5）。
///
/// 规则：取可靠源语言 → 解析有效目标 → 源 == 有效目标且**可靠**时润色，否则翻译。
/// 源不可靠时**恒翻译**到有效目标并标 `uncertain`（绝不把外语原样吐回，PRD-01-004）。
#[must_use]
pub fn decide_branch(detection: &Detection, mode: &TargetLangMode, ctx: &LangContext) -> Branch {
    let reliable = detection.reliable_lang().cloned();
    let target = resolve_effective_target(mode, reliable.as_ref(), ctx);

    // 仅当源语言可靠、且与有效目标同语言时才润色；否则翻译。
    let is_refine = matches!(&reliable, Some(src) if langs_equal(src, &target));
    Branch {
        kind: if is_refine {
            BranchKind::Refine
        } else {
            BranchKind::Translate
        },
        target,
        source: reliable,
        uncertain: !detection.confident,
    }
}

/// 由分支 + 风格描述 + 源文本构造最终 [`JobKind`]（合成在 Job 构造前，引擎只见 prompt）。
///
/// `style` 为模板的"风格/场景描述"（弹窗模板的 `{style}`）；翻译/润色**复用同一份**，
/// 按分支落入 `Translate.instruction` 或 `Refine.instruction`（TDD-03 §3.3）。术语表注入
/// 与 PromptPreset 完整渲染为后续增量（第二块砖）。
///
/// 注：`Refine` 同语言润色，其指令显式带上目标语言，确保模型不"跨语言"。
/// 语言用**英文全称**（`display_name`）而不是 BCP-47 代码：小模型对 `in English` 远比
/// `in en` 敏感（TDD-03 §3.3，与翻译 system 模板同一策略）。
#[must_use]
pub fn compose_job_kind(
    registry: &PromptRegistry,
    branch: &Branch,
    style: Option<&Instruction>,
    text: String,
) -> JobKind {
    match branch.kind {
        BranchKind::Translate => JobKind::Translate {
            text: TextRef::Inline(text),
            source: branch.source.clone(),
            target: branch.target.clone(),
            instruction: style.cloned(),
            template: None,
        },
        BranchKind::Refine => {
            // 润色规则与风格后缀都是能力模板（`refine.popup_rule` / `refine.popup_style`，
            // ADR-050）；此前是两句 Rust 字面量。
            let style = style.map_or_else(String::new, |s| {
                registry.render(PromptScope::Refine, "popup_style", &[("style", s.as_str())])
            });
            let instruction = registry.instruction(
                PromptScope::Refine,
                "popup_rule",
                &[("lang", branch.target.display_name()), ("style", &style)],
            );
            JobKind::Refine {
                text: TextRef::Inline(text),
                instruction: Some(instruction),
            }
        }
    }
}

// ─── 译文按段界摊回 ───────────────────────────────────────────────────────────

/// 句末/子句标点：摊回时优先在这些位置切开，切出的片段才读得像句子。
const CLAUSE_ENDINGS: &[char] = &[
    '.', '!', '?', ';', ',', ':', '。', '！', '？', '；', '，', '：', '…',
];

/// 把**一条整句译文**按各源段的字符占比摊回到各段（PRD-04-034 / TDD-08 §3）。
///
/// 为什么需要它：翻译粒度是**句界单元**（一句话常被 ASR 切成 3–4 段；半句送 LLM 会
/// 得到语无伦次的译文），而回放列表与双语导出的粒度是**段**。此前的约定是"整句译文挂
/// 到单元首段、其余段留 `None`"，于是双语视图里一条长译文顶着 1 条原文、后面 3 条原文
/// 空着——用户看到的就是"译文与原文对不上"（真机反馈）。
///
/// 做法（纯逻辑、零 LLM 不确定性）：把译文切成最小单位（拉丁按词、CJK 按字），按各段
/// 源文的**可见字符占比**分配，并**优先在子句标点后收口**——ASR 的段界本就落在停顿处，
/// 与译文的子句边界天然对应，故切出来的片段基本成句。
///
/// 边界：`sources` 只有一段 → 整条译文归它（不切）；译文单位数不足以分给每一段时，
/// 靠后的段拿不到译文（返回 `None`），**绝不硬塞半个词**。
#[must_use]
pub fn distribute_translation(translation: &str, sources: &[&str]) -> Vec<Option<String>> {
    let n = sources.len();
    let trimmed = translation.trim();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(!trimmed.is_empty()).then(|| trimmed.to_string())];
    }

    let units = split_units(trimmed);
    if units.is_empty() {
        return vec![None; n];
    }

    // 各段的目标占比（按源文可见字符数；全空则均分）。
    let weights: Vec<usize> = sources
        .iter()
        .map(|s| s.chars().filter(|c| !c.is_whitespace()).count().max(1))
        .collect();
    let total_weight: usize = weights.iter().sum();
    let total_len: usize = units.iter().map(|u| u.chars().count()).sum();

    let mut out: Vec<Option<String>> = vec![None; n];
    let mut cursor = 0usize; // 已消费的单位数
    let mut consumed_len = 0usize; // 已消费的字符数
    let mut acc_weight = 0usize;

    for (i, weight) in weights.iter().enumerate() {
        acc_weight += weight;
        let remaining_slots = n - i - 1;
        if cursor >= units.len() {
            break; // 单位已分完：靠后的段诚实留空，不硬塞
        }
        // 最后一段吃掉全部剩余，避免因取整丢尾巴。
        let take_to = if remaining_slots == 0 {
            units.len()
        } else {
            // 目标：累计字符数达到 total_len × acc_weight / total_weight
            let target = total_len * acc_weight / total_weight;
            // 本段应分到的字符数——切点允许偏离它的幅度（见下方 tolerance）按此缩放。
            let share = target.saturating_sub(consumed_len).max(1);
            // 至少给一个单位，且**尽量**给后面每段各留一个；单位不够分时（译文比段数还短）
            // 前面的段照常各拿一个，靠后的段在下一轮因 `cursor >= units.len()` 诚实留空。
            let max_idx = units
                .len()
                .saturating_sub(remaining_slots)
                .max(cursor + 1)
                .min(units.len());

            // 扫一遍候选切点：**以子句标点收尾**的位置优先（切出来才读得像句子），
            // 只在偏离目标不超过半个 share 时采用；否则退回"首个达到目标长度"的位置。
            let tolerance = share.div_ceil(2);
            let mut best_mark: Option<(usize, usize, usize)> = None; // (切到, 累计长度, 偏离)
            let mut first_reach: Option<(usize, usize)> = None;
            let mut len = consumed_len;
            for (idx, unit) in units.iter().enumerate().take(max_idx).skip(cursor) {
                len += unit.chars().count();
                let at = idx + 1;
                if ends_with_clause_mark(unit) {
                    let dist = len.abs_diff(target);
                    if best_mark.is_none_or(|(_, _, best)| dist < best) {
                        best_mark = Some((at, len, dist));
                    }
                }
                if first_reach.is_none() && len >= target {
                    first_reach = Some((at, len));
                }
            }
            let (take_to, taken_len) = best_mark
                .filter(|(_, _, dist)| *dist <= tolerance)
                .map(|(at, len, _)| (at, len))
                .or(first_reach)
                .unwrap_or((max_idx, len));
            consumed_len = taken_len;
            take_to.max(cursor + 1).min(max_idx)
        };
        let piece = units[cursor..take_to].concat();
        let piece = piece.trim();
        if !piece.is_empty() {
            out[i] = Some(piece.to_string());
        }
        cursor = take_to;
        if remaining_slots == 0 {
            break;
        }
    }
    out
}

/// 切成最小可分配单位：拉丁按"词 + 其后空白"，CJK 按单字（含其后空白）。
/// 单位保留原文空白，`concat` 后即还原原文（不丢空格、不多空格）。
fn split_units(text: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        // CJK 单字即一个单位；拉丁词以空白收尾成一个单位。
        let boundary = crate::cue::is_cjk_dominant(&c.to_string())
            || c.is_whitespace()
            || chars.peek().is_none_or(|next| next.is_whitespace());
        if boundary {
            // 把紧随其后的空白并入本单位（拼回时空格位置不变）。
            while chars.peek().is_some_and(|n| n.is_whitespace()) {
                current.push(chars.next().expect("peeked"));
            }
            units.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        units.push(current);
    }
    units
}

/// 该单位是否以子句标点收尾（切口偏好）。
fn ends_with_clause_mark(unit: &str) -> bool {
    unit.trim_end()
        .chars()
        .next_back()
        .is_some_and(|c| CLAUSE_ENDINGS.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Capability;

    fn ctx() -> LangContext {
        LangContext {
            native: Lang::from("zh"),
            default_foreign: Lang::from("en"),
        }
    }

    #[test]
    fn langs_equal_normalizes_subtag_and_case() {
        assert!(langs_equal(&Lang::from("en"), &Lang::from("EN")));
        assert!(langs_equal(&Lang::from("en"), &Lang::from("en-US")));
        assert!(langs_equal(&Lang::from("zh"), &Lang::from("zh_Hans")));
        assert!(!langs_equal(&Lang::from("en"), &Lang::from("zh")));
    }

    #[test]
    fn fixed_mode_same_lang_is_refine() {
        // 固定英文模板 + 选中英文 → 润色（非"英译英"）。
        let det = Detection::confident(Lang::from("en"));
        let mode = TargetLangMode::Fixed(Lang::from("en"));
        let b = decide_branch(&det, &mode, &ctx());
        assert_eq!(b.kind, BranchKind::Refine);
        assert_eq!(b.target, Lang::from("en"));
        assert!(!b.uncertain);
    }

    #[test]
    fn fixed_mode_diff_lang_is_translate() {
        // 固定英文模板 + 选中中文 → 翻译到英文。
        let det = Detection::confident(Lang::from("zh"));
        let mode = TargetLangMode::Fixed(Lang::from("en"));
        let b = decide_branch(&det, &mode, &ctx());
        assert_eq!(b.kind, BranchKind::Translate);
        assert_eq!(b.target, Lang::from("en"));
    }

    #[test]
    fn smart_bidirectional_native_goes_to_foreign() {
        // 智能双向 + 选中母语(中) → 翻成默认外语(英)，不润色。
        let det = Detection::confident(Lang::from("zh"));
        let b = decide_branch(&det, &TargetLangMode::SmartBidirectional, &ctx());
        assert_eq!(b.kind, BranchKind::Translate);
        assert_eq!(b.target, Lang::from("en"));
    }

    #[test]
    fn smart_bidirectional_foreign_goes_to_native() {
        // 智能双向 + 选中外语(英) → 翻成母语(中)。
        let det = Detection::confident(Lang::from("en"));
        let b = decide_branch(&det, &TargetLangMode::SmartBidirectional, &ctx());
        assert_eq!(b.kind, BranchKind::Translate);
        assert_eq!(b.target, Lang::from("zh"));
    }

    #[test]
    fn unreliable_detection_forces_translate_and_marks_uncertain() {
        // 不可靠检测 + 固定英文 → 翻译到英文且标 uncertain（绝不润色）。
        let mode = TargetLangMode::Fixed(Lang::from("en"));
        let b = decide_branch(&Detection::unknown(), &mode, &ctx());
        assert_eq!(b.kind, BranchKind::Translate);
        assert_eq!(b.target, Lang::from("en"));
        assert!(b.uncertain);
    }

    #[test]
    fn unreliable_smart_bidirectional_defaults_to_native() {
        // 不可靠 + 智能双向 → 退回母语翻译，标 uncertain。
        let b = decide_branch(
            &Detection::unknown(),
            &TargetLangMode::SmartBidirectional,
            &ctx(),
        );
        assert_eq!(b.kind, BranchKind::Translate);
        assert_eq!(b.target, Lang::from("zh"));
        assert!(b.uncertain);
    }

    #[test]
    fn compose_translate_carries_style_into_instruction() {
        let det = Detection::confident(Lang::from("zh"));
        let mode = TargetLangMode::Fixed(Lang::from("en"));
        let b = decide_branch(&det, &mode, &ctx());
        let kind = compose_job_kind(
            &PromptRegistry::bundled(),
            &b,
            Some(&Instruction::raw("Slack casual tone")),
            "你好".into(),
        );
        assert_eq!(kind.capability(), Capability::Translate);
        match kind {
            JobKind::Translate {
                instruction,
                target,
                source,
                ..
            } => {
                assert_eq!(instruction.as_deref(), Some("Slack casual tone"));
                assert_eq!(target, Lang::from("en"));
                assert_eq!(source, Some(Lang::from("zh")));
            }
            _ => panic!("expected Translate"),
        }
    }

    // ── 译文按段界摊回（PRD-04-034）────────────────────────────────────────

    #[test]
    fn single_source_keeps_translation_whole() {
        assert_eq!(
            distribute_translation("Hello world.", &["你好世界。"]),
            vec![Some("Hello world.".to_string())]
        );
        assert_eq!(distribute_translation("   ", &["你好"]), vec![None]);
    }

    #[test]
    fn translation_is_spread_across_all_source_segments() {
        // 真机形态：一句中文被 ASR 切成 4 段，整句译文此前只挂首段、后 3 段空着。
        let sources = [
            "好,今天凌晨也就4月9号凌晨,",
            "据说,美国对中国的追加50%的关税生效也就104,",
            "但是这里面是不是有个时差问题,",
            "应该要按照美国的4月9号凌晨来算,也就是还没有到时间。",
        ];
        let translation = "Okay, it was 4 a.m. on April 9th; an extra 50% tariff took effect \
after 104 hours; but is there a time zone difference here? It should be counted as US local \
time, meaning it has not started yet.";
        let out = distribute_translation(translation, &sources);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(Option::is_some), "每段都应拿到译文：{out:?}");
        // 一字不丢、顺序不乱：拼回去就是原译文。
        let joined = out
            .iter()
            .map(|p| p.as_deref().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            joined.split_whitespace().collect::<Vec<_>>(),
            translation.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn longer_source_segment_gets_a_longer_share() {
        let out = distribute_translation(
            "one two three four five six seven eight",
            &["短", "这是一个长得多的源段落文本内容"],
        );
        let first = out[0].as_deref().unwrap().split_whitespace().count();
        let second = out[1].as_deref().unwrap().split_whitespace().count();
        assert!(second > first, "长段应分到更多译文（{first} vs {second}）");
    }

    #[test]
    fn cjk_translation_splits_by_character() {
        let out = distribute_translation("你好世界再见世界", &["hello world", "bye world"]);
        assert_eq!(out[0].as_deref(), Some("你好世界"));
        assert_eq!(out[1].as_deref(), Some("再见世界"));
    }

    #[test]
    fn too_few_units_leaves_trailing_segments_empty_instead_of_forcing() {
        // 译文只有 2 个词却有 4 段 → 靠后的段诚实留空，绝不硬塞半个词。
        let out = distribute_translation("Yes indeed", &["一", "二", "三", "四"]);
        assert_eq!(out.iter().filter(|p| p.is_some()).count(), 2);
        assert_eq!(out[0].as_deref(), Some("Yes"));
        assert_eq!(out[1].as_deref(), Some("indeed"));
        assert!(out[2].is_none() && out[3].is_none());
    }

    #[test]
    fn compose_refine_mentions_target_language() {
        let det = Detection::confident(Lang::from("en"));
        let mode = TargetLangMode::Fixed(Lang::from("en"));
        let b = decide_branch(&det, &mode, &ctx());
        let kind = compose_job_kind(
            &PromptRegistry::bundled(),
            &b,
            Some(&Instruction::raw("email tone")),
            "helo".into(),
        );
        assert_eq!(kind.capability(), Capability::Refine);
        match kind {
            JobKind::Refine { instruction, .. } => {
                let s = instruction.unwrap();
                assert!(
                    s.contains("in English"),
                    "润色指令必须点名语言全称——小模型对 `in en` 不敏感（TDD-03 §3.3）"
                );
                assert!(s.contains("email tone"));
            }
            _ => panic!("expected Refine"),
        }
    }
}
