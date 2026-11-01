//! 进程内 ASR Provider（`local-whisper`，whisper-rs FFI）。事实源：TDD-03 §2/§4、ADR-010/013。
//!
//! **ADR-010 分治铁律**：whisper.cpp 经 FFI 跑在主进程的**专用阻塞线程池**（每个转写 Job
//! 一条 `std::thread`，**绝不上 tokio async executor**）；结果经 channel 桥回 `execute` 的
//! `JobEventStream`。**abort 接 `CancelToken`**：whisper 解码边界回调轮询 `is_cancelled()`
//! 即时中止（TDD-01 §5.5）。
//!
//! **统一上层（DRY）**：与随包引擎的 `local-llm` 同实现 `AIProvider`，对 JobQueue/Router 等价，
//! 仅 Provider 内部执行路径不同（TDD-03 抬头）。
//!
//! **两种执行路径**：
//! - `AudioRef::Pcm` — 端点整段一次性 `Finished(Segments)`，**不发 Partial**。
//! - `AudioRef::Stream(id)` — 流式窗口 + **LocalAgreement-2** 提交策略（TDD-02 §3.3-B）：
//!   连续两次解码对前缀一致 → `committed`；未一致尾部 → `hypothesis`；
//!   committed 一旦发出**永不回退**（不变量）；`capabilities().streaming = true`。
//!
//! **幻觉抑制为一等参数**（非可选后处理，TDD-03 §2.3）：
//! `no_speech_thold` / `logprob_thold` / `entropy_thold` + `suppress_blank`/`suppress_nst`。

use crate::hallucination::{normalize_text, HallucinationFilter};
use async_trait::async_trait;
use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, CapabilitySet, Job, JobError, JobEvent,
    JobEventStream, JobKind, JobResult, ModelId, PartialPayload, PcmBuffer, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, Segment, SegmentId, StreamId,
    Timestamp, Timings, Usage, UsageEstimate, WindowInput, WordTiming,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use whisper_rs::{
    get_lang_str, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
    WhisperState,
};

/// 转写工作采样率（whisper 约定 16k mono f32，与 [`luna_audio`] 输出一致）。
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// `local-whisper` 的稳定 Provider id。
pub const PROVIDER_ID: &str = "local-whisper";

/// 幻觉抑制默认阈值（TDD-03 §2.3，可由设置覆盖）。
const NO_SPEECH_THOLD: f32 = 0.6;
const LOGPROB_THOLD: f32 = -1.0;
const ENTROPY_THOLD: f32 = 2.4;

// ─── 配置与错误 ────────────────────────────────────────────────────────────────

/// whisper 运行后端。
///
/// Phase 4 当前把 CoreML 后置，Metal 也必须先过 S3 稳定性探针后才能作为默认路径。当前本机已确认
/// whisper-rs 0.14.4 + whisper.cpp 1.7.4 的 Metal 初始化可 native crash；默认使用 CPU，避免
/// 在 Provider 加载阶段把主进程打崩。后续 GPU 探测应在受控探针中完成，再显式切到 [`Self::Metal`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhisperBackend {
    /// CPU/BLAS 路径，安全默认。
    Cpu,
    /// Metal GPU 路径，需 S3 探针证明稳定后显式启用。
    Metal,
}

/// `local-whisper` 批量档 Provider id（重转写/导入用独立模型，经 JobQueue 定向路由，
/// 与实时档 [`PROVIDER_ID`] 并存互不干扰，TDD-01 §4 preferred_provider）。
pub const BATCH_PROVIDER_ID: &str = "local-whisper-batch";

/// whisper 引擎配置。
#[derive(Clone, Debug)]
pub struct WhisperConfig {
    /// ggml 模型权重路径（内置极小模型或下载所得，TDD-04/PRD-14 §6）。
    pub model_path: PathBuf,
    /// 解码线程数。
    pub n_threads: i32,
    /// 默认转写语种（`None` = 自动检测；中文优先场景可设 `"zh"`，PRD-02-006）。
    pub default_lang: Option<String>,
    /// 运行后端。
    pub backend: WhisperBackend,
    /// 注册到 registry 的 Provider id（默认 [`PROVIDER_ID`]；批量档用 [`BATCH_PROVIDER_ID`]
    /// 以便与实时档并存并经 `routing.preferred_provider` 定向）。
    pub provider_id: ProviderId,
}

impl WhisperConfig {
    /// 以模型路径构造默认配置（线程数取物理核数的保守值，后端默认 CPU 安全档）。
    #[must_use]
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            n_threads: default_threads(),
            default_lang: None,
            backend: WhisperBackend::Cpu,
            provider_id: ProviderId::from(PROVIDER_ID),
        }
    }

    /// 覆盖注册 Provider id（批量档用 [`BATCH_PROVIDER_ID`]，与实时档并存）。
    #[must_use]
    pub fn with_provider_id(mut self, id: impl Into<ProviderId>) -> Self {
        self.provider_id = id.into();
        self
    }

    /// 覆盖运行后端（GPU 探针证明稳定后由编排层显式切 [`WhisperBackend::Metal`]）。
    #[must_use]
    pub fn with_backend(mut self, backend: WhisperBackend) -> Self {
        self.backend = backend;
        self
    }

    /// 覆盖默认转写语种（`None` = 自动检测）。
    #[must_use]
    pub fn with_default_lang(mut self, lang: Option<String>) -> Self {
        self.default_lang = lang;
        self
    }
}

/// 默认解码线程数，上限 8。Windows 取物理核数：ggml 的自旋线程池铺到超线程兄弟上，解码步骤
/// 互相抢同一物理核，tiny 解码耗时为物理核数时的 3~5 倍且波动大（TDD-03 §4 Windows 条）。
/// macOS 不变（Apple Silicon 无超线程，逻辑核即物理核）。
fn default_threads() -> i32 {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    #[cfg(windows)]
    let n = sysinfo::System::new()
        .physical_core_count()
        .map_or(logical, |p| p.min(logical));
    #[cfg(not(windows))]
    let n = logical;
    (n as i32).clamp(1, 8)
}

/// whisper 引擎错误。
#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    /// 模型加载失败（路径错误/格式不符/内存不足）。
    #[error("failed to load the whisper model: {0}")]
    Load(String),
}

// ─── 流式注册表 ────────────────────────────────────────────────────────────────

/// 内部流注册表条目（窗口接收端 + 推理耗时发送端 + 共享语言锁）。
struct StreamEntry {
    window_rx: std::sync::mpsc::Receiver<Option<WindowInput>>,
    infer_tx: std::sync::mpsc::SyncSender<u64>,
    lang: SharedStreamLang,
}

// ─── LocalAgreement-2 ──────────────────────────────────────────────────────────

/// 时间前沿门控的最小推进量：候选段大半落在已提交前沿之前时，仅当其把前沿
/// 推进至少此量才视为携带新内容，否则判为**同段音频重解码的变体**丢弃。
/// （真机根因 K3：同窗重解码首段文本漂移 → 前缀一致中断 → 已提交区间被整段重提交，
/// 表现为"同时间戳重复行/截断版+完整版并存"；见 `feed` 文档。）
const MIN_COMMIT_ADVANCE: Duration = Duration::from_secs(1);

/// LocalAgreement-2 提交策略（TDD-02 §3.3-B）+ **窗界感知** + **时间前沿单调门控**：
///
/// - **同一窗重解码**（增长缓冲模式，预留 seam）：连续两次解码对段列表**前缀**一致 →
///   前缀 committed（永不回退）；未达成一致的尾部 → hypothesis（可改写）。
/// - **跨窗（新 `window_abs_start`）**：`StreamSegmenter` 产出的是互不重叠的 utterance 窗、
///   每窗只解码一次绝不重解——上一窗的全部剩余段**即定稿提交**。若缺此规则，纯"前缀一致"
///   在互斥窗下永不触发，除最后一窗外全部静默丢弃（真机实测的转写丢失根因 K1）。
/// - **committed 永不回退（K3 修复）**：`committed_count` 单调不减——同窗重解码在已提交
///   前缀内出现文本漂移（agree < committed_count）时，绝不把计数拨回去，否则后续一致
///   提交会把已提交区间**整段重发**。配套 [`Self::admit`] 时间前沿门控兜底段界漂移：
///   任何提交候选段必须把 `committed_end` 前沿向前推进，重解码变体（大半落在前沿之前
///   且推进不足 [`MIN_COMMIT_ADVANCE`]）一律拒绝。
///
/// 纯状态机，零 IO，可在单元测试中独立验证。
struct LocalAgreement2 {
    /// 上一次解码结果（绝对时间戳）。
    prev_segs: Vec<Segment>,
    /// `prev_segs` 中已发出的前缀长度（committed_count..end 尚未发出）。单调不减
    /// （仅在窗界复位；同窗内 agree 回落不回拨，见结构体文档 K3）。
    committed_count: usize,
    /// 上一次解码所属窗的会话绝对采样起点（窗界判定）。
    last_window_abs: Option<u64>,
    /// 已提交时间前沿（最后一个 committed 段的 `end`；全流单调不减，跨窗有效）。
    committed_end: Duration,
}

impl LocalAgreement2 {
    fn new() -> Self {
        Self {
            prev_segs: Vec::new(),
            committed_count: 0,
            last_window_abs: None,
            committed_end: Duration::ZERO,
        }
    }

    /// 时间前沿门控：候选 committed 段准入判定（通过则推进前沿）。
    ///
    /// - `end <= committed_end`：完全落在已提交区间内 → 重解码复制品，拒绝。
    /// - 段中点在前沿之前且前沿推进 < [`MIN_COMMIT_ADVANCE`]：大半是已提交音频的
    ///   重解码变体（含跨窗 overlap 尾字重出）→ 拒绝。
    /// - 其余：接受并把 `committed_end` 推进到该段 `end`。
    fn admit(&mut self, seg: &Segment) -> bool {
        if seg.end <= self.committed_end {
            return false;
        }
        let midpoint = seg.start + (seg.end.saturating_sub(seg.start)) / 2;
        if midpoint < self.committed_end
            && seg.end.saturating_sub(self.committed_end) < MIN_COMMIT_ADVANCE
        {
            return false;
        }
        self.committed_end = seg.end;
        true
    }

    /// 喂入一窗解码段，返回 `(new_committed, hypothesis_text, stability)`。
    ///
    /// `window_abs_start` = 本窗会话绝对采样起点：与上次不同即视为**新窗**，
    /// 先定稿上一窗全部剩余段；相同则按前缀一致提交（同窗重解码）。
    fn feed(
        &mut self,
        window_abs_start: u64,
        curr_segs: Vec<Segment>,
    ) -> (Vec<Segment>, String, f32) {
        let mut new_committed: Vec<Segment> = Vec::new();

        // 窗界：新窗到来 → 上一窗剩余段即定稿（该音频绝不重解码，结果不可能再被修订）。
        if self.last_window_abs != Some(window_abs_start) {
            if self.committed_count < self.prev_segs.len() {
                let tail = self.prev_segs[self.committed_count..].to_vec();
                new_committed.extend(tail.into_iter().filter(|s| self.admit(s)));
            }
            self.prev_segs = Vec::new();
            self.committed_count = 0;
            self.last_window_abs = Some(window_abs_start);
        }

        // 同窗前缀一致提交（新窗时 prev 已清空 → agree=0，curr 全部为 hypothesis）。
        let agree = segs_agree_prefix_count(&self.prev_segs, &curr_segs);
        if agree > self.committed_count {
            let batch = self.prev_segs[self.committed_count..agree].to_vec();
            new_committed.extend(batch.into_iter().filter(|s| self.admit(s)));
        }

        let stability: f32 = if new_committed.is_empty() { 0.5 } else { 1.0 };

        // 状态推进：prev 变为 curr。committed_count **单调不减**（K3：agree 在已提交
        // 前缀内回落时不回拨，防已提交区间被后续一致提交整段重发），仅按 curr 长度截顶。
        self.committed_count = self.committed_count.max(agree).min(curr_segs.len());

        // hypothesis = curr 中已提交前缀之后的文本（暂定、可改写；不复读已提交内容）。
        let hypothesis = curr_segs[self.committed_count..]
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        self.prev_segs = curr_segs;

        (new_committed, hypothesis, stability)
    }

    /// 流结束后 flush prev_segs 中未 committed 的剩余段（过时间前沿门控后全部 committed）。
    fn flush(mut self) -> Vec<Segment> {
        if self.committed_count < self.prev_segs.len() {
            let tail = self.prev_segs[self.committed_count..].to_vec();
            tail.into_iter().filter(|s| self.admit(s)).collect()
        } else {
            Vec::new()
        }
    }
}

/// 进程级幻觉过滤器（不可变内置词表；流式/批量共用，避免每窗重复解析词表）。
fn hallucination_filter() -> &'static HallucinationFilter {
    static FILTER: OnceLock<HallucinationFilter> = OnceLock::new();
    FILTER.get_or_init(HallucinationFilter::builtin)
}

/// 文本归一化（忽略大小写与标点）：委托 [`normalize_text`] 单一事实源，段对比与幻觉
/// 过滤共用同一规范化，避免两处漂移。
fn normalize_seg_text(s: &str) -> String {
    normalize_text(s)
}

/// 两段列表从头部开始的一致前缀长度（以段为单位，文本归一化后比较）。
fn segs_agree_prefix_count(prev: &[Segment], curr: &[Segment]) -> usize {
    let max = prev.len().min(curr.len());
    let mut n = 0;
    for i in 0..max {
        if normalize_seg_text(&prev[i].text) == normalize_seg_text(&curr[i].text) {
            n += 1;
        } else {
            break;
        }
    }
    n
}

// ─── 绝对时钟换算 ──────────────────────────────────────────────────────────────

/// whisper centiseconds 相对窗口起点 + 窗口绝对采样起点 → 绝对 `Duration`。
///
/// whisper 的 t0/t1 为**相对于本次 `full()` 输入音频**的 centiseconds（1cs = 10ms）。
/// `abs_start_samples ÷ sample_rate` = 窗口在会话中的绝对起点秒。
fn abs_time_from_cs(abs_start_samples: u64, sample_rate: u32, cs: i64) -> Duration {
    let window_start_ms = abs_start_samples * 1000 / u64::from(sample_rate);
    let offset_ms = cs.max(0) as u64 * 10; // centiseconds → ms
    Duration::from_millis(window_start_ms + offset_ms)
}

// ─── 解码函数抽象（可注入 stub，支持确定性单测）────────────────────────────────

/// 可注入解码函数类型。
/// `(samples, abs_start, cancel) → (absolute-timed Segments, infer_ms)`。
///
/// 真实路径 = whisper FFI（`make_whisper_decode_fn`）；
/// 测试路径 = 确定性 stub（无需模型权重）。
// FnMut：闭包内持 `StickyLanguage` 会话状态（首窗检测语言后锁定后续），故可变。
type DecodeFn = Box<dyn FnMut(&[f32], u64, &CancelToken) -> (Vec<Segment>, u64) + Send + 'static>;

/// 解码单个窗口，返回绝对时间戳段列表与推理耗时（毫秒）。
/// 解码一个窗口。返回 `(段, 推理耗时 ms, 检测语言 id)`——语言 id 仅在**本窗产出实质文本**
/// 时回传（供 [`StickyLanguage::observe`] 锁定；静音/空窗回 `None` 防误锁），且仅 auto 模式
/// 有意义（`lang` 已指定时 whisper 不做检测）。
fn decode_window_with_abs_time(
    ctx: &WhisperContext,
    cfg: &WhisperConfig,
    lang: Option<&str>,
    initial_prompt: Option<&str>,
    samples: &[f32],
    abs_start: u64,
    cancel: &CancelToken,
) -> (Vec<Segment>, u64, Option<i32>) {
    let t0 = Instant::now();
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(_) => return (Vec::new(), 0, None),
    };
    let params = build_params(cfg, lang, initial_prompt, cancel, false);
    match state.full(params, samples) {
        Ok(_) if cancel.is_cancelled() => (Vec::new(), t0.elapsed().as_millis() as u64, None),
        Ok(_) => {
            let segs = collect_segments_abs(&state, abs_start);
            let detected = detected_lang_for(&state, lang, &segs);
            (segs, t0.elapsed().as_millis() as u64, detected)
        }
        Err(_) => (Vec::new(), t0.elapsed().as_millis() as u64, None),
    }
}

/// 从解码后的 `state` 取检测语言 id 供会话锁定：仅当 **auto（`configured_lang` 为空）
/// 且本段有实质文本**时回传，否则 `None`（已指定语言无需锁定；空段防误锁）。
fn detected_lang_for(
    state: &WhisperState,
    configured_lang: Option<&str>,
    segs: &[Segment],
) -> Option<i32> {
    if configured_lang.is_some() || segs.is_empty() {
        return None;
    }
    state.full_lang_id_from_state().ok()
}

/// 从 `WhisperState` 抽取段，时间戳换算为绝对 `Duration`。
fn collect_segments_abs(state: &WhisperState, abs_start: u64) -> Vec<Segment> {
    let n = state.full_n_segments().unwrap_or(0);
    let mut segs = Vec::new();
    for i in 0..n {
        let text = state.full_get_segment_text_lossy(i).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let cs0 = state.full_get_segment_t0(i).unwrap_or(0);
        let cs1 = state.full_get_segment_t1(i).unwrap_or(0);
        segs.push(Segment {
            id: SegmentId::new(),
            start: abs_time_from_cs(abs_start, WHISPER_SAMPLE_RATE, cs0),
            end: abs_time_from_cs(abs_start, WHISPER_SAMPLE_RATE, cs1),
            text: text.trim().to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        });
    }
    // 幻觉过滤（TDD-03 §2.3）：字幕署名/水印/重复循环在此统一收口（流式路径）。
    hallucination_filter().filter(segs)
}

/// 构造实际 whisper 解码闭包（捕获 context + config + prompt + **共享会话语言锁**）。
///
/// `lang` 为 [`SharedStreamLang`] 共享句柄：auto 模式下首个有实质文本的窗口检测出语言即
/// 锁定后续（根治逐窗漂移）；控制端可经同一句柄现场切换语言 / 回读检测结果（PRD-04-046）。
fn make_whisper_decode_fn(
    ctx: Arc<WhisperContext>,
    cfg: WhisperConfig,
    lang: SharedStreamLang,
    initial_prompt: Option<String>,
) -> DecodeFn {
    Box::new(move |samples, abs_start, cancel| {
        let current = lang.current();
        let (segs, infer_ms, detected) = decode_window_with_abs_time(
            &ctx,
            &cfg,
            current.as_deref(),
            initial_prompt.as_deref(),
            samples,
            abs_start,
            cancel,
        );
        lang.observe(detected);
        (segs, infer_ms)
    })
}

// ─── 流式 ASR 主循环（可注入 decode_fn，支持确定性测试）──────────────────────

/// 流式转写工作线程主循环（TDD-02 §3.3-B，LocalAgreement-2）。
///
/// 阻塞线程（ADR-010：绝不上 tokio executor）；结果经 `ev_tx` 桥回异步 `execute` 流。
/// `infer_tx` 可选——每窗推理耗时 ms 经此通道回传（不污染对外 `JobEvent`，TDD-02 §5.1 RTF）。
///
/// **快照合并（过载主阀）**：`StreamSegmenter` 对同一 utterance 产出多个同 `abs_start`
/// 的增长快照，后到者是先到者的超集——解码前先 drain 通道里同 `abs_start` 的更新快照，
/// **只解最新**。解码慢于快照节奏时自动跳过过期快照，成本上限 ≈ 解码吞吐本身，
/// 不积压、不放大（RTF 降级档位是次级阀门）。
#[allow(clippy::too_many_arguments)]
fn run_streaming_transcribe(
    job_id: luna_core::JobId,
    provider_id: ProviderId,
    model_id: Option<ModelId>,
    rx: std::sync::mpsc::Receiver<Option<WindowInput>>,
    mut decode_fn: DecodeFn,
    cancel: CancelToken,
    ev_tx: tokio::sync::mpsc::UnboundedSender<JobEvent>,
    infer_tx: Option<std::sync::mpsc::SyncSender<u64>>,
) {
    let mut la2 = LocalAgreement2::new();
    let mut all_committed: Vec<Segment> = Vec::new();
    // 跨窗预取暂存（合并 drain 时读到的下一 utterance 首窗）。
    let mut stashed: Option<WindowInput> = None;
    // 收到流关闭标记（`None`）后处理完在手窗即收尾。
    let mut stream_closed = false;

    // 入口取消检测
    if cancel.is_cancelled() {
        let _ = ev_tx.send(JobEvent::Cancelled);
        return;
    }

    loop {
        let mut window = match stashed.take() {
            Some(w) => w,
            None => match rx.recv() {
                Ok(Some(w)) => w,
                // `None` 关闭标记或发送端 drop → 收尾 flush。
                Ok(None) | Err(_) => break,
            },
        };
        // 合并过期快照：同 abs_start 后到者取代先到者；异 abs_start 暂存给下一轮。
        loop {
            match rx.try_recv() {
                Ok(Some(newer)) if newer.abs_start == window.abs_start => window = newer,
                Ok(Some(next_win)) => {
                    stashed = Some(next_win);
                    break;
                }
                Ok(None) => {
                    stream_closed = true;
                    break;
                }
                Err(_) => break,
            }
        }

        if cancel.is_cancelled() {
            let _ = ev_tx.send(JobEvent::Cancelled);
            return;
        }

        let (curr_segs, infer_ms) = decode_fn(&window.samples, window.abs_start, &cancel);

        if cancel.is_cancelled() {
            let _ = ev_tx.send(JobEvent::Cancelled);
            return;
        }

        // 脱敏诊断（仅计数/耗时，无正文；LUNA_DEBUG_ASR 开启）：确认每窗解码产出，
        // 定位"解码空段→无 Partial"并监测 RTF（infer_ms vs 音频时长）。
        if std::env::var_os("LUNA_DEBUG_ASR").is_some() {
            eprintln!(
                "[whisper-stream] 窗 abs_start={} 解码 {} 段（推理 {infer_ms}ms）。",
                window.abs_start,
                curr_segs.len()
            );
        }

        // 推理耗时回传（不污染对外 JobEvent；供 RealtimeSession 聚合 RTF）
        if let Some(ref tx) = infer_tx {
            let _ = tx.try_send(infer_ms);
        }

        let (committed_delta, hypothesis, stability) = la2.feed(window.abs_start, curr_segs);
        all_committed.extend_from_slice(&committed_delta);

        // 只在有新内容时发 Partial（避免无意义空事件；padding 静音窗不产幻觉则 committed/hypothesis 均空）
        if (!committed_delta.is_empty() || !hypothesis.is_empty())
            && ev_tx
                .send(JobEvent::Partial(PartialPayload::Asr {
                    committed_delta,
                    hypothesis,
                    stability,
                }))
                .is_err()
        {
            return; // 订阅方已关闭
        }

        // drain 时读到关闭标记：处理完暂存窗（若有）再收尾。
        if stream_closed && stashed.is_none() {
            break;
        }
    }

    // 最后取消检测（stream 关闭后）
    if cancel.is_cancelled() {
        let _ = ev_tx.send(JobEvent::Cancelled);
        return;
    }

    // Flush：将 prev_segs 中尚未 committed 的剩余段全部 committed
    let remaining = la2.flush();
    if !remaining.is_empty() {
        all_committed.extend_from_slice(&remaining);
        // 最后一批 committed delta（hypothesis 已清空）
        if ev_tx
            .send(JobEvent::Partial(PartialPayload::Asr {
                committed_delta: remaining,
                hypothesis: String::new(),
                stability: 1.0,
            }))
            .is_err()
        {
            return;
        }
    }

    let total_audio_secs = all_committed
        .last()
        .map(|s| s.end.as_secs_f32())
        .unwrap_or(0.0);

    let _ = ev_tx.send(JobEvent::Finished(JobResult {
        job_id,
        provider_id,
        model_id,
        payload: ResultPayload::Segments(all_committed),
        usage: Usage {
            audio_seconds: Some(total_audio_secs),
            ..Usage::default()
        },
        timings: Timings::default(),
        output_truncated: None,
        finished_at: Timestamp::ZERO,
    }));
}

// ─── 端点整段批量路径（PCM batch，原有逻辑） ─────────────────────────────────

/// 把 Job 的音频解析为 **16k mono f32**（whisper 输入约定，批量路径）。
fn pcm_16k_mono(job: &JobKind) -> Result<Vec<f32>, JobError> {
    let JobKind::Transcribe { audio, .. } = job else {
        return Err(JobError::InvalidInput("非转写 Job".into()));
    };
    match audio {
        AudioRef::Pcm(PcmBuffer {
            sample_rate,
            channels,
            samples,
        }) => {
            if *sample_rate != WHISPER_SAMPLE_RATE {
                return Err(JobError::InvalidInput(format!(
                    "whisper 需 16k mono，收到 {sample_rate}Hz（应由 luna-audio 归一化）"
                )));
            }
            if *channels != 1 {
                return Err(JobError::InvalidInput(format!(
                    "whisper 需 16k mono，收到 {channels} 声道（应由 luna-audio 降混）"
                )));
            }
            Ok(samples.clone())
        }
        // 文件批量转写：编排层（重转写/导入服务）用 `luna_audio::decode_media_to_pcm` 解码为
        // 16k mono PCM 后以 `AudioRef::Pcm` 提交——引擎保持纯推理、零设备/文件 IO 依赖
        // （luna-core `AudioRef::File` 文档：路径，不在此做 IO）。故此分支为契约兜底，正常流不触达。
        AudioRef::File(_) => Err(JobError::InvalidInput(
            "AudioRef::File 须由编排层解码为 Pcm 后提交（引擎不做文件 IO）".into(),
        )),
        AudioRef::Stream(_) => Err(JobError::InvalidInput(
            "Stream 路径不经 pcm_16k_mono（应走 execute 流式分支）".into(),
        )),
        // 已上传到计算服务的音频（ADR-036 ①）：字节根本不在本机，本地引擎无从解码。
        // 报「本地引擎不接受远程音频」而不是「解码失败」——后者会让用户去查
        // 文件格式，而真正的问题是这个 Job 被路由到了错误的 Provider。
        AudioRef::Uploaded(_) => Err(JobError::InvalidInput(
            "AudioRef::Uploaded 是计算服务侧的音频，本地 whisper 引擎不接受\
             （应路由到远程转写 Provider）"
                .into(),
        )),
    }
}

fn lang_hint(job: &JobKind, cfg: &WhisperConfig) -> Option<String> {
    match job {
        JobKind::Transcribe { lang_hint, .. } => lang_hint
            .as_ref()
            .map(|l| l.as_str().to_owned())
            .or_else(|| cfg.default_lang.clone()),
        _ => cfg.default_lang.clone(),
    }
}

/// 会话级语言锁（sticky language）——根治 whisper "auto"（`set_language(None)`）模式下
/// **逐窗（流式）/逐块（批量分块）独立检测语言**导致的语言漂移（TDD-03 §3.6）。
///
/// 第一性：一场录音/一段音频本是**单一主语言**；whisper 对每个解码窗口独立判语种时，
/// 停顿、口音、背景音、极短片段会被误判成他语（中文录音里冒出韩/日/法片段）。
///
/// 策略：
/// - **用户显式指定语言**（`transcribe_lang != auto`）→ `pinned`，恒用该语言，永不被检测覆盖；
/// - **auto** → 首个**产出实质文本**的窗/块所检测出的语言即锁定，后续全部窗/块复用之
///   （静音/空窗不据以锁定，防误锁——见 [`decode_window_with_abs_time`] 仅在有段时回传语言）。
struct StickyLanguage {
    /// 当前锁定语言（BCP-47，如 `"zh"`）；`None` = auto 且尚未锁定（让 whisper 检测本段）。
    locked: Option<String>,
    /// 用户是否显式指定：`true` 时永不被自动检测结果覆盖。
    pinned: bool,
}

impl StickyLanguage {
    /// `configured` = 解析后的转写语言（`Some` = 用户显式指定；`None` = auto）。
    fn new(configured: Option<String>) -> Self {
        Self {
            pinned: configured.is_some(),
            locked: configured,
        }
    }

    /// 本段解码应传给 whisper 的语言（`None` = 让 whisper 自行检测本段，仅 auto 未锁定时）。
    fn current(&self) -> Option<&str> {
        self.locked.as_deref()
    }

    /// 观察本段 whisper 检测出的语言 id（`None` = 本段无实质文本，不据以锁定）：
    /// auto 模式下**首个有效检测即锁定**，此后所有段复用该语言。
    fn observe(&mut self, detected_lang_id: Option<i32>) {
        if self.pinned || self.locked.is_some() {
            return;
        }
        if let Some(code) = detected_lang_id.and_then(get_lang_str) {
            self.locked = Some(code.to_owned());
        }
    }

    /// 现场重配（会话中经面板下拉切换，PRD-04-046）：`Some` = 显式指定并锁定该语言；
    /// `None` = 切回 auto——**清空已锁语言**，下一个有实质文本的窗口重新检测再锁定。
    fn reconfigure(&mut self, configured: Option<String>) {
        self.pinned = configured.is_some();
        self.locked = configured;
    }

    /// auto 模式下已检测锁定的语言（BCP-47；面板「自动检测（中文）」回显用）。
    /// 显式指定（pinned）时回 `None`——用户自己选的语言无需回显检测结果。
    fn auto_detected(&self) -> Option<&str> {
        if self.pinned {
            None
        } else {
            self.locked.as_deref()
        }
    }
}

/// 流式会话的**共享**语言锁句柄：解码线程（每窗读/写）与控制端（现场切换/检测回显）
/// 各持同一 `Arc`。锁粒度 = 每窗一次微秒级 `Mutex`，相对百 ms 级窗解码可忽略；
/// 绝不进音频回调（ADR-009 无关此路径）。
#[derive(Clone)]
pub struct SharedStreamLang(Arc<Mutex<StickyLanguage>>);

impl SharedStreamLang {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(StickyLanguage::new(None))))
    }

    /// 会话开始时以解析后的 `lang_hint` 初始化（execute 流式分支调用）。
    fn init(&self, configured: Option<String>) {
        self.0.lock().unwrap().reconfigure(configured);
    }

    fn current(&self) -> Option<String> {
        self.0.lock().unwrap().current().map(str::to_owned)
    }

    fn observe(&self, detected_lang_id: Option<i32>) {
        self.0.lock().unwrap().observe(detected_lang_id);
    }
}

/// 语言控制面的领域实现。`observe(lang_id)` **不在** trait 里：那是 whisper 的
/// 语言编号，属引擎私有（见 `StreamLangControl` 文档）。
impl luna_core::StreamLangControl for SharedStreamLang {
    fn set_configured(&self, configured: Option<String>) {
        self.0.lock().unwrap().reconfigure(configured);
    }

    fn auto_detected(&self) -> Option<String> {
        self.0.lock().unwrap().auto_detected().map(str::to_owned)
    }
}

fn initial_prompt(job: &JobKind) -> Option<String> {
    match job {
        JobKind::Transcribe { initial_prompt, .. } => initial_prompt
            .as_deref()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// 合成 whisper 解码参数（含幻觉抑制一等参数 + abort 接 CancelToken）。
///
/// **🐛 实测踩坑 + 干净规避（结论如实标注，重要）**：whisper-rs **0.14 与 0.16 的
/// `set_abort_callback_safe` 都有同一处 UB**——其 trampoline 用 `*mut F`（**调用方传入的
/// 闭包具体类型**）解引用，但实际存进 `abort_callback_user_data` 的是 `*mut Box<dyn FnMut>`。
/// 若直接传一个具体闭包（`F = {closure}`），二者**类型错位**，trampoline 读到错位内存、**随机
/// 返回 true 致 whisper 中途 abort → 转写结果随机为空**（本机复现：传具体闭包约半数空；下面写法
/// 15/15 确定通过）。
///
/// **规避（纯安全、零 unsafe）**：**把闭包显式装成 `Box<dyn FnMut() -> bool>` 再传**，令
/// `F = Box<dyn FnMut>`——此时 trampoline 的 `*mut F == *mut Box<dyn FnMut>` 正好与它存储的指针
/// 类型一致，类型自洽、不再错位。turbofish 定死 `O`/`F` 防 `Into<Option<F>>` 推断歧义。这样**既
/// 恢复了 ADR-010/TDD-01 §5.5 的 mid-decode 即时 abort，又无 UB、无 unsafe**。
/// 防御纵深：`execute` 仍在推理前、`run_blocking_transcribe` 仍在推理后各查一次 `CancelToken`
/// （取消必达，绝不注入已取消结果，PRD-02-005）。
fn build_params<'a>(
    cfg: &WhisperConfig,
    lang: Option<&'a str>,
    initial_prompt: Option<&'a str>,
    cancel: &CancelToken,
    batch: bool,
) -> FullParams<'a, 'a> {
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_n_threads(cfg.n_threads);
    p.set_translate(false);
    p.set_language(lang);
    p.set_print_special(false);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_timestamps(false);
    p.set_debug_mode(false);
    p.set_suppress_blank(true);
    p.set_suppress_nst(true);
    p.set_no_speech_thold(NO_SPEECH_THOLD);
    p.set_logprob_thold(LOGPROB_THOLD);
    p.set_entropy_thold(ENTROPY_THOLD);
    // **批量长音频去重（问题4根治，TDD-03 §2.3）**：整段/长分块解码必须
    // (a) `no_context` 断开跨分块 prompt 传递——`condition_on_previous_text` 默认开，一旦某段
    //     出现重复即被当作 prompt 反馈放大，是长音频"递增式重复自激循环"的根源；
    // (b) `temperature_inc` 开启温度回退——贪心（temp=0）遇到高压缩比/低 logprob 的循环无法自拔，
    //     回退到更高温度重采样即打断循环。二者仅批量路启用，实时流式路保持 temp=0 低延迟不回退。
    if batch {
        p.set_temperature(0.0);
        p.set_temperature_inc(0.2);
        p.set_no_context(true);
        // 词级时间戳（TDD-03 §2.3/§10.3，仅批量档）：token 级 t0/t1 由 whisper 轻量启发式
        // 算出（无 DTW，开销小；真机 RTF 对比留验收记录），`collect_segments` 聚合为
        // `Segment.words` 供骑跨段说话人切开。流式路**不开**（实时性能预算，短窗重解码
        // 的词界也不稳定，明确不做）。
        p.set_token_timestamps(true);
    } else {
        // 流式：每窗独立 create_state（无跨窗上下文）、短窗重解码，保持零回退最低延迟。
        p.set_temperature(0.0);
    }
    if let Some(prompt) = initial_prompt {
        p.set_initial_prompt(prompt);
    }
    let cancel = cancel.clone();
    type AbortCb = Box<dyn FnMut() -> bool>;
    let cb: AbortCb = Box::new(move || cancel.is_cancelled());
    p.set_abort_callback_safe::<Option<AbortCb>, AbortCb>(Some(cb));
    p
}

/// 把一段的 token 序列 `(文本, t0cs, t1cs)` 聚合为词级时间戳（TDD-03 §2.3 口径）：
///
/// - **拉丁系**：token 以空格起始 = 新词开始（whisper BPE 的词界约定）；
/// - **CJK**（无空格）：逐 token 即"词"（token 级近似，切开精度验收放宽到 ±1 词）；
/// - 特殊标记 token（`[_BEG_]`/`[_TT_…]` 等 `[_` 前缀）跳过；
/// - `WordTiming.text` 保留原始拼写（含拉丁词的前导空格）——骑跨切开按词重组段文本时
///   直接拼接后 trim 即还原原文（`retranscribe::assign_speakers`）。
///
/// 纯函数，headless 单测。
fn aggregate_word_timings(tokens: &[(String, i64, i64)]) -> Vec<WordTiming> {
    let mut words: Vec<WordTiming> = Vec::new();
    for (text, t0, t1) in tokens {
        if text.starts_with("[_") || text.is_empty() {
            continue;
        }
        let start = Duration::from_millis(t0.max(&0).unsigned_abs() * 10);
        let end = Duration::from_millis(t1.max(&0).unsigned_abs() * 10);
        let new_word = text.starts_with(' ') || words.is_empty();
        if new_word {
            words.push(WordTiming {
                text: text.clone(),
                start,
                end,
            });
        } else if let Some(last) = words.last_mut() {
            last.text.push_str(text);
            last.end = end.max(last.end);
        }
    }
    words
}

/// 从 `WhisperState` 抽取段（批量路径；时间戳相对于本次输入起点）。
/// 批量档开 `token_timestamps`（见 [`build_params`]），此处聚合 token → `Segment.words`。
fn collect_segments(state: &WhisperState) -> Vec<Segment> {
    let n = state.full_n_segments().unwrap_or(0);
    let mut segs = Vec::new();
    for i in 0..n {
        let text = state.full_get_segment_text_lossy(i).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let t0 = state.full_get_segment_t0(i).unwrap_or(0).max(0) as u64;
        let t1 = state.full_get_segment_t1(i).unwrap_or(0).max(0) as u64;
        // 词级时间戳（TDD-03 §10.3 骑跨切开的输入）：token 文本 + token t0/t1 聚合。
        let n_tokens = state.full_n_tokens(i).unwrap_or(0);
        let tokens: Vec<(String, i64, i64)> = (0..n_tokens)
            .filter_map(|j| {
                let txt = state.full_get_token_text_lossy(i, j).ok()?;
                let data = state.full_get_token_data(i, j).ok()?;
                Some((txt, data.t0, data.t1))
            })
            .collect();
        let words = aggregate_word_timings(&tokens);
        segs.push(Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(t0 * 10),
            end: Duration::from_millis(t1 * 10),
            text: text.trim().to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: (!words.is_empty()).then_some(words),
        });
    }
    // 幻觉过滤（TDD-03 §2.3）：字幕署名/水印/重复循环在此统一收口（批量路径）。
    hallucination_filter().filter(segs)
}

/// 批量分块解码上限（~120s）：超过即分块解码。whisper.cpp 对长音频单次 `full()` 会跨其内部
/// 30s 窗传递 `condition_on_previous_text`——一旦出现重复即被反馈放大成"递增式重复自激循环"
/// （问题4 真机签名）。分块 + 逐块 fresh state + `no_context` 从根上截断该传播。
const BATCH_CHUNK_SAMPLES: usize = 120 * WHISPER_SAMPLE_RATE as usize;
/// 分块 overlap（~3s）：防跨块断词；overlap 内重复段由时间前沿去重合并剔除。
const BATCH_CHUNK_OVERLAP: usize = 3 * WHISPER_SAMPLE_RATE as usize;

/// 单块批量解码（fresh state；返回相对本块起点的段，已过幻觉过滤）。
///
/// 同时回传本块检测语言 id（仅 auto 且有实质文本时非 `None`）供 [`StickyLanguage`] 锁定，
/// 根治长音频分块时逐块独立检测的语言漂移。
fn decode_one_batch(
    ctx: &WhisperContext,
    cfg: &WhisperConfig,
    lang: Option<&str>,
    prompt: Option<&str>,
    samples: &[f32],
    cancel: &CancelToken,
) -> Result<(Vec<Segment>, Option<i32>), JobError> {
    let mut state = ctx
        .create_state()
        .map_err(|e| JobError::ResourceExhausted {
            detail: format!("create_state: {e}"),
        })?;
    let params = build_params(cfg, lang, prompt, cancel, true);
    state
        .full(params, samples)
        .map_err(|e| JobError::ResourceExhausted {
            detail: format!("whisper full: {e}"),
        })?;
    let segs = collect_segments(&state);
    let detected = detected_lang_for(&state, lang, &segs);
    Ok((segs, detected))
}

/// 时间前沿去重准入（批量分块合并）：段完全落在已提交前沿内、或中点在前沿之前（overlap
/// 重复），拒绝；否则接受并推进前沿。与流式 [`LocalAgreement2::admit`] 同源语义（DRY）。
fn admit_batch_seg(committed_end: &mut Duration, seg: &Segment) -> bool {
    if seg.end <= *committed_end {
        return false;
    }
    let midpoint = seg.start + (seg.end.saturating_sub(seg.start)) / 2;
    if midpoint < *committed_end {
        return false;
    }
    *committed_end = seg.end;
    true
}

/// 批量解码（长音频分块 + 时间前沿去重合并）：≤120s 单次解码；更长按 ~120s 分块、3s overlap，
/// 逐块 fresh state 解码后时间偏移、去重合并。返回会话 0-based 绝对时间的段序列。
fn decode_batch(
    ctx: &WhisperContext,
    cfg: &WhisperConfig,
    lang: Option<&str>,
    prompt: Option<&str>,
    samples: &[f32],
    cancel: &CancelToken,
) -> Result<Vec<Segment>, JobError> {
    // 会话级语言锁：auto 模式下首块检测语言即锁定后续块，根治长音频逐块语言漂移。
    let mut sticky = StickyLanguage::new(lang.map(str::to_owned));
    if samples.len() <= BATCH_CHUNK_SAMPLES {
        let (segs, _detected) =
            decode_one_batch(ctx, cfg, sticky.current(), prompt, samples, cancel)?;
        return Ok(segs);
    }
    let step = BATCH_CHUNK_SAMPLES - BATCH_CHUNK_OVERLAP;
    let mut merged: Vec<Segment> = Vec::new();
    let mut committed_end = Duration::ZERO;
    let mut off = 0usize;
    while off < samples.len() {
        if cancel.is_cancelled() {
            return Ok(merged); // 取消：返回已合并部分（上层据 cancel 判定 Cancelled）
        }
        let end = (off + BATCH_CHUNK_SAMPLES).min(samples.len());
        let chunk_offset = Duration::from_millis(off as u64 * 1000 / WHISPER_SAMPLE_RATE as u64);
        let (chunk_segs, detected) = decode_one_batch(
            ctx,
            cfg,
            sticky.current(),
            prompt,
            &samples[off..end],
            cancel,
        )?;
        sticky.observe(detected);
        for mut s in chunk_segs {
            s.start += chunk_offset;
            s.end += chunk_offset;
            // 词级时间戳与段同轴偏移（骑跨切开按词时间归属簇，时基必须一致）。
            if let Some(words) = &mut s.words {
                for w in words {
                    w.start += chunk_offset;
                    w.end += chunk_offset;
                }
            }
            if admit_batch_seg(&mut committed_end, &s) {
                merged.push(s);
            }
        }
        if end == samples.len() {
            break;
        }
        off += step;
    }
    // 合并后再过一遍幻觉过滤：收口跨块边界的重复循环（单块 collect_segments 只见块内）。
    Ok(hallucination_filter().filter(merged))
}

/// 在专用阻塞线程上跑批量转写（长音频分块去重），产出终态 `JobEvent`。
fn run_blocking_transcribe(
    ctx: Arc<WhisperContext>,
    cfg: WhisperConfig,
    job: Job,
    samples: Vec<f32>,
    cancel: CancelToken,
) -> JobEvent {
    let t_start = Instant::now();
    let lang = lang_hint(&job.kind, &cfg);
    let prompt = initial_prompt(&job.kind);

    match decode_batch(
        &ctx,
        &cfg,
        lang.as_deref(),
        prompt.as_deref(),
        &samples,
        &cancel,
    ) {
        Ok(_) if cancel.is_cancelled() => JobEvent::Cancelled,
        Ok(segments) => {
            let audio_seconds = samples.len() as f32 / WHISPER_SAMPLE_RATE as f32;
            JobEvent::Finished(JobResult {
                job_id: job.id,
                provider_id: cfg.provider_id.clone(),
                model_id: Some(ModelId::from(
                    cfg.model_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("whisper"),
                )),
                payload: ResultPayload::Segments(segments),
                usage: Usage {
                    audio_seconds: Some(audio_seconds),
                    ..Usage::default()
                },
                timings: Timings {
                    total_ms: t_start.elapsed().as_millis() as u64,
                    ..Timings::default()
                },
                output_truncated: None,
                finished_at: Timestamp::ZERO,
            })
        }
        Err(e) => {
            if cancel.is_cancelled() {
                JobEvent::Cancelled
            } else {
                JobEvent::Failed(e)
            }
        }
    }
}

// ─── WhisperProvider ──────────────────────────────────────────────────────────

fn context_params(cfg: &WhisperConfig) -> WhisperContextParameters<'static> {
    let mut params = WhisperContextParameters::default();
    params.use_gpu(matches!(cfg.backend, WhisperBackend::Metal));
    params
}

/// 进程内 whisper Provider。模型随构造加载并 warm-up，常驻供交互式低延迟（TDD-03 §4）。
///
/// `WhisperContext` 是 `Send + Sync`（whisper-rs `unsafe impl`），故以 `Arc` 共享给每个转写
/// 工作线程；**每条 ASR 流独占一个 `WhisperState`**（context 非线程安全的部分在 state，
/// TDD-02 §3.3 末），在工作线程内 `create_state`。
pub struct WhisperProvider {
    ctx: Arc<WhisperContext>,
    cfg: WhisperConfig,
    /// 流式 ASR 注册表：StreamId → (窗口接收端, 推理耗时发送端)。
    /// `execute` 收到 `AudioRef::Stream(id)` 时从此处 `take` 并接管所有权。
    ///
    /// 回收语义在 [`luna_core::StreamRegistry`] 里只写一次（三个流式实现共用）：此前这份
    /// 表只在 `execute` 里 `remove`，而走不到 `execute` 的会话（准入被拒 / 路由失败）
    /// 会连同它攥着的窗缓冲永久滞留。
    stream_registry: luna_core::StreamRegistry<StreamEntry>,
}

impl WhisperProvider {
    /// 已加载模型的文件路径（供 `MediaRef.model` 等元数据展示，不含权重内容）。
    #[must_use]
    pub fn model_path(&self) -> &std::path::Path {
        &self.cfg.model_path
    }

    /// 加载模型并 warm-up（一次极短 dummy 推理，预热 Metal shader/计算图，TDD-03 §4）。
    ///
    /// # Errors
    /// 模型加载失败时返回 [`WhisperError::Load`]。
    pub fn load(cfg: WhisperConfig) -> Result<Self, WhisperError> {
        whisper_rs::install_logging_hooks();
        let path = cfg
            .model_path
            .to_str()
            .ok_or_else(|| WhisperError::Load("模型路径非 UTF-8".into()))?;
        let ctx = WhisperContext::new_with_params(path, context_params(&cfg))
            .map_err(|e| WhisperError::Load(e.to_string()))?;
        let provider = Self {
            ctx: Arc::new(ctx),
            cfg,
            stream_registry: luna_core::StreamRegistry::new(),
        };
        provider.warm_up();
        Ok(provider)
    }

    /// 后端可用性探针：以指定后端加载模型并 warm-up（真实解码一次静音）。
    ///
    /// **必须在独立子进程调用**：whisper.cpp 的 Metal 初始化在部分环境会 **native crash**
    /// （非可捕获的 Rust `Err`，而是 SIGABRT/SIGSEGV），进程内探测会打崩主进程。子进程探针
    /// 以退出码表达结果：`Ok(())`→子进程 `exit(0)`；优雅失败→`Err`→`exit(1)`；原生崩溃→
    /// 子进程被信号杀死（父进程据非零退出判定该后端不可用，安全回退 CPU）。见
    /// `src-tauri` `whisper_backend`（TDD-03 §4、TDD-13 §3.1）。
    ///
    /// # Errors
    /// 后端下模型加载/预热失败时返回 [`WhisperError::Load`]。
    pub fn probe(
        model_path: impl Into<PathBuf>,
        backend: WhisperBackend,
    ) -> Result<(), WhisperError> {
        Self::load(WhisperConfig::new(model_path).with_backend(backend)).map(|_| ())
    }

    fn warm_up(&self) {
        if let Ok(mut state) = self.ctx.create_state() {
            let params = build_params(&self.cfg, None, None, &CancelToken::new(), false);
            let _ = state.full(params, &vec![0.0_f32; WHISPER_SAMPLE_RATE as usize]);
        }
    }

    /// 注册流式 ASR 会话，返回 [`StreamRegistration`]（含窗口发送端 + RTF 推理耗时接收端）。
    ///
    /// 调用方（Task 4 `RealtimeSession`）持有 `window_tx` 并向其投递 `SpeechWindow`；
    /// 会话结束时发 `None` 关闭流，`execute` 将 flush 剩余段后发出 `Finished`。
    fn register_stream_inner(&self, id: StreamId) -> luna_core::StreamRegistration {
        register_into(&self.stream_registry, id)
    }
}

/// 建通道 → 登记 → 交出句柄。
///
/// 与 `WhisperProvider` 分开是为了**能被测到**：构造 `WhisperProvider` 要真的加载一份
/// 权重（`WhisperContext`），于是「登记与回收成对」这条在真机之外一个字都验不了——而它
/// 恰恰是与模型完全无关的那一半。
fn register_into(
    registry: &luna_core::StreamRegistry<StreamEntry>,
    id: StreamId,
) -> luna_core::StreamRegistration {
    let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(128);
    let (inf_tx, inf_rx) = std::sync::mpsc::sync_channel::<u64>(64);
    let lang = SharedStreamLang::new();
    let release = registry.register(
        id,
        StreamEntry {
            window_rx: win_rx,
            infer_tx: inf_tx,
            lang: lang.clone(),
        },
    );
    luna_core::StreamRegistration {
        window_tx: win_tx,
        infer_rx: inf_rx,
        lang: std::sync::Arc::new(lang),
        release,
    }
}

/// 流式喂帧接缝的本机实现（`luna_core::StreamingAsr`）。
///
/// 采集侧经此 trait 拿句柄，于是它不需要知道自己在喂本机 whisper 还是远程转写
/// ——两条通道走同一个接缝，这是 ADR-038 能做「断线切回本地」的前提。
impl luna_core::StreamingAsr for WhisperProvider {
    fn register_stream(&self, id: StreamId) -> luna_core::StreamRegistration {
        self.register_stream_inner(id)
    }
}

#[async_trait]
impl AIProvider for WhisperProvider {
    fn id(&self) -> ProviderId {
        self.cfg.provider_id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: "本地 Whisper".into(),
            // 权重文件名（`ggml-base` 等）：归档进 `MediaRef.model` 的正是这个，
            // 它将来要回答「这段转写当时用了哪个权重」。
            model: self
                .model_path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned),
            location: ProviderLocation::Device,
            license_summary: Some("whisper.cpp MIT；模型权重许可见模型管理".into()),
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        let mut capabilities = std::collections::BTreeSet::new();
        capabilities.insert(Capability::Transcribe);
        CapabilitySet {
            capabilities,
            streaming: true, // 支持 AudioRef::Stream 流式路径（LocalAgreement-2，TDD-02 §3.3-B）
            location: ProviderLocation::Device,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let ctx = Arc::clone(&self.ctx);
        let cfg = self.cfg.clone();
        let stream_registry = self.stream_registry.clone();

        Box::pin(async_stream::stream! {
            yield JobEvent::Started;

            // 提前提取 StreamId（Copy），避免 `match &job.kind` 借用与后续 batch 路径 move job 冲突。
            let maybe_stream_id = match &job.kind {
                JobKind::Transcribe { audio: AudioRef::Stream(id), .. } => Some(*id),
                _ => None,
            };

            if let Some(stream_id) = maybe_stream_id {
                // ── 流式路径（AudioRef::Stream）────────────────────────────────
                let entry = match stream_registry.take(stream_id) {
                    Some(e) => e,
                    None => {
                        yield JobEvent::Failed(JobError::InvalidInput(
                            format!("StreamId {stream_id} 未注册到 WhisperProvider（先调 register_stream）"),
                        ));
                        return;
                    }
                };

                // 共享语言锁：以解析后的 lang_hint 初始化；控制端持同一句柄（StreamRegistration.lang）
                // 可现场切换/回读 auto 检测结果（PRD-04-046）。
                entry.lang.init(lang_hint(&job.kind, &cfg));
                let prompt = initial_prompt(&job.kind);
                let decode_fn =
                    make_whisper_decode_fn(Arc::clone(&ctx), cfg.clone(), entry.lang.clone(), prompt);
                let job_id = job.id;
                let model_id = Some(ModelId::from(
                    cfg.model_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("whisper"),
                ));

                let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
                let provider_id = cfg.provider_id.clone();

                let spawn_result = std::thread::Builder::new()
                    .name("luna-whisper-stream".into())
                    .spawn(move || {
                        run_streaming_transcribe(
                            job_id,
                            provider_id,
                            model_id,
                            entry.window_rx,
                            decode_fn,
                            cancel,
                            ev_tx,
                            Some(entry.infer_tx),
                        );
                    });

                if spawn_result.is_err() {
                    yield JobEvent::Failed(JobError::ResourceExhausted {
                        detail: "无法启动流式 ASR 线程".into(),
                    });
                    return;
                }

                while let Some(ev) = ev_rx.recv().await {
                    let done = matches!(
                        ev,
                        JobEvent::Finished(_) | JobEvent::Failed(_) | JobEvent::Cancelled
                    );
                    yield ev;
                    if done {
                        break;
                    }
                }
            } else {
                // ── 端点整段批量路径（AudioRef::Pcm）─────────────────────────
                let samples = match pcm_16k_mono(&job.kind) {
                    Ok(s) if !s.is_empty() => s,
                    Ok(_) => {
                        yield JobEvent::Failed(JobError::InvalidInput("空音频".into()));
                        return;
                    }
                    Err(e) => {
                        yield JobEvent::Failed(e);
                        return;
                    }
                };

                if cancel.is_cancelled() {
                    yield JobEvent::Cancelled;
                    return;
                }

                // 音频时长在推理前就是事实：先发一份用量快照，推理失败 / 被取消时账本上
                // 记的是这段真实喂进去的时长而不是 0（TDD-14 §3 修订注）。
                yield JobEvent::Usage(Usage {
                    audio_seconds: Some(samples.len() as f32 / WHISPER_SAMPLE_RATE as f32),
                    ..Usage::default()
                });

                // ADR-010：在专用阻塞线程跑 FFI 推理（绝不占用 tokio async worker）；结果经
                // oneshot 桥回异步流。线程内部 abort 回调轮询 CancelToken 即时中止。
                let (tx, rx) = tokio::sync::oneshot::channel();
                let builder = std::thread::Builder::new().name("luna-whisper-asr".into());
                let spawn = builder.spawn(move || {
                    let ev = run_blocking_transcribe(ctx, cfg, job, samples, cancel);
                    let _ = tx.send(ev);
                });
                if spawn.is_err() {
                    yield JobEvent::Failed(JobError::ResourceExhausted {
                        detail: "无法启动 ASR 线程".into(),
                    });
                    return;
                }

                match rx.await {
                    Ok(ev) => yield ev,
                    Err(_) => {
                        yield JobEvent::Failed(JobError::ResourceExhausted {
                            detail: "ASR 线程异常退出".into(),
                        })
                    }
                }
            }
        })
    }

    fn estimate(&self, job: &Job) -> Option<UsageEstimate> {
        if let JobKind::Transcribe {
            audio: AudioRef::Pcm(buf),
            ..
        } = &job.kind
        {
            let secs = buf.samples.len() as f32
                / (buf.sample_rate.max(1) as f32 * f32::from(buf.channels.max(1)));
            return Some(UsageEstimate {
                est_duration_ms: Some((secs * 300.0) as u64),
                ..UsageEstimate::default()
            });
        }
        None
    }
}

// ─── 测试 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // 语言控制面经接缝 trait 暴露（`observe(lang_id)` 仍是引擎私有）。
    use super::*;
    use luna_core::{JobId, JobOrigin, Lang};
    use luna_core::{StreamLangControl as _, StreamingAsr as _};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use whisper_rs::get_lang_id;

    // ── 词级时间戳聚合（TDD-03 §2.3，批量档）────────────────────────────────

    #[test]
    fn word_timings_aggregate_latin_by_leading_space() {
        // whisper BPE：" Hello"" wor""ld" → 2 词；子词并入前词、end 取末 token。
        let tokens = vec![
            (" Hello".to_string(), 0i64, 50i64),
            (" wor".to_string(), 50, 80),
            ("ld".to_string(), 80, 100),
        ];
        let words = aggregate_word_timings(&tokens);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, " Hello");
        assert_eq!(words[0].start, Duration::ZERO);
        assert_eq!(words[0].end, Duration::from_millis(500));
        assert_eq!(words[1].text, " world");
        assert_eq!(words[1].start, Duration::from_millis(500));
        assert_eq!(words[1].end, Duration::from_millis(1000));
    }

    #[test]
    fn word_timings_cjk_tokens_stand_alone_and_specials_skipped() {
        // CJK 无空格：逐 token 即"词"（口径：token 级近似）；[_BEG_] 等特殊标记跳过。
        // 注意：CJK token 无前导空格 → 依"首词开新"规则后续并入前词是错误的——
        // 但 whisper CJK token 常为单字且中间无空格，聚合口径即按 token 序列首 token 开词、
        // 后续无空格 token 并入（形成短语级"词"）；切开精度验收放宽 ±1 词（TDD-03 §10.3）。
        let tokens = vec![
            ("[_BEG_]".to_string(), 0i64, 0i64),
            ("你好".to_string(), 0, 30),
            ("世界".to_string(), 30, 60),
        ];
        let words = aggregate_word_timings(&tokens);
        assert!(!words.is_empty());
        let joined: String = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined, "你好世界", "特殊标记不入文本，正文无损");
        // 单调性：起止时间不回退。
        let mut prev_end = Duration::ZERO;
        for w in &words {
            assert!(w.start <= w.end, "词内起止有序");
            assert!(w.end >= prev_end, "词间时间单调不回退");
            prev_end = w.end;
        }
    }

    #[test]
    fn word_timings_empty_and_special_only_yield_none() {
        assert!(aggregate_word_timings(&[]).is_empty());
        assert!(aggregate_word_timings(&[("[_TT_50]".to_string(), 0, 50)]).is_empty());
    }

    // ── 会话级语言锁（StickyLanguage）─────────────────────────────────────────

    #[test]
    fn sticky_language_pinned_never_overridden_by_detection() {
        // 用户显式指定 zh：current 恒 zh，即便检测出 en 也不被覆盖（根治：显式语言绝对优先）。
        let mut sticky = StickyLanguage::new(Some("zh".to_owned()));
        assert_eq!(sticky.current(), Some("zh"));
        let en = get_lang_id("en");
        sticky.observe(en);
        assert_eq!(sticky.current(), Some("zh"), "显式语言不得被检测覆盖");
    }

    #[test]
    fn sticky_language_auto_locks_on_first_detection() {
        // auto：起始 current=None（让 whisper 检测本段）；首次检测到 zh 即锁定后续。
        let mut sticky = StickyLanguage::new(None);
        assert_eq!(sticky.current(), None);
        sticky.observe(get_lang_id("zh"));
        assert_eq!(sticky.current(), Some("zh"), "首次检测即锁定");
        // 后续窗口即便检测成 en 也不再改（根治逐窗漂移）。
        sticky.observe(get_lang_id("en"));
        assert_eq!(sticky.current(), Some("zh"), "锁定后不再随后续检测漂移");
    }

    #[test]
    fn shared_stream_lang_reconfigure_switches_and_reenters_auto() {
        // 现场切换（PRD-04-046）：auto 已锁 zh → 显式切 en → 回 auto 清锁重新检测。
        let shared = SharedStreamLang::new();
        shared.init(None); // auto
        shared.observe(get_lang_id("zh"));
        assert_eq!(shared.current().as_deref(), Some("zh"));
        assert_eq!(
            shared.auto_detected().as_deref(),
            Some("zh"),
            "auto 检测结果可回显"
        );

        shared.set_configured(Some("en".to_owned()));
        assert_eq!(shared.current().as_deref(), Some("en"), "显式切换立即生效");
        assert_eq!(shared.auto_detected(), None, "显式指定不回显检测");
        shared.observe(get_lang_id("ko"));
        assert_eq!(
            shared.current().as_deref(),
            Some("en"),
            "显式语言不被检测覆盖"
        );

        shared.set_configured(None);
        assert_eq!(shared.current(), None, "回 auto 清锁，重新检测");
        shared.observe(get_lang_id("ja"));
        assert_eq!(
            shared.current().as_deref(),
            Some("ja"),
            "回 auto 后首检重新锁定"
        );
        assert_eq!(shared.auto_detected().as_deref(), Some("ja"));
    }

    #[test]
    fn sticky_language_auto_ignores_empty_and_invalid_detection() {
        // 空段（None）/无效 id 不锁定，保持 auto 直到首个有效检测（防静音窗误锁）。
        let mut sticky = StickyLanguage::new(None);
        sticky.observe(None);
        assert_eq!(sticky.current(), None, "空段不锁定");
        sticky.observe(Some(-1));
        assert_eq!(sticky.current(), None, "无效 id 不锁定");
        sticky.observe(get_lang_id("ko"));
        assert_eq!(sticky.current(), Some("ko"));
    }

    // ── 测试辅助 ────────────────────────────────────────────────────────────────

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

    fn stream_job(id: StreamId) -> Job {
        Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(id),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::Recording,
        )
    }

    fn pcm_job(buf: PcmBuffer) -> Job {
        Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Pcm(buf),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::VoiceInput,
        )
    }

    /// 确定性 stub：按序返回预设解码结果（无需 whisper 模型）。
    fn make_stub_decode(responses: Vec<Vec<Segment>>) -> DecodeFn {
        let responses = Arc::new(std::sync::Mutex::new(responses));
        let idx = Arc::new(AtomicUsize::new(0));
        Box::new(
            move |_samples: &[f32], _abs_start: u64, _cancel: &CancelToken| {
                let i = idx.fetch_add(1, Ordering::SeqCst);
                let segs = responses
                    .lock()
                    .unwrap()
                    .get(i)
                    .cloned()
                    .unwrap_or_default();
                (segs, 5)
            },
        )
    }

    // ── LocalAgreement2 单元测试 ─────────────────────────────────────────────────

    #[test]
    fn la2_first_window_produces_no_committed() {
        let mut la2 = LocalAgreement2::new();
        let (committed, hypothesis, _) = la2.feed(0, vec![seg(0, 1000, "hello world")]);
        assert!(
            committed.is_empty(),
            "第一窗尚无前窗可对比，committed 应为空"
        );
        assert_eq!(hypothesis, "hello world");
    }

    #[test]
    fn la2_second_window_commits_agreed_prefix() {
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "hello"), seg(1000, 2000, "world")]);
        let (committed, hypothesis, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "hello"),
                seg(1000, 2000, "world"),
                seg(2000, 3000, "today"),
            ],
        );
        assert_eq!(
            committed.len(),
            2,
            "两窗均含 hello+world → 应 commit 这 2 段"
        );
        assert_eq!(committed[0].text, "hello");
        assert_eq!(committed[1].text, "world");
        assert_eq!(hypothesis, "today");
    }

    #[test]
    fn la2_committed_does_not_repeat_prior_committed() {
        // committed 段只出现一次（不重复发出已 committed 的段）
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "a"), seg(1000, 2000, "b")]);
        let (d1, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
            ],
        );
        // d1 committed a,b
        let (d2, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
                seg(3000, 4000, "d"),
            ],
        );
        // d2 应只 commit c，不重复 a,b
        assert_eq!(d1.len(), 2, "第一次 commit a,b");
        assert_eq!(d2.len(), 1, "第二次只 commit c（a,b 已发出）");
        assert_eq!(d2[0].text, "c");
    }

    #[test]
    fn la2_committed_start_is_monotonic() {
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "a"), seg(1000, 2000, "b")]);
        let (d1, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
            ],
        );
        let (d2, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
                seg(3000, 4000, "d"),
            ],
        );
        let all: Vec<&Segment> = d1.iter().chain(d2.iter()).collect();
        for w in all.windows(2) {
            assert!(w[1].start >= w[0].start, "committed 起点须单调非递减");
        }
    }

    #[test]
    fn la2_flush_commits_remaining_tail() {
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "hello")]);
        la2.feed(0, vec![seg(0, 1000, "hello"), seg(1000, 2000, "world")]);
        // 此时 hello committed, prev=[hello,world], committed_count=1
        let remaining = la2.flush();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].text, "world");
    }

    #[test]
    fn la2_no_agreement_keeps_as_hypothesis() {
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "hello")]);
        // 完全不同的文本 → agree=0 → committed 为空
        let (committed, hypothesis, _) = la2.feed(0, vec![seg(0, 1000, "goodbye world")]);
        assert!(committed.is_empty(), "无前缀一致则不 commit");
        assert_eq!(hypothesis, "goodbye world");
    }

    #[test]
    fn la2_new_window_finalizes_previous_window_segments() {
        // 真机数据丢失根因（K1）回归：StreamSegmenter 产互不重叠 utterance 窗，
        // 各窗文本必然不同——纯前缀一致永不触发。窗界感知须在新窗到来时定稿上一窗全部段。
        let mut la2 = LocalAgreement2::new();
        // 窗 0（abs=0）：utterance A
        let (c0, h0, _) = la2.feed(0, vec![seg(0, 1000, "第一段话")]);
        assert!(c0.is_empty(), "首窗暂为 hypothesis");
        assert_eq!(h0, "第一段话");
        // 窗 1（abs=48000，新窗、内容完全不同）：上一窗应即定稿，绝不丢弃
        let (c1, h1, _) = la2.feed(48_000, vec![seg(3000, 4000, "第二段话")]);
        assert_eq!(
            c1.len(),
            1,
            "新窗到来 → 上一窗全部段定稿（修复前此处为空 = 丢失）"
        );
        assert_eq!(c1[0].text, "第一段话");
        assert_eq!(h1, "第二段话");
        // 窗 2（abs=96000）：同理定稿窗 1
        let (c2, _, _) = la2.feed(96_000, vec![seg(6000, 7000, "第三段话")]);
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].text, "第二段话");
        // flush 定稿最后一窗
        let tail = la2.flush();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].text, "第三段话");
    }

    #[test]
    fn la2_disjoint_windows_lose_nothing_end_to_end() {
        // 不变量：N 个互斥窗各 1 段 → committed(沿途) + flush(末窗) 合计恰 N 段、顺序保持。
        let mut la2 = LocalAgreement2::new();
        let mut all: Vec<Segment> = Vec::new();
        for i in 0..5u64 {
            let (c, _, _) = la2.feed(
                i * 50_000,
                vec![seg(i * 2000, i * 2000 + 1000, &format!("u{i}"))],
            );
            all.extend(c);
        }
        all.extend(la2.flush());
        assert_eq!(all.len(), 5, "5 个互斥窗的段必须一个不丢");
        for (i, s) in all.iter().enumerate() {
            assert_eq!(s.text, format!("u{i}"), "顺序保持");
        }
    }

    #[test]
    fn la2_redecode_drift_never_recommits_committed_region() {
        // 真机重复根因（K3）回归：同窗重解码首段文本漂移（p → px）打断前缀一致后，
        // 后续一致提交绝不能把已提交的 p,q 区间重发（修复前表现为
        // "同时间戳重复行 / 截断版+完整版并存"）。
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "p"), seg(1000, 2000, "q")]);
        let (d1, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "p"),
                seg(1000, 2000, "q"),
                seg(2000, 3000, "r"),
            ],
        );
        assert_eq!(d1.len(), 2, "前缀一致 → 提交 p,q");
        // 漂移解码：首段变体 px 令 agree=0（修复前 committed_count 被拨回 0）。
        let (d2, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "px"),
                seg(1000, 2000, "q"),
                seg(2000, 3000, "r"),
                seg(3000, 4000, "s"),
            ],
        );
        assert!(d2.is_empty(), "漂移解码本身不产生提交");
        let (d3, _, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "px"),
                seg(1000, 2000, "q"),
                seg(2000, 3000, "r"),
                seg(3000, 4000, "s"),
                seg(4000, 5000, "t"),
            ],
        );
        let texts: Vec<&str> = d3.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["r", "s"],
            "只提交前沿之后的新段；px/q（已提交音频区间）绝不重发"
        );
    }

    #[test]
    fn la2_time_gate_rejects_redecode_variant_after_count_clamp() {
        // committed_count 因空解码被截顶回 0 后，时间前沿门控必须兜底：
        // 覆盖已提交音频、仅多出 <1s 尾巴的"完整版变体"拒绝提交（截断版+完整版并存根因）。
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 2000, "alpha beta")]);
        let (d1, _, _) = la2.feed(
            0,
            vec![seg(0, 2000, "alpha beta"), seg(2000, 3000, "gamma")],
        );
        assert_eq!(d1.len(), 1, "提交 alpha beta（前沿=2s）");
        la2.feed(0, vec![]); // 空解码 → committed_count 截顶为 0
        la2.feed(0, vec![seg(0, 2400, "alpha beta plus")]);
        let (d2, _, _) = la2.feed(
            0,
            vec![seg(0, 2400, "alpha beta plus"), seg(2400, 3400, "delta")],
        );
        assert!(
            d2.iter().all(|s| s.text != "alpha beta plus"),
            "重解码变体（中点在前沿前、推进仅 0.4s）必须被时间门控拒绝"
        );
        let (d3, _, _) = la2.feed(
            0,
            vec![
                seg(0, 2400, "alpha beta plus"),
                seg(2400, 3400, "delta"),
                seg(3400, 4000, "epsilon"),
            ],
        );
        let all: Vec<&str> = d2
            .iter()
            .chain(d3.iter())
            .map(|s| s.text.as_str())
            .collect();
        assert!(all.contains(&"delta"), "前沿之后的真实新段照常提交");
    }

    #[test]
    fn la2_hypothesis_excludes_committed_prefix_after_drift() {
        // 漂移后 hypothesis 不得复读已提交前缀（HUD 灰字重复的来源）。
        let mut la2 = LocalAgreement2::new();
        la2.feed(0, vec![seg(0, 1000, "a"), seg(1000, 2000, "b")]);
        la2.feed(
            0,
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
            ],
        );
        // 漂移：agree=0，但 committed_count 保持 2 → hypothesis 从索引 2 起。
        let (_, hypothesis, _) = la2.feed(
            0,
            vec![
                seg(0, 1000, "a2"),
                seg(1000, 2000, "b2"),
                seg(2000, 3000, "c"),
                seg(3000, 4000, "d"),
            ],
        );
        assert_eq!(hypothesis, "c d", "hypothesis 不含已提交的 a/b 区间文本");
    }

    #[test]
    fn la2_normalize_text_ignores_case_and_punctuation() {
        assert_eq!(
            normalize_seg_text("Hello, World!"),
            normalize_seg_text("hello world")
        );
        assert_eq!(normalize_seg_text("  多余   空格  "), "多余 空格");
    }

    // ── 绝对时钟换算 ─────────────────────────────────────────────────────────────

    #[test]
    fn abs_time_from_cs_converts_correctly() {
        // abs_start=32000 samples @16k = 2 seconds
        // cs=0 → 2.000s；cs=100 → 2.000 + 1.000 = 3.000s
        assert_eq!(abs_time_from_cs(32_000, 16_000, 0), Duration::from_secs(2));
        assert_eq!(
            abs_time_from_cs(32_000, 16_000, 100),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn abs_time_negative_cs_clamps_to_zero_offset() {
        // 负数 centiseconds → max(0) → offset=0
        assert_eq!(abs_time_from_cs(0, 16_000, -5), Duration::ZERO);
    }

    #[test]
    fn abs_time_zero_abs_start() {
        // abs_start=0 → segment 起点 = 窗口内的相对时间
        assert_eq!(
            abs_time_from_cs(0, 16_000, 50), // 50cs = 500ms
            Duration::from_millis(500)
        );
    }

    // ── 流式会话登记与回收 ──────────────────────────────────────────────────────

    /// **`execute` 没跑到的会话，登记同样要回收。**
    ///
    /// 采集侧先 `register_stream` 拿句柄、再 `submit(job)`，而 Job 有两条走不到 `execute`
    /// 的出口（会话池准入到期即拒 `LocalSessionBusy`、路由解析失败）。只由 `execute`
    /// 回收的话，滞留的条目攥着 `window_rx` 那一端——采集侧在 Job 失败之前一直在投窗，
    /// 单窗最大 ~10 秒 16 kHz f32，而本机这条通道深 128（`register_into`），一条滞留的
    /// 登记最多攥着约 80 MB；whisper Provider 实例在模型未换时是复用的，故它活到进程退出。
    ///
    /// 反向验证：把 `StreamRegistration.release` 换成 `StreamRelease::detached()`
    /// （= 回到只有 `execute` 会 remove），下面两条断言同时红。
    #[test]
    fn a_registration_execute_never_took_is_recycled_by_its_handle() {
        let registry = luna_core::StreamRegistry::new();
        let id = StreamId::new();
        let reg = register_into(&registry, id);
        assert_eq!(registry.len(), 1, "登记应在表里");

        // Job 在 execute 之前就失败了 —— 采集侧收到终态，丢掉回收句柄。
        drop(reg.release);

        assert!(registry.is_empty(), "登记滞留，它攥着的窗缓冲不会被释放");
        assert!(
            matches!(
                reg.window_tx.try_send(None),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            ),
            "接收端还在 = 那 128 窗还在"
        );
    }

    /// `execute` 摘走之后，采集侧那份句柄必须是空操作——它与 `execute` 的先后不确定，
    /// 抢在前面把表清掉的话，正在跑的会话会突然报「未注册」。
    #[test]
    fn the_handle_is_a_no_op_once_execute_took_the_entry() {
        let registry = luna_core::StreamRegistry::new();
        let id = StreamId::new();
        let reg = register_into(&registry, id);
        assert!(registry.take(id).is_some(), "execute 应能摘到自己那条");
        drop(reg);
        assert!(registry.is_empty());
    }

    // ── 流式执行路径（stub decode，确定性） ──────────────────────────────────────

    #[tokio::test]
    async fn streaming_finished_after_stream_end() {
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        let decode_fn = make_stub_decode(vec![
            vec![seg(0, 1000, "hello")],
            vec![seg(0, 1000, "hello"), seg(1000, 2000, "world")],
        ]);

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                None,
            );
        });

        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 160],
                abs_start: 0,
                speech_len: 160,
            }))
            .unwrap();
        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 160],
                abs_start: 2560,
                speech_len: 160,
            }))
            .unwrap();
        win_tx.send(None).unwrap(); // 关闭流

        let mut got_finished = false;
        let mut saw_after_terminal = false;
        let mut terminal_seen = false;
        while let Some(ev) = ev_rx.recv().await {
            if terminal_seen {
                saw_after_terminal = true;
            }
            match &ev {
                JobEvent::Finished(_) => {
                    got_finished = true;
                    terminal_seen = true;
                }
                JobEvent::Failed(_) | JobEvent::Cancelled => {
                    terminal_seen = true;
                }
                _ => {}
            }
        }
        assert!(got_finished, "流关闭后应产出 Finished");
        assert!(!saw_after_terminal, "Finished 后不得有更多事件");
    }

    #[tokio::test]
    async fn streaming_committed_is_monotonic() {
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        let decode_fn = make_stub_decode(vec![
            vec![seg(0, 1000, "a"), seg(1000, 2000, "b")],
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
            ],
            vec![
                seg(0, 1000, "a"),
                seg(1000, 2000, "b"),
                seg(2000, 3000, "c"),
                seg(3000, 4000, "d"),
            ],
        ]);

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                None,
            );
        });

        // 同一增长缓冲的三次重解码（stub 返回 a,b → a,b,c → a,b,c,d，绝对时间戳一致）
        // → abs_start 相同 = 同窗（窗界感知下前缀一致提交语义生效）。
        for _ in 0..3u64 {
            win_tx
                .send(Some(WindowInput {
                    samples: vec![0.0; 160],
                    abs_start: 0,
                    speech_len: 160,
                }))
                .unwrap();
        }
        win_tx.send(None).unwrap();

        let mut committed: Vec<Segment> = Vec::new();
        while let Some(ev) = ev_rx.recv().await {
            match ev {
                JobEvent::Partial(PartialPayload::Asr {
                    committed_delta, ..
                }) => {
                    for s in committed_delta {
                        if let Some(last) = committed.last() {
                            assert!(
                                s.start >= last.start,
                                "committed 起点必须单调非递减：{:?} >= {:?}",
                                s.start,
                                last.start
                            );
                        }
                        committed.push(s);
                    }
                }
                JobEvent::Finished(_) => break,
                _ => {}
            }
        }
        assert!(!committed.is_empty(), "应有 committed 段");
    }

    #[tokio::test]
    async fn streaming_hypothesis_is_tail_only_on_first_window() {
        // 第一窗：无前窗对比 → committed 空，hypothesis = 全文
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        let decode_fn = make_stub_decode(vec![vec![seg(0, 1000, "hello world")]]);

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                None,
            );
        });

        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 160],
                abs_start: 0,
                speech_len: 160,
            }))
            .unwrap();
        win_tx.send(None).unwrap();

        // 收集所有事件直到 Finished
        let mut partials: Vec<(Vec<Segment>, String)> = Vec::new();
        while let Some(ev) = ev_rx.recv().await {
            if let JobEvent::Partial(PartialPayload::Asr {
                committed_delta,
                hypothesis,
                ..
            }) = ev
            {
                partials.push((committed_delta, hypothesis));
            } else if matches!(ev, JobEvent::Finished(_)) {
                break;
            }
        }
        // 第一窗：无前窗对比 → 第一个 Partial 的 committed 应为空，hypothesis 应包含全文
        let (first_committed, first_hyp) = partials.first().expect("应发出至少一个 Partial");
        assert!(
            first_committed.is_empty(),
            "第一窗 committed 应为空（无前窗对比）"
        );
        assert!(!first_hyp.is_empty(), "第一窗 hypothesis 应包含全文");
    }

    #[tokio::test]
    async fn streaming_cancel_reaches_cancelled_terminal() {
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        // decode fn 阻塞直到 cancel，模拟长推理
        let decode_fn: DecodeFn = Box::new(|_samples, _abs_start, cancel| {
            while !cancel.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            (vec![], 0)
        });

        let cancel = CancelToken::new();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel_clone,
                ev_tx,
                None,
            );
        });

        // 触发一次解码（让线程进入 decode_fn）
        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 160],
                abs_start: 0,
                speech_len: 160,
            }))
            .unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;
        cancel.cancel();
        drop(win_tx);

        let mut got_cancelled = false;
        while let Some(ev) = ev_rx.recv().await {
            if matches!(ev, JobEvent::Cancelled) {
                got_cancelled = true;
                break;
            }
        }
        assert!(got_cancelled, "取消必须传达为 Cancelled 终态");
    }

    #[tokio::test]
    async fn streaming_silent_window_produces_no_partial() {
        // padding 静音窗 decode 返回空段 → 不幻觉、不产出 Partial
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        let decode_fn = make_stub_decode(vec![vec![]]); // 静音：decode 返回空段

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                None,
            );
        });

        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 16_000],
                abs_start: 0,
                speech_len: 16_000,
            }))
            .unwrap();
        win_tx.send(None).unwrap();

        let mut partial_count = 0usize;
        while let Some(ev) = ev_rx.recv().await {
            if matches!(ev, JobEvent::Partial(_)) {
                partial_count += 1;
            }
            if matches!(ev, JobEvent::Finished(_)) {
                break;
            }
        }
        assert_eq!(partial_count, 0, "静音窗不应产出 Partial（不幻觉）");
    }

    #[tokio::test]
    async fn streaming_coalesces_stale_snapshots_of_same_window() {
        // 增长快照合并：预先排队同 abs_start 的 3 个快照 + 一个新窗 + 关闭标记，
        // worker 只应解码 2 次（最新快照 + 新窗），且第一次解的是最新快照（样本最长）。
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

        let decoded: Arc<std::sync::Mutex<Vec<(usize, u64)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let decoded_clone = Arc::clone(&decoded);
        let decode_fn: DecodeFn = Box::new(move |samples, abs_start, _cancel| {
            decoded_clone
                .lock()
                .unwrap()
                .push((samples.len(), abs_start));
            (vec![], 1)
        });

        // 先排满再起线程，保证 drain 语义确定性。
        for len in [160usize, 320, 480] {
            win_tx
                .send(Some(WindowInput {
                    samples: vec![0.0; len],
                    abs_start: 0,
                    speech_len: len,
                }))
                .unwrap();
        }
        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 640],
                abs_start: 48_000,
                speech_len: 640,
            }))
            .unwrap();
        win_tx.send(None).unwrap();

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                None,
            );
        });

        while let Some(ev) = ev_rx.recv().await {
            if matches!(ev, JobEvent::Finished(_)) {
                break;
            }
        }

        let calls = decoded.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![(480, 0), (640, 48_000)],
            "同 abs_start 过期快照应被合并跳过，只解最新 + 下一窗"
        );
    }

    #[tokio::test]
    async fn streaming_infer_ms_is_reported_via_internal_channel() {
        let (win_tx, win_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(32);
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(32);

        let decode_fn = make_stub_decode(vec![vec![seg(0, 1000, "hello")]]);

        let cancel = CancelToken::new();
        std::thread::spawn(move || {
            run_streaming_transcribe(
                JobId::new(),
                ProviderId::from(PROVIDER_ID),
                None,
                win_rx,
                decode_fn,
                cancel,
                ev_tx,
                Some(infer_tx),
            );
        });

        win_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 160],
                abs_start: 0,
                speech_len: 160,
            }))
            .unwrap();
        win_tx.send(None).unwrap();

        while let Some(ev) = ev_rx.recv().await {
            if matches!(ev, JobEvent::Finished(_)) {
                break;
            }
        }

        // stub 返回 infer_ms=5；应从 infer_rx 可收到
        let infer_ms = infer_rx.try_recv().expect("infer_ms 应经内部通道回传");
        assert_eq!(infer_ms, 5, "infer_ms 应为 stub 返回值 5");
    }

    // ── 批量路径（PCM）── 保持原有测试 ─────────────────────────────────────────

    fn transcribe_job(buf: PcmBuffer, lang: Option<Lang>) -> Job {
        Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Pcm(buf),
                lang_hint: lang,
                initial_prompt: None,
            },
            JobOrigin::VoiceInput,
        )
    }

    #[test]
    fn pcm_extraction_rejects_non_mono() {
        let buf = PcmBuffer {
            sample_rate: 16_000,
            channels: 2,
            samples: vec![1.0, 0.0],
        };
        assert!(matches!(
            pcm_16k_mono(&transcribe_job(buf, None).kind),
            Err(JobError::InvalidInput(_))
        ));
    }

    #[test]
    fn default_config_uses_cpu_until_gpu_probe_is_safe() {
        let cfg = WhisperConfig::new("x");
        assert_eq!(cfg.backend, WhisperBackend::Cpu);
    }

    #[test]
    fn pcm_extraction_rejects_non_16k() {
        let buf = PcmBuffer {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.0; 100],
        };
        assert!(matches!(
            pcm_16k_mono(&transcribe_job(buf, None).kind),
            Err(JobError::InvalidInput(_))
        ));
    }

    #[test]
    fn lang_hint_prefers_job_then_default() {
        let cfg = WhisperConfig {
            model_path: "x".into(),
            n_threads: 4,
            default_lang: Some("zh".into()),
            backend: WhisperBackend::Cpu,
            provider_id: ProviderId::from(PROVIDER_ID),
        };
        let job = transcribe_job(
            PcmBuffer {
                sample_rate: 16_000,
                channels: 1,
                samples: vec![],
            },
            Some(Lang::from("en")),
        );
        assert_eq!(lang_hint(&job.kind, &cfg).as_deref(), Some("en"));
        let job2 = transcribe_job(
            PcmBuffer {
                sample_rate: 16_000,
                channels: 1,
                samples: vec![],
            },
            None,
        );
        assert_eq!(lang_hint(&job2.kind, &cfg).as_deref(), Some("zh"));
    }

    #[test]
    fn capabilities_streaming_is_true() {
        // WhisperProvider 声明 streaming=true（支持 AudioRef::Stream 流式路径）
        // 无需加载模型即可验证 capabilities 的静态属性
        let caps = CapabilitySet {
            capabilities: [Capability::Transcribe].into_iter().collect(),
            streaming: true,
            location: ProviderLocation::Device,
            explicit_only: false,
        };
        assert!(caps.streaming);
        assert!(caps.supports(Capability::Transcribe));
    }

    // ── 真模型端到端（#[ignore]，需真机） ────────────────────────────────────────

    /// 真模型端到端（默认 ignore；用 S3 探针下载的 tiny 模型 + wav 手动验）：
    /// `LUNA_WHISPER_MODEL=/tmp/luna-s3-whisper-probe/ggml-tiny.bin`
    /// `LUNA_WHISPER_WAV=/tmp/luna-s3-whisper-probe/jfk.wav cargo test -p luna-engine -- --ignored`
    #[test]
    #[ignore = "需真实模型 + wav（真机）"]
    fn real_model_transcribes_jfk() {
        use futures::StreamExt as _;
        let model = std::env::var("LUNA_WHISPER_MODEL").expect("set LUNA_WHISPER_MODEL");
        let wav = std::env::var("LUNA_WHISPER_WAV").expect("set LUNA_WHISPER_WAV");
        let provider = WhisperProvider::load(WhisperConfig::new(model)).unwrap();
        let mut reader = hound::WavReader::open(&wav).unwrap();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        let job = pcm_job(PcmBuffer {
            sample_rate: 16_000,
            channels: 1,
            samples,
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut stream = provider.execute(job, CancelToken::new()).await;
            let mut text = String::new();
            while let Some(ev) = stream.next().await {
                if let JobEvent::Finished(result) = ev {
                    if let ResultPayload::Segments(segs) = result.payload {
                        for s in segs {
                            text.push_str(&s.text);
                        }
                    }
                }
            }
            assert!(
                text.to_lowercase().contains("country"),
                "JFK 转写应含 'country', got: {text}"
            );
        });
    }

    /// 并发共享 ctx 解码一致性（真机，Both 双流乱码根因回归锁）：同一段音频在同一
    /// `WhisperContext` 上「两线程并发解码 × N 轮」结果必须与「串行基线」逐字一致。
    /// Metal 后端并发 `full()` 会互相污染计算图 → 多语言乱码/破损 UTF-8（真机 Both 签名）。
    /// 运行：`LUNA_WHISPER_MODEL=… LUNA_WHISPER_WAV=…/jfk.wav cargo test -p luna-engine \
    /// real_model_concurrent -- --ignored --nocapture`
    #[test]
    #[ignore = "需真实模型 + wav（真机）"]
    fn real_model_concurrent_same_ctx_decodes_match_serial() {
        let model = std::env::var("LUNA_WHISPER_MODEL").expect("set LUNA_WHISPER_MODEL");
        let wav = std::env::var("LUNA_WHISPER_WAV").expect("set LUNA_WHISPER_WAV");
        let backend = match std::env::var("LUNA_WHISPER_BACKEND").as_deref() {
            Ok("cpu") => WhisperBackend::Cpu,
            _ => WhisperBackend::Metal,
        };
        let cfg = WhisperConfig::new(model).with_backend(backend);
        let provider = WhisperProvider::load(cfg.clone()).unwrap();
        let ctx = Arc::clone(&provider.ctx);
        let mut reader = hound::WavReader::open(&wav).unwrap();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();

        let decode_text = |ctx: &WhisperContext, cfg: &WhisperConfig, samples: &[f32]| -> String {
            let (segs, _ms, _lang) = decode_window_with_abs_time(
                ctx,
                cfg,
                Some("en"),
                None,
                samples,
                0,
                &CancelToken::new(),
            );
            segs.iter().map(|s| s.text.as_str()).collect()
        };

        // 串行基线（贪心解码确定性：串行两次结果一致才有效）。
        let baseline = decode_text(&ctx, &cfg, &samples);
        assert!(!baseline.is_empty(), "基线解码不得为空");
        assert_eq!(
            decode_text(&ctx, &cfg, &samples),
            baseline,
            "串行重复解码必须确定（否则本测试无效）"
        );

        // 并发：两线程 × 3 轮，同一 ctx、各自 state（模拟 Both 双流）。
        for round in 0..3 {
            let results: Vec<String> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..2)
                    .map(|_| {
                        let ctx = &ctx;
                        let cfg = &cfg;
                        let samples = &samples;
                        scope.spawn(move || decode_text(ctx, cfg, samples))
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for (i, text) in results.iter().enumerate() {
                assert_eq!(
                    text, &baseline,
                    "第 {round} 轮线程 {i} 并发解码结果偏离串行基线（并发污染）"
                );
            }
        }
    }

    /// 预取消 token → `run_blocking_transcribe` 应返回 `Cancelled`。
    #[test]
    #[ignore = "需真实模型（真机）"]
    fn precancelled_run_returns_cancelled() {
        let model = std::env::var("LUNA_WHISPER_MODEL").expect("set LUNA_WHISPER_MODEL");
        let provider = WhisperProvider::load(WhisperConfig::new(model)).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let samples = vec![0.1_f32; WHISPER_SAMPLE_RATE as usize * 3];
        let job = pcm_job(PcmBuffer {
            sample_rate: 16_000,
            channels: 1,
            samples: samples.clone(),
        });
        let ev = run_blocking_transcribe(
            Arc::clone(&provider.ctx),
            provider.cfg.clone(),
            job,
            samples,
            cancel,
        );
        assert!(
            matches!(ev, JobEvent::Cancelled),
            "预取消应产 Cancelled，得到 {ev:?}"
        );
    }

    /// 流式路径真模型端到端：两窗 → committed 单调 + Finished（真机）。
    #[test]
    #[ignore = "需真实模型（真机）"]
    fn real_model_streaming_committed_monotonic() {
        use futures::StreamExt as _;
        let model = std::env::var("LUNA_WHISPER_MODEL").expect("set LUNA_WHISPER_MODEL");
        let wav = std::env::var("LUNA_WHISPER_WAV").expect("set LUNA_WHISPER_WAV");
        let provider = WhisperProvider::load(WhisperConfig::new(model)).unwrap();

        let stream_id = StreamId::new();
        let reg = provider.register_stream(stream_id);
        let job = stream_job(stream_id);

        // 读取 wav，切成两窗（~3s 每窗）
        let mut reader = hound::WavReader::open(&wav).unwrap();
        let all: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        let w1 = all[..48_000.min(all.len())].to_vec();
        let w2 = all[48_000.min(all.len())..96_000.min(all.len())].to_vec();

        reg.window_tx
            .send(Some(WindowInput {
                speech_len: w1.len(),
                samples: w1,
                abs_start: 0,
            }))
            .unwrap();
        reg.window_tx
            .send(Some(WindowInput {
                speech_len: w2.len(),
                samples: w2,
                abs_start: 48_000,
            }))
            .unwrap();
        reg.window_tx.send(None).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut stream = provider.execute(job, CancelToken::new()).await;
            let mut committed: Vec<Segment> = Vec::new();
            let mut got_finished = false;
            while let Some(ev) = stream.next().await {
                match ev {
                    JobEvent::Partial(PartialPayload::Asr {
                        committed_delta, ..
                    }) => {
                        for s in committed_delta {
                            if let Some(last) = committed.last() {
                                assert!(s.start >= last.start, "committed 单调违反");
                            }
                            committed.push(s);
                        }
                    }
                    JobEvent::Finished(_) => {
                        got_finished = true;
                        break;
                    }
                    _ => {}
                }
            }
            assert!(got_finished, "流结束应产 Finished");
        });
    }
}
