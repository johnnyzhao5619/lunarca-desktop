//! 提示词预设与模板（`PromptPreset`，统一抽象，TDD-01 §2.5）。零 IO。
//!
//! `JobKind` 的 `template`、弹窗翻译模板（PRD-01）、对话角色
//! 预设、笔记 AI 悬浮菜单动作本质都是「命名的提示词 + 选项」——统一为一个 `PromptPreset`
//! （分三套必违规，DRY）。**不存两套提示词**：弹窗翻译的"翻译/润色双模式"由同一预设按
//! "源 vs 有效目标语言"派生 `Translate`/`Refine` Job（[`crate::translate`]），引擎只见最终 prompt。
//!
//! 存储：内置预设随包（见 [`builtin_presets`]）；用户预设属 Vault 级（Phase 5 接 luna-vault，
//! 此处先给内置默认 + 组装函数，存储接缝留给 config/Vault）。术语表 / `vars` 模板变量为后续
//! 增量（P1），此处不实现——只留字段语义说明，避免半成品类型（标注：与 TDD-01 §2.5 的
//! `vars: Vec<VarSpec>` 暂缺，待术语表落地一并补）。

use crate::ids::PresetId;
use crate::job::{JobKind, JobOptions};
use crate::prompt::{Instruction, PromptRegistry, PromptScope};
use crate::translate::{
    compose_job_kind, decide_branch, Branch, Detection, LangContext, TargetLangMode,
};
use serde::{Deserialize, Serialize};

/// 预设建议的默认 `JobKind`（与 `Capability` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    /// 翻译（弹窗翻译模板；运行时按语言判定可能派生为润色）。
    Translate,
    /// 润色。
    Refine,
    /// 对话角色。
    Chat,
    /// 总结。
    Summarize,
    /// 自定义模板。
    Custom,
}

/// 一个命名的提示词预设（弹窗模板 / 对话角色 / 悬浮菜单动作共用，TDD-01 §2.5）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptPreset {
    /// 稳定身份（即 `JobKind.template` 引用的 id）。
    pub id: PresetId,
    /// 建议的默认 JobKind。
    pub kind: PresetKind,
    /// 展示名（PRD-01-029 结果区标注模板名/场景）。
    ///
    /// 注：MVP 骨架期直存中文；Phase 6 i18n 落地后改走 i18n key（TDD-15，与 App.tsx 同纪律）。
    pub name: String,
    /// 系统提示（对话角色用；翻译/润色多为 `None`）。
    pub system_prompt: Option<String>,
    /// 指令/风格主体（弹窗模板的 `{style}`、Refine 的润色纪律等）。空 = 无额外风格。
    /// 是 [`Instruction`]：它最终落进 `JobKind::Translate.instruction`，只能来自注册表。
    pub instruction: Instruction,
    /// 目标语言模式（**仅弹窗翻译模板**用；其它类型预设为 `None`，PRD-01-010）。
    pub target_lang_mode: Option<TargetLangMode>,
    /// 默认选项（温度/流式等）。
    pub options: JobOptions,
    /// 内置 vs 用户自定义。
    pub builtin: bool,
}

impl PromptPreset {
    /// 由本预设 + 源语言检测 + 语言基准 + 源文本，派生**分支决策**与最终 [`JobKind`]。
    ///
    /// 弹窗翻译核心合成点（TDD-03 §3.3/§3.5）：按"源 vs 有效目标"决定翻译/润色，并把本预设的
    /// `instruction`（`{style}`）注入最终指令。**返回 `None`** 表示本预设不是弹窗翻译模板
    /// （无 `target_lang_mode`），调用方不应用它走弹窗路径。
    #[must_use]
    pub fn compose(
        &self,
        registry: &PromptRegistry,
        detection: &Detection,
        ctx: &LangContext,
        text: String,
    ) -> Option<(Branch, JobKind)> {
        let mode = self.target_lang_mode.as_ref()?;
        let branch = decide_branch(detection, mode, ctx);
        let style = (!self.instruction.trim().is_empty()).then_some(&self.instruction);
        let kind = compose_job_kind(registry, &branch, style, text);
        Some((branch, kind))
    }
}

/// 内置默认预设（随包发布）。MVP 骨架：通用 + Slack + 邮件（智能双向）+ 英文润色（固定英文，
/// 演示同语言润色路径）+ 对话角色预设（PRD-03-010，`kind: Chat`，见 [`builtin_chat_presets`]）。
/// 用户自定义/Vault 级预设为后续增量。
///
/// 第一个（`general`）为默认启用项（PRD-01-012 默认 1 槽）。弹窗翻译消费方按
/// `target_lang_mode.is_some()` 过滤，对话角色预设（`target_lang_mode: None`）不会混入。
#[must_use]
pub fn builtin_presets(registry: &PromptRegistry) -> Vec<PromptPreset> {
    // 风格片段与角色系统提示词一律来自 [`PromptRegistry`]（ADR-050）——它们是能力
    // 模板提示词，与 `build_chat_messages` 里的 system 模板同族，两处各写一份字面量
    // 正是本批收口掉的形态。空风格（`to_zh` 等纯目标语言模板）不查表：那不是提示词，
    // 是「没有额外风格」。
    let style = |key: &str| registry.instruction(PromptScope::Translate, key, &[]);
    let role = |key: &str| registry.render(PromptScope::Chat, key, &[]);
    let translate =
        |id: &str, name: &str, instruction: Instruction, mode: TargetLangMode| PromptPreset {
            id: PresetId::from(id),
            kind: PresetKind::Translate,
            name: name.to_string(),
            system_prompt: None,
            instruction,
            target_lang_mode: Some(mode),
            options: JobOptions {
                streaming: true,
                ..JobOptions::default()
            },
            builtin: true,
        };
    vec![
        translate(
            "general",
            "通用翻译",
            Instruction::default(),
            TargetLangMode::SmartBidirectional,
        ),
        translate(
            "slack",
            "Slack 口吻",
            style("preset_style.slack"),
            TargetLangMode::SmartBidirectional,
        ),
        translate(
            "email",
            "邮件",
            style("preset_style.email"),
            TargetLangMode::SmartBidirectional,
        ),
        translate(
            "to_zh",
            "中文",
            Instruction::default(),
            TargetLangMode::Fixed(crate::Lang::from("zh")),
        ),
        translate(
            "to_en",
            "English",
            Instruction::default(),
            TargetLangMode::Fixed(crate::Lang::from("en")),
        ),
        translate(
            "to_ja",
            "日本語",
            Instruction::default(),
            TargetLangMode::Fixed(crate::Lang::from("ja")),
        ),
        translate(
            "to_ko",
            "한국어",
            Instruction::default(),
            TargetLangMode::Fixed(crate::Lang::from("ko")),
        ),
        translate(
            "polish_en",
            "英文润色",
            style("preset_style.polish_en"),
            TargetLangMode::Fixed(crate::Lang::from("en")),
        ),
        chat_role("chat_general", "通用助手", &role("role.chat_general")),
        chat_role("chat_translator", "翻译助手", &role("role.chat_translator")),
        chat_role("chat_coder", "代码解释", &role("role.chat_coder")),
        chat_role("chat_summarizer", "精炼总结", &role("role.chat_summarizer")),
    ]
}

/// 构造一个对话角色预设（PRD-03-010：系统提示词沉淀为命名配置，与弹窗模板同一 `PromptPreset`
/// 抽象，DRY）。展示名走前端 i18n（按 id 映射 `chat:preset.<id>`），此处 `name` 为回退值。
fn chat_role(id: &str, name: &str, system_prompt: &str) -> PromptPreset {
    PromptPreset {
        id: PresetId::from(id),
        kind: PresetKind::Chat,
        name: name.to_string(),
        system_prompt: Some(system_prompt.to_string()),
        instruction: Instruction::default(),
        target_lang_mode: None,
        options: JobOptions {
            streaming: true,
            ..JobOptions::default()
        },
        builtin: true,
    }
}

/// 内置对话角色预设（`kind: Chat`，PRD-03-010）。快捷对话「角色」选择器的数据源。
#[must_use]
pub fn builtin_chat_presets(registry: &PromptRegistry) -> Vec<PromptPreset> {
    builtin_presets(registry)
        .into_iter()
        .filter(|p| p.kind == PresetKind::Chat)
        .collect()
}

/// 按 id 在内置预设中查找。
#[must_use]
pub fn builtin_preset(registry: &PromptRegistry, id: &str) -> Option<PromptPreset> {
    builtin_presets(registry)
        .into_iter()
        .find(|p| p.id.as_str() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::BranchKind;
    use crate::{Capability, Lang};

    fn reg() -> PromptRegistry {
        PromptRegistry::bundled()
    }

    fn ctx() -> LangContext {
        LangContext {
            native: Lang::from("zh"),
            default_foreign: Lang::from("en"),
        }
    }

    #[test]
    fn builtins_have_unique_ids_and_first_is_general() {
        let ps = builtin_presets(&reg());
        assert_eq!(ps[0].id.as_str(), "general");
        let mut ids: Vec<_> = ps.iter().map(|p| p.id.as_str().to_string()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ps.len(), "preset ids must be unique");
    }

    #[test]
    fn slack_preset_translate_carries_style() {
        // 智能双向 + 选中母语(中) → 翻成英文，Slack 风格注入 instruction。
        let preset = builtin_preset(&reg(), "slack").unwrap();
        let det = Detection::confident(Lang::from("zh"));
        let (branch, kind) = preset.compose(&reg(), &det, &ctx(), "你好".into()).unwrap();
        assert_eq!(branch.kind, BranchKind::Translate);
        assert_eq!(branch.target, Lang::from("en"));
        match kind {
            JobKind::Translate { instruction, .. } => {
                assert_eq!(
                    instruction.as_deref(),
                    reg()
                        .template(PromptScope::Translate, "preset_style.slack")
                        .as_deref(),
                    "风格片段必须来自注册表——写死在用例里就等于允许它被写死在实现里"
                );
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn fixed_english_preset_refines_english_selection() {
        // 固定英文模板 + 选中英文 → 润色（非"英译英"，US-01-6 / PRD-01-006）。
        let preset = builtin_preset(&reg(), "polish_en").unwrap();
        let det = Detection::confident(Lang::from("en"));
        let (branch, kind) = preset
            .compose(&reg(), &det, &ctx(), "helo wrld".into())
            .unwrap();
        assert_eq!(branch.kind, BranchKind::Refine);
        assert_eq!(kind.capability(), Capability::Refine);
    }

    #[test]
    fn general_preset_no_style_yields_plain_translate() {
        let preset = builtin_preset(&reg(), "general").unwrap();
        let det = Detection::confident(Lang::from("en"));
        let (_, kind) = preset
            .compose(&reg(), &det, &ctx(), "hello there".into())
            .unwrap();
        match kind {
            JobKind::Translate {
                instruction,
                target,
                ..
            } => {
                assert_eq!(instruction, None); // 空风格 → 无 instruction
                assert_eq!(target, Lang::from("zh")); // 选中外语 → 翻成母语
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn fixed_target_language_presets_translate_to_requested_language() {
        let preset = builtin_preset(&reg(), "to_ja").unwrap();
        let det = Detection::confident(Lang::from("en"));
        let (branch, kind) = preset
            .compose(&reg(), &det, &ctx(), "hello there".into())
            .unwrap();
        assert_eq!(branch.kind, BranchKind::Translate);
        assert_eq!(branch.target, Lang::from("ja"));
        match kind {
            JobKind::Translate { target, .. } => assert_eq!(target, Lang::from("ja")),
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn non_translate_preset_compose_returns_none() {
        let preset = PromptPreset {
            id: PresetId::from("role"),
            kind: PresetKind::Chat,
            name: "助手".into(),
            system_prompt: Some("You are helpful.".into()),
            instruction: Instruction::default(),
            target_lang_mode: None,
            options: JobOptions::default(),
            builtin: true,
        };
        let det = Detection::confident(Lang::from("en"));
        assert!(preset
            .compose(&reg(), &det, &ctx(), "hi there".into())
            .is_none());
    }

    #[test]
    fn builtin_chat_presets_have_system_prompts_and_no_translate_mode() {
        // PRD-03-010：对话角色预设 = 系统提示词；不带目标语言模式（不会混入弹窗模板列表）。
        let chats = builtin_chat_presets(&reg());
        assert!(!chats.is_empty(), "应内置对话角色预设");
        for p in &chats {
            assert_eq!(p.kind, PresetKind::Chat);
            assert!(p.target_lang_mode.is_none());
            assert!(p
                .system_prompt
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()));
        }
        assert!(chats.iter().any(|p| p.id.as_str() == "chat_general"));
    }

    #[test]
    fn preset_serde_round_trip() {
        let p = builtin_preset(&reg(), "polish_en").unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: PromptPreset = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
