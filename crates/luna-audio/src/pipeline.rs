//! 消费线程管线：原始帧 → 归一化 → VAD → 端点分段/流式分段 + 波形（TDD-02 §2 ②消费线程）。
//!
//! 把 [`AudioNormalizer`] + [`Vad`] + [`Endpointer`]/[`StreamSegmenter`] + [`WaveformMeter`]
//! 串成一条**可脱离 cpal 在 headless CI 单测**的纯处理管线——cpal 采集（[`crate::capture`]）
//! 只负责把设备数据驱动到这里（依赖反转：真机 IO 在外、确定性逻辑在内，§8）。
//!
//! 模式选择（`PipelineMode`）：`VoiceInput` 走 `Endpointer`（端点整段），
//! `RealtimeStream` 走 `StreamSegmenter`（滑动窗 + overlap），`Recording` 仅落盘无分段。
//! 两者并存，按 mode 选路——**不合并为同一组件**（TDD-02 §3.3 拆开的理由）。

use crate::aec::AecRole;
use crate::capture::CaptureTuning;
use crate::endpointer::{EndpointConfig, EndpointEvent, Endpointer, FinalizeReason};
use crate::level::{encode_levels, WaveformMeter};
use crate::normalizer::{AudioNormalizer, NormalizeError};
use crate::segmenter::{SegmenterConfig, StreamSegmenter};
use crate::types::{CaptureDiagnostic, RawFrame, SpeechEvent};
use crate::vad::{dbfs, EnergyVad, Vad, VadDecision, VadLabel, VadSensitivity, VAD_FRAME_SAMPLES};
use crate::TARGET_SAMPLE_RATE;

/// 连续静音超此阈值产 `SuspectedSilence` 诊断（§3.1-G，默认 3s 可配）。
const SILENCE_DIAG_SAMPLES: usize = 3 * TARGET_SAMPLE_RATE as usize;

/// 连续判**语音**超此阈值产一次 [`CaptureDiagnostic::VadSaturated`]（噪声底跟踪疑似锁死）。
///
/// 取 10s 的两条推导：
/// ① 必须**远长于**任何真实的连续说话——`Endpointer` 的强制收尾上限是 30s、
///    `StreamSegmenter` 的窗长上限是 ~10s，人不间断发声（无音节间隙）的经验上限 < 2s；
///    10s 连续判语音意味着这 500 帧里**一帧静音都没有**，正常语音做不到这件事。
/// ② 必须**短于**用户的忍耐——现场是"说完了字还在冒/一直不出字"，等一分钟才留下一行
///    日志等于事后验尸。
const VAD_SATURATION_SAMPLES: usize = 10 * TARGET_SAMPLE_RATE as usize;

/// 帧 RMS 分布（[`CaptureDiagnostic::InputLevels`]）的统计窗，单位 = VAD 帧数。
///
/// 取 250 帧 = 250 × 20ms = **5s**，三条推导同时成立：
/// ① 分位数要有意义，样本数得够——250 个样本下 p10/p90 各压着 25 个帧，个别瞬态
///    （咳嗽、敲键）拉不动分位数；
/// ② 窗要短于「用户注意到不对劲」的时间——现场诊断要能在说话过程中看到分布在变，
///    一分钟才出一行等于事后验尸；
/// ③ 与本仓其它节流出口同阶（实时窗 5s、上行统计 5s），几条诊断在 stdout 上按同一
///    节奏交替出现，肉眼可对齐时间。
///
/// 它同时是缓冲容量：缓冲一次性分配、满了就地排序后 `clear()` 复用，**全程零分配**。
const LEVEL_WINDOW_FRAMES: usize = 250;

/// 消费管线产出的事件。
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    /// 起音确认（HUD 切"识别中…"，语音输入路径）。
    SpeechStarted,
    /// 一段完整 utterance（mono 16k），供构造 `transcribe` Job（语音输入路径）。
    Utterance {
        /// 单声道 16k 样本。
        samples: Vec<f32>,
        /// 收尾原因。
        reason: FinalizeReason,
    },
    /// 流式分段窗（实时转录路径）：含 abs_start + speech_len 供 LocalAgreement-2 + RTF。
    Window(SpeechEvent),
    /// 二进制波形电平字节（每帧 `[peak, rms]`），供 `ipc::Channel`（ADR-011）。
    Level(Vec<u8>),
    /// 采集自检/诊断。
    Diagnostic(CaptureDiagnostic),
}

/// 管线工作模式（决定下游分段策略）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PipelineMode {
    /// 语音输入（端点整段，MVP 默认）。
    #[default]
    VoiceInput,
    /// 快捷录音（只落盘，不分段转写）。
    Recording,
    /// 实时转录（流式滑动窗，P1）。
    RealtimeStream,
}

/// 消费管线配置。
///
/// **不派生 `Copy`**：它已经携带 VAD 的行为参数，而 VAD 是有状态组件——一个能被隐式
/// 按位复制的"配置"很容易在下一次扩展里变成"被复制的状态"，那类错误不会报错、只会
/// 让两条支路各跟各的噪声底。显式 `clone()` 一次的代价可以忽略（每会话一次）。
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    /// 设备输入采样率（归一化器据此建重采样器）。
    pub input_sample_rate: u32,
    /// 管线模式（VoiceInput/Recording/RealtimeStream）。
    pub mode: PipelineMode,
    /// 端点参数（VoiceInput 模式用）。
    pub endpoint: EndpointConfig,
    /// 流式分段参数（RealtimeStream 模式用）。
    pub segmenter: SegmenterConfig,
    /// VAD 灵敏度档位（= 门限相对本轨噪声底的余量；**不是绝对门限**，见 [`VadSensitivity`]）。
    pub vad_sensitivity: VadSensitivity,
    /// 是否产出波形电平事件。
    pub emit_level: bool,
    /// 波形帧率。
    pub waveform_fps: u32,
}

impl PipelineConfig {
    /// **本仓构造 `PipelineConfig` 的唯一入口**：按模式 + 设备采样率 + 采集调参装配。
    ///
    /// # 为什么必须是单点
    ///
    /// 三种 [`PipelineMode`] 的差别只有 `mode` 一个字段，其余全同；而"其余"里有一半
    /// （目前是 `waveform_fps` 与 `vad_sensitivity`）来自用户设置，经 [`CaptureTuning`]
    /// 注入。此前 [`crate::capture`] 里是**三个分支各抄一遍**同样的覆盖列表，于是
    /// 「往 `CaptureTuning` 加一个字段」这件事的正确做法是在三处分别补一行——
    /// 漏掉任意一处的后果是"某一条采集路径（比如只有语音输入、或只有实时字幕）上，
    /// 这个设置不生效"，而所有单测照样全绿，因为没有任何用例同时跑三条支路。
    /// 收成单点之后，加字段只需要改这里一处，三条支路自动一致。
    ///
    /// **`emit_level` 刻意不进 `CaptureTuning`**：它不是用户旋钮而是**会话级**开关
    /// （Both 双轨要给副轨单独开电平，见 `RecordingStartOptions`），由调用方在本函数
    /// 结果上覆盖。
    #[must_use]
    pub fn for_mode(mode: PipelineMode, input_sample_rate: u32, tuning: &CaptureTuning) -> Self {
        Self {
            input_sample_rate,
            mode,
            endpoint: EndpointConfig::new(TARGET_SAMPLE_RATE),
            segmenter: SegmenterConfig::new(TARGET_SAMPLE_RATE),
            vad_sensitivity: tuning.vad_sensitivity,
            emit_level: true,
            waveform_fps: tuning.waveform_fps,
        }
    }
}

/// 消费管线（每采集会话一个；有状态，含归一化/VAD/端点或流式分段/波形）。
pub struct ConsumerPipeline {
    normalizer: AudioNormalizer,
    vad: Box<dyn Vad>,
    /// 语音输入路径端点检测器（VoiceInput 模式）。
    endpointer: Endpointer,
    /// 流式分段器（RealtimeStream 模式）。
    segmenter: StreamSegmenter,
    /// 当前管线模式。
    mode: PipelineMode,
    /// 落盘编码器（`Some` = 边录边落盘；归一化 16k mono 帧的第三路分叉，TDD-02 §2/§3.5）。
    /// 与转写/波形分叉**同源于归一化帧**，保证落盘音频与转写时间轴逐样本对齐（回放 seek 命门）。
    writer: Option<crate::writer::AudioFileWriter>,
    /// AEC 接线（TDD-02 §3.1-I 层③，Both 双轨专用）：归一化之后、writer/波形/VAD 之前
    /// 的唯一插入点——Capture 角色消回声（处理后信号三路同源全部受益）、Render 角色把
    /// 归一化输出分叉为参考信号。`None` = 非 Both/AEC 不可用（直通）。
    aec: Option<AecRole>,
    meter: WaveformMeter,
    emit_level: bool,
    /// 软静音（PRD-04-003）：由采集层每批显式同步（[`Self::set_muted`]），**不是**从
    /// 样本形态反推出来的。语义见 [`Self::set_muted`]。
    muted: bool,
    // VAD 定长重切缓冲（消费线程变长帧 → 320 定长，§3.2 同位置）。
    vad_buf: Vec<f32>,
    // 连续静音样本计数（用于 SuspectedSilence 诊断）。
    silence_run: usize,
    silence_diag_emitted: bool,
    /// 本段静音里的峰值绝对幅度（线性，0..1）；见 `SuspectedSilence::peak_dbfs`。
    silence_peak: f32,
    /// 连续**语音**样本计数（用于 `VadSaturated` 诊断）；首个静音帧复位。
    speech_run: usize,
    vad_saturated_emitted: bool,
    /// 帧 RMS 统计窗（**dBFS**，未排序）。容量恒为 [`LEVEL_WINDOW_FRAMES`]，满窗就地
    /// 排序 → 取分位 → `clear()`，**不重新分配**（见 [`Self::track_levels`]）。
    level_window: Vec<f32>,
}

impl ConsumerPipeline {
    /// 用配置构造（注入默认能量 VAD）。
    ///
    /// # Errors
    /// 归一化器（rubato）构造失败时返回错误。
    pub fn new(cfg: PipelineConfig) -> Result<Self, NormalizeError> {
        let vad = Box::new(EnergyVad::new(cfg.vad_sensitivity));
        Self::with_vad(cfg, vad)
    }

    /// 用配置 + 自定义 VAD 构造（Silero 后置替换入口）。
    ///
    /// # Errors
    /// 归一化器（rubato）构造失败时返回错误。
    pub fn with_vad(cfg: PipelineConfig, mut vad: Box<dyn Vad>) -> Result<Self, NormalizeError> {
        vad.reset(); // 会话开始清状态（§3.2 硬约束 2）。
        Ok(Self {
            normalizer: AudioNormalizer::new(cfg.input_sample_rate)?,
            vad,
            endpointer: Endpointer::new(cfg.endpoint),
            segmenter: StreamSegmenter::new(cfg.segmenter),
            mode: cfg.mode,
            writer: None,
            aec: None,
            meter: WaveformMeter::new(TARGET_SAMPLE_RATE, cfg.waveform_fps),
            emit_level: cfg.emit_level,
            muted: false,
            vad_buf: Vec::new(),
            silence_run: 0,
            silence_diag_emitted: false,
            silence_peak: 0.0,
            speech_run: 0,
            vad_saturated_emitted: false,
            level_window: Vec::with_capacity(LEVEL_WINDOW_FRAMES),
        })
    }

    /// 同步软静音状态（PRD-04-003）。由采集层在**每批喂帧之前**调用。
    ///
    /// # 为什么 mute 必须显式传进来，而不是让 VAD 自己看出来
    ///
    /// 采集层的静音是把样本置**精确零**再往下喂，于是"管线能不能认出静音"这件事就变成了
    /// "精确零帧检测"。它挡不住：归一化的去直流高通（`normalizer::DcBlocker`，极点
    /// `R ≈ 0.9686`）在输入突然变零之后还会吐一小段**非零**衰减拖尾。那段残渣是"正但极小"
    /// 的合法读数，会穿过 VAD 的精确零闸门进入噪声底窗，把底一路拖到 `F_FLOOR_MIN`；
    /// 解除静音后门限塌在 −65，整片底噪被判成语音（PRD-02-007 的幻觉护栏当场失效）。
    ///
    /// mute 是**采集层已知的事实**，把它传下来只要一个 bool；靠信号特征去猜，就是在解一个
    /// 本来不必存在的反问题。VAD 里那道精确零闸门**保留**，但它管的是另一类东西：
    /// 未授权麦克风 / 虚拟设备无信号 / VPIO 全抑——来源未知、只能启发式兜底的情形。
    ///
    /// # muted 期间的行为（PRD-04-003 语义不变）
    ///
    /// - **时钟推进、落盘节奏、VAD 帧切分照旧**——静音写的是零样本，不是"不写"；
    /// - **不调 `Vad::score`**，直接以 [`VadLabel::Silence`] 推进端点器/分段器：VAD 内部
    ///   状态（噪声底、子窗计数）整体冻结，解除静音即从冻结点续上；
    /// - **不产 `SuspectedSilence` / `InputLevels`**（见 [`Self::run_vad_endpoint`] 的裁决注释）。
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// 最近一帧的 VAD 判定依据（**测试专用**：噪声底/门限没有产品侧读者，现场看的是
    /// 诊断事件里那一份，见 [`crate::types::CaptureDiagnostic::SuspectedSilence`]）。
    /// 用例靠它断言"底冻住了没有"——这件事从事件流上看不见，因为 muted 期间恰好不产事件。
    #[cfg(test)]
    fn vad_decision(&self) -> VadDecision {
        self.vad.last_decision()
    }

    /// 注入落盘编码器（边录边落盘）。归一化 16k mono 帧将被喂给它（TDD-02 §2 第三路分叉）。
    pub fn attach_writer(&mut self, writer: crate::writer::AudioFileWriter) {
        self.writer = Some(writer);
    }

    /// 注入 AEC 接线角色（TDD-02 §3.1-I 层③，Both 编排层构造；随会话 part 重建）。
    pub fn attach_aec(&mut self, role: AecRole) {
        self.aec = Some(role);
    }

    /// 取出落盘编码器（会话结束时由采集层 `stop()` → 返回分片区间表）。
    pub fn take_writer(&mut self) -> Option<crate::writer::AudioFileWriter> {
        self.writer.take()
    }

    /// 把一批归一化 16k mono 帧喂给落盘编码器（blocking：绝不丢转写音频，TDD-02 §5.3；
    /// 背压经此传导回 rtrb，由磁盘水位监测兜底优雅停录，而非静默丢样本）。
    fn feed_writer(&self, mono: &[f32]) {
        if let Some(w) = &self.writer {
            if !mono.is_empty() {
                let _ = w.send_samples_blocking(mono.to_vec());
            }
        }
    }

    /// 会话累计 16k mono 输出样本数（时钟漂移闭环的控制观测量，TDD-02 §3.1-I）。
    #[must_use]
    pub fn total_output_samples(&self) -> u64 {
        self.normalizer.total_output_samples()
    }

    /// rate-aware 微调重采样比例（相对基准 ratio；漂移校正用，§3.1-C）。返回是否生效
    /// （passthrough 无重采样器时为 `false`）。
    pub fn set_resample_ratio_relative(&mut self, rel: f64) -> bool {
        self.normalizer.set_resample_ratio_relative(rel)
    }

    /// 暂停实时分段器的绝对时钟（RealtimeStream 模式，暂停录音时调用）。
    pub fn pause_segmenter(&mut self) {
        self.segmenter.pause();
    }

    /// 恢复实时分段器的绝对时钟（RealtimeStream 模式）。
    pub fn resume_segmenter(&mut self) {
        self.segmenter.resume();
    }

    /// 处理一帧原始采集数据，产出 0..N 个事件。
    ///
    /// # Errors
    /// 归一化失败时返回错误（调用方应记一次诊断并继续，不中断会话）。
    pub fn process(&mut self, raw: &RawFrame) -> Result<Vec<CaptureEvent>, NormalizeError> {
        let mono = self.normalizer.process(raw)?;
        // AEC 插入点（层③，§3.1-I）：Capture 消回声后信号供三路同源；Render 分叉参考后原样下行。
        let samples = match &mut self.aec {
            Some(AecRole::Capture(canceller)) => canceller.process(&mono.samples),
            Some(AecRole::Render(feeder)) => {
                feeder.push(&mono.samples);
                mono.samples
            }
            None => mono.samples,
        };
        let mut events = Vec::new();
        self.feed_writer(&samples);
        self.emit_waveform(&samples, &mut events);
        self.run_vad_endpoint(&samples, &mut events);
        Ok(events)
    }

    /// 会话结束（手动停/松开）：冲出重采样残余 + 端点 flush + 波形 flush。
    ///
    /// # Errors
    /// 归一化器 flush 失败时返回错误。
    pub fn finish(&mut self) -> Result<Vec<CaptureEvent>, NormalizeError> {
        let mut events = Vec::new();
        let mut tail = self.normalizer.flush()?;
        // AEC 收尾（与 process 同一插入点）：重采样残余过 AEC，再冲出 AEC 自身块余量。
        match &mut self.aec {
            Some(AecRole::Capture(canceller)) => {
                let mut out = canceller.process(&tail);
                out.extend(canceller.flush());
                tail = out;
            }
            Some(AecRole::Render(feeder)) => feeder.push(&tail),
            None => {}
        }
        if !tail.is_empty() {
            self.feed_writer(&tail);
            self.emit_waveform(&tail, &mut events);
            self.run_vad_endpoint(&tail, &mut events);
        }
        // VAD 缓冲残余补足一帧（不足 320 的尾巴）。
        if !self.vad_buf.is_empty() {
            let frame = std::mem::take(&mut self.vad_buf);
            // 与 `run_vad_endpoint` 同一条 mute 语义：静音中收尾不评分，尾帧照常推进下游。
            let label = if self.muted {
                VadLabel::Silence
            } else {
                self.vad.score(&frame)
            };
            match self.mode {
                PipelineMode::VoiceInput => {
                    for ev in self.endpointer.push(&frame, label) {
                        push_endpoint_event(ev, &mut events);
                    }
                }
                PipelineMode::RealtimeStream => {
                    for win in self.segmenter.push(&frame, label) {
                        events.push(CaptureEvent::Window(speech_window_event(win)));
                    }
                }
                PipelineMode::Recording => {}
            }
        }
        // 手动收尾（按 mode 选路）。
        match self.mode {
            PipelineMode::VoiceInput => {
                for ev in self.endpointer.flush() {
                    push_endpoint_event(ev, &mut events);
                }
            }
            PipelineMode::RealtimeStream => {
                if let Some(win) = self.segmenter.flush() {
                    events.push(CaptureEvent::Window(speech_window_event(win)));
                }
            }
            PipelineMode::Recording => {}
        }
        if self.emit_level {
            if let Some(frame) = self.meter.flush() {
                events.push(CaptureEvent::Level(encode_levels(&[frame])));
            }
        }
        Ok(events)
    }

    fn emit_waveform(&mut self, samples: &[f32], events: &mut Vec<CaptureEvent>) {
        if !self.emit_level {
            return;
        }
        let frames = self.meter.push(samples);
        if !frames.is_empty() {
            events.push(CaptureEvent::Level(encode_levels(&frames)));
        }
    }

    /// 逐帧跑 VAD → 端点/分段（muted 期间跳过评分与统计，见方法内的裁决注释）。
    fn run_vad_endpoint(&mut self, samples: &[f32], events: &mut Vec<CaptureEvent>) {
        self.vad_buf.extend_from_slice(samples);
        while self.vad_buf.len() >= VAD_FRAME_SAMPLES {
            let frame: Vec<f32> = self.vad_buf.drain(..VAD_FRAME_SAMPLES).collect();
            let label = if self.muted {
                // **裁决：muted 期间不产 `SuspectedSilence` / `InputLevels`。**
                //
                // 两条诊断的存在理由都是"回答一个我们不知道答案的问题"，而 mute 把答案
                // 直接给了：
                // - `SuspectedSilence` 问的是「在录，为什么一直没有语音」——静音期间这不
                //   *疑似*，它是用户按下去的事实，HUD 上正显示着。照产会让这条诊断在最
                //   常见的正常操作里刷屏，把它训练成"看见了也不用管"的噪声；更糟的是它
                //   的载荷带 `decision`（底/门限），而 muted 期间 VAD 冻结，那是一份**过期
                //   快照**——现场照着它去判断门限画在哪，读到的是几分钟前的数。
                // - `InputLevels` 是决定"门限该画在哪"的分布本身。静音样本是精确零（外加
                //   去直流拖尾的残渣），入窗只会把 p10/p50/min 整体拉到封底 −120，污染的
                //   恰好是用来校准判据的那份数据——与"精确零不入噪声底窗"同一条道理。
                //
                // 两条游程计数（`silence_run`/`speech_run`）随之**冻结而非清零**：它们计的
                // 是"观测到的音频"，静音期间没有观测，解除后从冻结点续上是诚实的读数。
                VadLabel::Silence
            } else {
                let label = self.vad.score(&frame);
                // 帧 RMS **只算这一次**：VAD 在 `score()` 里算完经 `last_decision()` 交出来，
                // 电平统计与诊断全部复用它。此前这里和 `track_levels` 各算一份，是每帧多跑
                // 320 次乘加 + 一次开方的纯重复——而两份一旦算法微调就会不一致，
                // 现场读到的「门限」与「分布」将来自两个不同的数。
                let decision = self.vad.last_decision();
                self.track_runs(label, &frame, decision, events);
                self.track_levels(decision.rms_dbfs(), events);
                label
            };
            match self.mode {
                PipelineMode::VoiceInput => {
                    for ev in self.endpointer.push(&frame, label) {
                        push_endpoint_event(ev, events);
                    }
                }
                PipelineMode::RealtimeStream => {
                    for win in self.segmenter.push(&frame, label) {
                        events.push(CaptureEvent::Window(speech_window_event(win)));
                    }
                }
                PipelineMode::Recording => {
                    // 纯录音模式：不分段，仅波形（落盘由 AudioFileWriter 处理）。
                }
            }
        }
    }

    /// 跟踪**两条**连续游程，各产一条诊断：静音游程 → `SuspectedSilence`；
    /// 语音游程 → `VadSaturated`。两者互为复位（任一标签出现即清掉另一条的计数），
    /// 故同一时刻最多只有一条在计。
    ///
    /// 两条都要，因为门限画错有**两个方向**，而它们的现场症状完全不同：门限偏高 =
    /// 一直判静音（说了话没字），门限偏低/底跟踪锁死 = 一直判语音（没说话却出字、
    /// 分段永不收尾）。只报其中一条，另一半故障在日志里就是一片空白。
    fn track_runs(
        &mut self,
        label: VadLabel,
        frame: &[f32],
        decision: VadDecision,
        events: &mut Vec<CaptureEvent>,
    ) {
        let n = frame.len();
        match label {
            VadLabel::Speech => {
                self.silence_run = 0;
                self.silence_diag_emitted = false;
                self.silence_peak = 0.0;
                self.speech_run += n;
                if self.speech_run >= VAD_SATURATION_SAMPLES && !self.vad_saturated_emitted {
                    self.vad_saturated_emitted = true;
                    let speech_run_ms =
                        (self.speech_run as u64 * 1000) / u64::from(TARGET_SAMPLE_RATE);
                    events.push(CaptureEvent::Diagnostic(CaptureDiagnostic::VadSaturated {
                        speech_run_ms,
                    }));
                }
            }
            VadLabel::Silence => {
                // 首个静音帧即复位（连同已上报标志）——锁死解除后下一次再锁死要能再报。
                self.speech_run = 0;
                self.vad_saturated_emitted = false;
                self.silence_run += n;
                // 峰值取这一段静音里的最大绝对幅度：它回答「到底有没有信号」，而
                // 「判不出语音」只回答「够不够得上语音」——两者缺一不可（见枚举注释）。
                let peak = frame.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
                self.silence_peak = self.silence_peak.max(peak);
                if self.silence_run >= SILENCE_DIAG_SAMPLES && !self.silence_diag_emitted {
                    self.silence_diag_emitted = true;
                    let elapsed_ms =
                        (self.silence_run as u64 * 1000) / u64::from(TARGET_SAMPLE_RATE);
                    events.push(CaptureEvent::Diagnostic(
                        CaptureDiagnostic::SuspectedSilence {
                            elapsed_ms,
                            peak_dbfs: dbfs(self.silence_peak),
                            decision,
                        },
                    ));
                }
            }
        }
    }

    /// 累计帧 RMS（dBFS，**由 VAD 算好后传入**），满窗产一次分位数分布
    /// （[`CaptureDiagnostic::InputLevels`]）。
    ///
    /// # 为什么这段代码在这里、且允许排序
    ///
    /// 它跑在**消费线程**（`process` 由采集桥接线程驱动，不是 cpal 实时回调）——ADR-009
    /// 的 wait-free 铁律约束的是回调，这里合法。但**仍按零分配写**：缓冲在构造时一次性
    /// 分配到满容量，满窗后 `clear()` 复用（`clear` 不释放容量），排序用
    /// `sort_unstable_by`（原地、无辅助分配）。250 个 f32 的排序是几微秒量级，每 5 秒
    /// 一次，对 20ms 帧节奏不构成任何压力。
    ///
    /// **无条件统计，不分 VAD 标签**：这条诊断要回答的正是「门限画在哪里才能把语音和
    /// 底噪分开」——只统计其中一侧，等于用待检验的判据去筛检验它的数据。
    ///
    /// 会话收尾时不足一窗的尾巴**刻意不出数**（`finish` 不补发）：几十帧的分位数没有
    /// 统计意义，却会在日志末尾多出一行口径不同、又长得一模一样的数据，误导读日志的人。
    fn track_levels(&mut self, rms_dbfs: f32, events: &mut Vec<CaptureEvent>) {
        self.level_window.push(rms_dbfs);
        if self.level_window.len() < LEVEL_WINDOW_FRAMES {
            return;
        }
        // 窗内**直接存 dBFS**（VAD 判定本来就在 dB 域，换算已经做过一次，见调用点的去重注释）。
        // 分位数在单调变换下不变，`dbfs` 在正数上严格单调，故先换算再排序与先排序再换算
        // 给出的分位点完全相同——口径不因此改变。
        self.level_window
            .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let win = &self.level_window;
        events.push(CaptureEvent::Diagnostic(CaptureDiagnostic::InputLevels {
            window_ms: (LEVEL_WINDOW_FRAMES * VAD_FRAME_SAMPLES) as u64 * 1000
                / u64::from(TARGET_SAMPLE_RATE),
            p10_dbfs: percentile(win, 10),
            p50_dbfs: percentile(win, 50),
            p90_dbfs: percentile(win, 90),
            min_dbfs: win[0],
        }));
        self.level_window.clear();
    }
}

/// 取已升序排好的样本的第 `pct` 分位（最近秩，无插值）。
///
/// 不插值是刻意的：分位数在这里只用来读数量级（噪声底 / 语音电平相差十几 dB），
/// 插值带来的零点几 dB 精度买不到任何判断力，却要多一处可写错的算术。
/// 调用方保证 `sorted` 非空且已升序。
fn percentile(sorted: &[f32], pct: usize) -> f32 {
    sorted[(sorted.len() - 1) * pct / 100]
}

/// [`crate::segmenter::SpeechWindow`] → [`SpeechEvent::Window`]（单一映射点，去样板）。
fn speech_window_event(win: crate::segmenter::SpeechWindow) -> SpeechEvent {
    SpeechEvent::Window {
        abs_start: win.abs_start,
        speech_len: win.speech_len,
        speech_delta: win.speech_delta,
        is_snapshot: win.is_snapshot,
        samples: win.samples,
    }
}

fn push_endpoint_event(ev: EndpointEvent, events: &mut Vec<CaptureEvent>) {
    match ev {
        EndpointEvent::SpeechStarted => events.push(CaptureEvent::SpeechStarted),
        EndpointEvent::Utterance { samples, reason } => {
            events.push(CaptureEvent::Utterance { samples, reason });
        }
        // Discarded 是内部诊断，不对外产事件（过短噪声静默丢弃，PRD-02-005 边界③）。
        EndpointEvent::Discarded { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::{noise, tone, Rng};
    use crate::types::RawSamples;

    /// 100ms @16k——本模块所有夹具的投喂粒度。
    const CHUNK: usize = 1600;

    fn raw_16k(samples: Vec<f32>) -> RawFrame {
        RawFrame {
            sample_rate: 16_000,
            channels: 1,
            samples: RawSamples::F32(samples),
        }
    }

    /// 按模式建管线（调参取默认；灵敏度默认 = Normal，余量 10dB）。
    fn pipeline(mode: PipelineMode, input_sample_rate: u32) -> ConsumerPipeline {
        ConsumerPipeline::new(PipelineConfig::for_mode(
            mode,
            input_sample_rate,
            &CaptureTuning::default(),
        ))
        .unwrap()
    }

    /// 当前噪声底（dBFS）。
    fn floor_of(p: &ConsumerPipeline) -> f32 {
        let VadDecision::Energy { floor_dbfs, .. } = p.vad_decision() else {
            panic!("本组用例只跑能量 VAD");
        };
        floor_dbfs
    }

    /// **逐帧**喂（一次 320 样本 = 恰好一个 VAD 帧；16k 输入直通，不重采样也不攒帧），
    /// 返回每帧的 VAD 标签。
    ///
    /// 标签由判定依据复算（`rms_dbfs >= threshold_dbfs`）——与 `EnergyVad::score` 是**同一个
    /// 比较、同一组数**，不会与真实标签分叉。之所以要复算而不是从事件流里读：逐帧标签
    /// 本来就不出事件（出去的只有分段与诊断），而本组用例要钉的恰好是逐帧行为。
    ///
    /// ⚠ 只能用在**非 muted** 区段：muted 期间管线刻意不评分，判定依据是冻结前的旧快照。
    fn run_frames(p: &mut ConsumerPipeline, frames: &[Vec<f32>]) -> Vec<VadLabel> {
        frames
            .iter()
            .map(|f| {
                p.process(&raw_16k(f.clone())).unwrap();
                let VadDecision::Energy {
                    rms_dbfs,
                    threshold_dbfs,
                    ..
                } = p.vad_decision()
                else {
                    panic!("本组用例只跑能量 VAD");
                };
                if rms_dbfs >= threshold_dbfs {
                    VadLabel::Speech
                } else {
                    VadLabel::Silence
                }
            })
            .collect()
    }

    /// 现场那段 1s 信号：0.5s −60dBFS 底噪 + 0.5s −35dBFS 语音（逐帧切好，可原样复用）。
    fn noise_then_speech(seed: u64) -> Vec<Vec<f32>> {
        let mut rng = Rng::new(seed);
        let mut frames: Vec<Vec<f32>> = (0..25)
            .map(|_| noise(&mut rng, -60.0, VAD_FRAME_SAMPLES))
            .collect();
        frames.extend((0..25).map(|_| tone(-35.0, VAD_FRAME_SAMPLES, 0)));
        frames
    }

    /// 去直流高通在输入置零之后吐出的衰减拖尾：`0.9686^n`（极点 R = 1 − 2π·80/16000）。
    /// `frames` 帧、逐样本连续衰减；**全程非零**，故穿得过 VAD 的"精确零不入窗"闸门。
    fn dc_tail(frames: usize) -> Vec<Vec<f32>> {
        const R: f32 = 0.9686;
        const START_AMP: f32 = 0.2;
        (0..frames)
            .map(|f| {
                (0..VAD_FRAME_SAMPLES)
                    .map(|i| START_AMP * R.powi((f * VAD_FRAME_SAMPLES + i) as i32))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn end_to_end_speech_yields_utterance_and_levels() {
        // 夹具口径 = 真实电平：**−60dBFS 底噪垫 + −35dBFS 语音**（25dB SNR，比实测最差的
        // VPIO 路径还紧一点）。
        //
        // 原夹具是「全零静音 + 0.5 幅度正弦（≈ −9dBFS）」，那是 30dB 余量下的平凡情形——
        // 门限画在 −44 还是 −50 它都过，于是它对"门限对不对"这件事**一个比特的信息都不给**，
        // 而现场坏掉的恰好就是门限。底噪垫也不能用全零：全零走"无信息"分支（数字静默不入
        // 噪声底窗），跟踪器根本不被触发。
        let mut rng = Rng::new(0x0E2E);
        let mut p = pipeline(PipelineMode::VoiceInput, 16_000);
        let mut all = Vec::new();
        // 0.5s 底噪（底落到 −60，门限 −50）→ 1s 语音 −35 → 1s 底噪（尾静音超 hangover 收尾）。
        for _ in 0..5 {
            all.extend(p.process(&raw_16k(noise(&mut rng, -60.0, CHUNK))).unwrap());
        }
        for k in 0..10 {
            all.extend(p.process(&raw_16k(tone(-35.0, CHUNK, k * CHUNK))).unwrap());
        }
        for _ in 0..10 {
            all.extend(p.process(&raw_16k(noise(&mut rng, -60.0, CHUNK))).unwrap());
        }
        all.extend(p.finish().unwrap());

        let utts = all
            .iter()
            .filter(|e| matches!(e, CaptureEvent::Utterance { .. }))
            .count();
        assert_eq!(utts, 1, "应恰好一段 utterance");
        assert!(
            all.iter().any(|e| matches!(e, CaptureEvent::SpeechStarted)),
            "应有起音事件"
        );
        assert!(
            all.iter().any(|e| matches!(e, CaptureEvent::Level(_))),
            "应有波形电平事件"
        );
    }

    #[test]
    fn pure_silence_emits_no_utterance_but_emits_diagnostic() {
        // P0 幻觉抑制：只有底噪、没有人说话时不产 utterance；超 3s 产 SuspectedSilence 诊断。
        //
        // 用 **−60dBFS 高斯噪**而非全零：全零是"无信息"（数字静默不入噪声底窗，另有
        // `SilentInput` 看门狗管），测不到自适应跟踪器；而现场的"安静"从来都是有底噪的。
        let mut rng = Rng::new(0x51_1E);
        let mut p = pipeline(PipelineMode::VoiceInput, 16_000);
        let mut all = Vec::new();
        for _ in 0..40 {
            all.extend(p.process(&raw_16k(noise(&mut rng, -60.0, CHUNK))).unwrap());
            // 4s
        }
        all.extend(p.finish().unwrap());
        assert!(
            !all.iter()
                .any(|e| matches!(e, CaptureEvent::Utterance { .. })),
            "纯底噪绝不产 utterance"
        );
        // 诊断必须**带上当时的判定依据**（底与门限），否则现场只知道"判静音"、
        // 不知道"门限画在哪"——那正是 2026-08-18 卡住的地方。
        let diag = all
            .iter()
            .find_map(|e| match e {
                CaptureEvent::Diagnostic(CaptureDiagnostic::SuspectedSilence {
                    decision, ..
                }) => Some(*decision),
                _ => None,
            })
            .expect("应产静音诊断");
        let VadDecision::Energy {
            rms_dbfs,
            floor_dbfs,
            threshold_dbfs,
        } = diag
        else {
            panic!("本组用例只跑能量 VAD");
        };
        assert!(
            (-64.0..-56.0).contains(&floor_dbfs),
            "底应跟到 −60 量级：{floor_dbfs:.1}"
        );
        assert!(
            (threshold_dbfs - (floor_dbfs + 10.0)).abs() < 1e-3,
            "normal 档门限 = 底 + 10dB"
        );
        assert!(rms_dbfs < threshold_dbfs, "判静音的帧必然低于门限");
    }

    /// 底跟踪被锁死时必须**说出口**——持续 12s 大信号（−15dBFS）把底顶到上夹取 −30，
    /// 门限 −20 追不上去，于是一路判语音。
    ///
    /// 这是自适应门限唯一的现场可见失效信号：下游只会表现成"分段一直不收尾/凭空出字"，
    /// 从那些症状反推不到 VAD。本仓已经证明「离线全绿、现场不出字」会发生。
    #[test]
    fn vad_saturated_fires_when_locked() {
        let mut p = pipeline(PipelineMode::Recording, 16_000);
        let mut all = Vec::new();
        for k in 0..120 {
            all.extend(p.process(&raw_16k(tone(-15.0, CHUNK, k * CHUNK))).unwrap());
            // 12s
        }
        let fired: Vec<u64> = all
            .iter()
            .filter_map(|e| match e {
                CaptureEvent::Diagnostic(CaptureDiagnostic::VadSaturated { speech_run_ms }) => {
                    Some(*speech_run_ms)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            fired.len(),
            1,
            "12s 连续语音应恰好报一次（不刷屏）：{fired:?}"
        );
        assert!(fired[0] >= 10_000, "触发时游程应已达 10s：{}ms", fired[0]);
    }

    #[test]
    fn frame_rms_distribution_separates_noise_floor_from_speech() {
        // 这条诊断是后续定 VAD 门限的唯一依据，故锁死它的语义而不只是"有没有发出来"：
        // 一半底噪（−60dBFS 量级）一半语音，p10 必须落在底噪、p90 必须落在语音，
        // 二者之差 = 这条轨的可用 SNR。分位数一旦取反或取错，这个断言立刻红。
        let mut p = pipeline(PipelineMode::Recording, 16_000);
        let mut all = Vec::new();
        // 2.5s 底噪（−63dBFS）+ 2.5s 语音（−9dBFS）= 恰好一窗 5s。
        for k in 0..25 {
            all.extend(p.process(&raw_16k(tone(-63.0, CHUNK, k * CHUNK))).unwrap());
        }
        for k in 25..50 {
            all.extend(p.process(&raw_16k(tone(-9.0, CHUNK, k * CHUNK))).unwrap());
        }

        let levels = all
            .iter()
            .find_map(|e| match e {
                CaptureEvent::Diagnostic(CaptureDiagnostic::InputLevels {
                    window_ms,
                    p10_dbfs,
                    p90_dbfs,
                    min_dbfs,
                    ..
                }) => Some((*window_ms, *p10_dbfs, *p90_dbfs, *min_dbfs)),
                _ => None,
            })
            .expect("满 5s 应产出一次电平分布");
        let (window_ms, p10, p90, min) = levels;
        assert_eq!(window_ms, 5_000, "窗口口径必须是 5s");
        assert!((-65.0..-55.0).contains(&p10), "p10 应落在底噪上：{p10}");
        assert!(p90 > -15.0, "p90 应落在语音上：{p90}");
        assert!(p90 - p10 > 30.0, "可用 SNR 应显著为正：{}", p90 - p10);
        assert!(min.is_finite(), "min 必须封底，绝不为 -inf");
    }

    /// 软静音期间 VAD **整体冻结**：解除静音后，同一段信号必须给出与静音之前**逐帧相同**
    /// 的标签。
    ///
    /// 夹具形状：段 A（0.5s −60 底噪 + 0.5s −35 语音）→ muted 30s（其间照喂样本）→ 解除 →
    /// **段 A 的逐比特副本**。静音期喂的是 −95dBFS 的极弱信号——它是"非零但极小"，正是
    /// VAD 那道精确零闸门放行、而底会被一路拖到 `F_FLOOR_MIN` 的那一类输入。
    ///
    /// 两条断言各钉一头：
    /// - **底冻结**：静音前后的噪声底必须**逐比特相等**（不是"接近"——冻结就是一次
    ///   `observe` 都没发生）；
    /// - **标签复现**：副本的逐帧标签与原段完全相同。
    ///
    /// 末尾的**反向夹具**证明这两条不是空跑：同一段静音期输入不经 `set_muted` 喂进去，
    /// 底会塌到 `F_FLOOR_MIN`（−75）、门限跟着塌到 −65，于是副本里那 0.5s −60 底噪整片
    /// 被判成语音，标签序列与原段不同。
    #[test]
    fn mute_freezes_vad_state() {
        let segment_a = noise_then_speech(0x4D75_7465);
        // 静音期的"任意样本"：30s（300 × 100ms）−95dBFS 极弱噪声。
        let mut quiet_rng = Rng::new(0x0091_11E7);
        let muted_feed: Vec<Vec<f32>> = (0..300)
            .map(|_| noise(&mut quiet_rng, -95.0, CHUNK))
            .collect();

        let mut p = pipeline(PipelineMode::Recording, 16_000);
        let labels_before = run_frames(&mut p, &segment_a);
        let floor_before = floor_of(&p);

        p.set_muted(true);
        for chunk in &muted_feed {
            p.process(&raw_16k(chunk.clone())).unwrap();
        }
        let floor_after = floor_of(&p);
        p.set_muted(false);
        let labels_after = run_frames(&mut p, &segment_a);

        assert_eq!(
            floor_before.to_bits(),
            floor_after.to_bits(),
            "静音 30s 后噪声底从 {floor_before:.2} 变成了 {floor_after:.2}——VAD 没有真正冻结"
        );
        assert_eq!(
            labels_before,
            labels_after,
            "解除静音后同一段信号给出了不同的逐帧标签（底 {:.2} 门限 {:.2}）",
            floor_of(&p),
            floor_of(&p) + 10.0
        );

        // 反向夹具：不显式传 mute，同样的输入必然污染底 → 断言不是空跑。
        let mut naive = pipeline(PipelineMode::Recording, 16_000);
        let naive_before = run_frames(&mut naive, &segment_a);
        for chunk in &muted_feed {
            naive.process(&raw_16k(chunk.clone())).unwrap();
        }
        assert!(
            (floor_of(&naive) - crate::vad::F_FLOOR_MIN).abs() < 1e-3,
            "反向夹具没能把底压到下夹取（实得 {:.2}），本用例失去意义",
            floor_of(&naive)
        );
        let naive_after = run_frames(&mut naive, &segment_a);
        assert_ne!(
            naive_before, naive_after,
            "不冻结 VAD 时标签本应改变，反向夹具没红——说明冻结断言什么都没证明"
        );
    }

    /// 置零之后那段去直流拖尾**不得**污染噪声底——这正是"精确零"启发式挡不住、只能靠
    /// 显式 mute 解决的那一类残渣（`ConsumerPipeline::set_muted` 的文档理由）。
    ///
    /// 拖尾按 `0.9686^n` 逐样本衰减 10 帧：首帧还有 −27dBFS 量级，之后迅速掉到 `dbfs()`
    /// 的封底 −120——但**始终非零**，所以 VAD 里那道"精确零不入窗"的闸门放它过去。
    #[test]
    fn dc_tail_after_mute_does_not_pollute_floor() {
        let mut rng = Rng::new(0x00DC_7A11);
        let prime: Vec<Vec<f32>> = (0..150)
            .map(|_| noise(&mut rng, -60.0, VAD_FRAME_SAMPLES))
            .collect();
        let tail = dc_tail(10);

        let mut p = pipeline(PipelineMode::Recording, 16_000);
        run_frames(&mut p, &prime); // 3s 底噪，底跟到 −60
        let floor_before = floor_of(&p);
        assert!(
            (-63.0..-57.0).contains(&floor_before),
            "预热后底应在 −60 量级：{floor_before:.2}"
        );

        p.set_muted(true);
        for frame in &tail {
            p.process(&raw_16k(frame.clone())).unwrap();
        }
        assert_eq!(
            floor_of(&p).to_bits(),
            floor_before.to_bits(),
            "muted 期间的去直流拖尾改动了噪声底：{floor_before:.2} → {:.2}",
            floor_of(&p)
        );

        // 反向夹具：同一段拖尾不经 mute 喂进去，底被一路拖到下夹取（−75），
        // 门限塌到 −65 —— 恢复后整片 −60 底噪会被判成语音。
        let mut naive = pipeline(PipelineMode::Recording, 16_000);
        run_frames(&mut naive, &prime);
        run_frames(&mut naive, &tail);
        assert!(
            (floor_of(&naive) - crate::vad::F_FLOOR_MIN).abs() < 1e-3,
            "拖尾本应把底拖到 {:.1}（实得 {:.2}）——夹具不再复现那条污染路径",
            crate::vad::F_FLOOR_MIN,
            floor_of(&naive)
        );
    }

    #[test]
    fn recording_mode_persists_audio_via_attached_writer() {
        // C1 回归：归一化帧的第三路必须真正落盘（此前 SaveSpec 被忽略 → 录音无音频文件）。
        use crate::encoder::RecordingFormat;
        use crate::writer::{AudioFileWriter, WriterConfig};

        let path = std::env::temp_dir().join("luna_pipeline_writer_c1.wav");
        let _ = std::fs::remove_file(&path);

        let (diag_tx, _diag_rx) = std::sync::mpsc::channel();
        let wcfg = WriterConfig::new(path.clone(), RecordingFormat::Wav, 16_000);
        let writer = AudioFileWriter::spawn(wcfg, 64, diag_tx, None).unwrap();

        let mut p = pipeline(PipelineMode::Recording, 16_000);
        p.attach_writer(writer);

        // 喂 1s 16k mono（10 帧 × 1600），16k 输入无重采样。
        for _ in 0..10 {
            p.process(&raw_16k(vec![0.2; 1600])).unwrap();
        }
        p.finish().unwrap();

        let intervals = p.take_writer().expect("writer attached").stop();
        assert_eq!(intervals.len(), 1, "应有一个落盘分片");
        assert_eq!(intervals[0].abs_start, 0);

        let reader = hound::WavReader::open(&path).expect("落盘 WAV 应可读回");
        // 16k 输入直通，落盘样本数应等于喂入样本数（回放 seek 对齐的前提）。
        assert_eq!(reader.len(), 16_000, "落盘样本数应等于喂入样本数");
        assert_eq!(intervals[0].abs_end, 16_000);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resamples_48k_input() {
        // 48k 输入路径也跑通（归一化器重采样到 16k）。
        let mut p = pipeline(PipelineMode::VoiceInput, 48_000);
        let raw = RawFrame {
            sample_rate: 48_000,
            channels: 2,
            samples: RawSamples::F32(vec![0.4; 48_000 * 2]), // 0.5s 立体声语音
        };
        let ev = p.process(&raw).unwrap();
        // 不强求此帧即出 utterance，但应已产生波形电平（归一化成功）。
        assert!(ev.iter().any(|e| matches!(e, CaptureEvent::Level(_))));
    }
}
