//! 蒸馏模板（知识笔记的产出结构，PRD-06-051 / PRD-06-044）。零 IO，纯数据 + 纯函数。
//!
//! # 为什么模板是数据而不是几个 prompt 函数
//!
//! 初版把「输入是什么」与「输出长什么样」焊在同一句 prompt 里（`"The text is a transcript
//! of a recording. …Use these sections…"`）。一旦允许用户自选模板，对一篇 AI 对话选「会议
//! 纪要式」就会让 prompt **对着对话文本谎称"这是录音转写"**，说话人归属指令随之全部错位。
//! 故指令按三段正交拼装，任何模板对任何源都诚实成立：
//!
//! ```text
//! compose_instruction = 输入形态前言（跟着源走）
//!                     + 输出区块结构（跟着模板走）
//!                     + 纪律尾注（恒定）
//! ```
//!
//! # 模板按「内容体裁」而非「来源类型」划分
//!
//! 体裁决定知识形态：会议是"决定了什么、谁去做"，讲述是"观点与论据"，访谈是"谁持什么
//! 观点"。刻意**只做四个**——这是体裁维度上的自然类目，不是「销售通话 / 站会 / 周报」
//! 那种业务场景模板集（与 PRD-06-047 拒绝周报专有功能同一裁决：Lunarca 是通用工具而非业务
//! 模板集）。用户自定义模板（P1）走同一结构，`builtin: false` 即接缝。
//!
//! # 区块标题的语言只有两档
//!
//! 输出语言为中文系 → 中文标题；其余一律英文标题。标题**不被机器解析**（「用户编辑过」
//! 判定用的是正文指纹而非结构），纯呈现；让模型自造第三种语言的标题会牺牲确定性，
//! 而按输出语言给定标题则顺带根治了初版「英文正文配中文小节标题」的混合体缺陷。

use crate::lang::Lang;
use crate::prompt::{Instruction, PromptRegistry, PromptScope};
use serde::{Deserialize, Serialize};

/// 内置蒸馏模板身份。
///
/// 序列化形态即 frontmatter `luna.distill_template` 的取值与前端 i18n 动态键的后缀，
/// 三处同一份字符串（改一处必须改三处，故收敛为此枚举的 serde 契约）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateId {
    /// 会议纪要式：多人会议 / 讨论。
    Meeting,
    /// 讲述式：视频文稿 / 讲座 / 教程 / 单人播客 / 文章朗读。
    Talk,
    /// 访谈式：播客对谈 / 采访。**不参与自动建议**（见 [`suggest`]）。
    Interview,
    /// 对话结晶式：与 AI 的对话。
    Chat,
}

impl TemplateId {
    /// 全部内置模板（前端选单顺序即此顺序）。
    pub const ALL: [Self; 4] = [Self::Meeting, Self::Talk, Self::Interview, Self::Chat];

    /// 稳定字符串键（frontmatter 取值 / i18n 动态键后缀 / DTO 字段，单一事实源）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Meeting => "meeting",
            Self::Talk => "talk",
            Self::Interview => "interview",
            Self::Chat => "chat",
        }
    }

    /// 由字符串键还原（frontmatter 回读）。未知取值 → `None`，调用方回退建议规则。
    #[must_use]
    pub fn from_key(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.key() == raw.trim())
    }

    /// 蒸馏笔记的文件名后缀（`<源标题> · <后缀>`）。
    ///
    /// 兑现 PRD-06-044 写下却从未落地的「`· 纪要` / `· 笔记`」两种形态——初版对 AI 对话
    /// 蒸馏出来的笔记也叫「纪要」。
    #[must_use]
    pub fn name_suffix(self) -> &'static str {
        match self {
            Self::Meeting => "纪要",
            Self::Talk | Self::Interview | Self::Chat => "笔记",
        }
    }

    /// 本模板默认是否附「整理稿」。
    ///
    /// 讲述类内容的整理稿**就是那篇文章本身**（核心价值）；会议的整理稿是"去口头语的
    /// 对话实录"，长而低密度且回放页一键可达，故默认不附。用户可逐次覆盖。
    #[must_use]
    pub fn full_text_default(self) -> bool {
        matches!(self, Self::Talk)
    }

    /// 本模板的输出区块（顺序即产出顺序；空区块由模型省略）。
    #[must_use]
    pub fn sections(self) -> &'static [Section] {
        match self {
            Self::Meeting => MEETING_SECTIONS,
            Self::Talk => TALK_SECTIONS,
            Self::Interview => INTERVIEW_SECTIONS,
            Self::Chat => CHAT_SECTIONS,
        }
    }
}

const MEETING_SECTIONS: &[Section] = &[
    Section::new("概要", "Overview", "2-4 sentences of what this was about"),
    Section::new("要点", "Key points", "the substance that was discussed"),
    Section::new("决议", "Decisions", "what was actually decided"),
    Section::new("行动项", "Action items", "concrete follow-ups"),
    Section::new("待决问题", "Open questions", "what was left unresolved"),
];

const TALK_SECTIONS: &[Section] = &[
    Section::new("概要", "Overview", "2-4 sentences of what this covers"),
    Section::new("核心观点", "Key arguments", "the claims being made"),
    Section::new(
        "论据与事实",
        "Evidence and facts",
        "data, examples and references cited",
    ),
    Section::new(
        "术语与概念",
        "Terms and concepts",
        "notable terms with a one-line explanation each",
    ),
    Section::new(
        "值得跟进",
        "Worth following up",
        "what a reader may want to explore next",
    ),
];

const INTERVIEW_SECTIONS: &[Section] = &[
    Section::new("概要", "Overview", "2-4 sentences of what was discussed"),
    Section::new(
        "各方观点",
        "Positions",
        "who holds which view, attributed by name",
    ),
    Section::new(
        "关键问答",
        "Key exchanges",
        "the questions that produced the most substance",
    ),
    Section::new("金句", "Notable quotes", "verbatim, attributed"),
    Section::new("值得跟进", "Worth following up", "threads left open"),
];

const CHAT_SECTIONS: &[Section] = &[
    Section::new("问题", "Problem", "what the user was trying to solve"),
    Section::new("结论", "Conclusion", "what was concluded"),
    Section::new(
        "关键信息",
        "Key details",
        "facts, snippets and caveats worth keeping",
    ),
    Section::new("待办", "To do", "follow-ups for the user"),
];

/// 一个输出区块：中英标题 + 给模型的一句指导语。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    /// 中文标题（输出语言为中文系时使用）。
    pub heading_zh: &'static str,
    /// 英文标题（其余语言使用）。
    pub heading_en: &'static str,
    /// 指导语（恒英文——它是给模型的指令，不是给用户看的文案）。
    pub guidance: &'static str,
}

impl Section {
    const fn new(
        heading_zh: &'static str,
        heading_en: &'static str,
        guidance: &'static str,
    ) -> Self {
        Self {
            heading_zh,
            heading_en,
            guidance,
        }
    }

    /// 按输出语言取标题（两档：中文系 vs 其余）。
    #[must_use]
    pub fn heading(&self, lang: &Lang) -> &'static str {
        if is_chinese(lang) {
            self.heading_zh
        } else {
            self.heading_en
        }
    }
}

/// 输出语言是否为中文系（区块标题选档的唯一判据）。
fn is_chinese(lang: &Lang) -> bool {
    lang.as_str()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .eq_ignore_ascii_case("zh")
}

/// 建议模板的判据（随建议下发，界面上如实说明"为什么是它"）。
///
/// 判据可见是「自动适配」可被信任的前提：用户看得懂它凭什么这么判，才知道什么时候
/// 该改。稳定字符串键 → 前端 i18n 动态键 `distill.reason.*`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestReason {
    /// 这是一篇 AI 对话。
    Conversation,
    /// 这是自己录的音（默认按会议对待）。
    OwnRecording,
    /// 检测到多位说话人。
    MultipleSpeakers,
    /// 单人讲述（无说话人或只有一位）。
    SingleSpeaker,
    /// 这是用户自己写的记录（ADR-086 的记录态，[`SourceShape::WrittenNotes`]）。
    WrittenNotes,
}

impl SuggestReason {
    /// 稳定字符串键（i18n 动态键后缀）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::OwnRecording => "own_recording",
            Self::MultipleSpeakers => "multiple_speakers",
            Self::SingleSpeaker => "single_speaker",
            Self::WrittenNotes => "written_notes",
        }
    }
}

/// 源笔记的体裁线索（由装配阶段如实给出，不靠事后猜文本）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceShape {
    /// 与 AI 的对话（`## User` / `## Assistant` 结构）。
    Conversation,
    /// 自己录制的音频。
    OwnRecording {
        /// 已识别出的说话人数（未细分时为 0）。
        speakers: usize,
    },
    /// 导入 / 转写得来的媒体文稿。
    ImportedTranscript {
        /// 已识别出的说话人数（未细分时为 0）。
        speakers: usize,
    },
    /// **用户自己写的记录**（ADR-086 的记录态：会议速记 / 灵感碎片 / 草稿）。
    ///
    /// 与前三种的分界不是"谁说的"而是**已经是不是文字**：前三者的输入是转写或对话存档，
    /// 有时间码、口头语与说话人行；这一种的输入本来就是用户敲下的散文，故指令里既不该
    /// 提"转写稿"，也不该提说话人归属（[`input_preamble`]）。
    WrittenNotes,
}

/// 按源的体裁线索建议一个模板。
///
/// **访谈式不参与建议**：「两个说话人」区分不了会议与访谈，而错判的代价是整篇结构
/// 错误的笔记——错判率高的判据不如不判，把它留给手动选择。
///
/// **不用 LLM 判体裁**：那要多跑一轮分钟级本地推理，结果还不可解释；规则判据免费、
/// 可解释、判错了用户一眼看得见也一键改得掉（这与 Notion 的 Auto 档是同一用户心智，
/// 只是换了更适合本地引擎的实现手段）。
#[must_use]
pub fn suggest(shape: SourceShape) -> (TemplateId, SuggestReason) {
    match shape {
        SourceShape::Conversation => (TemplateId::Chat, SuggestReason::Conversation),
        SourceShape::OwnRecording { speakers } if speakers >= 2 => {
            (TemplateId::Meeting, SuggestReason::MultipleSpeakers)
        }
        // 自录音频默认按会议对待：用户按下录音键多半是在开会或与人交谈，且未细分时
        // 说话人数恒为 0——此时按「单人讲述」处理会把绝大多数会议判错。
        SourceShape::OwnRecording { .. } => (TemplateId::Meeting, SuggestReason::OwnRecording),
        SourceShape::ImportedTranscript { speakers } if speakers >= 2 => {
            (TemplateId::Meeting, SuggestReason::MultipleSpeakers)
        }
        SourceShape::ImportedTranscript { .. } => (TemplateId::Talk, SuggestReason::SingleSpeaker),
        // 记录（ADR-086）建议**会议纪要式**，理由有二，都不是"记录一定是会议"：
        // ① 它的五个小节里 `决议 / 行动项 / 待决问题` 恰是过程性记录最常有的东西，而
        //    指令本就要求「空的小节整节省略」，套错时代价是少几节、不是长出错误结构；
        // ② 讲述式**默认附整理稿**，而记录的整理稿就是把用户自己写的散文重抄一遍
        //    ——分钟级的模型时间换一份近乎重复的正文，是这条路上最贵的默认值。
        SourceShape::WrittenNotes => (TemplateId::Meeting, SuggestReason::WrittenNotes),
    }
}

// ── 用户自定义模板（PRD-06-051 批次 2）────────────────────────────────────────

/// 用户键前缀。内置模板的键是裸的 `meeting`/`talk`/…，用户模板一律 `user:<id>`——
/// 两个键空间必须能一眼分开：frontmatter 里存的就是这个字符串，若某天用户把模板命名成
/// `meeting`，不带前缀就会与内置的撞车，而"撞车时听谁的"没有正确答案。
const USER_KEY_PREFIX: &str = "user:";

/// 模板包带来的整理模板的键前缀：`pack:<包 id>/<模板 id>`（ADR-169 ⑤）。与 `user:` 分开的理由
/// 同上——包里的模板与库里的同名时，frontmatter 记下的指针也要分得清是哪一份。
const PACK_KEY_PREFIX: &str = "pack:";

/// 一个用户自定义模板（结构化区块清单，Granola 形态）。
///
/// **刻意不做「自由 prompt」式模板**：产出结构是下游多处的契约（区块标题决定笔记长
/// 什么样、来源区由 Rust 拼接在末尾），一段自由文本无法保证模型给出可预期的结构，
/// 出了问题也无从定位是模板写坏了还是模型不听话。区块清单既够表达，又让"哪里不对"
/// 一眼可见。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTemplate {
    /// 稳定 id（Rust 生成，用户不可见也不可改——改名不该让既有蒸馏笔记的指针失效）。
    pub id: String,
    /// 展示名（用户自己写的，直接呈现，不进 i18n）。
    pub name: String,
    /// 输出区块（顺序即产出顺序）。
    pub sections: Vec<UserSection>,
    /// 本模板默认是否附整理稿。
    #[serde(default)]
    pub full_text_default: bool,
}

/// 用户模板的一个区块：标题 + 给模型的一句指导语。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSection {
    /// 小节标题，**按用户写的原样输出**（不随输出语言切换——这是他自己的措辞）。
    pub heading: String,
    /// 指导语（可空：只给标题时由模型自行判断该写什么）。
    #[serde(default)]
    pub guidance: String,
}

/// 单个模板允许的区块数上限（多到读不完的纪要没有价值，且每多一节都摊薄模型注意力）。
pub const MAX_USER_SECTIONS: usize = 12;
/// 模板名与区块标题的长度上限。
pub const MAX_TEMPLATE_NAME_CHARS: usize = 40;
/// 指导语长度上限。
pub const MAX_GUIDANCE_CHARS: usize = 200;
/// 一个库里允许的用户模板数上限（选单要保持可扫读）。
pub const MAX_USER_TEMPLATES: usize = 30;

/// 模板校验失败的原因（稳定键 → 前端 i18n `distill.templateError.*`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateError {
    /// 模板名为空。
    EmptyName,
    /// 模板名过长。
    NameTooLong,
    /// 一个区块都没有。
    NoSections,
    /// 区块过多。
    TooManySections,
    /// 某个区块标题为空。
    EmptyHeading,
    /// 某个区块标题过长。
    HeadingTooLong,
    /// 指导语过长。
    GuidanceTooLong,
    /// 区块标题重复（同名小节会让模型无从判断该往哪一节写）。
    DuplicateHeading,
}

impl TemplateError {
    /// 稳定字符串键（i18n 动态键后缀）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::EmptyName => "empty_name",
            Self::NameTooLong => "name_too_long",
            Self::NoSections => "no_sections",
            Self::TooManySections => "too_many_sections",
            Self::EmptyHeading => "empty_heading",
            Self::HeadingTooLong => "heading_too_long",
            Self::GuidanceTooLong => "guidance_too_long",
            Self::DuplicateHeading => "duplicate_heading",
        }
    }
}

/// 校验一个用户模板（**保存前**的唯一判定点，前端不重复实现，ADR-003）。
///
/// 标题里的 `#` 与换行会被剥掉而不是报错：用户按 Markdown 习惯写 `## 概要` 是很自然的
/// 事，为此弹一条错误既啰嗦又显得机器很笨——能无歧义纠正的就纠正（同 `normalize`）。
///
/// # Errors
/// 名称/区块为空、超长或标题重复时返回对应的 [`TemplateError`]。
pub fn validate(template: &UserTemplate) -> Result<(), TemplateError> {
    let name = template.name.trim();
    if name.is_empty() {
        return Err(TemplateError::EmptyName);
    }
    if name.chars().count() > MAX_TEMPLATE_NAME_CHARS {
        return Err(TemplateError::NameTooLong);
    }
    if template.sections.is_empty() {
        return Err(TemplateError::NoSections);
    }
    if template.sections.len() > MAX_USER_SECTIONS {
        return Err(TemplateError::TooManySections);
    }
    let mut seen: Vec<String> = Vec::with_capacity(template.sections.len());
    for section in &template.sections {
        let heading = normalize_heading(&section.heading);
        if heading.is_empty() {
            return Err(TemplateError::EmptyHeading);
        }
        if heading.chars().count() > MAX_TEMPLATE_NAME_CHARS {
            return Err(TemplateError::HeadingTooLong);
        }
        if section.guidance.trim().chars().count() > MAX_GUIDANCE_CHARS {
            return Err(TemplateError::GuidanceTooLong);
        }
        // 大小写不敏感比对：`要点` 与 `要点 ` 是同一节，`Key` 与 `key` 也是。
        let folded = heading.to_lowercase();
        if seen.contains(&folded) {
            return Err(TemplateError::DuplicateHeading);
        }
        seen.push(folded);
    }
    Ok(())
}

/// 规范化一个区块标题：剥前导 `#`、去首尾空白、压掉换行。
#[must_use]
pub fn normalize_heading(raw: &str) -> String {
    raw.replace(['\n', '\r'], " ")
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_owned()
}

/// 规范化整个模板（保存前跑一次，使存进去的就是将来喂给模型的那份）。
#[must_use]
pub fn normalize(mut template: UserTemplate) -> UserTemplate {
    template.name = template.name.trim().to_owned();
    for section in &mut template.sections {
        section.heading = normalize_heading(&section.heading);
        section.guidance = section.guidance.trim().to_owned();
    }
    template
}

/// 模板包带来的整理模板（ADR-169 ⑤）：与用户模板同一种结构，只是不归库管——随包装上、随包卸掉，
/// 不在库里编辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackTemplate {
    /// 来自哪个模板包（包 id）。
    pub pack: String,
    pub template: UserTemplate,
}

/// 除内置之外可选的模板：库里的用户模板与内容层里模板包带来的。
#[derive(Clone, Copy, Debug, Default)]
pub struct TemplateSet<'a> {
    pub user: &'a [UserTemplate],
    pub packs: &'a [PackTemplate],
}

impl<'a> TemplateSet<'a> {
    /// 只有库里的用户模板。
    #[must_use]
    pub fn user(user: &'a [UserTemplate]) -> Self {
        Self { user, packs: &[] }
    }
}

/// 解析模板包里的一份整理模板（TOML：`name`、可选的 `full_text_default`、`[[sections]]` 的
/// `heading` 与可选的 `guidance`），并过与库内模板同一道校验。`id` 由调用方给（包内文件名）。
///
/// # Errors
/// 不是合规的 TOML、多了未知字段，或不过 [`validate`] 时。
pub fn parse_pack_template(id: &str, source: &str) -> Result<UserTemplate, PackTemplateError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Raw {
        name: String,
        #[serde(default)]
        full_text_default: bool,
        sections: Vec<RawSection>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawSection {
        heading: String,
        #[serde(default)]
        guidance: String,
    }
    let raw: Raw =
        toml::from_str(source).map_err(|error| PackTemplateError::Parse(error.to_string()))?;
    let template = normalize(UserTemplate {
        id: id.to_owned(),
        name: raw.name,
        sections: raw
            .sections
            .into_iter()
            .map(|section| UserSection {
                heading: section.heading,
                guidance: section.guidance,
            })
            .collect(),
        full_text_default: raw.full_text_default,
    });
    validate(&template).map_err(PackTemplateError::Invalid)?;
    Ok(template)
}

/// [`parse_pack_template`] 的失败原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackTemplateError {
    Parse(String),
    Invalid(TemplateError),
}

impl std::fmt::Display for PackTemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "invalid distill template: {reason}"),
            Self::Invalid(error) => write!(f, "invalid distill template: {}", error.key()),
        }
    }
}

impl std::error::Error for PackTemplateError {}

/// 一个可用于生成的模板引用（内置或用户自定义）。
///
/// 引入它而不是把用户模板塞进 [`TemplateId`]：内置模板是**编译期已知的四个**（区块是
/// `&'static` 常量、有中英两档标题），用户模板是运行期数据（一套标题、随库存储）。
/// 硬并成一个枚举会让两边各背一半用不上的字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateRef<'a> {
    Builtin(TemplateId),
    User(&'a UserTemplate),
    Pack(&'a PackTemplate),
}

impl TemplateRef<'_> {
    /// 落 frontmatter 的稳定键。
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Builtin(id) => id.key().to_owned(),
            Self::User(template) => format!("{USER_KEY_PREFIX}{}", template.id),
            Self::Pack(pack) => format!("{PACK_KEY_PREFIX}{}/{}", pack.pack, pack.template.id),
        }
    }

    /// 本模板默认是否附整理稿。
    #[must_use]
    pub fn full_text_default(&self) -> bool {
        match self {
            Self::Builtin(id) => id.full_text_default(),
            Self::User(template) => template.full_text_default,
            Self::Pack(pack) => pack.template.full_text_default,
        }
    }

    /// 文件名后缀。用户模板一律「笔记」——「纪要」是会议这一体裁的专名，
    /// 套在用户自定义结构上多半名不副实。
    #[must_use]
    pub fn name_suffix(&self) -> &'static str {
        match self {
            Self::Builtin(id) => id.name_suffix(),
            Self::User(_) | Self::Pack(_) => "笔记",
        }
    }

    /// 区块（标题 + 指导语），标题已按输出语言定档（用户模板恒用其自写标题）。
    fn section_pairs(&self, lang: &Lang) -> Vec<(&str, &str)> {
        match self {
            Self::Builtin(id) => id
                .sections()
                .iter()
                .map(|section| (section.heading(lang), section.guidance))
                .collect(),
            Self::User(template) => user_section_pairs(template),
            Self::Pack(pack) => user_section_pairs(&pack.template),
        }
    }
}

fn user_section_pairs(template: &UserTemplate) -> Vec<(&str, &str)> {
    template
        .sections
        .iter()
        .map(|section| (section.heading.as_str(), section.guidance.as_str()))
        .collect()
}

/// 按键查一个模板（内置、库里的用户模板、模板包带来的，三个键空间互不相撞）。未知键 → `None`，
/// 调用方回退建议。
#[must_use]
pub fn find<'a>(key: &str, templates: TemplateSet<'a>) -> Option<TemplateRef<'a>> {
    if let Some(id) = TemplateId::from_key(key) {
        return Some(TemplateRef::Builtin(id));
    }
    let key = key.trim();
    if let Some(id) = key.strip_prefix(USER_KEY_PREFIX) {
        return templates
            .user
            .iter()
            .find(|template| template.id == id)
            .map(TemplateRef::User);
    }
    let (pack, id) = key.strip_prefix(PACK_KEY_PREFIX)?.split_once('/')?;
    templates
        .packs
        .iter()
        .find(|entry| entry.pack == pack && entry.template.id == id)
        .map(TemplateRef::Pack)
}

/// 一次解析的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// 本次实际要用的模板键。
    pub key: String,
    /// 本次实际的整理稿开关。
    pub full_text: bool,
    /// 规则建议的模板（**恒为内置**——规则不认识用户模板，也不该替用户猜）。
    pub suggested: TemplateId,
    /// 建议的判据。
    pub reason: SuggestReason,
}

/// 解析本次该用哪个模板、要不要附整理稿。**四条触发路径的唯一判定点**。
///
/// 优先级：**显式指定（用户这次选的） > 已记录（既有蒸馏笔记 frontmatter）> 规则建议**。
///
/// 少了中间那一层，「源已更新」重生成 / 自动档 / 批量整理都会按规则重新建议，把用户
/// 手选过的模板**静默换掉**——而批量的待办桶里本就**包含**「已生成但源已更新」这些重生成
/// 件，所以这不是边角情形，是必然发生。
///
/// 已记录但**已被删除**的用户模板：回退建议（模板没了就是没了，不复活也不报错——用户删
/// 模板是显式动作，为此拦住一次重新整理是本末倒置）。
#[must_use]
pub fn resolve(
    explicit: Option<&str>,
    explicit_full_text: Option<bool>,
    recorded: Option<&str>,
    recorded_full_text: Option<bool>,
    shape: SourceShape,
    templates: TemplateSet<'_>,
) -> Resolution {
    let (suggested, reason) = suggest(shape);
    let explicit_ref = explicit.and_then(|key| find(key, templates));
    let recorded_ref = recorded.and_then(|key| find(key, templates));
    let chosen = explicit_ref
        .or(recorded_ref)
        .unwrap_or(TemplateRef::Builtin(suggested));

    // 整理稿开关**跟着最终模板走**：用户换了模板却没碰这个勾选时，取新模板的默认值比
    // 沿用旧模板的取值更符合预期（他选的是"按这个模板整理"，那就该带上它该有的东西）。
    let keeps_recorded = explicit_ref.is_none() || explicit_ref == recorded_ref;
    let full_text = explicit_full_text
        .or(if keeps_recorded {
            recorded_full_text
        } else {
            None
        })
        .unwrap_or_else(|| chosen.full_text_default());

    Resolution {
        key: chosen.key(),
        full_text,
        suggested,
        reason,
    }
}

/// 输入形态前言（**跟着源走**，与模板无关）。措辞归注册表 `distill` 域（ADR-050）。
fn input_preamble(
    registry: &PromptRegistry,
    shape: SourceShape,
    has_speaker_labels: bool,
) -> String {
    let distill =
        |key: &str, vars: &[(&str, &str)]| registry.render(PromptScope::Distill, key, vars);
    match shape {
        SourceShape::Conversation => distill("preamble.conversation", &[]),
        SourceShape::OwnRecording { .. } | SourceShape::ImportedTranscript { .. } => {
            let attribution = if has_speaker_labels {
                distill("preamble.attribution", &[])
            } else {
                String::new()
            };
            distill("preamble.transcript", &[("attribution", &attribution)])
        }
        // 记录（ADR-086）：输入本来就是用户敲下的散文。**绝不说"转写稿"**——那会让模型
        // 去"去掉时间码与口头语"，而这里没有时间码可去，它只会开始改写用户的措辞。
        SourceShape::WrittenNotes => distill("preamble.written_notes", &[]),
    }
}

/// 合成最终蒸馏指令（三段正交拼装：输入形态 + 输出结构 + 纪律）。
///
/// 产出的是**能独立读懂**的一篇笔记——读者几个月后没有原始素材也能看明白。
/// 来源上**已经确认过的事实**（PRD-06-058 C′ 与 PRD-06-054 的衔接）：语音速记落盘后的 AI 提炼
/// 已经把待办与日期抠出来并写回了那一篇，蒸馏时把它们作为既定事实交给模型、要求原样保留——
/// 否则同一段话被模型第二次"理解"，待办措辞会漂、日期可能被丢。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownFacts {
    /// 已提炼的行动项（原文措辞）。
    pub action_items: Vec<String>,
    /// 已提炼的日期线索（ISO `YYYY-MM-DD`）。
    pub dates: Vec<String>,
}

impl KnownFacts {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.action_items.is_empty() && self.dates.is_empty()
    }

    fn sentence(&self, registry: &PromptRegistry) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if !self.action_items.is_empty() {
            parts.push(registry.render(
                PromptScope::Distill,
                "known_facts.action_items",
                &[("items", &self.action_items.join("; "))],
            ));
        }
        if !self.dates.is_empty() {
            parts.push(registry.render(
                PromptScope::Distill,
                "known_facts.dates",
                &[("dates", &self.dates.join(", "))],
            ));
        }
        registry.render(
            PromptScope::Distill,
            "known_facts",
            &[("facts", &parts.join(" | "))],
        )
    }
}

#[must_use]
pub fn compose_instruction(
    registry: &PromptRegistry,
    template: TemplateRef<'_>,
    shape: SourceShape,
    has_speaker_labels: bool,
    lang: &Lang,
    facts: &KnownFacts,
) -> Instruction {
    let sections = template
        .section_pairs(lang)
        .into_iter()
        .map(|(heading, guidance)| {
            // 指导语可空（用户只给标题时）：不写成 `## 标题 ()` 那种空括号，
            // 它对模型是噪声，对读到 prompt 的人是困惑。
            if guidance.is_empty() {
                format!("`## {heading}`")
            } else {
                format!("`## {heading}` ({guidance})")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    registry.instruction(
        PromptScope::Distill,
        "instruction",
        &[
            (
                "preamble",
                &input_preamble(registry, shape, has_speaker_labels),
            ),
            ("lang", lang.display_name()),
            ("sections", &sections),
            ("facts", &facts.sentence(registry)),
        ],
    )
}

/// 分片摘要（map 阶段）指令：只做忠实压缩，**不要求结构**——结构留给汇总阶段，
/// 否则每片都长出一套「决议 / 行动项」，汇总时就是一堆互相矛盾的小标题。
#[must_use]
pub fn compose_chunk_instruction(registry: &PromptRegistry, lang: &Lang) -> Instruction {
    registry.instruction(
        PromptScope::Distill,
        "chunk",
        &[("lang", lang.display_name())],
    )
}

/// 整理稿（rewrite 阶段）指令：**改写而非压缩**。
///
/// 这条指令与 map-reduce 的摘要指令目标相反，故整理稿**绝不能走 reduce**（reduce 的
/// 本职就是压缩）——逐片改写后由调用方按序确定性拼接。同理，它必须以
/// [`crate::SummaryStyle::Faithful`] 提交，否则 Provider 会在外层套上
/// "Summarize the user's text concisely" 的前缀，与本指令正面相冲。
#[must_use]
pub fn compose_rewrite_instruction(registry: &PromptRegistry, lang: &Lang) -> Instruction {
    registry.instruction(
        PromptScope::Distill,
        "rewrite",
        &[("lang", lang.display_name())],
    )
}

/// 汇总蒸馏（多篇 → 一篇）指令：把一堆笔记按主题归纳成一篇，而不是逐条复述。
///
/// 与单篇蒸馏刻意分开：候选集来自多个来源，没有统一的"输入形态"可言，模板体系
/// （按体裁分类单一素材）在这里不适用（PRD-06-047）。
#[must_use]
pub fn compose_merge_instruction(
    registry: &PromptRegistry,
    lang: &Lang,
    count: usize,
) -> Instruction {
    registry.instruction(
        PromptScope::Distill,
        "merge",
        &[("lang", lang.display_name()), ("count", &count.to_string())],
    )
}

/// 剥掉模型抄进蒸馏产出的**死引用角标**（`[1]`、`[1][4]` 这类）。
///
/// **为什么必须确定性地剥，而不是只在 prompt 里请求**：蒸馏的输入常常是**本身带
/// 角标的素材**——对话归档里 `PRD-16-005` 的 `[n]` 角标就逐字躺在正文里。模型做摘要
/// 时会把它们连同句子一起搬进产出，而蒸馏笔记的「来源」区是 Rust 拼的、通常只有一条：
/// 于是正文里出现 `[4]`、`[3][5]` 这种**指向不存在来源**的编号。
///
/// 它比"多余的噪声"更糟：CommonMark 把 `[4]` 当**引用式链接**，编辑器于是按链接着色、
/// 在 Live Preview 里连方括号一起藏掉——用户看到一个链接色的数字，点下去什么也不会发生。
/// 这正是 `PRD-16-005` 明令禁止的那件事：「把角标做得容易显示、难以点开就是在制造过度
/// 信任」。而**把它们改成可点**是错的更远一步：模型是把角标抄到**改写过**的句子上的，
/// 那个对应关系根本不成立，做成可跳转等于凭空捏造出处。
///
/// 故唯一正确的产出是：正文不带角标，出处只由 Rust 拼的「来源」区表达。
///
/// 刻意**不**碰的四类（都不是引用角标）：
/// - 真链接 `[文本](路径)` 与图片 `![alt](路径)`——后面跟着 `(`；
/// - wikilink `[[目标]]`（`PRD-06-030` 的两类链接语法之一）；
/// - 待办标记 `- [ ]` / `- [x]`——方括号里不是数字，天然不匹配；
/// - 下标式写法 `list[1]`、`arr[0]`——紧挨在标识符字符后面，是内容不是引用。
///
/// 代码（围栏块与行内代码）整块跳过：里面的 `[0]` 是代码。
#[must_use]
pub fn strip_dead_citations(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&strip_line(line));
    }
    out
}

/// 单行剥离。分出来是为了让 [`strip_dead_citations`] 只负责「哪些行该被看」。
fn strip_line(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < line.len() {
        let ch = line[i..].chars().next().expect("i 恒在字符边界上");
        // 行内代码整段照抄：`` `list[0]` `` 里的方括号是代码不是引用。
        if ch == '`' {
            let ticks = line[i..].chars().take_while(|c| *c == '`').count();
            let fence = &line[i..i + ticks];
            let rest = &line[i + ticks..];
            let end = rest
                .find(fence)
                .map_or(line.len(), |p| i + ticks + p + ticks);
            out.push_str(&line[i..end]);
            i = end;
            continue;
        }
        // wikilink 开头照抄两个字节即可：其内部再无 `[` 开头的位置可被误判。
        if line[i..].starts_with("[[") {
            out.push_str("[[");
            i += 2;
            continue;
        }
        if ch == '[' {
            if let Some(end) = citation_run_end(line, i) {
                let after = line[end..].chars().next();
                let before = out.chars().next_back();
                let is_inline_link = after == Some('(');
                // `[1]: http://…` 是链接引用定义，不是正文角标。
                let is_ref_definition = after == Some(':');
                // `list[1]`：紧挨标识符字符 → 下标写法，是内容。
                let is_subscript = before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                if !is_inline_link && !is_ref_definition && !is_subscript {
                    let leading_only = out.chars().all(char::is_whitespace);
                    let after_is_word = after.is_some_and(|c| c.is_ascii_alphanumeric());
                    // 整串被一对括号包着（`（[1], [2]）` / `([1][2])`）：括号是**为角标而生**的，
                    // 角标走了它就该一起走，否则留下一对空括号——比不剥更难看。
                    let wrapped = before
                        .zip(after)
                        .is_some_and(|(b, a)| matches!((b, a), ('(', ')') | ('（', '）')));
                    if wrapped {
                        out.pop();
                        i = end + after.map_or(0, char::len_utf8);
                        if !out.chars().all(char::is_whitespace) && out.ends_with(' ') {
                            out.pop();
                        }
                        continue;
                    }
                    if leading_only {
                        // 角标在行首（缩进之后）：吃掉它后面的一个空格，别在句首留个坑。
                        i = end + usize::from(after == Some(' '));
                    } else {
                        // 常规位置：连同它**前面**的那个空格一起去掉，
                        // 否则 "结果 [4]。" 会剩成 "结果 。"。
                        if !after_is_word && out.ends_with(' ') {
                            out.pop();
                        }
                        i = end;
                    }
                    continue;
                }
            }
        }
        let step = ch.len_utf8();
        out.push_str(&line[i..i + step]);
        i += step;
        debug_assert!(i <= bytes.len());
    }
    out
}

/// 角标之间允许出现的分隔符。
const CITATION_SEPARATORS: &[char] = &[' ', ',', '\u{ff0c}', '\u{3001}', ';', '\u{ff1b}'];

/// 角标之间允许出现的连接词（**必须**后面紧跟另一个角标才被吃掉）。
///
/// 「见 \[1\]、\[2\] 与 \[3\]」里的「与」属于这串引用列表，不属于句子；把它留下会剩出
/// 「见 与。」这种断句。反过来「见 \[1\] 和结论 \[3\]」里的「和」后面跟的是「结论」而不是
/// 角标，于是不匹配、原样保留——**连接词只有夹在两个角标中间时才算列表的一部分**。
const CITATION_CONNECTORS: &[&str] = &["\u{4e0e}", "\u{548c}", "\u{53ca}", "\u{6216}", "and", "or"];

/// 从 `start`（必须指向 `[`）起，一整串角标的结束位置；一组都不成立则 `None`。
///
/// 认「一整串」而非逐组，有两个理由：① `[1][4]` 在 CommonMark 里是**一个**引用式链接
/// 节点，逐组处理会把它劈成两半；② 逐组剥 `([1], [2], [3])` 会剩下 `(,,,)`——**制造出
/// 比原样保留更难看的东西**，那样的"修复"不如不做。故分隔符与夹在中间的连接词一并纳入，
/// 但只有当它们后面**确实**还跟着一个角标时才吃掉。
fn citation_run_end(line: &str, start: usize) -> Option<usize> {
    let mut pos = start;
    let mut groups = 0usize;
    while let Some(next) = one_citation_end(line, pos) {
        pos = next;
        groups += 1;
        // 试探：分隔符（+ 可选连接词 + 分隔符）之后还是不是角标？是才把它们并进来。
        let mut probe = pos;
        probe += span_of(&line[probe..], CITATION_SEPARATORS);
        if let Some(word) = CITATION_CONNECTORS
            .iter()
            .find(|w| line[probe..].starts_with(**w))
        {
            let after_word = probe + word.len();
            let gap = span_of(&line[after_word..], CITATION_SEPARATORS);
            // 连接词必须**两侧都有分隔**（`与[1]` 与 `X与[1]` 里的「与」可能是词的一部分）。
            if gap > 0 && probe > pos {
                probe = after_word + gap;
            }
        }
        if probe > pos && one_citation_end(line, probe).is_some() {
            pos = probe;
        }
    }
    (groups > 0).then_some(pos)
}

/// 单个 `[数字]` 组的结束位置。
fn one_citation_end(line: &str, at: usize) -> Option<usize> {
    let rest = line.get(at..)?.strip_prefix('[')?;
    let close = rest.find(']')?;
    let inner = &rest[..close];
    (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit())).then(|| at + 1 + close + 1)
}

/// 开头连续属于 `set` 的字符所占字节数。
fn span_of(s: &str, set: &[char]) -> usize {
    s.chars()
        .take_while(|c| set.contains(c))
        .map(char::len_utf8)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::PromptRegistry;

    #[test]
    fn dead_citation_markers_are_stripped_with_their_leading_space() {
        // 真机原样：用户 2026-08-04 报的那篇蒸馏笔记，来源区只有 1 条而正文引用 1/3/4/5。
        let cases = [
            (
                "反馈数据路由更新及测试结果 [4]。在此期间",
                "反馈数据路由更新及测试结果。在此期间",
            ),
            (
                "以避免阻碍 SQ 团队的测试和发布流程 [1][4]。这需要",
                "以避免阻碍 SQ 团队的测试和发布流程。这需要",
            ),
            (
                "不在此阶段直接干预核心逻辑修改 [3][5]。",
                "不在此阶段直接干预核心逻辑修改。",
            ),
            (
                "以便审计 AWS 账户的发送状态 [4]。",
                "以便审计 AWS 账户的发送状态。",
            ),
            ("the result [4]. Next", "the result. Next"),
            ("结论 [1] 很重要", "结论 很重要"),
            ("[1] 行首角标", "行首角标"),
            ("  - 要点 [2]。", "  - 要点。"),
        ];
        for (input, want) in cases {
            assert_eq!(strip_dead_citations(input), want, "输入：{input}");
        }
    }

    #[test]
    fn enumerated_marker_lists_go_away_whole_instead_of_leaving_debris() {
        // 逐组剥离会把 `([1], [2], [3])` 剩成 `(,,,)`——**制造出比原样保留更难看的
        // 东西**。分隔符、夹在两个角标之间的连接词、以及"专为角标而生"的那对括号，
        // 必须与角标一并消失。真机样本来自「验收 出处五态 · 笔记」。
        let cases = [
            (
                "- **数据来源**：共五条依据 ([1], [2], [3], [4], [5])。",
                "- **数据来源**：共五条依据。",
            ),
            ("见 [1]、[2] 与 [3]。", "见。"),
            ("依据 [1]; [2].", "依据."),
            ("See [1] and [2].", "See."),
            ("参见（[1]，[2]）。", "参见。"),
        ];
        for (input, want) in cases {
            assert_eq!(strip_dead_citations(input), want, "输入：{input}");
        }
    }

    #[test]
    fn a_connector_not_between_two_markers_survives() {
        // 反向判据：连接词只有**夹在两个角标之间**时才属于引用列表。「和结论」里的
        // 「和」后面跟的是词不是角标，吃掉它就把句子改坏了。
        let cases = [
            ("见 [1] 和结论 [3] 都成立。", "见 和结论 都成立。"),
            ("对比 [1] 与实测值 [2] 的差异。", "对比 与实测值 的差异。"),
        ];
        for (input, want) in cases {
            assert_eq!(strip_dead_citations(input), want, "输入：{input}");
        }
    }

    #[test]
    fn strip_leaves_every_non_citation_bracket_alone() {
        // 这四类都不是引用角标，被误剥的代价分别是：链接失效、wikilink 断掉、
        // 待办变普通行、代码被改写。逐条锁死。
        let keep = [
            "真链接 [标题](../Notes/a.md) 在此",
            "图片 ![alt](../assets/x.png) 在此",
            "wikilink [[目标笔记]] 在此",
            "带别名 [[目标|别名]] 在此",
            "- [ ] 待办项",
            "- [x] 已完成",
            "下标写法 list[1] 与 arr[0] 是内容",
            "行内代码 `list[1]` 是代码",
            "链接引用定义 [1]: https://example.com",
        ];
        for line in keep {
            assert_eq!(strip_dead_citations(line), line, "不该改动：{line}");
        }
    }

    #[test]
    fn fenced_code_blocks_are_left_verbatim() {
        // 围栏里的 `[0]` 是代码；剥它等于改写用户的代码块。
        let src = "正文 [1]。\n\n```rust\nlet x = v[0];\nlet y = v[1][2];\n```\n\n尾部 [2]。";
        let want = "正文。\n\n```rust\nlet x = v[0];\nlet y = v[1][2];\n```\n\n尾部。";
        assert_eq!(strip_dead_citations(src), want);
    }

    #[test]
    fn strip_is_idempotent_and_preserves_clean_bodies() {
        // 没有角标的正文必须逐字不动——否则这个函数会在每次重生成时悄悄改写用户内容。
        let clean = "## 结论\n\n这是一段没有任何角标的正文。\n\n- 要点一\n- 要点二\n";
        assert_eq!(strip_dead_citations(clean), clean);
        let once = strip_dead_citations("结果 [1][2]。");
        assert_eq!(strip_dead_citations(&once), once, "剥离必须幂等");
    }

    #[test]
    fn known_facts_are_pinned_verbatim_and_absent_when_empty() {
        let lang = Lang::new("zh-Hans");
        let plain = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::WrittenNotes,
            false,
            &lang,
            &KnownFacts::default(),
        );
        assert!(!plain.contains("already extracted"));
        let pinned = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::WrittenNotes,
            false,
            &lang,
            &KnownFacts {
                action_items: vec!["通知小李".into(), "订会议室".into()],
                dates: vec!["2026-09-10".into()],
            },
        );
        assert!(
            pinned.contains("action items: 通知小李; 订会议室"),
            "{pinned}"
        );
        assert!(pinned.contains("dates (ISO): 2026-09-10"), "{pinned}");
        assert!(pinned.contains("keep them exactly as given"));
    }

    #[test]
    fn distill_instructions_forbid_citation_markers() {
        // 确定性剥离是兜底，prompt 是第一道——两道都在，才不会出现「模型照抄、
        // 剥离改坏」的中间态。三条蒸馏指令都得说这句话。
        let lang = Lang::new("zh-Hans");
        let single = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::Conversation,
            false,
            &lang,
            &KnownFacts::default(),
        );
        let merge = compose_merge_instruction(&PromptRegistry::bundled(), &lang, 3);
        let chunk = compose_chunk_instruction(&PromptRegistry::bundled(), &lang);
        for (name, text) in [("单篇", &single), ("汇总", &merge), ("分片", &chunk)] {
            assert!(
                text.contains("citation"),
                "{name}蒸馏指令没有禁止引用角标：{text}"
            );
        }
    }

    #[test]
    fn template_keys_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for id in TemplateId::ALL {
            assert!(seen.insert(id.key()), "模板键重复：{}", id.key());
            assert_eq!(TemplateId::from_key(id.key()), Some(id));
        }
        assert_eq!(TemplateId::from_key("nope"), None);
        // 存量产物（无此字段）与手写错值都要回退建议规则，而不是 panic。
        assert_eq!(TemplateId::from_key(""), None);
    }

    #[test]
    fn suggestion_rules_cover_every_shape() {
        // 对话恒结晶式。
        assert_eq!(
            suggest(SourceShape::Conversation),
            (TemplateId::Chat, SuggestReason::Conversation)
        );
        // 自录音频默认按会议（未细分时 speakers=0，此时**不能**判成单人讲述）。
        assert_eq!(
            suggest(SourceShape::OwnRecording { speakers: 0 }),
            (TemplateId::Meeting, SuggestReason::OwnRecording)
        );
        assert_eq!(
            suggest(SourceShape::OwnRecording { speakers: 3 }),
            (TemplateId::Meeting, SuggestReason::MultipleSpeakers)
        );
        // 导入文稿：多说话人 → 会议；单人/未细分 → 讲述。
        assert_eq!(
            suggest(SourceShape::ImportedTranscript { speakers: 2 }),
            (TemplateId::Meeting, SuggestReason::MultipleSpeakers)
        );
        assert_eq!(
            suggest(SourceShape::ImportedTranscript { speakers: 1 }),
            (TemplateId::Talk, SuggestReason::SingleSpeaker)
        );
        assert_eq!(
            suggest(SourceShape::ImportedTranscript { speakers: 0 }),
            (TemplateId::Talk, SuggestReason::SingleSpeaker)
        );
        // 记录（ADR-086 / PRD-06-054）：会议纪要式，且**默认不附整理稿**——记录的整理稿
        // 就是把用户自己写的散文重抄一遍。第二条断言正是把这个理由钉住：改建议成讲述式，
        // 它会立刻变红（讲述式是唯一默认附整理稿的模板）。
        assert_eq!(
            suggest(SourceShape::WrittenNotes),
            (TemplateId::Meeting, SuggestReason::WrittenNotes)
        );
        assert!(!suggest(SourceShape::WrittenNotes).0.full_text_default());
    }

    /// 记录的指令里**不得**出现"转写稿"这个词：说了它，模型就会去"删掉时间码与口头语"
    /// ——而这里没有时间码可删，它只会开始改写用户自己的措辞。
    #[test]
    fn written_notes_are_never_described_as_a_transcript() {
        let text = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::WrittenNotes,
            false,
            &Lang::new("zh-CN"),
            &KnownFacts::default(),
        );
        assert!(text.contains("own rough notes"), "{text}");
        assert!(!text.contains("transcript of recorded audio"), "{text}");
        // 说话人归属对手写记录没有意义：即便调用方传了 true 也不该冒出来。
        let labelled = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::WrittenNotes,
            true,
            &Lang::new("zh-CN"),
            &KnownFacts::default(),
        );
        assert!(!labelled.contains("speaker's name"), "{labelled}");
    }

    #[test]
    fn interview_is_never_suggested_automatically() {
        // 「两个说话人」分不清会议与访谈；访谈式只在手动选择时出现。
        for speakers in 0..6 {
            for shape in [
                SourceShape::OwnRecording { speakers },
                SourceShape::ImportedTranscript { speakers },
            ] {
                assert_ne!(suggest(shape).0, TemplateId::Interview);
            }
        }
        assert_ne!(suggest(SourceShape::Conversation).0, TemplateId::Interview);
    }

    #[test]
    fn section_headings_switch_with_output_language() {
        let zh = Lang::from("zh-Hans");
        let en = Lang::from("en-US");
        let meeting = TemplateId::Meeting.sections();
        assert_eq!(meeting[0].heading(&zh), "概要");
        assert_eq!(meeting[0].heading(&en), "Overview");
        // 非中文系一律英文标题（不为每种语言各造一套）。
        assert_eq!(meeting[0].heading(&Lang::from("ja")), "Overview");
    }

    #[test]
    fn instruction_pairs_any_template_with_any_source_shape() {
        let lang = Lang::from("zh");
        // 对话 + 会议模板：前言必须如实说这是对话，绝不谎称"录音转写"。
        let text = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            SourceShape::Conversation,
            false,
            &lang,
            &KnownFacts::default(),
        );
        assert!(text.contains("conversation between a user and an AI assistant"));
        assert!(!text.contains("transcript of recorded audio"));
        // 区块结构跟着模板走。
        assert!(text.contains("## 决议"));
        assert!(text.contains("Chinese"));

        // 转写 + 讲述模板：前言说转写，区块是讲述式的。
        let talk = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Talk),
            SourceShape::ImportedTranscript { speakers: 1 },
            false,
            &lang,
            &KnownFacts::default(),
        );
        assert!(talk.contains("transcript of recorded audio"));
        assert!(talk.contains("## 核心观点"));
        assert!(!talk.contains("## 决议"));
    }

    #[test]
    fn speaker_attribution_only_when_labels_are_present() {
        let lang = Lang::from("en");
        let shape = SourceShape::ImportedTranscript { speakers: 2 };
        assert!(compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            shape,
            true,
            &lang,
            &KnownFacts::default(),
        )
        .contains("Each line starts with the speaker's name"));
        assert!(!compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::Builtin(TemplateId::Meeting),
            shape,
            false,
            &lang,
            &KnownFacts::default(),
        )
        .contains("Each line starts with the speaker's name"));
    }

    #[test]
    fn rewrite_instruction_forbids_summarising() {
        let text = compose_rewrite_instruction(&PromptRegistry::bundled(), &Lang::from("zh"));
        assert!(text.contains("NOT a summary"));
        assert!(text.contains("preserve every fact"));
        // 它是"改写"，绝不能出现要求压缩的字眼。
        assert!(!text.to_lowercase().contains("concise"));
    }

    #[test]
    fn full_text_default_is_on_only_for_talk() {
        assert!(TemplateId::Talk.full_text_default());
        for id in [TemplateId::Meeting, TemplateId::Interview, TemplateId::Chat] {
            assert!(!id.full_text_default(), "{} 不该默认附整理稿", id.key());
        }
    }

    /// 跨语言契约守护：**每个模板 / 判据都必须有中英文案**。
    ///
    /// 模板清单由 Rust 下发（`DistillOptionsDto.templates`，ADR-003），前端照单渲染——
    /// 于是"新增一个模板却漏写文案"这件事**前端测试无从感知**（它只看得见 JSON 里已有
    /// 什么），界面上表现为浮层里一行裸 key。守护只能住在 Rust 侧：这里直接读那两个
    /// i18n JSON，新增枚举变体而没补文案即刻变红。
    #[test]
    fn distill_template_i18n_keys_exist() {
        for locale in ["zh-CN", "en-US"] {
            let path = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/desktop/src/i18n/locales/"
            )
            .to_owned()
                + locale
                + "/workspace.json";
            let raw =
                std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("读不到 {path}：{err}"));
            let distill = &serde_json::from_str::<serde_json::Value>(&raw)
                .expect("workspace.json 应为合法 JSON")["distill"];

            for id in TemplateId::ALL {
                for group in ["template", "templateDesc"] {
                    assert!(
                        distill[group][id.key()].is_string(),
                        "{locale} 缺 distill.{group}.{}",
                        id.key()
                    );
                }
            }
            for reason in [
                SuggestReason::Conversation,
                SuggestReason::OwnRecording,
                SuggestReason::MultipleSpeakers,
                SuggestReason::SingleSpeaker,
                SuggestReason::WrittenNotes,
            ] {
                assert!(
                    distill["reason"][reason.key()].is_string(),
                    "{locale} 缺 distill.reason.{}",
                    reason.key()
                );
            }

            // 队列阶段的文案同样是动态查表（`distill.phase.*` 由徽标、
            // `processing.phase.*` 由任务面板），漏写即界面上一行裸 key。
            let workspace = serde_json::from_str::<serde_json::Value>(&raw).unwrap();
            for phase in ["distilling", "rewriting"] {
                assert!(
                    workspace["distill"]["phase"][phase].is_string(),
                    "{locale} 缺 distill.phase.{phase}"
                );
                assert!(
                    workspace["processing"]["phase"][phase].is_string(),
                    "{locale} 缺 processing.phase.{phase}"
                );
            }
            assert!(
                workspace["processing"]["kind"]["distill"].is_string(),
                "{locale} 缺 processing.kind.distill"
            );

            // 模板校验的错误码同样跨语言：Rust 判定、前端查 `distill.templateError.*`
            // 翻成人话。新增一档校验而漏写文案 = 用户看到的是 `empty_name` 这样的裸码。
            for error in [
                TemplateError::EmptyName,
                TemplateError::NameTooLong,
                TemplateError::NoSections,
                TemplateError::TooManySections,
                TemplateError::EmptyHeading,
                TemplateError::HeadingTooLong,
                TemplateError::GuidanceTooLong,
                TemplateError::DuplicateHeading,
            ] {
                assert!(
                    workspace["distill"]["templateError"][error.key()].is_string(),
                    "{locale} 缺 distill.templateError.{}",
                    error.key()
                );
            }
        }
    }

    fn user_template(id: &str, name: &str) -> UserTemplate {
        UserTemplate {
            id: id.into(),
            name: name.into(),
            sections: vec![UserSection {
                heading: "要点".into(),
                guidance: "what matters".into(),
            }],
            full_text_default: false,
        }
    }

    #[test]
    fn user_and_builtin_keys_live_in_separate_namespaces() {
        // 用户把模板命名成 "meeting" 时不得与内置撞车——键带 `user:` 前缀即两不相干。
        let mine = user_template("abc123", "meeting");
        let templates = vec![mine.clone()];
        assert_eq!(TemplateRef::User(&mine).key(), "user:abc123");
        assert_eq!(
            find("meeting", TemplateSet::user(&templates)),
            Some(TemplateRef::Builtin(TemplateId::Meeting)),
            "裸键恒指内置"
        );
        assert_eq!(
            find("user:abc123", TemplateSet::user(&templates)),
            Some(TemplateRef::User(&mine))
        );
        assert_eq!(find("user:missing", TemplateSet::user(&templates)), None);
    }

    #[test]
    fn resolution_prefers_explicit_then_recorded_then_suggested() {
        let shape = SourceShape::ImportedTranscript { speakers: 1 }; // 建议 = 讲述式
        let mine = user_template("u1", "我的模板");
        let user = vec![mine];

        // ① 显式最高。
        let out = resolve(
            Some("meeting"),
            None,
            Some("user:u1"),
            None,
            shape,
            TemplateSet::user(&user),
        );
        assert_eq!(out.key, "meeting");
        assert_eq!(
            out.suggested,
            TemplateId::Talk,
            "建议仍如实下发，只是没被采用"
        );

        // ② 无显式 → 已记录（含用户模板）。这一层若缺失，重生成会把用户手选的换成建议值。
        let out = resolve(
            None,
            None,
            Some("user:u1"),
            Some(true),
            shape,
            TemplateSet::user(&user),
        );
        assert_eq!(out.key, "user:u1");
        assert!(out.full_text, "整理稿开关与模板成对沿用");

        // ③ 都没有 → 建议。
        let out = resolve(None, None, None, None, shape, TemplateSet::user(&user));
        assert_eq!(out.key, "talk");
        assert_eq!(out.reason, SuggestReason::SingleSpeaker);
    }

    #[test]
    fn batch_and_auto_paths_reuse_the_recorded_template() {
        // 批量与自动档传的就是 (None, None)：待办桶里**包含**「已生成但源已更新」这些
        // 重生成件，这里若按规则重判，用户手选过的模板会被静默换掉。
        let shape = SourceShape::ImportedTranscript { speakers: 2 };
        assert_eq!(suggest(shape).0, TemplateId::Meeting);
        let out = resolve(
            None,
            None,
            Some("interview"),
            Some(true),
            shape,
            TemplateSet::default(),
        );
        assert_eq!(out.key, "interview", "重生成必须沿用用户的选择");
        assert!(out.full_text);
    }

    #[test]
    fn switching_template_takes_the_new_templates_full_text_default() {
        let shape = SourceShape::ImportedTranscript { speakers: 2 };
        // 换模板但没碰勾选 → 取**新模板**的默认值。
        let out = resolve(
            Some("talk"),
            None,
            Some("meeting"),
            Some(false),
            shape,
            TemplateSet::default(),
        );
        assert_eq!(out.key, "talk");
        assert!(out.full_text, "讲述式默认附整理稿");
        // 显式勾选恒最高。
        let out = resolve(
            Some("talk"),
            Some(false),
            Some("meeting"),
            Some(false),
            shape,
            TemplateSet::default(),
        );
        assert!(!out.full_text);
    }

    #[test]
    fn pack_templates_parse_like_user_templates_and_live_in_their_own_key_space() {
        let template = parse_pack_template(
            "weekly",
            "name = \"周报\"\nfull_text_default = true\n\n[[sections]]\nheading = \"## 本周\"\nguidance = \"做了什么\"\n\n[[sections]]\nheading = \"下周\"\n",
        )
        .expect("valid pack template");
        assert_eq!(template.id, "weekly");
        assert_eq!(
            template.sections[0].heading, "本周",
            "与库内模板同一套规范化"
        );
        let packs = [PackTemplate {
            pack: "com.example.kit".into(),
            template: template.clone(),
        }];
        let user = [UserTemplate {
            id: "weekly".into(),
            ..template
        }];
        let set = TemplateSet {
            user: &user,
            packs: &packs,
        };
        assert!(matches!(
            find("pack:com.example.kit/weekly", set),
            Some(TemplateRef::Pack(_))
        ));
        assert!(matches!(
            find("user:weekly", set),
            Some(TemplateRef::User(_))
        ));
        assert_eq!(find("pack:com.example.kit/gone", set), None);
        assert_eq!(find("pack:no-slash", set), None);
        assert_eq!(
            TemplateRef::Pack(&packs[0]).key(),
            "pack:com.example.kit/weekly"
        );
        // 同一道校验：没有区块、多了字段、不是 TOML 都拒。
        assert_eq!(
            parse_pack_template("x", "name = \"空\"\nsections = []\n"),
            Err(PackTemplateError::Invalid(TemplateError::NoSections))
        );
        assert!(matches!(
            parse_pack_template("x", "name = \"a\"\nsections = []\nprompt = \"evil\"\n"),
            Err(PackTemplateError::Parse(_))
        ));
    }

    #[test]
    fn deleted_user_template_falls_back_to_the_suggestion() {
        // 用户删掉了曾经用过的模板：回退建议，不报错也不复活（删除是显式动作）。
        let shape = SourceShape::ImportedTranscript { speakers: 1 };
        let out = resolve(
            None,
            None,
            Some("user:gone"),
            Some(true),
            shape,
            TemplateSet::default(),
        );
        assert_eq!(out.key, "talk");
        // 整理稿开关随之取新模板的默认值，而不是沿用一个已不存在的模板的设定。
        assert!(out.full_text);
    }

    #[test]
    fn unknown_recorded_template_falls_back_to_the_suggestion() {
        let shape = SourceShape::ImportedTranscript { speakers: 1 };
        for raw in ["", "weekly-report", "MEETING"] {
            assert_eq!(
                resolve(None, None, Some(raw), None, shape, TemplateSet::default()).key,
                "talk",
                "未知模板键 {raw:?} 应回退建议"
            );
        }
    }

    #[test]
    fn validation_rejects_the_shapes_that_would_break_generation() {
        let ok = user_template("u", "我的");
        assert_eq!(validate(&ok), Ok(()));

        let mut empty_name = ok.clone();
        empty_name.name = "   ".into();
        assert_eq!(validate(&empty_name), Err(TemplateError::EmptyName));

        let mut no_sections = ok.clone();
        no_sections.sections.clear();
        assert_eq!(validate(&no_sections), Err(TemplateError::NoSections));

        let mut too_many = ok.clone();
        too_many.sections = (0..=MAX_USER_SECTIONS)
            .map(|i| UserSection {
                heading: format!("节{i}"),
                guidance: String::new(),
            })
            .collect();
        assert_eq!(validate(&too_many), Err(TemplateError::TooManySections));

        // 同名小节让模型无从判断该往哪节写（大小写与空白不敏感）。
        let mut duplicate = ok.clone();
        duplicate.sections = vec![
            UserSection {
                heading: "Key points".into(),
                guidance: String::new(),
            },
            UserSection {
                heading: " key POINTS ".into(),
                guidance: String::new(),
            },
        ];
        assert_eq!(validate(&duplicate), Err(TemplateError::DuplicateHeading));

        // 空标题（只写了 `##`）。
        let mut empty_heading = ok;
        empty_heading.sections = vec![UserSection {
            heading: "##".into(),
            guidance: String::new(),
        }];
        assert_eq!(validate(&empty_heading), Err(TemplateError::EmptyHeading));
    }

    #[test]
    fn heading_normalization_fixes_markdown_habits_instead_of_erroring() {
        // 按 Markdown 习惯写 `## 概要` 很自然，为此报错既啰嗦又显得机器笨。
        assert_eq!(normalize_heading("## 概要"), "概要");
        assert_eq!(normalize_heading("  ### Key points \n"), "Key points");
        assert_eq!(normalize_heading("行动项"), "行动项");
    }

    #[test]
    fn user_sections_go_into_the_instruction_verbatim() {
        let mine = UserTemplate {
            id: "u".into(),
            name: "读书笔记".into(),
            sections: vec![
                UserSection {
                    heading: "金句摘抄".into(),
                    guidance: "quotes worth keeping".into(),
                },
                UserSection {
                    heading: "我的想法".into(),
                    guidance: String::new(),
                },
            ],
            full_text_default: true,
        };
        let text = compose_instruction(
            &PromptRegistry::bundled(),
            TemplateRef::User(&mine),
            SourceShape::ImportedTranscript { speakers: 1 },
            false,
            &Lang::from("zh"),
            &KnownFacts::default(),
        );
        assert!(text.contains("`## 金句摘抄` (quotes worth keeping)"));
        // 指导语为空时不写成 `## 我的想法 ()`——空括号对模型是噪声。
        assert!(text.contains("`## 我的想法`"));
        assert!(!text.contains("()"));
        // 用户模板的整理稿默认值随其自身设定。
        assert!(TemplateRef::User(&mine).full_text_default());
        // 用户模板一律「笔记」后缀——「纪要」是会议体裁的专名。
        assert_eq!(TemplateRef::User(&mine).name_suffix(), "笔记");
    }

    #[test]
    fn only_meeting_uses_the_minutes_suffix() {
        assert_eq!(TemplateId::Meeting.name_suffix(), "纪要");
        for id in [TemplateId::Talk, TemplateId::Interview, TemplateId::Chat] {
            assert_eq!(id.name_suffix(), "笔记");
        }
    }
}
