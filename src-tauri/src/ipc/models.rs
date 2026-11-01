//! 模型管理命令域（PRD-10/TDD-04，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：模型目录与就绪度、逐槽位分配、算力落点状态、安装与删除的计划与执行。
//! 权重与目录事实源在 [`crate::model_store`]，本模块只做 IPC 面与装配刷新。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;
use crate::model_target::ModelTarget;

/// 列出本地可见模型（模型库里的 whisper 与随包引擎条目），含来源/License/占用（TDD-04）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelAssignmentDto {
    /// 稳定槽位名。
    pub slot: String,
    /// 该槽位对应的设置 key（`ai.assignment.<slot>`）。
    ///
    /// 面板据此给自己那一行打 `data-setting-key`，于是搜索命中这一项时的深链能滚动定位过来。
    /// 由 Rust 下发而不是前端拼 `"ai.assignment." + slot`：槽位名与 key 的对应关系是注册表
    /// 知识，抄第二遍就会在改名那天悄悄失配（表现为深链跳过去后停在页面顶端，无人会红）。
    pub setting_key: String,
    /// 用户明确写入的目标；空值代表继承或设备策略。
    pub configured_target: Option<String>,
    /// 此刻实际解析出的目标。
    pub effective_target: Option<String>,
    /// 有效目标的**展示名**。
    ///
    /// 前端不得再从 `candidates` 里反查它：候选表结构性地**只含已就绪的模型**，而"有效目标
    /// 尚未就绪"恰恰是最需要说清楚的那一档——反查必然落空，界面就会把 `whisper-tiny-bundled`、
    /// `bge-m3-q8_0-gguf` 这类**给解析器看的串**直接摆给用户（真机坐实）。目录里没有这个目标时
    /// 为 `None`，此时由前端呈现"未解析"，而不是退回打印 id。
    pub effective_name: Option<String>,
    /// `explicit` / `inherited` / `device_policy`。
    pub source: String,
    /// `ready` / `missing` / `incompatible` / `profile_unverified` / `unresolved`
    /// （托管嵌入档的就绪与执行同源于注册表——见 `ServerManagedFacts`）。
    pub readiness: String,
    /// 该槽位所需的有效能力。
    pub required_capability: String,
    /// `model` / `managed` / `profile` / `channel`。`managed` 表示服务端选择文本模型；
    /// `profile` 表示服务端执行固定版本的向量 Profile，而非按请求切换向量空间。
    /// 「此刻实际在哪执行」不在本 DTO 重复下发——那份事实只归 `compute_status`（DRY）。
    pub selection_mode: ModelSelectionMode,
    /// **清掉本槽的显式分配后是否落到「服务端托管」**（ADR-037 连上服务端即用它、
    /// ADR-048 服务端托管）。
    ///
    /// 存在的理由是一条真机缺陷：用户显式钉了本机模型后，唯一的切回路径是清掉分配（下拉的
    /// 空选项 = 跟随），而那个空选项在有本机 pin 时写着「跟随设备策略」——没人看得出它其实
    /// 是「交回服务端托管」。转写槽位有一档显式的「服务端转写」可选，对话/嵌入却没有对应的
    /// 「服务端托管」，于是连着组织服务端、服务端明明能跑 chat/embed，用户却找不到切回去的路。
    /// 由 Rust 判定（前端零业务，ADR-003）：= 端点原生托管**且**清掉本槽后整条继承链不再有
    /// 本机显式立场；前端据此把那个空选项如实标成「服务端托管」而不是泛化的「跟随」。
    pub follow_is_server_managed: bool,
    /// 已由 Rust 按安装、实测能力与运行条件裁决的可选目标；前端不得再次筛选。
    pub candidates: Vec<ModelAssignmentCandidateDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelAssignmentCandidateDto {
    pub target_id: String,
    pub name: String,
    /// `device` / `server`。前端只据此分组展示，不解析 target id 前缀。
    pub location: ModelExecutionLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelExecutionLocation {
    Device,
    Server,
}

/// 一条能力此刻的**有效执行位置 + 为什么是它 + 从哪儿改**（ADR-059 ④）。
///
/// # 为什么「为什么」必须一起下发
///
/// 算力卡此前只说「服务端 / 这台设备」。同样是「这台设备」，背后可能是四件完全不同的事：
/// 用户自己选的、隐私档压的、还没同意把麦克风交出去、或者服务端根本不提供这条能力。
/// 它们的下一步动作互不相同，而卡片给的是同一个词——于是用户只能逐个槽位去翻设置，
/// 才能发现「原来是我某次自己钉的」。同行做法（Cursor / Copilot / Raycast）一律把有效
/// 位置与理由摆在一起，并就地给切换入口。
///
/// 判定全在 Rust（ADR-003）：前端拿 `reason` 取一句文案、拿 `setting_key` 拼深链，不自己推。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComputeRouteDto {
    /// `chat` / `embedding` / `transcription`。前端按它取标题，不解析槽位名。
    pub capability: String,
    /// 此刻**实际**在哪跑。
    pub location: ModelExecutionLocation,
    /// 为什么是这个位置。
    pub reason: ComputeRouteReason,
    /// 决定这条能力的槽位设置键——就地切换入口（`settings://ai_services/<key>`）。
    pub setting_key: String,
    /// 这条能力此刻**可以**交给服务端吗。`false` 时不摆一个点了也没用的入口。
    pub can_use_server: bool,
}

/// 「为什么是这个执行位置」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ComputeRouteReason {
    /// 「仅本地」隐私档硬拦：任何服务端选择此刻都不生效（ADR-038 ④）。
    StrictLocal,
    /// 用户在这条能力上**显式**选了本机模型。连接状态不改写它（ADR-037/048）。
    ExplicitDevice,
    /// 用户**显式**把这条能力交给了服务端。
    ExplicitServer,
    /// 没人选过，服务端在，按「连上即用」落到服务端（ADR-037）。
    ServerDefault,
    /// 没人选过，也没有可用服务端，落到设备策略档（ADR-059 起按已装清单挑）。
    DevicePolicy,
    /// 差一次单独同意才会上服务端（麦克风与会议录音，ADR-038 ④）。
    NeedsConsent,
    /// 服务端不提供这条能力（或此刻用不了），退到本机。
    ServerUnavailable,
    /// 已登录，但这台服务端没给这项能力配上游（ADR-041 ①），在本机跑。
    ServerNotConfigured,
    /// 已登录，但账户所在组织没开放这项能力（配额策略的能力开关，统一登录 R4b），在本机跑。
    ServerNotAllowed,
    /// 「优先在哪运行」设成了本机优先：服务端在，但这条能力先在本机跑。
    StrategyLocal,
    /// 「优先在哪运行」设成了仅本机：外部候选一律不进路由，连着服务端也不外发。
    StrategyLocalOnly,
    /// 设成了本机优先，但本机解析不出可用模型，这条能力暂由服务端执行。
    StrategyLocalUnavailable,
}

/// 服务端路径上的本机候选如何参与执行。
///
/// 这不是「是否可重试」的另一种说法：它投影的是 `JobError::failover_disposition` 的
/// 用户可见边界。只有链路不可达时，文本/转写才会在首个可见输出前换到本机；嵌入的
/// 向量空间是索引身份，绝不能把一次云端请求悄悄改写成本地索引结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalCandidatePolicy {
    /// `ProviderUnavailable` / `Timeout` 且尚无可见输出时，允许从服务端切到本机。
    UnreachableBeforeVisibleOutput,
    /// 该能力只接受显式选择；不在服务端失败时自动改写执行位置。
    ExplicitOnly,
}

/// 本机候选的实际就绪结论。前端只渲染此枚举，不由模型名或 Provider 前缀反推。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalCandidateReadiness {
    Ready,
    ModelMissing,
    RuntimeUnavailable,
    NotApplicable,
}

/// 一条能力在服务端路径上的本机候选及下一步动作（ADR-038 / ADR-085）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LocalCandidateDto {
    /// `chat` / `embedding` / `transcription`。与 [`ComputeRouteDto`] 共用稳定能力名。
    pub capability: String,
    /// 自动兜底或显式选择的边界。
    pub policy: LocalCandidatePolicy,
    /// 已解析的本机候选展示名；没有候选时为空，绝不打印内部 target id。
    pub model_name: Option<String>,
    /// 真实就绪结论及不可用原因。
    pub readiness: LocalCandidateReadiness,
    /// 仅转写候选有意义：它是否为随包 tiny；其他能力恒为 `false`。
    pub is_minimal: bool,
    /// 本条能力的模型设置入口；嵌入也只允许从这里显式配置。
    pub setting_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectionMode {
    Model,
    Managed,
    Profile,
    Channel,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComputeServiceDto {
    pub id: String,
    pub name: String,
    /// `ready` / `unavailable`。
    pub status: String,
    pub managed_by: String,
    /// 网络服务的实际端点；进程内运行时为空。
    pub endpoint: Option<String>,
    pub model_count: u32,
    pub latency_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelManagementSnapshotDto {
    pub models: Vec<ModelEntryDto>,
    pub assignments: Vec<ModelAssignmentDto>,
    pub storage: StorageInfoDto,
    pub services: Vec<ComputeServiceDto>,
    /// 计算服务端点已配置时非空。为空 = 全本地（绝大多数用户的路径）。
    pub remote_compute: Option<RemoteComputeDto>,
}

/// 计算服务端点接管 LLM 后的目录状态（PRD-17 私有化形态）。
///
/// 存在的理由：配了端点之后，本机那些**只会做 LLM**的模型既不再被调用、也删不得
/// （分配槽位还指着它们的名字），却仍占满整个目录页——用户看到的是一份与实际执行
/// 路径无关的清单。这里让「哪些被接管、藏了几个」成为 Rust 下发的事实，而不是前端
/// 按模型名去猜（ADR-003）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RemoteComputeDto {
    /// 端点地址（[`compute_endpoint_source`] 的解析结果）。展示用，便于用户核对打到了哪
    /// ——**给解析后的值而不是原始设置**：只填了组织服务端地址的用户，原值是空的。
    pub endpoint: String,
    /// 已安装或随包可用的本机模型数。标准云端投影隐藏它们，完整设置会原样显示。
    pub available_local_models: u32,
    /// 被端点接管的能力（前端原样展示，不在 TS 里再抄一份常量）。
    pub taken_over_capabilities: Vec<String>,
}

/// 走原生推理面时被服务端接管的文本能力。
///
/// **从 `CloudLlmProvider` 声明的那份派生，不在这里再抄一遍**：抄一份就是同一个进程
/// 里的第二份清单，漏改一处不会有任何东西变红，而表现是模型页说的接管范围与 Provider
/// 实际声明的能力对不上——用户读到「翻译由服务端执行」，翻译却在本机跑。
fn chat_shaped_capabilities() -> [&'static str; 4] {
    luna_provider::cloud_llm::served_capability_names()
}

/// 这个端点接管了哪些能力 —— **随生效分配变，不是一个常量**。
///
/// 判据是**能力 + 该能力的生效分配**，不是管理方（按管理方切会把只做嵌入的模型与只做
/// 对话的模型一起处理）：Lunarca 服务端 / 官方云接管未显式分配的能力；显式指向本机模型
/// 的仍在本机。`service:<model>` 已废止且判不兼容，不能再成为接管条件。
///
/// 判错就是「屏幕说的 ≠ 实际」。判据与 `refresh_embed_provider` 同源
/// （都问 `ComputeEndpoint::is_server`），各判一遍必然有一天分叉。
///
/// ASR **任何一档都不在列**，尽管远程 ASR 两条通道都已交付：它由**槽位**（`service:asr`）
/// 而不是端点接管，且本机 whisper 权重恒为断线兜底（ADR-038 ⑤）——藏掉它们等于把兜底
/// 模型从用户眼前拿走。嵌入没有这条兜底：同一时刻只有一个嵌入 Provider 在册。
#[must_use]
pub(crate) fn assignment_uses_native_service(
    settings: &Settings,
    slot: &str,
    target: &crate::cloud::ComputeEndpoint,
) -> bool {
    use crate::model_target::ModelTarget;
    if !target.is_server() {
        return false;
    }
    match configured_assignment(settings, slot)
        .as_deref()
        .and_then(ModelTarget::parse)
    {
        // 显式目标的命名空间就是执行位置；用户选了本机模型，连接着服务端也不能改写它。
        // 原生服务端不再接受客户端逐模型路由。旧的 `service:<model>` 不解释、不迁移；
        // 就绪判定会把它标为不兼容，用户清除后才回到服务端托管策略。
        Some(ModelTarget::Service(_) | ModelTarget::StoreEntry(_)) => false,
        // 没显式选过才沿用 ADR-037 的默认：连上服务端即使用服务端算力。
        None => true,
    }
}

pub(crate) fn taken_over_capabilities(
    settings: &Settings,
    target: &crate::cloud::ComputeEndpoint,
) -> Vec<&'static str> {
    match target {
        crate::cloud::ComputeEndpoint::OnDevice => Vec::new(),
        crate::cloud::ComputeEndpoint::LunarcaServer(_)
        | crate::cloud::ComputeEndpoint::OfficialCloud(_) => {
            let mut capabilities = Vec::new();
            if assignment_uses_native_service(settings, "default_llm", target) {
                capabilities.extend(chat_shaped_capabilities());
            }
            if assignment_uses_native_service(settings, "default_embedding", target) {
                capabilities.push("embed");
            }
            capabilities
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelInstallPlanDto {
    pub plan_id: String,
    pub model_id: String,
    pub name: String,
    /// 这次**还要**下载的字节：续传时已扣掉暂存区里断点的那部分。
    pub download_bytes: Option<f64>,
    /// 续传：暂存区里已下好的字节（`None` = 从头下载）。确认卡据此写「还需下载 / 已下」。
    pub resumed_bytes: Option<f64>,
    pub estimated_on_disk_bytes: Option<f64>,
    pub available_bytes: Option<f64>,
    pub remaining_after_bytes: Option<f64>,
    pub license_label: String,
    pub strict_local_blocked: bool,
    pub can_start: bool,
    pub blocking_reason: Option<String>,
    /// 计划时运行时摘要；执行前用于识别目录已变化。LunaManaged 可为空。
    pub runtime_digest: Option<String>,
    /// 这是一次**更新**：下完、通过核对后替换已装的旧版本（ADR-145 ④：从不静默替换，
    /// 这张确认卡就是那次确认）。
    pub replaces_installed: bool,
    /// 更新的是嵌入模型：换了权重，向量空间就变了，库内索引要全量重建一次（ADR-027）。
    pub requires_reindex: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelDeletePlanDto {
    pub plan_id: String,
    pub model_id: String,
    pub name: String,
    pub release_bytes: Option<f64>,
    pub blocking_slots: Vec<String>,
    pub permanent: bool,
    pub can_delete: bool,
    pub blocking_reason: Option<String>,
    pub runtime_digest: Option<String>,
}

fn assignment_source(settings: &Settings, slot: &str) -> &'static str {
    if configured_assignment(settings, slot).is_some() {
        "explicit"
    } else if assignment_meta(slot)
        .and_then(|(_, inherits, _)| inherits)
        .is_some()
    {
        "inherited"
    } else {
        "device_policy"
    }
}

/// 「服务端转写」这一档的保留分配目标。
///
/// **是一个保留 id 而不是某个模型名**：服务端跑哪个模型由部署方决定、随时可换，
/// 客户端把它钉成具体模型就等于把运营决定写死在设备配置里。用户在这里选的是
/// **「交给谁做」**，不是「用哪个权重」。
///
/// 前缀 `service:` 与模型 id 的命名空间（目录条目 id / `whisper-*`）不重叠，故老配置
/// 不可能意外落进这一档；反过来，认不出这个前缀的旧客户端会把它当成"模型缺失"
/// 而如实报 `missing`——诚实降级，不是静默走本地。
pub(crate) const REMOTE_ASR_TARGET: &str = "service:asr";

/// 该分配目标是否指向服务端转写。
#[must_use]
pub(crate) fn is_remote_asr_target(target: &str) -> bool {
    target == REMOTE_ASR_TARGET
}

/// 服务端转写此刻可用吗（供候选与就绪判定共用——两处分开写必然漂）。
///
/// 判据只有一条：**开关开着**。不查登录态：未登录时 Provider 自己会以「需要登录」
/// 收尾，那是可执行的提示；而把它做成"没登录就看不见这个选项"，用户连"原来可以用
/// 服务端"都不知道。
#[must_use]
pub(crate) fn remote_asr_available(settings: &Settings) -> bool {
    settings
        .get_bool("provider.remote_asr_enabled")
        .unwrap_or(false)
}

/// 「优先在哪运行」的唯一读点：设置串 → 策略；认不出的值按默认。
#[must_use]
pub(crate) fn route_strategy(settings: &Settings) -> luna_provider::RouteStrategy {
    settings
        .get_str("ai.route_strategy")
        .and_then(|value| luna_provider::RouteStrategy::parse(&value))
        .unwrap_or_default()
}

/// 转写此刻允许交给服务端：同意开着、没开严格本地、策略不是仅本机。
///
/// 三个闸各自独立，判定只在这一处：装配（`device_policy_target`）、就绪判定、候选表与
/// 算力卡都问它，否则会出现「界面说服务端转写、路由却把那条腿拦掉」。
#[must_use]
pub(crate) fn server_transcription_allowed(settings: &Settings) -> bool {
    remote_asr_available(settings)
        && !settings.strict_local()
        && route_strategy(settings) != luna_provider::RouteStrategy::LocalOnly
}

/// 默认档 LLM 该打哪个端点（见 [`crate::cloud::ComputeEndpoint`]）。
///
/// 登录了哪台服务器，AI 就由谁提供；未登录或严格本地全在本机（ADR-155）。地址是云客户端
/// 派生出来的事实（[`crate::cloud::ServerRef`]），这里不自己解析——对话、账户、目录、制品、
/// 转写打的必须是同一台，两处解析迟早分叉成「卡片写着 A、字节发往 B」。
#[must_use]
pub(crate) fn compute_endpoint_source(
    availability: crate::cloud::CloudAvailability,
    server: &crate::cloud::ServerRef,
) -> crate::cloud::ComputeEndpoint {
    use crate::cloud::ComputeEndpoint;
    // 严格本地与未登录：**没有**远端端点可言，对话回到本机随包引擎。
    //
    // 与 ADR-038 ④ 修订同一条纪律——「路由按 location 过滤」只是两半里的一半，装配
    // 那一半得在这里成立：严格本地下若仍按远端端点构造 Provider，随后被路由拦掉，而本机
    // 那一个从来没被注册过——对话直接无候选可用（真机坐实 2026-08-08）。
    if !availability.allowed() {
        return ComputeEndpoint::OnDevice;
    }
    if server.official {
        ComputeEndpoint::OfficialCloud(server.origin.clone())
    } else {
        ComputeEndpoint::LunarcaServer(server.origin.clone())
    }
}

pub(crate) fn assignment_readiness(
    store: &ModelStore,
    slot: &str,
    target: Option<&str>,
    required_capability: &str,
    settings: &Settings,
) -> &'static str {
    let Some(target) = target else {
        return "unresolved";
    };
    if is_remote_asr_target(target) {
        // **只有两个转写槽位能用服务端目标**。
        //
        // 不限槽位的话，LLM/嵌入槽位指向 `service:asr` 时会报 `ready`——界面说"就绪"、
        // 一用就失败，是最伤的一种误报：用户以为配好了，直到真正要用的那一刻才发现没有。
        // 候选表里不给这个选项是第一道；这条是第二道（手工改配置也拦得住）。
        if !matches!(slot, "realtime_asr" | "batch_asr") {
            return "incompatible";
        }
        // 严格本地或「仅本机」与「交给服务端做」直接矛盾。报 `incompatible` 而不是
        // 悄悄按本地跑：后者会让设置页一直显示「服务端转写」，而设备上从来没走过那条路。
        if settings.strict_local()
            || route_strategy(settings) == luna_provider::RouteStrategy::LocalOnly
        {
            return "incompatible";
        }
        // 服务端目标不查本地模型库——那里当然没有它。
        // 开关关掉后仍留着这个分配值时报 `missing`：如实说「你选了服务端但它没开」，
        // 而不是悄悄回落本地（用户会以为音频没出设备，实际取决于我们悄悄选了谁）。
        return if remote_asr_available(settings) {
            "ready"
        } else {
            "missing"
        };
    }
    // 服务端**模型**目标已由服务端托管路由取代。这里不兼容、不迁移、不兜底解释；
    // `service:asr` 已在上面的专用通道分支返回。
    if let Some(crate::model_target::ModelTarget::Service(_)) =
        crate::model_target::ModelTarget::parse(target)
    {
        return "incompatible";
    }
    let Some(entry) = store.entry(target) else {
        return "missing";
    };
    if entry.status != "installed" && entry.status != "bundled" {
        return "missing";
    }
    if !entry
        .effective_capabilities
        .iter()
        .any(|capability| capability == required_capability)
    {
        return "incompatible";
    }
    if slot == "default_embedding" && !embed::assignment_target_supported(target) {
        return "profile_unverified";
    }
    "ready"
}

/// 服务端托管档的**注册表事实**（就绪判定的输入）。
///
/// 托管档此前无条件报 `ready`——它只看设置，不看装配结论。嵌入模型没有服务端档案时
/// 装配什么都不装，任务必然失败，而设置页写着「可用」（真机第八轮）。就绪必须与执行同源：
/// 执行按注册表路由，就绪就问注册表。
#[derive(Clone, Copy)]
pub(crate) struct ServerManagedFacts {
    /// 云端嵌入 Provider 此刻在注册表里。
    pub(crate) cloud_embed_registered: bool,
}

impl ServerManagedFacts {
    pub(crate) fn snapshot(state: &AppState) -> Self {
        Self {
            cloud_embed_registered: state
                .provider_registry
                .get(&luna_core::ProviderId::from(
                    luna_provider::CLOUD_EMBED_PROVIDER_ID,
                ))
                .is_some(),
        }
    }

    /// 「端点未托管」（`native_managed=false`，托管档根本不出现）的调用点与测试基线用：
    /// 事实取全绿——它不会被读到，读到也只该是全绿。
    pub(crate) const fn all_clear() -> Self {
        Self {
            cloud_embed_registered: true,
        }
    }
}

pub(crate) fn model_assignment_dtos(
    settings: &Settings,
    store: &ModelStore,
    installed: &[crate::local_engine::EngineModelRef],
    native_managed: bool,
    server_facts: ServerManagedFacts,
) -> Vec<ModelAssignmentDto> {
    let entries = store.list();
    MODEL_ASSIGNMENT_SLOTS
        .iter()
        .map(|(slot, _, parent, required_capability)| {
            let configured_target = configured_assignment(settings, slot);
            // 清掉本槽 → 落到服务端托管吗？= 端点原生托管 且 清掉本槽后继承链不再有本机
            // 显式立场（对话/嵌入三槽才有「服务端托管」这一模式；转写走独立的「服务端转写」
            // 通道，不在此列）。前端据此把空选项如实标成「服务端托管」，而不是泛化的「跟随」。
            let follow_is_server_managed = native_managed
                && matches!(*slot, "default_llm" | "realtime_llm" | "default_embedding")
                && parent.is_none_or(|ancestor| !assignment_chain_configured(settings, ancestor));
            // 判的是**整条继承链**而不是本槽的空值：实时 LLM 未配置而默认 LLM 显式
            // 指向本机时，实时任务继承的是那个本机模型，标成「服务端托管」就是屏幕与
            // 字节去向相反（`assignment_chain_configured` 的注释）。
            let server_selection = if native_managed && !assignment_chain_configured(settings, slot)
            {
                match *slot {
                    "default_llm" | "realtime_llm" => Some(ModelSelectionMode::Managed),
                    "default_embedding" => Some(ModelSelectionMode::Profile),
                    _ => None,
                }
            } else {
                None
            };
            let effective_target = if server_selection.is_some() {
                None
            } else {
                effective_assignment_target_with(settings, installed, slot)
            };
            let candidates = entries
                .iter()
                .filter(|entry| {
                    (entry.status == "installed" || entry.status == "bundled")
                        && entry
                            .effective_capabilities
                            .iter()
                            .any(|capability| capability == required_capability)
                        && (*slot != "default_embedding"
                            || embed::assignment_target_supported(&entry.id))
                })
                .map(|entry| ModelAssignmentCandidateDto {
                    target_id: entry.id.clone(),
                    name: entry.name.clone(),
                    location: ModelExecutionLocation::Device,
                })
                .collect::<Vec<_>>();
            // 两个转写槽位都给这一档：实时走 ADR-038 的双向流式通道，离线走 ADR-036
            // 的上传+任务通道。两条是不同的服务端端点，但对用户是同一个选择——
            // 「这件事交给服务端做」。
            let mut candidates = candidates;
            if matches!(*slot, "realtime_asr" | "batch_asr")
                && server_transcription_allowed(settings)
            {
                candidates.push(ModelAssignmentCandidateDto {
                    target_id: REMOTE_ASR_TARGET.to_owned(),
                    // 空名不是遗漏：服务端托管的目标**没有客户端知道的模型名**
                    // （ADR-048：具体模型由服务端选，客户端不得列云端模型目录）。此处
                    // 此前写死了中文「服务端转写」——那既是把文案钉进 Rust（英文界面
                    // 照样显示中文），也是同一个状态的第四个名字（ADR-059 ④）。
                    name: String::new(),
                    location: ModelExecutionLocation::Server,
                });
            }
            let effective_name = effective_target.as_deref().and_then(|target| {
                if is_remote_asr_target(target) {
                    // 同上：名字是术语不是数据，由前端按 `selection_mode` 取 i18n。
                    return None;
                }
                store.entry(target).map(|entry| entry.name)
            });
            let selection_mode = if let Some(mode) = server_selection {
                mode
            } else if effective_target
                .as_deref()
                .is_some_and(is_remote_asr_target)
            {
                ModelSelectionMode::Channel
            } else {
                ModelSelectionMode::Model
            };
            ModelAssignmentDto {
                slot: (*slot).to_owned(),
                setting_key: format!("ai.assignment.{slot}"),
                source: assignment_source(settings, slot).to_owned(),
                // 托管两档（Managed/Profile）问注册表事实，不再无条件 `ready`
                // （`ServerManagedFacts` 头注）；其余档维持模型库判定。
                readiness: match &selection_mode {
                    ModelSelectionMode::Managed => "ready",
                    ModelSelectionMode::Profile => {
                        if server_facts.cloud_embed_registered {
                            "ready"
                        } else {
                            "incompatible"
                        }
                    }
                    _ => assignment_readiness(
                        store,
                        slot,
                        effective_target.as_deref(),
                        required_capability,
                        settings,
                    ),
                }
                .to_owned(),
                configured_target,
                effective_target,
                effective_name,
                required_capability: (*required_capability).to_owned(),
                selection_mode,
                follow_is_server_managed,
                candidates,
            }
        })
        .collect()
}

fn assignment_slots_by_target(assignments: &[ModelAssignmentDto]) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::new();
    for assignment in assignments {
        if assignment.readiness == "ready" {
            if let Some(target) = assignment.effective_target.as_ref() {
                result
                    .entry(target.clone())
                    .or_insert_with(Vec::new)
                    .push(assignment.slot.clone());
            }
        }
    }
    result
}

/// 「AI 在哪跑」的一次性快照——设置首屏与首启引导**共用同一份事实**。
///
/// 由 Rust 一次算清，而不是前端拿四个设置项自己拼（ADR-003 前端零业务）。
/// 此前「算力在哪」没有名字，谁想知道就各组合一遍 `strict_local` + 登录态 +
/// `api_base` + 转写开关——于是同一件事在不同界面上说法不同，而没有任何东西会变红。
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct ComputeStatusDto {
    /// 算力此刻落在哪。
    pub location: crate::cloud::ComputeLocation,
    /// 三条能力此刻的**有效执行位置 + 为什么是它 + 从哪儿改**（ADR-059 ④）。
    ///
    /// 取代此前的三个布尔（`chat_on_server` / `embedding_on_server` /
    /// `transcription_on_server`）。布尔只说得出「在不在服务端」，而同一个「在这台设备」
    /// 背后有四种互不相同的原因与四种不同的下一步动作；用户于是只能逐个槽位去翻设置，
    /// 才能发现是自己某次钉的。同一事实不留两种表示（ADR-046）。
    pub routes: Vec<ComputeRouteDto>,
    /// 服务端路径可用时的本机候选。`chat` / `transcription` 可在严格边界内兜底，
    /// `embedding` 明确为 `explicit_only`，避免把云端嵌入透明改写为本地索引结果。
    pub local_candidates: Vec<LocalCandidateDto>,
    /// 仍显式钉在本机、而服务端此刻跑得了的能力（一次性交接提示的对象）。
    ///
    /// 空 = 不提示：要么没有这种槽位，要么这台服务端已经问过一次（`compute.handover_prompted_for`）。
    pub pending_handover: Vec<String>,
    /// `managed_service` / `on_device`。
    /// 前端据此选择展示文案，不得再从本机模型目录推断当前执行路径。
    pub chat_route_kind: ChatRouteKind,
    /// 服务端转写此刻为什么用不了；`None` = 能用，同意项可以勾。
    ///
    /// 结论连同原因一起下发：界面按原因换说明，不拿 `signed_in` + `location` 自己拼（ADR-003）。
    pub transcription_server_blocker: Option<TranscriptionServerBlocker>,
    /// 「优先在哪运行」设成了仅本机：连着服务端也不外发。卡头据此换说法。
    pub outbound_blocked_by_strategy: bool,
    /// 连着的服务端一个上游都没登记（ADR-041 ①）：卡头如实说「尚未配置算力，在本机运行」，
    /// 而不是「请求发往服务端」。
    pub server_setup_incomplete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChatRouteKind {
    ManagedService,
    OnDevice,
}

/// 服务端转写此刻用不了的原因（见 [`ComputeStatusDto::transcription_server_blocker`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionServerBlocker {
    /// 没有服务端可言（未登录 / 严格本地）。
    NoServer,
    /// 「优先在哪运行」设成了仅本机。
    LocalOnlyStrategy,
    /// 这台服务端没给转写配上游（ADR-041 ①）。
    ServerNotConfigured,
}

/// 此刻**对话能力**的真实算力落点。端点只说明连接到了哪里，不能覆盖显式指向本机模型的
/// 分配；上下文窗、模型推荐与本机就绪提醒都必须问这一个事实。
pub(crate) fn current_chat_compute_location(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
) -> crate::cloud::ComputeLocation {
    let settings = state.settings_snapshot();
    let availability = crate::cloud::evaluate_for(app);
    chat_compute_location(&settings, availability, &state.cloud_server())
}

#[must_use]
pub(crate) fn chat_compute_location(
    settings: &Settings,
    availability: crate::cloud::CloudAvailability,
    server: &crate::cloud::ServerRef,
) -> crate::cloud::ComputeLocation {
    let endpoint = compute_endpoint_source(availability, server);
    let endpoint_location = crate::cloud::compute_location(availability, &endpoint);
    if endpoint.is_server() && !assignment_uses_native_service(settings, "default_llm", &endpoint) {
        crate::cloud::ComputeLocation::OnDevice
    } else {
        endpoint_location
    }
}

/// 一条能力此刻的执行位置与理由（ADR-059 ④）。
///
/// 判据全部复用既有的**同一条**判定链——`strict_local` 硬闸、`configured_assignment`
/// 显式立场、`is_server` 端点形态、`remote_asr_available` 转写同意。这里
/// 只是把「已经算出来的答案」翻译成一句用户读得懂的理由，不引入第二套判定。
/// 对话 / 改写能力此刻是否交给服务端跑。
///
/// 与 [`compute_status`] 是**同一份判据**：设置页的「执行位置」卡片与笔记 AI 悬浮菜单
/// 里的「本机 · 不联网 / 云」标记如果各算一遍，迟早有一天两处说不一样的话。
pub(crate) fn chat_runs_on_server(
    settings: &Settings,
    endpoint: &crate::cloud::ComputeEndpoint,
) -> bool {
    assignment_uses_native_service(settings, "default_llm", endpoint)
}

/// 一项文本生成能力（对话 / 摘要 / 改写…）此刻是不是交给服务端跑——设置页算力卡与任务前预估
/// （PRD-13-040 ②，`ipc::estimate`）问的同一个问题，判据只此一份。
///
/// 顺序同 Router：端点是服务端且默认档用原生服务；策略「仅本机」拦下；本机优先且本机有对话腿时
/// 留在本机；账户所在组织没开放这项（R4b）时回本机。
pub(crate) fn llm_leg_on_server(
    settings: &Settings,
    endpoint: &crate::cloud::ComputeEndpoint,
    local_chat_leg: bool,
    server_serves: bool,
) -> bool {
    let strategy = route_strategy(settings);
    chat_runs_on_server(settings, endpoint)
        && strategy != luna_provider::RouteStrategy::LocalOnly
        && !(strategy.prefers_local() && local_chat_leg)
        && server_serves
}

/// 同 [`llm_leg_on_server`]，从运行态取齐四个输入（门控、端点、本机对话腿、组织开放清单）。
pub(crate) fn llm_capability_on_server(
    app: &tauri::AppHandle,
    state: &AppState,
    capability: &str,
) -> bool {
    let settings = state.settings_snapshot();
    let availability = crate::cloud::evaluate_for(app);
    let endpoint = compute_endpoint_source(availability, &state.cloud_server());
    let local_chat_leg =
        crate::ipc::local_llm_for_slot(&settings, &state.engine_models_snapshot(), "default_llm")
            .is_some();
    llm_leg_on_server(
        &settings,
        &endpoint,
        local_chat_leg,
        state.server_serves(capability),
    )
}

fn compute_route(
    settings: &Settings,
    endpoint: &crate::cloud::ComputeEndpoint,
    capability: &str,
    slot: &str,
    on_server: bool,
    local_leg: bool,
    offer: crate::ipc::ServerOffer,
) -> ComputeRouteDto {
    use crate::ipc::ServerOffer;
    use luna_provider::RouteStrategy;
    // 转写另需 ADR-038 的单独同意，那由 `on_server` 回答；这里只问端点是不是服务端、
    // 以及服务端肯不肯跑这项能力（部署配没配上游 ADR-041 ①、组织开没开放 R4b）。
    let server_can_serve = endpoint.is_server() && offer == ServerOffer::Serves;
    let strategy = route_strategy(settings);
    let explicit = configured_assignment(settings, slot);
    // 顺序即优先级，与 Router 一致：隐私闸 > 显式分配 > 路由策略 > 转写同意 > 连上即用。
    // 策略只驱动对话：嵌入同一时刻只装一个 Provider，转写由同意决定；仅本机则三条都拦。
    let reason = if settings.strict_local() {
        ComputeRouteReason::StrictLocal
    } else if endpoint.is_server() && offer == ServerOffer::NotConfigured {
        ComputeRouteReason::ServerNotConfigured
    } else if endpoint.is_server() && offer == ServerOffer::NotAllowed {
        ComputeRouteReason::ServerNotAllowed
    } else if on_server {
        if explicit.is_some() {
            ComputeRouteReason::ExplicitServer
        } else if capability == "chat" && strategy.prefers_local() && !local_leg {
            ComputeRouteReason::StrategyLocalUnavailable
        } else {
            ComputeRouteReason::ServerDefault
        }
    } else if explicit.is_some() {
        ComputeRouteReason::ExplicitDevice
    } else if strategy == RouteStrategy::LocalOnly && server_can_serve {
        ComputeRouteReason::StrategyLocalOnly
    } else if capability == "chat" && strategy.prefers_local() && server_can_serve {
        ComputeRouteReason::StrategyLocal
    } else if capability == "transcription" && server_can_serve {
        // 转写没上服务端、又没有显式本机选择，唯一的原因就是那次同意还没给。
        ComputeRouteReason::NeedsConsent
    } else if server_can_serve {
        ComputeRouteReason::ServerDefault
    } else if matches!(endpoint, crate::cloud::ComputeEndpoint::OnDevice) {
        ComputeRouteReason::DevicePolicy
    } else {
        ComputeRouteReason::ServerUnavailable
    };
    ComputeRouteDto {
        capability: capability.to_owned(),
        location: if on_server {
            ModelExecutionLocation::Server
        } else {
            ModelExecutionLocation::Device
        },
        reason,
        setting_key: format!("ai.assignment.{slot}"),
        can_use_server: server_can_serve
            && !settings.strict_local()
            && strategy != RouteStrategy::LocalOnly,
    }
}

fn local_chat_candidate(
    settings: &Settings,
    engine_tripped: bool,
    store: &ModelStore,
    target: Option<String>,
) -> LocalCandidateDto {
    let model_name = target
        .as_deref()
        .and_then(|target| store.entry(target))
        .map(|entry| entry.name);
    let readiness =
        match assignment_readiness(store, "default_llm", target.as_deref(), "chat", settings) {
            // 模型装着、引擎却熔断了：不是「未安装」——把它说成缺模型会把用户带去重新下载，
            // 而真正的动作是看引擎为什么起不来（本地模型页「本地推理引擎」一节有原因与重启）。
            "ready" if engine_tripped => LocalCandidateReadiness::RuntimeUnavailable,
            "ready" => LocalCandidateReadiness::Ready,
            _ => LocalCandidateReadiness::ModelMissing,
        };
    LocalCandidateDto {
        capability: "chat".to_owned(),
        policy: LocalCandidatePolicy::UnreachableBeforeVisibleOutput,
        model_name,
        readiness,
        is_minimal: false,
        setting_key: "ai.assignment.default_llm".to_owned(),
    }
}

fn local_transcription_candidate(
    local_weight: Option<&std::path::Path>,
    is_minimal: bool,
) -> LocalCandidateDto {
    LocalCandidateDto {
        capability: "transcription".to_owned(),
        policy: LocalCandidatePolicy::UnreachableBeforeVisibleOutput,
        model_name: local_weight.map(crate::ipc::processing::model_display_name),
        readiness: if local_weight.is_some() {
            LocalCandidateReadiness::Ready
        } else {
            LocalCandidateReadiness::ModelMissing
        },
        is_minimal,
        setting_key: "ai.assignment.realtime_asr".to_owned(),
    }
}

fn local_embedding_candidate(
    settings: &Settings,
    endpoint: &crate::cloud::ComputeEndpoint,
    engine_tripped: bool,
    store: &ModelStore,
    target: Option<String>,
) -> LocalCandidateDto {
    // 嵌入不参与自动换腿。本机事实有两种：用户显式钉在本机，或者根本没有服务端
    // 于是按设备策略在本机跑。有原生服务端而没显式选时，空配置由
    // 服务端 profile 接管——不能因为设备策略能给出 bge-m3 就把它说成「本机已配置」。
    let explicit_target = configured_assignment(settings, "default_embedding");
    let is_explicit_local = explicit_target
        .as_deref()
        .and_then(ModelTarget::parse)
        .is_some_and(|target| matches!(target, ModelTarget::StoreEntry(_)));
    if !is_explicit_local && endpoint.is_server() {
        return LocalCandidateDto {
            capability: "embedding".to_owned(),
            policy: LocalCandidatePolicy::ExplicitOnly,
            model_name: None,
            readiness: LocalCandidateReadiness::NotApplicable,
            is_minimal: false,
            setting_key: "ai.assignment.default_embedding".to_owned(),
        };
    }

    let model_name = target
        .as_deref()
        .and_then(|target| store.entry(target))
        .map(|entry| entry.name);
    let readiness = match assignment_readiness(
        store,
        "default_embedding",
        target.as_deref(),
        "embed",
        settings,
    ) {
        // 同对话：装着而引擎熔断 = 运行时不可用，不是没装。
        "ready" if engine_tripped => LocalCandidateReadiness::RuntimeUnavailable,
        "ready" => LocalCandidateReadiness::Ready,
        _ => LocalCandidateReadiness::ModelMissing,
    };
    LocalCandidateDto {
        capability: "embedding".to_owned(),
        policy: LocalCandidatePolicy::ExplicitOnly,
        model_name,
        readiness,
        is_minimal: false,
        setting_key: "ai.assignment.default_embedding".to_owned(),
    }
}

/// 设置首屏与首启引导共用的算力状态。
#[tauri::command]
#[specta::specta]
pub(crate) fn compute_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ComputeStatusDto, String> {
    let settings = state.settings_snapshot();
    // 门控与状态共用同一套判据——各算一遍必然有一天两处说不一样的话。
    let availability = crate::cloud::evaluate_for(&app);
    let endpoint = compute_endpoint_source(availability, &state.cloud_server());
    let strategy = route_strategy(&settings);
    let local_only = strategy == luna_provider::RouteStrategy::LocalOnly;
    // 本机对话腿存不存在，与 `register_llm_provider` 同一判据：有效分配解析得出本机对话目标。
    // 有这条腿时本机优先的策略才改得了去向；没有它，Router 手里只有服务端那一个候选。
    let local_chat_leg =
        crate::ipc::local_llm_for_slot(&settings, &state.engine_models_snapshot(), "default_llm")
            .is_some();
    // 账户所在组织开放了哪几项（R4b）：没开放的一律在本机跑，与 Provider 装配同一份清单。
    let chat_offer = state.server_offer("chat");
    let embed_offer = state.server_offer("embed");
    let transcribe_offer = state.server_offer("transcribe");
    let serves_chat = chat_offer == crate::ipc::ServerOffer::Serves;
    let serves_embed = embed_offer == crate::ipc::ServerOffer::Serves;
    let serves_transcribe = transcribe_offer == crate::ipc::ServerOffer::Serves;
    let chat_on_server = llm_leg_on_server(&settings, &endpoint, local_chat_leg, serves_chat);
    let embedding_on_server =
        assignment_uses_native_service(&settings, "default_embedding", &endpoint)
            && !local_only
            && serves_embed;
    let location = crate::cloud::compute_location(availability, &endpoint);
    // 兜底模型问的是「装配**会**用哪一个」，不是「此刻装了哪一个」——故直接问装配
    // 的输入（`desired_stream_asr_shape`，纯函数），而不是问已装的 Provider。ASR 是懒加载
    // 的：冷启动到首次转写之间它恒为 `None`，问它会把随包 tiny 说成用户的兜底档，录一次音
    // 之后说法又变。服务端形态下退到随包模型这一档也在那个函数里（ADR-038 ⑤），
    // 在这里再写一遍就是第二处判据，迟早与装配分叉。
    let root = state.models_root(&app);
    let bundled = luna_managed_weight_path(&root, crate::model_store::BUNDLED_WHISPER_ID);
    let local_weight = state.desired_stream_asr_shape(&settings, &root).local;
    // `compute_status` 不主动探测/联网；运行态用最近一次启动或模型页探测的快照。这既不把
    // 设置渲染变成网络动作，也保留「问不到运行时」与「确认一个都没装」的事实差别。
    let local_store = ModelStore::load(root.clone());
    let engine_tripped = state.local_engine_tripped();
    let local_candidates = vec![
        local_chat_candidate(
            &settings,
            engine_tripped,
            &local_store,
            state.effective_assignment(&settings, "default_llm"),
        ),
        local_embedding_candidate(
            &settings,
            &endpoint,
            engine_tripped,
            &local_store,
            state.effective_assignment(&settings, "default_embedding"),
        ),
        local_transcription_candidate(
            local_weight.as_deref(),
            local_weight.as_deref() == Some(bundled.as_path()),
        ),
    ];
    let transcription_on_server =
        location.is_server() && server_transcription_allowed(&settings) && serves_transcribe;
    let chat_route = compute_route(
        &settings,
        &endpoint,
        "chat",
        "default_llm",
        chat_on_server,
        local_chat_leg,
        chat_offer,
    );
    let routes = vec![
        chat_route,
        compute_route(
            &settings,
            &endpoint,
            "embedding",
            "default_embedding",
            embedding_on_server,
            false,
            embed_offer,
        ),
        compute_route(
            &settings,
            &endpoint,
            "transcription",
            "realtime_asr",
            transcription_on_server,
            local_weight.is_some(),
            transcribe_offer,
        ),
    ];
    // **仍钉在本机、而服务端此刻跑得了**的那几条 = 一次性批量交接提示的对象。
    //
    // ADR-037「连上即用」与 ADR-058「显式 pin 被尊重」这两条同时成立时，会留下一个空档：
    // 用户连上服务端，而某几个槽位仍指着本机——这是**对的**（我们不静默改他的显式选择），
    // 但他多半并不知道还有这几项没交出去，只能逐槽去发现。同行做法是连上那一刻问一次
    // 「检测到 N 项仍指向本机，要交给服务端吗」，给一键切换与保留两个出口。此处只算
    // 「该问什么」，问没问过由 `compute.handover_prompted_for` 记（内部键）。
    let prompted_for = settings
        .get_str("compute.handover_prompted_for")
        .unwrap_or_default();
    let bucket = if endpoint.is_server() {
        luna_cloud::config::server_bucket(endpoint.url())
    } else {
        String::new()
    };
    let pending_handover = if bucket.is_empty() || prompted_for == bucket {
        Vec::new()
    } else {
        routes
            .iter()
            .filter(|route| {
                route.reason == ComputeRouteReason::ExplicitDevice && route.can_use_server
            })
            .map(|route| route.capability.clone())
            .collect()
    };
    Ok(ComputeStatusDto {
        location,
        routes,
        local_candidates,
        pending_handover,
        chat_route_kind: if chat_on_server {
            ChatRouteKind::ManagedService
        } else {
            ChatRouteKind::OnDevice
        },
        // 账户会话是转写这条路的凭据来源（`CloudBearer`）；没有它，勾上也只会拿到
        // `not_logged_in`。判据落在 Rust 是因为「凭据从哪来」是装配知识，不是界面知识。
        transcription_server_blocker: if !location.is_server() {
            Some(TranscriptionServerBlocker::NoServer)
        } else if local_only {
            Some(TranscriptionServerBlocker::LocalOnlyStrategy)
        } else if transcribe_offer == crate::ipc::ServerOffer::NotConfigured {
            Some(TranscriptionServerBlocker::ServerNotConfigured)
        } else {
            None
        },
        outbound_blocked_by_strategy: local_only,
        server_setup_incomplete: location.is_server() && state.server_setup_incomplete(),
    })
}

/// 「试跑一次」的结果（ADR-126 第 8 条）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SelfCheckResultDto {
    /// 跑通了没有。
    pub ok: bool,
    /// **实际**执行者的展示名（「Lunarca 云」/「本地 Whisper」…）。
    ///
    /// 由 Rust 从 Provider 描述符取，前端不从 id 猜——这一格的全部价值就是「说的那台
    /// 与真跑的那台是不是同一台」，让前端去翻译 id 等于把判据交回给猜。
    pub executed_by: Option<String>,
    /// 这一次在哪跑的（`device` / `local_service` / `cloud`）。与算力卡三格同一组取值。
    pub location: Option<String>,
    /// 端到端耗时（毫秒）——含排队。它回答的是「这条路此刻有多快」。
    pub duration_ms: u32,
    /// 失败的稳定错误码；成功恒为 `None`。前端翻成人话，**绝不裸显**（TDD-15 §4.3）。
    pub error_code: Option<String>,
}

/// 自检等多久算超时。
///
/// 端点不可达时 Provider 自己会超时收尾，但那条超时按「一次真实任务」定，可能长达一分钟；
/// 用户点的是一颗「试一下」，等到那时他早已认定这颗按钮坏了。超时本身也是一个答案。
const SELF_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 「试跑一次」：走**同一条主干**发一个最小真实任务，如实回报谁跑的、多久、失败在哪
/// （ADR-126 第 8 条）。
///
/// # 为什么是一次真任务而不是一次探测
///
/// 「连接服务端」那一步的「测试连接」探的是控制面（`/healthz` 那一档）——它答得了
/// 「这台机器在不在」，答不了「对话真的跑得通吗」：鉴权、额度、模型分配、本机运行时
/// 是否就绪，全都不在那条路上。配置到第一次真用之间此前没有任何验证点，而第一次真用
/// 往往发生在用户正要做事的时候。
///
/// **走 `AIProvider + JobQueue`**（ADR-001 不另起调用栈）：于是它与真实任务共享路由、
/// 凭据、降级与记账——自检通过意味着真任务也通过，这正是它存在的理由。代价是它会在
/// 执行记录里留下一条（`JobOrigin::SelfCheck`），那是**有意**的：用户能在账上看到自己
/// 点过这一次。
///
/// **只由用户显式点触发**（ADR-037 默认零云网络行为不变）：不在进入页面、切换落点或
/// 任何后台时机自动跑。
#[tauri::command]
#[specta::specta]
pub(crate) async fn compute_self_check(
    state: tauri::State<'_, AppState>,
) -> Result<SelfCheckResultDto, String> {
    use luna_core::{Job, JobEvent, JobKind, JobOrigin, Lang, TextRef};
    use luna_jobqueue::JobSink;

    let started = std::time::Instant::now();
    // 最小的一次真实文本任务：三种落点都跑得了，token 消耗可忽略。
    let job = Job::new(
        JobKind::Translate {
            text: TextRef::Inline("Hello".to_owned()),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        JobOrigin::SelfCheck,
    );
    let mut handle = state.queue_handle().submit(job);
    let registry = std::sync::Arc::clone(&state.provider_registry);
    let outcome = tokio::time::timeout(SELF_CHECK_TIMEOUT, async move {
        loop {
            let Some(event) = handle.recv().await else {
                // 通道提前关闭：队列已收尾而没有给出终态，这本身就是"没跑通"。
                return (false, None, None);
            };
            match event {
                JobEvent::Finished(result) => {
                    let descriptor = registry
                        .get(&result.provider_id)
                        .map(|provider| provider.descriptor());
                    return (
                        true,
                        descriptor.as_ref().map(|d| d.display_name.clone()),
                        descriptor.map(|d| d.location.as_str().to_owned()),
                    );
                }
                JobEvent::Failed(error) => return (false, Some(error.code().to_owned()), None),
                JobEvent::Cancelled => return (false, None, None),
                _ => {}
            }
        }
    })
    .await;

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    Ok(match outcome {
        Ok((true, executed_by, location)) => SelfCheckResultDto {
            ok: true,
            executed_by,
            location,
            duration_ms,
            error_code: None,
        },
        // 失败那一支把错误码放在第二格（见上面的 `JobEvent::Failed`）。
        Ok((false, error_code, _)) => SelfCheckResultDto {
            ok: false,
            executed_by: None,
            location: None,
            duration_ms,
            error_code,
        },
        Err(_) => SelfCheckResultDto {
            ok: false,
            executed_by: None,
            location: None,
            duration_ms,
            // 超时是一个答案，不是"没结果"：它与"端点拒绝"指向完全不同的下一步。
            error_code: Some("SELF_CHECK_TIMEOUT".to_owned()),
        },
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn list_models(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ModelEntryDto>, String> {
    let store = ModelStore::load(state.models_root(&app));
    let settings = state.settings_snapshot();
    let endpoint = compute_endpoint_source(crate::cloud::evaluate_for(&app), &state.cloud_server());
    let installed = state.engine_models_snapshot();
    let assignments = model_assignment_dtos(
        &settings,
        &store,
        &installed,
        endpoint.is_server(),
        ServerManagedFacts::snapshot(&state),
    );
    Ok(store.list_with_assignments(&assignment_slots_by_target(&assignments)))
}

/// 模型管理页的一致快照：目录、分配、存储与真实计算服务只探测一次。
#[tauri::command]
#[specta::specta]
pub(crate) async fn model_management_snapshot(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ModelManagementSnapshotDto, String> {
    let store = ModelStore::load(state.models_root(&app));
    let settings = state.settings_snapshot();
    let target = compute_endpoint_source(crate::cloud::evaluate_for(&app), &state.cloud_server());
    let installed = state.engine_models_snapshot();
    let assignments = model_assignment_dtos(
        &settings,
        &store,
        &installed,
        target.is_server(),
        ServerManagedFacts::snapshot(&state),
    );
    let mut models = store.list_with_assignments(&assignment_slots_by_target(&assignments));
    let location = current_chat_compute_location(&app, &state);
    let hardware = tokio::task::spawn_blocking(move || crate::hardware::probe(location))
        .await
        .map_err(|error| format!("设备能力探测任务失败：{error}"))?;
    // 每种落点都有本机 LLM 主力或最小兜底推荐；推荐清单不再用 `None` 隐藏云端用户的
    // 本地备用模型。
    let recommended = [
        hardware.recommended_asr_model_id.as_str(),
        hardware.recommended_llm_model.as_str(),
        DEVICE_POLICY_EMBED,
    ];
    for model in &mut models {
        let enough_memory = model
            .estimated_memory_bytes
            .map(|required| hardware.memory_bytes >= required);
        model.device_fit = match enough_memory {
            Some(false) => "insufficient",
            // 推荐清单里的条目都是模型库 id（`whisper-small-ggml` / `bge-m3-q8_0-gguf`），
            // id 全等即推荐。`service:` 目标不参与——推荐讲的是「这台设备适合装什么」，
            // 而服务端跑什么与本机硬件无关。
            Some(true) if recommended.iter().any(|target| *target == model.id) => "recommended",
            Some(true) => "supported",
            None => "unknown",
        }
        .to_owned();
    }
    let asr_model_count = models
        .iter()
        .filter(|model| {
            (model.status == "installed" || model.status == "bundled")
                && model
                    .effective_capabilities
                    .iter()
                    .any(|capability| capability == "transcribe")
        })
        .count();
    let asr_assignment_ready = assignments
        .iter()
        .find(|assignment| assignment.slot == "realtime_asr")
        .is_some_and(|assignment| assignment.readiness == "ready");
    let asr_provider_loaded = state
        .provider_registry
        .get(&luna_core::ProviderId::from(WHISPER_PROVIDER_ID))
        .is_some();
    let asr_status = if asr_provider_loaded {
        "ready"
    } else if asr_assignment_ready {
        "standby"
    } else {
        "unavailable"
    };
    // 端点接管能力后，快照仍保留本机目录；标准/完整投影只影响呈现，绝不改写目录事实。
    // 走**同一条**派生（`compute_endpoint_source`），不是再读一遍原始设置：
    // 读原值的话，「只填了组织服务端地址」的用户在这里会被判成「没配计算服务」，
    // 于是本地目录不隐藏、「计算服务」页也不出现——而对话其实早就打到服务端了。
    let taken_over = taken_over_capabilities(&settings, &target);
    let remote_compute = if taken_over.is_empty() {
        None
    } else {
        // 快照保留完整本机目录，标准投影由前端隐藏、完整设置原样展示；Rust 只下发
        // 「默认投影会藏多少个已可用模型」这一事实。这样不会为显示模式造第二份目录。
        let available_local_models = models
            .iter()
            .filter(|model| matches!(model.status.as_str(), "installed" | "bundled"))
            .count();
        Some(RemoteComputeDto {
            endpoint: target.url().to_owned(),
            available_local_models: u32::try_from(available_local_models).unwrap_or(u32::MAX),
            taken_over_capabilities: taken_over.iter().map(|c| (*c).to_owned()).collect(),
        })
    };
    Ok(ModelManagementSnapshotDto {
        storage: store.storage_info(),
        models,
        assignments,
        services: vec![ComputeServiceDto {
            id: "local_asr".to_owned(),
            name: "Lunarca ASR".to_owned(),
            status: asr_status.to_owned(),
            managed_by: "InProcess".to_owned(),
            endpoint: None,
            model_count: u32::try_from(asr_model_count).unwrap_or(u32::MAX),
            latency_ms: None,
        }],
        remote_compute,
    })
}

pub(crate) fn delete_plan_for(
    store: &ModelStore,
    settings: &Settings,
    installed: &[crate::local_engine::EngineModelRef],
    model_id: &str,
) -> Result<ModelDeletePlanDto, String> {
    let entry = store
        .entry(model_id)
        .ok_or_else(|| format!("MODEL_NOT_FOUND: {model_id}"))?;
    let assignments = model_assignment_dtos(
        settings,
        store,
        installed,
        false,
        ServerManagedFacts::all_clear(),
    );
    let blocking_slots = assignments
        .iter()
        .filter(|assignment| assignment.effective_target.as_deref() == Some(model_id))
        .map(|assignment| assignment.slot.clone())
        .collect::<Vec<_>>();
    let blocking_reason = if entry.status == "bundled" {
        Some("MODEL_BUNDLED_NOT_DELETABLE".to_owned())
    } else if entry.status != "installed" {
        Some("MODEL_NOT_INSTALLED".to_owned())
    } else if !blocking_slots.is_empty() {
        Some("MODEL_DELETE_BLOCKED_ASSIGNMENT".to_owned())
    } else {
        None
    };
    Ok(ModelDeletePlanDto {
        plan_id: uuid::Uuid::now_v7().to_string(),
        model_id: entry.id,
        name: entry.name,
        release_bytes: entry.on_disk_bytes.or(entry.est_on_disk_bytes),
        blocking_slots,
        permanent: true,
        can_delete: blocking_reason.is_none(),
        blocking_reason,
        runtime_digest: entry.runtime_digest,
    })
}

/// 只读删除计划；永久删除前必须先把释放空间与阻塞槽位呈现给用户。
#[tauri::command]
#[specta::specta]
pub(crate) async fn plan_delete_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<ModelDeletePlanDto, String> {
    let store = ModelStore::load(state.models_root(&app));
    let plan = delete_plan_for(
        &store,
        &state.settings_snapshot(),
        state.engine_models_snapshot().as_slice(),
        &model_id,
    )?;
    if plan.can_delete {
        let mut plans = state
            .model_delete_plans
            .lock()
            .map_err(|_| "模型删除计划锁中毒".to_owned())?;
        plans.retain(|_, existing| existing.model_id != plan.model_id);
        plans.insert(plan.plan_id.clone(), plan.clone());
    }
    Ok(plan)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn start_delete_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    plan_id: String,
) -> Result<(), String> {
    let planned = state
        .model_delete_plans
        .lock()
        .map_err(|_| "模型删除计划锁中毒".to_owned())?
        .remove(&plan_id)
        .ok_or_else(|| coded_error(&app, "MODEL_PLAN_NOT_FOUND", "model-plan-not-found-delete"))?;
    let store = ModelStore::load(state.models_root(&app));
    let current = delete_plan_for(
        &store,
        &state.settings_snapshot(),
        state.engine_models_snapshot().as_slice(),
        &planned.model_id,
    )?;
    if !current.can_delete
        || current.runtime_digest != planned.runtime_digest
        || current.release_bytes != planned.release_bytes
    {
        return Err(coded_error(
            &app,
            "MODEL_PLAN_STALE",
            "model-plan-stale-assignment",
        ));
    }
    execute_delete_model(&app, &state, &planned.model_id).await
}

async fn execute_delete_model(
    app: &tauri::AppHandle,
    state: &AppState,
    model_id: &str,
) -> Result<(), String> {
    // 两种命名空间各说各的（[`crate::model_target`]）：「删一个 `service:` 目标」要说清
    // **它压根不归本机管**，而不是去模型库里找一个不存在的条目、报「模型不存在」。
    match crate::model_target::ModelTarget::parse(model_id) {
        Some(crate::model_target::ModelTarget::Service(_)) => {
            return Err(coded_error(
                app,
                "MODEL_DELETE_UNSUPPORTED",
                "model-delete-server-managed",
            ));
        }
        // 裸 id = 模型库条目，落到下面的本地删除。
        Some(crate::model_target::ModelTarget::StoreEntry(_)) => {}
        None => {
            return Err(coded_error(
                app,
                "MODEL_DELETE_UNSUPPORTED",
                "model-name-empty",
            ))
        }
    }

    let mut store = ModelStore::load(state.models_root(app));
    store.delete(model_id).map_err(|e| e.to_string())?;
    // GGUF 条目被删 → 引擎按剩下的模型重新编排（没有了就停）。
    if remote_model_spec(model_id).is_some_and(|spec| spec.gguf.is_some())
        || model_id.starts_with(crate::model_import::IMPORTED_PREFIX)
    {
        crate::local_engine::spawn_reconcile(app);
    }
    // 细分模型被删 → 约定路径不再齐备时立即下线 Provider（诚实无此能力，与热注册对称）。
    if remote_model_spec(model_id)
        .is_some_and(|spec| spec.task == crate::model_store::Task::Diarize)
        && !crate::diarize_provider::SherpaDiarizeProvider::models_present(&state.models_root(app))
    {
        state.provider_registry.remove(&luna_core::ProviderId::from(
            crate::diarize_provider::PROVIDER_ID,
        ));
    }
    Ok(())
}

/// 清除全部本地模型（PRD-09-026「清除全部模型」）：模型库里每一个已安装条目走与逐个
/// 删除**同一条**路径；被分配槽引用的先解除分配（回到设备策略），捆绑模型不动。
/// 返回删除条数。
#[tauri::command]
#[specta::specta]
pub(crate) async fn clear_all_local_models(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let store = ModelStore::load(state.models_root(&app));
    let settings = state.settings_snapshot();
    let installed = state.engine_models_snapshot();
    let ids: Vec<String> = store
        .list()
        .into_iter()
        .filter(|entry| entry.status == "installed")
        .filter(|entry| {
            matches!(
                crate::model_target::ModelTarget::parse(&entry.id),
                Some(crate::model_target::ModelTarget::StoreEntry(_))
            )
        })
        .map(|entry| entry.id)
        .collect();
    let mut deleted = 0u32;
    for id in ids {
        let plan = delete_plan_for(&store, &settings, &installed, &id)?;
        if plan.blocking_reason.as_deref() == Some("MODEL_BUNDLED_NOT_DELETABLE") {
            continue;
        }
        for slot in &plan.blocking_slots {
            let (key, _, _) = assignment_meta(slot)
                .ok_or_else(|| format!("MODEL_ASSIGNMENT_SLOT_UNKNOWN: {slot}"))?;
            apply_model_assignment(&app, &state, key, "")?;
        }
        execute_delete_model(&app, &state, &id).await?;
        deleted += 1;
    }
    state.evict_asr_caches();
    Ok(deleted)
}

fn persist_model_assignment(state: &AppState, key: &str, target: &str) -> Result<(), String> {
    let spec = setting_spec(key).ok_or_else(|| format!("Unknown setting: {key}"))?;
    if !matches!(
        spec.control,
        luna_config::SettingControl::ModelAssignment { .. }
    ) {
        return Err(coded_error_args_with(
            state.copy(),
            "SETTING_DOMAIN_CONTROL",
            "setting-domain-control-not-a-slot",
            &[("key", key.to_string())],
        ));
    }
    let value = SettingValue::Str(target.to_owned());
    luna_config::validate(spec, &value)
        .map_err(|rejection| format!("{}: {} {}", rejection.code(), key, rejection.reason()))?;
    let mut guard = state
        .settings
        .lock()
        .map_err(|_| "Settings 锁中毒".to_string())?;
    let mut next = guard.clone();
    next.set(key, value);
    persist_device_settings(&next, &state.settings_persistence)?;
    *guard = next;
    Ok(())
}

/// 模型分配的唯一事务入口：配置落盘与 Provider 刷新必须同成同败。
/// 一次性算力交接提示的两个出口（ADR-059 ④）。
///
/// `hand_over = true` → 把 `pending_handover` 里那几条的显式本机分配清掉（清空 = 跟随，
/// 而跟随此刻落到服务端托管）；`false` → 什么都不改。两条路都记下「这台服务端已经问过」，
/// 于是提示不再出现——直到用户换一台服务端。
///
/// # 只清 `pending_handover` 算出来的那几条，绝不「一键全交」
///
/// 名单由 [`compute_status`] 用同一条判据算出（显式钉在本机 **且** 服务端跑得了）。让前端
/// 传槽位名进来，就等于把「哪些该交」这个业务判断挪到前端（ADR-003），而它一旦与 Rust
/// 分叉，用户点一下「交给服务端」会把一个服务端根本跑不了的槽位也清掉——表现是那条能力
/// 直接不可用。
#[tauri::command]
#[specta::specta]
pub(crate) fn compute_resolve_handover(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    hand_over: bool,
) -> Result<(), String> {
    let status = compute_status(app.clone(), state.clone())?;
    if hand_over {
        for route in &status.routes {
            if status.pending_handover.contains(&route.capability) {
                apply_model_assignment(&app, &state, &route.setting_key, "")?;
            }
        }
    }
    let endpoint = compute_endpoint_source(crate::cloud::evaluate_for(&app), &state.cloud_server());
    let bucket = if endpoint.is_server() {
        luna_cloud::config::server_bucket(endpoint.url())
    } else {
        String::new()
    };
    persist_model_assignment(&state, "compute.handover_prompted_for", &bucket)?;
    let _ = (SettingsChanged {
        key: "compute.handover_prompted_for".to_owned(),
    })
    .emit(&app);
    Ok(())
}

fn apply_model_assignment(
    app: &tauri::AppHandle,
    state: &AppState,
    key: &str,
    target: &str,
) -> Result<(), String> {
    let previous = state.settings_snapshot().get_str(key).unwrap_or_default();
    persist_model_assignment(state, key, target)?;
    if let Err(apply_error) = refresh_model_assignment_consumers(app, state) {
        let rollback = persist_model_assignment(state, key, &previous)
            .and_then(|()| refresh_model_assignment_consumers(app, state));
        return match rollback {
            Ok(()) => Err(coded_error_args(
                app,
                "MODEL_ASSIGNMENT_APPLY_FAILED",
                "model-assignment-apply-failed",
                &[("error", apply_error.to_string())],
            )),
            Err(rollback_error) => Err(coded_error_args(
                app,
                "MODEL_ASSIGNMENT_ROLLBACK_FAILED",
                "model-assignment-rollback-failed",
                &[
                    ("error", apply_error.to_string()),
                    ("rollback", rollback_error.to_string()),
                ],
            )),
        };
    }
    let _ = (SettingsChanged {
        key: key.to_owned(),
    })
    .emit(app);
    Ok(())
}

fn refresh_model_assignment_consumers(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(), String> {
    // 落点没变（改的是模型），但端点与有效分配仍由装配函数从同一份设置快照解析；
    // 门控结论求一次、三处共用——各求各的迟早在翻严格本地的瞬间读出两个答案。
    let availability = crate::cloud::evaluate_for(app);
    let bearer: Arc<dyn luna_provider::asr_remote::BearerSource> =
        Arc::new(crate::cloud_bearer::CloudBearer::new(app.clone()));
    state.register_llm_provider(availability, Arc::clone(&bearer));
    state.refresh_realtime_llm_provider(availability);
    // 嵌入与对话共用同一台服务端、同一份账户会话。
    refresh_embed_provider(state, availability, bearer);
    // ASR 遵循懒加载纪律（providers.rs 顶注同一句话）：未加载 → 首个任务经
    // `ensure_asr_provider_ready` 按新分配读权重；已加载 → 只在解析路径**真的变化**时
    // 热替换（正在用的表面立刻切到新权重）。绝不为一次 LLM/嵌入分配顺手重读几百 MB
    // whisper 权重——那会把每次改分配都变成秒级的命令线程阻塞。
    state.refresh_asr_assembly(app)?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        chat::refresh_chat_context_window(&app).await;
    });
    Ok(())
}

/// 分配一个已就绪且能力匹配的模型。唯一写入口；路径和模型名解析都留在 Rust。
#[tauri::command]
#[specta::specta]
pub(crate) async fn assign_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    slot: String,
    target_id: String,
) -> Result<(), String> {
    let (key, _, required_capability) =
        assignment_meta(&slot).ok_or_else(|| format!("MODEL_ASSIGNMENT_SLOT_UNKNOWN: {slot}"))?;
    let store = ModelStore::load(state.models_root(&app));
    let settings = state.settings_snapshot();
    let readiness = assignment_readiness(
        &store,
        &slot,
        Some(&target_id),
        required_capability,
        &settings,
    );
    if readiness != "ready" {
        return Err(coded_error_args(
            &app,
            "MODEL_ASSIGNMENT_NOT_READY",
            "model-assignment-not-ready",
            &[
                ("slot", slot.to_string()),
                ("capability", required_capability.to_string()),
                ("readiness", readiness.to_string()),
            ],
        ));
    }
    apply_model_assignment(&app, &state, key, &target_id)
}

/// 清除显式分配并回到该槽位的继承或设备策略。
#[tauri::command]
#[specta::specta]
pub(crate) async fn clear_model_assignment(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    slot: String,
) -> Result<(), String> {
    let (key, _, _) =
        assignment_meta(&slot).ok_or_else(|| format!("MODEL_ASSIGNMENT_SLOT_UNKNOWN: {slot}"))?;
    apply_model_assignment(&app, &state, key, "")
}

const MODEL_INSTALL_FREE_SPACE_RESERVE: u64 = 512 * 1024 * 1024;

fn available_space_for(path: &Path) -> Option<u64> {
    let mut candidate = path;
    while !candidate.exists() {
        candidate = candidate.parent()?;
    }
    fs4::available_space(candidate).ok()
}

fn install_plan_for(
    state: &AppState,
    root: &Path,
    store: &ModelStore,
    model_id: &str,
) -> Result<ModelInstallPlanDto, String> {
    let entry = store
        .entry(model_id)
        .ok_or_else(|| format!("MODEL_NOT_FOUND: {model_id}"))?;
    let luna_managed = entry.managed_by == "LunaManaged";
    let available = luna_managed.then(|| available_space_for(root)).flatten();
    let requires_reindex = remote_model_spec(model_id)
        .is_some_and(|spec| spec.task == crate::model_store::Task::Embed);
    let mut plan = install_plan_from_entry(entry, state.strict_local(), available);
    plan.requires_reindex = plan.replaces_installed && requires_reindex;
    Ok(plan)
}

pub(crate) fn install_plan_from_entry(
    entry: ModelEntryDto,
    strict_local_blocked: bool,
    available: Option<u64>,
) -> ModelInstallPlanDto {
    // 有更新的已装条目走同一张卡：下载体积取新版本，「已安装」不再拦。
    let replaces_installed = entry.status == "installed" && entry.update_size_bytes.is_some();
    // 续传：断点已经在盘上，下载量与还需的磁盘空间都扣掉它（P7 真机：继续下载时卡上仍写
    // 完整大小，与旁边「已下 451 MiB」对不上）。
    let resumed = entry.partial_bytes.filter(|bytes| *bytes > 0.0);
    let already = resumed.unwrap_or(0.0);
    let required = entry
        .est_on_disk_bytes
        .or(entry.size_bytes)
        .map(|bytes| (bytes - already).max(0.0));
    let remaining = available
        .zip(required)
        .map(|(free, bytes)| free as f64 - bytes);
    let disk_blocked = available
        .zip(required)
        .is_some_and(|(free, bytes)| free < bytes as u64 + MODEL_INSTALL_FREE_SPACE_RESERVE);
    let blocking_reason =
        if (entry.status == "installed" && !replaces_installed) || entry.status == "bundled" {
            Some("MODEL_ALREADY_INSTALLED".to_owned())
        } else if strict_local_blocked {
            Some("STRICT_LOCAL_BLOCKED".to_owned())
        } else if disk_blocked {
            Some("MODEL_DISK_SPACE_INSUFFICIENT".to_owned())
        } else if entry.status == "unavailable" {
            Some("MODEL_INSTALL_UNAVAILABLE".to_owned())
        } else {
            None
        };
    ModelInstallPlanDto {
        plan_id: uuid::Uuid::now_v7().to_string(),
        model_id: entry.id,
        name: entry.name,
        download_bytes: entry
            .update_size_bytes
            .or(entry.size_bytes)
            .map(|bytes| (bytes - already).max(0.0)),
        resumed_bytes: resumed,
        estimated_on_disk_bytes: entry.est_on_disk_bytes,
        available_bytes: available.map(|bytes| bytes as f64),
        remaining_after_bytes: remaining,
        license_label: entry.license_label,
        strict_local_blocked,
        can_start: blocking_reason.is_none(),
        blocking_reason,
        runtime_digest: entry.runtime_digest,
        replaces_installed,
        requires_reindex: false,
    }
}

/// 只读安装计划：联网、空间、License 与装机代价在开始下载前一次下发。
#[tauri::command]
#[specta::specta]
pub(crate) async fn plan_install_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<ModelInstallPlanDto, String> {
    let root = state.models_root(&app);
    let store = ModelStore::load(state.models_root(&app));
    let plan = install_plan_for(&state, &root, &store, &model_id)?;
    if plan.can_start {
        let mut plans = state
            .model_install_plans
            .lock()
            .map_err(|_| "模型安装计划锁中毒".to_owned())?;
        plans.retain(|_, existing| existing.model_id != plan.model_id);
        plans.insert(plan.plan_id.clone(), plan.clone());
    }
    Ok(plan)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn start_install_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    plan_id: String,
) -> Result<String, String> {
    let planned = state
        .model_install_plans
        .lock()
        .map_err(|_| "模型安装计划锁中毒".to_owned())?
        .remove(&plan_id)
        .ok_or_else(|| coded_error(&app, "MODEL_PLAN_NOT_FOUND", "model-plan-not-found-install"))?;
    let root = state.models_root(&app);
    let store = ModelStore::load(state.models_root(&app));
    let current = install_plan_for(&state, &root, &store, &planned.model_id)?;
    if !current.can_start
        || current.runtime_digest != planned.runtime_digest
        || current.estimated_on_disk_bytes != planned.estimated_on_disk_bytes
    {
        return Err(coded_error(
            &app,
            "MODEL_PLAN_STALE",
            "model-plan-stale-install",
        ));
    }
    execute_install_model(&app, &state, &planned.model_id).await
}

/// 安装编排的暂停外壳（PRD-14-020「可暂停」）：登记取消令牌，把真正的安装 future 与
/// 「令牌被取消」赛跑；令牌先到就**丢弃安装 future**——直连下载的 `.part` 留在暂存目录
/// （已写入的字节是连续的，下次 Range 从文件长度续）。
/// 不改 `ArtifactFetcher` 契约：三种取字节实现（直连 / 制品面 / 云感知）一律适用。
async fn execute_install_model(
    app: &tauri::AppHandle,
    state: &AppState,
    model_id: &str,
) -> Result<String, String> {
    let token = state.begin_model_install(model_id)?;
    // 通知里用展示名：`whisper-small-ggml` 是给解析器看的目录 id，不是给用户看的。
    let display_name = remote_model_spec(model_id)
        .map(|spec| spec.name.to_owned())
        .unwrap_or_else(|| model_id.to_owned());
    let outcome = tokio::select! {
        result = execute_install_model_inner(app, state, model_id) => result,
        () = wait_cancelled(&token) => {
            let _ = ModelDownloadProgress {
                model_id: model_id.to_owned(),
                downloaded_bytes: 0.0,
                total_bytes: 0.0,
                done: false,
                paused: true,
            }
            .emit(app);
            Ok(format!("MODEL_DOWNLOAD_PAUSED: {display_name}"))
        }
    };
    state.finish_model_install(model_id);
    outcome
}

/// 令牌被取消即返回；轮询间隔 200ms——暂停不是实时操作，省掉一条通知通道。
async fn wait_cancelled(token: &luna_core::CancelToken) {
    while !token.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// 暂停一次进行中的模型下载 / 拉取（PRD-14-020）。没有在装则如实报错。
#[tauri::command]
#[specta::specta]
pub(crate) fn pause_install_model(
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<(), String> {
    if state.pause_model_install(&model_id) {
        Ok(())
    } else {
        Err(format!("MODEL_INSTALL_NOT_RUNNING: {model_id}"))
    }
}

async fn execute_install_model_inner(
    app: &tauri::AppHandle,
    state: &AppState,
    model_id: &str,
) -> Result<String, String> {
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), app);
        }
        return Err(coded_error(
            app,
            "STRICT_LOCAL_BLOCKED",
            "strict-local-blocked-install",
        ));
    }
    // 下载前按龄刷新一次本地模型目录（ADR-145 ⑥）：未连接的纯本地用户只在这类显式动作里
    // 取官方签名目录，于是来源 / sha256 若已在服务端更新，这一次下载用的就是新的。超龄才真的
    // 拉（已连接的用户后台循环早已保持新鲜，这里多半是零网络）；失败不挡下载——随包目录在。
    if let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() {
        let _ = catalogs
            .models
            .refresh_if_stale(luna_cloud::catalog::AUTO_REFRESH_MAX_AGE_SECS)
            .await;
    }
    if let Some(spec) = remote_model_spec(model_id) {
        let root = state.models_root(app);
        let fetcher = state.artifact_fetcher();
        let progress = TauriInstallProgressSink::new(app.clone(), &spec.id);
        // 阶段日志（真机 2026-09-06「进度 100% 后停住」无法定位）：只打阶段与耗时。
        let t0 = std::time::Instant::now();
        install_luna_managed_model(
            root.clone(),
            spec,
            &progress,
            fetcher.as_ref(),
            download_source_preference(state),
        )
        .await?;
        eprintln!(
            "[install] {} 落盘完成，用时 {} ms；开始重装配消费者",
            spec.id,
            t0.elapsed().as_millis()
        );
        let t1 = std::time::Instant::now();
        refresh_model_assignment_consumers(app, state)?;
        eprintln!(
            "[install] {} 消费者重装配完成，用时 {} ms",
            spec.id,
            t1.elapsed().as_millis()
        );
        // 细分模型下载完成即热注册（两文件齐备才生效；先装一件时静默等待另一件）。
        if spec.task == crate::model_store::Task::Diarize {
            state.register_diarize_provider_if_available(app);
        }
        // GGUF 条目装好 / 更新好 → 引擎按新的已装集合编排并重启（更新时文件变了而 preset 没变）。
        if spec.gguf.is_some() {
            crate::local_engine::spawn_reconcile_restart(app);
        }
        return Ok(format!("MODEL_INSTALLED: {}", spec.name));
    }

    // 走到这里说明不是模型库里的已知条目（那一段在上面已 return）。`service:` 目标要单独
    // 说清楚——它不是"不支持的模型"，而是**根本不装在本机**，混进同一句
    // `MODEL_INSTALL_UNSUPPORTED` 会把用户引去找一个不存在的下载入口。
    match crate::model_target::ModelTarget::parse(model_id) {
        Some(crate::model_target::ModelTarget::Service(_)) => Err(coded_error(
            app,
            "MODEL_INSTALL_UNSUPPORTED",
            "model-install-server-managed",
        )),
        Some(crate::model_target::ModelTarget::StoreEntry(_)) => Err(coded_error_args(
            app,
            "MODEL_INSTALL_UNSUPPORTED",
            "model-install-unsupported",
            &[("model", model_id.to_string())],
        )),
        None => Err(coded_error(
            app,
            "MODEL_INSTALL_UNSUPPORTED",
            "model-name-empty",
        )),
    }
}

/// CoreML 编码器的安装阶段（`model://coreml-phase`，ADR-115 / PRD-14-020「转换带进度」）。
///
/// 字节进度仍走 `model://download-progress`（`model_id` = `<id>#coreml`）；本事件只讲
/// 阶段：`downloading` → `extracting` → `compiling`（子进程探针里的首次 ANE 编译，可达
/// 数分钟）→ `ready`；探针没过则 `failed`（编码器被挪到 `.disabled`，转写照常走 Metal/CPU）；
/// 用户移除即 `removed`。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "model://coreml-phase")]
pub struct CoremlPhase {
    pub model_id: String,
    pub phase: CoremlPhaseKind,
    /// 阶段起点（Unix 毫秒）：前端据此显示「已编译 N 秒」。
    pub started_at_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CoremlPhaseKind {
    Downloading,
    Extracting,
    Compiling,
    Ready,
    Failed,
    Removed,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

fn emit_coreml_phase(app: &tauri::AppHandle, model_id: &str, phase: CoremlPhaseKind) {
    let _ = CoremlPhase {
        model_id: model_id.to_owned(),
        phase,
        started_at_ms: now_ms(),
    }
    .emit(app);
}

/// 安装 CoreML 编码器（PRD-14-020 / ADR-115）：下载预转换 zip → 解压到权重旁 →
/// 子进程探针（首次 ANE 编译在这里发生、有人看着）→ 清 ASR 缓存让下一次加载用上。
///
/// 只在本构建编入了 CoreML 且权重已装时可用；暂停走 `pause_install_model("<id>#coreml")`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn install_coreml_encoder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<String, String> {
    if !luna_engine::COREML_COMPILED_IN {
        return Err(format!("COREML_UNSUPPORTED: {model_id}"));
    }
    let spec = crate::model_store::coreml_encoder_spec(&model_id)
        .ok_or_else(|| format!("COREML_UNAVAILABLE: {model_id}"))?;
    let root = state.models_root(&app);
    let weight = crate::model_store::luna_managed_weight_path(&root, &model_id);
    if !weight.is_file() {
        return Err(format!("MODEL_NOT_INSTALLED: {model_id}"));
    }
    if state.strict_local() {
        return Err(coded_error(
            &app,
            "STRICT_LOCAL_BLOCKED",
            "strict-local-blocked-install",
        ));
    }
    // 磁盘预检：zip + 解压后（近似同体积）。
    if let Some(available) = available_space_for(&root) {
        if available < spec.size_bytes.saturating_mul(2) {
            return Err(format!("MODEL_DISK_SPACE_INSUFFICIENT: {model_id}"));
        }
    }
    let install_key = format!("{model_id}#coreml");
    let token = state.begin_model_install(&install_key)?;
    let outcome = tokio::select! {
        result = install_coreml_encoder_inner(&app, &state, &model_id, &install_key, spec) => result,
        () = wait_cancelled(&token) => {
            let _ = ModelDownloadProgress {
                model_id: install_key.clone(),
                downloaded_bytes: 0.0,
                total_bytes: 0.0,
                done: false,
                paused: true,
            }
            .emit(&app);
            Ok(format!("MODEL_DOWNLOAD_PAUSED: CoreML · {model_id}"))
        }
    };
    state.finish_model_install(&install_key);
    outcome
}

/// 用户选的首选下载来源（`models.download_source`，ADR-144 ①）：只改尝试顺序。
fn download_source_preference(state: &AppState) -> luna_model_catalog::SourcePreference {
    luna_model_catalog::SourcePreference::from_setting(
        state
            .settings_snapshot()
            .get_str("models.download_source")
            .unwrap_or_default()
            .as_str(),
    )
}

async fn install_coreml_encoder_inner(
    app: &tauri::AppHandle,
    state: &AppState,
    model_id: &str,
    install_key: &str,
    spec: &'static crate::model_store::Artifact,
) -> Result<String, String> {
    use crate::model_store::{
        coreml_encoder_path, coreml_staging_dir, extract_mlmodelc_zip, luna_managed_weight_path,
        COREML_ENCODER_DISABLED_DIR, INSTALL_PART_FILE,
    };
    let root = state.models_root(app);
    let staging = coreml_staging_dir(&root, model_id);
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    let part = staging.join(INSTALL_PART_FILE);

    emit_coreml_phase(app, model_id, CoremlPhaseKind::Downloading);
    let progress_app = app.clone();
    let progress_key = install_key.to_owned();
    let on_progress = move |received: u64| {
        let _ = ModelDownloadProgress {
            model_id: progress_key.clone(),
            downloaded_bytes: received as f64,
            total_bytes: spec.size_bytes as f64,
            done: false,
            paused: false,
        }
        .emit(&progress_app);
    };
    let fetcher = state.artifact_fetcher();
    if let Err(err) = fetcher
        .fetch(
            crate::artifact_fetcher::ArtifactRequest {
                urls: &spec.urls_preferring(download_source_preference(state)),
                dest: &part,
                expected_size_bytes: spec.size_bytes,
                expected_sha256: &spec.sha256,
            },
            &on_progress,
        )
        .await
    {
        // 与权重安装同一条纪律：校验失败 fetcher 已删 `.part`（目录空 → 删得掉）；
        // 断网留下的字节保住（目录非空 → `remove_dir` 失败）。
        let _ = tokio::fs::remove_dir(&staging).await;
        emit_coreml_phase(app, model_id, CoremlPhaseKind::Failed);
        return Err(err);
    }

    emit_coreml_phase(app, model_id, CoremlPhaseKind::Extracting);
    let extracted = staging.join("encoder.mlmodelc");
    let _ = tokio::fs::remove_dir_all(&extracted).await;
    {
        let archive = part.clone();
        let out = extracted.clone();
        tokio::task::spawn_blocking(move || extract_mlmodelc_zip(&archive, &out))
            .await
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?
            .map_err(|e| e.to_string())?;
    }
    let dest = coreml_encoder_path(&root, model_id);
    let _ = tokio::fs::remove_dir_all(&dest).await;
    let _ = tokio::fs::remove_dir_all(root.join(model_id).join(COREML_ENCODER_DISABLED_DIR)).await;
    tokio::fs::rename(&extracted, &dest)
        .await
        .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = ModelDownloadProgress {
        model_id: install_key.to_owned(),
        downloaded_bytes: spec.size_bytes as f64,
        total_bytes: spec.size_bytes as f64,
        done: true,
        paused: false,
    }
    .emit(app);

    // 「转换」= 首次 ANE 编译，发生在第一次 create_state；放进子进程探针里做：
    // 一来 native crash 打不崩主进程，二来这一段有阶段事件与计时给用户看。
    emit_coreml_phase(app, model_id, CoremlPhaseKind::Compiling);
    let weight = luna_managed_weight_path(&root, model_id);
    let probe_weight = weight.clone();
    let passed =
        tokio::task::spawn_blocking(move || crate::whisper_backend::reprobe(&probe_weight))
            .await
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    if !passed {
        // 探针没过：把编码器挪开（不删——用户花流量下的），转写照常走 Metal/CPU。
        let disabled = root.join(model_id).join(COREML_ENCODER_DISABLED_DIR);
        let _ = tokio::fs::remove_dir_all(&disabled).await;
        let _ = tokio::fs::rename(&dest, &disabled).await;
        let _ = tokio::task::spawn_blocking(move || crate::whisper_backend::reprobe(&weight)).await;
        emit_coreml_phase(app, model_id, CoremlPhaseKind::Failed);
        return Err(format!("COREML_PROBE_FAILED: {model_id}"));
    }
    state.evict_asr_caches();
    emit_coreml_phase(app, model_id, CoremlPhaseKind::Ready);
    Ok(format!("COREML_ENABLED: {model_id}"))
}

/// 移除 CoreML 编码器（含被探针禁用的那份）；权重不动。
#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_coreml_encoder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<(), String> {
    use crate::model_store::{
        coreml_encoder_path, coreml_staging_dir, COREML_ENCODER_DISABLED_DIR,
    };
    let root = state.models_root(&app);
    let _ = tokio::fs::remove_dir_all(coreml_encoder_path(&root, &model_id)).await;
    let _ = tokio::fs::remove_dir_all(root.join(&model_id).join(COREML_ENCODER_DISABLED_DIR)).await;
    let _ = tokio::fs::remove_dir_all(coreml_staging_dir(&root, &model_id)).await;
    state.evict_asr_caches();
    emit_coreml_phase(&app, &model_id, CoremlPhaseKind::Removed);
    Ok(())
}

/// 生产进度出口：把编排的类型化阶段翻译成既有的 `model://download-progress` 事件。
///
/// **翻译是有损的，而这份有损是 wire 的属性、不是编排的属性**：现行事件只有
/// 「已下载 / 总量 / 是否完成」三个数字，没有能表示阶段的字段，故 `Started` /
/// `Extracting` / `Finalizing` 在这里**如实丢弃**，而不是伪造成一帧 `done: false` 的
/// 百分之百。`Started` 尤其不能补帧：为它发一帧 `downloaded_bytes: 0` 会让**续传**的
/// 进度条先跳回 0 再弹到断点，那是肉眼可见的行为变化。
/// 哪天事件补上阶段字段，要改的只有这一个 impl——编排一行都不用动。
pub(crate) struct TauriInstallProgressSink {
    app: tauri::AppHandle,
    /// 模型身份在此绑定：一次安装 = 一个 sink = 一个模型（故阶段事件本身不带 id）。
    model_id: String,
}

impl TauriInstallProgressSink {
    pub(crate) fn new(app: tauri::AppHandle, model_id: &str) -> Self {
        Self {
            app,
            model_id: model_id.to_owned(),
        }
    }

    /// 阶段 → wire 帧的**纯**翻译。`None` = 现行事件没有能表示这个阶段的字段，如实丢弃。
    ///
    /// 抽成纯函数是为了让「哪些阶段上 wire、上成什么样」可以在没有 Tauri app 的情况下被钉住
    /// ——`emit` 本身要 app，翻译规则不要。
    fn wire_frame(
        model_id: &str,
        progress: crate::model_store::ModelInstallProgress,
    ) -> Option<ModelDownloadProgress> {
        use crate::model_store::ModelInstallProgress as Stage;
        let (downloaded_bytes, total_bytes, done) = match progress {
            Stage::Downloading {
                received_bytes,
                total_bytes,
            } => (received_bytes, total_bytes, false),
            Stage::Installed { total_bytes } => (total_bytes, total_bytes, true),
            Stage::Started { .. } | Stage::Extracting | Stage::Finalizing => return None,
        };
        Some(ModelDownloadProgress {
            model_id: model_id.to_owned(),
            downloaded_bytes: downloaded_bytes as f64,
            total_bytes: total_bytes as f64,
            done,
            paused: false,
        })
    }
}

impl crate::model_store::ModelInstallProgressSink for TauriInstallProgressSink {
    fn report(&self, progress: crate::model_store::ModelInstallProgress) {
        if let Some(frame) = Self::wire_frame(&self.model_id, progress) {
            let _ = frame.emit(&self.app);
        }
    }
}

/// LunaManaged 模型的安装编排（TDD-04 §2.3/§2.4）：下载 → 校验 → 提取 → 落地。
///
/// 进度经 [`crate::model_store::ModelInstallProgressSink`] 注入而**不收 `&AppHandle`**：
/// app 在这条编排里唯一的用途就是发进度，收着它等于让整段编排只能在跑起来的 Tauri
/// app 里被验证——于是这段最容易留下半成品、最难重来的代码此前零测试覆盖。
pub(crate) async fn install_luna_managed_model(
    root: PathBuf,
    spec: &crate::model_store::ModelEntry,
    progress: &dyn crate::model_store::ModelInstallProgressSink,
    fetcher: &dyn crate::artifact_fetcher::ArtifactFetcher,
    preference: luna_model_catalog::SourcePreference,
) -> Result<(), String> {
    use crate::model_store::ModelInstallProgress as Stage;

    let final_dir = root.join(&spec.id);
    let weight_final = spec_weight_path(&root, spec);
    // 更新 = 已装那份的 sha256（manifest 里记的）与目录当前这条不同（ADR-145 ④）。
    let is_update = weight_final.is_file()
        && crate::model_store::installed_sha256(&root, spec)
            .is_some_and(|sha| sha != spec.weights.sha256);
    if weight_final.is_file() && !is_update {
        // 幂等早退：权重已在最终路径上，什么都不做也什么都不报（重复点安装不该重下一遍）。
        return Ok(());
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    let tmp_dir = crate::model_store::install_staging_dir(&root, &spec.id);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    let part_path = tmp_dir.join(crate::model_store::INSTALL_PART_FILE);
    let weights_path = tmp_dir.join(crate::model_store::weight_file_name(spec));

    progress.report(Stage::Started {
        total_bytes: spec.weights.size_bytes,
    });

    // 「取字节到 part 文件 + sha256 终验」经 ArtifactFetcher 接缝（TDD-14 §3.2 ②）：
    // OSS 默认直连实现的断点/覆盖语义与错误分类零变化；进度节奏 = 回调节奏 = 既有节奏
    // （开始一次 + 每 chunk 一次）。
    let on_progress = |received_bytes: u64| {
        progress.report(Stage::Downloading {
            received_bytes,
            total_bytes: spec.weights.size_bytes,
        });
    };
    if let Err(err) = fetcher
        .fetch(
            crate::artifact_fetcher::ArtifactRequest {
                urls: &spec.weights.urls_preferring(preference),
                dest: &part_path,
                expected_size_bytes: spec.weights.size_bytes,
                expected_sha256: &spec.weights.sha256,
            },
            &on_progress,
        )
        .await
    {
        // 下载段的残留由 fetcher 定夺，本层只收拾**空**暂存目录（`remove_dir` 非递归）：
        // 大小/摘要不符时 fetcher 已删掉 `.part`，目录随即为空 → 这里删得掉，root 下零残留；
        // 网络中断时 fetcher 刻意留下部分字节，目录非空 → `remove_dir` 失败，那些字节得以保住。
        // 一行同时防住两个相反的错：「校验失败留下一个空壳目录」与
        // 「一次断网就把已下载的 1.6 GB 全丢掉、下次从头再来」。
        let _ = tokio::fs::remove_dir(&tmp_dir).await;
        return Err(err);
    }

    let staged: Result<(), String> = async {
        // 归档发布物（细分-分割模型 tar.bz2）：校验后提取成员为权重；直下发布物：part 即权重。
        let staged_weight = if let Some(member) = &spec.weights.archive_member {
            progress.report(Stage::Extracting);
            let extracted = tmp_dir.join(crate::model_store::INSTALL_EXTRACTED_FILE);
            let archive = part_path.clone();
            let member = member.to_owned();
            let out = extracted.clone();
            tokio::task::spawn_blocking(move || {
                crate::model_store::extract_tar_bz2_member(&archive, &member, &out)
            })
            .await
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?
            .map_err(|e| e.to_string())?;
            let _ = tokio::fs::remove_file(&part_path).await;
            extracted
        } else {
            tokio::fs::rename(&part_path, &weights_path)
                .await
                .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
            weights_path.clone()
        };

        progress.report(Stage::Finalizing);
        if let Some(gguf) = &spec.gguf {
            let path = staged_weight.clone();
            let gguf = gguf.clone();
            tokio::task::spawn_blocking(move || {
                crate::model_store::check_gguf_header(&path, &gguf)
            })
            .await
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))??;
        }
        let manifest = serde_json::json!({
            "id": spec.id,
            "name": spec.name,
            "source_url": spec.weights.primary_url(),
            "sha256": spec.weights.sha256,
            "license": spec.license.label,
            "license_upstream": spec.license.upstream,
            "quantized_by": spec.license.quantized_by,
            "size_bytes": spec.weights.size_bytes,
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
        if spec.weight_rel.is_some() {
            // 文件式安装（约定路径，如 diarize/）：权重直落最终路径 + 邻接 manifest 边车，
            // 同目录可容纳多个模型（删除互不影响）；临时目录整体清理。
            if let Some(parent) = weight_final.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
            }
            let sidecar = weight_manifest_path(&weight_final);
            if is_update {
                // 更新：新边车先落临时名，权重原子覆盖成功后才改名——中途失败时旧权重与
                // 旧边车仍是一对，不会出现「边车说新版、权重还是旧版」于是再也提示不出更新。
                let sidecar_tmp = sidecar.with_extension("json.updating");
                tokio::fs::write(&sidecar_tmp, &manifest_bytes)
                    .await
                    .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
                if let Err(e) = tokio::fs::rename(&staged_weight, &weight_final).await {
                    let _ = tokio::fs::remove_file(&sidecar_tmp).await;
                    return Err(format!("MODEL_INSTALL_FAILED: {e}"));
                }
                tokio::fs::rename(&sidecar_tmp, &sidecar)
                    .await
                    .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                return Ok(());
            }
            tokio::fs::write(&sidecar, &manifest_bytes)
                .await
                .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
            if let Err(e) = tokio::fs::rename(&staged_weight, &weight_final).await {
                // 边车先写、权重后落地；改名失败就留下一个「有 License 溯源、没有模型」的
                // 孤儿文件——而删除路径（§2.3）按权重是否存在去找伴生件，孤儿边车再没人清。
                let _ = tokio::fs::remove_file(&sidecar).await;
                return Err(format!("MODEL_INSTALL_FAILED: {e}"));
            }
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        } else {
            tokio::fs::write(
                tmp_dir.join(crate::model_store::INSTALL_DIR_MANIFEST_FILE),
                &manifest_bytes,
            )
            .await
            .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
            if is_update {
                // 更新：旧目录先挪开、新目录就位后再删旧的；就位失败就挪回——任何一刻
                // 最终路径上要么是完整的旧版、要么是完整的新版。
                let replaced = root.join(format!(".{}.replaced", spec.id));
                let _ = tokio::fs::remove_dir_all(&replaced).await;
                tokio::fs::rename(&final_dir, &replaced)
                    .await
                    .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
                if let Err(e) = tokio::fs::rename(&tmp_dir, &final_dir).await {
                    let _ = tokio::fs::rename(&replaced, &final_dir).await;
                    return Err(format!("MODEL_INSTALL_FAILED: {e}"));
                }
                let _ = tokio::fs::remove_dir_all(&replaced).await;
            } else {
                tokio::fs::rename(&tmp_dir, &final_dir)
                    .await
                    .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(err) = staged {
        // 走到这里 sha256 **已经过了**，字节却依然不可用（归档损坏 / 缺成员 / 落地失败）：
        // 续传救不了它——下次安装会看到一个「大小正好、摘要正确」的 `.part` 而直接重放同一个
        // 失败，用户于是永远装不上，且没有任何迹象指向「上次的残骸」。这堆字节必须整包丢弃。
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return Err(err);
    }
    progress.report(Stage::Installed {
        total_bytes: spec.weights.size_bytes,
    });
    Ok(())
}

#[cfg(test)]
mod gguf_header_check_tests {
    use crate::model_store::check_gguf_header;
    use luna_model_catalog::{Gguf, GgufRole, Pooling};

    /// 最小 GGUF 头：架构 + 上下文 + 可选池化 + 可选模板。
    fn header(arch: &str, pooling: Option<u32>, template: bool) -> Vec<u8> {
        let mut kv: Vec<u8> = Vec::new();
        let mut n = 0u64;
        let mut key = |kv: &mut Vec<u8>, k: &str, ty: u32| {
            kv.extend_from_slice(&(k.len() as u64).to_le_bytes());
            kv.extend_from_slice(k.as_bytes());
            kv.extend_from_slice(&ty.to_le_bytes());
            n += 1;
        };
        key(&mut kv, "general.architecture", 8);
        kv.extend_from_slice(&(arch.len() as u64).to_le_bytes());
        kv.extend_from_slice(arch.as_bytes());
        key(&mut kv, &format!("{arch}.context_length"), 4);
        kv.extend_from_slice(&8192u32.to_le_bytes());
        if let Some(p) = pooling {
            key(&mut kv, &format!("{arch}.pooling_type"), 4);
            kv.extend_from_slice(&p.to_le_bytes());
        }
        if template {
            key(&mut kv, "tokenizer.chat_template", 8);
            kv.extend_from_slice(&2u64.to_le_bytes());
            kv.extend_from_slice(b"{}");
        }
        let mut out = b"GGUF".to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&kv);
        out
    }

    fn check(bytes: &[u8], role: GgufRole) -> Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weights.gguf");
        std::fs::write(&path, bytes).unwrap();
        check_gguf_header(
            &path,
            &Gguf {
                engine_min_build: 1,
                quant: "Q8_0".into(),
                max_context: 8192,
                role,
                translation_only: false,
            },
        )
    }

    const CHAT: GgufRole = GgufRole::Chat { thinking: false };
    const CLS: GgufRole = GgufRole::Embed {
        pooling: Pooling::Cls,
        encoder: true,
    };

    #[test]
    fn accepts_headers_that_match_the_catalog() {
        assert!(check(&header("qwen35", None, true), CHAT).is_ok());
        assert!(check(&header("bert", Some(2), false), CLS).is_ok());
    }

    #[test]
    fn rejects_headers_that_contradict_the_catalog() {
        // 对话模型没有模板
        assert!(check(&header("qwen35", None, false), CHAT)
            .unwrap_err()
            .contains("chat template"));
        // 登记为对话，文件却是嵌入模型
        assert!(check(&header("bert", Some(2), true), CHAT).is_err());
        // 嵌入池化对不上（文件是 LAST，目录写 CLS）
        assert!(check(&header("qwen3", Some(3), true), CLS)
            .unwrap_err()
            .contains("pooling"));
        // 根本不是 GGUF
        assert!(check(b"not a gguf at all", CHAT).is_err());
    }
}

#[cfg(test)]
mod install_orchestration_tests {
    use super::*;
    use crate::artifact_fetcher::{ArtifactFetcher, ArtifactProgress, ArtifactRequest};
    use crate::model_store::{
        install_staging_dir, weight_manifest_path, ModelInstallProgress as Stage,
        ModelInstallProgressSink, INSTALL_DIR_MANIFEST_FILE, INSTALL_PART_FILE,
        LUNA_MANAGED_WEIGHT_FILE,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// 夹具归档里的成员路径（sherpa 发布物同形：顶层包目录 + model.onnx）。
    const FIXTURE_ARCHIVE_MEMBER: &str = "fixture-pkg/model.onnx";
    /// 夹具权重字节。落地后逐字节比对——只断言「文件在」会让「装了个空文件」照样绿。
    const FIXTURE_WEIGHT: &[u8] = b"fixture-onnx-weight-bytes";
    /// 夹具 sha256 占位：真正的摘要终验归 `ArtifactFetcher`（本层桩不校验），
    /// 但它必须原样出现在 manifest 里，故仍是被断言的值。
    const FIXTURE_SHA256: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    /// 归档腿 + 文件式落地（真实对应细分-分割模型）。
    static ARCHIVE_FILE_SPEC: std::sync::LazyLock<crate::model_store::ModelEntry> =
        std::sync::LazyLock::new(|| crate::model_store::ModelEntry {
            id: "fixture-diarize-segmentation".to_owned(),
            name: "Fixture Segmentation".to_owned(),
            task: crate::model_store::Task::Diarize,
            weights: crate::model_store::Artifact {
                sources: vec![luna_model_catalog::Source::Url {
                    url: "https://example.invalid/fixture.tar.bz2".to_owned(),
                }],
                sha256: FIXTURE_SHA256.to_owned(),
                size_bytes: 4096,
                archive_member: Some(FIXTURE_ARCHIVE_MEMBER.to_owned()),
            },
            est_on_disk_bytes: 4096,
            license: luna_model_catalog::License {
                label: "MIT".to_owned(),
                upstream: None,
                quantized_by: None,
            },
            weight_rel: Some("diarize/segmentation.onnx".to_owned()),
            coreml_encoder: None,
            gguf: None,
            bundled: false,
            min_client_version: None,
        });

    /// 直下腿 + 文件式落地（真实对应细分-声纹嵌入模型）。
    static DIRECT_FILE_SPEC: std::sync::LazyLock<crate::model_store::ModelEntry> =
        std::sync::LazyLock::new(|| crate::model_store::ModelEntry {
            id: "fixture-diarize-embedding".to_owned(),
            name: "Fixture Embedding".to_owned(),
            task: crate::model_store::Task::Diarize,
            weights: crate::model_store::Artifact {
                sources: vec![luna_model_catalog::Source::Url {
                    url: "https://example.invalid/fixture.onnx".to_owned(),
                }],
                sha256: FIXTURE_SHA256.to_owned(),
                size_bytes: 4096,
                archive_member: None,
            },
            est_on_disk_bytes: 4096,
            license: luna_model_catalog::License {
                label: "Apache-2.0".to_owned(),
                upstream: None,
                quantized_by: None,
            },
            weight_rel: Some("diarize/embedding.onnx".to_owned()),
            coreml_encoder: None,
            gguf: None,
            bundled: false,
            min_client_version: None,
        });

    /// 直下腿 + 目录式落地（真实对应 whisper 惯例 `<id>/weights.bin`）。
    static DIRECT_DIR_SPEC: std::sync::LazyLock<crate::model_store::ModelEntry> =
        std::sync::LazyLock::new(|| crate::model_store::ModelEntry {
            id: "fixture-whisper".to_owned(),
            name: "Fixture Whisper".to_owned(),
            task: crate::model_store::Task::Asr,
            weights: crate::model_store::Artifact {
                sources: vec![luna_model_catalog::Source::Url {
                    url: "https://example.invalid/ggml-fixture.bin".to_owned(),
                }],
                sha256: FIXTURE_SHA256.to_owned(),
                size_bytes: 4096,
                archive_member: None,
            },
            est_on_disk_bytes: 4096,
            license: luna_model_catalog::License {
                label: "MIT".to_owned(),
                upstream: None,
                quantized_by: None,
            },
            weight_rel: None,
            coreml_encoder: None,
            gguf: None,
            bundled: false,
            min_client_version: None,
        });

    /// 运行时构造 tar.bz2 夹具——仓库里不塞二进制。
    fn tar_bz2_with(member: &str, payload: &[u8]) -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, member, payload).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// 下载桩的一次行为。三档刻意与 `DirectArtifactFetcher` 对 `.part` 的处置同口径：
    /// 校验失败它**删**、网络中断它**留**——编排的清理规则正是建立在这个分工上。
    enum Outcome {
        /// 成功：把这些字节写进 `dest`（摘要终验由真实 fetcher 负责，此处不重复实现）。
        Wrote(Vec<u8>),
        /// 大小/摘要不符：删掉 `.part` 后报错。
        ChecksumFailed,
        /// 网络中断：留下部分字节供下次续传。
        Interrupted(Vec<u8>),
    }

    /// 不联网的下载桩：让整条安装编排在没有 Tauri app、也没有网络的情况下跑完。
    struct StubFetcher {
        outcome: Outcome,
        calls: AtomicUsize,
    }

    impl StubFetcher {
        fn new(outcome: Outcome) -> Self {
            Self {
                outcome,
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ArtifactFetcher for StubFetcher {
        async fn fetch(
            &self,
            request: ArtifactRequest<'_>,
            on_progress: &ArtifactProgress<'_>,
        ) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // 既有节奏：开始一次 + 每 chunk 一次。桩用「0 → 全量」两帧代表它。
            on_progress(0);
            match &self.outcome {
                Outcome::Wrote(bytes) => {
                    tokio::fs::write(request.dest, bytes).await.unwrap();
                    on_progress(bytes.len() as u64);
                    Ok(())
                }
                Outcome::ChecksumFailed => {
                    let _ = tokio::fs::remove_file(request.dest).await;
                    Err("MODEL_CHECKSUM_FAILED: sha256 校验失败".to_owned())
                }
                Outcome::Interrupted(partial) => {
                    tokio::fs::write(request.dest, partial).await.unwrap();
                    on_progress(partial.len() as u64);
                    Err("MODEL_DOWNLOAD_FAILED: 连接中断".to_owned())
                }
            }
        }
    }

    /// 记录型进度出口：把阶段序列原样收下，供断言。
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<Stage>>);

    impl RecordingSink {
        fn stages(&self) -> Vec<Stage> {
            self.0.lock().unwrap().clone()
        }
    }

    impl ModelInstallProgressSink for RecordingSink {
        fn report(&self, progress: Stage) {
            self.0.lock().unwrap().push(progress);
        }
    }

    /// `root` 下当前有哪些条目（排序后比对；「零残留」要看的就是这张表）。
    fn entries(root: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn manifest_of(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// 归档腿 + 文件式落地：权重提取到约定路径、边车 manifest 内容如实、暂存目录清空。
    #[tokio::test]
    async fn archive_leg_extracts_member_to_the_conventional_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(tar_bz2_with(
            FIXTURE_ARCHIVE_MEMBER,
            FIXTURE_WEIGHT,
        )));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &ARCHIVE_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("夹具归档必须能装完");

        let weight = root.join("diarize/segmentation.onnx");
        assert_eq!(
            std::fs::read(&weight).unwrap(),
            FIXTURE_WEIGHT,
            "落地的必须是归档**成员**的字节，而不是归档本身"
        );
        let manifest = manifest_of(&weight_manifest_path(&weight));
        assert_eq!(manifest["id"], ARCHIVE_FILE_SPEC.id);
        assert_eq!(manifest["sha256"], FIXTURE_SHA256);
        assert_eq!(manifest["license"], "MIT");
        assert_eq!(manifest["size_bytes"], ARCHIVE_FILE_SPEC.weights.size_bytes);
        assert!(
            !install_staging_dir(&root, &ARCHIVE_FILE_SPEC.id).exists(),
            "暂存目录必须清掉——留着会在下次安装时被当成可续传的残骸"
        );
        assert_eq!(entries(&root), vec!["diarize".to_owned()]);
    }

    /// 直下腿 + 文件式落地：`.part` 改名即权重，同目录可与别的细分件并存。
    #[tokio::test]
    async fn direct_leg_renames_part_into_the_file_shaped_landing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &DIRECT_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("直下件必须能装完");

        let weight = root.join("diarize/embedding.onnx");
        assert_eq!(std::fs::read(&weight).unwrap(), FIXTURE_WEIGHT);
        assert_eq!(
            manifest_of(&weight_manifest_path(&weight))["license"],
            "Apache-2.0"
        );
        assert!(!install_staging_dir(&root, &DIRECT_FILE_SPEC.id).exists());
    }

    /// 直下腿 + 目录式落地（whisper 惯例）：整个暂存目录改名成 `<id>/`，
    /// 内含 `weights.bin` 与目录内 `manifest.json`——与文件式是两段不同的代码。
    #[tokio::test]
    async fn direct_leg_renames_the_whole_staging_dir_for_directory_shaped_landing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("whisper 惯例件必须能装完");

        let install_dir = root.join(&DIRECT_DIR_SPEC.id);
        assert_eq!(
            std::fs::read(install_dir.join(LUNA_MANAGED_WEIGHT_FILE)).unwrap(),
            FIXTURE_WEIGHT
        );
        assert_eq!(
            manifest_of(&install_dir.join(INSTALL_DIR_MANIFEST_FILE))["name"],
            DIRECT_DIR_SPEC.name
        );
        assert!(
            !install_dir.join(INSTALL_PART_FILE).exists(),
            "`.part` 不得随目录一起落地"
        );
        assert_eq!(entries(&root), vec![DIRECT_DIR_SPEC.id.to_owned()]);
    }

    /// **原子性：损坏归档不得留下半成品。**
    ///
    /// 此前编排的所有失败路径都不清理暂存目录，于是这里会留下一个 `.part`——它的大小
    /// 与摘要都恰好满足期望（坏的是归档结构，不是字节完整性），下次安装会直接从它续传、
    /// 重放同一个失败。用户于是**永远装不上**，而界面只会一遍遍说「安装失败」。
    #[tokio::test]
    async fn a_corrupt_archive_leaves_nothing_behind() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(b"this is not a bzip2 stream".to_vec()));
        let sink = RecordingSink::default();

        let err = install_luna_managed_model(
            root.clone(),
            &ARCHIVE_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect_err("归档损坏必须失败");

        assert!(err.starts_with("MODEL_INSTALL_FAILED"), "错误码：{err}");
        assert_eq!(entries(&root), Vec::<String>::new(), "root 下必须零残留");
        assert!(!crate::model_store::spec_weight_path(&root, &ARCHIVE_FILE_SPEC).exists());
    }

    /// **原子性：归档缺成员同样不得留下半成品**（发布物换了内部结构时的真实形态）。
    #[tokio::test]
    async fn an_archive_missing_the_member_leaves_nothing_behind() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(tar_bz2_with(
            "fixture-pkg/some-other-file.onnx",
            FIXTURE_WEIGHT,
        )));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &ARCHIVE_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect_err("成员缺失必须失败");

        assert_eq!(entries(&root), Vec::<String>::new(), "root 下必须零残留");
    }

    /// **原子性：校验失败不得留下一个空壳暂存目录。**
    ///
    /// fetcher 在摘要不符时已删掉 `.part`，但暂存目录本身此前无人清理——它会以
    /// `.<id>.download` 的形态长期躺在模型根下（目录扫描刻意看不见它，所以也没人会发现）。
    #[tokio::test]
    async fn a_checksum_failure_leaves_nothing_behind() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::ChecksumFailed);
        let sink = RecordingSink::default();

        let err = install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect_err("摘要不符必须失败");

        assert!(err.starts_with("MODEL_CHECKSUM_FAILED"), "错误码：{err}");
        assert_eq!(entries(&root), Vec::<String>::new(), "root 下必须零残留");
    }

    /// **反面：断网留下的部分字节必须保住**——清理不能一刀切成 `remove_dir_all`。
    ///
    /// 续传是 `ArtifactFetcher` 的既有能力，也是 1.6 GB 权重在弱网下唯一能装完的方式。
    /// 上面几条「零残留」若用递归删实现，这条就会红：一次断网 = 全部重下。
    #[tokio::test]
    async fn an_interrupted_download_keeps_its_partial_bytes_for_resume() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let partial = b"first-half".to_vec();
        let fetcher = StubFetcher::new(Outcome::Interrupted(partial.clone()));
        let sink = RecordingSink::default();

        let err = install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect_err("断网必须失败");

        assert!(err.starts_with("MODEL_DOWNLOAD_FAILED"), "错误码：{err}");
        let part = install_staging_dir(&root, &DIRECT_DIR_SPEC.id).join(INSTALL_PART_FILE);
        assert_eq!(
            std::fs::read(&part).unwrap(),
            partial,
            "断点字节被删掉了——下次安装只能从头再来"
        );
    }

    /// **落地失败不得留下孤儿边车**：边车先写、权重后改名，改名失败就会剩下一个
    /// 「有 License 溯源、没有模型」的 `*.manifest.json`，而删除路径按权重是否存在
    /// 去找伴生件——没人会再清它。用「最终权重路径已被一个目录占住」逼出改名失败。
    #[tokio::test]
    async fn a_failed_landing_removes_the_orphan_manifest_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = crate::model_store::spec_weight_path(&root, &DIRECT_FILE_SPEC);
        std::fs::create_dir_all(&weight).unwrap();
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &DIRECT_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect_err("最终路径被占住时改名必须失败");

        assert!(
            !weight_manifest_path(&weight).exists(),
            "孤儿边车没有被清掉"
        );
        assert!(!install_staging_dir(&root, &DIRECT_FILE_SPEC.id).exists());
    }

    /// 幂等早退：权重已在最终路径上时直接 `Ok(())`——不重下、不报进度、不碰 fetcher。
    #[tokio::test]
    async fn an_already_installed_weight_short_circuits_without_touching_the_fetcher() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = crate::model_store::spec_weight_path(&root, &DIRECT_DIR_SPEC);
        std::fs::create_dir_all(weight.parent().unwrap()).unwrap();
        std::fs::write(&weight, FIXTURE_WEIGHT).unwrap();
        let fetcher = StubFetcher::new(Outcome::ChecksumFailed);
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("已装好就该直接成功");

        assert_eq!(fetcher.calls(), 0, "早退却仍然下载了一遍");
        assert!(sink.stages().is_empty(), "早退不该报任何进度阶段");
        assert_eq!(std::fs::read(&weight).unwrap(), FIXTURE_WEIGHT);
    }

    /// 旧版的安装现场：权重 + 记着旧 sha256 的 manifest（目录式写进目录、文件式写邻接边车）。
    fn plant_old_install(
        root: &std::path::Path,
        spec: &crate::model_store::ModelEntry,
    ) -> std::path::PathBuf {
        let weight = crate::model_store::spec_weight_path(root, spec);
        std::fs::create_dir_all(weight.parent().unwrap()).unwrap();
        std::fs::write(&weight, b"old-weight-bytes").unwrap();
        let old = serde_json::json!({ "id": spec.id, "sha256": "2".repeat(64) });
        let manifest = if spec.weight_rel.is_some() {
            weight_manifest_path(&weight)
        } else {
            root.join(&spec.id).join(INSTALL_DIR_MANIFEST_FILE)
        };
        std::fs::write(manifest, serde_json::to_vec(&old).unwrap()).unwrap();
        weight
    }

    /// 更新（ADR-145 ④）·目录式：新版就位、manifest 记新 sha256、挪开的旧目录删干净。
    #[tokio::test]
    async fn an_update_replaces_a_directory_install_and_leaves_no_debris() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = plant_old_install(&root, &DIRECT_DIR_SPEC);
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));

        install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &RecordingSink::default(),
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("更新必须成功");

        assert_eq!(fetcher.calls(), 1, "sha256 不同就该真的下载新版");
        assert_eq!(std::fs::read(&weight).unwrap(), FIXTURE_WEIGHT);
        let manifest = manifest_of(
            &root
                .join(&DIRECT_DIR_SPEC.id)
                .join(INSTALL_DIR_MANIFEST_FILE),
        );
        assert_eq!(manifest["sha256"], FIXTURE_SHA256);
        assert_eq!(
            entries(&root),
            vec![DIRECT_DIR_SPEC.id.clone()],
            "旧目录与暂存都不能留下"
        );
    }

    /// 更新·文件式：权重与边车一起换成新版，临时边车不留。
    #[tokio::test]
    async fn an_update_replaces_a_file_install_together_with_its_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = plant_old_install(&root, &DIRECT_FILE_SPEC);
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));

        install_luna_managed_model(
            root.clone(),
            &DIRECT_FILE_SPEC,
            &RecordingSink::default(),
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("更新必须成功");

        assert_eq!(std::fs::read(&weight).unwrap(), FIXTURE_WEIGHT);
        assert_eq!(
            manifest_of(&weight_manifest_path(&weight))["sha256"],
            FIXTURE_SHA256
        );
        let mut dir = entries(weight.parent().unwrap());
        dir.retain(|name| name.ends_with(".updating"));
        assert!(dir.is_empty(), "临时边车残留：{dir:?}");
    }

    /// 更新失败（摘要不符）：旧版权重与旧 manifest 原封不动——用户不会因为一次失败的更新
    /// 丢掉一个能用的模型。
    #[tokio::test]
    async fn a_failed_update_keeps_the_old_version_intact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = plant_old_install(&root, &DIRECT_DIR_SPEC);
        let fetcher = StubFetcher::new(Outcome::ChecksumFailed);

        let result = install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &RecordingSink::default(),
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(&weight).unwrap(), b"old-weight-bytes");
        let manifest = manifest_of(
            &root
                .join(&DIRECT_DIR_SPEC.id)
                .join(INSTALL_DIR_MANIFEST_FILE),
        );
        assert_eq!(manifest["sha256"], "2".repeat(64));
    }

    /// manifest 里的 sha256 与目录一致 = 已是最新：照旧早退，不碰 fetcher。
    #[tokio::test]
    async fn a_current_install_is_not_redownloaded() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let weight = plant_old_install(&root, &DIRECT_DIR_SPEC);
        let current = serde_json::json!({ "id": DIRECT_DIR_SPEC.id, "sha256": FIXTURE_SHA256 });
        std::fs::write(
            root.join(&DIRECT_DIR_SPEC.id)
                .join(INSTALL_DIR_MANIFEST_FILE),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        let fetcher = StubFetcher::new(Outcome::ChecksumFailed);

        install_luna_managed_model(
            root.clone(),
            &DIRECT_DIR_SPEC,
            &RecordingSink::default(),
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .expect("已是最新就该直接成功");
        assert_eq!(fetcher.calls(), 0);
        assert_eq!(std::fs::read(&weight).unwrap(), b"old-weight-bytes");
    }

    /// **阶段序列如实、终态恰好一次。**
    ///
    /// 这正是接缝从 `Fn(u64)` 换成类型化事件才买到的断言：编排到底走了归档腿还是直下腿、
    /// 有没有真的进到落地段，此前在接缝上根本没有词可以表达，也就无从验起。
    #[tokio::test]
    async fn the_archive_leg_reports_every_stage_in_order_and_terminates_exactly_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let archive = tar_bz2_with(FIXTURE_ARCHIVE_MEMBER, FIXTURE_WEIGHT);
        let size = archive.len() as u64;
        let fetcher = StubFetcher::new(Outcome::Wrote(archive));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root,
            &ARCHIVE_FILE_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .unwrap();

        let total = ARCHIVE_FILE_SPEC.weights.size_bytes;
        assert_eq!(
            sink.stages(),
            vec![
                Stage::Started { total_bytes: total },
                Stage::Downloading {
                    received_bytes: 0,
                    total_bytes: total
                },
                Stage::Downloading {
                    received_bytes: size,
                    total_bytes: total
                },
                Stage::Extracting,
                Stage::Finalizing,
                Stage::Installed { total_bytes: total },
            ]
        );
    }

    /// 直下腿**不得**报 `Extracting`——报了就说明它跑进了归档分支。
    #[tokio::test]
    async fn the_direct_leg_never_reports_extracting() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let fetcher = StubFetcher::new(Outcome::Wrote(FIXTURE_WEIGHT.to_vec()));
        let sink = RecordingSink::default();

        install_luna_managed_model(
            root,
            &DIRECT_DIR_SPEC,
            &sink,
            &fetcher,
            luna_model_catalog::SourcePreference::Auto,
        )
        .await
        .unwrap();

        let stages = sink.stages();
        assert!(!stages.contains(&Stage::Extracting), "直下腿走了归档分支");
        assert!(stages.contains(&Stage::Finalizing));
        assert_eq!(
            stages
                .iter()
                .filter(|stage| matches!(stage, Stage::Installed { .. }))
                .count(),
            1,
            "终态必须恰好一次"
        );
    }

    /// 失败时**不得**报终态——终态是「装好了」的唯一信号，界面据此收掉进度条。
    ///
    /// 两条失败腿都要覆盖：下载段失败（编排在进落地段之前就返回）与落地段失败
    /// （已过 sha256 才炸）。只验前者的话，一个「在落地判定之前无条件报终态」的写法
    /// 照样能骗过这条守护。
    #[tokio::test]
    async fn a_failed_install_never_reports_the_terminal_stage() {
        for (leg, outcome, spec) in [
            (
                "下载段失败",
                Outcome::ChecksumFailed,
                &*DIRECT_DIR_SPEC as &crate::model_store::ModelEntry,
            ),
            (
                "落地段失败（归档损坏）",
                Outcome::Wrote(b"this is not a bzip2 stream".to_vec()),
                &ARCHIVE_FILE_SPEC,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let fetcher = StubFetcher::new(outcome);
            let sink = RecordingSink::default();

            install_luna_managed_model(
                temp.path().to_path_buf(),
                spec,
                &sink,
                &fetcher,
                luna_model_catalog::SourcePreference::Auto,
            )
            .await
            .unwrap_err();

            assert!(
                !sink
                    .stages()
                    .iter()
                    .any(|stage| matches!(stage, Stage::Installed { .. })),
                "{leg}：失败却报了终态，界面会把进度条收成「已装好」"
            );
        }
    }

    /// **wire 契约不变**：类型化阶段落到既有 `model://download-progress` 事件上时，
    /// 只有「下载中」与「完成」两种帧——与重构前逐字一致。
    ///
    /// 尤其 `Started` 不得补一帧 `downloaded_bytes: 0`：续传时那会让进度条先跳回 0
    /// 再弹到断点，是肉眼可见的行为变化。
    #[test]
    fn only_downloading_and_installed_reach_the_wire() {
        let id = "fixture-model";
        for stage in [
            Stage::Started { total_bytes: 9 },
            Stage::Extracting,
            Stage::Finalizing,
        ] {
            assert!(
                TauriInstallProgressSink::wire_frame(id, stage).is_none(),
                "{stage:?} 不该产生 wire 帧——现行事件没有表示阶段的字段"
            );
        }

        let downloading = TauriInstallProgressSink::wire_frame(
            id,
            Stage::Downloading {
                received_bytes: 3,
                total_bytes: 9,
            },
        )
        .expect("下载帧必须上 wire");
        assert_eq!(downloading.model_id, id);
        assert_eq!(downloading.downloaded_bytes, 3.0);
        assert_eq!(downloading.total_bytes, 9.0);
        assert!(!downloading.done);

        let installed =
            TauriInstallProgressSink::wire_frame(id, Stage::Installed { total_bytes: 9 })
                .expect("终态帧必须上 wire");
        assert_eq!(installed.downloaded_bytes, 9.0);
        assert_eq!(installed.total_bytes, 9.0);
        assert!(installed.done, "终态帧的 done 必须为 true");
    }

    // 前提是本平台带随包推理引擎：没有引擎的平台上 GGUF 条目不可见（`model_store::visible`）。
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn local_candidate_dto_distinguishes_runtime_model_and_capability_states() {
        let root = tempfile::tempdir().unwrap();
        // 模型库按权重文件在不在判「已装」（随包引擎条目，ADR-143）。
        let install = |id: &str| {
            let dir = root.path().join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(crate::model_store::LUNA_MANAGED_GGUF_FILE), b"x").unwrap();
        };
        let load = || ModelStore::load(root.path().to_path_buf());
        let settings = Settings::with_defaults();
        let chat = "qwen3.5-0.8b-q8_0-gguf".to_owned();

        assert_eq!(
            local_chat_candidate(&settings, false, &load(), Some(chat.clone())).readiness,
            LocalCandidateReadiness::ModelMissing
        );
        install(&chat);
        let ready = local_chat_candidate(&settings, false, &load(), Some(chat.clone()));
        assert_eq!(ready.readiness, LocalCandidateReadiness::Ready);
        assert!(ready.model_name.is_some());
        assert_eq!(
            ready.policy,
            LocalCandidatePolicy::UnreachableBeforeVisibleOutput
        );
        assert_eq!(
            local_chat_candidate(&settings, true, &load(), Some(chat)).readiness,
            LocalCandidateReadiness::RuntimeUnavailable,
            "装着而引擎熔断不是『没有模型』，界面必须给修引擎的动作而不是再下载"
        );
        // 专用翻译模型装着也不是对话候选。
        install("hy-mt2-1.8b-q8_0-gguf");
        assert_ne!(
            local_chat_candidate(
                &settings,
                false,
                &load(),
                Some("hy-mt2-1.8b-q8_0-gguf".into())
            )
            .readiness,
            LocalCandidateReadiness::Ready
        );

        let missing = local_transcription_candidate(None, false);
        assert_eq!(missing.readiness, LocalCandidateReadiness::ModelMissing);

        let embed = crate::model_store::RECOMMENDED_EMBED.to_owned();
        let mut explicit_local = Settings::with_defaults();
        explicit_local.set("ai.assignment.default_embedding", embed.as_str());
        // 有原生推理面时空配置归服务端 profile；没有时本机就是唯一执行者（ADR-126 ②）。
        let server = crate::cloud::ComputeEndpoint::OfficialCloud("https://api.example".to_owned());
        let on_device = crate::cloud::ComputeEndpoint::OnDevice;
        let candidate = |settings: &Settings, endpoint, tripped, target: &str| {
            local_embedding_candidate(
                settings,
                endpoint,
                tripped,
                &load(),
                Some(target.to_owned()),
            )
        };
        assert_eq!(
            candidate(&explicit_local, &server, false, &embed).readiness,
            LocalCandidateReadiness::ModelMissing
        );
        install(&embed);
        let ready = candidate(&explicit_local, &server, false, &embed);
        assert_eq!(ready.policy, LocalCandidatePolicy::ExplicitOnly);
        assert_eq!(ready.readiness, LocalCandidateReadiness::Ready);
        assert_eq!(
            candidate(&explicit_local, &server, true, &embed).readiness,
            LocalCandidateReadiness::RuntimeUnavailable
        );

        // 没有本机显式分配时，即使设备策略能推导出 bge-m3，原生服务端 profile 也不能被说成
        // 本机候选；显式 `service:*` 同理。
        let unconfigured = Settings::with_defaults();
        assert_eq!(
            candidate(&unconfigured, &server, false, &embed).readiness,
            LocalCandidateReadiness::NotApplicable
        );
        let mut service_profile = Settings::with_defaults();
        service_profile.set("ai.assignment.default_embedding", "service:bge-m3");
        assert_eq!(
            candidate(&service_profile, &server, false, "service:bge-m3").readiness,
            LocalCandidateReadiness::NotApplicable
        );
        // 没有原生服务端时「未显式配置」不是不适用：那台机器上本机就是唯一执行者。
        assert_eq!(
            candidate(&unconfigured, &on_device, false, &embed).readiness,
            LocalCandidateReadiness::Ready
        );
    }

    /// 「优先在哪运行」必须进入执行位置的判定链（ADR-126 ①）。
    ///
    /// 此前 `compute_route` 只看隐私档、显式分配与端点，而 `ai.route_strategy` 真的改变
    /// 去向（`Router` 按它排序、仅本机过滤外部候选）——用户调成「仅本机」，卡片仍写
    /// 「服务端托管」，而那是隐私相关的方向。顺序在这里逐档钉住。
    #[test]
    fn route_strategy_enters_the_reason_chain_in_router_order() {
        use luna_provider::RouteStrategy;

        let server = crate::cloud::ComputeEndpoint::OfficialCloud("https://api.example".to_owned());
        let with_strategy = |value: &str| {
            let mut settings = Settings::with_defaults();
            settings.set("ai.route_strategy", value);
            settings
        };
        let chat = |settings: &Settings, on_server: bool, local_leg: bool| {
            compute_route(
                settings,
                &server,
                "chat",
                "default_llm",
                on_server,
                local_leg,
                crate::ipc::ServerOffer::Serves,
            )
        };

        // 默认档：连上即用，理由是「连上服务端」而不是任何策略。
        let cloud_first = with_strategy("cloud_first");
        assert_eq!(route_strategy(&cloud_first), RouteStrategy::CloudFirst);
        assert_eq!(
            chat(&cloud_first, true, true).reason,
            ComputeRouteReason::ServerDefault
        );

        // 本机优先且本机腿在：在本机跑，理由说的是那个策略。
        let local_first = with_strategy("local_first");
        let local = chat(&local_first, false, true);
        assert_eq!(local.reason, ComputeRouteReason::StrategyLocal);
        assert_eq!(local.location, ModelExecutionLocation::Device);
        assert!(local.can_use_server, "本机优先仍然可以改回服务端");

        // 本机优先但本机解析不出模型：如实说「暂由服务端执行」，不假装是默认行为——
        // 否则用户看到的是一句与他的设定相反、又没有原因的话。
        assert_eq!(
            chat(&local_first, true, false).reason,
            ComputeRouteReason::StrategyLocalUnavailable
        );

        // 仅本机：三条能力都不外发，且「更改」入口不该出现（`can_use_server` 为假）。
        let local_only = with_strategy("local_only");
        for (capability, slot) in [
            ("chat", "default_llm"),
            ("embedding", "default_embedding"),
            ("transcription", "realtime_asr"),
        ] {
            let route = compute_route(
                &local_only,
                &server,
                capability,
                slot,
                false,
                true,
                crate::ipc::ServerOffer::Serves,
            );
            assert_eq!(
                route.reason,
                ComputeRouteReason::StrategyLocalOnly,
                "{capability} 在仅本机下的理由"
            );
            assert!(!route.can_use_server, "{capability} 仅本机下不给外发入口");
        }

        // 显式分配压过策略（与 `Router::preferred_for` 同序），隐私闸压过一切。
        let mut pinned = with_strategy("local_only");
        pinned.set("ai.assignment.default_llm", "qwen3.5-0.8b-q8_0-gguf");
        assert_eq!(
            chat(&pinned, false, true).reason,
            ComputeRouteReason::ExplicitDevice
        );
        let mut strict = with_strategy("local_only");
        strict.set("privacy.strict_local", true);
        assert_eq!(
            chat(&strict, false, true).reason,
            ComputeRouteReason::StrictLocal
        );

        // 认不出的值退回默认，而不是让一个笔误静默改变去向。
        let mut typo = Settings::with_defaults();
        typo.set("ai.route_strategy", "local-only");
        assert_eq!(route_strategy(&typo), RouteStrategy::CloudFirst);
    }

    /// 组织没开放的能力（R4b）：服务端在，但这一条在本机跑，理由如实说「组织未开放」——
    /// 不能混进「服务端不可用」（那句话会把用户送去查网络）。严格本地仍在它之前。
    #[test]
    fn a_capability_the_organization_did_not_allow_says_so() {
        let server = crate::cloud::ComputeEndpoint::LunarcaServer("https://org.example".to_owned());
        let route = compute_route(
            &Settings::with_defaults(),
            &server,
            "transcription",
            "realtime_asr",
            false,
            true,
            crate::ipc::ServerOffer::NotAllowed,
        );
        assert_eq!(route.reason, ComputeRouteReason::ServerNotAllowed);
        assert_eq!(route.location, ModelExecutionLocation::Device);
        assert!(
            !route.can_use_server,
            "组织没开放就不给「交给服务端」的出口"
        );

        let mut strict = Settings::with_defaults();
        strict.set("privacy.strict_local", true);
        let route = compute_route(
            &strict,
            &server,
            "chat",
            "default_llm",
            false,
            true,
            crate::ipc::ServerOffer::NotAllowed,
        );
        assert_eq!(route.reason, ComputeRouteReason::StrictLocal);
    }

    /// 服务端没给这一项配上游（ADR-041 ①）：在本机跑，理由说「服务端尚未配置」——不能说成
    /// 「组织未开放」（那会把用户送去找管理员，而该动的是部署方），也不给「交给服务端」的出口。
    #[test]
    fn a_capability_the_deployment_has_not_configured_says_so() {
        let server = crate::cloud::ComputeEndpoint::OfficialCloud("https://api.example".to_owned());
        let route = compute_route(
            &Settings::with_defaults(),
            &server,
            "chat",
            "default_llm",
            false,
            true,
            crate::ipc::ServerOffer::NotConfigured,
        );
        assert_eq!(route.reason, ComputeRouteReason::ServerNotConfigured);
        assert_eq!(route.location, ModelExecutionLocation::Device);
        assert!(!route.can_use_server);
    }

    /// 服务端转写的三个闸只有一个判据（ADR-126 ①）。
    ///
    /// 装配、就绪判定、候选表与算力卡此前各写一遍「开关 + 严格本地」，而「仅本机」这一档
    /// 谁都没看——于是界面说服务端转写，路由却把那条腿过滤掉了。
    #[test]
    fn server_transcription_needs_consent_and_both_gates_open() {
        let mut consented = Settings::with_defaults();
        consented.set("provider.remote_asr_enabled", true);
        assert!(server_transcription_allowed(&consented));

        let mut strict = consented.clone();
        strict.set("privacy.strict_local", true);
        assert!(!server_transcription_allowed(&strict));

        let mut local_only = consented.clone();
        local_only.set("ai.route_strategy", "local_only");
        assert!(
            !server_transcription_allowed(&local_only),
            "仅本机下不得把音频交给服务端"
        );
        assert_eq!(
            assignment_readiness(
                &ModelStore::load(std::env::temp_dir()),
                "realtime_asr",
                Some(REMOTE_ASR_TARGET),
                "transcribe",
                &local_only,
            ),
            "incompatible",
            "仅本机与服务端转写目标直接矛盾，不能报就绪"
        );

        assert!(
            !server_transcription_allowed(&Settings::with_defaults()),
            "没给过同意就不该允许"
        );
    }
}
