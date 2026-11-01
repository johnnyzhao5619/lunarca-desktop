//! `luna-engine` — 本地推理引擎部署（ADR-013 分治）。
//!
//! - [`llama`]（随包 `llama-server` 的运行时管理：preset、内存预检、路由器监管，ADR-143）。
//! - [`WhisperProvider`]（ASR 进程内专用阻塞线程池，ADR-010，Phase 4）——实现 `AIProvider`，
//!   对 JobQueue/Router 与 LLM Provider 等价（DRY）。
//!
//! 后续增量：CoreML 后端（Q3 后置）、FFmpeg sidecar（批量 P1）。
//! 事实源：[`TDD-03`]、[`TDD-04`] §2.3。
#![forbid(unsafe_code)]

mod hallucination;
pub mod llama;
mod whisper;

pub use whisper::{
    SharedStreamLang, WhisperBackend, WhisperConfig, WhisperError, WhisperProvider,
    BATCH_PROVIDER_ID as WHISPER_BATCH_PROVIDER_ID, PROVIDER_ID as WHISPER_PROVIDER_ID,
};

/// 本构建是否编入了 whisper.cpp 的 CoreML 编码器后端（ADR-115）。
///
/// 与 `Cargo.toml` 里 `[target.'cfg(target_os = "macos")'.dependencies]` 那一段是同一件事的
/// 两面：CoreML 特性只在 macOS 目标上打开（Cargo 特性不能按目标条件声明，故以目标判定），
/// 运行期不需要开关——`<weights>-encoder.mlmodelc` 在场即用，缺席或加载失败自动回落。
pub const COREML_COMPILED_IN: bool = cfg!(target_os = "macos");
