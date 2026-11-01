//! `luna-jobqueue` — Job 调度（TDD-01 §5）。
//!
//! Phase 1 落地：
//! - **提交即绑定事件流（无竞态，§5.3）**：`submit` 同步返回 `JobHandle`，其事件通道在
//!   命令返回那一刻已存在，不存在"先拿 id 再订阅、中间丢 `Queued`/`Started`"的窗口。
//! - **优先级调度 + 分层准入闸门（ADR-078 ①）**：`Interactive > Normal > Background`，
//!   同优先级 FIFO；额度由**两个**信号量控制——`AdmissionLane::Session`（在飞的实时会话
//!   的腿）与 `AdmissionLane::Compute`（领一份活算完就还的重计算单元）互不占用。一条
//!   流式 ASR Job 会在飞几分钟而真正的计算逐窗发生，拿它去占一个「同时在算」的额度是
//!   类型错误；错了的表现是「批式转写在跑时，新开的实时会话一个字都不出」。
//! - **卸载方向（ADR-078 ②）**：高负载时先停的是**批式转写**的准入（音频已全量落盘、
//!   无截止期），而不是错过就永远补不回来的实时窗。队列侧只提供事实（`AsrLoadGauge`
//!   的在飞计数与准入闸），裁决归 `RealtimeSession`（TDD-02 §5.2 决策归属）。
//! - **等待也说出口（ADR-078 ③）**：会话池的 Job 等超 `STREAM_ADMISSION_NOTICE` 发
//!   `JobEvent::AdmissionDelayed`，等超 `STREAM_ADMISSION_LIMIT` 以
//!   `JobError::LocalSessionBusy` 终态放弃——排队本身此前零观测。
//! - **路由执行 + 取消传播**：经 `Router` 解析候选 → 选定 Provider 执行 → 转发 `JobEvent`；
//!   `JobHandle::cancel()` 一路传播到 Provider 的 `CancelToken`（§5.5）。
//! - **兜底硬边界（§4）**：候选 Provider 在**向消费者转发任何事件之前**失败（可重试）才
//!   一跳回退到下一候选；一旦选定（转发过 `Started`/产出），中途失败**不再切换**——
//!   保留已产出、终态 `Failed`。这是 §4.1"首个 Partial 后不跨 Provider 重来"的保守子集
//!   （更严：选定即不切换，杜绝可见的 Provider 跳变）。
//!
//! 抢占 / 持久化续跑 / 可配兜底链为后续增量（§5.2/§5.4/§4.1）。
//!
//! **「内存预算信号量」这条已经不在增量清单里了**（2026-08-18，ADR-078）：它原本要解决
//! 的是「同时在算的东西太多把内存撑爆」，而这一批把并发额度按**泳道**分开之后，那个单一
//! 数字的语义已经不成立——会话池的额度度量的是「有几条流在飞」（几乎不吃内存，真正的
//! 计算逐窗发生），计算池的额度度量的是「有几台引擎在算」。再往上叠一个跨泳道的内存预算
//! 信号量，等于把刚拆开的两种量纲又合回一个数。内存压力的正确接缝是 `AsrLoad` 那一侧的
//! 卸载判据（先停没有截止期的那一份活），不是准入额度。
#![forbid(unsafe_code)]

mod chat_session;
mod queue;
mod realtime;
mod realtime_translate;

pub use chat_session::{
    ChatContext, ChatSession, ChatSessionConfig, ChatSessionError, ChatTurn, ChatTurnOutcome,
};
pub use queue::{
    AdmissionCapacity, AsrLoadGauge, JobHandle, JobQueue, JobSink, NoopUsageSink, UsageSink,
    STREAM_ADMISSION_LIMIT, STREAM_ADMISSION_NOTICE,
};
pub use realtime::{DegradationReason, DegradedEvent, RealtimeSession};
pub use realtime_translate::{
    document_instruction, live_instruction, translate_segments_batch, BatchTranslateOutcome,
    RealtimeTranslationPipeline, SentenceBuffer, TranslateUnit, TranslatedUnit, TranslationConfig,
    TranslationEvent, UnitPart,
};
