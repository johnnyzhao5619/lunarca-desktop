//! 统一处理队列（链接抓取 / 导入音视频 / 二次转写 / 二次翻译 / 说话人细分，PRD-04-032/055
//! / PRD-05-006）。
//!
//! 五类处理操作汇入**单一队列 + 单 worker 顺序处理**（一次一件）——单 worker 天然消除
//! 并发写回冲突，并使「先完成二次转写、再进行二次翻译」由入队顺序自然保证（用户可调序）。
//! 队列支持：排队调序、移除待处理项、逐件确定性进度、终态历史与清除。每件状态变化即快照
//! 全队列经 typed event `processing://queue` 推前端任务面板。
//!
//! 可控性（PRD-05-010/012）：排队项可调序/移除，**运行项可取消**（一路传播到 whisper
//! abort，非仅丢弃 future），失败/取消项**可重试**，未完成队列**跨重启续跑**。
//!
//! 隐私铁律：队列项仅含展示标签（文件名 / 笔记名 / 视频标题）与状态阶段 / 计数 / 路径 /
//! 错误码，**绝不含转写或译文正文**。DTO 更严——连载荷路径与抓取 URL 都不投影；本地续跑档
//! （`persist_path`）需要载荷才能恢复任务，故落盘含路径与 URL，但它是设备本地文件、不经 IPC
//! 外泄（与 `config.toml` 含 Vault 路径同一档次）。日志侧的 URL 一律经
//! `luna_fetch::url::redact` 只留主机名。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

pub(crate) use luna_core::cancel::TaskCancel;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// 续跑档文件名。**随库**存放于 `<Vault>/.luna/`（ADR-025 / TDD-01 §5.4 修订）——
/// 任务载荷里的 `note_rel` 是**库相对路径**，续跑档若存设备级，切库后会把它解析到新库，
/// 新库存在同名路径时二次转写/翻译会改写**另一个库**的同名笔记。任务的归属 = 它的落点库。
pub const QUEUE_STATE_FILENAME: &str = "processing-queue.json";

/// 由 Vault 根推出续跑档路径（与 sidecars/snapshots 并列于 `.luna/`，单一事实源）。
#[must_use]
pub fn queue_state_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".luna").join(QUEUE_STATE_FILENAME)
}

/// 任务类型 + 处理所需载荷（内部 + 续跑档；DTO 只投影 `kind` 标签，不外泄载荷路径）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProcTaskKind {
    /// **上次会话的非成功终态**（PRD-04-062）：从用量库回填，只有类型标签与归属，**没有载荷**
    /// ——它不能重跑（`retry` 拒绝），只供回看原因与逐条移除。永远不进续跑档（终态不存）。
    History {
        tag: String,
        note_rel: Option<String>,
    },
    /// 导入本地音/视频文件（解码 → 建笔记 → 转写 →（可选）转译）。
    Import {
        path: PathBuf,
        /// 转写笔记建在哪个文件夹（库相对路径）；`None` = 转写笔记的默认文件夹。创作者发布包页
        /// 「导入成片」用它把成片直接建进项目文件夹（ADR-162 ⑪）。
        folder: Option<String>,
        /// 转写成功后在这篇研究笔记的「素材」一节里链上它（项目概览「导入素材」，ADR-162 ⑯）；
        /// `None` = 不链。
        link_from: Option<String>,
    },
    /// 导入一份文档（ADR-164 ③）：读字节 → 转 Markdown → 建 `document` 资料笔记、原文件进附件目录。
    ImportDocument {
        path: PathBuf,
        /// 笔记建在哪个文件夹（库相对路径）；`None` = 文档的默认文件夹（`vault.documents_dir`）。
        folder: Option<String>,
        /// 成功后在这篇研究笔记的「素材」一节里链上它（与 `Import` 同一时机，ADR-162 ⑯）。
        link_from: Option<String>,
    },
    /// 网页剪藏（ADR-164 ⑦）：抓一页 → 抽正文 → 建 `document` 资料笔记（整页快照与图片进附件），
    /// 成功后链进研究笔记「来源」一节。
    ClipWeb {
        url: String,
        /// 笔记建在哪个文件夹（项目的「素材」文件夹）。
        folder: String,
        link_from: Option<String>,
    },
    /// 从网络地址抓取媒体再走导入全程（PRD-05-006，ADR-023）：**抓取只是导入多出来的
    /// 一个前置阶段**，故不另立一条队列/一套进度/一套取消——同一件任务先 `Fetching`
    /// 再 `Decoding → Ingesting → Transcribing`，用户看到的是一条连续的进度。
    ///
    /// 载荷刻意携带 `title` 与 `media`：二者在**探测确认那一刻**就已定（用户看着标题点的
    /// 「开始」），下载时再探一次既慢又可能拿到不同结果（站点 A/B 测试、标题被改）。
    /// `media` 随任务走而不读全局开关——同一批链接里"这条只要音频、那条要画面"是正常诉求
    /// （PRD-05-031 的教训）。
    Fetch {
        url: String,
        title: String,
        media: luna_fetch::FetchMediaKind,
        /// `Some(笔记)` = **按需取回媒体**（PRD-05-006）：这篇「仅存链接」的转写已经在库里，
        /// 本次只补媒体并登记为它的分片，**绝不重跑转写**——文稿可能已被用户校对过。
        /// `None` = 首次抓取（下载 → 建笔记 → 转写全程）。
        into_note: Option<String>,
        /// 转写笔记建在哪个文件夹（创作「添加来源 ▸ 视频链接」= 项目的素材文件夹，ADR-167 ④）；
        /// `None` = 转写笔记的默认文件夹。
        folder: Option<String>,
        /// 转写成功后在这篇研究笔记的「素材」一节里链上它；`None` = 不链。
        link_from: Option<String>,
    },
    /// 对已归档录音二次转写（重跑 whisper →（可选）逐句转译 → 权威写回）。
    ///
    /// `translate_to` 让**翻译意图随任务走**而不是靠全局开关猜（PRD-05-031 修订）：
    /// `None` = 只转写（手动「二次转写」按钮与导入均如此——翻译是显式动作）；
    /// `Some(目标)` = 转写后自动补译（停录后台重转写用，目标取录音会话的实时转译目标，
    /// 语义 = "我录音时就开着转译，停录后请给我权威双语"）。
    Retranscribe {
        note_rel: String,
        translate_to: Option<String>,
    },
    /// 对已归档录音二次翻译（按目标语言批量转译既有段 → 权威写回）。
    Translate {
        note_rel: String,
        target_lang: String,
    },
    /// 对已归档录音说话人细分（本地 diarization → 权威写回标签）。
    Diarize { note_rel: String },
    /// 笔记 AI 的**整篇**动作转入后台（PRD-06-021）。
    ///
    /// **只收整篇作用域**，两条理由缺一不可：① 载荷只带笔记路径，正文执行时重读——
    /// 选区的输入是一段范围，面板一关那段范围就随编辑漂移，续跑档里存正文又违反脱敏
    /// （TDD-12，同 [`Self::Distill`] 的 `full_text` 只存开关不存文本）；② 整篇的落地
    /// 方式全是**插入**（顶部 / 文末），不依赖冻结选区，而选区作用域含**替换**——替换
    /// 必须对准打开面板那一刻的那段文字，隔了一次后台往返就不再对得准。
    ///
    /// 动作与子菜单选择以稳定字符串随任务走（队列不认识 IPC 的 DTO 类型）。
    NoteAi {
        note_rel: String,
        action: String,
        target_lang: Option<String>,
        summary_style: Option<String>,
    },
    /// 把一篇资料整理成知识笔记（PRD-06-051）。
    ///
    /// **入队而非直接跑**：一次蒸馏是几十次 LLM 调用、可达数分钟，此前它在命令里直接
    /// await——用户切走视图就再也看不到它跑到哪儿了，也无从一次排上三篇（只能等一篇完
    /// 再手动点下一篇，真机反馈原话）。进了队列即白得顺序执行、阶段进度、取消、失败重试
    /// 与断点续跑，与导入/转写用的是同一套面板。
    ///
    /// 载荷携带 `template`/`full_text`：与 [`Self::Fetch`] 的 `media` 同理——用户是**看着
    /// 那份选择**点的开始，排队期间全局默认值若变了不该改写他这一件（`None` = 未显式
    /// 指定，执行时由 `resolve_template` 按「已记录 > 建议」解析）。
    Distill {
        note_rel: String,
        template: Option<String>,
        full_text: Option<bool>,
    },
    /// 把这几篇合成一篇（PRD-06-047）。
    ///
    /// **入队而非内联**（2026-09-02，随候选集放开到资料一并收口）：它此前是一条直接跑的
    /// 命令，理由是"速记都很短、不会跑很久"——候选集放开到录音与转写之后这个前提没了，
    /// 而一条脱离队列的分钟级 LLM 任务会与队列里正在跑的蒸馏**同时抢一个本地引擎**，
    /// 且在任务面板里**看不见、取消不了**（真机 2026-09-02：处理过程没有在队列中显示）。
    /// 载荷携带 `notes`：用户是看着「这 N 篇」点的汇总，排队期间标签下又多了笔记不该
    /// 改写他这一件。
    Merge {
        notes: Vec<String>,
        origin: MergeOrigin,
    },
    /// 隐藏的知识生命周期意图（PRD-16-037）。Prepared 在知识文件写盘前先持久化；写成后
    /// 转 Queued，由同一 worker 串行执行标签→后台准备整理建议，嵌入并行。它不投影到任务面板。
    KnowledgeLifecycle {
        operation_id: String,
        vault_generation: u64,
        note_rel: String,
        body_hash: String,
        origin: String,
        aggregate: bool,
    },
    /// 用户确认后的封存主题计划应用（ADR-134 / TDD-06 §3.9.3）。
    ///
    /// 续跑档只存 `operation_id` 与目标路径：声明正文留在设备本地的封存载荷里，重启后
    /// 按 ID 取回**同一份**计划复核再写，绝不重新调用模型换稿。入队即持久化，失败不入队。
    ApplyIngestPlan {
        operation_id: String,
        vault_generation: u64,
        target_rel: String,
    },
    /// 后台为一页新格式主题准备整理建议（ADR-134 / PRD-16-048「建议」档）：只生成并封存计划，
    /// 用户到这一页时就地预览、确认后才写入；不写主题、不发通知。候选组没变就不重跑模型。
    /// 在任务面板可见：它会调用模型，可能联网，用户应当看得到、取消得了。
    PrepareTopicPlan {
        topic_rel: String,
        vault_generation: u64,
    },
    /// 后台为一条已成簇、还没有主题页的标签起草新主题初稿（PRD-16-060）：只封存初稿、
    /// 不建页，用户点「创建主题页」时直接看到备好的那份。会调用模型，故在任务面板可见可取消。
    PrepareTopicDraft { tag: String, vault_generation: u64 },
}

/// 一次汇总的**候选集来源**——三个入口共用一个内核，真正不同的只有这一点（TDD-07 §2.1）。
///
/// - `Selection`：用户逐条选出的（速记勾选集 / 文件树与清单多选）。少一条就是错，故执行时
///   读不到任何一条即**整体拒绝**；超预算时在发 Job 前拒绝（`RejectPartial`）。
/// - `Tag`：按标签查询派生的**开放集合**。其中一篇刚被删掉不该让整个动作失败，故读不到的
///   **跳过**；超预算时如实标注"只覆盖了前一部分"（`AnnotatePartial`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeOrigin {
    Selection,
    Tag { tag: String },
}

/// 全部任务类型标签（`tag()` 的值域；回填历史时用它把库里的字符串映射回静态标签）。
const KNOWN_TAGS: [&str; 16] = [
    "import",
    "import_document",
    "clip_web",
    "fetch",
    "retranscribe",
    "translate",
    "diarize",
    "note_ai",
    "distill",
    // 旧版「合成主题页」任务的历史行（新建主题页已改为先预览、确认后发布，不再入队）。
    "topic",
    "merge",
    // 旧版片段归入任务的历史行（已随 ADR-136 退役，不再入队）。
    "ingest",
    "knowledge_lifecycle",
    "knowledge_apply",
    "topic_prepare",
    "topic_draft_prepare",
];

impl ProcTaskKind {
    /// DTO 展示标签（脱敏，前端据此选图标/文案）。
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            // 回填项的标签来自库里的字符串：映射回已知静态标签，认不出的归 `history`。
            Self::History { tag, .. } => KNOWN_TAGS
                .iter()
                .find(|known| **known == tag)
                .copied()
                .unwrap_or("history"),
            Self::Import { .. } => "import",
            Self::ImportDocument { .. } => "import_document",
            Self::ClipWeb { .. } => "clip_web",
            Self::Fetch { .. } => "fetch",
            Self::Retranscribe { .. } => "retranscribe",
            Self::Translate { .. } => "translate",
            Self::Diarize { .. } => "diarize",
            Self::NoteAi { .. } => "note_ai",
            Self::Distill { .. } => "distill",
            Self::Merge { .. } => "merge",
            Self::KnowledgeLifecycle { .. } => "knowledge_lifecycle",
            Self::ApplyIngestPlan { .. } => "knowledge_apply",
            Self::PrepareTopicPlan { .. } => "topic_prepare",
            Self::PrepareTopicDraft { .. } => "topic_draft_prepare",
        }
    }

    /// 归属笔记相对路径（导入在完成建笔记前无归属 → `None`）。
    #[must_use]
    pub fn note_rel(&self) -> Option<&str> {
        match self {
            Self::History { note_rel, .. } => note_rel.as_deref(),
            Self::Import { .. } | Self::ImportDocument { .. } | Self::ClipWeb { .. } => None,
            // 首次抓取在建笔记前无归属；按需取回则一开始就归属目标笔记（内联进度据此过滤）。
            Self::Fetch { into_note, .. } => into_note.as_deref(),
            Self::Retranscribe { note_rel, .. }
            | Self::Translate { note_rel, .. }
            | Self::Diarize { note_rel }
            | Self::NoteAi { note_rel, .. }
            | Self::Distill { note_rel, .. }
            | Self::KnowledgeLifecycle { note_rel, .. } => Some(note_rel),
            Self::ApplyIngestPlan { target_rel, .. } => Some(target_rel),
            Self::PrepareTopicPlan { topic_rel, .. } => Some(topic_rel),
            // 主题页还不存在：没有可归属、可深链的笔记。
            Self::PrepareTopicDraft { .. } => None,
            // 产出一篇**新**笔记，完成前无归属；产物路径经 `ProcStatus::Done` 下发。
            Self::Merge { .. } => None,
        }
    }

    /// 该任务开始运行时的初始阶段。
    fn initial_step(&self) -> ProcPhase {
        match self {
            // 回填的历史项是终态、不会运行；这里只为穷尽。
            Self::History { .. } => ProcPhase::Decoding,
            Self::Import { .. } => ProcPhase::Decoding,
            Self::ImportDocument { .. } => ProcPhase::Converting,
            Self::ClipWeb { .. } => ProcPhase::Fetching,
            Self::Fetch { .. } => ProcPhase::Fetching,
            Self::Retranscribe { .. } => ProcPhase::Transcribing,
            Self::Translate { .. } => ProcPhase::Translating,
            Self::Diarize { .. } => ProcPhase::Diarizing,
            // 与蒸馏同型：一次整篇 AI 动作也是若干次 LLM 调用，用户看到的是同一种进度。
            Self::NoteAi { .. } => ProcPhase::Distilling,
            Self::Distill { .. } => ProcPhase::Distilling,
            Self::Merge { .. } => ProcPhase::Distilling,
            Self::PrepareTopicPlan { .. } | Self::PrepareTopicDraft { .. } => ProcPhase::Filing,
            Self::KnowledgeLifecycle { .. } | Self::ApplyIngestPlan { .. } => {
                ProcPhase::MaintainingKnowledge
            }
        }
    }
}

/// 运行阶段（脱敏；供统一进度条阶段标签）。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProcPhase {
    /// 从网络抓取媒体（PRD-05-006）。**进度计数单位是 KiB 而非"件数"**
    /// （`luna_fetch::FetchProgress::as_kib`）——`ProcStatus::Running{done,total}` 是 `u32`，
    /// 字节数轻易越过 4GiB；前端按阶段分派把它格式化成「12.4 MB / 88.1 MB」而不是裸计数。
    Fetching,
    /// 解码媒体音轨。
    Decoding,
    /// 把文档转成 Markdown（ADR-164）。
    Converting,
    /// 媒体落库（视频拷原容器 / 音频编 WAV；大文件拷贝可观耗时，故独立成阶段，PRD-04-058）。
    Ingesting,
    /// **在服务端队列里等着**（ADR-049 ⑧）。
    ///
    /// 与 `ProcStatus::Queued` 是两件事，故做成一个**阶段**而不是复用那个状态：
    /// `Queued` 说的是「本机队列还没轮到它」——那时用户可以上移/下移/移除；这一档
    /// 说的是「本机已经在跑它了，而服务端把它排在第 N 位」——那时能做的只有等或取消。
    /// 复用 `Queued` 会给出三个对服务端任务毫无意义的按钮，而取消反倒会失灵
    /// （`request_cancel` 只对运行项生效）。
    ///
    /// 排位与等待区间不在计数里，而在 [`ProcTaskDto::queue`]：`done/total` 是「几分之几」，
    /// 把「第 3 位」塞进去会让进度条显示成 3/0。
    AwaitingServer,
    /// 转写中（分片计数）。
    Transcribing,
    /// 转译中（分句计数；导入/二次转写的转译阶段无计数则 total=0 不定量）。
    Translating,
    /// 说话人细分中（分片计数）。
    Diarizing,
    /// 整理成知识笔记中（分片摘要计数；单片直出时 total=1）。
    Distilling,
    /// 生成整理稿中（改写分片计数）。
    ///
    /// **与 `Distilling` 分成两个阶段而非合并**：它们是两段目标相反的工作（压缩 vs 保真
    /// 改写）、分片粒度也不同，合成一个计数会让进度条在中途突然"倒退"或跳变。
    Rewriting,
    /// 归入主题页中（PRD-16-024；目标页计数——一次片段生成 + 逐页选锚点）。
    ///
    /// 名字不用 `Ingesting`（已被**导入**的媒体落库阶段占用）：同一个词在面板上指两件事，
    /// 用户就再也说不清"我点的那个到底跑到哪儿了"。`Filing` = 归档入册，正是这一步在做的事。
    Filing,
    /// 知识产物落盘后的标签/归入/索引收口（隐藏任务仍需稳定阶段用于恢复与诊断）。
    MaintainingKnowledge,
}

/// 队列项状态（脱敏：仅阶段 / 计数 / 路径 / 错误码）。
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "snake_case", tag = "state", content = "data")]
pub enum ProcStatus {
    /// 写盘前已经持久化的知识意图；同进程 worker 不领取，重启后按文件与指纹安全恢复。
    Prepared,
    /// 排队等待（可调序 / 可移除）。
    Queued,
    /// 运行中（阶段 + 确定性进度；`total=0` = 不定量）。
    Running {
        step: ProcPhase,
        done: u32,
        total: u32,
    },
    /// 完成（归属笔记相对路径，前端据此跳转）。
    ///
    /// `awaiting_review` = 这一件跑完之后**还有几处等用户确认**；`0` = 完成即落盘。
    ///
    /// # 为什么「完成」不足以描述完成（真机 2026-08-31，批次 H）
    ///
    /// 归入在**建议档**下按设计不写主题页，只在源笔记上留一条建议条——这部分是对的。
    /// 但队列只说「已完成」：用户连点五次「立即归入」，看到五条绿勾，而主题页的
    /// mtime 停在合成那一刻，一个字节都没动。执行体那一侧本来就分得清
    /// （`IngestOutcome` 有 `applied` 与 `pending` 两份名单，它的头注甚至写着「只回一个
    /// 计数的话，两句话就只能靠前端猜，而那正是 ADR-003 禁止的」）——而队列的终态把两者
    /// 压成了同一个值，那句注释担心的事在下游又发生了一次。
    ///
    /// **是一个计数而不是「ingest 专用字段」**：它问的是「完成的是事实还是待确认的
    /// 建议」，那对任何一种任务都成立；做成 ingest 专属的话，下一个有审阅态的任务类型
    /// 又要在这里加第二个字段。
    Done {
        note_rel: Option<String>,
        awaiting_review: u32,
    },
    /// 失败（错误码，无正文）。
    ///
    /// `retryable` = **同一件任务原样重跑是否可能成功**，由 Rust 按错误码判定（ADR-003：
    /// 前端零业务，只渲染有没有「重试」按钮）。多数失败是环境性的（模型未装、磁盘满、
    /// 网络不通）故默认可重试；但抓取会撞上**站点侧硬事实**——视频已下架、区域限制、
    /// 需要登录、正在直播——那种情况下重试只会重复失败，**给一个注定失败的按钮比不给更糟**
    /// （PRD-05-006）。
    ///
    /// `retry_charges_again` = 这次失败**已经产生了一次计费**，重试是一次新的、会再计费的调用
    /// （服务端转写完成却取不回结果，`RESULT_UNAVAILABLE`）。重试按钮必须把这句后果写在按钮上，
    /// 而不是留在可能被截断的原因文案里（2026-10-08 文案与交互审查 C32）。
    Failed {
        code: String,
        retryable: bool,
        retry_charges_again: bool,
    },
    /// 用户取消（PRD-05-010）。与 `Failed` 分开——取消是**用户意图**不是故障，
    /// 不该在历史里显示为红色错误，但同样可重试。
    Cancelled,
}

impl ProcStatus {
    /// 是否终态（Done/Failed/Cancelled）。
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }

    /// 是否可重试：取消恒可重试（用户意图，环境未变）；失败看错误码的判定结果；
    /// 已完成的不重复做。
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Failed { retryable, .. } => *retryable,
            Self::Cancelled => true,
            _ => false,
        }
    }

    fn is_queued(&self) -> bool {
        matches!(self, Self::Queued)
    }

    /// 未成功终态（失败 / 取消）——面板「未成功」段与「清除未成功」的唯一口径。
    #[must_use]
    pub fn is_unsuccessful(&self) -> bool {
        matches!(self, Self::Failed { .. } | Self::Cancelled)
    }

    /// 可被用户逐条移除：排队项（还没开始）与未成功项（不打算重试）。
    fn is_removable(&self) -> bool {
        self.is_queued() || self.is_unsuccessful()
    }

    fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

/// 队列项快照（前端渲染）。
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct ProcTaskDto {
    /// 稳定项 id（进程内单调）。
    pub id: String,
    /// 任务类型标签：`fetch` / `import` / `retranscribe` / `translate` / `diarize`。
    pub kind: String,
    /// 展示标签（文件名 / 笔记名，脱敏）。
    pub label: String,
    /// 归属笔记相对路径（导入完成后为其建的笔记；供内联状态按笔记过滤）。
    pub note_rel: Option<String>,
    /// 当前状态。
    pub status: ProcStatus,
    /// **实际执行者**（PRD-17-033）：`None` = 本机跑的。
    ///
    /// 用户要能看出「这段录音是在我机器上转的，还是发到了服务端／第三方」。
    /// 静默替换是信任事故——而降级路由恰恰会让执行者与用户以为的那个不一致。
    pub executed_by: Option<ProcExecutorDto>,
    /// **服务端队列里的排位**（ADR-049 ⑧）。`None` = 不在服务端队列里（本机跑的，
    /// 或者服务端已经开始解码了）。
    ///
    /// 一个可空对象而不是几个平级的可空字段：它的缺席只有一个含义——**这件任务
    /// 此刻不在服务端队列里**。位次与等待区间分别可空会让前端去处理「有位次没区间」
    /// 「有区间没位次」这些不存在的组合。
    pub queue: Option<ProcQueueDto>,
    /// 来自上次会话的历史（PRD-04-062）：没有载荷，不可重试，只供回看原因与移除。
    pub restored: bool,
}

/// 服务端排位的展示投影。
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq)]
pub struct ProcQueueDto {
    /// 从 1 起算。
    pub position: u32,
    /// `standard` / `batch`——用户据此理解「为什么我排在后面」。
    pub tier: String,
    /// 等待区间下界（秒）。`None` = 服务端尚无样本，只报位次。
    ///
    /// **两个界一起有或一起没有**，由 Rust 保证（它们来自同一个 `Option<EtaRange>`）：
    /// 前端因此只判一次，不必处理「有下界没上界」这种编不出来的状态。
    pub eta_min_secs: Option<u32>,
    /// 等待区间上界（秒）。
    pub eta_max_secs: Option<u32>,
}

impl From<luna_core::RemoteQueueStatus> for ProcQueueDto {
    fn from(status: luna_core::RemoteQueueStatus) -> Self {
        Self {
            position: status.position,
            tier: status.tier.as_str().to_owned(),
            eta_min_secs: status.eta.map(|e| e.min_secs),
            eta_max_secs: status.eta.map(|e| e.max_secs),
        }
    }
}

/// 执行者的展示投影。不含地址与凭据线索（那是运营信息，不发给终端用户）。
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct ProcExecutorDto {
    pub display_name: String,
    /// `local_runtime` / `third_party` / `unknown`。
    pub kind: String,
    pub model: String,
    /// 数据是否离开了部署方的边界——**UI 据此决定提示强度**。
    ///
    /// 由 Rust 判定而不是让前端按 `kind` 字符串自己推：`unknown` 该归哪一侧是个
    /// 隐私判断（答案是"按离开处理"），不该散落在 TS 里（ADR-003）。
    pub leaves_deployment: bool,
}

impl From<luna_core::ExecutorInfo> for ProcExecutorDto {
    fn from(info: luna_core::ExecutorInfo) -> Self {
        Self {
            display_name: info.display_name,
            kind: match info.kind {
                luna_core::ExecutorKind::LocalRuntime => "local_runtime",
                luna_core::ExecutorKind::ThirdParty => "third_party",
                luna_core::ExecutorKind::Unknown => "unknown",
            }
            .to_owned(),
            model: info.model,
            leaves_deployment: info.kind.leaves_deployment(),
        }
    }
}

/// 一份封存计划的应用任务阶段（持续整理面板据此从「已加入队列」落定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanApplyState {
    Active,
    Failed,
    /// 已成功写入。载荷要等续跑档确认落盘后才清理，这段空隙里它仍在盘上——
    /// 现场查询须据此跳过，不能把刚应用完的计划当成过期计划接回（真机 2026-09-24）。
    Applied,
}

/// 队列快照事件（整队列推送，前端替换渲染）。
#[derive(Clone, Debug, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "processing://queue")]
pub struct ProcessingQueueEvent {
    /// 全部队列项（入队序）。
    pub items: Vec<ProcTaskDto>,
}

/// 内部队列项（亦为续跑档条目）。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProcTask {
    id: u64,
    kind: ProcTaskKind,
    label: String,
    status: ProcStatus,
    /// 实际执行者（PRD-17-033）。`None` = 本机。
    ///
    /// **不进续跑档**：它是本次运行的事实，重启续跑时会重新解析——把上次的执行者
    /// 带过来会告诉用户一件当下未必成立的事（服务端可能已经关了、槽位可能已改回本地）。
    ///
    /// 用 `skip` 而不是 `skip_serializing_if = "Option::is_none"`：后者只跳过 `None`，
    /// `Some` 照样写盘——注释说"不进档"而代码在存，正是本仓一直在防的那类不一致。
    #[serde(skip)]
    executed_by: Option<ProcExecutorDto>,
    /// 服务端队列里的排位（ADR-049 ⑧）。**同样不进续跑档**，理由与 `executed_by`
    /// 一字不差：它是本次运行的事实，重启后那个位次早就不成立了。
    #[serde(skip)]
    queue: Option<ProcQueueDto>,
    /// 用量库历史行 id（PRD-04-062）：非成功终态落盘后回写；逐条移除据此删行。不进续跑档。
    #[serde(skip)]
    history_id: Option<i64>,
}

impl ProcTask {
    fn is_hidden(&self) -> bool {
        matches!(
            self.kind,
            ProcTaskKind::KnowledgeLifecycle { .. } | ProcTaskKind::ApplyIngestPlan { .. }
        )
    }
    fn to_dto(&self) -> ProcTaskDto {
        // 完成的导入项归属其新建笔记；其余任务归属自身 note_rel。
        let note_rel = match &self.status {
            ProcStatus::Done { note_rel, .. } => note_rel.clone(),
            _ => self.kind.note_rel().map(str::to_string),
        };
        ProcTaskDto {
            id: self.id.to_string(),
            kind: self.kind.tag().to_string(),
            label: self.label.clone(),
            note_rel,
            status: self.status.clone(),
            executed_by: self.executed_by.clone(),
            queue: self.queue.clone(),
            restored: matches!(self.kind, ProcTaskKind::History { .. }),
        }
    }
}

struct Inner {
    items: VecDeque<ProcTask>,
    next_id: u64,
    /// worker 是否在跑（防止并发多 worker）。
    worker_running: bool,
    /// 当前运行项的取消令牌（单 worker → 至多一个）。
    running: Option<(u64, Arc<TaskCancel>)>,
    /// **结构性**变化版本号（入队/终态/移除/调序/取消/重试）。进度更新**不**计入——
    /// 否则每秒数次的进度回调会把续跑档写成高频 IO（PRD-05-012 的实现陷阱）。
    structural_rev: u64,
    /// 已落盘的版本号（与 `structural_rev` 相等即无需重写）。
    persisted_rev: u64,
    /// 续跑档落点（`None` = 不持久化：未打开库、或测试）。**随库**，切库经
    /// [`ProcessingQueue::rebind`] 交接，故与队列内容同处一个临界区。
    persist_path: Option<PathBuf>,
    /// 交接暂停（ADR-025 / TDD-06 §12）：切库期间 worker **不得领取新任务**——
    /// 取消只针对运行中那一件，旧 worker 在它落定后会立刻去领下一件旧库任务，
    /// 那件任务的写盘会撞上换根。由 [`ProcessingQueue::begin_handover`] 置位、
    /// [`ProcessingQueue::rebind`]（或失败回退 [`ProcessingQueue::end_handover`]）清除。
    handover: bool,
}

impl Inner {
    fn touch(&mut self) {
        self.structural_rev += 1;
    }
}

fn rebase_knowledge_tasks(tasks: &mut [ProcTask], generation: u64) {
    for task in tasks {
        if let ProcTaskKind::KnowledgeLifecycle {
            vault_generation, ..
        }
        | ProcTaskKind::ApplyIngestPlan {
            vault_generation, ..
        }
        | ProcTaskKind::PrepareTopicPlan {
            vault_generation, ..
        }
        | ProcTaskKind::PrepareTopicDraft {
            vault_generation, ..
        } = &mut task.kind
        {
            *vault_generation = generation;
        }
    }
}

/// 从用量库读回的一条非成功终态（PRD-04-062），供 [`ProcessingQueue::restore_history`] 回填。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredOutcome {
    pub history_id: i64,
    pub tag: String,
    pub label: String,
    pub note_rel: Option<String>,
    /// `Some(码)` = 失败；`None` = 取消。
    pub code: Option<String>,
}

/// 统一处理队列（AppState 持有；线程安全）。
pub struct ProcessingQueue {
    inner: Mutex<Inner>,
    /// 运行项落定信号（`running` 变 `None` 时通知）：切库交接据此**定界等待**
    /// 运行中任务真正退出，而不是发完取消就换根（迟到写盘会落进新库）。
    settled: Condvar,
}

impl Default for ProcessingQueue {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ProcessingQueue {
    /// 建队列；`persist_path` 非空则**从续跑档恢复**未完成任务（PRD-05-012）。
    ///
    /// 恢复语义：`Running` 项无法从中途续（whisper 无检查点）→ 一律回到 `Queued` 重跑；
    /// 终态项**不恢复**（历史属于上次会话，重启后从干净队列开始）。恢复失败（文件损坏/
    /// schema 变更）→ 静默从空队列开始，绝不 panic、绝不阻塞启动。
    #[must_use]
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        let mut restored = persist_path.as_deref().map(load_state).unwrap_or_default();
        // 进程重启后没有任何旧回调仍可存活，全局库代际也从 0 重新开始。把本 Vault 的
        // durable 意图定基到新进程的初始代际，才能真正续跑；正文指纹仍是第二道门。
        rebase_knowledge_tasks(&mut restored, 0);
        let next_id = restored.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        Self {
            inner: Mutex::new(Inner {
                items: restored.into(),
                next_id,
                worker_running: false,
                running: None,
                structural_rev: 0,
                persisted_rev: 0,
                persist_path,
                handover: false,
            }),
            settled: Condvar::new(),
        }
    }

    /// 切库交接（PRD-06-049 / TDD-06 §12）：把未完成项落到**旧库**续跑档 → 清空内存 →
    /// 换落点并从**新库**续跑档恢复。三步在同一临界区内完成，中途没有"半个库"的队列。
    ///
    /// 旧库续跑档写失败时返回错误且不改变队列绑定或内存项；调用方须中止切库。
    /// 调用方无须关心顺序——顺序错了就是把 A 库的任务写进 B 库，故收敛在这一个方法里。
    /// 运行中的任务由调用方在此之前取消（切库的顺序契约：先停活儿再交接状态）。
    pub fn rebind(
        &self,
        new_root: Option<&Path>,
        knowledge_generation: u64,
    ) -> std::io::Result<()> {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(old) = inner.persist_path.clone() {
            let pending: Vec<ProcTask> = inner
                .items
                .iter()
                .filter(|it| !it.status.is_terminal())
                .cloned()
                .collect();
            // 切库前必须确认旧库续跑档已落盘；失败时保持内存、路径和 handover 状态原样。
            save_state_checked(&old, &pending)?;
        }
        let next_path = new_root.map(queue_state_path);
        let mut restored = next_path.as_deref().map(load_state).unwrap_or_default();
        // 生命周期意图随 Vault 自己的续跑档保存。再次回到同一 Vault 时，全局代际已经变化，
        // 尚未开始的意图应在当前代际续跑；旧库在途任务已由 handover 定界等待完毕，不会被
        // 这次重定基复活。这样既拦跨库迟到回调，也不丢“离开后再回来”的未完成维护。
        rebase_knowledge_tasks(&mut restored, knowledge_generation);
        inner.next_id = restored.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        inner.items = restored.into();
        inner.running = None;
        // 版本号归零对齐：新库刚恢复的内容即"已落盘"状态，不该立刻被重写一遍。
        inner.structural_rev = 0;
        inner.persisted_rev = 0;
        inner.persist_path = next_path;
        // 交接完成：恢复领取（新库的续跑任务由切库编排在 `bind_vault` 里拉起 worker）。
        inner.handover = false;
        Ok(())
    }

    /// 未完成项数（排队 + 运行中）——切库预检据此如实告知"有几件会被暂停"。
    #[must_use]
    pub fn active_count(&self) -> u32 {
        let inner = self.inner.lock().expect("processing queue poisoned");
        u32::try_from(
            inner
                .items
                .iter()
                .filter(|it| !it.status.is_terminal())
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    /// 取消当前运行项（切库前的收尾：正在跑的活儿写的是**旧库**）。
    pub fn cancel_running(&self) {
        let inner = self.inner.lock().expect("processing queue poisoned");
        if let Some((_, cancel)) = inner.running.as_ref() {
            cancel.cancel();
        }
    }

    /// 队列静着：没有运行项、也没有等着领取的排队项（`Prepared` 不算——同进程 worker 不领取它）。
    /// 推迟的换账户切库据此等处理收尾（[`crate::vault_lists`]），不当场取消刚入队的活儿。
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.running.is_none() && !inner.items.iter().any(|it| it.status.is_queued())
    }

    /// 是否有待处理项（启动后据此决定要不要立刻拉起 worker 续跑）。
    #[must_use]
    pub fn has_pending(&self) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.iter().any(|it| it.status.is_queued())
    }

    /// 结构性变化时把未完成项写入续跑档（无变化则空操作）。
    ///
    /// 由广播快照的单一入口调用——凡是队列被推给前端的时刻都顺带落盘，不会漏。
    pub fn persist_if_changed(&self) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let Some(path) = inner.persist_path.clone() else {
            return;
        };
        if inner.persisted_rev == inner.structural_rev {
            return;
        }
        // 与结构变更共用同一把锁：不能让较旧的快照在较新的快照之后落盘。
        // 写失败时保留脏版本，下一次广播即重试，而非错误宣称已经持久化。
        let pending = inner
            .items
            .iter()
            .filter(|it| !it.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        match save_state_checked(&path, &pending) {
            Ok(()) => inner.persisted_rev = inner.structural_rev,
            Err(error) => eprintln!("[queue] 续跑档写入失败（仅内存生效）：{error}"),
        }
    }

    /// 立即把当前结构版本写入续跑档并返回结果；供「落盘确认后才能清理外部载荷」的调用方用。
    ///
    /// 与 [`Self::persist_if_changed`] 同锁同序，只是不吞错误；未绑库时如实报错，
    /// 调用方据此保留载荷而不是假设已经持久化。
    pub fn persist_checked(&self) -> std::io::Result<()> {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let path = inner
            .persist_path
            .clone()
            .ok_or_else(|| std::io::Error::other("processing queue has no vault"))?;
        if inner.persisted_rev == inner.structural_rev {
            return Ok(());
        }
        let pending = inner
            .items
            .iter()
            .filter(|it| !it.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        save_state_checked(&path, &pending)?;
        inner.persisted_rev = inner.structural_rev;
        Ok(())
    }

    /// 持久化一件封存计划应用意图；落盘失败即回滚，调用方不得宣称「已确认」。
    ///
    /// 同一 `operation_id` 已有未终态任务时返回那一件的 id，不重复入队。
    pub fn enqueue_plan_apply(&self, kind: ProcTaskKind, label: String) -> Result<u64, String> {
        let ProcTaskKind::ApplyIngestPlan { operation_id, .. } = &kind else {
            return Err("KNOWLEDGE_APPLY_INVALID".to_owned());
        };
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(existing) = inner.items.iter().find(|task| {
            !task.status.is_terminal()
                && matches!(
                    &task.kind,
                    ProcTaskKind::ApplyIngestPlan { operation_id: id, .. } if id == operation_id
                )
        }) {
            return Ok(existing.id);
        }
        let Some(path) = inner.persist_path.clone() else {
            return Err("KNOWLEDGE_APPLY_NO_VAULT".to_owned());
        };
        let id = inner.next_id;
        inner.next_id += 1;
        inner.items.push_back(ProcTask {
            id,
            kind,
            label,
            status: ProcStatus::Queued,
            executed_by: None,
            queue: None,
            history_id: None,
        });
        inner.touch();
        let pending = inner
            .items
            .iter()
            .filter(|it| !it.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = save_state_checked(&path, &pending) {
            inner.items.retain(|task| task.id != id);
            return Err(format!("KNOWLEDGE_APPLY_PERSIST_FAILED: {error}"));
        }
        inner.persisted_rev = inner.structural_rev;
        Ok(id)
    }

    /// 某份封存计划最近一件应用任务的阶段：排队/运行中为 `Active`，失败终态为 `Failed`，
    /// 成功为 `Applied`，取消或从未入队为 `None`。只看最新一件——失败后再次确认会入队新的一件。
    #[must_use]
    pub fn plan_apply_state(&self, operation_id: &str) -> Option<PlanApplyState> {
        let inner = self.inner.lock().expect("processing queue poisoned");
        let task = inner.items.iter().rev().find(|task| {
            matches!(
                &task.kind,
                ProcTaskKind::ApplyIngestPlan { operation_id: id, .. } if id == operation_id
            )
        })?;
        match task.status {
            ProcStatus::Failed { .. } => Some(PlanApplyState::Failed),
            ProcStatus::Done { .. } => Some(PlanApplyState::Applied),
            ref status if !status.is_terminal() => Some(PlanApplyState::Active),
            _ => None,
        }
    }

    /// 某份封存计划是否仍有未终态的应用任务（处置前据此拒绝，避免排队中的应用读到被删载荷）。
    #[must_use]
    pub fn has_active_plan_apply(&self, operation_id: &str) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.iter().any(|task| {
            !task.status.is_terminal()
                && matches!(
                    &task.kind,
                    ProcTaskKind::ApplyIngestPlan { operation_id: id, .. } if id == operation_id
                )
        })
    }

    /// 入队一件（状态 Queued），返回稳定 id。
    pub fn enqueue(&self, kind: ProcTaskKind, label: String) -> u64 {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let id = inner.next_id;
        inner.next_id += 1;
        inner.items.push_back(ProcTask {
            id,
            kind,
            label,
            status: ProcStatus::Queued,
            executed_by: None,
            queue: None,
            history_id: None,
        });
        inner.touch();
        id
    }

    /// 写盘前持久化知识意图。持久化失败会回滚内存项并返回错误，调用方不得继续写文件。
    pub fn prepare_knowledge(&self, kind: ProcTaskKind, label: String) -> Result<u64, String> {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if !matches!(kind, ProcTaskKind::KnowledgeLifecycle { .. }) {
            return Err("KNOWLEDGE_INTENT_INVALID".to_owned());
        }
        let id = inner.next_id;
        inner.next_id += 1;
        inner.items.push_back(ProcTask {
            id,
            kind,
            label,
            status: ProcStatus::Prepared,
            executed_by: None,
            queue: None,
            history_id: None,
        });
        inner.touch();
        let pending = inner
            .items
            .iter()
            .filter(|it| !it.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        let Some(path) = inner.persist_path.clone() else {
            inner.items.pop_back();
            return Err("KNOWLEDGE_INTENT_NO_VAULT".to_owned());
        };
        if let Err(error) = save_state_checked(&path, &pending) {
            inner.items.retain(|task| task.id != id);
            return Err(format!("KNOWLEDGE_INTENT_PERSIST_FAILED: {error}"));
        }
        inner.persisted_rev = inner.structural_rev;
        Ok(id)
    }

    /// 文件原子写成功后才把同一意图推进到可运行态。
    pub fn mark_knowledge_written(&self, id: u64) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let Some(task) = inner
            .items
            .iter_mut()
            .find(|task| task.id == id && matches!(task.status, ProcStatus::Prepared))
        else {
            return Err("KNOWLEDGE_INTENT_NOT_PREPARED".to_owned());
        };
        task.status = ProcStatus::Queued;
        inner.touch();
        let pending = inner
            .items
            .iter()
            .filter(|it| !it.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        let path = inner
            .persist_path
            .clone()
            .ok_or_else(|| "KNOWLEDGE_INTENT_NO_VAULT".to_owned())?;
        match save_state_checked(&path, &pending) {
            Ok(()) => inner.persisted_rev = inner.structural_rev,
            Err(error) => {
                // 文件已写成功；旧续跑档仍是 Prepared，重启会按文件指纹恢复，本进程则按
                // Queued 继续。此刻向用户报“保存失败”会与磁盘事实相反。
                eprintln!("[knowledge] 提交态持久化失败，将以内存队列继续：{error}");
            }
        }
        Ok(())
    }

    /// 知识文件未能落盘时撤销尚未提交的意图。只允许撤销 `Prepared`，避免调用方
    /// 误删已经开始维护的任务；撤销本身也同步持久化，重启后不会复活一件不存在的产物。
    pub fn abandon_prepared_knowledge(&self, id: u64) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let before = inner.items.len();
        inner
            .items
            .retain(|task| task.id != id || !matches!(task.status, ProcStatus::Prepared));
        if inner.items.len() == before {
            return;
        }
        inner.touch();
        let pending = inner
            .items
            .iter()
            .filter(|task| !task.status.is_terminal())
            .cloned()
            .collect::<Vec<_>>();
        if let Some(path) = inner.persist_path.clone() {
            if save_state_checked(&path, &pending).is_ok() {
                inner.persisted_rev = inner.structural_rev;
            }
        }
    }

    /// 单项快照（终态落盘时取展示字段用）。
    #[must_use]
    pub fn task_dto(&self, id: u64) -> Option<ProcTaskDto> {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .iter()
            .find(|it| it.id == id)
            .map(ProcTask::to_dto)
    }

    /// 非成功终态落盘后回写历史行 id（逐条移除据此删行）。
    pub fn attach_history_id(&self, id: u64, history_id: i64) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(it) = inner.items.iter_mut().find(|it| it.id == id) {
            it.history_id = Some(history_id);
        }
    }

    /// 某项对应的历史行 id（移除前取）。
    #[must_use]
    pub fn history_id_of(&self, id: u64) -> Option<i64> {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .iter()
            .find(|it| it.id == id)
            .and_then(|it| it.history_id)
    }

    /// 用上次会话的非成功终态回填面板（PRD-04-062；绑库时调用）：先清掉已有回填项，再按
    /// 时间正序追加为不可重试的终态。**不进续跑档**（终态本就不存），不影响 worker。
    pub fn restore_history(&self, records: Vec<RestoredOutcome>) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .retain(|it| !matches!(it.kind, ProcTaskKind::History { .. }));
        for record in records {
            let id = inner.next_id;
            inner.next_id += 1;
            let status = match record.code {
                Some(code) => ProcStatus::Failed {
                    code,
                    retryable: false,
                    retry_charges_again: false,
                },
                None => ProcStatus::Cancelled,
            };
            inner.items.push_back(ProcTask {
                id,
                kind: ProcTaskKind::History {
                    tag: record.tag,
                    note_rel: record.note_rel,
                },
                label: record.label,
                status,
                executed_by: None,
                queue: None,
                history_id: Some(record.history_id),
            });
        }
        inner.touch();
    }

    /// 当前队列快照（入队序）。
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProcTaskDto> {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .iter()
            .filter(|task| !task.is_hidden())
            .map(ProcTask::to_dto)
            .collect()
    }

    /// 该笔记是否已有**未终态**（排队/运行中）的指定类型任务（去重防重复入队）。
    #[must_use]
    pub fn has_active(&self, note_rel: &str, tag: &str) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.iter().any(|it| {
            !it.status.is_terminal() && it.kind.tag() == tag && it.kind.note_rel() == Some(note_rel)
        })
    }

    /// 这条标签的新主题初稿是否已在排队（尚未开始）。
    #[must_use]
    pub fn has_queued_topic_draft(&self, tag: &str) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.iter().any(|it| {
            it.status.is_queued()
                && matches!(&it.kind, ProcTaskKind::PrepareTopicDraft { tag: queued, .. }
                    if queued.eq_ignore_ascii_case(tag))
        })
    }

    /// 该笔记是否已有**尚未开始**的指定类型任务。
    ///
    /// 给「执行时才读现场」的任务去重用：排着的那件开跑时自会读到最新状态，可以合并；
    /// 已在执行的那件读的是旧状态，不能拿来吞掉之后的新触发。
    #[must_use]
    pub fn has_queued(&self, note_rel: &str, tag: &str) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.iter().any(|it| {
            it.status.is_queued() && it.kind.tag() == tag && it.kind.note_rel() == Some(note_rel)
        })
    }

    /// 队列里是否还有**未终态**的指定类型任务（不限笔记）。
    ///
    /// 批量确认卡据此如实说「还有 N 件在队列里」——运行态的真值只有队列一处，模块自持
    /// 一份运行标志必然与它漂移。
    #[must_use]
    pub fn has_active_kind(&self, tag: &str) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .iter()
            .any(|it| !it.status.is_terminal() && it.kind.tag() == tag)
    }

    /// worker 单步：**同一把锁内**取下一件 Queued（置 Running{初始阶段}）或释放 worker
    /// ——消除「取尽后释放前有新入队却认领失败」的竞态（入队 + `try_claim_worker` 同锁串行）。
    /// 一并交出该项的取消令牌（worker 传给 `process_*`，用户取消即传播到 whisper abort）。
    pub fn next_or_release(&self) -> Option<(u64, ProcTaskKind, Arc<TaskCancel>)> {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        // 交接暂停中不领新任务：让 worker 退出，待 `rebind` 后由切库编排重新拉起。
        if inner.handover {
            inner.worker_running = false;
            inner.running = None;
            self.settled.notify_all();
            return None;
        }
        let mut picked = None;
        for it in inner.items.iter_mut() {
            if it.status.is_queued() {
                it.status = ProcStatus::Running {
                    step: it.kind.initial_step(),
                    done: 0,
                    total: 0,
                };
                picked = Some((it.id, it.kind.clone()));
                break;
            }
        }
        if let Some((id, kind)) = picked {
            let cancel = Arc::new(TaskCancel::default());
            inner.running = Some((id, Arc::clone(&cancel)));
            inner.touch();
            return Some((id, kind, cancel));
        }
        inner.worker_running = false;
        inner.running = None;
        None
    }

    /// 请求取消**运行中**的某项（PRD-05-010）：置位其令牌，一路传播到 whisper 解码边界。
    /// 返回是否发出了取消（该项非运行中 → `false`，调用方应改走 `remove`）。
    pub fn request_cancel(&self, id: u64) -> bool {
        let inner = self.inner.lock().expect("processing queue poisoned");
        let running_matches = inner
            .items
            .iter()
            .any(|it| it.id == id && it.status.is_running());
        if !running_matches {
            return false;
        }
        match inner.running.as_ref() {
            Some((running_id, cancel)) if *running_id == id => {
                cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// 重跑一件**失败/取消**的任务（PRD-05-010）：原地回到队尾排队态（保留 id 与载荷）。
    /// 返回是否发生重排（已完成/排队中/运行中 → `false`）。
    pub fn retry(&self, id: u64) -> bool {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let Some(pos) = inner
            .items
            .iter()
            // 回填的历史项没有载荷，原样重跑无从谈起（PRD-04-062 刻意不做失败自动重试）。
            .position(|it| {
                it.id == id
                    && it.status.is_retryable()
                    && !matches!(it.kind, ProcTaskKind::History { .. })
            })
        else {
            return false;
        };
        let Some(mut task) = inner.items.remove(pos) else {
            return false;
        };
        task.status = ProcStatus::Queued;
        inner.items.push_back(task);
        inner.touch();
        true
    }

    /// 更新某项运行进度（阶段 + 计数）。仅在该项仍处运行态时生效。
    pub fn set_running(&self, id: u64, step: ProcPhase, done: u32, total: u32) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(it) = inner.items.iter_mut().find(|it| it.id == id) {
            it.status = ProcStatus::Running { step, done, total };
        }
    }

    /// 记下这次任务的实际执行者（转写路径在拿到 `JobEvent::ExecutedBy` 后调用）。
    ///
    /// `None` = 本机跑的：保持默认，**不写一个"本机"的伪记录**——把推断出来的东西
    /// 写成事实，日后就分不清「确实是本机」与「没人告诉过我们」。
    pub fn set_executor(&self, id: u64, executor: Option<luna_core::ExecutorInfo>) {
        let Some(executor) = executor else {
            return;
        };
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(it) = inner.items.iter_mut().find(|it| it.id == id) {
            it.executed_by = Some(executor.into());
        }
        inner.touch();
    }

    /// 记下这件任务在**服务端队列**里的排位（ADR-049 ⑧），并把阶段切到
    /// [`ProcPhase::AwaitingServer`]。
    ///
    /// 阶段与排位一起置：只更新排位而让阶段停在 `Transcribing`，面板上会写着
    /// 「转写中」而实际上服务端一个字节都还没开始解——那正是这条事件要纠正的误述。
    ///
    /// `None` = 已经不在服务端队列里（开跑了）：清掉排位，但**不动阶段**——真正的
    /// 阶段由后续的进度回调给出，在这里猜一个会与它打架。
    pub fn set_queue(&self, id: u64, queue: Option<luna_core::RemoteQueueStatus>) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(it) = inner.items.iter_mut().find(|it| it.id == id) {
            it.queue = queue.map(Into::into);
            if it.queue.is_some() {
                if let ProcStatus::Running { done, total, .. } = it.status {
                    it.status = ProcStatus::Running {
                        step: ProcPhase::AwaitingServer,
                        done,
                        total,
                    };
                }
            }
        }
        inner.touch();
    }

    /// 置某项终态（Done/Failed/Cancelled）。
    pub fn set_status(&self, id: u64, status: ProcStatus) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if let Some(it) = inner.items.iter_mut().find(|it| it.id == id) {
            it.status = status;
        }
        if inner.running.as_ref().is_some_and(|(rid, _)| *rid == id) {
            inner.running = None;
            // 运行项落定：唤醒可能正在定界等待的切库交接。
            self.settled.notify_all();
        }
        inner.touch();
    }

    /// 开始交接（ADR-025 / TDD-06 §12）：此后 worker 不再领取新任务。
    ///
    /// 取消只能拦住**运行中那一件**——旧 worker 在它落定后会立刻领取下一件旧库任务，
    /// 而那件任务的写盘会撞上换根的瞬间。暂停领取 + 定界等待运行项落定，才是完整的
    /// "队列真的静下来了"。
    pub fn begin_handover(&self) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        inner.handover = true;
    }

    /// 回退交接（等待超时、切库中止时调用）：恢复领取，旧库照常运转。
    pub fn end_handover(&self) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        inner.handover = false;
    }

    /// 定界等待运行项落定（`running == None`）。返回是否在时限内落定。
    ///
    /// 取消是协作式的（whisper 在包边界 abort、LLM Job 经 CancelToken 中断），通常
    /// 亚秒级；等不到就**不换根**——顺序契约是任何一步失败都停在旧库完好态。
    #[must_use]
    pub fn wait_running_settled(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        while inner.running.is_some() {
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _timeout) = self
                .settled
                .wait_timeout(inner, deadline - now)
                .expect("processing queue poisoned");
            inner = guard;
        }
        true
    }

    /// 排队项上/下移一位：只与**相邻排队项**交换（运行中/终态项不参与，位置不变）。
    /// 返回是否发生移动（目标项非排队态或已在端点 → `false`）。
    pub fn reorder(&self, id: u64, up: bool) -> bool {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let queued: Vec<usize> = inner
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.status.is_queued())
            .map(|(i, _)| i)
            .collect();
        let Some(pos) = queued.iter().position(|&i| inner.items[i].id == id) else {
            return false;
        };
        let swap_pos = if up {
            pos.checked_sub(1)
        } else {
            (pos + 1 < queued.len()).then_some(pos + 1)
        };
        let Some(swap_pos) = swap_pos else {
            return false;
        };
        inner.items.swap(queued[pos], queued[swap_pos]);
        inner.touch();
        true
    }

    /// 移除一个**排队项或未成功项**（失败 / 取消）。返回是否移除。
    ///
    /// 运行中不可移除（那是「取消」的事，语义是中断而非丢弃）；已完成项不逐条移除
    /// ——它们只由「清除已完成」整段清走（PRD-04-055 三段修订）。
    ///
    /// 未成功项落盘之后（PRD-04-062）：本处移除必须同时删掉持久化记录，否则
    /// 重启即复活——届时改这里，不另开第二条「删历史」命令。
    pub fn remove(&self, id: u64) -> bool {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        let Some(pos) = inner
            .items
            .iter()
            .position(|it| it.id == id && it.status.is_removable())
        else {
            return false;
        };
        inner.items.remove(pos);
        inner.touch();
        true
    }

    /// 清走全部**已完成**项（前端「清除已完成」）。
    ///
    /// 只清成功项，失败 / 取消一律留下：它们是待重试的工作，不是历史。此前这条叫
    /// `clear_finished` 且清的是全部终态——用户按字面理解「清除已完成」只收走绿勾，
    /// 实际把两条待重试的失败一并抹掉，且无确认、不可撤销（真机 2026-09-07）。
    pub fn clear_done(&self) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        inner
            .items
            .retain(|it| !matches!(it.status, ProcStatus::Done { .. }));
        inner.touch();
    }

    /// 清走全部**未成功**项（失败 / 取消；前端「清除未成功」，须二次确认）。
    pub fn clear_unsuccessful(&self) {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        inner.items.retain(|it| !it.status.is_unsuccessful());
        inner.touch();
    }

    /// 尝试认领 worker：无 worker 在跑则置位并返回 `true`（调用方负责启动 worker）。
    pub fn try_claim_worker(&self) -> bool {
        let mut inner = self.inner.lock().expect("processing queue poisoned");
        if inner.worker_running {
            false
        } else {
            inner.worker_running = true;
            true
        }
    }
}

/// 读续跑档（缺失/损坏 → 空队列；绝不 panic、绝不阻塞启动）。
///
/// **逐件解析**：认不出的单件（例如已退役的任务类型）只跳过它自己。整份一起解析的话，
/// 一件认不出就会连带丢掉同档里已确认的「应用到本页」意图——那是用户点过确认、Rust 承诺
/// 过会执行的事。跳过的件数只记数量，不记内容。
fn load_state(path: &Path) -> Vec<ProcTask> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return Vec::new();
    };
    let total = entries.len();
    let mut items: Vec<ProcTask> = entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect();
    if items.len() < total {
        eprintln!(
            "[processing] 续跑档里有 {} 件任务无法识别，已跳过",
            total - items.len()
        );
    }
    // 运行中的任务无法从中途续（whisper 无检查点）→ 回到排队重跑；终态不入档故无需过滤。
    for it in &mut items {
        if !it.status.is_queued() && !matches!(it.status, ProcStatus::Prepared) {
            it.status = ProcStatus::Queued;
        }
        if matches!(it.status, ProcStatus::Prepared) {
            it.status = ProcStatus::Queued;
        }
    }
    items
}

fn save_state_checked(path: &Path, items: &[ProcTask]) -> std::io::Result<()> {
    if items.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    if let Ok(dir) = std::fs::File::open(parent) {
                        let _ = dir.sync_all();
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(items).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    luna_vault::atomic::write_atomic(path, &json).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(name: &str) -> ProcTaskKind {
        ProcTaskKind::Import {
            path: PathBuf::from(format!("/src/{name}")),
            folder: None,
            link_from: None,
        }
    }

    /// 导入任务的目标文件夹是可缺省的：存档里没有这个字段的导入任务照常读回、落默认文件夹。
    #[test]
    fn an_import_task_without_a_folder_reads_back_with_the_default_folder() {
        let kind: ProcTaskKind =
            serde_json::from_str(r#"{"kind":"import","path":"/src/a.mp4"}"#).unwrap();
        assert!(matches!(kind, ProcTaskKind::Import { folder: None, .. }));
        let into_project = ProcTaskKind::Import {
            path: PathBuf::from("/src/a.mp4"),
            folder: Some("Creator/第一期".into()),
            link_from: Some("Creator/第一期/第一期-研究.md".into()),
        };
        let json = serde_json::to_string(&into_project).unwrap();
        let back: ProcTaskKind = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, ProcTaskKind::Import { folder: Some(folder), .. } if folder == "Creator/第一期")
        );
    }

    /// 「静着」= 没有运行项也没有排队项：排队、运行中都不算；落定之后才算（终态项不挡）。
    #[test]
    fn the_queue_is_quiet_only_with_nothing_queued_or_running() {
        let q = ProcessingQueue::default();
        assert!(q.is_quiet());
        q.enqueue(import("a"), "a".into());
        assert!(!q.is_quiet(), "排队中");
        let (id, _, _) = q.next_or_release().unwrap();
        assert!(!q.is_quiet(), "运行中");
        q.set_status(id, ProcStatus::Cancelled);
        assert!(q.is_quiet(), "落定之后");
    }

    /// 切库交接（ADR-025 / TDD-06 §12）：暂停期间 worker 不得领取新任务——取消只拦住
    /// 运行中那一件，不暂停领取的话旧 worker 会立刻抓起下一件旧库任务，写盘撞上换根。
    #[test]
    fn handover_pauses_pickup_until_rebind() {
        let q = ProcessingQueue::default();
        q.enqueue(import("a"), "a".into());
        q.enqueue(import("b"), "b".into());
        q.begin_handover();
        assert!(q.next_or_release().is_none(), "交接期间不得领取新任务");
        // 无运行项时定界等待立即成立（发完取消不必干等）。
        assert!(q.wait_running_settled(std::time::Duration::from_millis(10)));
        // 回退交接（等待超时、切库中止）：恢复领取，旧库照常运转。
        q.end_handover();
        assert!(q.next_or_release().is_some(), "回退后必须恢复领取");
    }

    /// 定界等待：运行项在另一线程落定（`set_status`）时等待方被唤醒；等不到则如实
    /// 返回 `false`——顺序契约是失败停在旧库完好态，绝不带着未落定的任务换根。
    #[test]
    fn wait_running_settled_wakes_on_terminal_status() {
        use std::sync::Arc;
        let q = Arc::new(ProcessingQueue::default());
        q.enqueue(import("a"), "a".into());
        assert!(q.try_claim_worker());
        let (id, _, _) = q.next_or_release().expect("领取运行项");
        // 运行项未落定：短等待必须诚实超时。
        assert!(!q.wait_running_settled(std::time::Duration::from_millis(30)));
        let q2 = Arc::clone(&q);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            q2.set_status(id, ProcStatus::Cancelled);
        });
        assert!(
            q.wait_running_settled(std::time::Duration::from_secs(5)),
            "运行项落定必须唤醒等待方"
        );
        handle.join().unwrap();
    }

    #[test]
    fn enqueue_snapshot_and_take_next_sets_initial_phase() {
        let q = ProcessingQueue::default();
        q.enqueue(import("x.mp3"), "x.mp3".into());
        q.enqueue(
            ProcTaskKind::Translate {
                note_rel: "R/a.md".into(),
                target_lang: "en".into(),
            },
            "a".into(),
        );
        let snap = q.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].kind, "import");
        assert_eq!(snap[0].status, ProcStatus::Queued);

        let (id, kind, _cancel) = q.next_or_release().unwrap();
        assert!(matches!(kind, ProcTaskKind::Import { .. }));
        // 导入初始阶段 = Decoding。
        assert_eq!(
            q.snapshot()[0].status,
            ProcStatus::Running {
                step: ProcPhase::Decoding,
                done: 0,
                total: 0
            }
        );
        q.set_running(id, ProcPhase::Transcribing, 1, 3);
        assert_eq!(
            q.snapshot()[0].status,
            ProcStatus::Running {
                step: ProcPhase::Transcribing,
                done: 1,
                total: 3
            }
        );
        q.set_status(
            id,
            ProcStatus::Done {
                awaiting_review: 0,
                note_rel: Some("R/x.md".into()),
            },
        );
        assert!(q.snapshot()[0].status.is_terminal());
        assert_eq!(q.snapshot()[0].note_rel.as_deref(), Some("R/x.md"));
    }

    #[test]
    fn translate_next_sets_translating_phase_and_note_rel() {
        let q = ProcessingQueue::default();
        q.enqueue(
            ProcTaskKind::Retranscribe {
                note_rel: "R/a.md".into(),
                translate_to: None,
            },
            "a".into(),
        );
        // 运行前即归属笔记（内联状态可按 note_rel 过滤）。
        assert_eq!(q.snapshot()[0].note_rel.as_deref(), Some("R/a.md"));
        q.next_or_release().unwrap();
        assert_eq!(
            q.snapshot()[0].status,
            ProcStatus::Running {
                step: ProcPhase::Transcribing,
                done: 0,
                total: 0
            }
        );
    }

    #[test]
    fn reorder_swaps_adjacent_queued_only() {
        let q = ProcessingQueue::default();
        let a = q.enqueue(import("a"), "a".into());
        let b = q.enqueue(import("b"), "b".into());
        let c = q.enqueue(import("c"), "c".into());
        // a 下移一位 → [b, a, c]。
        assert!(q.reorder(a, false));
        let snap = q.snapshot();
        let ids: Vec<&str> = snap.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
        // c 下移越界 → 不动。
        assert!(!q.reorder(c, false));
        // 让 b 进入运行态后，运行项不参与调序：a 上移应与「排队序列里的前一项」交换——
        // 运行的 b 不在排队集，故 a 已是排队首，无法上移。
        let (running_id, _, _) = q.next_or_release().unwrap();
        assert_eq!(running_id, b, "取到的运行项是当前排队首 b");
        assert!(!q.reorder(a, true), "a 已是排队首，无法上移越过运行项");
    }

    #[test]
    fn remove_accepts_queued_and_unsuccessful_but_not_running_or_done() {
        let q = ProcessingQueue::default();
        let a = q.enqueue(import("a"), "a".into());
        q.enqueue(import("b"), "b".into());
        // 移除排队项 a。
        assert!(q.remove(a));
        assert_eq!(q.snapshot().len(), 1);
        // 运行项不可移除。
        let (rid, _, _) = q.next_or_release().unwrap();
        assert!(!q.remove(rid), "运行项不可移除");
        // 失败项可逐条移除（不打算重试的那种）。
        q.set_status(
            rid,
            ProcStatus::Failed {
                code: "X".into(),
                retryable: true,
                retry_charges_again: false,
            },
        );
        assert!(q.remove(rid), "失败项可逐条移除");
        // 已完成项不逐条移除：它只由「清除已完成」整段清走。
        let c = q.enqueue(import("c"), "c".into());
        q.set_status(
            c,
            ProcStatus::Done {
                note_rel: None,
                awaiting_review: 0,
            },
        );
        assert!(!q.remove(c), "已完成项不逐条移除");
    }

    /// 「清除已完成」只收走绿勾：失败与取消是待重试的工作，必须留下。
    #[test]
    fn clear_done_keeps_failed_cancelled_and_active() {
        let q = ProcessingQueue::default();
        let done = q.enqueue(import("done"), "done".into());
        let failed = q.enqueue(import("failed"), "failed".into());
        let cancelled = q.enqueue(import("cancelled"), "cancelled".into());
        let queued = q.enqueue(import("queued"), "queued".into());
        q.set_status(
            done,
            ProcStatus::Done {
                note_rel: None,
                awaiting_review: 0,
            },
        );
        q.set_status(
            failed,
            ProcStatus::Failed {
                code: "X".into(),
                retryable: true,
                retry_charges_again: false,
            },
        );
        q.set_status(cancelled, ProcStatus::Cancelled);
        q.clear_done();
        let ids: Vec<u64> = q
            .snapshot()
            .iter()
            .map(|it| it.id.parse::<u64>().unwrap())
            .collect();
        assert_eq!(ids, vec![failed, cancelled, queued], "只清成功项");
    }

    /// 「清除未成功」只收走失败与取消：已完成与进行中都不动。
    #[test]
    fn clear_unsuccessful_keeps_done_and_active() {
        let q = ProcessingQueue::default();
        let done = q.enqueue(import("done"), "done".into());
        let failed = q.enqueue(import("failed"), "failed".into());
        let cancelled = q.enqueue(import("cancelled"), "cancelled".into());
        let queued = q.enqueue(import("queued"), "queued".into());
        q.set_status(
            done,
            ProcStatus::Done {
                note_rel: None,
                awaiting_review: 0,
            },
        );
        q.set_status(
            failed,
            ProcStatus::Failed {
                code: "X".into(),
                retryable: false,
                retry_charges_again: false,
            },
        );
        q.set_status(cancelled, ProcStatus::Cancelled);
        q.clear_unsuccessful();
        let ids: Vec<u64> = q
            .snapshot()
            .iter()
            .map(|it| it.id.parse::<u64>().unwrap())
            .collect();
        assert_eq!(ids, vec![done, queued], "只清失败与取消");
    }

    #[test]
    fn cancel_only_targets_the_running_item_and_propagates_to_its_token() {
        let q = ProcessingQueue::default();
        let a = q.enqueue(import("a"), "a".into());
        let b = q.enqueue(import("b"), "b".into());
        // 排队项不可「取消」（语义是移除，不是中断运行）。
        assert!(!q.request_cancel(a), "排队项不走取消路径");

        let (running, _, cancel) = q.next_or_release().unwrap();
        assert_eq!(running, a);
        assert!(!cancel.is_cancelled());
        // 取消运行项 → 令牌置位（worker 与 pipeline 据此在阶段边界短路）。
        assert!(q.request_cancel(a));
        assert!(cancel.is_cancelled());
        assert!(cancel.bail_if_cancelled().is_err());
        // 另一件仍在排队，不受影响。
        assert!(!q.request_cancel(b));
    }

    #[test]
    fn retry_requeues_only_failed_or_cancelled() {
        let q = ProcessingQueue::default();
        let a = q.enqueue(import("a"), "a".into());
        let b = q.enqueue(import("b"), "b".into());
        assert!(!q.retry(a), "排队项无需重试");

        q.next_or_release().unwrap();
        assert!(!q.retry(a), "运行中不可重试");
        q.set_status(
            a,
            ProcStatus::Failed {
                code: "X".into(),
                retryable: true,
                retry_charges_again: false,
            },
        );
        assert!(q.retry(a), "失败项可重试");
        // 回到队尾排队：b 先跑。
        let labels: Vec<String> = q.snapshot().into_iter().map(|d| d.label).collect();
        assert_eq!(labels, vec!["b", "a"]);
        assert_eq!(q.snapshot()[1].status, ProcStatus::Queued);

        q.set_status(b, ProcStatus::Cancelled);
        assert!(q.retry(b), "取消项同样可重试");
        q.set_status(
            b,
            ProcStatus::Done {
                awaiting_review: 0,
                note_rel: Some("T/b.md".into()),
            },
        );
        assert!(!q.retry(b), "已完成不重复做");
    }

    #[test]
    fn persist_round_trip_keeps_pending_and_requeues_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);

        let q = ProcessingQueue::new(Some(path.clone()));
        q.enqueue(import("pending.mp3"), "pending.mp3".into());
        let running = q.enqueue(import("running.mp3"), "running.mp3".into());
        let done = q.enqueue(import("done.mp3"), "done.mp3".into());
        q.set_status(
            done,
            ProcStatus::Done {
                note_rel: None,
                awaiting_review: 0,
            },
        );
        q.next_or_release().unwrap(); // pending 进入运行态
        q.persist_if_changed();

        // 重启：终态不恢复；运行中的回到排队（whisper 无检查点，只能整件重跑）。
        let restored = ProcessingQueue::new(Some(path.clone()));
        let snap = restored.snapshot();
        let labels: Vec<String> = snap.iter().map(|d| d.label.clone()).collect();
        assert_eq!(labels, vec!["pending.mp3", "running.mp3"]);
        assert!(snap.iter().all(|d| d.status == ProcStatus::Queued));
        assert!(restored.has_pending());
        // id 不与恢复项碰撞。
        assert!(restored.enqueue(import("new"), "new".into()) > running);
    }

    #[test]
    fn failed_persistence_retries_without_another_queue_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("blocked");
        std::fs::write(&parent, b"not a directory").unwrap();
        let path = parent.join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        q.enqueue(import("pending.mp3"), "pending.mp3".into());
        q.persist_if_changed();
        assert!(!path.exists());

        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        q.persist_if_changed();
        assert!(path.is_file(), "写盘环境恢复后同一结构版本必须重试");
        let restored = ProcessingQueue::new(Some(path));
        assert_eq!(restored.snapshot().len(), 1);
        assert_eq!(restored.snapshot()[0].label, "pending.mp3");
    }

    #[test]
    fn failed_rebind_keeps_old_queue_and_vault_binding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("blocked");
        std::fs::write(&parent, b"not a directory").unwrap();
        let old_path = parent.join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(old_path.clone()));
        q.enqueue(import("old.mp3"), "old.mp3".into());
        q.begin_handover();
        assert!(q.rebind(None, 1).is_err());
        assert_eq!(q.snapshot().len(), 1, "失败不能清掉旧库内存项");
        assert!(q.inner.lock().unwrap().handover, "失败后仍暂停领取");
        assert_eq!(
            q.inner.lock().unwrap().persist_path.as_ref(),
            Some(&old_path),
            "失败不能把队列绑定到其他库"
        );

        q.end_handover();
        assert!(q.has_pending(), "切库中止后旧库任务可继续领取");
        std::fs::remove_file(&parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        q.begin_handover();
        q.rebind(None, 1).unwrap();
        assert!(q.snapshot().is_empty());
        let restored = ProcessingQueue::new(Some(old_path));
        assert_eq!(restored.snapshot().len(), 1);
        assert_eq!(restored.snapshot()[0].label, "old.mp3");
    }

    fn plan_apply(operation_id: &str) -> ProcTaskKind {
        ProcTaskKind::ApplyIngestPlan {
            operation_id: operation_id.into(),
            vault_generation: 5,
            target_rel: "Knowledge/项目.md".into(),
        }
    }

    #[test]
    fn plan_apply_intent_is_durable_hidden_deduplicated_and_body_free() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        let op = "01900000-0000-7000-8000-00000000a001";
        let id = q.enqueue_plan_apply(plan_apply(op), "项目".into()).unwrap();
        assert_eq!(
            q.enqueue_plan_apply(plan_apply(op), "项目".into()).unwrap(),
            id,
            "同一封存计划重复确认只保留一件"
        );
        assert!(q.snapshot().is_empty(), "应用意图不进任务面板");
        assert!(q.has_active_plan_apply(op));
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(stored.contains(op));
        assert!(!stored.contains("预算已批准"), "续跑档不复制声明正文");

        // 重启：同一 ID 回到排队，代际按新进程定基，不需要也不会重新调用模型。
        let restored = ProcessingQueue::new(Some(path.clone()));
        let (_, kind, _) = restored.next_or_release().expect("restored apply intent");
        match kind {
            ProcTaskKind::ApplyIngestPlan {
                operation_id,
                vault_generation,
                target_rel,
            } => {
                assert_eq!(operation_id, op);
                assert_eq!(vault_generation, 0);
                assert_eq!(target_rel, "Knowledge/项目.md");
            }
            other => panic!("unexpected restored task: {other:?}"),
        }
    }

    #[test]
    fn plan_apply_intent_rolls_back_when_it_cannot_be_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("blocked");
        std::fs::write(&parent, b"not a directory").unwrap();
        let q = ProcessingQueue::new(Some(parent.join(QUEUE_STATE_FILENAME)));
        let op = "01900000-0000-7000-8000-00000000a002";
        assert!(q.enqueue_plan_apply(plan_apply(op), "项目".into()).is_err());
        assert!(
            !q.has_active_plan_apply(op),
            "未落盘的确认不能留在内存里伪装已排队"
        );
        assert!(q.next_or_release().is_none());
        assert!(ProcessingQueue::new(None)
            .enqueue_plan_apply(plan_apply(op), "项目".into())
            .is_err());
        assert!(q.enqueue_plan_apply(import("x.mp3"), "x".into()).is_err());
    }

    #[test]
    fn topic_preparation_is_visible_deduplicable_and_rebased_on_vault_switch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        q.enqueue(
            ProcTaskKind::PrepareTopicPlan {
                topic_rel: "Knowledge/项目.md".into(),
                vault_generation: 7,
            },
            "项目".into(),
        );
        let snapshot = q.snapshot();
        assert_eq!(snapshot.len(), 1, "会调用模型的后台任务在面板可见");
        assert_eq!(snapshot[0].kind, "topic_prepare");
        assert!(q.has_active("Knowledge/项目.md", "topic_prepare"));
        assert!(
            q.has_queued("Knowledge/项目.md", "topic_prepare"),
            "排着的可合并新触发"
        );
        q.persist_if_changed();
        // 重启回到本库：代际按新进程定基，与生命周期、计划应用同一规则。
        let restored = ProcessingQueue::new(Some(path));
        let (_, kind, _) = restored.next_or_release().expect("restored preparation");
        assert!(matches!(
            kind,
            ProcTaskKind::PrepareTopicPlan {
                vault_generation: 0,
                ..
            }
        ));
        assert!(
            !restored.has_queued("Knowledge/项目.md", "topic_prepare"),
            "执行中的读的是旧现场，不吞之后的新触发"
        );
        assert!(restored.has_active("Knowledge/项目.md", "topic_prepare"));
    }

    #[test]
    fn an_unrecognised_task_in_the_resume_file_does_not_drop_the_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        let op = "01900000-0000-7000-8000-00000000a009";
        q.enqueue_plan_apply(plan_apply(op), "项目".into()).unwrap();
        q.enqueue(
            ProcTaskKind::PrepareTopicPlan {
                topic_rel: "Knowledge/项目.md".into(),
                vault_generation: 1,
            },
            "项目".into(),
        );
        q.persist_if_changed();
        // 模拟旧版本留下的、本版已退役的任务类型。
        let mut entries: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut retired = entries[0].clone();
        retired["kind"] =
            serde_json::json!({ "kind": "topic_draft", "tag": "项目", "notes": ["a.md"] });
        entries.insert(0, retired);
        std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();

        let restored = ProcessingQueue::new(Some(path));
        assert_eq!(restored.snapshot().len(), 1, "只跳过认不出的那一件");
        assert!(restored.has_active("Knowledge/项目.md", "topic_prepare"));
        assert!(
            restored.has_active_plan_apply(op),
            "已确认的应用意图不能被连带丢掉"
        );
    }

    #[test]
    fn checked_persistence_confirms_terminal_removal_before_cleanup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        let op = "01900000-0000-7000-8000-00000000a003";
        let id = q.enqueue_plan_apply(plan_apply(op), "项目".into()).unwrap();
        q.next_or_release().unwrap();
        q.set_status(
            id,
            ProcStatus::Done {
                note_rel: Some("Knowledge/项目.md".into()),
                awaiting_review: 0,
            },
        );
        q.persist_checked().unwrap();
        assert!(
            !path.exists(),
            "终态不入续跑档：确认落盘后才允许清理封存载荷"
        );
        assert!(!q.has_active_plan_apply(op));
        assert_eq!(
            q.plan_apply_state(op),
            Some(PlanApplyState::Applied),
            "成功终态：载荷清理前的空隙里也不能被当成待确认"
        );
        assert!(ProcessingQueue::new(None).persist_checked().is_err());

        // 失败终态报告 Failed；再次确认入队的新一件覆盖为 Active。
        let failed_op = "01900000-0000-7000-8000-00000000a004";
        let failed = q
            .enqueue_plan_apply(plan_apply(failed_op), "项目".into())
            .unwrap();
        assert_eq!(q.plan_apply_state(failed_op), Some(PlanApplyState::Active));
        q.next_or_release().unwrap();
        q.set_status(
            failed,
            ProcStatus::Failed {
                code: "KNOWLEDGE_APPLY_UNAVAILABLE".into(),
                retryable: true,
                retry_charges_again: false,
            },
        );
        assert_eq!(q.plan_apply_state(failed_op), Some(PlanApplyState::Failed));
        q.enqueue_plan_apply(plan_apply(failed_op), "项目".into())
            .unwrap();
        assert_eq!(q.plan_apply_state(failed_op), Some(PlanApplyState::Active));
    }

    #[test]
    fn knowledge_intent_is_durable_before_write_and_recovers_without_ui_duplication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        let id = q
            .prepare_knowledge(
                ProcTaskKind::KnowledgeLifecycle {
                    operation_id: "01900000-0000-7000-8000-000000000001".into(),
                    vault_generation: 7,
                    note_rel: "Notes/A.md".into(),
                    body_hash: "hash".into(),
                    origin: "distill".into(),
                    aggregate: false,
                },
                "A".into(),
            )
            .expect("prepare persisted");
        assert!(path.exists(), "prepare 返回前续跑档必须已经存在");
        assert!(
            !q.has_pending(),
            "同进程不得在 Markdown 写成前领取 Prepared"
        );
        assert!(q.snapshot().is_empty(), "内部生命周期不污染用户任务面板");

        let restored = ProcessingQueue::new(Some(path));
        assert!(
            restored.has_pending(),
            "崩溃恢复后由文件指纹门控决定继续或放弃"
        );
        assert!(restored.snapshot().is_empty(), "恢复后仍是隐藏任务");
        let (_, kind, _) = restored.next_or_release().expect("restored lifecycle");
        assert!(matches!(
            kind,
            ProcTaskKind::KnowledgeLifecycle {
                vault_generation: 0,
                ..
            }
        ));
        assert!(id > 0);
    }

    #[test]
    fn progress_updates_do_not_dirty_the_resume_file() {
        // 进度每秒数次，若计入结构性版本就会把续跑档写成高频 IO。
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        let id = q.enqueue(import("a.mp3"), "a.mp3".into());
        q.next_or_release().unwrap();
        q.persist_if_changed();
        let first = std::fs::read_to_string(&path).expect("state written");

        q.set_running(id, ProcPhase::Transcribing, 3, 10);
        q.persist_if_changed();
        assert_eq!(
            std::fs::read_to_string(&path).expect("state"),
            first,
            "仅进度变化不应重写续跑档"
        );

        // 结构性变化（终态）才重写——且终态项不入档，队列清空即删档。
        q.set_status(
            id,
            ProcStatus::Done {
                note_rel: None,
                awaiting_review: 0,
            },
        );
        q.persist_if_changed();
        assert!(!path.exists(), "无未完成项时删除续跑档，不留空文件");
    }

    /// 抓取任务的队列语义（PRD-05-006）：起始阶段是 `Fetching`（不是 `Decoding`——此刻文件
    /// 还不存在）、完成前无归属笔记、载荷里的 URL 与标题**原样存续**（重试要能重新下载，
    /// 而标题是队列标签与笔记名的来源）。
    #[test]
    fn fetch_task_starts_at_fetching_phase_and_keeps_payload() {
        let q = ProcessingQueue::default();
        let id = q.enqueue(
            ProcTaskKind::Fetch {
                url: "https://www.youtube.com/watch?v=abc".into(),
                title: "会议分享".into(),
                media: luna_fetch::FetchMediaKind::Video,
                into_note: None,
                folder: None,
                link_from: None,
            },
            "会议分享".into(),
        );
        let snap = q.snapshot();
        assert_eq!(snap[0].kind, "fetch");
        assert_eq!(snap[0].label, "会议分享");
        assert!(snap[0].note_rel.is_none(), "抓取完成建笔记前无归属");

        let (taken, kind, _cancel) = q.next_or_release().unwrap();
        assert_eq!(taken, id);
        match kind {
            ProcTaskKind::Fetch {
                url, title, media, ..
            } => {
                assert_eq!(url, "https://www.youtube.com/watch?v=abc");
                assert_eq!(title, "会议分享");
                assert_eq!(media, luna_fetch::FetchMediaKind::Video, "形态随任务走");
            }
            other => panic!("取到的不是抓取任务：{other:?}"),
        }
        assert_eq!(
            q.snapshot()[0].status,
            ProcStatus::Running {
                step: ProcPhase::Fetching,
                done: 0,
                total: 0
            },
            "抓取任务的第一个阶段是下载，而不是解码一个还不存在的文件"
        );
    }

    /// 站点侧硬事实（已下架 / 登录墙 / 直播）标 `retryable: false` 后，`retry()` 必须**拒绝**——
    /// 否则界面上那枚「重试」按钮点下去只是让同一件任务再失败一次（PRD-05-006）。
    #[test]
    fn unretryable_failure_refuses_requeue() {
        let q = ProcessingQueue::default();
        let hard = q.enqueue(
            ProcTaskKind::Fetch {
                url: "https://a.com/gone".into(),
                title: "已下架".into(),
                media: luna_fetch::FetchMediaKind::Audio,
                into_note: None,
                folder: None,
                link_from: None,
            },
            "已下架".into(),
        );
        let env = q.enqueue(import("a.mp3"), "a.mp3".into());

        q.set_status(
            hard,
            ProcStatus::Failed {
                code: "FETCH_UNAVAILABLE".into(),
                retryable: false,
                retry_charges_again: false,
            },
        );
        assert!(!q.retry(hard), "站点侧硬事实不该被重排进队列");
        // 环境类失败照旧可重试（装上工具 / 恢复网络后原样重跑就成）。
        q.set_status(
            env,
            ProcStatus::Failed {
                code: "FETCH_NETWORK".into(),
                retryable: true,
                retry_charges_again: false,
            },
        );
        assert!(q.retry(env));
        // 取消恒可重试（用户意图，环境未变）。
        let cancelled = q.enqueue(import("b.mp3"), "b.mp3".into());
        q.set_status(cancelled, ProcStatus::Cancelled);
        assert!(q.retry(cancelled));
    }

    /// 抓取任务必须跨重启续跑（一条链接下到一半关了应用，重开继续）——URL 与形态都在档里。
    #[test]
    fn fetch_task_survives_restart_with_url_and_media_kind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(QUEUE_STATE_FILENAME);
        let q = ProcessingQueue::new(Some(path.clone()));
        q.enqueue(
            ProcTaskKind::Fetch {
                url: "https://b.com/BV1".into(),
                title: "课程 01".into(),
                media: luna_fetch::FetchMediaKind::Audio,
                into_note: None,
                folder: None,
                link_from: None,
            },
            "课程 01".into(),
        );
        q.persist_if_changed();

        let restored = ProcessingQueue::new(Some(path));
        let (_, kind, _) = restored.next_or_release().expect("恢复出任务");
        match kind {
            ProcTaskKind::Fetch { url, media, .. } => {
                assert_eq!(url, "https://b.com/BV1");
                assert_eq!(media, luna_fetch::FetchMediaKind::Audio);
            }
            other => panic!("续跑档丢了抓取载荷：{other:?}"),
        }
    }

    #[test]
    fn worker_claim_is_exclusive_and_released_when_drained() {
        let q = ProcessingQueue::default();
        assert!(q.try_claim_worker());
        assert!(!q.try_claim_worker(), "已有 worker 时再认领失败");
        // 队列空 → next_or_release 释放认领。
        assert!(q.next_or_release().is_none());
        assert!(q.try_claim_worker(), "释放后可再认领");
    }
}
