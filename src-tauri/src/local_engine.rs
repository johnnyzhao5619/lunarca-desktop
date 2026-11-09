//! 随包推理引擎的编排（ADR-143）：已装 GGUF → 启动前内存预检 → preset → 启动 / 停止路由器。
//!
//! 监管（拉起、探活、重启、熔断）在 `luna_engine::llama`；这里只回答「该不该跑、跑哪些、
//! 每个给多大上下文」。三个时机调用 [`reconcile`]：应用启动、GGUF 条目装完、GGUF 条目删掉。
//!
//! 内存预检是硬要求（P0-7：Metal 超额会让整机看门狗 panic，监管器根本来不及介入）：
//! 放不下的模型**不进 preset**，并如实报给界面，而不是让引擎去硬加载。

use std::path::PathBuf;

use luna_engine::llama::budget::{
    margin_mib, plan, Estimator, FitParamsEstimator, FitRequest, FitVerdict,
};
use luna_engine::llama::{render_preset, runtime, KvType, LaunchSpec, ModelRole, PresetModel};
use luna_model_catalog::GgufRole;
use tauri::Manager as _;

use crate::model_store::{DeviceTier, ModelEntry};

/// 档位决定的路由器参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TierLimits {
    /// 同时常驻的模型数（纯 LRU，没有嵌入豁免，P0-2）。
    pub models_max: u32,
    /// 空闲多久休眠（秒）：子进程保留、显存释放，首个请求自动唤醒。
    pub sleep_idle_seconds: u32,
    /// 对话模型的上下文上限（token）。
    pub chat_ctx: u32,
}

/// 档位 → 路由器参数（实施方案 §4.1：休眠 Low 5 min / Standard 15 min / High 30 min）。
#[must_use]
pub(crate) fn tier_limits(tier: DeviceTier) -> TierLimits {
    match tier {
        DeviceTier::Low => TierLimits {
            models_max: 1,
            sleep_idle_seconds: 300,
            chat_ctx: 8192,
        },
        // 16k 而不是 8k：蒸馏 / 主题生成按 16k 窗设计，而
        // 引擎的窗加载时就定死、超窗当场拒绝。Qwen3.5 是混合注意力，16k 的 KV 代价很小；
        // 真放不下时内存预检会先退 KV、再减窗，蒸馏随之按实际窗缩分片（`DistillBudget`）。
        DeviceTier::Standard => TierLimits {
            models_max: 2,
            sleep_idle_seconds: 900,
            chat_ctx: 16_384,
        },
        DeviceTier::High => TierLimits {
            models_max: 3,
            sleep_idle_seconds: 1800,
            chat_ctx: 16_384,
        },
    }
}

/// 对话模型的上下文下限：低于它不值得加载（对话 + 系统提示词放不下）。
const CHAT_MIN_CTX: u32 = 4096;
/// 嵌入模型的上下文下限：分块上限 1800 字符，2048 token 足够。
const EMBED_MIN_CTX: u32 = 2048;

/// 一个进了 preset 的模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedModel {
    pub id: String,
    pub name: String,
    pub preset: PresetModel,
    /// 走了退让档（KV 改 q8_0 或上下文被减）。
    pub degraded: bool,
}

/// 一个因内存没进 preset 的模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub id: String,
    pub name: String,
    /// `None` = 估不出来（工具缺失 / 输出格式变化），按拒绝处理。
    pub need_mib: Option<u64>,
    pub budget_mib: Option<u64>,
}

/// 一次编排的结论（界面读它说明「加载了哪些、为什么有的没加载」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EnginePlan {
    pub models: Vec<PlannedModel>,
    pub refused: Vec<Refusal>,
}

/// 引擎里一个已装模型的身份（Provider 装配据此把裸 id 目标认成「随包引擎条目」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineModelRef {
    /// 目录条目 / 导入条目 id，也是 preset 节名与请求里的 `model`。
    pub id: String,
    pub kind: EngineModelKind,
    /// 权重 sha256（安装 / 导入时记下的那个）。嵌入索引身份的 `runtime_digest` 取它：
    /// 换了权重（更新、换量化）就是换了向量空间。
    pub digest: Option<String>,
    /// 专用翻译模型（目录声明，Hy-MT2）：只进「翻译与实时档」，不当对话模型。
    pub translation_only: bool,
    /// 权重文件字节数（设备策略「挑最小的」用；读不到为 0，排在最前不冒充小——见调用处）。
    pub size_bytes: u64,
}

/// 引擎条目的用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineModelKind {
    Chat,
    Embed,
}

impl EngineCandidate {
    fn model_ref(&self) -> EngineModelRef {
        EngineModelRef {
            id: self.id.clone(),
            kind: match self.role {
                GgufRole::Chat { .. } => EngineModelKind::Chat,
                GgufRole::Embed { .. } => EngineModelKind::Embed,
            },
            digest: self.digest.clone(),
            translation_only: self.translation_only,
            size_bytes: std::fs::metadata(&self.path).map_or(0, |m| m.len()),
        }
    }
}

/// Provider 侧的「引擎现在在哪」（`luna_provider::LocalEngineSource`）：每次请求现问监管器。
pub(crate) struct SupervisorSource(pub luna_engine::llama::LlamaServerSupervisor);

impl luna_provider::LocalEngineSource for SupervisorSource {
    fn endpoint(&self) -> Option<luna_provider::LocalEngineEndpoint> {
        self.0
            .endpoint()
            .map(|(base_url, api_key)| luna_provider::LocalEngineEndpoint { base_url, api_key })
    }
}

/// 一个可进引擎的已装模型：目录条目与导入条目（ADR-144 ③）都转成它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineCandidate {
    pub id: String,
    pub name: String,
    pub role: GgufRole,
    /// 愿意给的最大上下文（目录条目取目录声明；导入条目取训练上下文与角色上限的较小者）。
    pub max_context: u32,
    pub path: PathBuf,
    /// 权重 sha256（manifest 里记的）。
    pub digest: Option<String>,
    /// 目录声明的专用翻译模型。
    pub translation_only: bool,
}

impl EngineCandidate {
    fn from_entry(entry: &ModelEntry, path: PathBuf) -> Option<Self> {
        let gguf = entry.gguf.as_ref()?;
        Some(Self {
            id: entry.id.clone(),
            name: entry.name.clone(),
            role: gguf.role,
            max_context: gguf.max_context,
            path,
            digest: None,
            translation_only: gguf.translation_only,
        })
    }

    fn from_imported(manifest: &crate::model_import::ImportedManifest, path: PathBuf) -> Self {
        let role = manifest.gguf_role();
        let cap: u32 = match role {
            GgufRole::Chat { .. } => 16_384,
            GgufRole::Embed { .. } => 8192,
        };
        Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            role,
            max_context: u32::try_from(manifest.context_length)
                .unwrap_or(u32::MAX)
                .min(cap),
            path,
            digest: Some(manifest.sha256.clone()),
            translation_only: false,
        }
    }
}

/// 纯决策：已装的 GGUF 条目 + 档位 + 可用内存 → 哪些进 preset、各自参数。
///
/// 顺序 = 预算优先级：对话模型先于嵌入（嵌入模型小，放不下的多半是对话）；同类按目录顺序。
pub(crate) fn plan_engine(
    installed: &[EngineCandidate],
    tier: DeviceTier,
    available_mib: u64,
    physical_mib: u64,
    estimator: &dyn Estimator,
) -> EnginePlan {
    let limits = tier_limits(tier);
    let mut ordered: Vec<&EngineCandidate> = installed.iter().collect();
    ordered.sort_by_key(|c| matches!(c.role, GgufRole::Embed { .. }));
    let requests: Vec<FitRequest> = ordered
        .iter()
        .map(|c| {
            let (ctx, min_ctx) = match c.role {
                GgufRole::Chat { .. } => (c.max_context.min(limits.chat_ctx), CHAT_MIN_CTX),
                GgufRole::Embed { .. } => (c.max_context, EMBED_MIN_CTX),
            };
            FitRequest {
                id: c.id.clone(),
                ctx,
                min_ctx,
            }
        })
        .collect();
    let verdicts = plan(
        available_mib,
        margin_mib(physical_mib),
        &requests,
        estimator,
    );

    let mut out = EnginePlan::default();
    for (c, verdict) in ordered.into_iter().zip(verdicts) {
        match verdict {
            FitVerdict::Fits {
                ctx, kv, degraded, ..
            } => out.models.push(PlannedModel {
                id: c.id.clone(),
                name: c.name.clone(),
                preset: PresetModel {
                    id: c.id.clone(),
                    path: c.path.clone(),
                    role: ModelRole::from(c.role),
                    ctx,
                    kv,
                },
                degraded,
            }),
            FitVerdict::Refused {
                need_mib,
                budget_mib,
            } => out.refused.push(Refusal {
                id: c.id.clone(),
                name: c.name.clone(),
                need_mib: Some(need_mib),
                budget_mib: Some(budget_mib),
            }),
            FitVerdict::Unknown(_) => out.refused.push(Refusal {
                id: c.id.clone(),
                name: c.name.clone(),
                need_mib: None,
                budget_mib: None,
            }),
        }
    }
    out
}

/// 已装、可进引擎的模型：按**磁盘上实际装了什么**查——目录条目（模型根下每个带 `weights.gguf`
/// 的目录按 id 查目录）加导入条目。不从可下载列表出发，否则远程目录撤下一条时，已装的模型会从
/// 引擎里消失（ADR-145 ⑤）。按 id 排序，编排结论稳定。
fn installed_candidates(models_root: &std::path::Path) -> Vec<EngineCandidate> {
    let Ok(dirs) = std::fs::read_dir(models_root) else {
        return Vec::new();
    };
    let mut found: Vec<EngineCandidate> = dirs
        .flatten()
        .filter_map(|dir| {
            let id = dir.file_name().to_string_lossy().into_owned();
            let entry = crate::model_store::remote_model_spec(&id).filter(|e| e.gguf.is_some())?;
            let path = crate::model_store::spec_weight_path(models_root, entry);
            path.is_file()
                .then(|| EngineCandidate::from_entry(entry, path))
                .flatten()
                .map(|candidate| EngineCandidate {
                    digest: crate::model_store::installed_sha256(models_root, entry),
                    ..candidate
                })
        })
        .collect();
    if crate::model_store::engine_build().is_some() {
        found.extend(crate::model_import::imported(models_root).iter().map(|m| {
            EngineCandidate::from_imported(m, crate::model_import::weights_path(models_root, &m.id))
        }));
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// 按当前已装模型与内存状况，让引擎处于该处的状态（幂等）：没有可跑的模型就停；
/// 有就按预检结论生成 preset 启动；参数与在跑的一致则不动（重启会丢掉已加载的模型）。
/// `force_restart`：权重文件换了而 preset 文本没变（模型更新）时，必须重启才会加载新权重。
pub(crate) async fn reconcile(app: &tauri::AppHandle, force_restart: bool) {
    let state = app.state::<crate::ipc::AppState>();
    let mut plan_slot = state.local_engine_plan().lock().await;
    let supervisor = state.local_engine().clone();

    let runtime = app
        .path()
        .resource_dir()
        .ok()
        .and_then(|root| runtime::locate(&root));
    let models_root = state.models_root(app);
    let installed = installed_candidates(&models_root);
    // 已装清单变了 → 槽位里的裸 id 可能刚从「不认识」变成「随包引擎条目」（或反过来），
    // 按新清单重装配 Provider；清单没变不动。
    if state.record_engine_models(installed.iter().map(EngineCandidate::model_ref).collect()) {
        crate::cloud::reconfigure_cloud_stack(app, &state);
    }
    let (Some(runtime), false) = (runtime, installed.is_empty()) else {
        supervisor.stop().await;
        *plan_slot = EnginePlan::default();
        emit_changed(app);
        return;
    };
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };

    let tier = crate::hardware::probe_tier();
    let fit_params = runtime.fit_params.clone();
    let computed = tauri::async_runtime::spawn_blocking(move || {
        let mut estimator = FitParamsEstimator::new(fit_params);
        for c in &installed {
            let full_batch = matches!(c.role, GgufRole::Embed { encoder: true, .. });
            estimator = estimator.with_model(c.id.clone(), c.path.clone(), full_batch);
        }
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let total_mib = sys.total_memory() / (1024 * 1024);
        let available = available_mib(total_mib, sys.available_memory() / (1024 * 1024));
        plan_engine(&installed, tier, available, total_mib, &estimator)
    })
    .await
    .unwrap_or_default();

    if computed.models.is_empty() {
        supervisor.stop().await;
    } else {
        let limits = tier_limits(tier);
        let presets: Vec<PresetModel> = computed.models.iter().map(|m| m.preset.clone()).collect();
        match render_preset(limits.sleep_idle_seconds, &presets) {
            Ok(preset_ini) => {
                let dir = data_dir.join("llama");
                let spec = LaunchSpec {
                    server_bin: runtime.server,
                    preset_ini,
                    preset_path: dir.join("preset.ini"),
                    pid_path: dir.join("router.pid"),
                    models_max: limits
                        .models_max
                        .min(u32::try_from(presets.len()).unwrap_or(u32::MAX)),
                };
                let running = !matches!(
                    supervisor.status().state,
                    luna_engine::llama::EngineState::Stopped
                        | luna_engine::llama::EngineState::Tripped
                );
                if force_restart || !(running && supervisor.current_spec().as_ref() == Some(&spec))
                {
                    supervisor.start(spec).await;
                }
            }
            Err(error) => {
                // 只有编程错误会走到这里（id / 路径非法）；宁可不起，也不带着坏 preset 起。
                eprintln!("[local-engine] preset 生成失败：{error}");
                supervisor.stop().await;
            }
        }
    }
    *plan_slot = computed;
    drop(plan_slot);
    emit_changed(app);
    // 加载给对话模型的上下文可能变了（换了模型、内存退让）：对话的上下文预算跟着重问。
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::ipc::refresh_chat_context_window(&app).await;
    });
}

/// 「还剩多少内存」（MiB）。
///
/// macOS 上**不用 sysinfo 的 `available_memory`**：它的公式是「空闲 + 非活跃 + 可清除 − 压缩器占用」，
/// 而压缩器占用本来就不在前三项里，等于重复扣除——内存压力越大越被低估。真机实测（2026-10-07，
/// 16 GB）：压缩器 4.6 GB 时它算出 −177 MiB（截成 0），把 1.9 GB 的翻译模型判成放不下，
/// 而系统自己报 53% 可用。改用内核的 `kern.memorystatus_level`（可用百分比，活动监视器的
/// 「内存压力」即据此）× 物理内存；读不到时才退回 sysinfo。
fn available_mib(total_mib: u64, sysinfo_available_mib: u64) -> u64 {
    memorystatus_level()
        .map(|percent| level_to_mib(percent, total_mib))
        .unwrap_or(sysinfo_available_mib)
}

fn level_to_mib(percent: u64, total_mib: u64) -> u64 {
    total_mib * percent.min(100) / 100
}

#[cfg(target_os = "macos")]
fn memorystatus_level() -> Option<u64> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.memorystatus_level"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn memorystatus_level() -> Option<u64> {
    None
}

/// 启动：首轮编排 → 记「引擎就绪」里程碑 → 通知设置状态（「本机文本模型就绪」的答案
/// 随已装清单而定，PRD-14-063）→ 核查默认对话模型。
pub(crate) fn spawn_startup(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        reconcile(&app, false).await;
        app.state::<crate::startup::StartupTimeline>()
            .mark(crate::startup::StartupMilestone::EngineReady);
        {
            use tauri_specta::Event as _;
            let _ = crate::ipc::SetupStatusChanged {}.emit(&app);
        }
        crate::check_llm_readiness(&app).await;
    });
}

/// 后台跑一次 [`reconcile`]（启动、删除之后调用，不阻塞命令返回）。
pub(crate) fn spawn_reconcile(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { reconcile(&app, false).await });
}

/// GGUF 装完 / 更新完：强制重启（更新时 preset 不变而文件变了；新装时 preset 本来就变了）。
pub(crate) fn spawn_reconcile_restart(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { reconcile(&app, true).await });
}

/// 把监管器的状态变化转成前端事件（应用启动时挂一次）。
pub(crate) fn forward_status_changes(app: &tauri::AppHandle) {
    let mut rx = app
        .state::<crate::ipc::AppState>()
        .local_engine()
        .subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while rx.changed().await.is_ok() {
            emit_changed(&app);
        }
    });
}

fn emit_changed(app: &tauri::AppHandle) {
    use tauri_specta::Event as _;
    let _ = crate::ipc::LocalEngineStatusChanged {}.emit(app);
}

/// 预检结论里 KV 的展示写法。
pub(crate) fn kv_label(kv: KvType) -> &'static str {
    kv.as_arg()
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_engine::llama::budget::Projection;

    /// 线性假估算：每个模型 base + ctx 部分；q8_0 时 ctx 部分减半。
    struct Fake;
    impl Estimator for Fake {
        fn estimate(&self, id: &str, ctx: u32, kv: KvType) -> Result<Projection, String> {
            let base = if id.contains("4b") {
                3000
            } else if id.contains("bge") {
                700
            } else {
                1900
            };
            let ctx_part = u64::from(ctx) / 16;
            let ctx_part = if kv == KvType::Q8_0 {
                ctx_part / 2
            } else {
                ctx_part
            };
            Ok(Projection {
                total_mib: base + ctx_part,
            })
        }
    }

    fn entry(id: &str) -> EngineCandidate {
        EngineCandidate::from_entry(
            luna_model_catalog::find(id).unwrap(),
            PathBuf::from(format!("/m/{id}/weights.gguf")),
        )
        .unwrap()
    }

    /// 导入条目的上下文取训练上下文与角色上限的较小者；对话一律按思考模型关思考。
    #[test]
    fn imported_models_become_candidates_with_capped_context() {
        use crate::model_import::{ImportOrigin, ImportedManifest, ImportedRole};
        let manifest = |role, ctx| ImportedManifest {
            kind: "imported".into(),
            id: "gguf-x".into(),
            name: "X".into(),
            origin: ImportOrigin::LocalFile {
                file_name: "x.gguf".into(),
            },
            sha256: "0".repeat(64),
            size_bytes: 1,
            architecture: "qwen35".into(),
            context_length: ctx,
            role,
            license: None,
        };
        let chat = EngineCandidate::from_imported(
            &manifest(ImportedRole::Chat, 262_144),
            PathBuf::from("/m/x"),
        );
        assert_eq!(chat.max_context, 16_384);
        assert_eq!(chat.role, GgufRole::Chat { thinking: true });
        let small = EngineCandidate::from_imported(
            &manifest(ImportedRole::Chat, 2048),
            PathBuf::from("/m/x"),
        );
        assert_eq!(
            small.max_context, 2048,
            "never more than the model was trained for"
        );
    }

    #[test]
    fn high_tier_with_room_loads_everything_at_full_settings_chat_first() {
        let installed = [
            entry("bge-m3-q8_0-gguf"),
            entry("qwen3.5-4b-q4_k_m-gguf"),
            entry("hy-mt2-1.8b-q8_0-gguf"),
        ];
        let p = plan_engine(&installed, DeviceTier::High, 12_000, 16_384, &Fake);
        let ids: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "qwen3.5-4b-q4_k_m-gguf",
                "hy-mt2-1.8b-q8_0-gguf",
                "bge-m3-q8_0-gguf"
            ]
        );
        assert!(p.refused.is_empty());
        let qwen = &p.models[0].preset;
        assert_eq!(qwen.ctx, 16_384);
        assert_eq!(qwen.role, ModelRole::Chat { thinking: true });
        assert_eq!(
            p.models[1].preset.ctx, 8192,
            "hy-mt2 capped by its catalog max_context"
        );
        assert!(matches!(
            p.models[2].preset.role,
            ModelRole::Embed { encoder: true, .. }
        ));
    }

    /// 档位封顶：Low 8k（连模型本身都要精打细算），Standard / High 16k（蒸馏按 16k 设计）。
    #[test]
    fn tiers_cap_chat_context() {
        let ctx = |tier| {
            plan_engine(
                &[entry("qwen3.5-2b-q8_0-gguf")],
                tier,
                12_000,
                16_384,
                &Fake,
            )
            .models[0]
                .preset
                .ctx
        };
        assert_eq!(ctx(DeviceTier::Low), 8192);
        assert_eq!(ctx(DeviceTier::Standard), 16_384);
        assert_eq!(ctx(DeviceTier::High), 16_384);
    }

    /// P0-7 的教训落到编排层：放不下的模型不进 preset，并带着需要 / 可用的数字报出来。
    #[test]
    fn a_model_that_does_not_fit_is_left_out_not_forced() {
        let installed = [entry("qwen3.5-4b-q4_k_m-gguf"), entry("bge-m3-q8_0-gguf")];
        // 物理 16 GB → 余量 2457；可用 5200 → 预算 2743：4B 下限 q8 也要 3128，放不下；bge 放得下
        let p = plan_engine(&installed, DeviceTier::High, 5200, 16_384, &Fake);
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].id, "bge-m3-q8_0-gguf");
        assert_eq!(p.refused.len(), 1);
        assert_eq!(p.refused[0].id, "qwen3.5-4b-q4_k_m-gguf");
        assert!(p.refused[0].need_mib.unwrap() > p.refused[0].budget_mib.unwrap());
    }

    #[test]
    fn an_estimator_failure_refuses_rather_than_guessing() {
        struct Broken;
        impl Estimator for Broken {
            fn estimate(&self, _: &str, _: u32, _: KvType) -> Result<Projection, String> {
                Err("missing".into())
            }
        }
        let p = plan_engine(
            &[entry("hy-mt2-1.8b-q8_0-gguf")],
            DeviceTier::High,
            64_000,
            65_536,
            &Broken,
        );
        assert!(p.models.is_empty());
        assert_eq!(p.refused[0].need_mib, None);
    }

    /// 启动钩子必须挂在 `app.manage(state)` 之后：它们经 `app.state::<AppState>()` 取监管器，
    /// 放在前面会在 setup 里 panic——setup 不能展开，应用启动即 abort。真机踩过一次
    /// （2026-10-07），headless 闸门全绿也抓不到，故用源码顺序钉住。
    #[test]
    fn startup_hooks_run_after_app_state_is_managed() {
        let lib = include_str!("lib.rs");
        let managed = lib
            .find("app.manage(state);")
            .expect("lib.rs manages AppState");
        for hook in [
            "local_engine::forward_status_changes(",
            "local_engine::spawn_startup(",
            "ipc::open_startup_vault_when_local(",
        ] {
            let at = lib
                .find(hook)
                .unwrap_or_else(|| panic!("{hook} not wired in lib.rs"));
            assert!(
                at > managed,
                "{hook} runs before app.manage(state) — setup would abort"
            );
        }
    }

    #[test]
    fn availability_comes_from_the_kernel_level_not_a_double_counted_formula() {
        // 真机那一刻：16 GB、内核报 53% 可用；sysinfo 公式给 0
        assert_eq!(level_to_mib(53, 16_384), 8683);
        assert_eq!(level_to_mib(150, 16_384), 16_384, "clamped");
        // 同一刻：1.9 GB 翻译模型（估 2.44 GiB）放得下，P0 事故那组（~14 GB）放不下
        let budget = level_to_mib(53, 16_384) - margin_mib(16_384);
        assert!(budget > 2500 && budget < 13_992);
        #[cfg(target_os = "macos")]
        assert!(
            memorystatus_level().is_some_and(|p| p <= 100),
            "sysctl readable on this host"
        );
    }

    #[test]
    fn tiers_scale_residency_and_sleep() {
        assert_eq!(tier_limits(DeviceTier::Low).models_max, 1);
        assert_eq!(tier_limits(DeviceTier::High).models_max, 3);
        assert!(
            tier_limits(DeviceTier::Low).sleep_idle_seconds
                < tier_limits(DeviceTier::High).sleep_idle_seconds
        );
    }
}
