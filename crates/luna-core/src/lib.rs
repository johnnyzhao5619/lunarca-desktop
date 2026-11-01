//! `luna-core` — Lunarca 的领域模型（**零 IO**，可被所有上层依赖）。
//!
//! 事实源：[`TDD-01`] §2（领域模型）、
//! [`TDD-00`] §3（crate 划分与单向依赖）。
//!
//! 本 crate 不做任何 IO（无文件/网络/线程/阻塞），仅承载纯数据类型与其上的
//! 不变量（能力映射、优先级序、错误分类）。这是 ADR-001"单一 AI 执行主干"的
//! 种子：上层的 Provider / JobQueue / 导出 / Vault 全部围绕这里的类型构建。
#![forbid(unsafe_code)]

pub mod backoff;
pub mod cancel;
pub mod capability;
pub mod chapters;
pub mod chat;
pub mod committed;
pub mod cue;
pub mod distill_template;
pub mod echo;
pub mod error;
pub mod event;
pub mod filename_template;
pub mod glossary;
pub mod ids;
pub mod job;
pub mod lang;
pub mod preset;
pub mod prompt;
pub mod prompter_align;
pub mod provider;
pub mod result;
pub mod segment;
mod text_norm;
pub mod time;
pub mod translate;
pub mod usage;

pub use backoff::Backoff;
pub use cancel::{TaskCancel, CANCELLED_CODE};
pub use capability::Capability;
pub use chat::{ChatMessage, ChatRole, ToolSpec};
pub use committed::CommittedLedger;
pub use cue::{
    is_cjk_dominant, segments_to_cues, subtitle_cps, subtitle_issues, Cue, CueConstraints, CueId,
    SubtitleIssue,
};
pub use distill_template::{
    compose_chunk_instruction, compose_instruction, compose_merge_instruction,
    compose_rewrite_instruction, find as find_distill_template, normalize as normalize_template,
    parse_pack_template as parse_pack_distill_template, resolve as resolve_distill_template,
    strip_dead_citations, suggest, validate as validate_distill_template, KnownFacts, PackTemplate,
    PackTemplateError, Resolution, Section, SourceShape, SuggestReason, TemplateError, TemplateId,
    TemplateRef, TemplateSet, UserSection, UserTemplate, MAX_USER_TEMPLATES,
};
pub use echo::drop_cross_track_echo;
pub use error::{JobError, PermissionKind};
pub use event::{
    EtaRange, ExecutorInfo, ExecutorKind, JobEvent, PartialPayload, QueueTier, ReconnectPhase,
    RemoteQueueStatus,
};
pub use glossary::{
    apply_glossary, apply_glossary_to_segments, glossary_constraint_for, glossary_initial_prompt,
    parse_glossary_import, with_glossary_constraint, GlossaryEntry,
};
pub use ids::{JobId, ModelId, PresetId, ProviderId, SegmentId, StreamId, UploadId};
pub use job::{
    AdmissionLane, AudioRef, EmbeddingPurpose, ImageRef, Job, JobKind, JobOptions, JobOrigin,
    ModelSlot, PcmBuffer, Priority, QualityTier, RoutingHint, ServiceClass, SummaryStyle, TextRef,
    WindowInput,
};
pub use lang::Lang;
pub use preset::{builtin_chat_presets, builtin_preset, builtin_presets, PresetKind, PromptPreset};
pub use prompt::{
    manifest as prompt_manifest, registry as prompt_registry, CapabilityPromptSet, Instruction,
    PromptManifest, PromptManifestScope, PromptManifestTemplate, PromptOverlayInstalled,
    PromptOverlayRejected, PromptRegistry, PromptRegistryStatus, PromptScope, PromptStamp,
    PROMPT_MANIFEST_VERSION, REQUIRED_TEMPLATES,
};
pub use provider::{
    AIProvider, AllowOutbound, CancelToken, CapabilitySet, HealthStatus, JobEventStream,
    OutboundGate, ProviderDescriptor, ProviderHealth, ProviderLocation, StreamLangControl,
    StreamRegistration, StreamRegistry, StreamRelease, StreamingAsr, UsageEstimate,
};
pub use result::{
    JobResult, Money, ResultPayload, Segment, Timings, TruncatedBy, Usage, WordTiming,
};
pub use segment::{
    assign_speaker, delete_segment, join_transcript_text, merge_with_previous, next_speaker_id,
    speaker_roster, split_segment, AssignScope, SegmentEditError, SpeakerStat,
};
pub use time::Timestamp;
pub use translate::{
    compose_job_kind, decide_branch, distribute_translation, langs_equal, resolve_effective_target,
    Branch, BranchKind, Detection, LangContext, LanguageDetector, TargetLangMode,
};
pub use usage::{TerminalAttribution, UsageRecord, UNRESOLVED_PROVIDER};
