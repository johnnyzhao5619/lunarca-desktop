//! 语义检索底座（PRD-16 §3.6 /
//! ADR-027）：建库、查询、内存矩阵。
//!
//! 分工与既有域一致：**算法在 `luna_vault`（`chunk` 分块 / `semantic` 余弦与 RRF，纯函数
//! 可无头单测）**，本模块只做编排——读设置、走 AI 主干拿向量、落派生索引、持内存矩阵。
//!
//! # 三条本模块特有的纪律
//!
//! 1. **内存矩阵必须能被整体作废**（ADR-025 的本批次考点）：切库、换模型、建库写入之后
//!    都要作废。矩阵是「派生数据的派生数据」，任何一层变了它就是陈旧的——而陈旧的矩阵不会
//!    报错，只会安静地按上一个库的内容排序。故它藏在 [`invalidate_matrix`] 一个入口后面，
//!    调用点逐一登记在切库交接表（TDD-06 §12）。
//! 2. **缺模型是诚实降级，不是失败**（PRD-16-029）：没装嵌入模型 / 用户关掉语义 /
//!    矩阵还空着，`semantic_hits` 一律返回空，检索退回纯关键词且**与批次 1 逐字节一致**。
//!    绝不把一个 404 当成"库里没有相关内容"。
//! 3. **建库耗时是十分钟级的**（S16 实测万片约 40 分钟），故全量建库照搬 graph.rs 全库重扫
//!    那套六要素骨架（预检盘点 → 确认卡给数字 → 单例后台逐篇 → 进度事件 + 快照 →
//!    连败中止 → 终局通知），而不是一个转圈等的模态。
//!
//! 脱敏（TDD-12）：日志只出路径与计数。**向量与笔记正文同级**——嵌入反演已是公开结果，
//! 把向量当"一串无意义的数字"打进日志与打印正文没有本质区别。

use super::*;
use luna_core::{Job, JobEvent, JobOrigin, Priority, ResultPayload};
use luna_vault::{SemanticHit, VectorMatrix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 一个 Job 一批多少段（S16 实测：批 8 即到吞吐平台，故取 16——吞吐无损，而单请求更短
/// 意味着取消更灵敏、进度更细）。
///
/// ⚠️ **这个数是本机口径**：S16 是对着本机运行时测的，那里一次往返的固定
/// 开销可以忽略。走服务端时每 16 片要付一次网络往返（1 万片 = 625 次），而服务端单批
/// 上限是 512——即落点在服务端时这个值几乎肯定偏小。**不据此就改数**：没有对着真实
/// 服务端测过之前调它，只是把一个有实测背书的值换成一个拍的值，而更大的批同时会让
/// 取消变迟钝、进度变粗、并更容易撞上那条 120 秒超时。要改先测。
const EMBED_BATCH: usize = 16;
/// 语义路一次取回多少个分片命中。取 `max_notes` 的数倍：同一篇的多个分片会被折叠成一条，
/// 故要多取一些才能凑够"若干篇不同的笔记"。
// 嵌入 profile 表与向量契约住在 luna-vault（与 `luna bench-embed` 同一份，PRD-10-017）。
pub(crate) use luna_vault::{
    embedding_profile, engine_entry_profile_key, normalize_embedding_vectors, profile_texts,
    EmbeddingProfile, VectorContract,
};

const SEMANTIC_HIT_LIMIT: usize = 40;
/// 连续失败即中止（与 graph.rs 重扫同一阈值与理由：引擎不可用时磨完全库只是浪费）。
const HALT_AFTER_CONSECUTIVE_FAILURES: u32 = 3;

/// 模型槽只接收完成 Lunarca Profile 与分块基准的目标。“运行时能 embed”不足以进入生产索引。
pub(crate) fn assignment_target_supported(target_id: &str) -> bool {
    use crate::model_target::ModelTarget;
    match ModelTarget::parse(target_id) {
        // profile 是**模型的属性，不是落点的属性**：同一个 `bge-m3` 的向量维度与分块
        // 基准与它跑在哪儿无关，故两种落点查同一张表。
        //
        // ⚠️ **但这不表示索引身份也跨落点相同**——恰恰相反，`resolve_embedding_identity`
        // 给服务端那一档的身份是 `server:<分桶>:<模型>`，与本机的权重摘要必然不同，
        // **换落点会触发全库重建**，理由见那个函数的头注释（我们无法证明服务端跑的是同一版
        // 权重，赌错的代价是两个向量空间被混进同一份索引）。此处只判「这个模型能不能进
        // 生产索引」，不判「已建的索引还能不能用」。
        Some(ModelTarget::Service(name)) => {
            embedding_profile(name).is_some_and(|profile| profile.chunk_policy.is_some())
        }
        // 裸模型库条目：随包引擎的嵌入条目映射回同一张 profile 表（ADR-143）；
        // 其余裸 id（`whisper-*`、导入条目）不是经审核的嵌入模型。
        Some(ModelTarget::StoreEntry(id)) => engine_entry_profile_key(id)
            .and_then(embedding_profile)
            .is_some_and(|profile| profile.chunk_policy.is_some()),
        None => false,
    }
}

/// 嵌入槽此刻指向的随包引擎条目——**只有引擎清单里登记为嵌入模型时才算**。
/// 指向一个还没装（或装的是对话模型）的条目 id 时为 `None`，就绪判定据此报「未安装」。
fn embed_engine_entry(state: &AppState) -> Option<crate::local_engine::EngineModelRef> {
    use crate::model_target::ModelTarget;
    let target = effective_assignment_target(&state.settings_snapshot(), "default_embedding")?;
    let ModelTarget::StoreEntry(id) = ModelTarget::parse(&target)? else {
        return None;
    };
    state
        .engine_models_snapshot()
        .into_iter()
        .find(|m| m.id == id && m.kind == crate::local_engine::EngineModelKind::Embed)
}

/// 嵌入槽是否指向一个随包引擎条目 id（不论装没装）。
fn embed_targets_engine(state: &AppState) -> bool {
    use crate::model_target::ModelTarget;
    effective_assignment_target(&state.settings_snapshot(), "default_embedding")
        .as_deref()
        .and_then(ModelTarget::parse)
        .is_some_and(|target| matches!(target, ModelTarget::StoreEntry(id) if engine_entry_profile_key(id).is_some()))
}

/// 嵌入能力此刻的就绪结论。**稳定 code**：IPC 原样透出，前端按 code 取文案，不解析人话。
///
/// 五种「用不了」必须各说各的，绝不并成一个 `available: false`：它们的下一步动作完全
/// 不同（去分配 / 去下载模型 / 把引擎跑起来 / 等基准通过），而把「引擎没跑」说成
/// 「模型没装」会引导用户去重装一个已经装好的模型——这正是既有验收轮修过的一处谎报。
pub(crate) enum EmbeddingReadiness {
    Ready(Box<luna_vault::EmbeddingIndexIdentity>),
    /// 没有解析出嵌入目标（槽位空且无设备策略）。
    NoModel,
    /// 目标不在 Lunarca 审核过的 profile 表里。
    UnsupportedModel,
    /// 有 profile 但分块基准未通过（Qwen3-Embedding 三档当前如此），不得进生产索引。
    ProfileUnverified,
    /// 运行时可达但目录里没有这个模型（或未报告权重摘要）。
    NotInstalled,
    /// 嵌入由服务端托管，而注册表里此刻没有服务端嵌入 Provider（装配还没跟上）。**不是**
    /// 模型没装：把它报成 `NotInstalled` 会把用户支去下载一个本就该在服务端跑的模型。
    ServerUnavailable,
}

impl EmbeddingReadiness {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Ready(_) => "ready",
            Self::NoModel => "no_model",
            Self::UnsupportedModel => "unsupported_model",
            Self::ProfileUnverified => "profile_unverified",
            Self::NotInstalled => "not_installed",
            Self::ServerUnavailable => "server_unavailable",
        }
    }
}

/// 嵌入此刻在服务端跑吗。
///
/// **问注册表，不重算一遍落点。** 装配已经做过这个判断（[`refresh_embed_provider`]），
/// 再算一次就是第二处判据；而这两处一旦分叉，**索引身份就会与真正产出向量的那一方对不上**
/// ——两个向量空间被写进同一份索引，余弦从此没有意义，而症状只是"搜得不太准"。
fn embeds_on_server(state: &AppState) -> bool {
    state
        .provider_registry
        .get(&luna_core::ProviderId::from(
            luna_provider::CLOUD_EMBED_PROVIDER_ID,
        ))
        .is_some()
}

/// 构造完整向量空间身份并缓存，并如实说明拿不到时是**哪一种**拿不到。
/// 摘要缺失时宁可不可用，也不以模型名冒充权重身份。
///
/// # 服务端那一档不问本机引擎
///
/// `runtime_digest` 的来源随落点分两条。此前只有本机那条：向本机引擎要权重摘要。落点在
/// 服务端时本机**根本没有那个权重**，于是这里必然回 `NotInstalled`
/// ——`prepare_embedding_identity` 拿不到身份，索引写入整条不执行，语义检索静默退回纯
/// 关键词。表现是：设置页写着「AI 由服务端提供」，而语义索引是唯一**完全不生效**的那一项，
/// 且不报任何错。服务端 `embed` 接入若不同批改这里，等于没接。
///
/// 服务端那一档的身份取 `server:<服务端分桶>:<模型>`：
/// - **前缀 `server:` 是刻意的**——它不是权重哈希，不能让下一个人以为它是。服务端不下发
///   权重摘要（`/inference/v1/models` 只有标识/展示名/能力/窗口），假装有一个才是错的。
/// - 因此**换落点会让索引身份变化，触发全库重建**。这不是遗憾，是唯一诚实的选择：我们
///   无法证明服务端跑的是与本机同一版权重，而赌错的代价是两个向量空间被混进同一份索引
///   ——一个不报错、也几乎无法归因的检索质量下降。`EmbeddingIndexIdentity` 存在的全部
///   理由就是不让这件事发生。
pub(crate) async fn resolve_embedding_identity(state: &AppState) -> EmbeddingReadiness {
    let Some(model) = embed_model(state) else {
        return EmbeddingReadiness::NoModel;
    };
    let Some(profile) = embedding_profile(&model) else {
        return EmbeddingReadiness::UnsupportedModel;
    };
    let Some(policy) = profile.chunk_policy else {
        return EmbeddingReadiness::ProfileUnverified;
    };
    let runtime_digest = if embeds_on_server(state) {
        // 地址取云客户端此刻指向的那一台：`embeds_on_server` 只在 `is_server()` 那两档
        // 为真，而那两档的 `ComputeEndpoint::url()` 恒等于它。故这不是第二处地址派生。
        let api_base = state.cloud_server().origin;
        format!(
            "server:{}:{model}",
            luna_cloud::config::server_bucket(&api_base)
        )
    } else if embed_targets_engine(state) {
        // 随包引擎：权重摘要取安装 / 导入时记下的 sha256，不必问引擎（它甚至可以还没起）。
        // 没在引擎清单里 = 没装；装了但 Provider 不在注册表 = 装配没跟上，同样不可用。
        let Some(entry) = embed_engine_entry(state) else {
            return EmbeddingReadiness::NotInstalled;
        };
        if state
            .provider_registry
            .get(&luna_core::ProviderId::from(
                luna_provider::LOCAL_EMBED_PROVIDER_ID,
            ))
            .is_none()
        {
            return EmbeddingReadiness::NotInstalled;
        }
        let Some(digest) = entry.digest.filter(|d| !d.trim().is_empty()) else {
            return EmbeddingReadiness::NotInstalled;
        };
        let Ok(dimension) = u32::try_from(profile.native_dimension) else {
            return EmbeddingReadiness::UnsupportedModel;
        };
        // 身份的 `target_id` 是**条目 id**而不是 profile 的身份串：同一模型换了量化或来源就
        // 换身份，全库重建一次，这是对的。
        return remember_identity(
            state,
            luna_vault::EmbeddingIndexIdentity {
                target_id: entry.id,
                runtime_digest: format!("sha256:{digest}"),
                dimension,
                profile_version: profile.profile_version,
                normalization: "l2".to_owned(),
                chunk_policy_id: policy.id.to_owned(),
            },
        );
    } else {
        // 既不在服务端、也不是随包引擎条目：只剩「服务端托管而装配还没装上」这一种（模型
        // 缺失 / 审核不过已在上方各自返回）。**以注册表为准**，不越过装配直接问引擎——
        // 同一事实两处判定必然分叉（真机第八轮）。
        return EmbeddingReadiness::ServerUnavailable;
    };
    let Ok(dimension) = u32::try_from(profile.native_dimension) else {
        return EmbeddingReadiness::UnsupportedModel;
    };
    remember_identity(
        state,
        luna_vault::EmbeddingIndexIdentity {
            target_id: profile.target_id.to_owned(),
            runtime_digest,
            dimension,
            profile_version: profile.profile_version,
            normalization: "l2".to_owned(),
            chunk_policy_id: policy.id.to_owned(),
        },
    )
}

/// 记下已核实的身份；变了就作废内存矩阵（旧矩阵属于另一个向量空间）。
fn remember_identity(
    state: &AppState,
    identity: luna_vault::EmbeddingIndexIdentity,
) -> EmbeddingReadiness {
    if let Ok(mut guard) = state.embed_index_identity.lock() {
        if guard.as_ref() != Some(&identity) {
            *guard = Some(identity.clone());
            invalidate_matrix(state);
        }
    }
    EmbeddingReadiness::Ready(Box::new(identity))
}

/// 只要身份、不问原因的调用点用它（索引写入路径：拿不到就是拿不到）。
pub(crate) async fn prepare_embedding_identity(
    state: &AppState,
) -> Option<luna_vault::EmbeddingIndexIdentity> {
    match resolve_embedding_identity(state).await {
        EmbeddingReadiness::Ready(identity) => Some(*identity),
        _ => None,
    }
}

/// 契约违规 → 一条日志（带形状事实）+ 一句本地化说明（带稳定码）。
fn vector_contract_error(
    i18n: BackendCopy<'_>,
    profile: EmbeddingProfile,
    violation: VectorContract,
) -> String {
    eprintln!(
        "[embed] 引擎返回的向量不符合约定：{} （{} profile v{}）",
        violation.detail(),
        profile.target_id,
        profile.profile_version
    );
    coded_error_with(i18n, violation.code(), "embed-vector-contract")
}

// ── 设置与能力 ──────────────────────────────────────────────────────────────

/// 当前有效嵌入模型（由五槽模型分配解析；未知/缺失在调用处诚实降级）。
#[must_use]
pub(crate) fn embed_model(state: &AppState) -> Option<String> {
    use crate::model_target::ModelTarget;
    // profile（维度 / 分块基准）是模型的属性，与它跑在哪儿无关，故 `service:bge-m3` 与
    // 随包引擎的 `bge-m3-q8_0-gguf` 在这里都解析为 `bge-m3`。
    // 「这批向量由谁产出」的差别由 `resolve_embedding_identity` 按注册表判，不在这里判——
    // 那里问的是注册表而不是设置，两处各判一次必然分叉（见该函数头注释）。
    match ModelTarget::parse(&effective_assignment_target(
        &state.settings_snapshot(),
        "default_embedding",
    )?)? {
        ModelTarget::Service(name) => Some(name.to_owned()),
        // 随包引擎的嵌入条目取它所属的 profile 键（同一模型、同一张表）。
        ModelTarget::StoreEntry(id) => engine_entry_profile_key(id).map(str::to_owned),
    }
}

/// 语义检索总开关（`knowledge.semantic`）。默认开——但**开着不等于可用**：
/// 模型不在场时照样降级（两个判定分开，故设置页能如实说清"开着但缺模型"）。
#[must_use]
pub(crate) fn semantic_enabled(state: &AppState) -> bool {
    state
        .settings_snapshot()
        .get_bool("knowledge.semantic")
        .unwrap_or(true)
}

/// 按当前设置与**算力落点**注册/下线嵌入 Provider。
///
/// 配了模型即 `register_or_replace`；未配即从 registry 移除——留一个指着空模型名的死
/// Provider 会让路由匹配到它然后必然失败，而那与"没有这个能力"是两种不同的用户体验
/// （后者诚实降级，前者是一串错误）。与 `refresh_realtime_llm_provider` 同一形态。
///
/// # 嵌入跟着分配目标走
///
/// 显式 `service:` 走服务端，显式本机条目留在本机；只有未显式分配时，连接服务端才按
/// ADR-037 默认使用服务端算力。端点存在不能覆盖用户选择，否则设置页与字节去向相反。
///
/// **换落点会触发全库重建**（`EmbeddingIndexIdentity` 的 `runtime_digest` 随之变化，
/// 理由见 [`resolve_embedding_identity`]）。这不是遗憾：我们无法证明服务端跑的是与本机
/// 同一版权重，而赌错的代价是两个向量空间被混进同一份索引——不报错、也无法归因。
///
/// # 同一时刻只留一个
///
/// 两档能力集完全相同（`Capability::Embed`），留两个就等于「服务端不通时静默回本机」。
/// 对嵌入而言那还多一层危害：两台机器的模型未必是同一版权重，混着写进同一份索引，
/// 余弦相似度从此没有意义，而症状只是"搜得不太准"。
pub(crate) fn refresh_embed_provider(
    state: &AppState,
    availability: crate::cloud::CloudAvailability,
    bearer: Arc<dyn luna_provider::asr_remote::BearerSource>,
) {
    let engine_id = luna_core::ProviderId::from(luna_provider::LOCAL_EMBED_PROVIDER_ID);
    let cloud_id = luna_core::ProviderId::from(luna_provider::CLOUD_EMBED_PROVIDER_ID);
    let settings = state.settings_snapshot();
    let target = crate::ipc::compute_endpoint_source(availability, &state.cloud_server());
    // 组织没开放嵌入（R4b）就不装服务端那一个：嵌入没有失败回落（向量空间是索引身份），
    // 只能事先就选对。
    let use_native_service =
        crate::ipc::models::assignment_uses_native_service(&settings, "default_embedding", &target)
            && state.server_serves("embed");

    match embed_model(state) {
        None => {
            state.provider_registry.remove(&engine_id);
            state.provider_registry.remove(&cloud_id);
        }
        Some(model) if use_native_service => {
            let Some(profile) = embedding_profile(&model) else {
                state.provider_registry.remove(&cloud_id);
                return;
            };
            state.provider_registry.remove(&engine_id);
            state.provider_registry.register_or_replace(Arc::new(
                luna_provider::CloudEmbedProvider::new(luna_provider::CloudEmbedConfig {
                    api_base: target.url().to_owned(),
                    display_name: "服务端嵌入".to_owned(),
                    profile_id: profile.cloud_profile_id.to_owned(),
                    bearer,
                    gate: state.outbound_gate(),
                }),
            ));
        }
        // 随包引擎：只有条目真在引擎清单里才装；指向没装的条目时不留一个注定失败的 Provider
        // （就绪判定会如实说「未安装」）。
        Some(_) => {
            state.provider_registry.remove(&cloud_id);
            match embed_engine_entry(state) {
                Some(entry) => state.provider_registry.register_or_replace(Arc::new(
                    luna_provider::EmbedHttpProvider::new(
                        luna_provider::EmbedHttpConfig::local_engine(
                            state.local_engine_source(),
                            entry.id,
                        ),
                    ),
                )),
                None => {
                    state.provider_registry.remove(&engine_id);
                }
            }
        }
    }
    // 模型可能刚换过 → 已核实身份与旧矩阵都不再适用，下一次异步入口重新向引擎取事实。
    invalidate_embedding_runtime(state);
}

// ── 内存矩阵（懒加载 + 整体作废）─────────────────────────────────────────────

/// 作废内存矩阵。**切库 / 换模型 / 建库写入之后必须调用**（ADR-025）。
pub(crate) fn invalidate_matrix(state: &AppState) {
    if let Ok(mut guard) = state.embed_matrix.lock() {
        *guard = None;
    }
}

fn invalidate_embedding_runtime(state: &AppState) {
    invalidate_matrix(state);
    if let Ok(mut guard) = state.embed_index_identity.lock() {
        *guard = None;
    }
}

fn cached_embedding_identity(
    state: &AppState,
    profile: EmbeddingProfile,
) -> Option<luna_vault::EmbeddingIndexIdentity> {
    let guard = state.embed_index_identity.lock().ok()?;
    guard
        .as_ref()
        .filter(|identity| {
            identity.target_id == profile.target_id
                && identity.profile_version == profile.profile_version
                && identity.dimension as usize == profile.native_dimension
                && profile
                    .chunk_policy
                    .is_some_and(|policy| identity.chunk_policy_id == policy.id)
        })
        .cloned()
}

/// 取内存矩阵（懒加载：第一次查询时从 `index.sqlite` 读全部向量并建矩阵）。
///
/// 懒加载而不是启动时预热：一个没开过语义检索的用户不该在启动路径上多付一次全表扫 +
/// 几十 MB 分配。首次查询多花的那几十毫秒，与它省掉的启动开销不是一个量级。
fn matrix_for(
    state: &AppState,
    identity: &luna_vault::EmbeddingIndexIdentity,
) -> Option<Arc<VectorMatrix>> {
    if let Ok(guard) = state.embed_matrix.lock() {
        if let Some(matrix) = guard.as_ref() {
            return Some(Arc::clone(matrix));
        }
    }
    let built = {
        let guard = state.vault.lock().ok()?;
        let vault = guard.as_ref()?;
        Arc::new(vault.vector_matrix(identity).ok()?)
    };
    if let Ok(mut guard) = state.embed_matrix.lock() {
        *guard = Some(Arc::clone(&built));
    }
    Some(built)
}

// ── 一次批量嵌入（走单一 AI 主干，ADR-001）───────────────────────────────────

/// 提交一批文本并取回向量。条数不符即判失败（对不上号的向量比没有更糟）。
///
/// **优先级由调用方给，不是恒 Background**：建库是全库尺度的批量任务（Background，
/// 绝不与用户抢队），而**查询侧那一次嵌入是用户正在等的**——两者同档的话，提问时的那
/// 一次会老老实实排在整场建库（S16：万片约 40 分钟）后面，表现出来就是"问一句话转了十分钟"。
async fn embed_texts(
    state: &AppState,
    i18n: BackendCopy<'_>,
    texts: Vec<String>,
    priority: Priority,
    purpose: luna_core::EmbeddingPurpose,
    profile: EmbeddingProfile,
) -> Result<Vec<Vec<f32>>, String> {
    let expected = texts.len();
    if expected == 0 {
        return Ok(Vec::new());
    }
    let texts = profile_texts(profile, purpose, texts);
    let mut job = Job::new(
        luna_core::JobKind::Embed { purpose, texts },
        JobOrigin::Embed,
    );
    job.priority = priority;
    job.routing.local_only = state.strict_local();

    let mut handle = state.queue.submit(job);
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Embeddings(vectors) if vectors.len() == expected => {
                        normalize_embedding_vectors(vectors, profile)
                            .map_err(|violation| vector_contract_error(i18n, profile, violation))
                    }
                    ResultPayload::Embeddings(vectors) => Err(vector_contract_error(
                        i18n,
                        profile,
                        VectorContract::Count {
                            expected,
                            got: vectors.len(),
                        },
                    )),
                    _ => Err(vector_contract_error(
                        i18n,
                        profile,
                        VectorContract::NotVectors,
                    )),
                };
            }
            JobEvent::Failed(err) => {
                // **只发码，诊断串进日志**（ADR-108）：`{err}` 是 `JobError` 的
                // `Display`，它的受众是开发者（ADR-101 ②）——拼进 IPC 返回串等于把一句
                // 不随界面语言变的诊断摆到用户面前，而界面本来就有按码取的本地化文案
                // （`common:failure.*`）。这三处是 ADR-101 ② 点名的最后障碍。
                eprintln!("[job] 失败 code={} detail={err}", err.code());
                return Err(err.code().to_owned());
            }
            JobEvent::Cancelled => {
                return Err(coded_error_with(i18n, "EMBED_CANCELLED", "embed-cancelled"))
            }
            _ => {}
        }
    }
    Err(coded_error_with(i18n, "EMBED_NO_RESULT", "embed-no-result"))
}

// ── 查询侧 ──────────────────────────────────────────────────────────────────

/// 问题 → 语义命中（供 `RetrieveOptions::semantic`）。
///
/// **一律不报错**：语义是增值能力，任何一步不成立都返回空、让调用方走纯关键词
/// （PRD-16-029 诚实降级）。四种"不成立"——关掉了 / 没配模型 / 库里还没有向量 /
/// 这一次嵌入失败——对用户的可见结果都是"检索照常，只是没有语义那一半"。
pub(crate) async fn semantic_hits(state: &AppState, question: &str) -> Vec<SemanticHit> {
    if !semantic_enabled(state) || question.trim().is_empty() {
        return Vec::new();
    }
    let Some(model) = embed_model(state) else {
        return Vec::new();
    };
    let Some(profile) = embedding_profile(&model) else {
        return Vec::new();
    };
    let Some(index_identity) = prepare_embedding_identity(state).await else {
        return Vec::new();
    };
    let Some(matrix) = matrix_for(state, &index_identity) else {
        return Vec::new();
    };
    if matrix.is_empty() {
        return Vec::new(); // 还没建过库：不必为一次注定无果的查询去跑模型
    }
    // 用户正在等这一句的答案 → Interactive（见 `embed_texts` 的纪律说明）。
    // `semantic_hits` 拿不到 `AppHandle`（它是检索路径上的纯查询），而这里的失败**本就
    // 不上界面**（下面 `else` 分支静默降级为无语义命中）。传 `None` 是这一刻唯一诚实的选择：
    // 假装取得到文案，只会产出一句没有任何人会读到的话。
    let Ok(mut vectors) = embed_texts(
        state,
        None,
        vec![question.to_owned()],
        Priority::Interactive,
        luna_core::EmbeddingPurpose::Query,
        profile,
    )
    .await
    else {
        // 脱敏：不打印问题原文，也不打印向量。这里连错误码都不记——查询侧失败是高频
        // 路径（引擎正忙/刚被取消），逐次记日志只会把真问题埋掉。
        return Vec::new();
    };
    if vectors.len() != 1 {
        return Vec::new();
    }
    matrix.search(&vectors.remove(0), SEMANTIC_HIT_LIMIT)
}

/// 语义能力此刻**真的可用吗**（开着 + 配了模型 + 库里已有向量）。
///
/// 知识体检的近重复检测据此决定是**跑**还是**诚实跳过**（PRD-16-031）——给一个空结果
/// 冒充"没有重复"，比明说"这一项没跑"糟得多。
pub(crate) fn semantic_ready(state: &AppState) -> bool {
    if !semantic_enabled(state) {
        return false;
    }
    let Some(model) = embed_model(state) else {
        return false;
    };
    let Some(profile) = embedding_profile(&model) else {
        return false;
    };
    let Some(identity) = cached_embedding_identity(state, profile) else {
        return false;
    };
    matrix_for(state, &identity).is_some_and(|matrix| !matrix.is_empty())
}

/// 全库近重复分片对（知识体检 T1 的第三项，PRD-16-031）。
///
/// `None` = 语义不可用（关掉 / 缺模型 / 还没建库）——与"算过了、没有重复"是两件事，
/// 故用 `Option` 而不是空 `Vec`：调用方要能把前者如实说出来。
///
/// 计算本体在 `luna_vault::near_duplicates`（LSH 预筛 + **精确余弦复核**，遗留清理轮
/// 已做过规模化），此处只负责取矩阵。
pub(crate) fn near_duplicate_pairs(
    state: &AppState,
    limit: usize,
) -> Option<Vec<luna_vault::NearDuplicate>> {
    if !semantic_enabled(state) {
        return None;
    }
    let model = embed_model(state)?;
    let profile = embedding_profile(&model)?;
    let threshold = profile.near_duplicate_threshold?;
    let identity = cached_embedding_identity(state, profile)?;
    let matrix = matrix_for(state, &identity)?;
    if matrix.is_empty() {
        return None;
    }
    Some(luna_vault::near_duplicates(&matrix, threshold, limit))
}

/// 某篇笔记的语义近邻（「相关」页签的静默升级，PRD-16-029）。缺模型/未建库即空。
pub(crate) fn semantic_neighbours(state: &AppState, note_rel: &str, limit: usize) -> Vec<String> {
    if !semantic_enabled(state) {
        return Vec::new();
    }
    let Some(model) = embed_model(state) else {
        return Vec::new();
    };
    let Some(profile) = embedding_profile(&model) else {
        return Vec::new();
    };
    let Some(identity) = cached_embedding_identity(state, profile) else {
        return Vec::new();
    };
    matrix_for(state, &identity)
        .map(|matrix| {
            matrix
                .neighbours(note_rel, limit)
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        })
        .unwrap_or_default()
}

// ── 建库：单篇 ──────────────────────────────────────────────────────────────

/// 嵌入一篇笔记（增量与全量共用的唯一执行体）。返回写入的分片数（`0` = 已最新/无内容）。
///
/// `generation` 是发起时的库代际（ADR-025）：分批嵌入是分钟级 await，落盘那一刻校验——
/// 期间切过库就放弃，绝不把上一个库的向量写进新库的索引。
pub(crate) async fn embed_note(
    app: &tauri::AppHandle,
    note_rel: &str,
    model: &str,
    generation: u64,
) -> Result<u32, String> {
    let state = app.state::<AppState>();
    // 取一次文案来源往下传：`embed_texts` 与归一化都拿不到 `AppHandle`（见 `BackendCopy`）。
    let i18n = backend_copy_arc(app);
    let profile =
        embedding_profile(model).ok_or_else(|| format!("EMBED_PROFILE_MISSING: {model}"))?;
    let policy = profile
        .chunk_policy
        .ok_or_else(|| format!("EMBED_PROFILE_BENCHMARK_PENDING: {model}"))?;
    if embed_model(&state).as_deref() != Some(model) {
        return Err(coded_error(
            app,
            "EMBED_ASSIGNMENT_CHANGED",
            "embed-assignment-changed",
        ));
    }
    let index_identity = match resolve_embedding_identity(&state).await {
        EmbeddingReadiness::Ready(identity) => *identity,
        // 带上就绪码：凭据不可读与引擎不可达的下一步动作完全不同，只报「身份不可用」
        // 等于让人从头猜。
        not_ready => {
            return Err(format!(
                "EMBED_RUNTIME_IDENTITY_UNAVAILABLE: {model} ({})",
                not_ready.as_str()
            ))
        }
    };
    let (chunks, fresh, reusable) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 排除路径（V4a）不进语义索引：保存时也不嵌，与全量建库的盘点同一口径。
        if vault.exclusions().covers(note_rel) {
            return Ok(0);
        }
        (
            vault.chunks_of(note_rel, policy).map_err(vault_err)?,
            vault
                .embeddings_fresh(note_rel, &index_identity)
                .unwrap_or(false),
            vault
                .reusable_vectors(note_rel, &index_identity)
                .unwrap_or_default(),
        )
    };
    if chunks.is_empty() || fresh {
        return Ok(0);
    }

    // **分片级复用**：编辑器每 ~800ms 自动保存一次、每次正文指纹都变——只按正文指纹
    // 判定的话，改一行会把一篇 40 片的长笔记整篇重嵌（S16：~0.25 秒/片 ≈ 10 秒 GPU）。
    // 故先按**分片文本指纹**捞出仍可用的那些，只把真的变了的送去嵌入。
    let hashes: Vec<String> = chunks
        .iter()
        .map(|c| luna_vault::chunk_fingerprint(&c.text))
        .collect();
    let missing: Vec<usize> = (0..chunks.len())
        .filter(|i| !reusable.contains_key(&hashes[*i]))
        .collect();

    let mut fresh_vectors: std::collections::HashMap<usize, Vec<f32>> =
        std::collections::HashMap::new();
    // 逐批嵌入。分批而不是一次全发：一篇长纪要能切出几十片，一次请求既撑爆单批预算
    // 也让取消无处插入。
    for batch in missing.chunks(EMBED_BATCH) {
        let texts: Vec<String> = batch.iter().map(|i| chunks[*i].text.clone()).collect();
        let got = embed_texts(
            &state,
            i18n.as_deref(),
            texts,
            Priority::Background,
            luna_core::EmbeddingPurpose::Document,
            profile,
        )
        .await?;
        for (slot, vector) in batch.iter().zip(got) {
            fresh_vectors.insert(*slot, vector);
        }
    }
    // 按分片顺序拼齐：复用的取旧向量，新算的取本轮结果。任一片缺失即整篇不写
    // （对不上号的向量比没有更糟——`put_embeddings` 也会再拦一次）。
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
    for (i, hash) in hashes.iter().enumerate() {
        let Some(vector) = fresh_vectors
            .remove(&i)
            .or_else(|| reusable.get(hash).cloned())
        else {
            return Err(coded_error(
                app,
                "EMBED_SHAPE",
                "embed-shape-chunks-missing",
            ));
        };
        vectors.push(vector);
    }

    if !state.still_current_vault(generation) {
        return Err(coded_error(
            app,
            "EMBED_VAULT_SWITCHED",
            "embed-vault-switched",
        ));
    }
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .put_embeddings(note_rel, &index_identity, &chunks, &vectors)
            .map_err(vault_err)?;
    }
    // 索引变了 → 内存矩阵陈旧。
    invalidate_matrix(&state);
    Ok(clamp_u32(chunks.len()))
}

/// 知识生命周期协调器使用的可等待入口。
///
/// 与 fire-and-forget 的 [`spawn_embed_if_configured`] 不同，这个入口把一次真实嵌入失败
/// 返回给协调器，后者才能在统一知识回执里准确标记 `embed` 降级。设置关闭或没有配置模型
/// 是明确跳过，不是失败。
pub(crate) async fn embed_if_configured(
    app: &tauri::AppHandle,
    note_rel: &str,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !semantic_enabled(&state) || !note_rel.ends_with(".md") {
        return Ok(());
    }
    let Some(model) = embed_model(&state) else {
        return Ok(());
    };
    let generation = state.vault_generation();
    embed_note(app, note_rel, &model, generation)
        .await
        .map(|_| ())
}

/// 增量建库的触发（笔记写盘后，PRD-16-028）。
///
/// **去抖靠指纹而不是定时器**：`embed_note` 的第一件事就是比对正文指纹，已最新即 0 成本
/// 返回。故这里只需一道 in-flight 去重（同一篇的并发触发只留一个），不必再造一个
/// "等 800ms 看看还改不改"的计时器——那层去抖编辑器保存本身已经做过了。
pub(crate) fn spawn_embed_if_configured(app: &tauri::AppHandle, note_rel: &str) {
    let state = app.state::<AppState>();
    if !semantic_enabled(&state) || !note_rel.ends_with(".md") {
        return;
    }
    let Some(model) = embed_model(&state) else {
        return;
    };
    {
        let Ok(mut inflight) = state.embed_inflight.lock() else {
            return;
        };
        if !inflight.insert(note_rel.to_owned()) {
            return;
        }
    }
    let generation = state.vault_generation();
    let app = app.clone();
    let rel = note_rel.to_owned();
    tauri::async_runtime::spawn(async move {
        let outcome = embed_note(&app, &rel, &model, generation).await;
        let state = app.state::<AppState>();
        if let Ok(mut inflight) = state.embed_inflight.lock() {
            inflight.remove(&rel);
        }
        if let Err(code) = outcome {
            // 脱敏：只留错误码与路径（TDD-12）。
            eprintln!("[embed] {rel} 嵌入跳过：{code}");
        }
    });
}

// ── 建库：全量（六要素骨架，照搬 graph.rs 全库重扫）──────────────────────────

/// 全量建库运行态（`AppState` 持有）：取消令牌在场 = 有一次建库在跑（单例）。
#[derive(Default)]
pub(crate) struct EmbedRebuild {
    /// 取消令牌在场 = 有一次建库在跑。切库交接（TDD-06 §12）也置位它。
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    progress: Option<EmbedProgress>,
}

/// 建库进度事件（`embed://progress`）。逐篇低频（每篇若干次批量嵌入，秒级），无需节流。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "embed://progress")]
pub struct EmbedProgress {
    /// 已处理篇数（含失败）。
    pub done: u32,
    /// 本次总篇数。
    pub total: u32,
    /// 已写入的分片数（进度条的第二维：篇数掩盖不了"这一篇有 40 段"这个事实）。
    pub chunks: u32,
    /// 失败篇数。
    pub failed: u32,
    /// 终态标记。
    pub finished: bool,
    /// 用户主动取消。
    pub cancelled: bool,
    /// 连续失败提前中止（引擎不可用的典型指纹）。
    pub halted: bool,
}

/// 建库预检 DTO：确认卡据此在**点开始之前**陈述代价。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct EmbedPreflightDto {
    /// 当前模型（空 = 未配，前端据此给「去下载」引导而不是开始按钮）。
    pub model: String,
    /// 语义检索总开关的真值。
    pub enabled: bool,
    /// 嵌入能力此刻可用（身份可核实 + profile 基准已过）。
    pub available: bool,
    /// 不可用时的**具体原因**（[`EmbeddingReadiness::as_str`] 的稳定 code；可用时为
    /// `ready`）。前端按它区分「去分配 / 去下载 / 把引擎跑起来」，不合并成一句谎话。
    pub readiness: String,
    /// 待嵌入篇数。
    pub pending: u32,
    /// 待嵌入的**分片**数（耗时按它估，见 `EmbedScan` 的说明）。
    pub pending_chunks: u32,
    /// 已最新的篇数 / 分片数。
    pub fresh: u32,
    pub fresh_chunks: u32,
    /// 无可嵌入内容的篇数。
    pub empty: u32,
    /// 预估耗时（秒；S16 实测约 0.25 秒/片）。**如实给数字**，不说"很快"。
    pub eta_seconds: u32,
    /// 内存矩阵此刻占用的字节数（0 = 还没加载）。
    pub matrix_bytes: u32,
    /// 是否已有一次建库在跑。
    pub running: bool,
}

/// S16 实测的单片耗时（秒/片，批 8+ 的平台值 ≈ 0.245s）。预估用它，不用乐观数。
const SECONDS_PER_CHUNK: f64 = 0.25;

fn clamp_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 建库预检（确认卡 + 设置页状态行共用的唯一取数口）。只读、不跑模型。
#[tauri::command]
#[specta::specta]
pub(crate) async fn embed_preflight(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<EmbedPreflightDto, String> {
    let model = embed_model(&state);
    // 可用 = 已安装且拿到真实 digest，并且该 profile 已有验收过的分块策略。探测失败不能
    // 冒充可用；否则会让用户先付一次全库扫描，再在执行阶段连败。
    let readiness = resolve_embedding_identity(&state).await;
    let readiness_code = readiness.as_str().to_owned();
    let identity = match readiness {
        EmbeddingReadiness::Ready(identity) => Some(*identity),
        _ => None,
    };
    let available = identity.is_some();
    let running = state
        .embed_rebuild
        .lock()
        .map_err(|_| "建库状态锁中毒".to_string())?
        .cancel
        .is_some();
    let matrix_bytes = state
        .embed_matrix
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| clamp_u32(m.bytes())))
        .unwrap_or(0);
    // 能力不可用就不去扫库：扫描本身是全库读盘，为一个用不了的能力付这个代价没道理。
    // 身份就绪时 profile/policy 必然在场（身份正是由它们构造的），此处直接取。
    let scan = match &identity {
        Some(identity) => {
            let policy = model
                .as_deref()
                .and_then(embedding_profile)
                .and_then(|profile| profile.chunk_policy)
                .ok_or_else(|| {
                    coded_error(&app, "EMBED_PROFILE_MISSING", "embed-profile-mismatch")
                })?;
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            vault.embed_scan(identity, policy).map_err(vault_err)?
        }
        None => luna_vault::EmbedScan::default(),
    };
    Ok(EmbedPreflightDto {
        model: model.unwrap_or_default(),
        enabled: semantic_enabled(&state),
        available,
        readiness: readiness_code,
        pending: clamp_u32(scan.pending.len()),
        pending_chunks: scan.pending_chunks,
        fresh: scan.fresh,
        fresh_chunks: scan.fresh_chunks,
        empty: scan.empty,
        eta_seconds: ((f64::from(scan.pending_chunks) * SECONDS_PER_CHUNK).ceil()) as u32,
        matrix_bytes,
        running,
    })
}

/// 启动全量建库。返回本次总篇数；单例——已有一次在跑即拒绝第二次。
#[tauri::command]
#[specta::specta]
pub(crate) async fn embed_rebuild_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let model =
        embed_model(&state).ok_or_else(|| coded_error(&app, "EMBED_NO_MODEL", "embed-no-model"))?;
    let profile =
        embedding_profile(&model).ok_or_else(|| format!("EMBED_PROFILE_MISSING: {model}"))?;
    let policy = profile
        .chunk_policy
        .ok_or_else(|| format!("EMBED_PROFILE_BENCHMARK_PENDING: {model}"))?;
    let index_identity = match resolve_embedding_identity(&state).await {
        EmbeddingReadiness::Ready(identity) => *identity,
        // 带上就绪码：凭据不可读与引擎不可达的下一步动作完全不同，只报「身份不可用」
        // 等于让人从头猜。
        not_ready => {
            return Err(format!(
                "EMBED_RUNTIME_IDENTITY_UNAVAILABLE: {model} ({})",
                not_ready.as_str()
            ))
        }
    };
    let targets = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .embed_scan(&index_identity, policy)
            .map_err(vault_err)?
            .pending
    };
    if targets.is_empty() {
        return Err(coded_error(
            &app,
            "EMBED_NOTHING_TO_DO",
            "embed-nothing-to-do",
        ));
    }
    let total = clamp_u32(targets.len());
    let cancel = Arc::new(AtomicBool::new(false));
    let initial = EmbedProgress {
        done: 0,
        total,
        chunks: 0,
        failed: 0,
        finished: false,
        cancelled: false,
        halted: false,
    };
    {
        let mut guard = state
            .embed_rebuild
            .lock()
            .map_err(|_| "建库状态锁中毒".to_string())?;
        if guard.cancel.is_some() {
            return Err(coded_error(&app, "EMBED_BUSY", "embed-busy"));
        }
        guard.cancel = Some(Arc::clone(&cancel));
        guard.progress = Some(initial.clone());
    }
    let _ = initial.emit(&app);
    let generation = state.vault_generation();
    tauri::async_runtime::spawn(async move {
        run_rebuild(app, targets, model, generation, cancel).await;
    });
    Ok(total)
}

/// 请求取消进行中的建库（当前那一篇收尾后停下；幂等）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn embed_rebuild_cancel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state
        .embed_rebuild
        .lock()
        .map_err(|_| "建库状态锁中毒".to_string())?;
    if let Some(cancel) = guard.cancel.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// 建库进度快照（设置页重挂载时恢复进行中的进度条）；`None` = 无建库在跑。
#[tauri::command]
#[specta::specta]
pub(crate) async fn embed_rebuild_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<EmbedProgress>, String> {
    Ok(state
        .embed_rebuild
        .lock()
        .map_err(|_| "建库状态锁中毒".to_string())?
        .progress
        .clone())
}

/// 建库执行体：逐篇嵌入 → 逐篇报进度 → 终态收尾。
async fn run_rebuild(
    app: tauri::AppHandle,
    targets: Vec<String>,
    model: String,
    generation: u64,
    cancel: Arc<AtomicBool>,
) {
    let total = clamp_u32(targets.len());
    let mut progress = EmbedProgress {
        done: 0,
        total,
        chunks: 0,
        failed: 0,
        finished: false,
        cancelled: false,
        halted: false,
    };
    let mut consecutive_failures: u32 = 0;
    for rel in &targets {
        if cancel.load(Ordering::Relaxed) {
            progress.cancelled = true;
            break;
        }
        // 切库即停：产物属于上一个库，继续跑只是白烧算力（`embed_note` 内的提交点也会拒）。
        if !app.state::<AppState>().still_current_vault(generation) {
            progress.cancelled = true;
            break;
        }
        match embed_note(&app, rel, &model, generation).await {
            Ok(written) => {
                progress.chunks += written;
                consecutive_failures = 0;
            }
            // **取消不算失败**：查询侧的 Interactive 嵌入会抢占在途的建库批次，于是
            // 「被抢占」是正常运行中的高频事件。把它计入连败，用户在建库期间问三个问题
            // 就会让建库"连续失败中止"——而那不是故障，那篇也仍留在待办里（指纹未写）。
            // 与 graph.rs 重扫把 `GRAPH_TAGS_BUSY` 排除在连败之外同一裁决。
            Err(code) if code.starts_with("EMBED_CANCELLED") => {}
            Err(code) => {
                progress.failed += 1;
                consecutive_failures += 1;
                eprintln!("[embed-rebuild] {rel} 失败：{code}");
            }
        }
        progress.done += 1;
        if consecutive_failures >= HALT_AFTER_CONSECUTIVE_FAILURES {
            progress.halted = true;
            break;
        }
        if !progress.cancelled && !progress.halted && progress.done < total {
            publish_progress(&app, progress.clone());
        }
    }
    progress.finished = true;
    // 先清运行态再发终态帧（订阅方收到 finished 后再查 status 必然是"无建库在跑"）。
    {
        let state = app.state::<AppState>();
        if let Ok(mut guard) = state.embed_rebuild.lock() {
            guard.cancel = None;
            guard.progress = None;
        }
        invalidate_matrix(&state);
    }
    let _ = progress.clone().emit(&app);
    notify_outcome(&app, &progress);
}

fn publish_progress(app: &tauri::AppHandle, progress: EmbedProgress) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.embed_rebuild.lock() {
        guard.progress = Some(progress.clone());
    }
    let _ = progress.emit(app);
}

/// 终局通知：建库是典型的「点了开始就切去做别的」长任务（S16：万片约 40 分钟）。
/// 用户主动取消不发（他知道自己做了什么），与全库重扫同一裁决。
fn notify_outcome(app: &tauri::AppHandle, progress: &EmbedProgress) {
    if progress.cancelled {
        return;
    }
    let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    let target = crate::notifications::NotificationTarget::Route("settings://notes".to_owned());
    let event = if progress.halted {
        // 嵌入的落点**自己判**，不借用对话那条：两者可以分处两地（对话在服务端、
        // 嵌入在本机是常态），借用会让文案把用户支到一台与失败无关的机器上。
        let location = if embeds_on_server(&app.state::<AppState>()) {
            super::current_chat_compute_location(app, &app.state::<AppState>())
        } else if app.state::<AppState>().strict_local() {
            crate::cloud::ComputeLocation::StrictLocal
        } else {
            crate::cloud::ComputeLocation::OnDevice
        };
        crate::notifications::NotificationEvent::failure("semantic_index_halted", target)
            .executed_at(location)
    } else {
        crate::notifications::NotificationEvent::completion(
            "semantic_index_built",
            target,
            crate::notifications::PrivatePayload::new(String::new()),
        )
    };
    nc.publish(event, app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_query_instruction_is_never_applied_to_document_chunks() {
        let profile = embedding_profile("qwen3-embedding:0.6b").unwrap();
        let query = profile_texts(
            profile,
            luna_core::EmbeddingPurpose::Query,
            vec!["how to retain a meeting decision".to_owned()],
        );
        let document = profile_texts(
            profile,
            luna_core::EmbeddingPurpose::Document,
            vec!["meeting decision text".to_owned()],
        );
        assert!(query[0].starts_with("Instruct:"));
        assert_eq!(document, ["meeting decision text"]);
    }

    #[test]
    fn profile_rejects_wrong_dimension_and_normalizes_valid_vector() {
        let profile = embedding_profile("bge-m3").unwrap();
        assert!(normalize_embedding_vectors(vec![vec![1.0]], profile).is_err());
        let mut vector = vec![0.0; profile.native_dimension];
        vector[0] = 3.0;
        vector[1] = 4.0;
        let normalized = normalize_embedding_vectors(vec![vector], profile).unwrap();
        assert!((normalized[0][0] - 0.6).abs() < f32::EPSILON);
        assert!((normalized[0][1] - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn qwen_profiles_stay_benchmark_gated_for_indexing_and_near_duplicates() {
        for model in [
            "qwen3-embedding:0.6b",
            "qwen3-embedding:4b",
            "qwen3-embedding:8b",
        ] {
            let profile = embedding_profile(model).unwrap();
            assert!(profile.near_duplicate_threshold.is_none());
            assert!(profile.chunk_policy.is_none());
            assert!(!assignment_target_supported(&format!("service:{model}")));
        }
        assert!(!assignment_target_supported(
            "qwen3-embedding-0.6b-q8_0-gguf"
        ));
        assert!(assignment_target_supported("bge-m3-q8_0-gguf"));
        assert_eq!(
            embedding_profile("bge-m3")
                .unwrap()
                .near_duplicate_threshold,
            Some(0.93)
        );
    }

    #[tokio::test]
    async fn explicit_local_embedding_stays_local_when_server_is_connected() {
        struct NoBearer;
        #[async_trait::async_trait]
        impl luna_provider::asr_remote::BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_embedding", "bge-m3-q8_0-gguf");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));
        state.record_engine_models(vec![crate::local_engine::EngineModelRef {
            id: "bge-m3-q8_0-gguf".into(),
            kind: crate::local_engine::EngineModelKind::Embed,
            digest: Some("ab".repeat(32)),
            translation_only: false,
            size_bytes: 0,
        }]);
        refresh_embed_provider(
            &state,
            crate::cloud::CloudAvailability::Allowed,
            Arc::new(NoBearer),
        );
        assert!(state
            .provider_registry
            .get(&luna_core::ProviderId::from(
                luna_provider::LOCAL_EMBED_PROVIDER_ID
            ))
            .is_some());
        assert!(
            state
                .provider_registry
                .get(&luna_core::ProviderId::from(
                    luna_provider::CLOUD_EMBED_PROVIDER_ID
                ))
                .is_none(),
            "显式本机嵌入被服务端连接覆盖"
        );
    }

    /// 嵌入槽指向随包引擎的嵌入条目（ADR-143）：
    /// ① 引擎清单里没有它 → 不装 Provider、就绪判定如实说「未安装」，不臆造一个目标；
    /// ② 装了 → 只装随包引擎嵌入 Provider，索引身份的目标是**条目 id**、摘要是**权重 sha256**
    ///    （换运行时身份必然不同 → 全库重建一次，这是对的）；
    /// ③ 同一时刻只留一个本机嵌入 Provider。
    #[tokio::test]
    async fn an_engine_embedding_entry_assembles_the_bundled_engine_and_its_own_identity() {
        use crate::local_engine::{EngineModelKind, EngineModelRef};
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_embedding", "bge-m3-q8_0-gguf");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        let registered = |id: &str| {
            state
                .provider_registry
                .get(&luna_core::ProviderId::from(id))
                .is_some()
        };
        struct UnusedBearer;
        #[async_trait::async_trait]
        impl luna_provider::asr_remote::BearerSource for UnusedBearer {
            async fn bearer(&self) -> Option<String> {
                unreachable!("本机档不取凭据")
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                unreachable!("本机档不取凭据")
            }
        }
        let assemble = || {
            refresh_embed_provider(
                &state,
                crate::cloud::CloudAvailability::SignedOut,
                Arc::new(UnusedBearer),
            );
        };

        assert!(assignment_target_supported("bge-m3-q8_0-gguf"));
        assert!(
            !assignment_target_supported("gguf-daec91ffb5dd"),
            "导入条目没有审核过的 profile"
        );
        assert_eq!(embed_model(&state).as_deref(), Some("bge-m3"));

        assemble();
        assert!(!registered(luna_provider::LOCAL_EMBED_PROVIDER_ID));
        assert_eq!(
            resolve_embedding_identity(&state).await.as_str(),
            "not_installed"
        );

        state.record_engine_models(vec![EngineModelRef {
            id: "bge-m3-q8_0-gguf".into(),
            kind: EngineModelKind::Embed,
            digest: Some("ab".repeat(32)),
            translation_only: false,
            size_bytes: 0,
        }]);
        assemble();
        assert!(registered(luna_provider::LOCAL_EMBED_PROVIDER_ID));
        let EmbeddingReadiness::Ready(identity) = resolve_embedding_identity(&state).await else {
            panic!("装了就该就绪");
        };
        assert_eq!(identity.target_id, "bge-m3-q8_0-gguf");
        assert_eq!(
            identity.runtime_digest,
            format!("sha256:{}", "ab".repeat(32))
        );
        assert_eq!(identity.dimension, 1024);

        // 清掉分配（无服务端）→ 设备策略仍落到同一条目，Provider 不掉。
        state
            .settings
            .lock()
            .unwrap()
            .set("ai.assignment.default_embedding", "");
        assemble();
        assert!(registered(luna_provider::LOCAL_EMBED_PROVIDER_ID));
    }

    /// 目录里每个嵌入 GGUF 条目都必须映射到一张 profile，映射里的每个 id 都必须真在目录里：
    /// 漏一条的表现是「装好了却说不支持」，多一条（目录删了映射还在）是死映射。
    #[test]
    fn every_catalog_embedding_entry_maps_to_a_profile() {
        let embed_entries: Vec<&str> = luna_model_catalog::bundled()
            .iter()
            .filter(|e| {
                matches!(
                    e.gguf.as_ref().map(|g| g.role),
                    Some(luna_model_catalog::GgufRole::Embed { .. })
                )
            })
            .map(|e| e.id.as_str())
            .collect();
        assert!(!embed_entries.is_empty());
        for id in &embed_entries {
            let key =
                engine_entry_profile_key(id).unwrap_or_else(|| panic!("{id} 没有 profile 映射"));
            assert!(
                embedding_profile(key).is_some(),
                "{id} → {key} 不在 profile 表里"
            );
        }
        for id in ["bge-m3-q8_0-gguf", "qwen3-embedding-0.6b-q8_0-gguf"] {
            assert!(embed_entries.contains(&id), "映射里的 {id} 已不在目录里");
        }
    }

    /// 预估耗时用 S16 的**平台值**而不是乐观数，批大小落在实测的吞吐平台上。
    ///
    /// 用极短串量到的 16 ms/片会把「1 万片 40 分钟」说成「2.7 分钟」——那种预估比不给更糟，
    /// 因为用户正是据此决定要不要现在点开始。写成编译期断言：调这两个常量的人立刻撞墙，
    /// 不必等跑测试。
    const _CONSTANTS_MATCH_THE_SPIKE: () = {
        assert!(
            SECONDS_PER_CHUNK >= 0.2,
            "S16 实测批 8+ 的平台是 0.245 秒/片"
        );
        // 1 万片 ≈ 41.7 分钟，与 spike 报告的 40.5–44.5 分钟对得上。
        assert!(10_000.0 * SECONDS_PER_CHUNK / 60.0 <= 45.0);
        assert!(10_000.0 * SECONDS_PER_CHUNK / 60.0 >= 40.0);
        // 批 8 即到吞吐平台，再大只是让取消更迟钝。
        assert!(EMBED_BATCH >= 8);
        assert!(EMBED_BATCH <= 32);
    };

    /// **服务端跑嵌入时，索引身份不得向本机引擎要权重摘要。**
    ///
    /// 这是「服务端 embed 接入」最容易漏掉的一半，而漏掉它整件事就是**不通**的：本机没有
    /// 那个权重 → `resolve_embedding_identity` 回 `NotInstalled` →
    /// `prepare_embedding_identity` 拿不到身份 → 索引写入整条不执行 → 语义检索静默退回纯
    /// 关键词。用户看到的是「AI 由服务端提供」，而语义索引是唯一完全不生效的那一项，
    /// **且不报任何错**。
    ///
    /// 断言**刻意不碰本机那一条分支**：开发机上多半装着本机嵌入模型，于是「本机拿不到身份」这个前提在 CI 上成立、在开发机上不成立——一条
    /// 看心情红的测试比没有测试更糟。故只钉服务端那一档的确定性质：装上云嵌入 Provider
    /// 之后，身份必须是 `server:<分桶>:<模型>` 形状。反向验证（删掉那条分支即红）证明
    /// 它不是空跑。
    #[tokio::test]
    async fn a_server_side_embedding_does_not_ask_the_local_runtime_for_a_digest() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_embedding", "service:bge-m3");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));

        // 装上云嵌入 Provider（= 落点在服务端），身份必须**不再**依赖本机引擎。
        struct NoBearer;
        #[async_trait::async_trait]
        impl luna_provider::asr_remote::BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }
        state.provider_registry.register_or_replace(Arc::new(
            luna_provider::CloudEmbedProvider::new(luna_provider::CloudEmbedConfig {
                api_base: "http://10.0.0.62:8090".into(),
                display_name: "服务端嵌入".into(),
                profile_id: luna_vault::BGE_M3_PROFILE.cloud_profile_id.to_owned(),
                bearer: Arc::new(NoBearer),
                gate: state.outbound_gate(),
            }),
        ));

        let EmbeddingReadiness::Ready(identity) = resolve_embedding_identity(&state).await else {
            panic!(
                "服务端跑嵌入却仍向本机引擎要权重摘要——索引写入整条不执行，\
                 语义检索静默退回纯关键词，且不报任何错"
            );
        };
        assert!(
            identity.runtime_digest.starts_with("server:"),
            "服务端那一档的身份必须**看得出**不是权重哈希（服务端不下发摘要，\
             假装有一个才是错的）：实得 {}",
            identity.runtime_digest
        );
        assert!(
            identity.runtime_digest.contains("10.0.0.62:8090"),
            "身份里没有服务端标识——换台服务端时索引不会重建，两个向量空间会被混用"
        );
    }

    /// **换落点必须换索引身份——而这条链此前没有任何一处被端到端断言过。**
    ///
    /// 上一条只钉了「服务端那一档的身份长什么样」（`starts_with("server:")` +
    /// `contains(一台写死的地址)`），它在**两台服务端返回同一个身份**时照样全绿。而那正是
    /// 要防的那个错：两台服务端跑的权重我们无法证明相同，身份若不跟着变，两个向量空间就被
    /// 混进同一份索引——症状只是"搜得不太准"，不报错、也无法归因（TDD-14 §3.3.1 ⑥）。
    ///
    /// 身份是 `EmbeddingIndexIdentity` **整体**参与比较的（`Vault::embeddings_fresh` 用
    /// `==`），所以这里断言的就是喂给它的那个值：A ≠ B ⇒ 每一篇笔记都变 pending ⇒ 全库重建。
    #[tokio::test]
    async fn switching_the_endpoint_changes_the_index_identity_and_forces_a_full_rebuild() {
        struct NoBearer;
        #[async_trait::async_trait]
        impl luna_provider::asr_remote::BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }

        async fn identity_on(api_base: &str) -> luna_vault::EmbeddingIndexIdentity {
            let dir = tempfile::tempdir().unwrap();
            let mut settings = Settings::with_defaults();
            settings.set("ai.assignment.default_embedding", "service:bge-m3");
            let state = build_state_with_settings_persistence(
                settings,
                SettingsPersistence::Writable(dir.path().join("config.toml")),
                luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
                None,
            );
            state.set_cloud_server(crate::cloud::ServerRef::of(api_base));
            state.provider_registry.register_or_replace(Arc::new(
                luna_provider::CloudEmbedProvider::new(luna_provider::CloudEmbedConfig {
                    api_base: api_base.to_owned(),
                    display_name: "服务端嵌入".into(),
                    profile_id: luna_vault::BGE_M3_PROFILE.cloud_profile_id.to_owned(),
                    bearer: Arc::new(NoBearer),
                    gate: state.outbound_gate(),
                }),
            ));
            match resolve_embedding_identity(&state).await {
                EmbeddingReadiness::Ready(identity) => *identity,
                other => panic!("服务端落点下必须解得出身份，实得 {}", other.as_str()),
            }
        }

        let on_a = identity_on("https://10.0.0.62:8443").await;
        let on_b = identity_on("https://another.internal:8443").await;

        assert_ne!(
            on_a, on_b,
            "换了服务端而索引身份没变——两台服务端的向量会被当成同一个空间混进一份索引，\
             `EmbeddingIndexIdentity` 存在的全部理由就是不让这件事发生"
        );
        assert_ne!(
            on_a.runtime_digest, on_b.runtime_digest,
            "区别必须落在 runtime_digest（「这批向量由谁产出」）上，而不是别的字段碰巧不同"
        );

        // 反过来同样要成立：同一台服务端换个写法**不算**换了一台，否则每次把地址从
        // http 改成 https、或多打一个尾斜杠，用户就要白等一次全库重建。
        let same_server_other_spelling = identity_on("http://10.0.0.62:8443/").await;
        assert_eq!(
            on_a, same_server_other_spelling,
            "同一台服务端被判成了两台——用户改个写法就要重建整库，而重建不是免费的"
        );
    }
}
