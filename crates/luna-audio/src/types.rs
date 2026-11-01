//! 音频管线的纯数据类型（零运行时句柄）。事实源：TDD-02 §3/§4。
//!
//! 设计目标 1（TDD-02 §1）：**正确性优先**——先保证"任何设备/格式都能采到正确的
//! 16k mono 数据"。故格式（`SampleFormat`）、声道、采样率全部显式携带，**不假设**
//! f32/mono/16k（"假设 f32/mono/16k"是采到噪声/静音的头号代码层根因，§3.1-B）。

use crate::vad::VadDecision;
use serde::{Deserialize, Serialize};

/// 转写管线统一目标采样率（16k mono f32，whisper 输入约定，TDD-02 §1/§3.6）。
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// 设备样本格式（与 `cpal::SampleFormat` 对应，但本 crate 自持以保持归一化逻辑
/// 可脱离 cpal 单测，§3.1-B 全分支统一转 f32）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    /// 有符号 16-bit 整数（转 f32 用 `/32768.0`）。
    I16,
    /// 无符号 16-bit 整数（先减偏置 32768 再 `/32768.0`）。
    U16,
    /// 32-bit 浮点（已在 [-1, 1]）。
    F32,
}

/// 采集回调取出的**原始交错样本**（多声道交错存储，未降混/未重采样）。
#[derive(Clone, Debug, PartialEq)]
pub enum RawSamples {
    /// i16 交错样本。
    I16(Vec<i16>),
    /// u16 交错样本。
    U16(Vec<u16>),
    /// f32 交错样本。
    F32(Vec<f32>),
}

/// cpal data callback 中推入 SPSC 环形缓冲的单个原始样本。
///
/// 该类型刻意保留设备样本格式，确保 ADR-009 实时回调只做无锁拷贝；格式转换统一留给消费线程的
/// [`crate::AudioNormalizer`]。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CapturedSample {
    /// i16 原始样本。
    I16(i16),
    /// u16 原始样本。
    U16(u16),
    /// f32 原始样本。
    F32(f32),
}

impl CapturedSample {
    /// 从 i16 构造。
    #[must_use]
    pub fn from_i16(sample: i16) -> Self {
        Self::I16(sample)
    }

    /// 从 u16 构造。
    #[must_use]
    pub fn from_u16(sample: u16) -> Self {
        Self::U16(sample)
    }

    /// 从 f32 构造。
    #[must_use]
    pub fn from_f32(sample: f32) -> Self {
        Self::F32(sample)
    }

    /// 同格式的静音样本（静音=数字静默：i16/f32 为 0，u16 为无符号中点）。
    ///
    /// 用于消费线程静音支路（PRD-04-003「静音」）：保持样本数与时钟推进不变，
    /// 仅内容置静默——时间轴与落盘逐样本对齐不受影响。
    #[must_use]
    pub fn silence(self) -> Self {
        match self {
            Self::I16(_) => Self::I16(0),
            Self::U16(_) => Self::U16(1 << 15),
            Self::F32(_) => Self::F32(0.0),
        }
    }

    /// 是否为**数字静默**（精确零值/无符号中点；f32 的 ±0.0 均判静默）。
    /// 真实麦克风必有底噪抖动，连续数字静默是「输入无信号」的可靠指纹
    /// （[`CaptureDiagnostic::SilentInput`] 看门狗判据）。
    #[must_use]
    pub fn is_digital_silence(self) -> bool {
        self == self.silence()
    }
}

impl RawSamples {
    /// 对应的样本格式。
    #[must_use]
    pub fn format(&self) -> SampleFormat {
        match self {
            Self::I16(_) => SampleFormat::I16,
            Self::U16(_) => SampleFormat::U16,
            Self::F32(_) => SampleFormat::F32,
        }
    }

    /// 交错样本总数（= 帧数 × 声道数）。
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::I16(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 把回调侧原始样本批量还原为按格式分组的 [`RawSamples`]。
    ///
    /// 采集会话内 cpal stream 的 sample format 固定，因此实际输入应全为同一 variant；若未来
    /// 设备热切换改变格式，应重建 stream，而不是在同一环形缓冲内混写格式。
    #[must_use]
    pub fn from_captured(samples: Vec<CapturedSample>) -> Self {
        match samples.first().copied() {
            Some(CapturedSample::I16(_)) => {
                debug_assert!(samples.iter().all(|s| matches!(s, CapturedSample::I16(_))));
                Self::I16(
                    samples
                        .into_iter()
                        .map(|s| match s {
                            CapturedSample::I16(v) => v,
                            _ => unreachable!("capture ring mixed sample formats"),
                        })
                        .collect(),
                )
            }
            Some(CapturedSample::U16(_)) => {
                debug_assert!(samples.iter().all(|s| matches!(s, CapturedSample::U16(_))));
                Self::U16(
                    samples
                        .into_iter()
                        .map(|s| match s {
                            CapturedSample::U16(v) => v,
                            _ => unreachable!("capture ring mixed sample formats"),
                        })
                        .collect(),
                )
            }
            Some(CapturedSample::F32(_)) | None => {
                debug_assert!(samples.iter().all(|s| matches!(s, CapturedSample::F32(_))));
                Self::F32(
                    samples
                        .into_iter()
                        .map(|s| match s {
                            CapturedSample::F32(v) => v,
                            _ => unreachable!("capture ring mixed sample formats"),
                        })
                        .collect(),
                )
            }
        }
    }
}

/// 一帧原始采集数据（消费线程从环形缓冲取出后构造）。
#[derive(Clone, Debug, PartialEq)]
pub struct RawFrame {
    /// 设备采样率（常见 44.1k/48k，**非 16k**，§3.1-B）。
    pub sample_rate: u32,
    /// 声道数（常见立体声=2，交错存储）。
    pub channels: u16,
    /// 原始交错样本。
    pub samples: RawSamples,
}

/// 归一化后的单声道 f32 帧（已降混；采样率可能仍是设备率，待重采样到 16k）。
#[derive(Clone, Debug, PartialEq)]
pub struct MonoF32Frame {
    /// 当前采样率。
    pub sample_rate: u32,
    /// 单声道 f32 样本。
    pub samples: Vec<f32>,
}

/// 采集源抽象——为"会议录对端声音"（loopback）预留扩展点（§3.1-I）。
///
/// 决策：录音 P1 首批仅实现 `InputDevice`（麦克风）；`SystemLoopback`/`Mixed` 作
/// P1 spike/后续小阶段，但**现在就把扩展点留出**，避免后期把 loopback 硬塞进只为
/// 麦克风设计的管线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureSource {
    /// cpal 输入设备（`None` = 系统默认输入；`Some(name)` = 具名设备）。
    InputDevice(Option<String>),
    /// 系统输出回环（ScreenCaptureKit/WASAPI loopback，P1 spike/后续）。
    SystemLoopback,
}

impl Default for CaptureSource {
    fn default() -> Self {
        Self::InputDevice(None)
    }
}

/// 采集模式（决定下游消费方式，TDD-02 §4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaptureMode {
    /// 语音输入（短句，端点整段，MVP 默认）。
    #[default]
    VoiceInput,
    /// 快捷录音（长，P1）。
    Recording,
    /// 实时转录（流式，P1 Phase 8）。
    RealtimeStream,
}

/// 采集自检/诊断事件（暴露给 UI，避免"采不到/静音"用户干等，§3.1-G/§6）。
///
/// 不派生 `Eq`：[`Self::ClockDrift`] 携带 f64 漂移量（仅 `PartialEq`，诊断枚举无键值用途）。
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureDiagnostic {
    /// 连续静音超阈值——疑似未授权/选错设备/设备静音（§3.1-G）。
    SuspectedSilence {
        /// 已持续静音的毫秒数。
        elapsed_ms: u64,
        /// 这段静音里的**峰值电平**（dBFS，−∞ 记 [`crate::vad::DBFS_FLOOR`]）。
        ///
        /// **只说「判不出语音」是半句话**：VAD 判静音有两种完全不同的原因——真的没人说话
        /// （峰值在 −60dBFS 上下的底噪），与**信号被削掉了**（峰值 −40dBFS 却仍判静音，说明
        /// 门限或增益不对）。少了这个数，现场就只能靠换配置去猜（2026-08-18：Both 档下麦克风
        /// 轨反复判静音而用户一直在说话，判不出是 VPIO 消掉了信号还是门限太高）。
        peak_dbfs: f32,
        /// 判静音时 VAD 用的**全部依据**（本帧 RMS / 噪声底 / 门限，见 [`VadDecision`]）。
        ///
        /// 门限改为自适应之后，光有峰值又变回半句话：门限不再是个能从代码里读出来的
        /// 常量，它随本轨噪声底实时变化。现场要回答的是「峰值 −40 却判静音，是因为底被
        /// 抬到了 −35，还是这一帧的 RMS 本来就只有 −56」——三个数一起给才答得上。
        decision: VadDecision,
    },
    /// 连续判语音超阈值（默认 10s）——噪声底跟踪疑似被**锁死**，门限已失效。
    ///
    /// **这是自适应门限唯一的现场可见失效信号**。底跟踪一旦被异常信号锁住
    /// （持续大信号把底顶到上夹取、设备回放自激、外部 AGC 追着门限跑），VAD 会一路判语音，
    /// 而下游表现只是「分段一直不收尾 / 转写出一堆无中生有的字」——从那些症状根本反推
    /// 不到 VAD 身上。本仓已经证明「离线全绿、现场不出字」会发生，且当时唯一缺的就是
    /// 这类「判定本身坏了」的直陈信号。
    ///
    /// 首个 `Silence` 帧即复位（连同「已上报」标志），故长会里每次锁死各报一次、不刷屏。
    /// 纯数值脱敏，**不升级用户通知**（它是给日志看的，不是给用户看的）。
    VadSaturated {
        /// 已连续判语音的毫秒数（触发时 ≥ 阈值）。
        speech_run_ms: u64,
    },
    /// 有回调但内容恒为**数字静默**（精确零）超阈值（§3.1-G 补强）。真实麦克风必有
    /// 底噪，连续数字零是「麦克风 TCC 被拒（macOS 对未授权输入给静音帧、不报错）/
    /// 系统输入音量为 0 / 虚拟设备无信号」的指纹——与 [`Self::SuspectedSilence`]
    /// （能量低=环境安静，正常沉默也触发）语义不同。基于 muted 置零**前**的原始样本
    /// 判定（软静音不误报）；语义归属在编排层：系统回环轨的零=系统静音属正常，仅
    /// 麦克风主轨升级健康提示。
    SilentInput {
        /// 已持续数字静默的毫秒数。
        elapsed_ms: u64,
    },
    /// 设备不可用/被拔出，正在重建（§3.1-E）。
    DeviceLost,
    /// 环形缓冲溢出，丢弃了不可丢的转写音频（容量低估，§3.1 公式）。
    RingOverflow {
        /// 本次丢弃的样本数。
        dropped: usize,
    },
    /// 磁盘剩余空间低于软阈值——预警（TDD-02 §5.3，Task 2 使用）。
    DiskLow {
        /// 当前卷剩余字节数。
        free_bytes: u64,
    },
    /// 磁盘剩余空间低于硬阈值——即将强制停录（TDD-02 §5.3，Task 2 使用）。
    DiskCritical {
        /// 当前卷剩余字节数。
        free_bytes: u64,
    },
    /// 录音文件逼近容器上限，已滚动切分到新分片（TDD-02 §5.3，Task 2 使用）。
    RolledToPart {
        /// 新分片编号（从 1 起）。
        part: u32,
    },
    /// 落盘不可用——编码器初始化失败（权限/只读卷等），本次会话**仅实时转写不落盘**。
    ///
    /// 语义上区别于 [`DiskCritical`]（录音中途磁盘满需优雅停录）：这是**会话起点**的落盘初始化
    /// 失败，属**非致命降级**——不终止会话，由上层告警「本次录音不落盘」并继续转写（§3.1-G）。
    SaveUnavailable {
        /// 失败原因（脱敏：仅 IO/路径级错误，绝不含音频样本）。
        reason: String,
    },
    /// 指定输入设备未找到，已回退系统默认设备（PRD-04 §5「设备拔出/切换回退默认」）。
    ///
    /// **非致命**：会话以默认设备继续；上层告警提示用户检查 `recording.input_device` 设置。
    InputDeviceFallback {
        /// 用户设置中请求的设备名（脱敏：设备名属系统配置，非用户内容）。
        requested: String,
    },
    /// 采集源自会话启动起**零回调超时**——一帧数据都没到达（§3.1-I，S11 实测口径）。
    ///
    /// 与 [`SuspectedSilence`](Self::SuspectedSilence)（有回调、内容为静音）语义不同：macOS
    /// 系统音频 tap 的 IOProc 即使系统无声也持续回调（内容为零）；**完全无回调 = TCC
    /// 「系统音频录制」未授权/被撤销，或聚合设备失效**——且系统不给任何错误码，此看门狗是
    /// 唯一检测手段。上层据此引导授权并降级仅麦克风（PRD-04-052）。
    NoCaptureData {
        /// 自会话启动起的等待毫秒数。
        elapsed_ms: u64,
    },
    /// 双轨时钟漂移量与重采样校正次数（§6 采集指标；节流上报，纯数值脱敏）。
    ///
    /// `drift_ppm` = 设备时钟相对主机的漂移（物理量，正 = 设备快）；`offset_ms` = 16k 输出
    /// 相对主机时钟的累计残余偏移（校正后趋近 0）；`corrections` = 会话累计校正次数。
    /// 供现场诊断长会时间线错位/AEC 失效（TDD-02 §3.1-I），不升级用户通知。
    ClockDrift {
        /// 设备时钟漂移（ppm）。
        drift_ppm: f64,
        /// 16k 输出相对主机的累计残余偏移（ms）。
        offset_ms: f64,
        /// 会话累计重采样校正次数。
        corrections: u32,
    },
    /// **本轨声明的采集格式与设备实际交付对不上**（一次性，ADR-082）。
    ///
    /// 与 [`Self::ClockDrift`] 是两回事，故不共用一个变体：漂移是 ppm 量级的物理量、会被
    /// 闭环校正吃掉；「不符」是我们对交付形态的假设错了（真机案例：Both 档 VPIO 给默认
    /// 输出设备装的 6 声道回声参考流被当成 2 声道 tap 计量 → 3.00×），校正上限 500ppm 面对
    /// 它差四个数量级，**永远追不上、不会自愈**，且这条轨的时间线在倍速前进——落盘混流
    /// 错位、云端按解出的 PCM 秒计费一起倍增。故它必须**升级为用户可见的通知**，
    /// 不许只留一行日志继续录（"成功但什么都没有"同族纪律）。
    SourceRateMismatch {
        /// 声明的采集采样率（Hz）。
        declared_hz: u32,
        /// 由实际采集帧数 / 主机时钟跨度反推的交付帧率（Hz）。
        measured_hz: f64,
    },
    /// 本轨**帧 RMS 的分位数分布**（§6「采集：输入电平」，每轨独立、按固定窗节流上报）。
    ///
    /// # 为什么峰值不够、必须是分布
    ///
    /// [`Self::SuspectedSilence`] 带的是**峰值**，而 VAD 的判据是**帧 RMS**——同一段语音，
    /// 两者差 12–18dB（正弦差 3dB，真实语音的波峰因子更大）。拿峰值去反推门限余量是
    /// **量纲错误**：按峰值看"信号明明有 −40dBFS"，换算到帧 RMS 可能只有 −56dBFS，
    /// 正好落在门限之下——这正是「用户一直在说话，VAD 一路判静音」的成因，而只看峰值
    /// 永远看不出来。
    ///
    /// 分位数则直接回答「门限该定在哪」，且**一条日志三个数就够**：
    /// - `p10` ≈ 这条轨的**噪声底**（安静帧占多数，十分位落在纯底噪上）；
    /// - `p90` ≈ **语音电平**（说话时的帧集中在高分位）；
    /// - `p90 − p10` = 这条轨真实可用的 **SNR**——它是判定「门限有没有落在噪声与语音之间」
    ///   的唯一依据，也是判定「这条轨压根没有可分离的语音」（差值趋 0）的唯一依据；
    /// - `p50` 给出分布重心（偏向 p10 = 大部分时间在沉默，偏向 p90 = 几乎一直有声）；
    /// - `min` 是封底哨兵的可见性检查：恒为 [`crate::vad::DBFS_FLOOR`] 说明窗内出现过精确
    ///   数字零（与 [`Self::SilentInput`] 互证）。
    ///
    /// **常驻诊断，不是临时打印**：门限一旦要按设备/环境自适应，唯一能回答"定在哪"的输入
    /// 就是这条；现场只跑一次、拿不到分布，就只能继续靠换配置去猜。
    ///
    /// 纯数值、零内容（不含任何音频样本或转写文本），**不升级用户通知**。
    InputLevels {
        /// 本次统计覆盖的音频时长（ms）。
        window_ms: u64,
        /// 帧 RMS 的 10 分位（dBFS，≈ 噪声底）。
        p10_dbfs: f32,
        /// 帧 RMS 的中位数（dBFS）。
        p50_dbfs: f32,
        /// 帧 RMS 的 90 分位（dBFS，≈ 语音电平）。
        p90_dbfs: f32,
        /// 窗内最小帧 RMS（dBFS，−∞ 记 [`crate::vad::DBFS_FLOOR`]）。
        min_dbfs: f32,
    },
}

/// 流式分段器产出的语音事件（TDD-02 §3.3-B，Task 1）。
///
/// `Window` 变体由 `StreamSegmenter` 产出，携带会话绝对起点偏移供 LocalAgreement-2 换算绝对时间。
/// `Utterance` 变体由 `Endpointer` 产出（端点整段，语音输入路径，既有）。
#[derive(Clone, Debug, PartialEq)]
pub enum SpeechEvent {
    /// 流式窗（实时转录路径）：VAD 门控滑动窗 + overlap + padding。
    /// 同一 utterance 的增长快照与终窗共享 `abs_start`（LA-2 同窗语义 + 解码端合并）。
    Window {
        /// 单声道 16k f32 样本（已 padding 到 window_samples）。
        samples: Vec<f32>,
        /// 本窗在会话中的绝对起点（采样数，÷ TARGET_SAMPLE_RATE = 绝对秒）。
        abs_start: u64,
        /// 本窗实际语音内容的样本数（padding 前）。
        speech_len: usize,
        /// 相比本 utterance 上一个已产出窗**新增**的语音样本数（RTF `T_audio` 只计增量）。
        speech_delta: usize,
        /// 是否为**增长快照**（同 utterance 中途滚动出字用，非终窗）。
        ///
        /// 用于消费端自适应卸载负载（TDD-02 §5.2）：高负载档位（Yellow/Orange/Red）**跳过快照解码**、
        /// 只解终窗/强切窗——去掉连续语音下 ~3× 的快照重解码开销，释放 CPU（终窗仍定稿全量文本，
        /// 不丢内容，仅牺牲 utterance 中途的滚动实时性）。
        is_snapshot: bool,
    },
    /// 端点整段（语音输入路径，既有路径事件，此处仅作统一枚举）。
    Utterance {
        /// 单声道 16k f32 样本。
        samples: Vec<f32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_samples_format_and_len() {
        let s = RawSamples::I16(vec![0, 1, 2, 3]);
        assert_eq!(s.format(), SampleFormat::I16);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        assert!(RawSamples::F32(vec![]).is_empty());
    }

    #[test]
    fn raw_samples_from_captured_preserves_format() {
        assert_eq!(
            RawSamples::from_captured(vec![CapturedSample::I16(-1), CapturedSample::I16(2)]),
            RawSamples::I16(vec![-1, 2])
        );
        assert_eq!(
            RawSamples::from_captured(vec![CapturedSample::U16(0), CapturedSample::U16(32768)]),
            RawSamples::U16(vec![0, 32768])
        );
        assert_eq!(
            RawSamples::from_captured(vec![CapturedSample::F32(0.25)]),
            RawSamples::F32(vec![0.25])
        );
    }

    #[test]
    fn capture_source_defaults_to_default_input() {
        assert_eq!(CaptureSource::default(), CaptureSource::InputDevice(None));
    }
}
