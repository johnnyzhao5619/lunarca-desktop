//! Job 的统一描述（零 IO）。事实源：TDD-01 §2.1。
//!
//! 第一性：弹窗翻译 / 语音输入 / 录音转录 / 批量 / 悬浮菜单 / 二次处理的差异只在
//! **输入来源**与 **Job 类型**，执行路径同构——故统一为一个 `Job`（ADR-001）。

use crate::chat::{ChatMessage, ToolSpec};
use crate::ids::UploadId;
use crate::ids::{JobId, PresetId, ProviderId, StreamId};
use crate::lang::Lang;
use crate::prompt::Instruction;
use crate::time::Timestamp;
use crate::Capability;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 音频来源引用（统一供 Provider 消费，TDD-02 §4）。
///
/// 注：`Stream` 仅携带 `StreamId`——真正的采集句柄由 `luna-audio` 按 id 持有，
/// 以保持 `luna-core` 零 IO（不持有运行时句柄）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioRef {
    /// 磁盘文件（路径，不在此做 IO）。
    File(PathBuf),
    /// 内存 PCM 缓冲。
    Pcm(PcmBuffer),
    /// 流式采集会话（句柄在 luna-audio，按 id 关联）。
    Stream(StreamId),
    /// **已上传到计算服务**的音频（远程批式转写，ADR-036 ①）。
    ///
    /// 存在的理由是**不重传**：分片上传一段会议录音可能是几百 MB，而「重试一次转写」
    /// 与「应用重启后重新挂上进行中的任务」都必须能只凭一个 id 完成。没有这个变体时，
    /// 唯一的引用方式是本地路径，于是 Provider 得自己维护一张 路径→上传 id 的私有表——
    /// 那张表随进程消失，重启后用户看到的就是"又从 0% 开始传"。
    ///
    /// 仍然零 IO：只是一个 id，字节在服务端。
    Uploaded(UploadId),
}

/// 图像来源引用（OCR 输入；统一供 OcrProvider 消费，与 [`AudioRef`] 同构）。
///
/// 居 `luna-core` 保持零 IO（纯路径/字节，无运行时句柄）。本地 Vision 走 `Path`；
/// 未来云 OCR API 走 `Bytes` 时按需扩展（不预建，YAGNI，[P2 需求驱动]）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ImageRef {
    /// 磁盘图像文件（路径，不在此做 IO；由 Provider 读取解码）。
    Path(PathBuf),
}

/// 归一化后的 PCM 缓冲（纯数据）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PcmBuffer {
    /// 采样率（Hz）。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 交错样本（f32）。
    pub samples: Vec<f32>,
}

/// 流式 ASR 窗口输入（16k mono f32，由 `StreamSegmenter` 产出，喂给任一
/// [`crate::StreamingAsr`] 实现）。
///
/// 居 `luna-core` 保持零 IO（纯数据，无运行时句柄）；`luna-audio` 产出后经
/// [`crate::StreamRegistration`] 的通道投递，不跨 crate 引入运行时依赖。
///
/// # 窗是**重解语义**，不是增量流
///
/// 分段器为进程内 whisper 而设计：同一句话每 ~1.5 秒产一个**增长快照**，每个快照都
/// 携带该句从起点至今的**全部**语音，并补零 padding 到定长；句子被强切时下一窗还带
/// ~250 ms overlap 前缀。whisper 每次拿全量重解、由 LocalAgreement-2 取稳定前缀——
/// 这些重复对它是**必需的**。
///
/// 但对「把音频按顺序喂进去」的持续流消费者（远程实时通道，ADR-038），照单上行意味着
/// 一句 10 秒的话要传 ~70 秒的字节：计费按上行字节算 → 用户付 7 倍；连续明文上行量
/// 同样放大 7 倍（ADR-038 ④ 说这是唯一无法用工程手段消除的代价，放大它是在加重它）；
/// 上游还会把重发当成「同一句说了好几遍」。
///
/// 故本结构必须让消费者**分得出哪些样本是新的**——那正是 [`Self::speech_len`] 的用途。
/// 只带 `samples` + `abs_start` 时这个信息不在接缝里，第二个消费者就只能猜。
#[derive(Clone, Debug)]
pub struct WindowInput {
    /// 16k mono f32 样本：前 [`Self::speech_len`] 个是真实语音，其后是补零 padding。
    pub samples: Vec<f32>,
    /// 本窗真实语音的**会话绝对采样起点**（÷ 16000 = 绝对秒）。
    ///
    /// 同一句话的多个增长快照共享同一个 `abs_start`（它们覆盖的是同一段的不同长度）。
    pub abs_start: u64,
    /// `samples` 里真实语音的样本数（padding 之前）。
    ///
    /// 与 `abs_start` 合起来，本窗覆盖的绝对区间就是
    /// `[abs_start, abs_start + speech_len)`。持续流消费者据此维护一个「已上行到哪」的
    /// 游标，只发游标之后的部分——重叠、快照重发、乃至丢窗造成的空洞都由此自然收敛，
    /// 不需要任何一方去猜窗与窗的关系。
    pub speech_len: usize,
}

impl WindowInput {
    /// 本窗覆盖的绝对采样区间 `[start, end)`。
    #[must_use]
    pub fn span(&self) -> std::ops::Range<u64> {
        self.abs_start..self.abs_start + self.speech_len as u64
    }

    /// 相对 `sent_upto`（已消费到的绝对采样位置）**尚未消费**的那一段真实语音。
    ///
    /// 返回 `None` = 本窗全部内容都在游标之前（纯重发的快照 / overlap 前缀），
    /// 持续流消费者据此**一个字节都不发**。
    ///
    /// 只取 `samples[..speech_len]`：padding 的零绝不上行。它们是喂给 whisper 抑制
    /// 幻觉的，对持续流上游只是噪声——而且照样按字节计费。
    #[must_use]
    pub fn fresh_after(&self, sent_upto: u64) -> Option<&[f32]> {
        let span = self.span();
        if span.end <= sent_upto {
            return None;
        }
        let from = sent_upto.max(span.start) - span.start;
        let from = usize::try_from(from).ok()?;
        self.samples
            .get(from..self.speech_len)
            .filter(|s| !s.is_empty())
    }
}

/// 文本来源引用。MVP 先支持内联文本；笔记/选区锚定的引用后续扩展。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRef {
    /// 直接内联的文本。
    Inline(String),
}

/// Summarize Job 的产出模式（**决定 Provider 怎么用 `instruction`**）。
///
/// 每一档都有真实的构造方：曾经还有 `Brief` / `Detailed` 两个变体，但它们**从未被任何
/// 调用方构造过**，Provider 也从不读取——一个声称四种模式而只有两种能工作的类型是在
/// 说谎，故按「不留死代码」的既有纪律删除（`SummaryStyle` 不进任何持久化数据：用量记录
/// 只存 `capability`，Job 本身不落盘，故删除无兼容负担）。
///
/// **压缩式三档（要点 / 段落 / 一句话）必须是 Job 的事实，不能只藏在 `instruction`
/// 文本里**：此前悬浮菜单的段落 / 一句话总结仍以 `Bullets` 提交，账本、服务端与任何
/// 读 Job 形状的地方看到的都是「要点」——一个说谎的字段比没有更糟。三档对 Provider 的
/// 用法完全相同（都外套通用摘要前缀），差别只在调用方合成的 `instruction`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStyle {
    /// 压缩式要点：产出远短于输入，Provider 外套通用摘要前缀。
    ///
    /// 调用方常只给半句 `instruction`（如 `"Write the summary as bullet points in Chinese."`），
    /// 靠那层前缀撑起语义——故不能一刀切改成"有 instruction 就整句替换"。
    Bullets,
    /// 压缩式段落：一段连贯的散文，不用项目符号。Provider 用法同 [`Self::Bullets`]。
    Paragraph,
    /// 压缩式一句话。Provider 用法同 [`Self::Bullets`]。
    OneLine,
    /// **忠实改写（不是总结）**：`instruction` 即完整 system prompt，Provider **不得**
    /// 在外层套「Summarize the user's text concisely」这类前缀。
    ///
    /// 为何必须存在这一档：蒸馏的「整理稿」要求的是零压缩的保真改写，而 Provider 对
    /// Summarize 的通用前缀命令模型"简明总结、只输出摘要"——两个指令正面相冲，本地小
    /// 模型多半听外层的，于是整理稿被系统性地做成另一份摘要，**且没有任何报错**。
    /// 与 [`JobKind::ExtractTags`] 的「调用方决策、引擎只渲染」同一先例。
    Faithful,
}

/// 嵌入用途。Query 与 Document 的输入模板可能不同，调用方必须显式声明，禁止靠字符串前缀猜测。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPurpose {
    Query,
    Document,
}

impl SummaryStyle {
    /// 本档是否要求 Provider 原样使用 `instruction` 作 system prompt（不加任何前后缀）。
    #[must_use]
    pub fn uses_raw_instruction(self) -> bool {
        matches!(self, Self::Faithful)
    }
}

/// 承载某能力所需输入的 Job 类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// 语音 → 文本。
    Transcribe {
        /// 音频来源。
        audio: AudioRef,
        /// 语种提示（None=自动检测）。
        lang_hint: Option<Lang>,
        /// Whisper 初始提示词（个人词典/专有名词等识别提示；None=不用提示）。
        initial_prompt: Option<String>,
    },
    /// 跨语言翻译。
    Translate {
        /// 源文本。
        text: TextRef,
        /// 源语言（None=运行时检测）。
        source: Option<Lang>,
        /// 目标语言。
        target: Lang,
        /// 合成后的最终翻译指令（风格/场景描述等；在 Job 构造前合成，引擎只渲染不决策，
        /// TDD-03 §3.3）。`None` = 用 Provider 通用译文 prompt。与 `Refine.instruction` 对称。
        instruction: Option<Instruction>,
        /// 模板/预设。
        template: Option<PresetId>,
    },
    /// 文本总结。
    Summarize {
        /// 源文本。
        text: TextRef,
        /// 风格。
        style: SummaryStyle,
        /// 总结指令（语言/格式等；None=Provider 默认简短总结）。
        instruction: Option<Instruction>,
    },
    /// 文本优化/润色。
    Refine {
        /// 源文本。
        text: TextRef,
        /// 润色指令（None=默认"仅润色不增删信息"纪律）。
        instruction: Option<Instruction>,
    },
    /// 多轮对话。
    Chat {
        /// 轮次历史。
        messages: Vec<ChatMessage>,
        /// 工具声明。
        tools: Vec<ToolSpec>,
    },
    /// 图像 → 文本（OCR）。
    Ocr {
        /// 图像来源。
        image: ImageRef,
    },
    /// 说话人细分（音频 → 说话人区间；`ResultPayload::Segments`，段 text 为空、
    /// `speaker=Some("1"/"2"/…)`，调用方按时间重叠归属到转写段，TDD-03 §10.3）。
    Diarize {
        /// 音频来源（16k mono PCM；文件路径由调用方先解码）。
        audio: AudioRef,
        /// 会话语言（嵌入模型按语言选档，TDD-03 §10.2：en → wespeaker-EN；
        /// `None`/其余语种/缺档 → zh 基础档。来源与转写 `lang_hint` 同源）。
        lang: Option<Lang>,
    },
    /// 关系标签提取（PRD-06-035）：LLM 从笔记文本提炼主题标签，服务关系图谱/标签索引。
    /// 指令在 Job 构造前合成（含既有词表/输出契约；引擎只渲染不决策，TDD-03 §3.3 同哲学），
    /// 路由按 [`Capability::Summarize`]（把一篇正文压成几个主题词，就是总结的一种形状）。
    ExtractTags {
        /// 源文本（标题 + 正文，调用方裁剪）。
        text: TextRef,
        /// 合成后的完整提取指令（system prompt）。
        instruction: Instruction,
    },
    /// 文本 → 向量（嵌入，ADR-027 / [PRD-16-028]）。
    ///
    /// **一个 Job 一批**（S16 实测：批 8 即到吞吐平台，故批大小取 16——吞吐无损，而单请求
    /// 更短意味着取消更灵敏、进度更细）。返回 [`crate::ResultPayload::Embeddings`]，
    /// 顺序与 `texts` 严格一一对应；条数不符即调用方判失败（对不上号的向量比没有更糟）。
    ///
    /// **不承载分块**：怎么切、切在哪一行，是 `luna_vault::chunk` 的纯函数职责；
    /// 本 Job 只回答「这几段文字的向量是什么」。
    Embed {
        /// Query / Document 输入 profile 分流。
        purpose: EmbeddingPurpose,
        /// 待嵌入文本（调用方保证每段都在模型上下文窗内——超窗时随包引擎如实拒绝，
        /// 归 [`crate::JobError::InputTooLong`]，绝不截断后照常给出一个只代表前半段的向量）。
        texts: Vec<String>,
    },
    /// 模板驱动的任意文本任务。
    Custom {
        /// 模板/预设。
        template: PresetId,
        /// 模板变量。
        vars: BTreeMap<String, String>,
    },
}

impl JobKind {
    /// 该 Job 类型对应的能力（路由按此匹配 Provider）。
    ///
    /// # `Custom` 只留给 [`JobKind::Custom`]
    ///
    /// [`Capability::Custom`] 的含义是「任意模板/提示词驱动的文本任务」——它是**扩展点**，
    /// 没有任何服务端会声明自己提供它（服务端的 `Capability::SERVED` 刻意不含它，同
    /// `Ocr`/`Diarize`：那些是客户端本地引擎型能力）。把一个**产品内置**的具体任务挂在
    /// 它下面，等于宣布这个任务只有本机引擎能跑。
    ///
    /// 真机坐实（2026-08-11，ADR-059）：`ExtractTags` 此前路由按 `Custom`，于是连着组织
    /// 服务端时全库关系标签重扫**每一篇都路由不到 Provider**，连撞 3 篇即中止；而同屏
    /// 的 AI 对话（`Capability::Chat`）与知识蒸馏（`JobKind::Summarize`）一切正常——
    /// 「同一台服务端，有的 AI 能用有的不能用」这种指纹，用户报告不出来。
    #[must_use]
    pub fn capability(&self) -> Capability {
        match self {
            Self::Transcribe { .. } => Capability::Transcribe,
            Self::Translate { .. } => Capability::Translate,
            // 标签提取 = 把正文压成几个主题词，与总结同形（messages 进、文本出），
            // 故按 `Summarize` 路由：任何提供总结的落点都跑得了它。
            Self::Summarize { .. } | Self::ExtractTags { .. } => Capability::Summarize,
            Self::Refine { .. } => Capability::Refine,
            Self::Chat { .. } => Capability::Chat,
            Self::Ocr { .. } => Capability::Ocr,
            Self::Diarize { .. } => Capability::Diarize,
            Self::Embed { .. } => Capability::Embed,
            Self::Custom { .. } => Capability::Custom,
        }
    }
}

/// 任务来源：**这次调用是谁发起的**这一事实。TDD-01 §2.1/§5.8。
///
/// **它本身不是策略，但已有两处执行判据据它推导**——[`Job::admission_lane`]
/// （向哪个池要额度，ADR-078 ①）与
/// [`Job::service_class`]（要哪一档服务，ADR-083 ③）。
/// 本条头注此前写的是「仅用于归档去向与遥测，**不影响执行逻辑**」，那句话自 ADR-078 起
/// 就不成立。留着它的代价很具体：下一个人会以为加一个 origin 只是多一行遥测标签，而
/// 穷尽 match 会在两处同时编译失败，他会顺手补一个 `_ =>` 把新 origin 静默塞进某一档
/// ——那正是这两条判据用穷尽 match 要防的事。
///
/// **DRY 那一半仍然成立**，且理由更强：判据据 origin 推导、只有一份，比让每个提交点
/// 自己声明少一个会漏的事实源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOrigin {
    /// 弹窗翻译（全局功能）。
    PopupTranslate,
    /// AI 语音输入。
    VoiceInput,
    /// 快捷录音。
    Recording,
    /// 批量转录/转译。
    Batch,
    /// 笔记 AI 悬浮菜单。
    FloatingMenu,
    /// CLI。
    Cli,
    /// 二次高质量处理。
    SecondPass,
    /// 快捷 AI 对话（全局功能）。
    Chat,
    /// 截屏翻译的 OCR 环节（图像 → 文本，PRD-01-007）。
    Screenshot,
    /// 关系图谱 AI 标签生成（保存/手动/二次转写后触发，PRD-06-035）。
    GraphTags,
    /// 资料蒸馏为知识笔记（录音/转写/对话 → 纪要，PRD-06-044 / ADR-024）。
    /// 独立 origin 使用量记账能回答「知识笔记花了多少 token」这个用户会问的问题。
    Distill,
    /// AI 辅助沉淀（**记录** → 知识笔记，PRD-06-054 / ADR-086 ②）。
    ///
    /// **只新增 origin、不新增 `JobKind`**：沉淀跑的就是蒸馏那条管线（同一
    /// `Summarize` 主干、同一模板体系、同一溯源字段），与 ADR-024「蒸馏走同一 `Summarize`
    /// 主干」/ ADR-026 ④ 归入是同一裁决。
    ///
    /// **那为什么不干脆复用 `Distill`**：用量视图要能回答「整理资料花了多少 / 沉淀我自己
    /// 的草稿花了多少」——这是两笔用户分得清、也会分开问的账，而 origin 正是这本账唯一
    /// 的分账维度（[`crate::UsageRecord::origin`]）。执行侧两者完全一致（同一档服务、
    /// 同一个池），故这个新变体只影响记账，不影响调度。
    Settle,
    /// 「试跑一次」自检（ADR-126 第 8 条）：用户在算力页显式点下的一次最小真实任务。
    ///
    /// **独立 origin 而不是借用某个功能的**：它进同一本账（走的就是那条主干，ADR-001），
    /// 而账上必须分得出「这条是我自己点的自检」——否则用户会在执行记录里看到一条自己
    /// 从没发起过的翻译，并据此怀疑有东西在背地里跑。
    SelfCheck,
    /// 归入主题页（知识笔记 → 既有主题页的一条要点，PRD-16-018 / ADR-026 ④）。
    ///
    /// **只新增 origin、不新增 `JobKind`**：归入拆成的两个小调用（写一条要点 / 选一个
    /// 小标题）复用既有主干，与 ADR-024「蒸馏走同一 `Summarize` 主干」同一裁决；独立
    /// origin 只为回答「归入花了多少 token」这个用量归因问题。
    Ingest,
    /// 语义检索建库（笔记分片 → 向量，PRD-16-028 / ADR-027）。
    ///
    /// 独立 origin 的理由与 `Distill`/`Ingest` 同：建库是本地库里**最重的**一类批量任务
    /// （S16 实测万片约 40 分钟），用量视图里必须能把它与用户等着的那些任务分开看。
    Embed,
    /// 知识体检的深度档（主题页声明 vs 其引用的源，PRD-16-031）。
    ///
    /// 同样**只新增 origin、不新增 `JobKind`**：一次判定就是"读两段文字、给一个结论"，
    /// 复用既有 `Summarize` 主干（与归入两个小调用同一裁决）。独立 origin 只为回答
    /// 「体检花了多少 token」——它是用户显式发起的批量任务，值得单独一行。
    Checkup,
}

impl JobOrigin {
    /// 全部取值（供遍历与守护测试；新增变体必须同步这里，否则守护会漏掉它）。
    pub const ALL: [Self; 16] = [
        Self::PopupTranslate,
        Self::VoiceInput,
        Self::Recording,
        Self::Batch,
        Self::FloatingMenu,
        Self::Cli,
        Self::SecondPass,
        Self::Chat,
        Self::Screenshot,
        Self::GraphTags,
        Self::Distill,
        Self::Settle,
        Self::Ingest,
        Self::Embed,
        Self::Checkup,
        Self::SelfCheck,
    ];

    /// serde 的 snake_case 串（= 用量记录里落盘的那个键，也是前端 i18n 的查表键）。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::PopupTranslate => "popup_translate",
            Self::VoiceInput => "voice_input",
            Self::Recording => "recording",
            Self::Batch => "batch",
            Self::FloatingMenu => "floating_menu",
            Self::Cli => "cli",
            Self::SecondPass => "second_pass",
            Self::Chat => "chat",
            Self::Screenshot => "screenshot",
            Self::SelfCheck => "self_check",
            Self::GraphTags => "graph_tags",
            Self::Distill => "distill",
            Self::Settle => "settle",
            Self::Ingest => "ingest",
            Self::Embed => "embed",
            Self::Checkup => "checkup",
        }
    }
}

/// 调度优先级：**判别值越大优先级越高**（`Interactive` 最高）。TDD-01 §5.2。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// 后台（批量队列，可暂停、让位交互式）。
    Background,
    /// 普通（单文件转录、二次处理）。
    #[default]
    Normal,
    /// 交互式（弹窗翻译/语音输入/对话/悬浮菜单，插队、低延迟）。
    Interactive,
}

/// 质量档位（与模型/二次处理对应，TDD-03 §2.2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// 快（实时/低配/首过）。
    #[default]
    Fast,
    /// 高质量（更大模型、更慢；二次处理）。
    Quality,
}

/// 路由偏好（用户/功能给出）。TDD-01 §4。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingHint {
    /// 严格本地：过滤掉一切联网 Provider（TDD-12 §1）。
    pub local_only: bool,
    /// 首选 Provider。
    pub preferred_provider: Option<ProviderId>,
    /// 质量档位。
    pub quality: QualityTier,
}

/// Job 选项（语言/模板/温度/流式开关…）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobOptions {
    /// 目标语言（翻译/转写）。
    pub target_lang: Option<Lang>,
    /// 模板/预设。
    pub template: Option<PresetId>,
    /// 采样温度。
    pub temperature: Option<f32>,
    /// 是否流式。
    pub streaming: bool,
    /// 输出长度预算（token）。
    ///
    /// 同理：模型 Modelfile 里保守的 `num_predict` 会把长改写**腰斩在半句话上**且不报错。
    /// 只对确实需要长输出的任务（整理稿改写）显式给足。
    pub output_budget: Option<u32>,
}

/// 一个 AI 任务的统一描述。
///
/// 注：TDD-01 §2.1 草案的 `Job.input: JobInput` 字段与 `JobKind` 内嵌的输入引用
/// （`audio` / `text` / `messages`）**重复**——本实现按第一性原理将输入并入 `kind`，
/// 不另设 `input` 字段。该 TDD 字段待回收口（详见评审）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    /// 稳定标识（UUIDv7）。
    pub id: JobId,
    /// 任务类型与输入。
    pub kind: JobKind,
    /// 选项。
    pub options: JobOptions,
    /// 路由偏好。
    pub routing: RoutingHint,
    /// 来源（归档/遥测，不影响执行）。
    pub origin: JobOrigin,
    /// 优先级。
    pub priority: Priority,
    /// 创建时间（由持有时钟的调度层戳入；核心默认 `ZERO`）。
    pub created_at: Timestamp,
}

/// 准入泳道：这个 Job 该向**哪一个**并发闸门要额度（ADR-078）。
///
/// # 为什么必须分开，而不是把同一个数字调大
///
/// 一个信号量只表达得了一件事：「同时在算的计算单元数」。批式转写、翻译、蒸馏、建库
/// 确实都是那种东西——领一份活、算完、还额度。**流式 ASR 不是**：它是一条会在飞几分钟
/// 的会话，真正的计算逐窗发生，而窗与窗之间的节流已由 RTF / `DegradationController`
/// 单独治理。拿它去占一个「同时在算」的额度是**类型错误**，把数字调大只是把撞车的
/// 门槛往后挪。
///
/// 代价是真金白银的：额度在**弹堆之前**获取、随任务持有到 Job 结束，而远程实时转写的
/// WebSocket 是在 `execute()` 里才连的——`execute()` 只在拿到额度之后才被调用。于是
/// 「批式在跑 ⇒ 新开的实时会话连一条 `session.start` 都发不出去」，采集侧照常投窗、
/// 窗队列填满、逐窗丢弃，用户看到的是「录着音，一个字都不出」。分池之后这条因果链
/// 在第一环就断了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionLane {
    /// **会话池**：属于一条在飞的实时采集会话的腿（流式 ASR 轨 + 该会话的实时转译）。
    ///
    /// 容量随**采集轨数**派生而不是随核数——它度量的是「同时有几条流在飞」，这个数由
    /// 麦克风硬互斥（TDD-05 §5.3）与采集源的轨数上限唯一决定。
    Session,
    /// **计算池**：领一份活、算完就还的重计算单元（批式转写 / 翻译 / 总结 / 嵌入 …）。
    Compute,
}

/// 服务档：**这次调用要哪一档服务**（ADR-083 ②③④）。
///
/// 判据一句话：**批量档 = 全量输入已经在手、产出要与全文一致、迟到不损失什么；
/// 实时档 = 短文本随到随译，产出的价值随时间衰减。**
///
/// **与 [`Priority`]（谁先被调度）、[`AdmissionLane`]（向哪个池要额度）是三个正交的
/// 问题，今天恰好两两不重合**：实时转译的 `Priority` 是 `Normal`（ADR-078 靠会话池兜住了
/// 它的准入，但「优先级」这一格与它的截止期是反的）；弹窗翻译的 `AdmissionLane` 是
/// `Compute`，**且应该是**——它没有在飞的会话。合并任意两个都会立刻产生一处说谎的映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceClass {
    /// 实时档：短文本随到随译，产出的价值随时间衰减。
    Realtime,
    /// 批量档：全量输入在手、产出要与全文一致、迟到不损失什么。
    Batch,
}

impl ServiceClass {
    /// 全部取值（供遍历与守护测试）。
    pub const ALL: [Self; 2] = [Self::Realtime, Self::Batch];

    /// 线上取值。**逐条写死而不是靠 `Debug` 派生**：它是**跨仓契约**（服务端持同一组
    /// 字符串校验 `POST /inference/v1/jobs` 的 `service_class`），派生出来的名字会跟着
    /// 字段改名悄悄变。判据同 [`crate::Capability::as_str`] 与 [`crate::PromptScope::as_str`]。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Realtime => "realtime",
            Self::Batch => "batch",
        }
    }

    /// 未知取值不回落到某一档——静默按批量跑的表现是实时字幕莫名其妙变慢，
    /// 而没有任何东西报错（同 `RouteLane::parse` 对未知泳道名回 `None`）。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }
}

/// 模型槽位：可选集合来自**模型注册表**（已下载/内置的模型），不由设置注册表枚举。
///
/// **住在 `luna-core` 而不是 `luna-config`**：它是 [`Job::model_slot`] 那张
/// 「路由泳道 → 槽位」总映射的值域（ADR-083 ⑥），
/// 而那张表既要被设置层消费（按槽位存分配），也要被路由层消费（按槽位解析 Provider）。
/// 两处各持一份枚举必然分叉，而分叉的表现是「设置页里的实时档在连上服务端之后悄悄失效」。
/// `luna-config` 原样再导出它，设置层的调用点一个字节不改。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSlot {
    /// 实时档 ASR（录音/字幕流式）。
    RealtimeAsr,
    /// 离线批量档 ASR（导入首转/二次转写）。
    BatchAsr,
    /// 默认/质量档 LLM（对话/翻译/笔记 AI）。
    DefaultLlm,
    /// 实时档 LLM（实时字幕/录音翻译）。
    RealtimeLlm,
    /// 默认嵌入模型（知识检索）。
    DefaultEmbed,
}

impl ModelSlot {
    /// 全部取值（供遍历与守护测试）。
    pub const ALL: [Self; 5] = [
        Self::RealtimeAsr,
        Self::BatchAsr,
        Self::DefaultLlm,
        Self::RealtimeLlm,
        Self::DefaultEmbed,
    ];

    /// 稳定序列化名（= `ai.assignment.*` 设置事实键的后缀）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RealtimeAsr => "realtime_asr",
            Self::BatchAsr => "batch_asr",
            Self::DefaultLlm => "default_llm",
            Self::RealtimeLlm => "realtime_llm",
            Self::DefaultEmbed => "default_embedding",
        }
    }
}

impl Job {
    /// 用默认选项/路由、`Normal` 优先级构造一个 Job（`created_at` 由调度层稍后戳入）。
    #[must_use]
    pub fn new(kind: JobKind, origin: JobOrigin) -> Self {
        Self {
            id: JobId::new(),
            kind,
            options: JobOptions::default(),
            routing: RoutingHint::default(),
            origin,
            priority: Priority::default(),
            created_at: Timestamp::ZERO,
        }
    }

    /// 本 Job 的准入泳道（[`AdmissionLane`]，调度器唯一的分池判据）。
    ///
    /// 判据是 **Job 的形状**，不是提交它的人自报的——同一个 `JobKind::Translate` 既可能是
    /// 弹窗里那一次性的一段话（计算池），也可能是一条实时会话逐句在译的那一股（会话池），
    /// 而两者的区别恰好已经写在 `origin` 里。不为此新加字段：新字段等于让每个提交点自己
    /// 声明泳道，而声明是会漏的——漏了的表现就是这次这个缺陷。
    ///
    /// 实时转译**必须**与它所属的 ASR 腿同池：它同样有截止期（一句译文晚到就随字幕
    /// 滚走了），也同样会被批式挤掉——把它留在计算池里，等于把刚修好的缺陷原样再留一份
    /// 给译文那条腿。TDD-08 §4「转译不被 ASR 降级挂起」说的是引擎分治，不是让它去和批式
    /// 抢同一个额度。
    #[must_use]
    pub fn admission_lane(&self) -> AdmissionLane {
        match &self.kind {
            JobKind::Transcribe {
                audio: AudioRef::Stream(_),
                ..
            } => AdmissionLane::Session,
            // 实时转译管线的 Job（`JobOrigin::Recording` + `Translate`）。停录后的批量
            // 补译走 `JobOrigin::SecondPass`，那一份没有截止期，是计算池的活。
            JobKind::Translate { .. } if matches!(self.origin, JobOrigin::Recording) => {
                AdmissionLane::Session
            }
            _ => AdmissionLane::Compute,
        }
    }

    /// 这次调用要哪一档服务（[`ServiceClass`]）。
    ///
    /// 判据是 **Job 的形状**，不是提交它的人自报的——ADR-083 ③
    /// 承的是 [`Job::admission_lane`] 的同一条纪律：**不给它一个「调用方可覆盖」的字段**，
    /// 那等于让每个提交点自己声明，而声明是会漏的——弹窗翻译就漏过（它连
    /// `RoutingHint.preferred_provider` 都不设，于是本该最看重延迟的那条调用落在质量档上）。
    ///
    /// **`JobOrigin` 那一支穷尽 match、不写 `_ =>`**：新增 origin 必须是编译错误，
    /// 而不是静默落进某一档。逐 origin 的答案见 TDD-01 §2.1b 的表。
    #[must_use]
    pub fn service_class(&self) -> ServiceClass {
        match &self.kind {
            // 转写按**音频形状**分：还在采的那条流才是实时的，其余（文件 / PCM / 已上传）
            // 全量在手。这与 [`Job::is_batch_asr`] 是**同一判据、合并成一处**——此前
            // 两处各 match 了一遍 `AudioRef`，加一个变体就要记得改两处。
            JobKind::Transcribe { audio, .. } => match audio {
                AudioRef::Stream(_) => ServiceClass::Realtime,
                AudioRef::File(_) | AudioRef::Pcm(_) | AudioRef::Uploaded(_) => ServiceClass::Batch,
            },
            _ => match self.origin {
                // 用户盯着一段短文本等即时取词，没有「全文」可言。
                JobOrigin::PopupTranslate | JobOrigin::Screenshot => ServiceClass::Realtime,
                // 一句译文晚到就随字幕滚走了（同 ADR-078 ①）。
                JobOrigin::Recording => ServiceClass::Realtime,
                // 结果要立刻注入光标处，用户手停在那里。
                JobOrigin::VoiceInput => ServiceClass::Realtime,
                // 同弹窗：选中一段、就地出结果。
                JobOrigin::FloatingMenu => ServiceClass::Realtime,
                // 用户点了「试跑一次」正盯着结果——它验的也正是交互档那条链。
                JobOrigin::SelfCheck => ServiceClass::Realtime,
                // 逐 token 上屏；今天 chat 只有一条泳道，两档同落。
                JobOrigin::Chat => ServiceClass::Realtime,
                // 全量在手、无截止期；`SecondPass` 正是「权威双语」那一遍。
                JobOrigin::Batch | JobOrigin::SecondPass => ServiceClass::Batch,
                // 脚本化的整段处理，没有逐字上屏的截止期。
                JobOrigin::Cli => ServiceClass::Batch,
                // 后台任务，全量输入在手。
                JobOrigin::Distill
                | JobOrigin::Settle
                | JobOrigin::Ingest
                | JobOrigin::GraphTags
                | JobOrigin::Embed
                | JobOrigin::Checkup => ServiceClass::Batch,
            },
        }
    }

    /// 本 Job 该落到哪个**客户端模型槽位**——「路由泳道 → 槽位」总映射的唯一一处
    /// （ADR-083 ⑥，表在 TDD-04 §2.4）。
    ///
    /// 客户端不持有 `RouteLane` 这个类型（泳道拓扑是**服务端**会演进的那一半），
    /// 泳道在客户端的身份恰好就是 `(能力, 服务档)` 这一对——`translate` × `realtime`
    /// 即 `translate_realtime`。故本函数就是那张表。
    ///
    /// **它同时是本机形态的选路依据与服务端形态的对照**：本机按槽位选模型、服务端按
    /// 泳道选模型，两边读的是同一个 [`Job::service_class`]。两处各写一份必然分叉，
    /// 而分叉的表现是「设置页里的实时档在连上服务端之后悄悄失效」。
    ///
    /// `None` = 这个能力没有槽位（`Ocr` / `Custom` 无分配面；`Diarize` 刻意没有槽位，
    /// 那是设计不是遗漏——见 TDD-04 §2.4 与 ADR-059 ③）。
    #[must_use]
    pub fn model_slot(&self) -> Option<ModelSlot> {
        match (self.kind.capability(), self.service_class()) {
            (Capability::Transcribe, ServiceClass::Realtime) => Some(ModelSlot::RealtimeAsr),
            (Capability::Transcribe, ServiceClass::Batch) => Some(ModelSlot::BatchAsr),
            // 翻译不论实时还是批量都落「翻译与实时档」：弹窗翻译同样首字敏感，
            // 专用翻译模型（Hy-MT2）在这一档；槽位没单独指派时路由回落默认候选链，行为不变。
            (Capability::Translate, _) => Some(ModelSlot::RealtimeLlm),
            // `chat` / `summarize` / `refine` 今天各只有一条泳道，两档同落质量档。
            // 新增第二条实时档 LLM 泳道时必须回来重看这三行与 `RealtimeLlm` 的作用域
            //（TDD-04 §2.4 那条注写着同一句）。
            (Capability::Chat | Capability::Summarize | Capability::Refine, _) => {
                Some(ModelSlot::DefaultLlm)
            }
            (Capability::Embed, _) => Some(ModelSlot::DefaultEmbed),
            (Capability::Ocr | Capability::Diarize | Capability::Custom, _) => None,
        }
    }

    /// 是否为**批式 ASR**：音频已经全量在手（文件 / PCM / 已上传），没有截止期。
    ///
    /// 高负载时该被卸载的就是这一类（TDD-02 §5.2）：它错过一秒什么也不损失，等会儿
    /// 照样解得出同样的字；而实时窗错过就永远补不回来。
    ///
    /// **判据不在这里，在 [`Job::service_class`]**（ADR-083 ③「同一判据、合并成一处」）：
    /// 本函数只是把「转写 + 批量档」这个组合起个名字给调度器用。
    #[must_use]
    pub fn is_batch_asr(&self) -> bool {
        matches!(&self.kind, JobKind::Transcribe { .. })
            && self.service_class() == ServiceClass::Batch
    }
}

#[cfg(test)]
mod tests {
    use super::WindowInput;

    /// 造一个窗：`speech_len` 个真实语音 + 补零到 `padded` 长。
    fn window(abs_start: u64, speech_len: usize, padded: usize) -> WindowInput {
        let mut samples = vec![0.5f32; speech_len];
        samples.resize(padded.max(speech_len), 0.0);
        WindowInput {
            samples,
            abs_start,
            speech_len,
        }
    }

    /// 分段器的四种重复形态，持续流消费者必须一个都不重发。
    ///
    /// 这条是 ADR-038 的计费与隐私底线：上行字节直接换算成秒数计费，也直接等于
    /// 「多少麦克风内容离开了设备」。任何一种重复被放过，用户都在为同一段话付第二次钱。
    #[test]
    fn a_continuous_stream_consumer_never_resends_audio() {
        let sr = 16_000u64;
        let mut sent = 0u64;
        let mut total = 0usize;
        let take = |w: &WindowInput, sent: &mut u64| {
            let n = w.fresh_after(*sent).map_or(0, <[f32]>::len);
            *sent = (*sent).max(w.span().end);
            n
        };

        // ① 增长快照：同一句每 1.5s 重发全量，只该取到新增的那 1.5s。
        total += take(&window(0, 24_000, 160_000), &mut sent);
        total += take(&window(0, 48_000, 160_000), &mut sent);
        total += take(&window(0, 72_000, 160_000), &mut sent);
        assert_eq!(total, 72_000, "增长快照被重复上行了");
        assert_eq!(sent, 72_000);

        // ② 丢窗（通道满 try_send 失败）：下一个快照是全量的，空洞自动补上。
        //    ——这正是游标法比「只发 speech_delta」强的地方。
        let after_drop = take(&window(0, 120_000, 160_000), &mut sent);
        assert_eq!(after_drop, 48_000, "丢窗造成的空洞没有被补上");

        // ③ 强切后的 overlap 前缀：`abs_start` 回退了 ~250ms，那段已经发过。
        let overlap = sr / 4;
        let next = window(120_000 - overlap, 30_000, 160_000);
        let fresh = take(&next, &mut sent);
        assert_eq!(
            fresh,
            30_000 - overlap as usize,
            "overlap 前缀被重发了——上游会听到重复的 250ms"
        );

        // ④ 两句之间的静音：VAD 不产窗，游标直接跳过，静音不上行（计费口径的前提）。
        let far = window(500_000, 16_000, 160_000);
        assert_eq!(take(&far, &mut sent), 16_000);
        assert_eq!(sent, 516_000);
    }

    /// padding 的零**绝不上行**：它是喂给 whisper 抑制幻觉的，对持续流上游只是
    /// 噪声，而且照样按字节计费。
    #[test]
    fn padding_never_leaves_the_device() {
        let w = window(0, 8_000, 160_000);
        assert_eq!(
            w.fresh_after(0).map(<[f32]>::len),
            Some(8_000),
            "补零被当成语音发出去了：一句 0.5 秒的话会按 10 秒计费"
        );
        // 全部落在游标之前 → 一个字节都不发。
        assert!(w.fresh_after(8_000).is_none());
        assert!(w.fresh_after(99_999).is_none());
    }

    /// `AudioRef` 的 wire 形状是**跨仓契约**：服务端按它反序列化转写请求。
    ///
    /// `#[serde(rename_all = "snake_case")]` 一旦被去掉或改成别的策略，标签就从
    /// `uploaded` 变成 `Uploaded`，服务端解析失败——而客户端这边编译、lint、其余单测
    /// 全绿。这条把标签名本身钉死。
    #[test]
    fn audio_ref_wire_tags_are_snake_case() {
        use crate::ids::UploadId;
        let json = serde_json::to_string(&AudioRef::Uploaded(UploadId::from("up_1"))).unwrap();
        assert_eq!(json, r#"{"uploaded":"up_1"}"#);
        assert_eq!(
            serde_json::from_str::<AudioRef>(r#"{"uploaded":"up_1"}"#).unwrap(),
            AudioRef::Uploaded(UploadId::from("up_1"))
        );
        // 既有三个变体一并钉住——加变体时顺手改了 rename_all 是真实发生过的一类改动。
        assert!(serde_json::to_string(&AudioRef::File("/tmp/a.wav".into()))
            .unwrap()
            .starts_with(r#"{"file":"#));
        assert!(
            serde_json::to_string(&AudioRef::Stream(crate::ids::StreamId::new()))
                .unwrap()
                .starts_with(r#"{"stream":"#)
        );
    }
    use super::*;

    #[test]
    fn kind_maps_to_capability() {
        let k = JobKind::Translate {
            text: TextRef::Inline("hello".into()),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        };
        assert_eq!(k.capability(), Capability::Translate);
        assert_eq!(
            JobKind::Refine {
                text: TextRef::Inline("x".into()),
                instruction: None
            }
            .capability(),
            Capability::Refine
        );
    }

    /// [`JobOrigin::key`] 必须与 serde 落盘的串**逐字相同**：用量记录写的是 serde 串，
    /// 而前端按它查 i18n 标签。两者分家的表现是用量视图里冒出一行原始 key（`checkup`），
    /// 而不是"知识体检"——批次 2/3/4 的三个 origin 正是这样漏掉了标签。
    #[test]
    fn origin_key_matches_serde_and_covers_every_variant() {
        for origin in JobOrigin::ALL {
            let json = serde_json::to_string(&origin).expect("serde");
            assert_eq!(
                json.trim_matches('"'),
                origin.key(),
                "{origin:?} 的 key 与 serde 串不一致"
            );
        }
        let keys: std::collections::BTreeSet<&str> =
            JobOrigin::ALL.iter().map(|o| o.key()).collect();
        assert_eq!(keys.len(), JobOrigin::ALL.len(), "key 必须互不相同");
    }

    #[test]
    fn priority_orders_interactive_highest() {
        assert!(Priority::Interactive > Priority::Normal);
        assert!(Priority::Normal > Priority::Background);
        let mut v = [
            Priority::Normal,
            Priority::Background,
            Priority::Interactive,
        ];
        v.sort_unstable();
        assert_eq!(
            v,
            [
                Priority::Background,
                Priority::Normal,
                Priority::Interactive
            ]
        );
    }

    #[test]
    fn job_serde_round_trip() {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("hello world".into()),
                source: Some(Lang::from("en")),
                target: Lang::from("zh"),
                instruction: Some(Instruction::raw("Slack casual tone")),
                template: Some(PresetId::from("slack")),
            },
            JobOrigin::PopupTranslate,
        );
        let json = serde_json::to_string(&job).unwrap();
        let back: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(job, back);
        assert_eq!(back.kind.capability(), Capability::Translate);
    }

    /// `Capability::Custom` 是**扩展点**，不是「还没想好归哪」的收容所。
    ///
    /// 判据：一个 `JobKind` 若是产品内置的具体任务，它的路由能力必须是某个**落点会声明
    /// 自己提供**的能力。服务端的 `SERVED` 与本机引擎的差集恰好是
    /// `Ocr / Diarize / Custom`——前两者是客户端本地引擎（设计如此），第三个是任意提示词
    /// 通道。挂进第三个的内置任务会**只在本机可用**，而失败形态是「路由不到 Provider」，
    /// 与「引擎没启动」长得一模一样（ADR-059：关系标签重扫因此在服务端形态下全军覆没）。
    ///
    /// 穷尽 match 无通配臂：新增 `JobKind` 变体时此处编译失败，倒逼作者当场回答
    /// 「这个任务在服务端跑得了吗」。
    #[test]
    fn only_the_custom_job_kind_routes_as_the_custom_capability() {
        let kinds = [
            JobKind::Transcribe {
                audio: AudioRef::Stream(StreamId::new()),
                lang_hint: None,
                initial_prompt: None,
            },
            JobKind::Translate {
                text: TextRef::Inline(String::new()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobKind::Summarize {
                text: TextRef::Inline(String::new()),
                style: SummaryStyle::Bullets,
                instruction: None,
            },
            JobKind::Refine {
                text: TextRef::Inline(String::new()),
                instruction: None,
            },
            JobKind::Chat {
                messages: Vec::new(),
                tools: Vec::new(),
            },
            JobKind::Ocr {
                image: ImageRef::Path("a.png".into()),
            },
            JobKind::Diarize {
                audio: AudioRef::Stream(StreamId::new()),
                lang: None,
            },
            JobKind::ExtractTags {
                text: TextRef::Inline(String::new()),
                instruction: Instruction::default(),
            },
            JobKind::Embed {
                purpose: EmbeddingPurpose::Document,
                texts: Vec::new(),
            },
            JobKind::Custom {
                template: PresetId::from("t"),
                vars: BTreeMap::new(),
            },
        ];
        for kind in &kinds {
            // 穷尽（无 `_` 臂）：加变体即编译失败。
            let is_custom_kind = match kind {
                JobKind::Custom { .. } => true,
                JobKind::Transcribe { .. }
                | JobKind::Translate { .. }
                | JobKind::Summarize { .. }
                | JobKind::Refine { .. }
                | JobKind::Chat { .. }
                | JobKind::Ocr { .. }
                | JobKind::Diarize { .. }
                | JobKind::ExtractTags { .. }
                | JobKind::Embed { .. } => false,
            };
            assert_eq!(
                kind.capability() == Capability::Custom,
                is_custom_kind,
                "{kind:?} 的路由能力与「只有 JobKind::Custom 用 Capability::Custom」相悖"
            );
        }
        // 反向钉死本次修复本身：标签提取必须落在服务端也提供的那一档。
        assert_eq!(
            JobKind::ExtractTags {
                text: TextRef::Inline(String::new()),
                instruction: Instruction::default(),
            }
            .capability(),
            Capability::Summarize
        );
    }

    /// **准入泳道的判据必须只看 Job 的形状**（ADR-078 ①）。
    ///
    /// 现场那一幕：批式二次转写在跑时开一场实时会话，实时腿一个字都不出。根因是四类
    /// 消费者共用同一个信号量的 4 个额度，而流式 ASR 的 WebSocket 是在 `execute()` 里
    /// 才连的、`execute()` 只在拿到额度后才被调用——排队即失声，且没有任何东西会变红。
    #[test]
    fn a_live_session_leg_never_competes_for_a_compute_permit() {
        use super::{AdmissionLane, AudioRef, Job, JobKind, JobOrigin, TextRef};
        use crate::ids::StreamId;
        use crate::lang::Lang;

        let stream_asr = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(StreamId::new()),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::Recording,
        );
        assert_eq!(stream_asr.admission_lane(), AdmissionLane::Session);
        assert!(!stream_asr.is_batch_asr(), "流式腿不是批式 ASR");

        // 语音输入的流式腿同样是会话腿——它与录音互斥，但形状一样。
        let voice_asr = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(StreamId::new()),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::VoiceInput,
        );
        assert_eq!(voice_asr.admission_lane(), AdmissionLane::Session);

        // 实时转译与它所属的 ASR 腿同池：晚到的译文与没有译文等价。
        let live_translate = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("x".into()),
                source: None,
                target: Lang::from("en"),
                instruction: None,
                template: None,
            },
            JobOrigin::Recording,
        );
        assert_eq!(live_translate.admission_lane(), AdmissionLane::Session);

        // 停录后的批量补译没有截止期 → 计算池。
        let second_pass_translate = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("x".into()),
                source: None,
                target: Lang::from("en"),
                instruction: None,
                template: None,
            },
            JobOrigin::SecondPass,
        );
        assert_eq!(
            second_pass_translate.admission_lane(),
            AdmissionLane::Compute
        );

        // 批式二次转写：计算池，且是高负载时该被卸载的那一类。
        let second_pass_asr = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::File("/tmp/a.wav".into()),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::SecondPass,
        );
        assert_eq!(second_pass_asr.admission_lane(), AdmissionLane::Compute);
        assert!(second_pass_asr.is_batch_asr());

        // 弹窗翻译是一次性的一段话，与实时会话无关。
        let popup = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("x".into()),
                source: None,
                target: Lang::from("en"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        assert_eq!(popup.admission_lane(), AdmissionLane::Compute);
    }

    // ── 服务档 `ServiceClass`（ADR-083 ②③④，逐 origin 的表在 TDD-01 §2.1b）────────

    #[test]
    fn service_class_wire_values_round_trip() {
        // 跨仓契约：服务端持同一组字符串校验 `/inference/v1/jobs` 的 `service_class`。
        use super::ServiceClass;
        assert_eq!(ServiceClass::Realtime.as_str(), "realtime");
        assert_eq!(ServiceClass::Batch.as_str(), "batch");
        for class in ServiceClass::ALL {
            assert_eq!(ServiceClass::parse(class.as_str()), Some(class));
            // serde 与 `as_str` 必须是同一组串（两者分叉 = wire 上悄悄换了值）。
            assert_eq!(
                serde_json::to_string(&class).expect("serialize"),
                format!("\"{}\"", class.as_str())
            );
        }
        // 未知取值不回落到某一档——静默按批量跑的表现是实时字幕莫名其妙变慢。
        for bogus in ["", "Realtime", "live", "streaming"] {
            assert!(ServiceClass::parse(bogus).is_none(), "{bogus} 不该被认出来");
        }
    }

    /// **`JobOrigin::ALL` 每一项都必须被显式回答**（ADR-083 ③）。
    ///
    /// 推导本身是穷尽 match（新增 origin 是编译错误），本条守的是另一半：**答案对不对**，
    /// 以及新增 origin 时有没有人顺手把它塞进某一档而没回来更新这张表。表逐字来自
    /// TDD-01 §2.1b。
    #[test]
    fn every_job_origin_is_explicitly_answered_by_service_class() {
        use super::{Job, JobKind, JobOrigin, ServiceClass, TextRef};
        use crate::lang::Lang;

        let expected: &[(JobOrigin, ServiceClass)] = &[
            // 用户盯着一段短文本等即时取词，没有「全文」可言。
            (JobOrigin::PopupTranslate, ServiceClass::Realtime),
            (JobOrigin::Screenshot, ServiceClass::Realtime),
            // 一句译文晚到就随字幕滚走了。
            (JobOrigin::Recording, ServiceClass::Realtime),
            // 结果要立刻注入光标处，用户手停在那里。
            (JobOrigin::VoiceInput, ServiceClass::Realtime),
            // 同弹窗：选中一段、就地出结果。
            (JobOrigin::FloatingMenu, ServiceClass::Realtime),
            // 逐 token 上屏。
            (JobOrigin::Chat, ServiceClass::Realtime),
            // 全量在手、无截止期；`SecondPass` 正是「权威双语」那一遍。
            (JobOrigin::Batch, ServiceClass::Batch),
            (JobOrigin::SecondPass, ServiceClass::Batch),
            // 脚本化的整段处理。
            (JobOrigin::Cli, ServiceClass::Batch),
            // 后台任务，全量输入在手。
            (JobOrigin::Distill, ServiceClass::Batch),
            // 沉淀跑的就是蒸馏那条管线（PRD-06-054：只加 origin、不加 `JobKind`），
            // 故服务档与 `Distill` 逐字相同——它分开的只有那本用量账。
            (JobOrigin::Settle, ServiceClass::Batch),
            (JobOrigin::Ingest, ServiceClass::Batch),
            (JobOrigin::GraphTags, ServiceClass::Batch),
            (JobOrigin::Embed, ServiceClass::Batch),
            (JobOrigin::Checkup, ServiceClass::Batch),
            // 「试跑一次」：用户点完正盯着结果，而它验的也正是交互档那条链——拿批量档去
            // 验，通过了也不能说明弹窗翻译跑得通（两档在服务端是两条独立的链，ADR-083 ②）。
            (JobOrigin::SelfCheck, ServiceClass::Realtime),
        ];

        for origin in JobOrigin::ALL {
            let (_, want) = expected
                .iter()
                .find(|(o, _)| *o == origin)
                .unwrap_or_else(|| panic!("新增的 origin {origin:?} 没有被这张表显式回答"));
            let job = Job::new(
                JobKind::Translate {
                    text: TextRef::Inline("x".into()),
                    source: None,
                    target: Lang::from("en"),
                    instruction: None,
                    template: None,
                },
                origin,
            );
            assert_eq!(job.service_class(), *want, "{origin:?} 的服务档不对");
        }
        assert_eq!(
            expected.len(),
            JobOrigin::ALL.len(),
            "表里有 JobOrigin::ALL 之外的条目（或反过来）"
        );
    }

    /// 转写按**音频形状**分档，每种 [`AudioRef`] 各一条；且它与 [`Job::is_batch_asr`]
    /// 是同一判据（ADR-083 ③「合并成一处」）。
    #[test]
    fn transcribe_service_class_follows_the_audio_shape() {
        use super::{AudioRef, Job, JobKind, JobOrigin, PcmBuffer, ServiceClass};
        use crate::ids::{StreamId, UploadId};

        let audio_cases = [
            (AudioRef::Stream(StreamId::new()), ServiceClass::Realtime),
            (AudioRef::File("/tmp/a.wav".into()), ServiceClass::Batch),
            (
                AudioRef::Pcm(PcmBuffer {
                    sample_rate: 16_000,
                    channels: 1,
                    samples: vec![0.0],
                }),
                ServiceClass::Batch,
            ),
            (
                AudioRef::Uploaded(UploadId::new("up_1")),
                ServiceClass::Batch,
            ),
        ];
        for (audio, want) in audio_cases {
            // origin 刻意取 `Recording`（实时档的那一个）：转写这一支**不看 origin**，
            // 只看音频形状——回落去看 origin 的话，一件已上传的批式转写会被判成实时。
            let job = Job::new(
                JobKind::Transcribe {
                    audio: audio.clone(),
                    lang_hint: None,
                    initial_prompt: None,
                },
                JobOrigin::Recording,
            );
            assert_eq!(job.service_class(), want, "{audio:?} 的服务档不对");
            assert_eq!(
                job.is_batch_asr(),
                want == ServiceClass::Batch,
                "{audio:?}：is_batch_asr 与 service_class 必须是同一判据"
            );
        }
    }

    /// **「路由泳道 → 模型槽位」总映射逐行一条断言**（ADR-083 ⑥，表在 TDD-04 §2.4）。
    ///
    /// 删任一行必红。这张表是「实时档到底生不生效」的唯一判据，而它失效的方式是安静的
    /// ——选路照常成功，只是选到了另一个模型。
    #[test]
    fn every_route_lane_maps_to_exactly_one_model_slot() {
        use super::{
            AudioRef, EmbeddingPurpose, ImageRef, Job, JobKind, JobOrigin, ModelSlot, SummaryStyle,
            TextRef,
        };
        use crate::ids::StreamId;
        use crate::lang::Lang;

        let text = || TextRef::Inline("x".into());
        let translate = |origin| {
            Job::new(
                JobKind::Translate {
                    text: text(),
                    source: None,
                    target: Lang::from("en"),
                    instruction: None,
                    template: None,
                },
                origin,
            )
        };

        // translate_realtime → realtime_llm
        assert_eq!(
            translate(JobOrigin::Recording).model_slot(),
            Some(ModelSlot::RealtimeLlm)
        );
        // translate_batch → realtime_llm（D3：翻译一律进「翻译与实时档」）
        assert_eq!(
            translate(JobOrigin::SecondPass).model_slot(),
            Some(ModelSlot::RealtimeLlm)
        );
        // chat → default_llm
        assert_eq!(
            Job::new(
                JobKind::Chat {
                    messages: Vec::new(),
                    tools: Vec::new()
                },
                JobOrigin::Chat
            )
            .model_slot(),
            Some(ModelSlot::DefaultLlm)
        );
        // summarize → default_llm
        assert_eq!(
            Job::new(
                JobKind::Summarize {
                    text: text(),
                    style: SummaryStyle::Bullets,
                    instruction: None
                },
                JobOrigin::Distill
            )
            .model_slot(),
            Some(ModelSlot::DefaultLlm)
        );
        // refine → default_llm
        assert_eq!(
            Job::new(
                JobKind::Refine {
                    text: text(),
                    instruction: None
                },
                JobOrigin::VoiceInput
            )
            .model_slot(),
            Some(ModelSlot::DefaultLlm)
        );
        // embed → default_embedding
        assert_eq!(
            Job::new(
                JobKind::Embed {
                    texts: vec!["x".into()],
                    purpose: EmbeddingPurpose::Document
                },
                JobOrigin::Embed
            )
            .model_slot(),
            Some(ModelSlot::DefaultEmbed)
        );
        // transcribe_realtime → realtime_asr
        assert_eq!(
            Job::new(
                JobKind::Transcribe {
                    audio: AudioRef::Stream(StreamId::new()),
                    lang_hint: None,
                    initial_prompt: None
                },
                JobOrigin::Recording
            )
            .model_slot(),
            Some(ModelSlot::RealtimeAsr)
        );
        // transcribe_batch → batch_asr
        assert_eq!(
            Job::new(
                JobKind::Transcribe {
                    audio: AudioRef::File("/tmp/a.wav".into()),
                    lang_hint: None,
                    initial_prompt: None
                },
                JobOrigin::SecondPass
            )
            .model_slot(),
            Some(ModelSlot::BatchAsr)
        );
        // 无槽位的三类：`Diarize` 刻意没有槽位（ADR-059 ③，那是设计不是遗漏）。
        assert_eq!(
            Job::new(
                JobKind::Ocr {
                    image: ImageRef::Path("/tmp/a.png".into())
                },
                JobOrigin::Screenshot
            )
            .model_slot(),
            None
        );
        assert_eq!(
            Job::new(
                JobKind::Diarize {
                    audio: AudioRef::File("/tmp/a.wav".into()),
                    lang: None
                },
                JobOrigin::SecondPass
            )
            .model_slot(),
            None
        );
        assert_eq!(
            Job::new(
                JobKind::Custom {
                    template: crate::ids::PresetId::from("x"),
                    vars: super::BTreeMap::new()
                },
                JobOrigin::Cli
            )
            .model_slot(),
            None
        );

        // 每个槽位都至少被一行映到：留一个「没有任何调用能落进去」的槽位，
        // 表现就是设置页里有一项永远不生效。
        assert_eq!(ModelSlot::ALL.len(), 5);
    }
}
