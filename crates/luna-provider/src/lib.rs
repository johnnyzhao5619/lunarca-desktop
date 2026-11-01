//! `luna-provider` — Provider 实现与路由（TDD-01 §3/§4）。
//!
//! Phase 1：`MockProvider`（契约测试 + 走查骨架）+ `Router`（能力匹配 / 严格本地过滤 /
//! 首选 + 兜底有序列表）+ `ProviderRegistry` + **契约检查器**（对任意 `AIProvider` 校验
//! 流式不变量）。真实 Provider（随包推理引擎 / `openai-compat` / 云端）必须通过同一套
//! 契约（TDD-01 §8）。
#![forbid(unsafe_code)]

pub mod asr_failover;
pub mod asr_realtime;
pub mod asr_reconnect;
pub mod asr_remote;
pub mod breaker;
pub mod cloud_embed;
pub mod cloud_llm;
pub mod contract;
pub mod embed_http;
mod idempotency;
pub mod lang_detect;
pub mod llm_http;
pub mod local_engine;
pub mod mock;
pub mod opus_uplink;
pub mod registry;
pub mod router;
pub mod sse;
pub mod token_counter;

pub use cloud_embed::{CloudEmbedConfig, CloudEmbedProvider, CLOUD_EMBED_PROVIDER_ID};
pub use cloud_llm::{CloudLlmConfig, CloudLlmProvider, CLOUD_LLM_PROVIDER_ID};
pub use contract::{check_invariants, StreamSummary, TerminalKind};
pub use embed_http::{parse_embeddings, EmbedHttpConfig, EmbedHttpProvider};
pub use lang_detect::LinguaDetector;
pub use llm_http::{build_chat_messages, engine_base, parse_delta, LlmHttpConfig, LlmHttpProvider};
pub use local_engine::{
    LocalEngineEndpoint, LocalEngineSource, LOCAL_EMBED_PROVIDER_ID, LOCAL_LLM_PROVIDER_ID,
    LOCAL_LLM_REALTIME_PROVIDER_ID,
};
pub use mock::{capability_set, MockProvider, MockScript};
pub use registry::ProviderRegistry;
pub use router::{RouteError, RouteStrategy, RouteStrategyHandle, Router};
pub use token_counter::{
    HeuristicTokenCounter, TokenBudget, TokenCount, TokenCountMethod, TokenCounter,
};
