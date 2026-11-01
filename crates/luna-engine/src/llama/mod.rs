//! 随包 `llama-server` 的运行时管理（ADR-143）：preset 生成、启动前内存预检、路由器监管。
//! Provider 侧（请求怎么发）在 `luna-provider` 的 `local_engine` 模块。

pub mod budget;
pub mod policy;
pub mod preset;
pub mod runtime;
pub mod supervisor;

pub use budget::{plan, FitParamsEstimator, FitRequest, FitVerdict};
pub use policy::Sentinel;
pub use preset::{render_preset, KvType, ModelRole, Pooling, PresetModel};
pub use supervisor::{
    EngineState, EngineStatus as LlamaEngineStatus, Failure, LaunchSpec, LlamaServerSupervisor,
};
