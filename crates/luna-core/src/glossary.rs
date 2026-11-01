//! 个人词典 / 术语表（PRD-01-017 / PRD-02 F-GAP-01 / PRD-05-003）。
//!
//! **一份词典，四条注入路径**（DRY，TDD-03 §3.3）：
//! 1. **ASR 提示**：合成 `initial_prompt` 让 whisper 倾向识别出正确的专有名词；
//! 2. **ASR 后兜底**：`apply_glossary_to_segments` 长词优先 canonical 化（模型没听对也能纠回来）；
//! 3. **翻译约束**：把**本句命中**的术语作为强制译法追加进 `Translate` Job 的 instruction；
//! 4. **润色约束**：同样注入 `Refine`，防 LLM 把专有名词改回错误形式。
//!
//! 事实源 = Vault `.luna/glossary.json`（跟库走、可迁移）；本模块只做纯文本合成与替换，
//! 不读写文件、不依赖控制器——故落在 `luna-core`（域类型层）而非某个功能模块。
//!
//! **长度纪律**：两处注入都有硬上限——whisper 的 `initial_prompt` 约 224 token
//! （超出被静默截断甚至劣化解码），本地 LLM 的每句 instruction 越长越慢。故 ASR 侧按
//! 字符预算截断（[`ASR_PROMPT_BUDGET_CHARS`]），翻译侧**只注入本句命中的术语**而非全表。

use crate::prompt::{Instruction, PromptRegistry, PromptScope};
use crate::result::Segment;
use serde::{Deserialize, Serialize};

/// 一条术语映射：识别/翻译中出现 `term` 时应规范为 `canonical`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    /// 原始词（ASR 易错形式 / 源语言术语）。
    pub term: String,
    /// 规范形（正确写法 / 强制译法）。
    pub canonical: String,
}

/// ASR `initial_prompt` 的字符预算。
///
/// whisper 的 prompt token 上限约 `n_text_ctx/2 = 224`；按最保守的「1 token ≈ 1 CJK 字符」
/// 折算并给固定前缀留余量。超出即**丢弃尾部条目**（保留前面的，不截断成半个词条）——
/// 词典再长也不会把 prompt 撑爆导致 whisper 静默截断或解码劣化。
pub const ASR_PROMPT_BUDGET_CHARS: usize = 180;

/// 解析词典批量导入文本（PRD-02 F-GAP-01：批量导入个人词典）。
///
/// 每行一条：`词条<TAB>规范形` / `词条,规范形` / 仅一列（词条即规范形，
/// 纯粹作为 ASR 提示词汇）。空行与 `#` 注释行跳过；行内多余分隔符归入规范形。
#[must_use]
pub fn parse_glossary_import(text: &str) -> Vec<GlossaryEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (term, canonical) = match line.split_once('\t') {
                Some((t, c)) => (t, c),
                None => match line.split_once(',') {
                    Some((t, c)) => (t, c),
                    None => (line, line),
                },
            };
            let term = term.trim();
            let canonical = canonical.trim();
            if term.is_empty() || canonical.is_empty() {
                return None;
            }
            Some(GlossaryEntry {
                term: term.to_string(),
                canonical: canonical.to_string(),
            })
        })
        .collect()
}

#[must_use]
pub fn glossary_initial_prompt(glossary: &[GlossaryEntry]) -> Option<String> {
    let prompt = glossary_prompt(glossary);
    (!prompt.is_empty()).then_some(prompt)
}

#[must_use]
pub fn apply_glossary(text: &str, glossary: &[GlossaryEntry]) -> String {
    let mut result = text.to_string();
    let mut entries: Vec<_> = glossary
        .iter()
        .filter(|entry| !entry.term.trim().is_empty() && !entry.canonical.trim().is_empty())
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.term.chars().count()));
    for entry in entries {
        result = replace_term(&result, entry.term.trim(), entry.canonical.trim());
    }
    result
}

#[must_use]
pub fn apply_glossary_to_segments(
    segments: Vec<Segment>,
    glossary: &[GlossaryEntry],
) -> Vec<Segment> {
    if glossary.is_empty() {
        return segments;
    }
    segments
        .into_iter()
        .map(|mut segment| {
            segment.text = apply_glossary(&segment.text, glossary);
            segment
        })
        .collect()
}

/// 合成 ASR `initial_prompt`（按 [`ASR_PROMPT_BUDGET_CHARS`] 截断，整条丢弃不切半条）。
fn glossary_prompt(glossary: &[GlossaryEntry]) -> String {
    const PREFIX: &str = "Respect this personal dictionary exactly: ";
    let mut pairs: Vec<String> = Vec::new();
    let mut used = 0usize;
    for entry in glossary {
        let term = entry.term.trim();
        let canonical = entry.canonical.trim();
        if term.is_empty() || canonical.is_empty() {
            continue;
        }
        let pair = format!("{term} => {canonical}");
        // +2 为 "; " 分隔符；预算用尽即停（保留前面的条目，绝不留半条）。
        let cost = pair.chars().count() + 2;
        if used + cost > ASR_PROMPT_BUDGET_CHARS {
            break;
        }
        used += cost;
        pairs.push(pair);
    }
    if pairs.is_empty() {
        String::new()
    } else {
        format!("{PREFIX}{}.", pairs.join("; "))
    }
}

fn replace_term(input: &str, term: &str, canonical: &str) -> String {
    if term.is_empty() {
        return input.to_string();
    }
    let ascii_case_insensitive = term.is_ascii();
    let haystack = if ascii_case_insensitive {
        input.to_ascii_lowercase()
    } else {
        input.to_string()
    };
    let needle = if ascii_case_insensitive {
        term.to_ascii_lowercase()
    } else {
        term.to_string()
    };
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;

    while cursor < input.len() {
        let Some(relative) = haystack[cursor..].find(&needle) else {
            out.push_str(&input[cursor..]);
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        if is_term_boundary(input, start, end) {
            out.push_str(&input[cursor..start]);
            out.push_str(canonical);
            cursor = end;
        } else {
            out.push_str(&input[cursor..end]);
            cursor = end;
        }
    }
    out
}

fn is_term_boundary(input: &str, start: usize, end: usize) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[end..].chars().next();
    !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 本句**命中**的术语约束（翻译/润色注入，PRD-01-017）。
///
/// 只取文本里真实出现的词条——全表注入会让每句 prompt 膨胀、拖慢本地 LLM 并挤占上下文窗，
/// 而没出现的术语对本句毫无约束力。命中判定与 canonical 化同口径（长词优先、ASCII 词界）。
/// 无命中返回 `None`，调用方即不追加任何约束段。
///
/// 约束句本身是能力模板提示词（`common.glossary_constraint`，ADR-050）：`term => canonical`
/// 的排版是用户内容派生、仍在这里拼；措辞归注册表。
#[must_use]
pub fn glossary_constraint_for(
    registry: &PromptRegistry,
    text: &str,
    glossary: &[GlossaryEntry],
) -> Option<String> {
    let mut entries: Vec<&GlossaryEntry> = glossary
        .iter()
        .filter(|e| !e.term.trim().is_empty() && !e.canonical.trim().is_empty())
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.term.chars().count()));
    let hits: Vec<String> = entries
        .into_iter()
        .filter(|e| contains_term(text, e.term.trim()))
        .map(|e| format!("{} => {}", e.term.trim(), e.canonical.trim()))
        .collect();
    if hits.is_empty() {
        return None;
    }
    Some(registry.render(
        PromptScope::Common,
        "glossary_constraint",
        &[("terms", &hits.join("; "))],
    ))
}

/// 把本次输入**命中**的术语约束追加进已合成的指令（`Translate` / `Refine` 的
/// `instruction`），无命中则原样返回。
///
/// **四条翻译/润色路径共用这一个函数**（PRD-01-017 / TDD-03 §3.3）：弹窗翻译、笔记
/// 悬浮菜单、实时逐句转译、批量转译。它此前只住在 `luna-jobqueue::realtime_translate`
/// 的一个私有函数里，于是弹窗那条装配路径根本接不上——PRD-01-017 自己记着这条缺口。
/// 各写一份的代价是两处措辞独立演化，而术语约束的全部价值就在措辞的一致上。
///
/// 收 / 回 `Option<Instruction>`：`None` + 无命中仍是 `None`（让 Provider 用通用译文
/// prompt）；`None` + 命中 = 只有约束句的指令；`Some` + 命中 = 追加。这是 [`Instruction`]
/// 在注册表之外唯一的写口（同 crate），生产代码没有第二条把字符串塞进指令的路。
#[must_use]
pub fn with_glossary_constraint(
    registry: &PromptRegistry,
    instruction: Option<Instruction>,
    text: &str,
    glossary: &[GlossaryEntry],
) -> Option<Instruction> {
    match glossary_constraint_for(registry, text, glossary) {
        Some(constraint) => {
            let mut out = instruction.unwrap_or_default();
            out.append_sentence(&constraint);
            Some(out)
        }
        None => instruction,
    }
}

/// 文本是否按词界包含该术语（与 [`apply_glossary`] 的替换判定同口径）。
fn contains_term(input: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    let ascii_ci = term.is_ascii();
    let haystack = if ascii_ci {
        input.to_ascii_lowercase()
    } else {
        input.to_string()
    };
    let needle = if ascii_ci {
        term.to_ascii_lowercase()
    } else {
        term.to_string()
    };
    let mut cursor = 0;
    while let Some(rel) = haystack[cursor..].find(&needle) {
        let start = cursor + rel;
        let end = start + needle.len();
        if is_term_boundary(input, start, end) {
            return true;
        }
        cursor = end.max(start + 1);
        if cursor >= input.len() {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, canonical: &str) -> GlossaryEntry {
        GlossaryEntry {
            term: term.into(),
            canonical: canonical.into(),
        }
    }

    #[test]
    fn asr_prompt_is_bounded_and_drops_whole_entries() {
        // whisper 的 initial_prompt 有硬上限，超出会被静默截断甚至劣化解码。
        // 词典再长也必须落在预算内，且绝不留半条（"foo =>" 这种残句会污染提示）。
        let long: Vec<GlossaryEntry> = (0..200)
            .map(|i| entry(&format!("term{i:03}"), &format!("Canonical{i:03}")))
            .collect();
        let prompt = glossary_initial_prompt(&long).expect("非空词典应产出提示");
        assert!(
            prompt.chars().count() <= ASR_PROMPT_BUDGET_CHARS + 64,
            "提示超出预算：{} 字符",
            prompt.chars().count()
        );
        assert!(prompt.ends_with('.'), "以完整句号收尾，不截半条");
        assert!(prompt.contains("term000 => Canonical000"), "保留靠前的条目");
        assert!(!prompt.contains("term199"), "预算用尽后丢弃尾部条目");
        assert!(glossary_initial_prompt(&[]).is_none(), "空词典不产出提示");
    }

    #[test]
    fn translate_constraint_only_carries_terms_present_in_the_sentence() {
        // 全表注入会让每句 prompt 膨胀、拖慢本地 LLM 并挤占上下文窗，
        // 而没出现在本句里的术语对这句翻译毫无约束力。
        let glossary = vec![
            entry("luna", "Lunarca"),
            entry("vault", "知识库"),
            entry("segment", "段"),
        ];
        let registry = PromptRegistry::bundled();
        let constraint = glossary_constraint_for(&registry, "open the vault in luna", &glossary)
            .expect("应有命中");
        assert!(constraint.contains("luna => Lunarca"));
        assert!(constraint.contains("vault => 知识库"));
        assert!(!constraint.contains("segment"), "未出现的术语不注入");
        assert!(
            constraint.starts_with(
                registry
                    .template(PromptScope::Common, "glossary_constraint")
                    .expect("随包有约束句")
                    .split('{')
                    .next()
                    .unwrap_or_default()
            ),
            "约束句措辞必须来自注册表（ADR-050）"
        );
        assert!(
            glossary_constraint_for(&registry, "nothing matches here", &glossary).is_none(),
            "零命中不追加约束段"
        );
    }

    #[test]
    fn canonicalization_prefers_longer_terms_and_respects_word_boundaries() {
        let glossary = vec![entry("luna", "Lunarca"), entry("luna code", "Lunarca Code")];
        assert_eq!(
            apply_glossary("run luna code now", &glossary),
            "run Lunarca Code now"
        );
        // ASCII 词界：lunar 不是 luna。
        assert_eq!(apply_glossary("lunar module", &glossary), "lunar module");
    }
}
