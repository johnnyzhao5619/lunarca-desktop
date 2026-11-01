//! cpal 采集（实时回调 wait-free → rtrb SPSC → 消费线程）。事实源：TDD-02 §3.1、ADR-009。
//!
//! **实时回调铁律（ADR-009）**：`build_input_stream` 的 data callback **只**做无锁拷贝
//! （`producer.push`）+ 满时原子计数；**禁分配/加锁/`println!`/文件 IO/任何阻塞**。所有
//! 处理（归一化/VAD/端点/波形）在消费线程的 [`ConsumerPipeline`] 内做（§2 修正 1）。
//!
//! **Stream 生命周期（§3.1-D）**：cpal `Stream` 在部分平台**非 `Send`** 且 **drop 即停采**。
//! 故 Stream 在**专用拥有线程**内构建并持有到停止信号，严防变量离开作用域被 drop → 没声音。
//!
//! **本模块为运行时 IO（需真麦克风），headless CI 只验编译**；确定性逻辑在
//! [`crate::pipeline`]/[`crate::endpointer`] 等已 headless 覆盖。

use crate::aec::AecRole;
use crate::clock::{CaptureClock, ClockSnapshot, DriftMonitor};
use crate::encoder::RecordingFormat;
use crate::pacer::LoopbackPacer;
use crate::pipeline::{CaptureEvent, ConsumerPipeline, PipelineConfig, PipelineMode};
use crate::ring::{ring_capacity, DEFAULT_BUFFER_SECS};
use crate::types::{
    CaptureDiagnostic, CaptureMode, CaptureSource, CapturedSample, RawFrame, RawSamples,
};
use crate::writer::{AudioFileWriter, PartInterval, WriterConfig};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 采集时钟锚点入参：Both 双轨传入共享 `epoch` 构造的时钟（跨会话锚点可比，§3.1-I）；
/// `None` = 单轨，函数内以本会话 `Instant::now()` 自建（漂移估计仍生效，启动偏差无意义）。
type SharedClock = Option<Arc<CaptureClock>>;

/// 零回调看门狗阈值（§3.1-I，S11 实测口径）：外部平台源（macOS CATap）自启动/上次数据起
/// 超过此时长完全无回调 → 判定采集失效（TCC 未授权/被撤销/聚合设备失效），上报
/// [`CaptureDiagnostic::NoCaptureData`]。tap 正常时即使系统无声也持续回调，不会误报。
const NO_DATA_TIMEOUT_MS: u64 = 3_000;

/// 数字零输入看门狗阈值（秒）：连续纯零样本超此时长报 [`CaptureDiagnostic::SilentInput`]。
/// 与零回调看门狗同 3s 口径（都是「录了个寂寞」的检测线）。
const SILENT_INPUT_SECS: u64 = 3;

/// 时钟漂移评估周期（消费线程节流；非实时线程，`Instant::now` 无 ADR-009 约束）。
const DRIFT_EVAL_INTERVAL: Duration = Duration::from_secs(1);

/// 漂移指标上报周期（评估次数）：每 N 次评估上报一次 [`CaptureDiagnostic::ClockDrift`]
/// （施加校正时另外即报），避免每秒刷日志（§6 观测节流）。
const DRIFT_REPORT_EVERY: u32 = 10;

/// 采集错误。
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// 找不到输入设备（未授权/无设备）。
    #[error("no input device available")]
    NoDevice,
    /// 设备配置查询失败。
    #[error("device configuration error: {0}")]
    Config(String),
    /// 构建 stream 失败。
    #[error("failed to build the capture stream: {0}")]
    BuildStream(String),
    /// 归一化器构造失败。
    #[error(transparent)]
    Normalize(#[from] crate::normalizer::NormalizeError),
    /// 本平台不支持该采集源（macOS 系统音频经平台层 CATap + [`start_external_capture`]
    /// 注入，不走本 crate 的 cpal 路径；Linux 暂无系统回环，§3.1-I）。
    #[error("this capture source is unsupported on this platform")]
    UnsupportedSource,
}

/// 录音/实时流的落盘规格（TDD-02 §4）。
#[derive(Clone, Debug)]
pub struct SaveSpec {
    /// 目标录音格式。
    pub format: RecordingFormat,
    /// 落盘路径（由 `AudioFileWriter` 管理）。
    pub path: String,
    /// 落盘采样率（默认与 TARGET_SAMPLE_RATE 一致）。
    pub sample_rate: u32,
}

/// VAD 工厂（ADR-012）：每条采集轨各建一个实例（有状态实现不得跨轨共享，ADR-077 ④）。
/// `None` = 用本 crate 的能量 VAD（ADR-077）。
#[derive(Clone)]
pub struct VadFactory(pub std::sync::Arc<dyn Fn() -> Box<dyn crate::vad::Vad> + Send + Sync>);

impl std::fmt::Debug for VadFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VadFactory(..)")
    }
}

/// 采集会话配置（TDD-02 §4，对齐 `CaptureConfig` 接口规范）。
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    /// 采集源（MVP 仅 `InputDevice`）。
    pub source: CaptureSource,
    /// 采集模式（决定下游管线分支）。
    pub mode: crate::types::CaptureMode,
    /// 环形缓冲目标秒数（§3.1-A，默认 [`DEFAULT_BUFFER_SECS`]）。
    pub buffer_secs: f32,
    /// 是否产波形电平。
    pub emit_level: bool,
    /// 落盘规格（`None` = 不保存；`Some` = 边录边落盘，Task 2 使用）。
    pub save: Option<SaveSpec>,
    /// 采集调参（波形帧率 / 磁盘阈值 / 滚动切分上限 / VAD 灵敏度；由设置注入，
    /// 见 [`CaptureTuning`]）。
    pub tuning: CaptureTuning,
    /// VAD 实现（`None` = 能量 VAD）。
    pub vad: Option<VadFactory>,
}

/// 采集调参：可由用户设置覆盖的运行参数（`recording.waveform_fps` /
/// `recording.disk_warn_gib` / `recording.rolling_split_gib` / `recording.vad_sensitivity`，
/// PRD-04 §5 / PRD-09-040 / PRD-04 §3.2）。
///
/// 这些项在注册表里登记已久却**从未接线**（writer/pipeline 一直只用编译期默认），
/// 于是设置页里几个旋钮怎么调都没反应。默认值恒等于各自的编译期默认，故不改现有行为。
///
/// **投影点唯一**：设置 → 本结构的换算只在 `ipc::workspace::capture_tuning` 一处；
/// 本结构 → 管线配置的装配只在 [`crate::pipeline::PipelineConfig::for_mode`] 一处。
/// 加字段时这两处各改一行，三条采集入口（录音 / 实时字幕 / 语音输入）自动一致。
#[derive(Clone, Copy, Debug)]
pub struct CaptureTuning {
    /// 波形电平帧率（fps）。
    pub waveform_fps: u32,
    /// 磁盘软阈值：低于此发 `DiskLow` 预警（字节）。
    pub disk_warn_bytes: u64,
    /// 单文件滚动切分上限（字节）。
    pub rolling_split_max_bytes: u64,
    /// VAD 灵敏度档位（= 门限相对本轨噪声底的余量，**不是绝对门限**）。
    pub vad_sensitivity: crate::vad::VadSensitivity,
}

impl Default for CaptureTuning {
    fn default() -> Self {
        Self {
            waveform_fps: crate::level::DEFAULT_WAVEFORM_FPS,
            disk_warn_bytes: crate::writer::DEFAULT_DISK_SOFT_THRESHOLD_BYTES,
            rolling_split_max_bytes: crate::writer::DEFAULT_ROLLING_SPLIT_MAX_BYTES,
            vad_sensitivity: crate::vad::VadSensitivity::default(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            source: CaptureSource::default(),
            mode: crate::types::CaptureMode::VoiceInput,
            buffer_secs: DEFAULT_BUFFER_SECS,
            emit_level: true,
            save: None,
            tuning: CaptureTuning::default(),
            vad: None,
        }
    }
}

/// 采集会话句柄；持有 cpal `Stream`（绑定拥有线程）+ 消费线程。drop/stop 即停止采集。
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    /// 静音开关（PRD-04-003）：置位后消费线程把样本置静默，时钟/落盘节奏不变。
    muted: Arc<AtomicBool>,
    /// 采集时钟锚点（§3.1-I）：Both 混流收口读两轨 `first_nanos` 差算启动偏差补零量。
    clock: Arc<CaptureClock>,
    owner: Option<JoinHandle<()>>,
    /// 消费线程返回落盘分片区间表（无落盘时为空 `Vec`）。
    consumer: Option<JoinHandle<Vec<PartInterval>>>,
}

impl CaptureHandle {
    /// 主动停止采集：通知拥有线程 drop stream、消费线程 flush 收尾并退出，
    /// 返回落盘分片区间表（`SaveSpec` 为 `None` 时为空；语音输入等不落盘场景忽略即可）。
    pub fn stop(mut self) -> Vec<PartInterval> {
        self.signal_stop();
        if let Some(h) = self.owner.take() {
            let _ = h.join();
        }
        match self.consumer.take() {
            Some(h) => h.join().unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 切换静音（录音中可调；静音期写零样本、时间轴照走，PRD-04-003）。
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    /// 读取采集时钟锚点快照（§3.1-I）。Both 混流收口据两轨快照的 `first_nanos` 差
    /// 计算启动偏差补零量；首个真实采集批次到达前为 `None`。
    #[must_use]
    pub fn clock_snapshot(&self) -> Option<ClockSnapshot> {
        self.clock.snapshot()
    }

    fn signal_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        // 兜底：句柄被丢弃也停止采集（防麦克风悬挂，PRD-02 §5 边界②）。
        self.signal_stop();
        if let Some(h) = self.owner.take() {
            let _ = h.join();
        }
        if let Some(h) = self.consumer.take() {
            let _ = h.join(); // 兜底路径丢弃区间表（已 stop() 的会话不会再走这里）。
        }
    }
}

/// 启动一路麦克风采集会话，事件经 `sink` 投递（`SpeechStarted`/`Utterance`/`Level`/诊断）。
///
/// 调用方负责**麦克风硬互斥**（同一时刻只允许一路，TDD-05 §5.3）——本函数不做仲裁。
///
/// # Errors
/// 无设备/配置查询/构建流/归一化器构造失败时返回 [`CaptureError`]。
pub fn start_capture(
    cfg: &CaptureConfig,
    sink: Sender<CaptureEvent>,
    clock: SharedClock,
    aec: Option<AecRole>,
) -> Result<CaptureHandle, CaptureError> {
    let host = cpal::default_host();
    // 设备与流配置解析（§3.1-I）：
    // - `InputDevice`：具名设备按名匹配（`recording.input_device` 设置）；未找到 → 回退系统
    //   默认并发非致命诊断（PRD-04 §5「设备拔出/切换回退默认」，绝不因设置残留而拒绝开录）。
    // - `SystemLoopback`：Windows 上取默认**输出**设备并以输入流打开——cpal 对 render
    //   endpoint 自动加 `AUDCLNT_STREAMFLAGS_LOOPBACK`（WASAPI loopback）；macOS 系统音频
    //   走平台层 CATap + [`start_external_capture`]，不进本函数。
    let (device, supported) = match &cfg.source {
        CaptureSource::InputDevice(Some(requested)) => {
            let named = host
                .input_devices()
                .ok()
                .and_then(|mut devices| devices.find(|d| d.name().is_ok_and(|n| &n == requested)));
            let device = match named {
                Some(device) => device,
                None => {
                    let _ = sink.send(CaptureEvent::Diagnostic(
                        CaptureDiagnostic::InputDeviceFallback {
                            requested: requested.clone(),
                        },
                    ));
                    host.default_input_device().ok_or(CaptureError::NoDevice)?
                }
            };
            let supported = device
                .default_input_config()
                .map_err(|e| CaptureError::Config(e.to_string()))?;
            (device, supported)
        }
        CaptureSource::InputDevice(None) => {
            let device = host.default_input_device().ok_or(CaptureError::NoDevice)?;
            let supported = device
                .default_input_config()
                .map_err(|e| CaptureError::Config(e.to_string()))?;
            (device, supported)
        }
        CaptureSource::SystemLoopback => {
            #[cfg(target_os = "windows")]
            {
                let device = host.default_output_device().ok_or(CaptureError::NoDevice)?;
                let supported = device
                    .default_output_config()
                    .map_err(|e| CaptureError::Config(e.to_string()))?;
                (device, supported)
            }
            #[cfg(not(target_os = "windows"))]
            {
                return Err(CaptureError::UnsupportedSource);
            }
        }
    };
    let sample_format = supported.sample_format();
    let stream_config: cpal::StreamConfig = supported.config();
    let sample_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels;

    // Windows loopback 的静音期不产帧 → 消费线程按主机时钟补零对齐时间轴（§3.1-I）。
    let pacer = (cfg.source == CaptureSource::SystemLoopback)
        .then(|| LoopbackPacer::new(sample_rate, channels));

    let core = start_session_core(cfg, sink, sample_rate, channels, false, pacer, clock, aec)?;

    // 拥有线程：构建并持有 cpal stream，play 后驻留至 stop（§3.1-D）。
    let owner_handle = spawn_owner(
        device,
        stream_config,
        sample_format,
        core.producer,
        Arc::clone(&core.dropped),
        Arc::clone(&core.device_lost),
        Arc::clone(&core.stop),
        Arc::clone(&core.clock),
    );

    Ok(CaptureHandle {
        stop: core.stop,
        muted: core.muted,
        clock: core.clock,
        owner: Some(owner_handle),
        consumer: Some(core.consumer),
    })
}

/// 平台注入源的流规格（macOS CATap 的 tap 原生格式实测 48kHz/2ch/f32，S11）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalSourceSpec {
    /// 源采样率（非 16k，下游归一化重采样，§3.1-B/C）。
    pub sample_rate: u32,
    /// 源声道数（交错）。
    pub channels: u16,
}

/// 外部平台源的无锁灌帧句柄（实时回调内使用）。
///
/// **ADR-009 铁律同样适用**：`push_f32` 只做无锁 push + 满时原子计数，禁分配/锁/IO/日志——
/// 调用方（macOS CATap IOProc 等平台实时回调）持有它独占灌帧（SPSC 单生产者）。
pub struct FrameInjector {
    producer: rtrb::Producer<CapturedSample>,
    dropped: Arc<AtomicUsize>,
    source_lost: Arc<AtomicBool>,
    /// 采集时钟锚点（§3.1-I）：为注入批次打主机时间戳（wait-free，与麦克风路径同口径）。
    clock: Arc<CaptureClock>,
}

impl FrameInjector {
    /// 实时回调内推入一批交错 f32 样本（wait-free）。
    pub fn push_f32(&mut self, interleaved: &[f32]) {
        // 主机时钟锚点（§3.1-I）：`Instant::now` + 无锁原子写，ADR-009 许可（唯一重操作）。
        self.clock.mark_captured(interleaved.len());
        for &s in interleaved {
            if self.producer.push(CapturedSample::F32(s)).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// 实时回调内把左右两个平面声道交织后推入（wait-free、零分配）。
    /// Core Audio 的非交错 `AudioBufferList` 不能只读首缓冲，否则会丢失右声道。
    pub fn push_f32_planar_stereo(&mut self, left: &[f32], right: &[f32]) {
        let frames = left.len().min(right.len());
        self.clock.mark_captured(frames.saturating_mul(2));
        for index in 0..frames {
            for sample in [left[index], right[index]] {
                if self.producer.push(CapturedSample::F32(sample)).is_err() {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// 平台源失效（聚合设备被系统移除/输出设备切换致重建等）→ 消费线程上报
    /// [`CaptureDiagnostic::DeviceLost`]，上层走既有降级/重建路径（§3.1-E 同款语义）。
    pub fn mark_source_lost(&self) {
        self.source_lost.store(true, Ordering::Release);
    }
}

/// 启动一路**外部平台源**采集会话（macOS CATap 系统音频等，§3.1-I / ADR-019）。
///
/// 采集侧由平台层实现并经返回的 [`FrameInjector`] 灌帧；本函数只建环形缓冲 + 消费管线
/// （归一化/VAD/分段/波形/落盘与麦克风路径**完全同源**，下游零分叉）。内建零回调看门狗
/// （[`CaptureDiagnostic::NoCaptureData`]）——TCC 拒绝无任何错误码，超时无帧是唯一检测口径。
///
/// 停止顺序约定：先停平台源（不再灌帧），再 `handle.stop()` 收尾 flush/落盘；顺序颠倒无害
/// （环形缓冲消费端退出后 push 仅计数丢弃），但会丢停止瞬间的尾帧。
///
/// # Errors
/// 归一化器/管线构造失败时返回 [`CaptureError`]。
pub fn start_external_capture(
    cfg: &CaptureConfig,
    sink: Sender<CaptureEvent>,
    spec: ExternalSourceSpec,
    clock: SharedClock,
    aec: Option<AecRole>,
) -> Result<(CaptureHandle, FrameInjector), CaptureError> {
    let core = start_session_core(
        cfg,
        sink,
        spec.sample_rate,
        spec.channels,
        true,
        None,
        clock,
        aec,
    )?;
    let injector = FrameInjector {
        producer: core.producer,
        dropped: core.dropped,
        source_lost: Arc::clone(&core.device_lost),
        clock: Arc::clone(&core.clock),
    };
    Ok((
        CaptureHandle {
            stop: core.stop,
            muted: core.muted,
            clock: core.clock,
            owner: None,
            consumer: Some(core.consumer),
        },
        injector,
    ))
}

/// 会话共享核心：环形缓冲 + 消费线程（归一化/VAD/分段/落盘）。
///
/// 采集侧（cpal 拥有线程 / 平台注入）与消费侧在此解耦：`producer` 交还调用方接采集源。
struct SessionCore {
    producer: rtrb::Producer<CapturedSample>,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
    device_lost: Arc<AtomicBool>,
    clock: Arc<CaptureClock>,
    consumer: JoinHandle<Vec<PartInterval>>,
}

#[allow(clippy::too_many_arguments)] // 会话核心装配点齐平传参（同 spawn_consumer 风格）。
fn start_session_core(
    cfg: &CaptureConfig,
    sink: Sender<CaptureEvent>,
    sample_rate: u32,
    channels: u16,
    no_data_watchdog: bool,
    pacer: Option<LoopbackPacer>,
    clock: SharedClock,
    aec: Option<AecRole>,
) -> Result<SessionCore, CaptureError> {
    // Both 双轨传入共享 epoch 时钟；单轨自建（本会话 Instant::now() 为 epoch）。
    let clock = clock.unwrap_or_else(|| CaptureClock::new(Instant::now()));
    let capacity = ring_capacity(sample_rate, channels, cfg.buffer_secs);
    let (producer, consumer) = rtrb::RingBuffer::<CapturedSample>::new(capacity);

    let dropped = Arc::new(AtomicUsize::new(0));
    let device_lost = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let muted = Arc::new(AtomicBool::new(false));

    // 消费线程：drain rtrb → ConsumerPipeline → sink。
    // 按 CaptureConfig.mode 选管线策略（不再写死 VoiceInput）。
    //
    // 这个 match **只做枚举映射**，一个配置字段都不在这里填——装配全部交给
    // `PipelineConfig::for_mode`（单一构造点，理由见该函数文档）。此前三个分支各抄一遍
    // 覆盖列表，往 `CaptureTuning` 加字段就意味着要记得在三处分别补一行。
    let pipeline_mode = match cfg.mode {
        CaptureMode::VoiceInput => PipelineMode::VoiceInput,
        CaptureMode::Recording => PipelineMode::Recording,
        CaptureMode::RealtimeStream => PipelineMode::RealtimeStream,
    };
    let pipeline_cfg = PipelineConfig {
        // 会话级开关，不属于"用户调参"（Both 副轨要单独开电平），故不进 CaptureTuning。
        emit_level: cfg.emit_level,
        ..PipelineConfig::for_mode(pipeline_mode, sample_rate, &cfg.tuning)
    };
    let mut pipeline = match &cfg.vad {
        Some(factory) => ConsumerPipeline::with_vad(pipeline_cfg, (factory.0)())?,
        None => ConsumerPipeline::new(pipeline_cfg)?,
    };
    // AEC 接线（TDD-02 §3.1-I 层③，Both 编排层构造注入；None = 直通）。
    if let Some(role) = aec {
        pipeline.attach_aec(role);
    }

    // 落盘分叉（TDD-02 §2/§5.3）：有 SaveSpec 即构造 AudioFileWriter（专用写线程），
    // 把归一化 16k mono 帧的第三路喂给它；磁盘水位诊断与强制停录信号经 channel 回流消费线程。
    let (diag_rx, force_stop_rx) = if let Some(save) = &cfg.save {
        let (diag_tx, diag_rx) = std::sync::mpsc::channel::<CaptureDiagnostic>();
        let (fs_tx, fs_rx) = std::sync::mpsc::channel::<()>();
        let mut writer_cfg = WriterConfig::new(
            std::path::PathBuf::from(&save.path),
            save.format,
            save.sample_rate,
        );
        writer_cfg.disk_soft_threshold_bytes = cfg.tuning.disk_warn_bytes;
        writer_cfg.rolling_split_max_bytes = cfg.tuning.rolling_split_max_bytes;
        match AudioFileWriter::spawn(writer_cfg, 128, diag_tx, Some(fs_tx)) {
            Ok(writer) => {
                pipeline.attach_writer(writer);
                (Some(diag_rx), Some(fs_rx))
            }
            // 落盘初始化失败（创建文件失败）→ 上报 SaveUnavailable（**非致命**降级，区别于磁盘满），
            // 继续纯转写不落盘。绝不发 DiskCritical（那会被上层误判为「磁盘满」而终止会话）。
            Err(e) => {
                let _ = sink.send(CaptureEvent::Diagnostic(
                    CaptureDiagnostic::SaveUnavailable {
                        reason: e.to_string(),
                    },
                ));
                eprintln!("[luna-audio] 录音落盘初始化失败（继续不落盘）：{e}");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    let consumer_handle = spawn_consumer(
        consumer,
        sink,
        Arc::clone(&stop),
        Arc::clone(&muted),
        Arc::clone(&dropped),
        Arc::clone(&device_lost),
        Arc::clone(&clock),
        sample_rate,
        channels,
        pipeline,
        diag_rx,
        force_stop_rx,
        no_data_watchdog,
        pacer,
    );

    Ok(SessionCore {
        producer,
        stop,
        muted,
        dropped,
        device_lost,
        clock,
        consumer: consumer_handle,
    })
}

/// 可用输入设备（HUD 设备选择，PRD-04-003）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioInputDevice {
    /// 设备名（cpal 名即 `recording.input_device` 设置值）。
    pub name: String,
    /// 是否系统默认输入。
    pub is_default: bool,
}

/// 枚举当前可用输入设备（cpal 阻塞调用，调用方应放阻塞线程，勿在 async executor 上直跑）。
#[must_use]
pub fn list_input_devices() -> Vec<AudioInputDevice> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.name().ok())
                .map(|name| AudioInputDevice {
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    name,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 消费线程把一帧原始数据喂给管线并转发事件；接收端关闭时要求调用方收尾退出。
enum FeedOutcome {
    /// 继续消费。
    Continue,
    /// 事件接收端已关闭（上层会话结束）——仍需收尾落盘，保住已录音频。
    SinkClosed,
}

fn feed_pipeline(
    pipeline: &mut ConsumerPipeline,
    raw: &RawFrame,
    sink: &Sender<CaptureEvent>,
) -> FeedOutcome {
    match pipeline.process(raw) {
        Ok(events) => {
            for ev in events {
                if sink.send(ev).is_err() {
                    return FeedOutcome::SinkClosed;
                }
            }
            FeedOutcome::Continue
        }
        Err(_) => FeedOutcome::Continue, // 归一化偶发失败：跳过本帧，不中断会话
    }
}

#[allow(clippy::too_many_arguments)] // 线程注入点齐平传参（与 spawn_owner 同风格），不引共享上下文结构。
fn spawn_consumer(
    mut consumer: rtrb::Consumer<CapturedSample>,
    sink: Sender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
    device_lost: Arc<AtomicBool>,
    clock: Arc<CaptureClock>,
    sample_rate: u32,
    channels: u16,
    mut pipeline: ConsumerPipeline,
    diag_rx: Option<std::sync::mpsc::Receiver<CaptureDiagnostic>>,
    force_stop_rx: Option<std::sync::mpsc::Receiver<()>>,
    no_data_watchdog: bool,
    mut pacer: Option<LoopbackPacer>,
) -> JoinHandle<Vec<PartInterval>> {
    std::thread::Builder::new()
        .name("luna-audio-consumer".into())
        .spawn(move || {
            let mut device_lost_reported = false;
            let started_at = Instant::now();
            // 零回调看门狗（外部平台源专用，§3.1-I）：跟踪「上次真实数据」时刻；
            // 超时未见任何帧 → 上报一次 NoCaptureData；新数据到达即复位可再报（覆盖
            // 「会话中途授权被撤销」——S11 实测撤销同样表现为静默无回调）。
            let mut last_data_at = started_at;
            let mut no_data_reported = false;
            // 数字零输入看门狗（§3.1-G 补强）：有回调但内容恒为数字静默 ≥3s——
            // 麦克风 TCC 被拒时 macOS 给静音帧不报错，此看门狗是唯一检测口径。
            // 基于 muted 置零**前**的原始样本判定（软静音不误报）；任何非零样本复位
            // 可再报（覆盖会话中途权限被撤销）。
            let silent_input_threshold =
                SILENT_INPUT_SECS as usize * sample_rate as usize * channels.max(1) as usize;
            let mut silent_input_run: usize = 0;
            let mut silent_input_reported = false;
            // 双轨时钟漂移监视（§3.1-I）：**仅无 pacer 的连续源**（麦克风 / macOS CATap）——
            // Windows loopback 的补零已是主机时钟对齐产物，喂监视器即自我抵消谬误。
            let mut drift_monitor = pacer.is_none().then(|| DriftMonitor::new(sample_rate));
            let mut last_drift_eval = started_at;
            let mut drift_ticks: u32 = 0;
            // 「声明 ≠ 交付」一次性上报（ADR-082）：它不会自愈，逐秒重报只会刷屏 +
            // 灌满通知历史，故与 SilentInput 同款一次性闩。
            let mut rate_mismatch_reported = false;
            loop {
                let stopping = stop.load(Ordering::Acquire);
                // 软静音（PRD-04-003）：每轮读一次并**在喂管线之前**同步下去。
                // 显式传递，不让管线从"样本是不是精确零"反推——去直流高通在置零之后
                // 还有一小段非零拖尾，那段残渣穿得过精确零闸门、会污染噪声底
                // （理由与代价见 `ConsumerPipeline::set_muted`）。
                // 放在循环顶部而不是紧贴 feed：本轮可能走的是 loopback 补零那条喂帧路径，
                // 两条路径必须看到同一个静音状态。
                let is_muted = muted.load(Ordering::Acquire);
                pipeline.set_muted(is_muted);

                // 转发落盘写线程的磁盘水位诊断（DiskLow/DiskCritical/RolledToPart）。
                if let Some(rx) = &diag_rx {
                    while let Ok(diag) = rx.try_recv() {
                        let _ = sink.send(CaptureEvent::Diagnostic(diag));
                    }
                }
                // 磁盘硬阈值强制停录：写线程已保住已落盘音频，这里停采（TDD-02 §5.3 优雅停录）。
                if let Some(rx) = &force_stop_rx {
                    if rx.try_recv().is_ok() {
                        stop.store(true, Ordering::Release);
                    }
                }

                if device_lost.load(Ordering::Acquire) && !device_lost_reported {
                    device_lost_reported = true;
                    let _ = sink.send(CaptureEvent::Diagnostic(CaptureDiagnostic::DeviceLost));
                }
                let n_dropped = dropped.swap(0, Ordering::AcqRel);
                if n_dropped > 0 {
                    let _ = sink.send(CaptureEvent::Diagnostic(CaptureDiagnostic::RingOverflow {
                        dropped: n_dropped,
                    }));
                }

                // 双轨时钟漂移评估 + rate-aware 校正（§3.1-C/§3.1-I，节流每 1s）：以采集主机
                // 锚点为主轴，比 16k 输出累计样本估残余偏移，超阈值微调重采样比例；漂移量与
                // 校正次数入 §6 指标（节流上报，脱敏仅数值）。
                if let Some(mon) = drift_monitor.as_mut() {
                    if last_drift_eval.elapsed() >= DRIFT_EVAL_INTERVAL {
                        last_drift_eval = Instant::now();
                        if let Some(snap) = clock.snapshot() {
                            let device_frames = snap.produced_samples / u64::from(channels.max(1));
                            let out = pipeline.total_output_samples();
                            if let Some(rep) =
                                mon.observe(snap.host_span_nanos(), device_frames, out)
                            {
                                if let Some(rel) = rep.apply_rel {
                                    pipeline.set_resample_ratio_relative(rel);
                                }
                                if rep.rate_mismatch && !rate_mismatch_reported {
                                    rate_mismatch_reported = true;
                                    let _ = sink.send(CaptureEvent::Diagnostic(
                                        CaptureDiagnostic::SourceRateMismatch {
                                            declared_hz: sample_rate,
                                            measured_hz: f64::from(sample_rate)
                                                * (1.0 + rep.drift_ppm / 1e6),
                                        },
                                    ));
                                }
                                drift_ticks = drift_ticks.wrapping_add(1);
                                if rep.apply_rel.is_some()
                                    || drift_ticks.is_multiple_of(DRIFT_REPORT_EVERY)
                                {
                                    let _ = sink.send(CaptureEvent::Diagnostic(
                                        CaptureDiagnostic::ClockDrift {
                                            drift_ppm: rep.drift_ppm,
                                            offset_ms: rep.offset_ms,
                                            corrections: rep.corrections,
                                        },
                                    ));
                                }
                            }
                        }
                    }
                }

                // **按声道对齐读取**：只取 `channels` 整数倍的样本，余数留在环里等下次——
                // 否则跨读取边界的半组交错样本会让 `downmix_to_mono` 声道错位，在立体声设备上
                // 产生金属味伪声（电流声根因之二）。单声道设备恒对齐、无额外开销。
                let ch = channels.max(1) as usize;
                let slots = consumer.slots();
                let aligned = slots - (slots % ch);
                if aligned > 0 {
                    if let Ok(chunk) = consumer.read_chunk(aligned) {
                        let (a, b) = chunk.as_slices();
                        let mut buf = Vec::with_capacity(aligned);
                        buf.extend_from_slice(a);
                        buf.extend_from_slice(b);
                        chunk.commit_all();
                        // 数字零看门狗判定须在 muted 置零之前（原始样本才是设备事实）。
                        if buf.iter().all(|s| s.is_digital_silence()) {
                            silent_input_run = silent_input_run.saturating_add(buf.len());
                            if !silent_input_reported && silent_input_run >= silent_input_threshold
                            {
                                silent_input_reported = true;
                                let denom = u64::from(sample_rate) * u64::from(channels.max(1));
                                let _ = sink.send(CaptureEvent::Diagnostic(
                                    CaptureDiagnostic::SilentInput {
                                        elapsed_ms: (silent_input_run as u64 * 1000) / denom.max(1),
                                    },
                                ));
                            }
                        } else {
                            silent_input_run = 0;
                            silent_input_reported = false;
                        }
                        // 静音支路（PRD-04-003）：样本置静默但**数量不变**——时钟推进、
                        // 落盘节奏、VAD 帧切分全部照旧；管线已在循环顶部被告知 muted，
                        // 会以 Silence 推进端点/分段器 → 不产 ASR 窗。
                        if is_muted {
                            for sample in &mut buf {
                                *sample = sample.silence();
                            }
                        }
                        last_data_at = Instant::now();
                        no_data_reported = false;
                        if let Some(p) = pacer.as_mut() {
                            p.on_delivered(aligned);
                        }
                        let raw = RawFrame {
                            sample_rate,
                            channels,
                            samples: RawSamples::from_captured(buf),
                        };
                        if matches!(
                            feed_pipeline(&mut pipeline, &raw, &sink),
                            FeedOutcome::SinkClosed
                        ) {
                            return stop_writer(&mut pipeline);
                        }
                    }
                } else if stopping {
                    // 已停止且缓冲排空 → flush 收尾后退出，收尾落盘并返回分片区间表。
                    if let Ok(events) = pipeline.finish() {
                        for ev in events {
                            let _ = sink.send(ev);
                        }
                    }
                    return stop_writer(&mut pipeline);
                } else {
                    // 零回调看门狗（外部平台源，§3.1-I）：超时无任何帧 → 上报一次。
                    if no_data_watchdog && !no_data_reported {
                        let idle = last_data_at.elapsed();
                        if idle.as_millis() as u64 >= NO_DATA_TIMEOUT_MS {
                            no_data_reported = true;
                            let _ = sink.send(CaptureEvent::Diagnostic(
                                CaptureDiagnostic::NoCaptureData {
                                    elapsed_ms: idle.as_millis() as u64,
                                },
                            ));
                        }
                    }
                    // Windows loopback 静音期补零（§3.1-I）：按主机时钟核算差额，保持
                    // 时间轴/落盘节奏与真实时钟对齐；差额不足最小粒度时照常小睡。
                    let deficit = pacer
                        .as_mut()
                        .map_or(0, |p| p.take_silence_deficit(started_at.elapsed()));
                    if deficit > 0 {
                        let raw = RawFrame {
                            sample_rate,
                            channels,
                            samples: RawSamples::F32(vec![0.0; deficit]),
                        };
                        if matches!(
                            feed_pipeline(&mut pipeline, &raw, &sink),
                            FeedOutcome::SinkClosed
                        ) {
                            return stop_writer(&mut pipeline);
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        })
        .expect("spawn consumer thread")
}

/// 收尾落盘编码器（若有），返回分片区间表；无落盘时返回空 `Vec`。
fn stop_writer(pipeline: &mut ConsumerPipeline) -> Vec<PartInterval> {
    match pipeline.take_writer() {
        Some(w) => w.stop(),
        None => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)] // 线程注入点齐平传参（同 spawn_consumer 风格），不引共享上下文结构。
fn spawn_owner(
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    producer: rtrb::Producer<CapturedSample>,
    dropped: Arc<AtomicUsize>,
    device_lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    clock: Arc<CaptureClock>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("luna-audio-capture".into())
        .spawn(move || {
            let stream = match build_stream(
                &device,
                &config,
                sample_format,
                producer,
                Arc::clone(&dropped),
                Arc::clone(&device_lost),
                clock,
            ) {
                Ok(s) => s,
                Err(_) => {
                    device_lost.store(true, Ordering::Release);
                    return;
                }
            };
            if stream.play().is_err() {
                device_lost.store(true, Ordering::Release);
                return;
            }
            // 驻留持有 stream 至停止信号（drop 即停采，§3.1-D）。
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(stream);
        })
        .expect("spawn capture owner thread")
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    producer: rtrb::Producer<CapturedSample>,
    dropped: Arc<AtomicUsize>,
    device_lost: Arc<AtomicBool>,
    clock: Arc<CaptureClock>,
) -> Result<cpal::Stream, CaptureError> {
    let err_lost = Arc::clone(&device_lost);
    let err_cb = move |_err: cpal::StreamError| {
        // 错误回调 = 设备拔出/重配的唯一通知（§3.1-E）。
        err_lost.store(true, Ordering::Release);
    };
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_typed::<f32>(
            device,
            config,
            producer,
            dropped,
            clock,
            err_cb,
            CapturedSample::from_f32,
        ),
        cpal::SampleFormat::I16 => build_typed::<i16>(
            device,
            config,
            producer,
            dropped,
            clock,
            err_cb,
            CapturedSample::from_i16,
        ),
        cpal::SampleFormat::U16 => build_typed::<u16>(
            device,
            config,
            producer,
            dropped,
            clock,
            err_cb,
            CapturedSample::from_u16,
        ),
        other => return Err(CaptureError::Config(format!("不支持的样本格式: {other:?}"))),
    };
    stream.map_err(|e| CaptureError::BuildStream(e.to_string()))
}

#[allow(clippy::too_many_arguments)] // 采集流构建注入点齐平传参，不引共享上下文结构。
fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: rtrb::Producer<CapturedSample>,
    dropped: Arc<AtomicUsize>,
    clock: Arc<CaptureClock>,
    err_cb: impl FnMut(cpal::StreamError) + Send + 'static,
    capture: fn(T) -> CapturedSample,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Send + 'static,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // ADR-009：回调内只做无锁拷贝（push）+ 满时原子计数 + 主机时钟锚点。禁分配/锁/IO/日志。
            // 主机锚点（§3.1-I）：一次 `Instant::now` + 无锁原子写/批次，ADR-009 唯一许可的重操作。
            clock.mark_captured(data.len());
            for &s in data {
                if producer.push(capture(s)).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        err_cb,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CapturedSample;

    #[test]
    fn callback_samples_preserve_original_format() {
        assert_eq!(
            CapturedSample::from_i16(-32768),
            CapturedSample::I16(-32768)
        );
        assert_eq!(CapturedSample::from_u16(32768), CapturedSample::U16(32768));
        assert_eq!(CapturedSample::from_f32(0.25), CapturedSample::F32(0.25));
    }

    /// 数字零输入看门狗（§3.1-G 补强，headless）：连续纯零样本 ≥3s → `SilentInput`；
    /// 出现真实样本后复位、不再误报。麦克风 TCC 被拒（macOS 给静音帧不报错）的唯一
    /// 检测口径——绝不让用户「录了 N 分钟寂寞」。
    #[test]
    fn silent_input_watchdog_fires_resets_and_stops_cleanly() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = CaptureConfig {
            mode: crate::types::CaptureMode::Recording,
            emit_level: false,
            save: None,
            ..CaptureConfig::default()
        };
        let (handle, mut injector) = start_external_capture(
            &cfg,
            tx,
            ExternalSourceSpec {
                sample_rate: 16_000,
                channels: 1,
            },
            None,
            None,
        )
        .unwrap();

        // 注入 3.5s 纯零（16k mono；分批推进给消费线程排空时间）。
        for _ in 0..35 {
            injector.push_f32(&[0.0f32; 1_600]);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut saw_silent_input = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(CaptureEvent::Diagnostic(CaptureDiagnostic::SilentInput { elapsed_ms })) => {
                    assert!(
                        elapsed_ms >= 3_000,
                        "上报时长应 ≥ 阈值（实得 {elapsed_ms}）"
                    );
                    saw_silent_input = true;
                    break;
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(saw_silent_input, "3s 纯零输入必须触发 SilentInput 诊断");

        // 真实样本（底噪级非零）复位看门狗——短零段不再误报。
        injector.push_f32(&[0.01f32; 1_600]);
        injector.push_f32(&[0.0f32; 1_600]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(
                    event,
                    CaptureEvent::Diagnostic(CaptureDiagnostic::SilentInput { .. })
                ),
                "复位后的短零段不得再报"
            );
        }
        // 数字静默诊断不能改变采集句柄的停止语义：即使注入端仍存活，stop 也须排空后有界返回。
        let stop_started = std::time::Instant::now();
        let parts = handle.stop();
        assert!(parts.is_empty(), "无落盘配置不应产生分片");
        assert!(
            stop_started.elapsed() < std::time::Duration::from_secs(2),
            "数字静默后停止采集不得挂起"
        );
    }
}
