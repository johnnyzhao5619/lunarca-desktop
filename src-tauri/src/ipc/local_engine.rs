//! 本地推理引擎（随包 `llama-server`，ADR-143）的状态与手动重启。
//!
//! 监管本身在 `luna_engine::llama`；这里只把状态投影成界面能读的形状，并给熔断后的
//! 「重启」一个入口。何时启动、装哪些模型由 `crate::local_engine` 的编排决定，本域不做决策。
//!
//! 跨模块可见性：命令与 DTO `pub(crate)`，经 `mod.rs` 的 `use local_engine::*` 回到
//! `collect_commands!` 作用域。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;

use super::AppState;
use luna_engine::llama::{runtime, EngineState, Failure, Sentinel};

/// 引擎所处的状态（界面文案按它分支）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalEngineState {
    /// 没在跑：还没有需要它的本地模型，或已停止。
    Stopped,
    /// 正在拉起。
    Starting,
    /// 就绪。
    Ready,
    /// 意外退出，正在按退避重启。
    Restarting,
    /// 连续失败已熔断，等用户手动重启。
    Tripped,
}

/// 最近一次故障的类别（只有类别，没有日志原文——日志可能含请求片段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalEngineFailure {
    /// 拉不起来（文件缺失 / 权限）。
    Spawn,
    /// 规定时间内没有就绪。
    StartTimeout,
    /// 进程退出。
    Exited,
    /// 进程挂起、健康探针不通。
    Unresponsive,
    /// 计算后端（Metal）报错。
    ComputeError,
    /// 模型文件损坏或不完整。
    CorruptModel,
    /// 参数被引擎拒绝。
    BadArguments,
}

/// 「本地推理引擎状态可能变了」（`local-engine://changed`）：监管器状态变化、编排完成时发；
/// 前端收到后重取 [`local_engine_status`]。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "local-engine://changed")]
pub struct LocalEngineStatusChanged {}

/// 一个进了引擎的模型。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct LocalEngineModelDto {
    /// 目录条目 id。
    pub model_id: String,
    /// 展示名。
    pub name: String,
    /// 定下的上下文（token）。
    pub context_tokens: u32,
    /// KV 类型（`f16` / `q8_0`）。
    pub kv_type: String,
    /// 走了退让档：内存不够全量，KV 改了 q8_0 或上下文被减。
    pub degraded: bool,
}

/// 一个因内存没进引擎的模型（启动前预检拒绝，ADR-143 ④）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct LocalEngineRefusalDto {
    /// 目录条目 id。
    pub model_id: String,
    /// 展示名。
    pub name: String,
    /// 最小参数下仍需要的内存（字节）；估不出来时为空。以 f64 表达供 JS number 消费。
    pub need_bytes: Option<f64>,
    /// 当时可给的额度（字节，已扣余量与先排定的模型）；估不出来时为空。
    pub budget_bytes: Option<f64>,
}

/// 本地推理引擎状态。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct LocalEngineStatusDto {
    /// 本平台是否随包了引擎、且文件齐全。`false` 时其余字段无意义。
    pub runtime_present: bool,
    /// 本构建是否启用 GGUF 模型（目录条目与导入入口）；关着时界面不露出本地推理相关的一切。
    pub gguf_enabled: bool,
    /// 引擎构建号（如 `b11429`）。
    pub engine_build: Option<String>,
    /// 状态。
    pub state: LocalEngineState,
    /// 重启中时：已连续失败次数。
    pub failures: u32,
    /// 最近一次故障类别。
    pub last_failure: Option<LocalEngineFailure>,
    /// 进了引擎的模型（按加载优先级）。
    pub models: Vec<LocalEngineModelDto>,
    /// 因内存没进引擎的模型。
    pub refused: Vec<LocalEngineRefusalDto>,
}

pub(crate) async fn status_dto(app: &tauri::AppHandle, state: &AppState) -> LocalEngineStatusDto {
    let plan = state.local_engine_plan().lock().await.clone();
    let mib = |v: u64| (v * 1024 * 1024) as f64;
    let runtime = app
        .path()
        .resource_dir()
        .ok()
        .and_then(|root| runtime::locate(&root));
    let status = state.local_engine().status();
    let (state_kind, failures) = match status.state {
        EngineState::Stopped => (LocalEngineState::Stopped, 0),
        EngineState::Starting => (LocalEngineState::Starting, 0),
        EngineState::Ready { .. } => (LocalEngineState::Ready, 0),
        EngineState::Restarting { failures, .. } => (LocalEngineState::Restarting, failures),
        EngineState::Tripped => (LocalEngineState::Tripped, 0),
    };
    LocalEngineStatusDto {
        gguf_enabled: runtime.is_some() && crate::model_store::engine_build().is_some(),
        runtime_present: runtime.is_some(),
        engine_build: runtime.map(|r| r.build.to_owned()),
        state: state_kind,
        failures,
        last_failure: status.last_failure.map(|failure| match failure {
            Failure::Spawn(_) => LocalEngineFailure::Spawn,
            Failure::StartTimeout => LocalEngineFailure::StartTimeout,
            Failure::Exited(_) => LocalEngineFailure::Exited,
            Failure::Unresponsive => LocalEngineFailure::Unresponsive,
            Failure::Sentinel(Sentinel::ComputeError) => LocalEngineFailure::ComputeError,
            Failure::Sentinel(Sentinel::CorruptModel) => LocalEngineFailure::CorruptModel,
            Failure::Sentinel(Sentinel::BadArguments) => LocalEngineFailure::BadArguments,
        }),
        models: plan
            .models
            .into_iter()
            .map(|m| LocalEngineModelDto {
                model_id: m.id,
                name: m.name,
                context_tokens: m.preset.ctx,
                kv_type: crate::local_engine::kv_label(m.preset.kv).to_owned(),
                degraded: m.degraded,
            })
            .collect(),
        refused: plan
            .refused
            .into_iter()
            .map(|r| LocalEngineRefusalDto {
                model_id: r.id,
                name: r.name,
                need_bytes: r.need_mib.map(mib),
                budget_bytes: r.budget_mib.map(mib),
            })
            .collect(),
    }
}

/// 本地推理引擎状态（本机进程查询，不联网）。
///
/// # Errors
/// 本命令不失败；返回类型保持 `Result` 以与其余命令一致。
#[tauri::command]
#[specta::specta]
pub(crate) async fn local_engine_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<LocalEngineStatusDto, String> {
    Ok(status_dto(&app, &state).await)
}

/// 手动重启本地推理引擎：熔断后的出路，重启次数清零。从未启动过时什么也不做
/// （没有可用的启动参数——那时该做的是装一个本地模型，而不是重启）。
///
/// 只重启、不重新预检：内存状况变了（用户关掉了别的应用）想让被拒的模型进来，
/// 走的是重新编排而不是这里——那条路在装 / 删模型与应用启动时自动走。
///
/// # Errors
/// 本命令不失败；返回类型保持 `Result` 以与其余命令一致。
#[tauri::command]
#[specta::specta]
pub(crate) async fn local_engine_restart(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<LocalEngineStatusDto, String> {
    state.local_engine().restart().await;
    Ok(status_dto(&app, &state).await)
}
