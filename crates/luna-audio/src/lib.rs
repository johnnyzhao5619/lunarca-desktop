//! `luna-audio` — 音频管线（采集 · 归一化 · VAD · 端点分段 · 波形）。
//!
//! 事实源：[`TDD-02`]、ADR-009（实时回调铁律）、ADR-011（高频流走二进制
//! Channel）、ADR-012（Silero VAD）。线程/执行模型见 [`TDD-00`] §5。
//!
//! 管线结构（TDD-02 §2）：
//! ```text
//! ① cpal 实时回调  ── 仅 try_push(原始交错样本) ──► [rtrb SPSC 环形缓冲]   (wait-free, ADR-009)
//! ② 消费线程       ── AudioNormalizer(格式→f32·降混·rubato 重采样到 16k mono)
//!                     ├─► [pre-roll] ─► Vad ─► Endpointer ─► Utterance ─► whisper(transcribe Job)
//!                     └─► WaveformMeter(RMS/峰值, 节流 ~18fps) ─► ipc::Channel(二进制) ─► 波形
//! ```
//!
//! **本增量（件 A）落地纯逻辑可单测核心**：[`types`]、[`ring`]、[`vad`]、[`level`]、
//! [`endpointer`]。`AudioNormalizer`（rubato）与 cpal `AudioCapture`（运行时）为后续增量。
//!
//! 依赖方向（TDD-00 §3 单向）：`luna-audio → luna-core`，不依赖任何上层。
#![forbid(unsafe_code)]

pub mod aec;
pub mod capture;
pub mod clock;
pub mod decoder;
pub mod degradation;
pub mod encoder;
pub mod endpointer;
#[cfg(test)]
mod golden;
pub mod level;
pub mod metrics;
pub mod muxer;
pub mod normalizer;
pub mod pacer;
pub mod pipeline;
pub mod ring;
pub mod segmenter;
/// 确定性测试信号发生器（仅测试编译；夹具电平口径的唯一事实源，见模块文档）。
#[cfg(test)]
mod testsig;
pub mod types;
pub mod vad;
pub mod writer;

pub use aec::{create_echo_cancel_pair, AecRole, EchoCanceller, FarEndFeeder};
pub use capture::{
    list_input_devices, start_capture, start_external_capture, AudioInputDevice, CaptureConfig,
    CaptureError, CaptureHandle, CaptureTuning, ExternalSourceSpec, FrameInjector, SaveSpec,
    VadFactory,
};
pub use clock::{CaptureClock, ClockSnapshot};
pub use decoder::{
    decode_media_to_pcm, decode_media_to_stereo_tracks, probe_media_duration_secs, DecodeError,
};
pub use degradation::{
    AsrLoad, DegradationController, DegradationDecision, DegradationState, SheddingTarget,
};
pub use encoder::{Encoder, EncoderError, RecordingFormat, WavEncoder};
pub use endpointer::{EndpointConfig, EndpointEvent, Endpointer, FinalizeReason};
pub use level::{
    encode_levels, tag_track_levels, LevelFrame, WaveformMeter, DEFAULT_WAVEFORM_FPS,
    LEVEL_TRACK_PRIMARY, LEVEL_TRACK_SYSTEM,
};
pub use metrics::{RtfTracker, DEFAULT_RTF_WINDOW};
pub use muxer::{mux_mono_to_stereo, MuxError};
pub use normalizer::{downmix_to_mono, to_f32_interleaved, AudioNormalizer, NormalizeError};
pub use pacer::LoopbackPacer;
pub use pipeline::{CaptureEvent, ConsumerPipeline, PipelineConfig, PipelineMode};
pub use ring::{ring_capacity, DEFAULT_BUFFER_SECS};
pub use segmenter::{SegmenterConfig, SpeechWindow, StreamSegmenter};
pub use types::{
    CaptureDiagnostic, CaptureMode, CaptureSource, CapturedSample, MonoF32Frame, RawFrame,
    RawSamples, SampleFormat, SpeechEvent, TARGET_SAMPLE_RATE,
};
pub use vad::{
    dbfs, frame_rms, EnergyVad, Vad, VadDecision, VadLabel, VadSensitivity, DBFS_FLOOR,
    F_FLOOR_MAX, F_FLOOR_MIN, F_INIT, NOISE_SUBWINDOWS, NOISE_SUBWINDOW_FRAMES, VAD_FRAME_SAMPLES,
};
pub use writer::{
    AudioFileWriter, PartInterval, WriterConfig, DEFAULT_DISK_CHECK_INTERVAL,
    DEFAULT_DISK_HARD_THRESHOLD_BYTES, DEFAULT_DISK_SOFT_THRESHOLD_BYTES,
    DEFAULT_ROLLING_SPLIT_MAX_BYTES,
};
