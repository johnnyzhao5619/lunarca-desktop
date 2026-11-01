//! 实时转译管线（TDD-08 落地实现）。
//!
//! 职责（TDD-08 §3/§4）：
//! - **只翻译 committed 段**：输入为 LocalAgreement-2 已定稿段（永不回退），从源头杜绝译文抖动。
//! - **句界批量（MT 质量关键）**：[`SentenceBuffer`] 把 committed 文本累积到句界（终止标点 /
//!   字符上限）再触发一次翻译——半句不译、请求频率天然合并。
//! - **单一 AI 主干（ADR-001）**：每个句界单元经 [`JobSink`] 提交 `JobKind::Translate`，
//!   与弹窗翻译走同一 `JobQueue → Router → Provider`（随包引擎 `local-llm`），不另起调用栈。
//! - **上下文连贯**：滑动维护最近 N 句（源文 ⇒ 译文）经 `Translate.instruction` 附带，
//!   避免逐句割裂、指代断裂。
//! - **降级门控**：接受 [`crate::RealtimeSession`] 下发的挂起信号（`watch<bool>`，
//!   Yellow 及以上挂起）——挂起期句缓冲继续累积，恢复后补译积压，**不静默丢句**。
//! - **两阶段汇合**：录音停止时上游关闭输入（drop feed 端），worker flush 句缓冲余量并
//!   译完在途单元后自然收尾；上游带超时 drain 输出，超时未译段 `translation` 留 `None`
//!   （可二次补译，TDD-06 §2.2 诚实元数据）。
//!
//! 隐私铁律：本模块 stderr 诊断仅含错误码/计数，绝不含转写或译文正文。

use crate::JobSink;
use luna_core::cancel::TaskCancel;
use luna_core::glossary::{with_glossary_constraint, GlossaryEntry};
use luna_core::prompt::{Instruction, PromptRegistry, PromptScope};
use luna_core::{
    decide_branch, langs_equal, Job, JobEvent, JobKind, JobOrigin, Lang, LangContext,
    LanguageDetector, Priority, ResultPayload, Segment, SegmentId, TargetLangMode, TextRef,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;

// ─── 句界缓冲 ─────────────────────────────────────────────────────────────────

/// 句终止标点（中英混排；命中即视为句界，TDD-08 §3「翻译粒度 = 句界」）。
const SENTENCE_ENDINGS: &[char] = &['。', '．', '.', '!', '！', '?', '？', '…', ';', '；'];

/// 单元内的一个源段：id + 该段原文。
///
/// 两者都要带着走——译文按各段**原文长度占比**摊回（`luna_core::distribute_translation`），
/// 只有 id 就没法算占比，只有文本就写不回段。
#[derive(Clone, Debug, PartialEq)]
pub struct UnitPart {
    /// 段 id（译文回填锚点）。
    pub id: SegmentId,
    /// 该段原文（摊回权重）。
    pub text: String,
}

/// 一个待翻译的句界单元（一句或到达字符上限的一批 committed 段）。
#[derive(Clone, Debug, PartialEq)]
pub struct TranslateUnit {
    /// 组成本单元的 committed 段（译文按各段原文占比摊回，见 [`UnitPart`]）。
    pub parts: Vec<UnitPart>,
    /// 单元起点（首段 start，会话绝对时钟）。
    pub start: Duration,
    /// 单元止点（末段 end）。
    pub end: Duration,
    /// 拼接后的源文本。
    pub text: String,
    /// 音轨身份：`self` / `remote`；单轨会话为 `None`。
    ///
    /// 双源会话的句界与上下文必须按轨隔离，绝不能按异步事件到达顺序跨轨拼句。
    pub track: Option<String>,
}

/// 句界缓冲：committed 段 → 句界单元（纯状态机，headless 可测）。
pub struct SentenceBuffer {
    pending: Vec<(SegmentId, Duration, Duration, String)>,
    pending_chars: usize,
    /// 字符上限：长时无标点（口语/方言/ASR 掉标点）也按上限强制出单元，防积压不译。
    max_chars: usize,
}

impl SentenceBuffer {
    /// 以字符上限构造。
    #[must_use]
    pub fn new(max_chars: usize) -> Self {
        Self {
            pending: Vec::new(),
            pending_chars: 0,
            max_chars: max_chars.max(1),
        }
    }

    /// 喂入一个 committed 段；命中句界或达字符上限时返回句界单元。
    pub fn push(&mut self, seg: &Segment) -> Option<TranslateUnit> {
        let text = seg.text.trim();
        if text.is_empty() {
            return None;
        }
        self.pending
            .push((seg.id, seg.start, seg.end, text.to_string()));
        self.pending_chars += text.chars().count();

        let at_boundary = text
            .chars()
            .next_back()
            .is_some_and(|c| SENTENCE_ENDINGS.contains(&c));
        if at_boundary || self.pending_chars >= self.max_chars {
            self.drain_unit()
        } else {
            None
        }
    }

    /// 缓冲里还有没有未出单元的文本。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 流结束：把缓冲余量（未达句界的尾巴）作为最后一个单元产出。
    pub fn flush(&mut self) -> Option<TranslateUnit> {
        self.drain_unit()
    }

    fn drain_unit(&mut self) -> Option<TranslateUnit> {
        if self.pending.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending_chars = 0;
        let start = pending.first().map(|(_, s, _, _)| *s)?;
        let end = pending.last().map(|(_, _, e, _)| *e)?;
        Some(TranslateUnit {
            parts: pending
                .iter()
                .map(|(id, _, _, t)| UnitPart {
                    id: *id,
                    text: t.clone(),
                })
                .collect(),
            start,
            end,
            text: pending
                .iter()
                .map(|(_, _, _, t)| t.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            track: None,
        })
    }
}

// ─── 管线 ─────────────────────────────────────────────────────────────────────

/// 转译配置（**实时与批量共用**）。
///
/// **名字里没有「Realtime」是有意的**（ADR-083 ⑦）：它从一开始就同时服务实时逐句管线
/// 与批量循环（`translate_segments_batch`），而此前那个名字里唯一属于实时的东西——
/// `preferred_provider`（「实时管线传实时档 id，批量留 None」）——已随本批删除：档位改由
/// [`luna_core::Job::service_class`] 推导、由路由层按槽位总映射填充，调用点一律不传。
/// 留着旧名字只会让下一个人以为批量路径复用了一个「实时」配置是临时凑合。
///
/// 两条路真正不同的只有「喂给模型的是哪一段上下文、用哪一句指令」（见
/// [`live_instruction`] / [`document_instruction`]）；切句、按轨隔离、译文摊回三个纯函数
/// 仍共用。本结构体里唯一只对实时生效的字段是 [`Self::max_live_backlog`]，注已写明。
///
/// **语言决策与弹窗翻译同源（DRY，ADR-001）**：不再持固定目标语言，而是按句检测源语言 +
/// [`TargetLangMode`] + [`LangContext`] 解析**有效目标**（[`decide_branch`]）。默认
/// [`TargetLangMode::SmartBidirectional`]：源==母语→默认外语，否则→母语，从根上规避
/// 「源==目标」退化为同语改写（问题：中文语音开转译却仍输出中文）。
#[derive(Clone)]
pub struct TranslationConfig {
    /// 目标语言模式（默认智能双向）。
    pub mode: TargetLangMode,
    /// 语言基准（母语 + 默认外语，`popup_translate.native_lang` + `realtime.translate_target_lang`）。
    ///
    /// `Arc<RwLock>` 持有：录音/字幕面板可**现场热切**默认外语（[`set_foreign_lang`]），管线 worker
    /// 持同一 Arc，下一句界单元即按新目标解析——无需重启会话（PRD-04-041）。
    ///
    /// [`set_foreign_lang`]: TranslationConfig::set_foreign_lang
    ctx: Arc<RwLock<LangContext>>,
    /// 源语言检测器（注入 trait；具体实现在 `luna-provider`，避免把外部 crate 拖进本 crate）。
    pub detector: Arc<dyn LanguageDetector>,
    /// 上下文窗大小（最近 N 组「源文 ⇒ 译文」随 instruction 附带）。
    pub context_pairs: usize,
    /// 句缓冲字符上限（无标点长语流的强制出单元阈值）。
    pub max_pending_chars: usize,
    /// **停顿句界**：句缓冲里有文本、且这么久没有新的 committed 段进来，就把缓冲当作一句
    /// 冲出去翻译（TDD-08「标点 / 足够长停顿」的后一半）。
    ///
    /// 只靠标点的话，不出标点的转写模型（随包 Whisper tiny 的中文常见）一句都凑不成单元，
    /// 要攒满 [`Self::max_pending_chars`] 才译一次——实时字幕看起来就是「从不出译文」。
    /// committed 段本就落在 VAD 停顿处，一段停顿后没有下文，说明这句话已经说完。
    pub idle_flush: Duration,
    /// **实时视图积压上限（就近优先卸载，TDD-08 §4）**：本地 LLM 每句 1–3s，会议连续讲话
    /// 必然使待译句积压。实时管线保留最近至多 N 个待译单元，超出即丢弃**最旧**的积压句
    /// （实时视图只保证展示最新句译文，落后太多的中间句 `translation` 留 `None`——完整权威
    /// 双语由停录后二次转写补齐并存入笔记）。
    ///
    /// **本结构体里唯一只对实时管线生效的字段**：批量转译路径全量逐句译，不受它约束。
    pub max_live_backlog: usize,
    /// 术语表（PRD-01-017）：翻译时把**本句命中**的词条作为强制译法追加进 instruction，
    /// 保证跨段跨文档术语一致。**只注入命中项**——全表会让每句 prompt 膨胀、拖慢本地 LLM
    /// 并挤占上下文窗，而没出现的术语对本句毫无约束力。空表 = 不追加任何约束段。
    pub glossary: Arc<[GlossaryEntry]>,
}

impl TranslationConfig {
    /// 以「目标模式 + 语言基准 + 检测器」构造默认配置（上下文 2 句、240 字符上限、停顿 1.5 s 断句）。
    #[must_use]
    pub fn new(
        mode: TargetLangMode,
        ctx: LangContext,
        detector: Arc<dyn LanguageDetector>,
    ) -> Self {
        Self {
            mode,
            ctx: Arc::new(RwLock::new(ctx)),
            detector,
            context_pairs: 2,
            max_pending_chars: 240,
            idle_flush: Duration::from_millis(1500),
            max_live_backlog: 3,
            glossary: Arc::from(Vec::new()),
        }
    }

    /// 挂上术语表（PRD-01-017）：实时与批量转译共用同一份，注入逻辑亦同一处。
    #[must_use]
    pub fn with_glossary(mut self, glossary: Arc<[GlossaryEntry]>) -> Self {
        self.glossary = glossary;
        self
    }

    /// 现场热切默认外语（智能双向的「另一语」）：写共享语言基准，管线下一单元即生效。
    /// 中毒锁静默忽略（不 panic，最坏保持旧目标；转译非关键路径）。
    pub fn set_foreign_lang(&self, lang: Lang) {
        if let Ok(mut ctx) = self.ctx.write() {
            ctx.default_foreign = lang;
        }
    }

    /// 按源文本解析**有效目标语言**（检测源语言 → `decide_branch`）。
    /// 返回 `None` 表示源语言已是有效目标（无需翻译，展示原文即可，诚实元数据）。
    #[must_use]
    fn resolve_target(&self, text: &str) -> Option<TranslateTarget> {
        let detection = self.detector.detect(text);
        let ctx = match self.ctx.read() {
            Ok(ctx) => ctx.clone(),
            Err(_) => return None, // 锁中毒：本单元跳过翻译（诚实留 None，不 panic）
        };
        let branch = decide_branch(&detection, &self.mode, &ctx);
        // 源可靠且与有效目标同语 → 不翻译（live caption 绝不同语改写；批量场景留 None 待补）。
        if let Some(src) = branch.source.as_ref() {
            if langs_equal(src, &branch.target) {
                return None;
            }
        }
        Some(TranslateTarget {
            source: branch.source,
            target: branch.target,
        })
    }
}

/// 一个句界单元解析出的翻译方向（源语言可选 + 有效目标语言）。
struct TranslateTarget {
    source: Option<Lang>,
    target: Lang,
}

/// 已译完的句界单元（回填 `Segment.translation` + HUD 双语显示）。
#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedUnit {
    /// 组成单元的段（译文按各段原文占比**摊回到每一段**，见 [`UnitPart`]）。
    pub parts: Vec<UnitPart>,
    /// 单元起点（会话绝对时钟）。
    pub start: Duration,
    /// 单元止点。
    pub end: Duration,
    /// 译文。
    pub translation: String,
    /// 译文所属音轨；与输入 committed 段一致，供 HUD 只和同轨原文配对。
    pub track: Option<String>,
}

/// 实时管线的输出事件。
///
/// **多出 [`Self::Stopped`] 这一支的理由**（ADR-083 ⑧）：此前撞上「重试与换腿都改变
/// 不了」的错误时，管线每句一行 `eprintln!` + 留 `None`——一场会议里用户看到的是「译文
/// 一直不出来」，而 stdout 里刷着一百条一模一样的失败。那既不是可见信号，也不是可归因的
/// 日志，而且那一百次请求全都注定失败。现在它是**一次**会话级信号，之后本会话不再提交
/// 任何句子（`translation` 一律留 `None`，完整双语仍由停录后的二次转写补齐）。
#[derive(Clone, Debug, PartialEq)]
pub enum TranslationEvent {
    /// 一个已译完的句界单元。
    Unit(TranslatedUnit),
    /// **本会话的转译到此为止**：结构性失败，每会话恰好发一次。
    Stopped {
        /// 稳定错误码（`MODEL_MISSING` / `UNAUTHORIZED` / `QUOTA_EXHAUSTED` /
        /// `STRICT_LOCAL_BLOCKED` …；无正文，log-safe）。UI 据此给可执行提示。
        code: &'static str,
    },
}

/// 实时转译管线：spawn 一个 tokio worker，输入 committed 段、输出已译单元。
///
/// 关闭输入（[`close_input`]）后 worker flush 句缓冲并译完在途单元，随后输出通道
/// 自然关闭（[`recv`] 返回 `None`）——上游据此实现带超时的两阶段汇合。
///
/// [`close_input`]: RealtimeTranslationPipeline::close_input
/// [`recv`]: RealtimeTranslationPipeline::recv
pub struct RealtimeTranslationPipeline {
    feed_tx: Option<mpsc::UnboundedSender<Vec<Segment>>>,
    out_rx: mpsc::UnboundedReceiver<TranslationEvent>,
}

impl RealtimeTranslationPipeline {
    /// 启动管线 worker（须在 tokio 运行时内调用）。
    ///
    /// **引擎分治（TDD-08 §4）**：转译（随包引擎）独立于 ASR（whisper）降级——ASR 负载**绝不**
    /// 挂起转译。管线自身以**就近优先**卸载积压（[`TranslationConfig::max_live_backlog`]）。
    #[must_use]
    pub fn spawn(sink: Arc<dyn JobSink + Send + Sync>, cfg: TranslationConfig) -> Self {
        let (feed_tx, feed_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        tokio::spawn(worker(sink, cfg, feed_rx, out_tx));
        Self {
            feed_tx: Some(feed_tx),
            out_rx,
        }
    }

    /// 喂入一批 committed 段（输入已关闭时静默忽略）。
    pub fn feed(&self, segments: Vec<Segment>) {
        if let Some(tx) = &self.feed_tx {
            let _ = tx.send(segments);
        }
    }

    /// 关闭输入（停录收尾第一步）：worker flush 缓冲 → 译完在途 → 输出通道关闭。
    pub fn close_input(&mut self) {
        self.feed_tx = None;
    }

    /// 取下一条输出事件（worker 收尾后返回 `None`）。
    pub async fn recv(&mut self) -> Option<TranslationEvent> {
        self.out_rx.recv().await
    }
}

async fn worker(
    sink: Arc<dyn JobSink + Send + Sync>,
    cfg: TranslationConfig,
    mut feed_rx: mpsc::UnboundedReceiver<Vec<Segment>>,
    out_tx: mpsc::UnboundedSender<TranslationEvent>,
) {
    // 双源的麦克风/系统声是两条独立语言流：各自拥有句界缓冲与上下文窗。
    // BTreeMap 让停录 flush 顺序可复现；实时输出仍按 ready 单元进入 pending 的先后顺序。
    let mut lanes: BTreeMap<Option<String>, TranslationLane> = BTreeMap::new();
    // 待译单元队列（就近优先卸载的作用对象）。
    let mut pending: VecDeque<TranslateUnit> = VecDeque::new();
    // 会话内丢弃的积压句计数（脱敏诊断，无正文）。
    let mut shed: usize = 0;
    // 结构性失败后**本会话不再提交任何句子**（ADR-083 ⑧）。仍然继续 drain 输入直到
    // 上游关流——不 drain 的话 feed 端会无声积压，而停录收尾靠的正是「输入关闭」这个信号。
    let mut stopped = false;

    loop {
        // 无待译单元 → 阻塞等下一批 committed 段；有则不阻塞，尽快翻译最新积压。
        if pending.is_empty() {
            // 句缓冲里压着半句时，只等一个停顿：超时没有下文就按停顿断句（见 `idle_flush`）。
            let buffered = lanes.values().any(|lane| !lane.buffer.is_empty());
            let next = if buffered {
                match tokio::time::timeout(cfg.idle_flush, feed_rx.recv()).await {
                    Ok(next) => next,
                    Err(_) => {
                        flush_lanes_into(&mut lanes, &mut pending);
                        continue;
                    }
                }
            } else {
                feed_rx.recv().await
            };
            match next {
                Some(segs) => {
                    drain_segments_into(&mut lanes, cfg.max_pending_chars, &segs, &mut pending)
                }
                None => break, // 输入关闭 = 停录收尾
            }
        }
        // 贪婪吸收所有已到 feed：把语速造成的积压一次性纳入 pending，供就近取舍。
        while let Ok(segs) = feed_rx.try_recv() {
            drain_segments_into(&mut lanes, cfg.max_pending_chars, &segs, &mut pending);
        }
        // 就近优先（TDD-08 §4）：落后太多 → 丢弃**最旧**积压句，只保留最近 max_live_backlog 个。
        // 被丢句 translation 留 None（不发 TranslatedUnit），完整双语由二次转写补齐。
        while pending.len() > cfg.max_live_backlog.max(1) {
            pending.pop_front();
            shed += 1;
        }
        if stopped {
            // 已停：丢掉待译队列（这些句的 translation 留 None），只等输入关闭。
            pending.clear();
            continue;
        }
        if let Some(unit) = pending.pop_front() {
            let lane = lanes
                .entry(unit.track.clone())
                .or_insert_with(|| TranslationLane::new(cfg.max_pending_chars));
            if let Some(code) = translate_unit(&sink, &cfg, &mut lane.context, unit, &out_tx).await
            {
                let _ = out_tx.send(TranslationEvent::Stopped { code });
                stopped = true;
            }
        }
    }
    // 停录收尾：flush 句缓冲尾巴入队，并把**全部**保留单元译完（收尾不再就近丢弃——
    // 保存/回填需完整；停录后语速积压已停，逐句译完即可）。
    flush_lanes_into(&mut lanes, &mut pending);
    for unit in pending.drain(..) {
        if stopped {
            break;
        }
        let lane = lanes
            .entry(unit.track.clone())
            .or_insert_with(|| TranslationLane::new(cfg.max_pending_chars));
        if let Some(code) = translate_unit(&sink, &cfg, &mut lane.context, unit, &out_tx).await {
            let _ = out_tx.send(TranslationEvent::Stopped { code });
            stopped = true;
        }
    }
    if shed > 0 {
        eprintln!(
            "[translate] 就近优先卸载：本会话丢弃 {shed} 个积压句的实时译文（二次转写补齐）。"
        );
    }
    // out_tx 随 worker 结束 drop → 上游 recv() 得 None（两阶段汇合的完成信号）。
}

struct TranslationLane {
    buffer: SentenceBuffer,
    context: VecDeque<(String, String)>,
}

impl TranslationLane {
    fn new(max_pending_chars: usize) -> Self {
        Self {
            buffer: SentenceBuffer::new(max_pending_chars),
            context: VecDeque::new(),
        }
    }
}

/// 把各轨句缓冲里的余量冲成单元、按起点时间排进待译队列（停顿断句与停录收尾共用）。
fn flush_lanes_into(
    lanes: &mut BTreeMap<Option<String>, TranslationLane>,
    pending: &mut VecDeque<TranslateUnit>,
) {
    let mut tails = Vec::new();
    for (track, lane) in lanes.iter_mut() {
        if let Some(mut unit) = lane.buffer.flush() {
            unit.track.clone_from(track);
            tails.push(unit);
        }
    }
    tails.sort_by_key(|unit| unit.start);
    pending.extend(tails);
}

/// 把一批 committed 段喂入句缓冲，命中句界的单元入待译队列（就近优先的作用对象）。
fn drain_segments_into(
    lanes: &mut BTreeMap<Option<String>, TranslationLane>,
    max_pending_chars: usize,
    segs: &[Segment],
    pending: &mut VecDeque<TranslateUnit>,
) {
    for seg in segs {
        let track = seg.speaker.clone();
        let lane = lanes
            .entry(track.clone())
            .or_insert_with(|| TranslationLane::new(max_pending_chars));
        if let Some(mut unit) = lane.buffer.push(seg) {
            unit.track = track;
            pending.push_back(unit);
        }
    }
}

/// 翻译一个句界单元：提交 Translate Job → 回填上下文窗 → 发出结果。
///
/// **引擎分治（TDD-08 §4）**：不受 ASR 降级挂起——转译走进程外的随包引擎，只受管线自身的
/// 就近优先卸载约束（在 [`worker`] 层）。可重试/单句性质的失败与空结果只发脱敏诊断并跳过
/// （该句 `translation` 留 `None`，可二次补译）。
///
/// 返回 `Some(code)` = 撞上**结构性**失败（ADR-083 ⑧），由 [`worker`] 升格为一次会话级
/// 信号并停止本会话的后续提交。
async fn translate_unit(
    sink: &Arc<dyn JobSink + Send + Sync>,
    cfg: &TranslationConfig,
    context: &mut VecDeque<(String, String)>,
    unit: TranslateUnit,
    out_tx: &mpsc::UnboundedSender<TranslationEvent>,
) -> Option<&'static str> {
    // 按句解析有效目标（智能规避同语）：源已是目标语言则跳过，不产同语改写。
    let dir = cfg.resolve_target(&unit.text)?;

    // 实时档：只有前文的窗 + 口语体 + 只输出本句（ADR-083 ⑦）。
    let translation = match run_translate_job(
        sink,
        TranslateJobSpec {
            source: dir.source,
            target: &dir.target,
            text: &unit.text,
            instruction: with_glossary_constraint(
                luna_core::prompt::registry(),
                Some(live_instruction(luna_core::prompt::registry(), context)),
                &unit.text,
                &cfg.glossary,
            )
            .unwrap_or_default(),
            origin: JobOrigin::Recording,
            cancel: None,
        },
    )
    .await
    {
        TranslateOutcome::Done(text) => text,
        TranslateOutcome::Skipped => return None,
        TranslateOutcome::Stopped(code) => return Some(code),
    };

    context.push_back((unit.text.clone(), translation.clone()));
    while context.len() > cfg.context_pairs {
        context.pop_front();
    }

    let _ = out_tx.send(TranslationEvent::Unit(TranslatedUnit {
        parts: unit.parts,
        start: unit.start,
        end: unit.end,
        translation,
        track: unit.track,
    }));
    None
}

// ─── 上下文指令：实时与批量分开（ADR-083 ⑦，对照表在 TDD-03 §3.3）────────────

/// 把「源文 ⇒ 译文」上下文对拼成行。**纯排版，不含任何提示词正文**——
/// `=>` 这个记号由模板自己解释（`source => translation`）。
fn render_pairs(pairs: &VecDeque<(String, String)>) -> String {
    let mut out = String::new();
    for (src, dst) in pairs {
        out.push_str("- ");
        out.push_str(src);
        out.push_str(" => ");
        out.push_str(dst);
        out.push('\n');
    }
    out
}

/// 把纯源文上下文拼成行（批量档的**后文**）。同上：纯排版。
fn render_sources(sources: &[String]) -> String {
    let mut out = String::new();
    for src in sources {
        out.push_str("- ");
        out.push_str(src);
        out.push('\n');
    }
    out
}

/// **实时档**的上下文指令（`PromptScope::Translate` 的 `live_context`）。
///
/// 上下文窗**只有前文**（最近 N 对「源文 ⇒ 译文」）、口语体、只输出本句。
///
/// **代码里零提示词字面量**（ADR-050 ①）：正文全在提示词集合里（随包
/// `prompts/bundled.json` ⊕ 已验签目录覆盖），本函数只负责把列表拼成行。
/// 此前这句话硬编码在本文件里，于是运营改不了、发布下发不了、评测也覆盖不到——
/// 而**批量补译与整篇重译复用的正是它**。既有门禁
/// `scripts/check-prompt-registry-single-source.py` 机器强制。
#[must_use]
pub fn live_instruction(
    registry: &PromptRegistry,
    context: &VecDeque<(String, String)>,
) -> Instruction {
    registry.instruction(
        PromptScope::Translate,
        "live_context",
        &[("preceding", &render_pairs(context))],
    )
}

/// **批量档**的上下文指令（`PromptScope::Translate` 的 `document_context`）。
///
/// 上下文窗**前后文都给**：之前的单元带译文（指代与术语的既定形态），之后的单元给源文。
/// 书面体、保留段落与结构。
///
/// **后文是这条与 [`live_instruction`] 的实质差别**：「符合上下文语义」这件事一半信息
/// 在后文里——指代、术语的最终形态、句子该断在哪，都要看后面那句。只给前文的窗在构造上
/// 就答不了它，而用户看到的症状只是「二次转写之后译文还是碎的、专名前后不一致」。
#[must_use]
pub fn document_instruction(
    registry: &PromptRegistry,
    preceding: &VecDeque<(String, String)>,
    following: &[String],
) -> Instruction {
    registry.instruction(
        PromptScope::Translate,
        "document_context",
        &[
            ("preceding", &render_pairs(preceding)),
            ("following", &render_sources(following)),
        ],
    )
}

/// 提交一次 `Translate` Job 并 drain 出译文（成功且非空 → `Some`；失败/取消/空 → `None`）。
/// 实时与批量共用（DRY）：单一主干 ADR-001，失败仅发脱敏诊断（无正文）。
/// `source` 为已检测源语言（附入 prompt 提升小模型指令遵从度，见 llm_http 合成）。
/// 一次逐句翻译 Job 的成形参数（避免长参数表；与 `Job` 字段一一对应，不含策略）。
struct TranslateJobSpec<'a> {
    source: Option<Lang>,
    target: &'a Lang,
    text: &'a str,
    instruction: Instruction,
    origin: JobOrigin,
    /// 任务级取消（批量路径传 `Some`，实时管线自有会话级取消故传 `None`）。
    cancel: Option<&'a TaskCancel>,
}

/// 一次逐句翻译的结局。
///
/// **「这一句没出来」与「这条路走不通了」必须分开**（ADR-083 ⑧）：前者是既有的诚实
/// 表达（该句 `translation` 留 `None`、继续下一句）；后者继续跑只是把一百次注定失败的
/// 请求发出去，并让调用方最后拿到一份「成功了但一句译文都没有」的结果——那比一次明确的
/// 失败贵得多。判据是 [`luna_core::JobError::is_structural`]，不是「失败了没有」。
enum TranslateOutcome {
    /// 译出来了。
    Done(String),
    /// 这一句没出来，但下一句可能会（可重试的失败 / 空结果 / 本句被取消）。
    Skipped,
    /// **结构性失败**：重试与换腿都改变不了，换下一句也一样。携带稳定错误码（无正文）。
    Stopped(&'static str),
}

async fn run_translate_job(
    sink: &Arc<dyn JobSink + Send + Sync>,
    spec: TranslateJobSpec<'_>,
) -> TranslateOutcome {
    let TranslateJobSpec {
        source,
        target,
        text,
        instruction,
        origin,
        cancel,
    } = spec;
    let mut job = Job::new(
        JobKind::Translate {
            text: TextRef::Inline(text.to_string()),
            source,
            target: target.clone(),
            instruction: Some(instruction),
            template: None,
        },
        origin,
    );
    job.priority = Priority::Normal;
    // **本函数不填 `routing.preferred_provider`**（ADR-083 ⑥）：档位由
    // `Job::service_class()` 推导、由路由层按「泳道 → 模型槽位」总映射填。此前它由调用点
    // 传下来，于是「实时档」这个区分在算力交给服务端之后当场消失（解析出来的只可能是本机
    // 引擎的 provider id），而设置页仍显示它生效。
    let mut handle = sink.submit(job);
    // 任务级取消传播到本句 Job（批量路径传 Some；实时管线自有会话级取消，传 None）。
    if let Some(cancel) = cancel {
        cancel.arm(handle.cancel_token());
    }

    let mut translation: Option<String> = None;
    let mut stopped: Option<&'static str> = None;
    while let Some(ev) = handle.recv().await {
        match ev {
            JobEvent::Finished(result) => {
                if let ResultPayload::Text(text) = result.payload {
                    translation = Some(text);
                }
                break;
            }
            JobEvent::Failed(err) => {
                if err.is_structural() {
                    // 结构性失败：交给调用方按会话/整批处置，本处**不**逐句刷 stderr
                    // ——一场会议里那是一百条一模一样的行，既不是可见信号也不可归因。
                    stopped = Some(err.code());
                } else {
                    eprintln!(
                        "[translate] 单元翻译失败（code={}，留待二次补译）。",
                        err.code()
                    );
                }
                break;
            }
            // **实时转译也在会话池里**（ADR-078 ①），故它也收得到这条。不让它落进
            // 下面的 `_`：那个兜底臂的含义是「这个事件与本模块无关」，而「这条腿在等一个
            // 拿不到的额度」恰恰是本模块此刻唯一该说的事。正常情况下它一次都不该出现——
            // 会话池给转译的额度恒为 1，而本 worker 是串行的，压根没有第二个来跟它抢；
            // 真出现了就说明有会话没收干净，那正是要被看见的那种状态。
            // 脱敏：只有时长，无正文（同本模块其余诊断）。
            JobEvent::AdmissionDelayed { waited_ms } => {
                eprintln!(
                    "[translate] 实时转译腿已等本机会话额度 {waited_ms}ms 仍未开始（本句留待二次补译）。"
                );
            }
            JobEvent::Cancelled => break,
            _ => {}
        }
    }
    if let Some(code) = stopped {
        return TranslateOutcome::Stopped(code);
    }
    translation
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map_or(TranslateOutcome::Skipped, TranslateOutcome::Done)
}

/// 一次批量转译的结局（交给调用方，进度事件据此落终态）。
///
/// `#[must_use]`：丢掉它就等于把「整批停」重新变回静默失败——调用方拿到一份
/// 「成功了但一句译文都没有」的结果并照常写回 Vault，而那是所有失败形态里最贵的一种
/// （它会写进唯一事实源，错误不会）。
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchTranslateOutcome {
    /// 跑完了（其中个别句可能因可重试失败或同语跳过而留 `None`，那是既有的诚实表达）。
    Completed,
    /// 用户取消（PRD-05-010）：已译的保留，未译的留空可再次补译。
    Cancelled,
    /// **整批停**：撞上结构性失败（ADR-083 ⑧）。携带稳定错误码，无正文。
    Stopped {
        /// 稳定错误码（`MODEL_MISSING` / `UNAUTHORIZED` / … ；log-safe）。
        code: &'static str,
    },
}

/// 批量转译（停录后台重转写 / 导入媒体 / 二次翻译）：对整段序列按句界成单元逐句翻译，
/// 回填 `Segment.translation`——译文整体挂到单元**首段**（其余段留 `None`），与实时/finalize
/// 双语落盘约定一致。复用 [`SentenceBuffer`]、按轨隔离与 `luna_core::distribute_translation`
/// 三个纯函数（DRY，TDD-08 §3）。
///
/// **与实时路分开的只有上下文与指令**（ADR-083 ⑦）：本路走 [`document_instruction`]
/// ——书面体、保留结构，且上下文窗**前后文都给**。此前它与实时路共用「这是一句直播口语、
/// 请简洁」并且只看得见前文，于是号称「补齐权威双语」的那一遍是拿口语口径译出来的，
/// 而「符合上下文语义」这件事一半信息在后文里。
///
/// **翻译单元粒度仍是句界，不合并成整段/整篇**——「全文一致」由**前后文窗**兑现而不是
/// 由更大的单元兑现，理由有三：① 译文按各段原文长度占比摊回（`distribute_translation`）
/// 本就没有对齐信息，单元越大摊回误差越线性放大，那是 PRD-04-034「译文对不上原文」的
/// 既有教训反过来重演；② 段级 `translation` 是双语导出与回放的锚定粒度，跨段越多错配
/// 越贵；③ 逐句是取消的天然检查点（PRD-05-010），整篇单元会让「取消」看起来卡住，
/// 且撞上本地 LLM 的 n_ctx 预算（TDD-03 §3.3）。译文单位数不足时靠后的段诚实留 `None`，
/// 绝不硬塞半个词——那是 `distribute_translation` 的既有契约，本批不动它。
///
/// 逐句顺序提交（保持上下文窗连贯）；某句可重试失败/空只跳过该句（其余照常，诚实元数据），
/// 撞上结构性失败则**整批停**并把码交给调用方。
///
/// `on_progress(done, total)` 每句界单元处理完回调一次（含被跳过的同语单元，保证单调），
/// `total` 为句界单元总数——供二次翻译入口投影确定性进度（脱敏：仅计数，无正文）。
pub async fn translate_segments_batch(
    sink: &Arc<dyn JobSink + Send + Sync>,
    cfg: &TranslationConfig,
    segments: &mut [Segment],
    cancel: &TaskCancel,
    mut on_progress: impl FnMut(usize, usize),
) -> BatchTranslateOutcome {
    // 1. 按轨独立切句；再按时间轴合并待译单元。说话人交错不等于同一句，批量与实时同契约。
    let mut lanes: BTreeMap<Option<String>, TranslationLane> = BTreeMap::new();
    let mut ready = VecDeque::new();
    drain_segments_into(&mut lanes, cfg.max_pending_chars, segments, &mut ready);
    let mut units: Vec<TranslateUnit> = ready.into();
    for (track, lane) in &mut lanes {
        if let Some(mut unit) = lane.buffer.flush() {
            unit.track.clone_from(track);
            units.push(unit);
        }
    }
    units.sort_by_key(|unit| unit.start);

    // 后文窗：**按轨隔离**（与前文窗、切句同一条纪律——说话人交错不等于同一篇上下文）。
    // 逐单元预先算好「同轨、排在它之后的至多 N 个单元的源文」。
    let following: Vec<Vec<String>> = units
        .iter()
        .enumerate()
        .map(|(idx, unit)| {
            units[idx + 1..]
                .iter()
                .filter(|next| next.track == unit.track)
                .take(cfg.context_pairs)
                .map(|next| next.text.clone())
                .collect()
        })
        .collect();

    // 2. 逐句翻译，各轨维护独立上下文窗；译文挂到单元首段。
    let total = units.len();
    on_progress(0, total);
    let mut translations: std::collections::HashMap<SegmentId, String> =
        std::collections::HashMap::new();
    let mut outcome = BatchTranslateOutcome::Completed;
    for (idx, unit) in units.into_iter().enumerate() {
        // 用户取消（PRD-05-010）：句界即停——已译的句子保留（调用方照常写回），
        // 未译的留空可再次补译。逐句是天然的检查点，不必等整批跑完；**当前那句**的
        // LLM 调用亦经 `TaskCancel::arm` 真 abort（本地 LLM 单句可达数秒，只在句界停
        // 会让「取消」看起来卡住）。
        if cancel.is_cancelled() {
            outcome = BatchTranslateOutcome::Cancelled;
            break;
        }
        // 按句解析有效目标（智能规避同语）：源已是目标语言则该句留 None（可二次补译）。
        if let Some(dir) = cfg.resolve_target(&unit.text) {
            let lane = lanes
                .entry(unit.track.clone())
                .or_insert_with(|| TranslationLane::new(cfg.max_pending_chars));
            let result = run_translate_job(
                sink,
                TranslateJobSpec {
                    source: dir.source,
                    target: &dir.target,
                    text: &unit.text,
                    instruction: with_glossary_constraint(
                        luna_core::prompt::registry(),
                        Some(document_instruction(
                            luna_core::prompt::registry(),
                            &lane.context,
                            &following[idx],
                        )),
                        &unit.text,
                        &cfg.glossary,
                    )
                    .unwrap_or_default(),
                    origin: JobOrigin::SecondPass,
                    cancel: Some(cancel),
                },
            )
            .await;
            match result {
                // 整批停：这个错不会因为换一句就变对，继续跑完只是把一百次注定失败的
                // 请求发出去，并让调用方最后拿到一份「成功了但一句译文都没有」的结果。
                TranslateOutcome::Stopped(code) => {
                    outcome = BatchTranslateOutcome::Stopped { code };
                    break;
                }
                TranslateOutcome::Skipped => {}
                TranslateOutcome::Done(translation) => {
                    lane.context
                        .push_back((unit.text.clone(), translation.clone()));
                    while lane.context.len() > cfg.context_pairs {
                        lane.context.pop_front();
                    }
                    // 摊回到单元内**每一段**（PRD-04-034）：翻译粒度是句界（半句送 LLM
                    // 得到的是语无伦次的译文），而回放列表与双语导出的粒度是段——中间这层
                    // 换算此前缺失，于是一条长译文顶着首段、后续段空着 =「译文对不上原文」。
                    let sources: Vec<&str> = unit.parts.iter().map(|p| p.text.as_str()).collect();
                    for (part, piece) in unit
                        .parts
                        .iter()
                        .zip(luna_core::distribute_translation(&translation, &sources))
                    {
                        if let Some(piece) = piece {
                            translations.insert(part.id, piece);
                        }
                    }
                }
            }
        }
        on_progress(idx + 1, total);
    }

    // 3. 回填（整批停/取消时已译的照常写回——它们是真的译出来了）。
    for seg in segments.iter_mut() {
        if let Some(t) = translations.remove(&seg.id) {
            seg.translation = Some(t);
        }
    }
    outcome
}

// ─── 测试 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use luna_core::{
        AIProvider, CancelToken, Capability, CapabilitySet, Detection, JobError, JobEventStream,
        JobResult, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation, Timestamp,
        Timings, Usage,
    };
    use luna_provider::ProviderRegistry;
    use std::time::Duration;

    /// 确定性测试检测器：含 CJK → zh，否则 → en（无需拖入 lingua，测试可复现）。
    struct ScriptDetector;
    impl LanguageDetector for ScriptDetector {
        fn detect(&self, text: &str) -> Detection {
            if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
                Detection::confident(Lang::from("zh"))
            } else {
                Detection::confident(Lang::from("en"))
            }
        }
    }

    /// 默认测试配置：智能双向（母语 zh / 外语 en）+ 脚本检测器。
    fn test_cfg() -> TranslationConfig {
        TranslationConfig::new(
            TargetLangMode::SmartBidirectional,
            LangContext {
                native: Lang::from("zh"),
                default_foreign: Lang::from("en"),
            },
            Arc::new(ScriptDetector),
        )
    }

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    fn tracked_seg(start_ms: u64, end_ms: u64, text: &str, track: &str) -> Segment {
        let mut segment = seg(start_ms, end_ms, text);
        segment.speaker = Some(track.to_string());
        segment
    }

    /// 取下一条输出并断言它是一个已译单元。撞上 `Stopped` 直接 panic——那是结构性
    /// 失败，用例若在这里收到它，说明被测的东西根本没跑到断言那一步。
    async fn next_unit(pipeline: &mut RealtimeTranslationPipeline) -> Option<TranslatedUnit> {
        match pipeline.recv().await {
            Some(TranslationEvent::Unit(unit)) => Some(unit),
            Some(TranslationEvent::Stopped { code }) => panic!("非预期的结构性停止：{code}"),
            None => None,
        }
    }

    // ── SentenceBuffer ───────────────────────────────────────────────────────

    #[test]
    fn buffer_emits_unit_at_sentence_ending_punctuation() {
        let mut buf = SentenceBuffer::new(240);
        assert!(
            buf.push(&seg(0, 1000, "今天我们讨论")).is_none(),
            "半句不出单元"
        );
        let unit = buf
            .push(&seg(1000, 2000, "三个议题。"))
            .expect("句号应触发句界");
        assert_eq!(unit.text, "今天我们讨论 三个议题。");
        assert_eq!(unit.parts.len(), 2);
        assert_eq!(unit.start, Duration::ZERO);
        assert_eq!(unit.end, Duration::from_millis(2000));
    }

    #[test]
    fn buffer_emits_unit_when_char_limit_reached_without_punctuation() {
        let mut buf = SentenceBuffer::new(10);
        assert!(buf.push(&seg(0, 500, "一二三四五")).is_none());
        let unit = buf
            .push(&seg(500, 1000, "六七八九十"))
            .expect("达字符上限应强制出单元");
        assert_eq!(unit.parts.len(), 2);
    }

    #[test]
    fn buffer_flush_emits_trailing_partial_sentence() {
        let mut buf = SentenceBuffer::new(240);
        buf.push(&seg(0, 800, "最后一句没有标点"));
        let unit = buf.flush().expect("flush 应产出尾巴");
        assert_eq!(unit.text, "最后一句没有标点");
        assert!(buf.flush().is_none(), "二次 flush 无余量");
    }

    #[test]
    fn buffer_ignores_empty_segments() {
        let mut buf = SentenceBuffer::new(240);
        assert!(buf.push(&seg(0, 100, "   ")).is_none());
        assert!(buf.flush().is_none());
    }

    #[test]
    fn english_terminators_also_close_units() {
        let mut buf = SentenceBuffer::new(240);
        let unit = buf.push(&seg(0, 900, "Let's begin!")).expect("感叹号句界");
        assert_eq!(unit.text, "Let's begin!");
    }

    // ── 管线（echo Provider 走真 JobQueue，ADR-001 主干）─────────────────────

    /// 回声翻译 Provider：`Translate` → `Text("T:<源文>")`（可断言映射关系）。
    struct EchoTranslateProvider;

    #[async_trait]
    impl AIProvider for EchoTranslateProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("echo-translate")
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: "echo".into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, job: Job, _cancel: CancelToken) -> JobEventStream {
            // 回声含**有效目标语言**（可断言智能双向解析 + 现场热切目标）。
            let payload = match &job.kind {
                JobKind::Translate { text, target, .. } => {
                    let TextRef::Inline(src) = text;
                    ResultPayload::Text(format!("{}:{src}", target.as_str()))
                }
                _ => ResultPayload::Text(String::new()),
            };
            let job_id = job.id;
            let provider_id = self.id();
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Finished(JobResult {
                    job_id,
                    provider_id,
                    model_id: None,
                    payload,
                    usage: Usage::default(),
                    timings: Timings::default(),
                    output_truncated: None,
                    finished_at: Timestamp::ZERO,
                });
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    /// 恒失败 Provider（测「失败句静默跳过、后续句不受影响」）。
    struct FailingTranslateProvider;

    #[async_trait]
    impl AIProvider for FailingTranslateProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("fail-translate")
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: "fail".into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, _job: Job, _cancel: CancelToken) -> JobEventStream {
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Failed(JobError::InvalidInput("engine down".into()));
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    fn queue_with(provider: Arc<dyn AIProvider>) -> Arc<crate::JobQueue> {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(provider);
        Arc::new(crate::JobQueue::new(
            registry,
            crate::AdmissionCapacity::for_capture_tracks(2),
        ))
    }

    /// 按自身 id 给译文打标的 Provider（用于断言"哪一档 LLM 承接了 Translate Job"）。
    struct TaggingTranslateProvider(&'static str);

    #[async_trait]
    impl AIProvider for TaggingTranslateProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from(self.0)
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: self.0.into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, job: Job, _cancel: CancelToken) -> JobEventStream {
            let tag = self.0;
            let (job_id, provider_id) = (job.id, self.id());
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Finished(JobResult {
                    job_id,
                    provider_id,
                    model_id: None,
                    payload: ResultPayload::Text(tag.to_string()),
                    usage: Usage::default(),
                    timings: Timings::default(),
                    output_truncated: None,
                    finished_at: Timestamp::ZERO,
                });
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    /// 双档 LLM 队列（默认 `llm-default` 先注册占首位 + 实时 `llm-realtime`），
    /// 并把 `llm-realtime` 登记为 [`luna_core::ModelSlot::RealtimeLlm`] 的承接者。
    ///
    /// **这就是「泳道 → 模型槽位」总映射在本机形态下的全部接线**（ADR-083 ⑥）：
    /// 管线与调用点一律不传 `preferred_provider`，Router 按 `Job::model_slot()` 查这张表。
    fn tiered_queue() -> Arc<crate::JobQueue> {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(TaggingTranslateProvider("llm-default")));
        registry.register(Arc::new(TaggingTranslateProvider("llm-realtime")));
        registry.assign_slot(
            luna_core::ModelSlot::RealtimeLlm,
            Some(ProviderId::from("llm-realtime")),
        );
        Arc::new(crate::JobQueue::new(
            registry,
            crate::AdmissionCapacity::for_capture_tracks(2),
        ))
    }

    /// **反向验证锚点**（TDD-04 §6.1 / TDD-08 §5）：删掉 `Job::model_slot()` 里
    /// `(Translate, Realtime) => RealtimeLlm` 那一行，本条必须**当场变红**——否则那张
    /// 映射表等于没有被任何东西守着，而它失效的方式是安静的（选路照常成功，只是选到了
    /// 另一个模型）。
    #[tokio::test]
    async fn realtime_pipeline_routes_to_preferred_realtime_llm() {
        // 管线一个字节都不传定向：档位由 Job 的形状（Translate × JobOrigin::Recording）
        // 推导出实时档，Router 据总映射解析到 RealtimeLlm 槽位登记的那个 Provider。
        let mut pipeline = RealtimeTranslationPipeline::spawn(tiered_queue(), test_cfg());
        pipeline.feed(vec![seg(0, 1000, "一句话。")]);
        pipeline.close_input();
        let unit = next_unit(&mut pipeline).await.expect("应有译文");
        assert_eq!(unit.translation, "llm-realtime", "实时管线应落实时档");
    }

    #[tokio::test]
    async fn batch_translate_also_goes_to_the_translation_tier() {
        // 批量（JobOrigin::SecondPass）→ `translate_batch` 那一行同样映到「翻译与实时档」
        // （翻译一律进专用翻译模型那一档）。槽位没单独指派时回落默认候选链，
        // 见下一条用例。
        let cfg = test_cfg();
        let sink: Arc<dyn JobSink + Send + Sync> = tiered_queue();
        let mut segments = vec![seg(0, 1000, "一句话。")];
        // 进度回调：首帧 (0,total)、末帧 (total,total)，单调不减（二次翻译进度投影契约）。
        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let outcome = translate_segments_batch(
            &sink,
            &cfg,
            &mut segments,
            &TaskCancel::default(),
            |done, total| ticks.push((done, total)),
        )
        .await;
        assert_eq!(outcome, BatchTranslateOutcome::Completed);
        assert_eq!(
            segments[0].translation.as_deref(),
            Some("llm-realtime"),
            "批量翻译同样落翻译档（D3）"
        );
        assert_eq!(ticks.first(), Some(&(0, 1)), "首帧为 (0,total)");
        assert_eq!(ticks.last(), Some(&(1, 1)), "末帧为 (total,total)");
        assert!(ticks.windows(2).all(|w| w[0].0 <= w[1].0), "done 单调不减");
    }

    #[tokio::test]
    async fn unregistered_realtime_slot_gracefully_falls_back_to_default() {
        // 实时档槽位指着一个未注册的 id（设置里配了、Provider 还没装上）→ 定向命不中
        // → Router 回落默认档，不失败（优雅降级，与此前逐字相同的行为）。
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(TaggingTranslateProvider("llm-default")));
        registry.assign_slot(
            luna_core::ModelSlot::RealtimeLlm,
            Some(ProviderId::from("llm-realtime-absent")),
        );
        let queue = Arc::new(crate::JobQueue::new(
            registry,
            crate::AdmissionCapacity::for_capture_tracks(2),
        ));
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, test_cfg());
        pipeline.feed(vec![seg(0, 1000, "一句话。")]);
        pipeline.close_input();
        let unit = next_unit(&mut pipeline)
            .await
            .expect("应有译文（优雅回落，不失败）");
        assert_eq!(unit.translation, "llm-default");
    }

    #[tokio::test]
    async fn pipeline_translates_sentence_units_and_closes_after_flush() {
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let cfg = test_cfg();
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);

        pipeline.feed(vec![
            seg(0, 1000, "第一句。"),
            seg(1000, 2000, "第二句没标点"),
        ]);
        pipeline.close_input(); // flush 尾巴

        // 中文源（母语 zh）→ 智能双向解析到默认外语 en（回声前缀即有效目标）。
        let first = next_unit(&mut pipeline).await.expect("第一句应有译文");
        assert_eq!(first.translation, "en:第一句。");
        assert_eq!(first.start, Duration::ZERO);
        let second = next_unit(&mut pipeline).await.expect("flush 尾巴应有译文");
        assert_eq!(second.translation, "en:第二句没标点");
        assert!(
            next_unit(&mut pipeline).await.is_none(),
            "收尾后输出通道应关闭"
        );
    }

    /// 不出标点的转写（随包 Whisper tiny 的中文）：停顿一过就按句译出，不等攒满字符上限、
    /// 也不等停录（P7 真机：实时字幕「从不出译文」）。
    #[tokio::test(start_paused = true)]
    async fn unpunctuated_speech_is_translated_after_a_pause() {
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let cfg = test_cfg();
        let idle = cfg.idle_flush;
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);

        pipeline.feed(vec![seg(0, 1000, "请大家提前准备好各自的进度报告")]);
        // 还在停顿窗内：不应出单元。
        tokio::time::sleep(idle / 2).await;
        pipeline.feed(vec![seg(1000, 2000, "会议地点在三楼")]);
        tokio::time::sleep(idle + Duration::from_millis(10)).await;

        let unit = next_unit(&mut pipeline).await.expect("停顿后应有译文");
        assert_eq!(
            unit.translation,
            "en:请大家提前准备好各自的进度报告 会议地点在三楼"
        );
        assert_eq!(unit.parts.len(), 2, "停顿窗内的两段合成一句");
        pipeline.close_input();
        assert!(
            next_unit(&mut pipeline).await.is_none(),
            "缓冲已冲净，收尾无余量"
        );
    }

    #[tokio::test]
    async fn interleaved_tracks_keep_sentence_buffers_and_outputs_isolated() {
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, test_cfg());

        pipeline.feed(vec![
            tracked_seg(0, 500, "我方半句", "self"),
            tracked_seg(0, 900, "对方完整一句。", "remote"),
            tracked_seg(500, 1200, "接着说完。", "self"),
        ]);
        pipeline.close_input();

        let remote = next_unit(&mut pipeline).await.expect("对方句应先完成");
        let own = next_unit(&mut pipeline).await.expect("我方句随后完成");
        assert_eq!(remote.track.as_deref(), Some("remote"));
        assert_eq!(remote.translation, "en:对方完整一句。");
        assert_eq!(own.track.as_deref(), Some("self"));
        assert_eq!(own.translation, "en:我方半句 接着说完。");
        assert!(next_unit(&mut pipeline).await.is_none());
    }

    #[tokio::test]
    async fn batch_translation_does_not_join_interleaved_speakers() {
        let sink: Arc<dyn JobSink + Send + Sync> = queue_with(Arc::new(EchoTranslateProvider));
        let cfg = test_cfg();
        let mut segments = vec![
            tracked_seg(0, 500, "我方半句", "self"),
            tracked_seg(100, 700, "对方一句。", "remote"),
            tracked_seg(500, 1000, "说完。", "self"),
        ];
        let outcome = translate_segments_batch(
            &sink,
            &cfg,
            &mut segments,
            &TaskCancel::default(),
            |_, _| {},
        )
        .await;
        assert_eq!(outcome, BatchTranslateOutcome::Completed);
        // 跨说话人绝不拼句：对方那句独立成单元、独立成译文。
        assert_eq!(segments[1].translation.as_deref(), Some("en:对方一句。"));
        // 我方两段属同一句界单元 → 整句译文**摊回到两段**（PRD-04-034），而不是只挂首段
        // 让后一段空着（那就是真机上「译文与原文对不上」的样子）。
        let own: String = [0usize, 2]
            .iter()
            .map(|i| {
                segments[*i]
                    .translation
                    .clone()
                    .expect("我方两段都应有译文")
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(own, "en:我方半句 说完。");
    }

    #[tokio::test]
    async fn set_foreign_lang_switches_target_live() {
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let cfg = test_cfg();
        cfg.set_foreign_lang(Lang::from("ja")); // 现场切目标外语 en → ja
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);

        pipeline.feed(vec![seg(0, 1000, "现场切换目标。")]);
        pipeline.close_input();

        let unit = next_unit(&mut pipeline).await.expect("应有译文");
        assert_eq!(unit.translation, "ja:现场切换目标。", "热切后按新目标解析");
    }

    #[tokio::test]
    async fn same_language_source_is_skipped_not_paraphrased() {
        // 英文源（非母语 zh）→ 智能双向解析到母语 zh；此处目标==源？否：源 en，目标 zh，会翻译。
        // 构造源==目标场景：把外语设为 en，喂英文 → 有效目标 zh（源 en≠zh）仍翻译。
        // 真正的「源==目标」跳过：Fixed 模式同语；此处验证智能双向下英文→中文（不跳过）。
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let cfg = test_cfg();
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);
        pipeline.feed(vec![seg(0, 1000, "This is English.")]);
        pipeline.close_input();
        let unit = next_unit(&mut pipeline).await.expect("英文源应翻成母语 zh");
        assert_eq!(unit.translation, "zh:This is English.");
    }

    #[tokio::test]
    async fn pipeline_skips_failed_units_but_continues() {
        let queue = queue_with(Arc::new(FailingTranslateProvider));
        let cfg = test_cfg();
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);

        pipeline.feed(vec![seg(0, 1000, "会失败的句子。")]);
        pipeline.close_input();

        assert!(
            next_unit(&mut pipeline).await.is_none(),
            "失败句无输出，通道正常关闭（translation 留 None 可二次补译）"
        );
    }

    #[tokio::test]
    async fn live_backlog_sheds_oldest_keeps_recent() {
        // 就近优先（TDD-08 §4）：一次性喂入远超 max_live_backlog 的句子，实时管线应丢弃最旧积压句，
        // 只译出最近的若干句（被丢句 translation 留 None，二次转写补齐）。
        let queue = queue_with(Arc::new(EchoTranslateProvider));
        let mut cfg = test_cfg();
        cfg.max_live_backlog = 2;
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, cfg);

        // 8 个独立句界单元一次性到达（模拟 LLM 跟不上语速的积压）。
        let segs: Vec<Segment> = (0..8)
            .map(|i| seg(i * 1000, i * 1000 + 800, &format!("第{i}句。")))
            .collect();
        pipeline.feed(segs);
        pipeline.close_input();

        let mut got: Vec<TranslatedUnit> = Vec::new();
        while let Some(u) = next_unit(&mut pipeline).await {
            got.push(u);
        }
        assert!(
            got.len() < 8,
            "积压应被就近卸载，产出句数应少于总句数（got {}）",
            got.len()
        );
        assert!(!got.is_empty(), "至少译出最近的句子");
        // 产出的必是较晚的句子（最旧的被丢弃）：末句必在产出集中。
        let last = got.last().expect("有产出");
        assert_eq!(last.translation, "en:第7句。", "最新句必被译出（就近优先）");
    }

    // ── 两条上下文指令按服务档分开（ADR-083 ⑦，对照表在 TDD-03 §3.3）─────────

    /// **后文只有批量档给得出**——这正是 ADR-083 ⑦ 要修的核心：「符合上下文语义」
    /// 这件事一半信息在后文里，而此前两条路共用同一句只看得见前文的指令。
    ///
    /// 断言刻意**不写任何提示词原文**：模板正文的唯一事实源是 `prompts/bundled.json`
    /// （ADR-050），用例里写死一句就等于允许实现里也写死它。故这里只断言**结构**——
    /// 两条模板取得到、前文两条都带、后文只有批量带、两条不是同一句。
    #[test]
    fn document_context_carries_the_following_window_and_live_context_does_not() {
        let registry = PromptRegistry::bundled();
        assert!(
            registry
                .template(PromptScope::Translate, "live_context")
                .is_some(),
            "live_context 必须在提示词集合里（代码里零字面量）"
        );
        assert!(
            registry
                .template(PromptScope::Translate, "document_context")
                .is_some(),
            "document_context 必须在提示词集合里（代码里零字面量）"
        );

        let mut ctx = VecDeque::new();
        ctx.push_back(("你好。".to_string(), "Hello.".to_string()));
        let following = vec!["后面还有一句。".to_string()];

        let live = live_instruction(&registry, &ctx);
        let doc = document_instruction(&registry, &ctx, &following);

        assert!(live.contains("你好。 => Hello."), "实时档带前文窗");
        assert!(doc.contains("你好。 => Hello."), "批量档也带前文窗");
        assert!(!live.contains("后面还有一句。"), "实时档的上下文窗只有前文");
        assert!(doc.contains("后面还有一句。"), "批量档的上下文窗必须含后文");
        assert_ne!(live, doc, "两条是不同的指令，不是同一句加了个开关");
    }

    /// 把 `Translate.instruction` 原样当译文回声的 Provider（断言「喂给模型的是哪一段
    /// 上下文」这件事真的走通了整条装配，而不只是那两个纯函数自己对）。
    struct InstructionEchoProvider;

    #[async_trait]
    impl AIProvider for InstructionEchoProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("echo-instruction")
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: "echo-instruction".into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, job: Job, _cancel: CancelToken) -> JobEventStream {
            let payload = match &job.kind {
                JobKind::Translate { instruction, .. } => {
                    ResultPayload::Text(instruction.clone().unwrap_or_default().into_string())
                }
                _ => ResultPayload::Text(String::new()),
            };
            let (job_id, provider_id) = (job.id, self.id());
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Finished(JobResult {
                    job_id,
                    provider_id,
                    model_id: None,
                    payload,
                    usage: Usage::default(),
                    timings: Timings::default(),
                    output_truncated: None,
                    finished_at: Timestamp::ZERO,
                });
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    #[tokio::test]
    async fn batch_loop_actually_feeds_the_following_units_to_the_model() {
        // 三句同轨：第一句的指令里必须看得见第二、三句的**源文**（后文窗），
        // 而它自己的译文里不会有——这条走的是整条批量装配，不是纯函数自测。
        let sink: Arc<dyn JobSink + Send + Sync> = queue_with(Arc::new(InstructionEchoProvider));
        let mut segments = vec![
            seg(0, 1000, "第一句。"),
            seg(1000, 2000, "第二句。"),
            seg(2000, 3000, "第三句。"),
        ];
        let outcome = translate_segments_batch(
            &sink,
            &test_cfg(),
            &mut segments,
            &TaskCancel::default(),
            |_, _| {},
        )
        .await;
        assert_eq!(outcome, BatchTranslateOutcome::Completed);
        let first = segments[0].translation.clone().expect("首句应有译文");
        assert!(first.contains("第二句。"), "批量指令必须带后文（下一句）");
        assert!(first.contains("第三句。"), "后文窗按 context_pairs 给足");
    }

    // ── 结构性失败：一次会话级信号 / 整批停（ADR-083 ⑧，TDD-08 §4）───────────

    /// 恒以**结构性**错误失败并**数提交次数**的 Provider。
    ///
    /// 只断言「发了事件」抓不住「仍在逐句发请求」那一半——而那一半才是真正的代价
    /// （一百句就是一百次注定失败的请求）。故这里数次数。
    struct StructuralFailureProvider(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait]
    impl AIProvider for StructuralFailureProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("structural-fail")
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: "structural-fail".into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, _job: Job, _cancel: CancelToken) -> JobEventStream {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Failed(JobError::ModelMissing {
                    model: luna_core::ModelId::from("qwen-nope"),
                    hint: "去模型页装一个".into(),
                });
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    /// 恒以 `Cancelled` 收场的 Provider（用户自己按的停止**不是**故障）。
    struct CancelledProvider(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait]
    impl AIProvider for CancelledProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("cancelled-provider")
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: self.id(),
                display_name: "cancelled".into(),
                location: ProviderLocation::Device,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [Capability::Translate].into_iter().collect(),
                streaming: false,
                location: ProviderLocation::Device,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        async fn execute(&self, _job: Job, _cancel: CancelToken) -> JobEventStream {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                yield JobEvent::Failed(JobError::Cancelled);
            })
        }
        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }
    }

    #[tokio::test]
    async fn structural_failure_stops_the_session_after_exactly_one_signal() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = queue_with(Arc::new(StructuralFailureProvider(Arc::clone(&calls))));
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, test_cfg());
        for i in 0..6 {
            pipeline.feed(vec![seg(i * 1000, i * 1000 + 900, "一句话。")]);
        }
        pipeline.close_input();

        let mut stops: Vec<&'static str> = Vec::new();
        let mut units = 0usize;
        while let Some(event) = pipeline.recv().await {
            match event {
                TranslationEvent::Unit(_) => units += 1,
                TranslationEvent::Stopped { code } => stops.push(code),
            }
        }
        assert_eq!(stops, vec!["MODEL_MISSING"], "会话级信号恰好一条、带稳定码");
        assert_eq!(units, 0, "停了之后一句译文都不该有");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "停了之后不许再提交任何句子——只断言发了事件抓不住这一半"
        );
    }

    #[tokio::test]
    async fn user_cancellation_never_raises_the_session_banner() {
        // 用户自己按的停止不是故障：`Cancelled` 不得触发那条横幅，否则每一次正常
        // 停录都会弹「翻译已停止」。逐句留 None 是既有的诚实表达。
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = queue_with(Arc::new(CancelledProvider(Arc::clone(&calls))));
        let mut pipeline = RealtimeTranslationPipeline::spawn(queue, test_cfg());
        pipeline.feed(vec![seg(0, 900, "第一句。"), seg(1000, 1900, "第二句。")]);
        pipeline.close_input();

        while let Some(event) = pipeline.recv().await {
            assert!(
                matches!(event, TranslationEvent::Unit(_)),
                "Cancelled 绝不能升格为会话级停止"
            );
        }
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "取消是逐句的：后面的句子照常提交"
        );
    }

    #[tokio::test]
    async fn structural_failure_stops_the_whole_batch_with_its_code() {
        // 判据是「这个错不会因为换一句就变对」：继续跑完只是把一百次注定失败的请求
        // 发出去，并让调用方最后拿到一份「成功了但一句译文都没有」的结果。
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink: Arc<dyn JobSink + Send + Sync> =
            queue_with(Arc::new(StructuralFailureProvider(Arc::clone(&calls))));
        let mut segments = vec![
            seg(0, 900, "第一句。"),
            seg(1000, 1900, "第二句。"),
            seg(2000, 2900, "第三句。"),
        ];
        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let outcome = translate_segments_batch(
            &sink,
            &test_cfg(),
            &mut segments,
            &TaskCancel::default(),
            |done, total| ticks.push((done, total)),
        )
        .await;
        assert_eq!(
            outcome,
            BatchTranslateOutcome::Stopped {
                code: "MODEL_MISSING"
            },
            "整批停，并把稳定码交给调用方"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "整批停 = 后面两句一次都不提交"
        );
        assert!(
            segments.iter().all(|s| s.translation.is_none()),
            "一句都没译出来时不得留下半份译文"
        );
        // 进度停在撞上它的那一帧（不伪造 total/total 的「跑完了」）。
        assert_eq!(ticks.last(), Some(&(0, 3)));
    }
}
