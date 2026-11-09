//! 能力提示词目录（ADR-050）。零 IO，纯数据 + 纯函数。
//!
//! # 两种提示词，只有一种归这里
//!
//! - **用户内容提示词**（对话上下文、笔记正文、剪贴板选区）——永远在客户端侧合成，
//!   服务端转发即忘（ADR-018 零留存）。本模块**不碰它**。
//! - **能力模板提示词**（四能力的 system prompt 模板，含目标语言 / 术语 / 风格占位符）
//!   ——是**产品配置**：它随评测结论演进，节奏与应用发版无关。归本模块。
//!
//! # 为什么必须收敛成一处
//!
//! 收敛之前，同一类提示词散落在三个 crate 里：`luna-provider` 的 `build_chat_messages`
//! （翻译 / 摘要 / 润色的 system 模板）、`luna-core` 的 `builtin_presets`（对话角色与弹窗
//! 模板风格）、`src-tauri` 的 `voice_input`（语音输入的润色与翻译指令）。它们语义同族、
//! 措辞互相牵制（改「保结构」那句话要同时改两处），却没有任何东西能在漏改时变红。
//! 现在**唯一事实源是 `prompts/bundled.json`**，代码里再出现提示词字面量由
//! `scripts/check-prompt-registry-single-source.py` 当场判红。
//!
//! # 三层解析（ADR-017 `Catalog<T>` 的第三个实例）
//!
//! ```text
//! 已验签目录(scope, 当前 locale) > 已验签目录(scope, 默认) > 随包(scope, 当前 locale) > 随包(scope, 默认)
//! ```
//!
//! **随包那一层恒在**：严格本地、离线、验签失败、服务端根本没有提示词目录——
//! 四种情形下产品都照常工作。目录刷新是**增强而不是依赖**，这与模型目录同一条纪律。
//!
//! # `prompt_version`
//!
//! 上报计量的版本号取**整份提示词发布的目录版本**，而不是逐 scope 的版本：发布与回滚
//! 的单位就是一次 release，逐 scope 记版本会让「回滚了哪一版」在账本上拼不回来。
//! `0` = 随包默认（不是「未知」——随包那份也是一个确定的版本，只是它随应用走）。

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// 随包默认模板（编译期嵌入；离线可用，无运行期文件依赖）。
const BUNDLED_JSON: &str = include_str!("../prompts/bundled.json");

/// 模板归属域：四能力 + 一个跨能力共享片段域 + 七个「由能力承载的产品任务」域。
///
/// `Common` 不是第五种能力，它是**共享片段**：`structure`（保留换行/列表/Markdown）
/// 对翻译与润色是同一句话，复制成两份就是下一次只改了一处的起点。
///
/// **任务域（`Editor` 起）不是新能力**（ADR-050 修订 2026-09-05）：它们各自走
/// `Summarize` / `Refine` 这两条既有泳道，只是 system prompt 由调用方整句合成
/// （`SummaryStyle::Faithful` 与 `ExtractTags` 的「调用方决策、引擎只渲染」）。此前这些整句
/// 散在 `src-tauri` 与 `luna-core` 的 `format!` 里——运营改不动、发布下发不到、评测覆盖
/// 不到，而 ADR-050 的门禁只认四能力那 19 句指纹。它们进注册表之后与四能力同一条纪律。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptScope {
    /// 跨能力共享片段。
    Common,
    /// 翻译。
    Translate,
    /// 摘要 / 总结。
    Summarize,
    /// 润色。
    Refine,
    /// 对话。
    Chat,
    /// 笔记 AI 悬浮菜单的各动作（优化 / 校对 / 改语气 / 续写 / 自定义 / 总结 / 纪要 / 解释）。
    Editor,
    /// 资料蒸馏为知识笔记（单篇 / 分片 / 整理稿 / 多篇汇总）。
    Distill,
    /// 库内问答的检索依据前言。
    Knowledge,
    /// 关系标签提取。
    Tags,
    /// 归入主题页（提炼要点 / 归位小标题）。
    Ingest,
    /// 知识体检（声明与源核对）。
    Checkup,
    /// 语音速记的 AI 提炼。
    QuickNote,
}

impl PromptScope {
    /// 全部域（顺序即 `bundled.json` 的书写顺序；完整性断言按它遍历）。
    pub const ALL: [Self; 12] = [
        Self::Common,
        Self::Translate,
        Self::Summarize,
        Self::Refine,
        Self::Chat,
        Self::Editor,
        Self::Distill,
        Self::Knowledge,
        Self::Tags,
        Self::Ingest,
        Self::Checkup,
        Self::QuickNote,
    ];

    /// 线上取值。逐条写死而不是靠 `Debug`：它是**跨仓契约**（服务端按 manifest 里的
    /// 同一组字符串校验发布内容），派生出来的名字会跟着字段改名悄悄变。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Translate => "translate",
            Self::Summarize => "summarize",
            Self::Refine => "refine",
            Self::Chat => "chat",
            Self::Editor => "editor",
            Self::Distill => "distill",
            Self::Knowledge => "knowledge",
            Self::Tags => "tags",
            Self::Ingest => "ingest",
            Self::Checkup => "checkup",
            Self::QuickNote => "quick_note",
        }
    }

    /// 未知取值不回落到某个默认域——一个拼错的 `sumarize` 静默落进 `common`，
    /// 表现是「发布成功了，可是摘要一点没变」。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一个能力提示词集合：`scope × locale × 模板表`。
///
/// **线格式与随包资源同形状**（同一份 schema，两个来源）：服务端签名下发的条目、
/// `prompts/bundled.json` 里的条目、以及评测脚本读的条目都是它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityPromptSet {
    pub scope: PromptScope,
    /// `None` = 默认变体；`Some("zh-CN")` = 该语言的覆盖变体（可只覆盖部分 key）。
    pub locale: Option<String>,
    /// 模板名 → 模板正文。
    pub templates: BTreeMap<String, String>,
}

/// `prompts/bundled.json` 的外壳（`_comment` 等说明字段被忽略，故此处不 deny）。
#[derive(Debug, Deserialize)]
struct BundledPrompts {
    sets: Vec<CapabilityPromptSet>,
}

/// 必备模板清单（manifest）的**当前版本**。
///
/// **往 [`REQUIRED_TEMPLATES`] 加任何一个键，都必须把它 +1，并把新键的 `since` 写成新值**
/// （单测 `manifest_keys_are_monotonic_and_checked_in` 钉住：`since` 不得超过本值，且随包
/// `prompts/manifest.json` 必须与代码一致）。它回答的是「客户端这一版要哪些模板」——
/// 服务端发布校验按它要求齐备，客户端装载覆盖时按它判「这份目录齐备到第几版」
/// （[`PromptRegistry::install_overlay`]）。
pub const PROMPT_MANIFEST_VERSION: u32 = 12;

/// 每个域**必须**齐备的模板名，以及各自**自哪一版 manifest 起成为必备**（`since`）。
///
/// 它同时是三处的判据：随包资源的完整性断言、目录覆盖装载时的完整性检查、以及
/// 服务端发布校验。服务端消费的是由本清单导出的 `prompts/manifest.json`
/// （`cargo run -p luna-app --bin export-prompt-manifest`），不再手抄第二份——
/// 少一个 key 的后果是那一条提示词悄悄回落随包，而界面上什么都不会说。
///
/// `since` 的用途（ADR-050 修订 2026-09-04）：客户端升级后新增了必备键、而现网 Active
/// 目录还是按旧清单签的——此前这会让客户端把整份目录拒掉、全域回落随包，直到运营重发。
/// 现在客户端按 `since` 判这份目录**齐备到第几版**：旧键齐备即接受，比它新的键从随包取，
/// 并把「齐备到 v 几、哪几条回落」如实报出来。
pub const REQUIRED_TEMPLATES: &[(PromptScope, &[(&str, u32)])] = &[
    (
        PromptScope::Common,
        &[
            ("structure", 1),
            // 术语约束句（PRD-01-017）：四条翻译 / 润色路径共用的那一句。
            ("glossary_constraint", 2),
        ],
    ),
    (
        PromptScope::Translate,
        &[
            ("system", 1),
            ("user", 1),
            ("source_hint", 1),
            ("style", 1),
            ("preset_style.slack", 1),
            ("preset_style.email", 1),
            ("preset_style.polish_en", 1),
            ("voice_instruction", 1),
            // 两条**按服务档分开**的上下文指令（ADR-083 ⑦，对照表在 TDD-03 §3.3）。
            // 它们此前是 luna-jobqueue::realtime_translate 里的一句 Rust 字面量，且实时
            // 与批量共用——于是停录后号称「补齐权威双语」的那一遍，是拿「这是一句直播
            // 口语、请简洁」的口径译出来的，而后文永远不可见。
            ("live_context", 1),
            ("document_context", 1),
        ],
    ),
    (PromptScope::Summarize, &[("system", 1)]),
    (
        PromptScope::Refine,
        &[
            ("system", 1),
            ("default_rule", 1),
            ("voice_instruction", 1),
            ("voice_style.general", 1),
            ("voice_style.email", 1),
            ("voice_style.slack", 1),
            ("voice_style.code_comment", 1),
            // 弹窗同语润色（固定语言模板选中同语文本时，TDD-03 §3.3）。
            ("popup_rule", 2),
            ("popup_style", 2),
        ],
    ),
    (
        PromptScope::Chat,
        &[
            ("role.chat_general", 1),
            ("role.chat_translator", 1),
            ("role.chat_coder", 1),
            ("role.chat_summarizer", 1),
        ],
    ),
    // ── 以下为任务域（v2 起，ADR-050 修订 2026-09-05）：此前全是 Rust 里的 format! 字面量。──
    (
        PromptScope::Editor,
        &[
            ("lang_lock.named", 2),
            ("lang_lock.same", 2),
            ("language_lock", 2),
            ("optimize", 2),
            ("proofread", 2),
            ("tone", 2),
            ("tone.formal", 2),
            ("tone.casual", 2),
            ("tone.concise", 2),
            ("continue", 2),
            ("continue_lang.named", 2),
            ("continue_lang.same", 2),
            ("context.intro", 2),
            ("context.before", 2),
            ("context.after", 2),
            ("summary.bullets", 2),
            ("summary.paragraph", 2),
            ("summary.one_line", 2),
            ("meeting_minutes", 2),
            ("ask.cursor", 2),
            ("ask.selection", 2),
            ("chunk_digest", 2),
            ("explain_change", 2),
            ("explain_lang.named", 2),
            ("explain_lang.same", 2),
            ("explain_what.insert", 2),
            ("explain_what.remove", 2),
            ("explain_what.change", 2),
            // v4：表格 AI 动作（对标方案 T6，PRD-06-011 2026-10-03）。
            ("table.from_text", 4),
            ("table.explain", 4),
            ("table.fill_cell", 4),
        ],
    ),
    (
        PromptScope::Distill,
        &[
            ("instruction", 2),
            ("preamble.conversation", 2),
            ("preamble.transcript", 2),
            ("preamble.attribution", 2),
            ("preamble.written_notes", 2),
            ("known_facts", 2),
            ("known_facts.action_items", 2),
            ("known_facts.dates", 2),
            ("chunk", 2),
            ("rewrite", 2),
            ("merge", 2),
        ],
    ),
    (PromptScope::Knowledge, &[("grounding", 2)]),
    (
        PromptScope::Tags,
        &[("system", 2), ("vocabulary", 2), ("language", 2)],
    ),
    (
        PromptScope::Ingest,
        &[("topic_generation", 3), ("topic_update", 3)],
    ),
    (PromptScope::Checkup, &[("verdict", 2), ("material", 2)]),
    (PromptScope::QuickNote, &[("refine", 2)]),
];

/// 一方应用**自带**的提示词（ADR-166 ⑤）：`(应用 id, 域, 模板名)`。
///
/// 与 [`REQUIRED_TEMPLATES`]（核心必备）分开：它们随应用代码一起随包发布、离线恒在，服务端
/// 目录**可以**覆盖（manifest 里登记，发布校验认得这些名字），但**不要求**——目录里没有就
/// 用随包那份，既不拖低「齐备到第几版」，也不报成「回落随包」。名字一律以应用 id 开头
/// （`creator.`），一眼看出归谁；应用 id 与 `luna_config::apps` 的清单由装配层的测试钉住。
pub const APP_TEMPLATES: &[(&str, PromptScope, &[&str])] = &[
    (
        "creator",
        PromptScope::Summarize,
        &[
            "creator.publish.chapters",
            "creator.swipe.patterns",
            "creator.research.guide",
            "creator.research.brief",
            "creator.research.topics",
            "creator.research.script",
            "creator.distribute.blog",
            "creator.distribute.newsletter",
            "creator.distribute.thread",
            "creator.distribute.shorts",
            "creator.publish.package",
            "creator.publish.package.limit.title",
            "creator.publish.package.limit.description",
            "creator.publish.package.limit.tags",
            "creator.publish.package.limit.tag",
            "creator.publish.package.limit.none",
            "creator.publish.package.style.none",
            "creator.publish.package.style.youtube",
            "creator.publish.package.style.bilibili",
            "creator.distribute.channel.none",
            "creator.distribute.limit.title",
            "creator.distribute.limit.body",
            "creator.distribute.limit.post",
            "creator.distribute.limit.clip",
            "creator.distribute.limit.none",
            "creator.distribute.style.none",
            "creator.distribute.style.x",
            "creator.distribute.style.weibo",
            "creator.distribute.style.linkedin",
            "creator.distribute.style.xiaohongshu_note",
            "creator.distribute.style.wechat_article",
            "creator.distribute.style.youtube_shorts",
            "creator.distribute.style.tiktok",
            "creator.distribute.style.instagram_reels",
            "creator.distribute.style.douyin",
            "creator.distribute.style.kuaishou",
            "creator.distribute.style.weixin_channels",
            "creator.distribute.style.xiaohongshu_video",
            "creator.notes.voice",
            "creator.notes.extra",
        ],
    ),
    (
        "creator",
        PromptScope::Editor,
        &[
            "creator.script.tighten",
            "creator.script.hook",
            "creator.script.spoken",
            "creator.script.dictation",
        ],
    ),
    // 提词器的 AI 口播动作（ADR-176 ⑧，PRD-20-016）：改成口播稿、按时长压缩、标注停顿与强调。
    (
        "teleprompter",
        PromptScope::Editor,
        &[
            "teleprompter.spoken",
            "teleprompter.fit",
            "teleprompter.cues",
        ],
    ),
];

/// 随包 manifest（由 `export-prompt-manifest` 生成；单测钉住与代码一致——运行期不读它，
/// 事实源永远是 [`REQUIRED_TEMPLATES`]，JSON 只是给服务端与门禁看的投影）。
#[cfg(test)]
const MANIFEST_JSON: &str = include_str!("../prompts/manifest.json");

/// 必备模板清单的线格式：服务端发布校验与管理台读的就是它（跨仓契约，ADR-050）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptManifest {
    /// = [`PROMPT_MANIFEST_VERSION`]。
    pub manifest_version: u32,
    pub scopes: Vec<PromptManifestScope>,
    /// 一方应用自带、可选覆盖的模板（[`APP_TEMPLATES`]）：发布校验认得这些名字，但不要求。
    pub apps: Vec<PromptManifestApp>,
}

/// 一个应用在一个域里自带的模板名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptManifestApp {
    pub app: String,
    pub scope: PromptScope,
    pub templates: Vec<String>,
}

/// 一个域的必备模板。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptManifestScope {
    pub scope: PromptScope,
    pub templates: Vec<PromptManifestTemplate>,
}

/// 一条必备模板：名字 + 自哪一版起必备。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptManifestTemplate {
    pub name: String,
    pub since: u32,
}

/// 由 [`REQUIRED_TEMPLATES`] 导出的 manifest（唯一事实源在代码，JSON 是它的投影）。
#[must_use]
pub fn manifest() -> PromptManifest {
    PromptManifest {
        manifest_version: PROMPT_MANIFEST_VERSION,
        scopes: REQUIRED_TEMPLATES
            .iter()
            .map(|(scope, keys)| PromptManifestScope {
                scope: *scope,
                templates: keys
                    .iter()
                    .map(|(name, since)| PromptManifestTemplate {
                        name: (*name).to_owned(),
                        since: *since,
                    })
                    .collect(),
            })
            .collect(),
        apps: APP_TEMPLATES
            .iter()
            .map(|(app, scope, keys)| PromptManifestApp {
                app: (*app).to_owned(),
                scope: *scope,
                templates: keys.iter().map(|name| (*name).to_owned()).collect(),
            })
            .collect(),
    }
}

/// 由注册表渲染出来的**整句指令**（ADR-050 修订 2026-09-05 ②：编译期约束）。
///
/// `JobKind` 的 `instruction` 字段只收它，而它在本 crate 之外**没有**从 `String` 构造的路——
/// 于是「就地写一句字符串塞进 Job」从此不是「门禁判红」而是**编译错误**。此前那道门禁
/// 认的是 19 句指纹，新写一处不会红；类型比指纹稳。
///
/// 构造口只有三个：[`PromptRegistry::instruction`]（渲染一条模板）、
/// [`crate::with_glossary_constraint`]（在既有指令后追加术语约束，同 crate 内）、
/// [`Instruction::raw`]（只准测试与 wire 反序列化用，门禁按调用点位置判红）。
///
/// `Deref<Target = str>`：读侧（Provider 组装、断言）像用 `&str` 一样用它；写侧没有
/// `DerefMut`，也没有 `From<String>`。`serde(transparent)`：线格式与 `String` 逐字相同。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Instruction(String);

impl Instruction {
    /// 指令正文。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 交出正文（Provider 把它作为 system 消息发出去时用）。
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// **绕过注册表的唯一口子——只准测试用**（`scripts/check-prompt-registry-single-source.py`
    /// 按调用点是否在 `#[cfg(test)]` 之后 / `tests/` 目录下判红）。生产代码要一句新指令，
    /// 唯一正确的做法是往 `bundled.json` 加一条模板、登记进 `REQUIRED_TEMPLATES`。
    #[doc(hidden)]
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// 同 crate 内的追加口（术语约束）：不对外暴露 `&mut String`。
    pub(crate) fn append_sentence(&mut self, sentence: &str) {
        if sentence.is_empty() {
            return;
        }
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0.push_str(sentence);
    }
}

impl std::ops::Deref for Instruction {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 装载覆盖时被拒的原因（如实回给状态展示，不静默吞掉）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOverlayRejected {
    /// 某个域的默认变体连 **v1 清单**里的必备模板都缺。这份目录不是「按旧清单签的」，
    /// 而是根本不齐备——半份覆盖会让一半提示词来自新版、一半来自随包，而
    /// `prompt_version` 只报得出一个数，账本从此说不清那次调用到底用的什么。
    MissingTemplate {
        scope: PromptScope,
        template: String,
    },
}

/// 一份覆盖被接受后的事实（如实回给通知与状态展示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOverlayInstalled {
    /// 目录版本。
    pub version: u32,
    /// 这份目录齐备到的 manifest 版本（≤ [`PROMPT_MANIFEST_VERSION`]）。
    pub manifest_version: u32,
    /// 比 `manifest_version` 更新的必备键（`scope.name`），它们从随包默认取。
    /// 空 = 这份目录对当前客户端完全齐备。
    pub missing: Vec<String>,
}

type SetIndex = BTreeMap<(PromptScope, Option<String>), BTreeMap<String, String>>;

fn index(sets: Vec<CapabilityPromptSet>) -> SetIndex {
    sets.into_iter()
        .map(|set| ((set.scope, set.locale), set.templates))
        .collect()
}

/// 这份索引的默认变体**齐备到第几版 manifest**：返回最大的 `v` 使得所有 `since <= v`
/// 的必备键都在；`0` = 连 v1 都不齐。局部 locale 变体允许只覆盖一部分，故不检查它们。
///
/// 抽成纯函数、清单作参数，以便独立验证多版本清单的前缀齐备性。
fn complete_upto(
    index: &SetIndex,
    requirements: &[(PromptScope, &[(&str, u32)])],
    current: u32,
) -> u32 {
    let present = |scope: PromptScope, key: &str| {
        index
            .get(&(scope, None))
            .is_some_and(|t| t.contains_key(key))
    };
    (1..=current)
        .take_while(|v| {
            requirements.iter().all(|(scope, keys)| {
                keys.iter()
                    .filter(|(_, since)| since <= v)
                    .all(|(key, _)| present(*scope, key))
            })
        })
        .last()
        .unwrap_or(0)
}

/// 目录默认变体真正缺失的必备键（`scope.name`，按清单顺序）。
/// 齐备版本只是前缀下界；较新版本的键可能部分在场，不能把它们全部报成缺失。
fn missing_keys(index: &SetIndex, requirements: &[(PromptScope, &[(&str, u32)])]) -> Vec<String> {
    requirements
        .iter()
        .flat_map(|(scope, keys)| {
            keys.iter()
                .filter(move |(key, _)| {
                    !index
                        .get(&(*scope, None))
                        .is_some_and(|templates| templates.contains_key(*key))
                })
                .map(move |(key, _)| format!("{}.{key}", scope.as_str()))
        })
        .collect()
}

/// 第一个缺失的 v1 必备键（用于拒绝原因；调用方已确认 `complete_upto == 0`）。
fn first_missing_v1(index: &SetIndex) -> PromptOverlayRejected {
    for (scope, keys) in REQUIRED_TEMPLATES {
        let templates = index.get(&(*scope, None));
        for (key, since) in *keys {
            if *since == 1 && !templates.is_some_and(|t| t.contains_key(*key)) {
                return PromptOverlayRejected::MissingTemplate {
                    scope: *scope,
                    template: (*key).to_owned(),
                };
            }
        }
    }
    // `complete_upto == 0` 蕴含至少一个 v1 键缺失；走到这里是调用方违约。
    unreachable!("complete_upto 报 0 却找不到缺失的 v1 键")
}

struct Overlay {
    version: u32,
    /// 这份覆盖来自哪一份灰度变体；`None` = 对照组。
    variant: Option<String>,
    manifest_version: u32,
    missing: Vec<String>,
    index: SetIndex,
}

/// 一次合成用的提示词身份（进用量账本的两个归因键，ADR-050 修订 ④）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptStamp {
    /// `0` = 随包默认，≥1 = 已验签目录版本。
    pub version: u32,
    /// 灰度变体 id；`None` = 对照组或随包默认。
    pub variant: Option<String>,
}

/// 注册表此刻的事实（设置页与通知读它；零 IO）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRegistryStatus {
    /// 当前生效的提示词版本；`0` = 随包默认。
    pub version: u32,
    /// 当前生效覆盖来自哪一份灰度变体（ADR-050 修订 ④）；`None` = 对照组或随包默认。
    pub variant: Option<String>,
    /// 已验签覆盖齐备到的 manifest 版本；`None` = 没有覆盖在场。
    pub manifest_version: Option<u32>,
    /// 本客户端的 manifest 版本（[`PROMPT_MANIFEST_VERSION`]）。
    pub client_manifest_version: u32,
    /// 覆盖里缺、正从随包取的必备键（`scope.name`）。
    pub missing: Vec<String>,
}

/// 提示词目录：随包默认 ⊕ 已验签 Active 覆盖。
pub struct PromptRegistry {
    bundled: SetIndex,
    overlay: RwLock<Option<Overlay>>,
    locale: RwLock<Option<String>>,
}

impl PromptRegistry {
    /// 只含随包默认的注册表（严格本地 / 离线 / 测试都用它）。
    ///
    /// # Panics
    /// 随包资源解析失败或不齐备时 panic——那是**构建期就该被发现的**事实错误
    /// （单测 `bundled_resource_is_complete` 在 CI 上先于任何用户撞上它）。
    #[must_use]
    pub fn bundled() -> Self {
        let parsed: BundledPrompts = serde_json::from_str(BUNDLED_JSON)
            .expect("随包提示词资源解析失败——prompts/bundled.json 不是合法 JSON 或形状不符");
        let bundled = index(parsed.sets);
        assert_eq!(
            complete_upto(&bundled, REQUIRED_TEMPLATES, PROMPT_MANIFEST_VERSION),
            PROMPT_MANIFEST_VERSION,
            "随包提示词资源缺模板——四能力必须齐备才谈得上离线可用"
        );
        for (app, scope, keys) in APP_TEMPLATES {
            for key in *keys {
                assert!(
                    bundled
                        .get(&(*scope, None))
                        .is_some_and(|templates| templates.contains_key(*key)),
                    "随包提示词资源缺应用 {app} 自带的模板 {}.{key}",
                    scope.as_str()
                );
            }
        }
        Self {
            bundled,
            overlay: RwLock::new(None),
            locale: RwLock::new(None),
        }
    }

    /// 装入一份已验签目录下发的覆盖。
    ///
    /// 接受规则（ADR-050 修订 2026-09-04）：按 [`REQUIRED_TEMPLATES`] 的 `since` 判这份
    /// 目录**齐备到第几版 manifest**——v1 齐备即接受，比它新的必备键从随包默认取，并在
    /// 返回值里如实列出。此前是「少任何一个键整份拒绝」，于是客户端一升级、现网那份按
    /// 旧清单签的 Active 目录就被整份拒掉、全域回落随包，直到运营重发一版。
    ///
    /// # Errors
    /// 连 v1 清单都不齐备时**整份拒绝**并沿用当前状态（见 [`PromptOverlayRejected`]）。
    pub fn install_overlay(
        &self,
        version: u32,
        variant: Option<String>,
        sets: Vec<CapabilityPromptSet>,
    ) -> Result<PromptOverlayInstalled, PromptOverlayRejected> {
        let candidate = index(sets);
        let manifest_version =
            complete_upto(&candidate, REQUIRED_TEMPLATES, PROMPT_MANIFEST_VERSION);
        if manifest_version == 0 {
            return Err(first_missing_v1(&candidate));
        }
        let missing = missing_keys(&candidate, REQUIRED_TEMPLATES);
        *self
            .overlay
            .write()
            .expect("prompt overlay rwlock poisoned") = Some(Overlay {
            version,
            variant,
            manifest_version,
            missing: missing.clone(),
            index: candidate,
        });
        Ok(PromptOverlayInstalled {
            version,
            manifest_version,
            missing,
        })
    }

    /// 此刻的事实快照（版本 / 齐备到哪一版 / 哪些键回落随包）。
    #[must_use]
    pub fn status(&self) -> PromptRegistryStatus {
        let overlay = self.overlay.read().expect("prompt overlay rwlock poisoned");
        PromptRegistryStatus {
            version: overlay.as_ref().map_or(0, |o| o.version),
            variant: overlay.as_ref().and_then(|o| o.variant.clone()),
            manifest_version: overlay.as_ref().map(|o| o.manifest_version),
            client_manifest_version: PROMPT_MANIFEST_VERSION,
            missing: overlay
                .as_ref()
                .map(|o| o.missing.clone())
                .unwrap_or_default(),
        }
    }

    /// 丢弃覆盖，回到随包默认（换服务端 / 登出时调用）。
    pub fn clear_overlay(&self) {
        *self
            .overlay
            .write()
            .expect("prompt overlay rwlock poisoned") = None;
    }

    /// 当前界面语言（灰度受众规则按它判语言）。
    #[must_use]
    pub fn locale(&self) -> Option<String> {
        self.locale
            .read()
            .expect("prompt locale rwlock poisoned")
            .clone()
    }

    /// 设置当前界面语言（决定 locale 变体的取用）。`None` = 只用默认变体。
    pub fn set_locale(&self, locale: Option<String>) {
        *self.locale.write().expect("prompt locale rwlock poisoned") =
            locale.filter(|l| !l.is_empty());
    }

    /// 当前生效的提示词版本；`0` = 随包默认。
    #[must_use]
    pub fn version(&self) -> u32 {
        self.stamp().version
    }

    /// 此刻生效的提示词身份：版本 + 灰度变体，**一次读锁**取齐。
    ///
    /// 合成点必须用它而不是分别问 [`Self::version`] 与变体：目录可能在两次调用之间
    /// 刷新，分两次问拿到的会是「上一版的版本号 + 这一版的变体」——一条永远对不上账的记录。
    #[must_use]
    pub fn stamp(&self) -> PromptStamp {
        let overlay = self.overlay.read().expect("prompt overlay rwlock poisoned");
        PromptStamp {
            version: overlay.as_ref().map_or(0, |o| o.version),
            variant: overlay.as_ref().and_then(|o| o.variant.clone()),
        }
    }

    /// 取一条模板原文（三层解析）。缺失返回 `None`——调用方**不得**就地编一句代替。
    #[must_use]
    pub fn template(&self, scope: PromptScope, key: &str) -> Option<String> {
        let locale = self
            .locale
            .read()
            .expect("prompt locale rwlock poisoned")
            .clone();
        self.template_for_locale(scope, key, locale.as_deref())
    }

    /// 同 [`Self::template`]，但**显式指定** locale 变体而不是用注册表当前的界面语言。
    ///
    /// 给那些「输出语言不等于界面语言」的调用方：标签 / 归入 / 体检的语言跟随界面，
    /// 但它们的单测要在英文进程里造中文样本——不给这条口子，测试就得去改全局状态。
    #[must_use]
    pub fn template_for_locale(
        &self,
        scope: PromptScope,
        key: &str,
        locale: Option<&str>,
    ) -> Option<String> {
        let overlay = self.overlay.read().expect("prompt overlay rwlock poisoned");
        let overlay_index = overlay.as_ref().map(|o| &o.index);
        for source in [overlay_index, Some(&self.bundled)].into_iter().flatten() {
            if let Some(l) = locale {
                if let Some(found) = source
                    .get(&(scope, Some(l.to_owned())))
                    .and_then(|t| t.get(key))
                {
                    return Some(found.clone());
                }
            }
            if let Some(found) = source.get(&(scope, None)).and_then(|t| t.get(key)) {
                return Some(found.clone());
            }
        }
        None
    }

    /// 取模板并渲染。模板缺失时返回空串——**并且这不会静默发生**：
    /// 每个 key 都由 [`REQUIRED_TEMPLATES`] 与随包完整性断言钉住，走到这一支
    /// 只可能是有人加了新 key 却没登记进清单，而那会被单测抓住。
    #[must_use]
    pub fn render(&self, scope: PromptScope, key: &str, vars: &[(&str, &str)]) -> String {
        self.template(scope, key)
            .map(|t| render(&t, vars))
            .unwrap_or_default()
    }

    /// 同 [`Self::render`]，但显式指定 locale 变体（见 [`Self::template_for_locale`]）。
    #[must_use]
    pub fn render_for_locale(
        &self,
        scope: PromptScope,
        key: &str,
        locale: Option<&str>,
        vars: &[(&str, &str)],
    ) -> String {
        self.template_for_locale(scope, key, locale)
            .map(|t| render(&t, vars))
            .unwrap_or_default()
    }

    /// 渲染一条模板为**整句指令**（[`Instruction`] 的正规构造口）。
    ///
    /// 与 [`Self::render`] 的分工：`render` 产片段（语言子句、风格句、上下文条款），片段作为
    /// 变量喂给最外层那条模板；最外层那一次用本函数，产物才能进 `JobKind`。
    #[must_use]
    pub fn instruction(&self, scope: PromptScope, key: &str, vars: &[(&str, &str)]) -> Instruction {
        Instruction(self.render(scope, key, vars))
    }

    /// 在一句既有指令后追加一条**已登记**模板渲染出的句子（ADR-166 ⑤：创作者写的频道风格与
    /// 「这次的额外要求」经 `{text}` 作为内容带进去，不改模板结构）。模板缺失或渲染为空时什么都
    /// 不加。单趟渲染：变量里的花括号不会被二次展开。
    pub fn append(
        &self,
        instruction: &mut Instruction,
        scope: PromptScope,
        key: &str,
        vars: &[(&str, &str)],
    ) {
        instruction.append_sentence(&self.render(scope, key, vars));
    }

    /// 同 [`Self::instruction`]，显式指定 locale 变体。
    #[must_use]
    pub fn instruction_for_locale(
        &self,
        scope: PromptScope,
        key: &str,
        locale: Option<&str>,
        vars: &[(&str, &str)],
    ) -> Instruction {
        Instruction(self.render_for_locale(scope, key, locale, vars))
    }
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self::bundled()
    }
}

/// 进程级注册表。
///
/// **提示词目录是进程级产品配置，不是逐 Provider 的状态**：同一进程里出现两份不同
/// 版本的模板，`prompt_version` 就不再是「这次调用用的哪一版」的答案，而计量按它分组的
/// 全部价值就没了。故装配侧只有这一个实例；纯逻辑一律接受 `&PromptRegistry` 参数，
/// 于是单测各建各的实例、互不干扰（全局只被装配层与目录观察者碰）。
#[must_use]
pub fn registry() -> &'static PromptRegistry {
    static GLOBAL: OnceLock<PromptRegistry> = OnceLock::new();
    GLOBAL.get_or_init(PromptRegistry::bundled)
}

/// 占位符渲染：把 `{name}` 换成 `vars` 里的值。
///
/// **单趟扫描，替换进去的值不再被重新扫描**。用一串 `str::replace` 实现的话，
/// 用户正文里的 `{text}` 会在后一轮里被当成占位符——那是一条从用户内容通到提示词
/// 结构的注入路径，而它平时完全看不出来。未登记的占位符**原样保留**（渲染结果里
/// 出现 `{foo}` 是一个刺眼的信号，胜过静默吞掉）。
#[must_use]
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            // 没有闭合花括号：剩下的全是字面量。
            None => {
                out.push('{');
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 模板里出现的全部占位符名（按出现顺序去重）。评测脚本与单测用它断言
/// 「模板要的变量」与「组装点给的变量」是同一组。
#[must_use]
pub fn placeholders(template: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty() && !found.iter().any(|f| f == name) {
            found.push(name.to_owned());
        }
        rest = &after[end + 1..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(
        scope: PromptScope,
        locale: Option<&str>,
        pairs: &[(&str, &str)],
    ) -> CapabilityPromptSet {
        CapabilityPromptSet {
            scope,
            locale: locale.map(ToOwned::to_owned),
            templates: pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    /// 一份齐备的覆盖（在随包基础上只改需要改的，其余照抄随包，模拟真实发布）。
    fn complete_overlay(extra: Vec<CapabilityPromptSet>) -> Vec<CapabilityPromptSet> {
        let bundled = PromptRegistry::bundled();
        let mut sets: Vec<CapabilityPromptSet> = REQUIRED_TEMPLATES
            .iter()
            .map(|(scope, keys)| CapabilityPromptSet {
                scope: *scope,
                locale: None,
                templates: keys
                    .iter()
                    .map(|(key, _)| {
                        let body = bundled.template(*scope, key).expect("随包必须有这条模板");
                        ((*key).to_owned(), body)
                    })
                    .collect(),
            })
            .collect();
        for over in extra {
            if let Some(existing) = sets
                .iter_mut()
                .find(|s| s.scope == over.scope && s.locale == over.locale)
            {
                existing.templates.extend(over.templates);
            } else {
                sets.push(over);
            }
        }
        sets
    }

    #[test]
    fn bundled_resource_is_complete_and_offline() {
        let registry = PromptRegistry::bundled();
        assert_eq!(registry.version(), 0, "随包默认恒为版本 0");
        assert_eq!(
            registry.status(),
            PromptRegistryStatus {
                version: 0,
                variant: None,
                manifest_version: None,
                client_manifest_version: PROMPT_MANIFEST_VERSION,
                missing: Vec::new(),
            }
        );
        for (scope, keys) in REQUIRED_TEMPLATES {
            for (key, _) in *keys {
                let template = registry.template(*scope, key);
                assert!(
                    template.as_deref().is_some_and(|t| !t.trim().is_empty()),
                    "随包缺模板 {}.{key}——离线用户会拿到一句空提示词",
                    scope.as_str()
                );
            }
        }
    }

    /// manifest 是跨仓契约：`since` 不得超过当前版本、不得为 0，且随包 `manifest.json`
    /// 必须与代码逐字一致（服务端读的是那份 JSON；两边分叉的表现是「服务端签得出、
    /// 客户端拒收」）。改了 `REQUIRED_TEMPLATES` 就重跑 `export-prompt-manifest`。
    #[test]
    fn manifest_keys_are_monotonic_and_checked_in() {
        for (scope, keys) in REQUIRED_TEMPLATES {
            for (key, since) in *keys {
                assert!(
                    (1..=PROMPT_MANIFEST_VERSION).contains(since),
                    "{}.{key} 的 since={since} 越出 1..={PROMPT_MANIFEST_VERSION}",
                    scope.as_str()
                );
            }
        }
        let checked_in: PromptManifest =
            serde_json::from_str(MANIFEST_JSON).expect("prompts/manifest.json 不是合法 manifest");
        assert_eq!(
            checked_in,
            manifest(),
            "prompts/manifest.json 与 REQUIRED_TEMPLATES 不一致——重跑 \
             `cargo run -p luna-app --bin export-prompt-manifest > crates/luna-core/prompts/manifest.json`"
        );
    }

    /// **按旧清单签的目录不再被整份拒掉**：v1 键齐备就接受，比它新的键回落随包并如实
    /// 报出；连 v1 都不齐才拒绝。用一份三版清单验证前缀性质。
    #[test]
    fn overlay_signed_against_an_older_manifest_is_accepted_up_to_that_version() {
        let fake: &[(PromptScope, &[(&str, u32)])] = &[
            (PromptScope::Common, &[("structure", 1)]),
            (
                PromptScope::Summarize,
                &[("system", 1), ("shape.bullets", 2), ("shape.paragraph", 3)],
            ),
        ];
        let v1_only = index(vec![
            set(PromptScope::Common, None, &[("structure", "s")]),
            set(PromptScope::Summarize, None, &[("system", "x")]),
        ]);
        assert_eq!(complete_upto(&v1_only, fake, 3), 1);
        assert_eq!(
            missing_keys(&v1_only, fake),
            vec![
                "summarize.shape.bullets".to_owned(),
                "summarize.shape.paragraph".to_owned()
            ]
        );

        let v2 = index(vec![
            set(PromptScope::Common, None, &[("structure", "s")]),
            set(
                PromptScope::Summarize,
                None,
                &[("system", "x"), ("shape.bullets", "b")],
            ),
        ]);
        assert_eq!(complete_upto(&v2, fake, 3), 2);
        assert_eq!(
            missing_keys(&v2, fake),
            vec!["summarize.shape.paragraph".to_owned()]
        );

        // 缺 v1 键 → 0，哪怕 v2 的键都在（齐备是前缀性质，不是计数）。
        let missing_v1 = index(vec![set(
            PromptScope::Summarize,
            None,
            &[
                ("system", "x"),
                ("shape.bullets", "b"),
                ("shape.paragraph", "p"),
            ],
        )]);
        assert_eq!(complete_upto(&missing_v1, fake, 3), 0);
        assert_eq!(missing_keys(&missing_v1, fake), vec!["common.structure"]);
    }

    #[test]
    fn v1_overlay_keeps_published_templates_and_uses_bundled_for_new_domains() {
        let registry = PromptRegistry::bundled();
        let fallback = registry.template(PromptScope::QuickNote, "refine").unwrap();
        let mut sets = complete_overlay(vec![set(
            PromptScope::Translate,
            None,
            &[("system", "published translation")],
        )]);
        for set in &mut sets {
            let (_, requirements) = REQUIRED_TEMPLATES
                .iter()
                .find(|(scope, _)| *scope == set.scope)
                .unwrap();
            set.templates.retain(|key, _| {
                requirements
                    .iter()
                    .any(|(name, since)| *name == key && *since == 1)
            });
        }
        sets.retain(|set| !set.templates.is_empty());
        let installed = registry.install_overlay(6, None, sets).unwrap();
        assert_eq!(installed.manifest_version, 1);
        assert_eq!(
            installed.missing.len(),
            REQUIRED_TEMPLATES
                .iter()
                .flat_map(|(_, keys)| *keys)
                .filter(|(_, since)| *since > 1)
                .count()
        );
        assert_eq!(
            registry
                .template(PromptScope::Translate, "system")
                .as_deref(),
            Some("published translation")
        );
        assert_eq!(
            registry.template(PromptScope::QuickNote, "refine"),
            Some(fallback)
        );
    }

    #[test]
    fn partially_complete_manifest_reports_only_templates_actually_missing() {
        let registry = PromptRegistry::bundled();
        let fallback = registry.template(PromptScope::Checkup, "verdict").unwrap();
        let mut sets = complete_overlay(vec![set(
            PromptScope::QuickNote,
            None,
            &[("refine", "published quick note")],
        )]);
        sets.iter_mut()
            .find(|set| set.scope == PromptScope::Checkup && set.locale.is_none())
            .unwrap()
            .templates
            .remove("verdict");

        let installed = registry.install_overlay(7, None, sets).unwrap();
        assert_eq!(installed.manifest_version, 1);
        assert_eq!(installed.missing, vec!["checkup.verdict"]);
        assert_eq!(registry.status().missing, installed.missing);
        assert_eq!(
            registry
                .template(PromptScope::QuickNote, "refine")
                .as_deref(),
            Some("published quick note")
        );
        assert_eq!(
            registry.template(PromptScope::Checkup, "verdict"),
            Some(fallback)
        );

        let installed = registry
            .install_overlay(8, None, complete_overlay(Vec::new()))
            .unwrap();
        assert_eq!(installed.manifest_version, PROMPT_MANIFEST_VERSION);
        assert!(installed.missing.is_empty());
        assert!(registry.status().missing.is_empty());
    }

    /// **覆盖优先级**：已验签目录压过随包，且只压它真的带了的那一条。
    #[test]
    fn overlay_beats_bundled_per_template() {
        let registry = PromptRegistry::bundled();
        let bundled_user = registry
            .template(PromptScope::Translate, "user")
            .expect("随包有 user 模板");

        let installed = registry
            .install_overlay(
                7,
                Some("b".to_owned()),
                complete_overlay(vec![set(
                    PromptScope::Translate,
                    None,
                    &[("system", "OVERLAID {target_lang}")],
                )]),
            )
            .expect("齐备的覆盖必须被接受");
        assert_eq!(
            installed,
            PromptOverlayInstalled {
                version: 7,
                manifest_version: PROMPT_MANIFEST_VERSION,
                missing: Vec::new(),
            }
        );
        assert_eq!(
            registry.status().manifest_version,
            Some(PROMPT_MANIFEST_VERSION)
        );

        assert_eq!(registry.version(), 7);
        assert_eq!(
            registry.stamp(),
            PromptStamp {
                version: 7,
                variant: Some("b".to_owned())
            },
            "版本与变体必须一次取齐——账本按这两个键归因"
        );
        assert_eq!(registry.status().variant.as_deref(), Some("b"));
        assert_eq!(
            registry
                .template(PromptScope::Translate, "system")
                .as_deref(),
            Some("OVERLAID {target_lang}")
        );
        assert_eq!(
            registry.template(PromptScope::Translate, "user"),
            Some(bundled_user.clone()),
            "覆盖没改的那条必须原样沿用随包"
        );

        // 丢弃覆盖 = 回到随包（换服务端 / 登出）。
        registry.clear_overlay();
        assert_eq!(registry.version(), 0);
        assert_ne!(
            registry
                .template(PromptScope::Translate, "system")
                .as_deref(),
            Some("OVERLAID {target_lang}")
        );
    }

    /// **不齐备的覆盖整份拒绝**：半份覆盖会让一半提示词来自新版、一半来自随包，
    /// 而 `prompt_version` 只报得出一个数——账本从此说不清那次调用到底用的什么。
    #[test]
    fn incomplete_overlay_is_rejected_wholesale() {
        let registry = PromptRegistry::bundled();
        let err = registry
            .install_overlay(
                9,
                None,
                vec![set(PromptScope::Translate, None, &[("system", "only one")])],
            )
            .expect_err("缺模板的覆盖必须被拒");
        assert!(matches!(err, PromptOverlayRejected::MissingTemplate { .. }));
        assert_eq!(registry.version(), 0, "被拒之后必须仍是随包默认");
    }

    /// locale 变体只压它自己带的那几条，其余落回同一来源的默认变体。
    #[test]
    fn locale_variant_overlays_only_what_it_carries() {
        let registry = PromptRegistry::bundled();
        registry
            .install_overlay(
                3,
                None,
                complete_overlay(vec![set(
                    PromptScope::Summarize,
                    Some("zh-CN"),
                    &[("system", "用中文写摘要：{instruction}")],
                )]),
            )
            .expect("齐备的覆盖必须被接受");

        registry.set_locale(Some("zh-CN".to_owned()));
        assert_eq!(
            registry
                .template(PromptScope::Summarize, "system")
                .as_deref(),
            Some("用中文写摘要：{instruction}")
        );
        registry.set_locale(Some("en-US".to_owned()));
        assert!(
            registry
                .template(PromptScope::Summarize, "system")
                .is_some_and(|t| t.starts_with("Summarize")),
            "没有 en-US 变体时落回默认变体"
        );
    }

    /// 渲染是单趟的：用户正文里的花括号不得成为占位符。
    #[test]
    fn rendering_never_rescans_substituted_values() {
        let rendered = render("A {x} B {y}", &[("x", "{y}"), ("y", "Z")]);
        assert_eq!(
            rendered, "A {y} B Z",
            "替换进去的 `{{y}}` 被二次展开了——那是一条从用户正文通到提示词结构的注入路径"
        );
        assert_eq!(render("no vars", &[]), "no vars");
        assert_eq!(
            render("{unknown}", &[]),
            "{unknown}",
            "未登记占位符原样保留"
        );
        assert_eq!(render("dangling {", &[]), "dangling {");
    }

    #[test]
    fn placeholders_are_listed_in_order_without_duplicates() {
        assert_eq!(
            placeholders("{a} {b} {a}"),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert!(placeholders("none").is_empty());
    }

    #[test]
    fn scope_wire_values_round_trip_and_unknown_is_rejected() {
        for scope in PromptScope::ALL {
            assert_eq!(PromptScope::parse(scope.as_str()), Some(scope));
        }
        for bogus in ["", "sumarize", "Translate", "prompt"] {
            assert!(PromptScope::parse(bogus).is_none());
        }
    }
}
