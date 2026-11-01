//! Lunarca 服务端**原生推理** Provider（ADR-008、
//! TDD-14 §3.3）。
//!
//! 它**不是新调用栈**，只是又一个 [`AIProvider`] 实现——`JobQueue` / `Router` /
//! `ResultSchema` 一概不变（ADR-001 单一 AI 主干）。
//!
//! # 为什么打原生面而不是服务端的 OpenAI 兼容层
//!
//! 服务端同时提供 `/openai/v1/chat/completions` 兼容层，它是原生 API 的薄适配，
//! 同鉴权、同路由、同计量——**一直是能用的**。默认档从它换到原生面，不是为了「能跑」，
//! 而是为了四件兼容层**表达不出来**的事：
//!
//! 1. **能力如实上报**。兼容层把每一次请求都写成 `capability = chat`（它只有
//!    `chat/completions` 这一个入口）。而服务端的计量与配额是**按能力分维**的，于是
//!    翻译、摘要、文本优化三类用量全部记在对话头上——用户看到的用量分布是错的，
//!    管理员按能力设的配额也拦不到该拦的东西。
//! 2. **执行者如实展示**（PRD-17-033/036）。原生面首帧 `accepted` 就告诉客户端这次
//!    实际跑在哪个上游、什么性质、哪个模型。兼容层没有这个位置，于是「你以为在用组织的
//!    服务端、实际被降级到了第三方」在对话这条路上**完全不可见**——转写那条路早就有
//!    [`JobEvent::ExecutedBy`] 了，对话侧一直是空的。
//! 3. **真取消**。用户显式停止时发 `DELETE /inference/v1/jobs/{id}`；隐私闸中途关闭时不能
//!    再发任何字节，只能断开响应流，由服务端断连守卫停上游。两条都必须停止计费。
//! 4. **结构化终态**。`quota_exhausted` 是中途耗尽时**在已产出内容之后**发的一帧，
//!    而不是一个 HTTP 状态码——已产出的不回退、只终止剩余（PRD-13-019 流式硬边界）。
//!
//! # 凭据
//!
//! 只有一种：已登录账户的 access token（ADR-155）。Lunarca 云与组织服务端走同一条路径，
//! 区别只在 [`BearerSource`] 背后是哪台服务端的会话。

use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, ExecutorInfo, ExecutorKind, Job, JobError,
    JobEvent, JobEventStream, JobResult, ModelId, PartialPayload, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, ServiceClass, Timestamp, Timings,
    TruncatedBy, Usage, UsageEstimate,
};
use luna_model_fetch::pinned_tls::PinnedClient;
use serde::Serialize;
use serde_json::Value;

use crate::asr_remote::BearerSource;
use crate::llm_http::build_chat_messages;
use crate::sse::{SseDecoder, SsePayload};

/// Provider id。
///
/// **与本机默认档分开，不复用它的 id。** 过渡通路当年刻意复用本机 id，理由是
/// 「id 是路由指派认的槽位名」——那条理由在这里不成立，也从未成立于本 Provider：装配层
/// 保证默认档**同一时刻只注册一个**（本机 / Lunarca 服务端二选一），换 id
/// 不会让任何指派落空。而分开的收益是实打实的：`descriptor().display_name` 与
/// `executed_by` 能如实说出这次是谁在跑，日志与用量归因也不再把两件事混成一个名字。
pub const CLOUD_LLM_PROVIDER_ID: &str = "lunarca-cloud";

/// 原生推理面的挂载路径（TDD-16 §5.1）。控制面占用了 `/v1`，故推理面挂 `/inference/v1`
/// ——**不是**可配置项，写死是对服务端契约的如实复述。
pub(crate) const NATIVE_PATH: &str = "/inference/v1";

/// 连接建立超时。与 `LlmHttpProvider` 同值同理由：引擎失联应尽快归
/// `ProviderUnavailable`；读超时不设（流式长生成合法）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 响应头抵达的死线（**不是**读超时——流式正文仍不设上限）。
///
/// `connect_timeout` 只管新建连接：网络断开时连接池里那条**已建立**的连接不会立刻报错，
/// 往上写只会进入 TCP 重传，几分钟后才失败——真机 2026-09-07「云端优先 + 断网」第一次
/// 翻译一直转圈。服务端对每个任务都先回 SSE 头（`accepted` 帧在 20 s 首字节死线之内），
/// 头都没到就是链路不通，按 `Timeout` 归「不可达」，路由据此换腿到本机。
const RESPONSE_HEADERS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
/// 连接池空闲上限：断网前建的连接不该在断网后还被复用。
const POOL_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// 长流控制面轮询周期。取消令牌与隐私闸都是无锁原子量；把检查夹在每个网络分片之间仍会
/// 被“上游迟迟不出首字”的阶段卡住，因此用一个短周期唤醒控制面。只读原子值，不做 IO。
const STREAM_CONTROL_POLL: std::time::Duration = std::time::Duration::from_millis(50);

pub struct CloudLlmConfig {
    /// Lunarca 服务端基址（控制面地址，不含 `/inference/v1`）。
    pub api_base: String,
    /// 展示名（卡片与日志上写的那一句，装配层按落点给出）。
    pub display_name: String,
    /// 会话凭据来源（账户 access token 或组织服务端 API Key）。
    pub bearer: Arc<dyn BearerSource>,
    /// 外发闸（严格本地随时可掐断，见 [`luna_core::OutboundGate`]）。
    pub gate: Arc<dyn luna_core::OutboundGate>,
    /// 账户可用的 AI 能力（wire 名，统一登录 R4b）；`None` = 不限。[`SERVED_CAPABILITIES`] 里
    /// 不在清单上的那几项不声明，路由于是直接走本机腿，而不是先打一次服务端吃 403。
    pub allowed: Option<Vec<String>>,
}

/// 本 Provider 在某份允许清单下实际声明的能力（[`SERVED_CAPABILITIES`] 的子集）。
#[must_use]
pub fn served_under(allowed: Option<&[String]>) -> Vec<Capability> {
    SERVED_CAPABILITIES
        .into_iter()
        .filter(|c| allowed.is_none_or(|list| list.iter().any(|a| a == wire_capability(*c))))
        .collect()
}

/// 本 Provider 声明的能力。
///
/// # 为什么是常量，而不是服务端下发的那份清单
///
/// TDD-14 §3.3 要求 `capabilities()` 取自 `/inference/v1/capabilities`。那需要一份**带
/// 失效**的本地缓存（探活本身是显式动作，不能每次装配都发一次请求——ADR-037），而缓存
/// 与它的失效时机今天还没有消费者。**先摆一个空的注入点、让它恒为空**，等于把
/// 「服务端说了算」写成一句今天不成立的话：那正是本仓一路在防的半成品。
///
/// 这四项不是猜的——它们逐字对应服务端的 `Capability::CHAT_SHAPED`（走 `/inference/v1/jobs`
/// 那条端点的能力：messages 进、增量流出）。**不是 `SERVED`**：那份清单还含 `Embed`，而嵌入
/// 的请求形状不同、走自己的端点，由 [`crate::cloud_embed`] 承担。真不一致时的代价也很小：
/// 服务端会如实回 `invalid_input:「能力 X 不走本端点」`，那条错误指向的是真问题。
///
/// 反过来**不能退化成空集**：空集 = 路由认为没有任何 Provider 支持对话，用户拿到的是
/// `NoProvider`——一条既不可执行、也与真实原因毫无关系的错误。
pub const SERVED_CAPABILITIES: [Capability; 4] = [
    Capability::Chat,
    Capability::Translate,
    Capability::Summarize,
    Capability::Refine,
];

/// [`SERVED_CAPABILITIES`] 的 wire 名（与服务端 `Capability::as_str` 逐字一致）。
///
/// **给装配层用，免得它另抄一份**。模型页要回答「服务端接管了哪些能力」，需要的正是
/// 这四个字符串；在 `src-tauri` 里再写一遍 `["chat","translate",…]`，就成了同一个进程里
/// 的第二份清单——漏改一处不会有任何东西变红，而表现是模型页说的接管范围与 Provider
/// 实际声明的能力对不上。
#[must_use]
pub fn served_capability_names() -> [&'static str; 4] {
    [
        wire_capability(SERVED_CAPABILITIES[0]),
        wire_capability(SERVED_CAPABILITIES[1]),
        wire_capability(SERVED_CAPABILITIES[2]),
        wire_capability(SERVED_CAPABILITIES[3]),
    ]
}

pub struct CloudLlmProvider {
    config: CloudLlmConfig,
    /// 自签部署钉住的根证书在这里生效（ADR-153）。
    http: PinnedClient,
    /// 失败计数熔断（TDD-01 §3.1）。与 `LlmHttpProvider` 同一实现同一纪律：
    /// 只记真实请求的结果，`health()` 永不发请求。
    breaker: Arc<crate::breaker::Breaker>,
}

impl CloudLlmProvider {
    #[must_use]
    pub fn new(config: CloudLlmConfig) -> Self {
        Self {
            config,
            breaker: Arc::new(crate::breaker::Breaker::new()),
            http: PinnedClient::new(|builder| {
                builder
                    .connect_timeout(CONNECT_TIMEOUT)
                    .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                    .tcp_keepalive(std::time::Duration::from_secs(15))
            }),
        }
    }
}

/// 领域能力 → 服务端 wire 名。
///
/// 逐条写死而不是 `Debug`/`serde` 派生：wire 名是**跨仓契约**，派生出来的名字会跟着
/// Rust 侧改名悄悄变，而服务端那边不会跟着变——表现是某个能力突然被判成
/// 「未知 capability」，且没有任何东西会在改名那一刻变红。
fn wire_capability(capability: Capability) -> &'static str {
    match capability {
        Capability::Transcribe => "transcribe",
        Capability::Translate => "translate",
        Capability::Summarize => "summarize",
        Capability::Refine => "refine",
        Capability::Chat => "chat",
        Capability::Ocr => "ocr",
        Capability::Diarize => "diarize",
        Capability::Embed => "embed",
        Capability::Custom => "custom",
    }
}

/// 服务端稳定错误码 → [`JobError`]（TDD-14 §3.3「不靠字符串猜测」的那张表）。
///
/// 每一档的分派依据都是「用户据此该做什么」：
/// - `unauthorized` → 去登录 / 重填凭据，**不可重试**（凭据没换，重试还是 401）；
/// - `quota_exhausted` → 等周期重置或找管理员加量，不可重试；`dimension` 说的是哪一维
///   超了（没有它，用户不知道该等还是该找人）；
/// - `rate_limited` → 稍后再试，**可重试**；
/// - `upstream_unavailable` / `internal` → [`JobError::ServerSideFailure`]，**可重试**：
///   服务端已经给出业务响应（端点可达，不是链路断开），但办不成的是它背后那一层；
/// - `invalid_input` → 请求本身有问题，重试无意义；
/// - `route_lane_empty` → 运营者还没往这一档的链上排任何候选，**不可重试、不换腿**
///   （下一步是去管理端「路由策略」页加一个候选，不是等）；
/// - `egress_blocked` / `upstream_protocol` → [`JobError::ServerMisconfigured`]，服务端
///   配置问题，用户改不动、重试也不会成功，但要说清是服务端侧的事而不是他的网络。
///
/// 认不出的码归 [`JobError::Upstream`] 并**保留原文**：运维要能据此定位，而客户端不该
/// 假装认识一个新版服务端才有的码。
pub(crate) fn map_server_error(
    provider: &ProviderId,
    site: CallSite,
    code: &str,
    dimension: Option<String>,
) -> JobError {
    map_server_error_with_retry(provider, site, code, dimension, None)
}

/// 这一次调用打的是**哪条链**：能力 + 服务档（ADR-083 ②）。
///
/// **存在的理由只有 `route_lane_empty` 一条**：那个码要说出「空的是哪一档的哪条链」，
/// 而服务端的错误体里只有码本身——把服务端那句中文原样转呈会带上它的内部构造（泳道名），
/// 而客户端手上本来就有这两格（请求正是它自己发的）。收成结构体而不是再加两个位置参数：
/// `map_server_error(provider, capability, class, code, dimension)` 里两个枚举靠顺序区分，
/// 传反了编译器不会说话。
#[derive(Clone, Copy)]
pub(crate) struct CallSite {
    pub capability: Capability,
    pub class: ServiceClass,
}

/// 服务端 403 稳定码：账户生效的配额策略没开放这项能力（与服务端 `ip_domain::error::
/// CAPABILITY_NOT_ALLOWED` 逐字一致）。对话、嵌入、批式与实时转写四条通道共用这一份。
pub const CAPABILITY_NOT_ALLOWED: &str = "capability_not_allowed";

/// 服务端 403 稳定码：账户因付款争议被冻结为只读（纵切 3 C3，与服务端逐字一致）。推理与转写
/// （批式、实时）四条通道共用这一份；同步与结账那一侧由 `luna-cloud` 映射成 `CloudError::AccountFrozen`。
pub const ACCOUNT_FROZEN: &str = "account_frozen";

/// 服务端 402 稳定码：这次预留要从预付 credits 扣，而用户自设的本月预付上限已到（纵切 3 C4，与服务端
/// 逐字一致）。单纯额度用尽仍是 `quota_exhausted`。推理与转写四条通道共用这一份；控制面那一侧由
/// `luna-cloud` 映射成 `CloudError::SpendCapReached`。
pub const SPEND_CAP_REACHED: &str = "spend_cap_reached";

/// 服务端 402 稳定码：这个账户此刻领不到免费额度（纵切 3 C5，与服务端逐字一致）——这台设备上已有
/// 别的账户领过，或要先做一次零扣费的付款方式验证（成因在权益 `free_quota` 里，不在这一帧上）。
/// 推理与转写四条通道共用这一份；控制面那一侧由 `luna-cloud` 映射成 `CloudError::FreeQuotaWithheld`。
pub const FREE_QUOTA_WITHHELD: &str = "free_quota_withheld";

/// 服务端稳定码 → `JobError`；`unauthorized` 一律是账户会话被拒（TDD-01 §2.4）。
pub(crate) fn map_server_error_with_retry(
    provider: &ProviderId,
    site: CallSite,
    code: &str,
    dimension: Option<String>,
    retry_after_secs: Option<u64>,
) -> JobError {
    match code {
        "unauthorized" => JobError::Unauthorized {
            provider: provider.clone(),
        },
        "quota_exhausted" => JobError::QuotaExhausted { dimension },
        // 组织没开放这项能力（R4b）：路由据此回落本机腿，而不是「去登录」或「额度用尽」。
        CAPABILITY_NOT_ALLOWED => JobError::CapabilityNotAllowed {
            provider: provider.clone(),
        },
        // 账户被冻结（C3）：与额度用尽同一侧不可重试，但下一步是联系支持而不是升级。
        ACCOUNT_FROZEN => JobError::AccountFrozen {
            provider: provider.clone(),
        },
        // 本月预付上限（C4）：与额度用尽同一侧不可重试，但下一步是调整上限而不是购买。
        SPEND_CAP_REACHED => JobError::SpendCapReached {
            provider: provider.clone(),
        },
        // 免费额度被扣下（C5）：与额度用尽同一侧不可重试，但下一步看成因（验证付款方式 / 预付与订阅），
        // 不是一律「购买」。
        FREE_QUOTA_WITHHELD => JobError::FreeQuotaWithheld {
            provider: provider.clone(),
        },
        "invalid_input" => JobError::InvalidInput("服务端拒绝了这次请求的参数".to_owned()),
        "rate_limited" => JobError::RateLimited {
            provider: provider.clone(),
            retry_after_secs,
        },
        // 服务端已返回稳定业务错误，端点可达。它们可重试，却**绝不**是换腿信号：归
        // `ProviderUnavailable` 会让云→本机在首个可见输出前静默切腿，掩盖服务端真实
        // 故障，也与第三方 HTTP 5xx → `Upstream` 的契约分叉。
        //
        // **归 `ServerSideFailure` 而不是 `Upstream`**（ADR-097）：`Upstream` 说的是
        // 「我们直连的那个上游回了错」，用户手上有那条链路的旋钮；这两个码说的是
        // 「服务端是好的，它背后那一层出了问题」，用户一个旋钮都没有。真机坐实过
        // （2026-08-31）：`translate/realtime` 泳道上游挂掉时，弹窗翻译显示的是
        // 「AI 引擎返回错误，请重试」，而同一台服务端的批量档同时完全正常。
        // 服务端码原样带上——它只进日志与用量账本供运维定位，不上界面。
        "upstream_unavailable" | "internal" => JobError::ServerSideFailure {
            provider: provider.clone(),
            code: code.to_owned(),
        },
        // **服务端这台部署配错了，且重试不会成功**：出站被网关策略拒绝、上游回了
        // 服务端认不出的协议。判据与上面 `unauthorized` / `route_lane_empty` 逐字同源
        // ——它是一份配置，不会因为再问一次就变对。此前它们落进 `other =>` 的 `Upstream`
        //（可重试），于是队列白烧候选链、界面写着「请重试」而重试永远不会成功。
        "egress_blocked" | "upstream_protocol" => JobError::ServerMisconfigured {
            provider: provider.clone(),
            code: code.to_owned(),
        },
        // **不可重试、不换腿**，判据与上面 `unauthorized` / `quota_exhausted` 那两条
        // 逐字同源：泳道成员是运营者在管理端排出来的一份名单，它不会因为再问一次、或者
        // 换一条腿就凭空多出一个候选。落进 `other =>` 的 `Upstream`（可重试）会退回本批
        // 要消灭的形态——实时字幕逐句打一次服务端、逐句失败、逐句回落本机。
        "route_lane_empty" => JobError::RouteLaneEmpty {
            capability: site.capability,
            class: site.class,
        },
        // 幂等裁决**必须显式落在不可重试的一侧**。拿同一个键重试永远得到同一个裁决，
        // 而落进 `other =>` 的 `Upstream` 是可重试的：队列会白烧候选链，界面显示一句
        // 无从下手的「上游出错」（前端读的是变体投影，`Upstream.code` 它看不到）。
        "idempotency_in_flight" | "idempotency_replayed" => JobError::AlreadyProcessed {
            provider: provider.clone(),
        },
        // 同键不同体 = 客户端复用了键，是**我们这一侧的 bug**，不是服务端的问题。
        // 归 `InvalidInput`（直接呈现、不重试）而不是上面那条：重新发起一次也还是错的，
        // 得改的是键的生成方式。
        "idempotency_key_reused" => JobError::InvalidInput("幂等键被复用于另一个请求体".to_owned()),
        "cancelled" => JobError::Cancelled,
        other => JobError::Upstream {
            provider: provider.clone(),
            code: Some(other.to_owned()),
            message: "服务端推理失败".to_owned(),
        },
    }
}

/// 服务端随错误发来的那句**可外露说明** → 本机日志。
///
/// # 为什么非要有这一处
///
/// 服务端的错误体里有 `error_description`，而客户端此前只读 `error` 与 `dimension`——
/// 那句话在进程里被原地丢掉。代价真机坐实过（2026-08-31）：一次「合成主题页」失败为
/// `invalid_input`，而这个码在这条路上有两个**下一步完全不同**的来源——服务端自己的
/// 入参校验（我们构造错了请求），与「候选链上每一台上游都拒绝了这次请求」
/// （上游 400/404/422/413 在服务端统一判成 `invalid_input`）。服务端**已经把是哪一个
/// 写在 `error_description` 里发过来了**，客户端读不到，于是排查只能靠逐条静态排除。
///
/// # 为什么记日志是安全的，而且只记日志
///
/// 两条服务端侧的契约同时成立：① `error_description` 只对服务端 `detail_is_public()`
/// 白名单里的码在场（内部错误与出站拒绝恒缺席）；② 上游状态映射**明禁回传上游响应体**
/// ——部分上游会在错误里回显 prompt，服务端只保留状态码这一无正文的事实。故它按契约
/// 不含用户正文。仍然**截断**：契约是对端的承诺，而日志脱敏（TDD-12）是我们自己的
/// 底线，两者不该互为前提。
///
/// **不上界面**：界面按稳定码取本地化文案（TDD-15 §4.3），服务端原文既不随界面语言变、
/// 也不该出现在用户面前。
/// 服务端错误体里那一格**可外露说明**的取值口（`error_description`）。
///
/// **单独一个函数，只为让字段名可判定**：内联成 `body.get("error_description")` 时，
/// 把它拼错——或服务端某天改名——的表现是本机日志从此永远写「服务端未提供」，而没有
/// 任何东西会变红。判据见 `the_detail_field_name_matches_the_server_error_body`：那条
/// 用例拿一份**逐字复制自服务端 `ErrorBody` 的响应体**来断言。
fn server_error_detail(body: &Value) -> Option<&str> {
    body.get("error_description")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn log_server_detail(status: u16, code: &str, detail: Option<&str>) {
    const MAX_DETAIL_CHARS: usize = 200;
    match detail {
        Some(text) if !text.is_empty() => {
            let clipped: String = text.chars().take(MAX_DETAIL_CHARS).collect();
            let ellipsis = if text.chars().count() > MAX_DETAIL_CHARS {
                "…"
            } else {
                ""
            };
            eprintln!("[云] 服务端拒绝 HTTP {status} code={code} 说明={clipped}{ellipsis}");
        }
        // 没有说明也要留一行：它本身是事实（这个码不在服务端的可外露名单里），
        // 而「日志里什么都没有」与「服务端没说」在排查时必须分得开。
        _ => eprintln!("[云] 服务端拒绝 HTTP {status} code={code} 说明=（服务端未提供）"),
    }
}

/// 服务端 `accepted` 帧里的执行者 → [`ExecutorInfo`]。
///
/// `leaves_device` 恒 `true` 且**由客户端填**：服务端把自己的运行时如实报成
/// `local_runtime`（对它而言确实是自有运行时），而那与「字节离开了这台设备吗」是两个
/// 问题。混用过一次、真机才打出来（见 [`ExecutorInfo::leaves_device`] 的注释）——
/// 走到本 Provider 就意味着正文已经出去了，这一位没有第二种取值。
pub(crate) fn executor_from(accepted: &Value) -> Option<ExecutorInfo> {
    let who = accepted.get("executed_by")?;
    let model = who.get("model").and_then(Value::as_str).unwrap_or_default();
    Some(ExecutorInfo {
        display_name: who
            .get("upstream_name")
            .and_then(Value::as_str)
            .unwrap_or("服务端")
            .to_owned(),
        kind: who
            .get("kind")
            .and_then(Value::as_str)
            .map_or(ExecutorKind::Unknown, ExecutorKind::parse),
        model: model.to_owned(),
        leaves_device: true,
    })
}

#[derive(Serialize)]
struct JobRequestBody<'a> {
    capability: &'a str,
    /// 这次调用要哪一档服务（`realtime` / `batch`，协议 v5 起**必填**，ADR-083 ②）。
    ///
    /// **取自 [`Job::service_class`]，调用点一律不声明**：档位是 Job 形状的函数，
    /// 而让每个提交点自己填的写法本仓已经付过代价——弹窗翻译当初就漏了（ADR-083 ③）。
    ///
    /// **恒发，且服务端无 `serde(default)`**：可选字段会把「调用方没想清楚」静默
    /// 变成「按批量跑」，表现是实时字幕莫名其妙变慢而没有任何东西报错。
    service_class: &'a str,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// 本次合成用的能力提示词版本（`0` = 随包默认，≥1 = 已验签目录版本，ADR-050）。
    ///
    /// **纯版本号，绝不是正文**：服务端把它记进用量事件，运营才回答得了
    /// 「换了这一版提示词之后，时延与 token 是变好还是变差」。零留存承诺一个字节
    /// 没动——请求体里本来就有的正文照旧转发即忘，这里多的只是一个整数。
    /// **恒发**（不是 `skip_serializing_if`）：可选字段会让服务端同时处理「没带」
    /// 与「带了 0」两种情况，而两者的正确处理完全一致。
    prompt_version: u32,
    /// 本次合成用的灰度变体 id（ADR-050 修订 ④）；`null` = 对照组或随包默认。
    ///
    /// **只是一个 id**（服务端按 `[A-Za-z0-9_.-]{1,64}` 收，不合形状直接丢），不是
    /// 正文；与 `prompt_version` 同一条纪律、同一个去处（账本）。恒发，理由同上。
    prompt_variant: Option<String>,
}

#[derive(Serialize)]
struct WireMessage {
    role: String,
    content: String,
}

/// 流读完时的观察结果 —— [`stream_terminal`] 的输入。
///
/// 用具名结构而不是继续加位置参数：五个里有两个是同类型的 `bool`、两个是
/// `Option<String>` 家族，靠顺序区分传错了编译器也不会说话，而传反 `saw_terminal`
/// 与 `saw_thinking` 恰好是「把截断说成完成」和「把空说成成功」这两件事。
struct StreamTally {
    /// 已累计的正文（`Partial` 早已逐 token 发出，这里只用于判空与装进结果）。
    accumulated: String,
    /// 见过 `done` / `error` 帧吗。
    saw_terminal: bool,
    /// 见过 `thinking` 帧吗。
    saw_thinking: bool,
    /// 中途收到的服务端错误（`error` 帧）。
    error: Option<JobError>,
    model_id: Option<String>,
    /// 服务端在终态帧回报的用量。见 [`parse_usage`]。
    usage: Usage,
    /// 服务端在终态帧回报的输出触顶归属。见 [`parse_output_truncated`]。
    output_truncated: Option<TruncatedBy>,
}

/// 终态帧里的 `usage` → [`Usage`]。
///
/// # 口径是「取，不猜」
///
/// 服务端**总是**带这个字段，并附 `source`：`upstream` = 上游自报的实测值，
/// `estimated` = 上游没报、服务端按字节兜底估的。我们只在前者时填数，后者留 `None`。
///
/// 为什么不把估算值也填上：`Usage` 的消费者是「活动 · 用量」，用户拿它与账单对照。
/// 填一个估算值进去，用户看到「用了 137」而账单写 152，问起来谁都答不上——而
/// `None`（如实显示「未知」）至少不制造一个假的锚点。**服务端的账本自己会记估算值
/// 并标注来源**，那是计费权威，与这里要回答的问题不是一件事。
#[must_use]
pub(crate) fn parse_usage(frame: &Value) -> Usage {
    let Some(usage) = frame.get("usage") else {
        return Usage::default();
    };
    if usage.get("source").and_then(Value::as_str) != Some("upstream") {
        return Usage::default();
    }
    let take = |key: &str| {
        usage
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    Usage {
        tokens_in: take("tokens_in"),
        tokens_out: take("tokens_out"),
        ..Usage::default()
    }
}

/// 终态帧里的 `output_truncated` → [`TruncatedBy`]。
///
/// # 字段缺席 = `None`，而这不是兼容分支
///
/// 输出触顶的服务端语义（触顶改判 `Finished` 并在终态帧带上归属）落在**下一批**。
/// 在它上线之前，本客户端连到的服务端不会发这一格——那是协议协商窗口内的正常状态，
/// 不是「为旧版本留的兜底」（ADR-046）：`negotiate()` 已经把版本差异表达出来了，
/// 这里只是如实回答「服务端没告诉我们，所以不知道」。
///
/// 取值口径与 `usage` 同源——**取，不猜**：认不出的字符串按没说处理，绝不猜一个归属。
/// 猜错的代价是把「上游机器上限」说成「你自己的额度设小了」，把用户引去改一个改了也没用的设置。
#[must_use]
pub(crate) fn parse_output_truncated(frame: &Value) -> Option<TruncatedBy> {
    TruncatedBy::from_key(frame.get("output_truncated").and_then(Value::as_str)?)
}

/// 流读完之后该给出什么终态。
///
/// # 为什么是纯函数
///
/// 它裁决三件事，每一件写反都只在**真机的特定时刻**才现形，而 `execute` 那段要真起一个
/// SSE 服务端才跑得到：
///
/// 1. **没见到终态帧 = 链路被截断，不是完成。** 服务端的 SSE 状态机保证终态必发
///    （`done` 或 `error`，见 `ip-api` 的 `sse_stream` `unfold`），所以「流没了却没有终态」
///    只有一种解释：中间设备提前关了连接、服务端进程消失、代理截断。此时报 `Finished`
///    就是把**半截译文说成完成**——用户看到一段莫名断掉的文本 + 一个成功状态，零线索。
///    这正是 PRD-13-019 / TDD-01 §4「不静默截断」；而**原生面是唯一分辨得出来的通路**
///    （OpenAI 兼容流的 `[DONE]` 不是强制的，那边没有这个信号），刚换过来就丢掉等于白换。
/// 2. **零正文不是成功。** 思考型模型在输出预算偏紧时会把额度全花在推理过程上，正文
///    一个字都不来。服务端注释把这种形态称为最难归因的一类：「不是失败，是静默的空」。
///    **见过 thinking 才这么判**——真的什么都没产出另有原因，套这句话会把人引偏。
///    它走 [`JobError::EmptyAfterThinking`] 这个**独立的稳定码**，而不是塞进 `Upstream`：
///    界面读的是 `code()`，塞进去等于这一类的下一步永远说不出口（真机坐实过一次）。
/// 3. 中途错误优先于以上两条：它是服务端**明确说出来**的原因，比我们推断的更准。
///
/// 已产出的 `Partial` 在任何一条上都不回退——它们早已发出去了，界面上就摆着（流式硬边界）。
fn stream_terminal(
    provider: &ProviderId,
    job_id: luna_core::JobId,
    tally: StreamTally,
) -> JobEvent {
    if let Some(err) = tally.error {
        return JobEvent::Failed(err);
    }
    if !tally.saw_terminal {
        return JobEvent::Failed(JobError::Upstream {
            provider: provider.clone(),
            code: Some("TRUNCATED".to_owned()),
            message: "服务端响应在收到终态之前中断".to_owned(),
        });
    }
    if tally.accumulated.is_empty() && tally.saw_thinking {
        // **一个字的文案都不在这里**：这一层给的是稳定码，句子由界面按码取（两个渲染
        // 面各一条，见 `ipc_error_copy`）。此前这里带着一句写得很好的可行动指引，而
        // `JobErrorCodeDto` 只投影 `code()`——那句话一次都没到过用户眼前。
        return JobEvent::Failed(JobError::EmptyAfterThinking {
            provider: provider.clone(),
        });
    }
    JobEvent::Finished(JobResult {
        job_id,
        provider_id: provider.clone(),
        model_id: tally.model_id.as_deref().map(ModelId::from),
        payload: ResultPayload::Text(tally.accumulated),
        usage: tally.usage,
        timings: Timings::default(),
        output_truncated: tally.output_truncated,
        finished_at: Timestamp::ZERO,
    })
}

/// 一次性失败流（构造前就能判定的那些）。
fn once_failed(err: JobError) -> JobEventStream {
    Box::pin(stream! {
        yield JobEvent::Started;
        yield JobEvent::Failed(err);
    })
}

#[async_trait]
impl AIProvider for CloudLlmProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(CLOUD_LLM_PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: self.config.display_name.clone(),
            model: None,
            // 标错这一项，严格本地模式就拦不住它（`routing.local_only` 按 location 过滤）。
            location: ProviderLocation::Cloud,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: served_under(self.config.allowed.as_deref())
                .into_iter()
                .collect(),
            streaming: true,
            location: ProviderLocation::Cloud,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // 不主动探活——那是一次没有任何用户动作能解释的云请求（ADR-037），且真实可用性
        // 在首个任务上就会暴露。可用性由失败计数熔断表达，与 `LlmHttpProvider` 同纪律。
        self.breaker.health()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let provider_id = self.id();
        let capability = job.kind.capability();
        // **在这里推导一次，之后全程带着它**（ADR-083 ③）：档位是 Job 形状的函数，
        // 不是提交它的人自报的。`site` 同时是错误映射那侧要用的那一格——两处各推一次的
        // 表现是「请求按实时档发出去、错误却按批量档解释」。
        let service_class = job.service_class();
        let site = CallSite {
            capability,
            class: service_class,
        };

        // 严格本地：一个请求都不发，且**在取凭据之前**判——顺序反了的话，「先探一下
        // 服务端再发现不该发」本身已经是一次可观测的云网络行为。
        if !self.config.gate.allows_outbound() {
            return once_failed(JobError::StrictLocalBlocked { capability });
        }
        // 版本与消息同时取（见 `AssembledPrompt`）：目录可能在两次调用之间刷新，
        // 事后再问一次注册表拿到的就不是合成这批消息的那一版。
        let assembled = build_chat_messages(luna_core::prompt::registry(), &job);
        let messages = assembled.messages;
        let prompt_version = assembled.prompt_version;
        let prompt_variant = assembled.prompt_variant;
        if messages.is_empty() {
            return once_failed(JobError::InvalidInput(
                "本 Provider 不支持该 Job 类型".to_owned(),
            ));
        }
        // 未登录 / 无凭据时**一个字节都不发**（ADR-037 零云网络行为不变量）。
        let Some(token) = self.config.bearer.bearer().await else {
            return once_failed(JobError::Unauthorized {
                provider: provider_id,
            });
        };

        let http = self.http.for_url(&self.config.api_base);
        let breaker = Arc::clone(&self.breaker);
        let bearer = Arc::clone(&self.config.bearer);
        let gate = Arc::clone(&self.config.gate);
        let base = self.config.api_base.trim_end_matches('/').to_owned();
        let job_id = job.id;
        // 输出预算：不给足的话，长改写会被腰斩在半句话上且不报错（同 `JobOptions` 的注释）。
        let max_tokens = job.options.output_budget;

        Box::pin(stream! {
            yield JobEvent::Started;

            // 熔断打开时**不再去连**——端点挂掉时此前每一次对话都要各等一次连接超时。
            if !breaker.health().is_ready() {
                yield JobEvent::Failed(JobError::ProviderUnavailable { provider: provider_id.clone() });
                return;
            }

            let body = JobRequestBody {
                capability: wire_capability(capability),
                service_class: service_class.as_str(),
                messages: messages
                    .into_iter()
                    .map(|m| WireMessage { role: m.role, content: m.content })
                    .collect(),
                max_tokens,
                prompt_version,
                prompt_variant: prompt_variant.clone(),
            };
            let url = format!("{base}{NATIVE_PATH}/jobs");

            // 401 → 刷新一次再试。一次长生成可能跨过 access token 的有效期，而
            // `luna-cloud` 的 `authed` 早就是这个形状——同一条纪律带到 Provider 侧。
            let mut token = token;
            let mut response = None;
            for attempt in 0..2u8 {
                // 取凭据/刷新凭据都可能跨 await；真正 send 前必须重问，启动时那次判定不能代替它。
                if !gate.allows_outbound() {
                    yield JobEvent::Failed(JobError::StrictLocalBlocked { capability });
                    return;
                }
                // 幂等键必须在**循环内、每一发都带**，且两发是同一个值——
                // 401 刷新后那一发正是「同一次操作的重发」，而第一发可能已经被服务端
                // 收下并开始计费（见 `crate::idempotency`）。
                let sent = tokio::time::timeout(
                    RESPONSE_HEADERS_DEADLINE,
                    crate::idempotency::tag(
                        http.post(&url).bearer_auth(&token).json(&body),
                        job_id,
                    )
                    .send(),
                )
                .await;
                let r = match sent {
                    Ok(Ok(r)) => {
                        // 抵达了端点——哪怕它随后返回 4xx/5xx，端点本身是活的。
                        breaker.record_reachable();
                        r
                    }
                    // 响应头死线：连接池里的死连接与半开链路都落在这里（见常量注释）。
                    Err(_elapsed) => {
                        breaker.record_unreachable();
                        yield JobEvent::Failed(JobError::Timeout);
                        return;
                    }
                    Ok(Err(e)) => {
                        // 只有「没能抵达」才计入熔断：额度用尽 / 未登录 / 参数错都证明端点
                        // 活着，且各有各的可执行动作，计入会让用户修好后还得再等一个退避周期。
                        let failure = if e.is_timeout() {
                            breaker.record_unreachable();
                            JobError::Timeout
                        } else if e.is_connect() {
                            breaker.record_unreachable();
                            JobError::ProviderUnavailable { provider: provider_id.clone() }
                        } else {
                            breaker.record_reachable();
                            JobError::Upstream {
                                provider: provider_id.clone(),
                                code: None,
                                message: e.to_string(),
                            }
                        };
                        yield JobEvent::Failed(failure);
                        return;
                    }
                };
                if r.status() != reqwest::StatusCode::UNAUTHORIZED || attempt == 1 {
                    response = Some(r);
                    break;
                }
                if !gate.allows_outbound() {
                    yield JobEvent::Failed(JobError::StrictLocalBlocked { capability });
                    return;
                }
                // 传刚被拒的这一枚：几路 Provider 撞上同一次过期时，只有第一路真轮换，
                // 其余复用（见 `BearerSource::refresh`）。
                let Some(fresh) = bearer.refresh(&token).await else {
                    yield JobEvent::Failed(JobError::Unauthorized {
                        provider: provider_id.clone(),
                    });
                    return;
                };
                token = fresh;
            }
            // 从这里起每一个错误都是**这一枚**凭据换来的——问一次，下面两条错误路径共用。
            let Some(response) = response else {
                yield JobEvent::Failed(JobError::Unauthorized {
                    provider: provider_id.clone(),
                });
                return;
            };

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let retry_after = crate::llm_http::retry_after_secs(response.headers());
                let body: Value = response.json().await.unwrap_or(Value::Null);
                let code = body.get("error").and_then(Value::as_str).unwrap_or("internal");
                let dimension = body
                    .get("dimension")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                log_server_detail(status, code, server_error_detail(&body));
                yield JobEvent::Failed(map_server_error_with_retry(
                    &provider_id,
                    site,
                    code,
                    dimension,
                    retry_after,
                ));
                return;
            }

            let mut sse = SseDecoder::new();
            let mut accumulated = String::new();
            // 服务端分配的 job id：显式取消要用它。拿不到就只能靠断流——服务端会因为
            // 客户端消失而取消，但那比主动 DELETE 慢一拍，且不适用于「连接还在、用户点了停」。
            let mut server_job_id: Option<String> = None;
            let mut executed_model: Option<String> = None;
            let mut terminal_error: Option<JobError> = None;
            // 见流末那段论证：服务端**保证**发终态帧，故「没见到」= 链路被截断。
            let mut saw_terminal = false;
            // 见过推理过程吗。零正文时它是唯一能把「思考吃光了预算」与「真的什么都没有」
            // 分开的信号——而原生面是唯一会单独告诉我们这件事的通路。
            let mut saw_thinking = false;
            // 服务端在终态帧带回来的用量（「取，不猜」，见 `parse_usage`）。
            let mut usage = Usage::default();
            // 输出触顶归属，同样只在终态帧里出现（见 `parse_output_truncated`）。
            let mut output_truncated: Option<TruncatedBy> = None;
            let mut bytes = response.bytes_stream();

            'outer: loop {
                // 控制面独立于上游分片：上游迟迟不出首字时，隐私撤销与用户取消也要在
                // `STREAM_CONTROL_POLL` 内生效，不能被 `bytes.next()` 无限卡住。
                if !gate.allows_outbound() {
                    // 隐私撤销后**零出站字节**：绝不再发 DELETE。丢弃响应流会断开下游连接，
                    // 服务端的断连守卫负责取消上游；这是隐私边界与业务取消的根本区别。
                    yield JobEvent::Failed(JobError::StrictLocalBlocked { capability });
                    return;
                }
                if cancel.is_cancelled() {
                    // **主动告诉服务端**，而不是只断本地连接（TDD-14 §3.3 硬约束）：
                    // 只断连的话上游还在跑、还在计费，用户点了停却仍被扣额度。
                    cancel_server_job(&http, &base, &token, server_job_id.as_deref()).await;
                    yield JobEvent::Cancelled;
                    return;
                }
                let chunk = tokio::select! {
                    chunk = bytes.next() => chunk,
                    () = tokio::time::sleep(STREAM_CONTROL_POLL) => continue,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield JobEvent::Failed(JobError::Upstream {
                            provider: provider_id.clone(),
                            code: None,
                            message: e.to_string(),
                        });
                        return;
                    }
                };
                let text = String::from_utf8_lossy(&chunk);
                for payload in sse.push(&text) {
                    let SsePayload::Data { event, json } = payload else {
                        continue; // 原生面不发 `[DONE]`，它有自己的 `done` 事件。
                    };
                    let value: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
                    match event.as_deref() {
                        Some("accepted") => {
                            server_job_id = value
                                .get("job_id")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            if let Some(who) = executor_from(&value) {
                                executed_model = Some(who.model.clone()).filter(|m| !m.is_empty());
                                yield JobEvent::ExecutedBy(who);
                            }
                        }
                        Some("delta") => {
                            if let Some(t) = value.get("text").and_then(Value::as_str) {
                                accumulated.push_str(t);
                                yield JobEvent::Partial(PartialPayload::Token(t.to_owned()));
                            }
                        }
                        // 推理过程**不进正文**。它与 `delta` 的载荷形状完全相同，靠形状
                        // 分不开——这正是 SSE 解码器必须带出事件名的原因。混进去的表现是
                        // 思考链被当成译文/摘要写进笔记。
                        //
                        // 但**丢弃 ≠ 假装没发生**：记一位，零正文时用它给出可执行的原因。
                        Some("thinking") => saw_thinking = true,
                        Some("error") => {
                            let code = value.get("error").and_then(Value::as_str).unwrap_or("internal");
                            let dimension = value
                                .get("dimension")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            terminal_error =
                                Some(map_server_error(&provider_id, site, code, dimension));
                            // 错误帧里服务端也带 `usage`：同一口径「取，不猜」（只认 `source ==
                            // upstream`，估算值按没说处理）。取到的经流末的 `JobEvent::Usage` 快照
                            // 送进 `JobQueue`，失败终态按它记账——失败任务的用量从此不恒记 0
                            // （TDD-14 §3 修订注，2026-09-06 收口）。
                            usage = parse_usage(&value);
                            saw_terminal = true;
                            break 'outer;
                        }
                        Some("done") => {
                            usage = parse_usage(&value);
                            output_truncated = parse_output_truncated(&value);
                            saw_terminal = true;
                            break 'outer;
                        }
                        _ => {}
                    }
                }
            }

            // 失败终态前先把服务端自报的用量快照送出去（成功终态的用量随 `JobResult` 走，
            // 不重复发；空快照不发——`Usage::default()` 就是"对方什么也没说"）。
            if terminal_error.is_some() && usage != Usage::default() {
                yield JobEvent::Usage(usage);
            }

            // 中途额度耗尽 / 上游失败：报错收尾。
            //
            // **已产出的部分不回退**（PRD-13-019 流式硬边界）——它早已作为
            // `JobEvent::Partial` 逐 token 发出去了，界面上就摆着。这里不再重发一遍
            // `Finished`：那会让同一段文本在流里出现两次，而「这次失败了」才是终态的事实。
            // 服务端那侧同样是「已产出照发、只终止剩余」，两边对同一件事的表达是一致的。
            yield stream_terminal(
                &provider_id,
                job_id,
                StreamTally {
                    accumulated,
                    saw_terminal,
                    saw_thinking,
                    error: terminal_error,
                    model_id: executed_model,
                    usage,
                    output_truncated,
                },
            );
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        // 定价表接入随计费纵切（纵切 3）。**不猜**：一个编出来的估算数字会被用户当成
        // 账单预期，而真实计费在服务端。
        None
    }
}

/// 显式取消服务端任务。尽力而为——失败不改变「本地已经停了」这个事实。
async fn cancel_server_job(http: &reqwest::Client, base: &str, token: &str, job_id: Option<&str>) {
    let Some(job_id) = job_id else {
        return;
    };
    let _ = http
        .delete(format!("{base}{NATIVE_PATH}/jobs/{job_id}"))
        .bearer_auth(token)
        .send()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 用例里的调用点：翻译 · 实时档。取一个**分档的**能力而不是 `chat`，是因为
    /// `route_lane_empty` 那条断言要能看出两格都被原样带了出来。
    const SITE: CallSite = CallSite {
        capability: Capability::Translate,
        class: ServiceClass::Realtime,
    };

    #[test]
    fn native_job_request_omits_client_model_selection() {
        let body = JobRequestBody {
            capability: "chat",
            service_class: "realtime",
            messages: vec![WireMessage {
                role: "user".to_owned(),
                content: "hello".to_owned(),
            }],
            max_tokens: None,
            prompt_version: 7,
            prompt_variant: None,
        };
        let json = serde_json::to_value(body).unwrap();
        assert!(
            json.get("model").is_none(),
            "原生云请求不得携带具体模型，null 也不应占据协议字段"
        );
        // **恒发**（不是 `skip_serializing_if`）：协议 v5 起服务端无 `serde(default)`，
        // 缺这一格直接 400。可选字段会把「调用方没想清楚」静默变成「按批量跑」。
        assert_eq!(
            json.get("service_class"),
            Some(&serde_json::json!("realtime")),
            "service_class 必须恒发——v5 服务端对缺它的请求回 400"
        );
        assert_eq!(
            json.get("prompt_version"),
            Some(&serde_json::json!(7)),
            "提示词版本必须恒发——可选字段会让服务端同时处理「没带」与「带了 0」"
        );
    }

    /// **提示词版本进请求体不得把正文也带进去**：`prompt_version` 是一个整数，
    /// 请求体里除既有的 messages 之外不得因本批多出任何新的自由文本字段。
    #[test]
    fn prompt_version_is_a_number_not_a_prompt() {
        let body = JobRequestBody {
            capability: "translate",
            service_class: "batch",
            messages: vec![WireMessage {
                role: "system".to_owned(),
                content: "SYSTEM TEMPLATE".to_owned(),
            }],
            max_tokens: None,
            prompt_version: 12,
            prompt_variant: Some("terse".to_owned()),
        };
        let json = serde_json::to_value(body).unwrap();
        assert!(json["prompt_version"].is_number());
        assert_eq!(json["prompt_variant"], serde_json::json!("terse"));
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec![
                "capability",
                "messages",
                "prompt_variant",
                "prompt_version",
                "service_class"
            ],
            "原生请求体的字段集合被改动了——多出来的每一个都要先回答「它会不会承载正文」"
        );
    }

    struct StaticBearer;

    #[async_trait]
    impl BearerSource for StaticBearer {
        async fn bearer(&self) -> Option<String> {
            Some("test-token".to_owned())
        }

        async fn refresh(&self, _rejected: &str) -> Option<String> {
            None
        }
    }

    struct MutableGate(AtomicBool);

    impl luna_core::OutboundGate for MutableGate {
        fn allows_outbound(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct ClosingBearer(Arc<MutableGate>);

    #[async_trait]
    impl BearerSource for ClosingBearer {
        async fn bearer(&self) -> Option<String> {
            self.0 .0.store(false, Ordering::SeqCst);
            Some("must-not-be-sent".to_owned())
        }

        async fn refresh(&self, _rejected: &str) -> Option<String> {
            panic!("隐私闸已关，不得刷新凭据")
        }
    }

    fn tally(accumulated: &str, saw_terminal: bool, saw_thinking: bool) -> StreamTally {
        StreamTally {
            accumulated: accumulated.to_owned(),
            saw_terminal,
            saw_thinking,
            error: None,
            model_id: Some("qwen3:8b".to_owned()),
            usage: Usage::default(),
            output_truncated: None,
        }
    }

    /// **只记服务端自报的实测值，估算值一律不记。**
    ///
    /// 服务端**总带** `usage`，并用 `source` 分辨这组数字是上游自报的（`upstream`）
    /// 还是它按字符兜底估的（`estimated`）。填估算值的代价是真实的：用户拿「活动 · 用量」
    /// 与账单对照，看到界面写 137、账单写 152，问起来谁都说不清哪个对——而 `None`
    /// （如实显示「未知」）不制造这个假锚点。服务端账本自己会记估算值并标注来源，
    /// 那是计费权威，与这里要回答的问题不是一件事。
    #[test]
    fn usage_takes_only_what_the_server_measured() {
        let upstream = serde_json::json!({
            "usage": { "tokens_in": 128, "tokens_out": 64, "source": "upstream" }
        });
        let taken = parse_usage(&upstream);
        assert_eq!(taken.tokens_in, Some(128));
        assert_eq!(taken.tokens_out, Some(64));

        // 估算：数字在，但我们不采信——改成 `Some(...)` 这条会红。
        let estimated = serde_json::json!({
            "usage": { "tokens_in": 128, "tokens_out": 64, "source": "estimated" }
        });
        assert_eq!(parse_usage(&estimated).tokens_in, None);
        assert_eq!(parse_usage(&estimated).tokens_out, None);

        // 没有 `source` 也不采信：分不清来源时按「不知道」处理，而不是按「实测」处理。
        let unlabelled = serde_json::json!({ "usage": { "tokens_in": 128 } });
        assert_eq!(parse_usage(&unlabelled).tokens_in, None);
        // 整个字段缺失（老服务端 / 非终态帧）：不 panic，归「未知」。
        assert_eq!(parse_usage(&serde_json::json!({})).tokens_in, None);
    }

    /// **截断归属只取服务端明说的那三种，认不出就当没说。**
    ///
    /// 猜一个归属的代价是把用户引去改一个改了也没用的设置：把「上游机器上限」说成
    /// 「你自己的 max_tokens 设小了」，用户调大之后还是同样截断，且再也不信这条提示。
    #[test]
    fn truncation_attribution_is_taken_only_when_the_server_says_which_cap_hit() {
        let by = |v: serde_json::Value| parse_output_truncated(&v);
        assert_eq!(
            by(serde_json::json!({ "output_truncated": "caller" })),
            Some(TruncatedBy::Caller)
        );
        assert_eq!(
            by(serde_json::json!({ "output_truncated": "upstream" })),
            Some(TruncatedBy::Upstream)
        );
        assert_eq!(
            by(serde_json::json!({ "output_truncated": "absolute" })),
            Some(TruncatedBy::Absolute)
        );
        // 认不出的取值：不猜，也不 panic。
        assert_eq!(by(serde_json::json!({ "output_truncated": "quota" })), None);
        // 字段整个缺席 —— 服务端侧的触顶语义在下一批才落地，在那之前**每一帧都长这样**。
        // 这不是兼容分支，是协议协商窗口内「服务端没告诉我们」的如实表达。
        assert_eq!(by(serde_json::json!({ "terminal": "finished" })), None);
    }

    /// **流断在半路必须报失败，不能报完成。**
    ///
    /// 服务端保证终态帧必发，故「没见到」= 链路被截断（中间设备提前关连接、服务端进程
    /// 消失、代理截断）。报 `Finished` 就是把半截译文说成完成——用户看到一段莫名断掉的
    /// 文本配一个成功状态，没有任何线索去归因。原生面是唯一分辨得出来的通路
    /// （兼容流的 `[DONE]` 不是强制的），这条钉的就是「别把这个信号丢了」。
    #[test]
    fn a_stream_that_ends_without_a_terminal_frame_is_a_failure() {
        let p = ProviderId::from(CLOUD_LLM_PROVIDER_ID);
        let truncated = stream_terminal(&p, luna_core::JobId::new(), tally("半截译", false, false));
        let JobEvent::Failed(err) = truncated else {
            panic!("截断被报成了完成——用户拿到半截文本却看到成功");
        };
        assert!(matches!(
            err,
            JobError::Upstream { code: Some(ref c), .. } if c == "TRUNCATED"
        ));

        // 见到 `done` 才是真完成，且已产出的正文照常交付。
        let done = stream_terminal(&p, luna_core::JobId::new(), tally("完整译文", true, false));
        assert!(matches!(
            done,
            JobEvent::Finished(JobResult { payload: ResultPayload::Text(ref t), .. }) if t == "完整译文"
        ));
    }

    /// **思考吃光预算导致的零正文，不是成功。**
    ///
    /// 服务端注释把它称为最难归因的一类：「不是失败，是静默的空」——用户看到一次成功、
    /// 什么都没有，而 token 照扣。原生面单独发 `thinking`，客户端因此第一次分得清
    /// 「思考吃光了」与「真的什么都没产出」。
    #[test]
    fn an_empty_result_after_thinking_is_reported_instead_of_passing_silently() {
        let p = ProviderId::from(CLOUD_LLM_PROVIDER_ID);
        let empty = stream_terminal(&p, luna_core::JobId::new(), tally("", true, true));
        let JobEvent::Failed(err) = empty else {
            panic!("一次「成功但什么都没有」被静默放过了");
        };
        // 判据落在**稳定码**上，不是 `Upstream` 里那个自述码：后者不进 DTO，
        // 断言它等于断言一个用户永远看不见的字段。
        assert_eq!(err.code(), "EMPTY_AFTER_THINKING");
        assert!(matches!(err, JobError::EmptyAfterThinking { .. }));

        // 没见过 thinking 的空结果**不套这句话**：那另有原因，
        // 把它说成「思考吃光了额度」会把人引到完全无关的方向。
        assert!(matches!(
            stream_terminal(&p, luna_core::JobId::new(), tally("", true, false)),
            JobEvent::Finished(_)
        ));
        // 有正文时思考只是过程，不影响结论。
        assert!(matches!(
            stream_terminal(&p, luna_core::JobId::new(), tally("有正文", true, true)),
            JobEvent::Finished(_)
        ));
    }

    /// 服务端明确说出来的错误，优先于我们推断的那两条。
    #[test]
    fn an_explicit_server_error_wins_over_inferred_ones() {
        let p = ProviderId::from(CLOUD_LLM_PROVIDER_ID);
        let mut t = tally("", false, true);
        t.error = Some(JobError::QuotaExhausted {
            dimension: Some("tokens_out".into()),
        });
        assert!(matches!(
            stream_terminal(&p, luna_core::JobId::new(), t),
            JobEvent::Failed(JobError::QuotaExhausted { .. })
        ));
    }

    /// wire 名是**跨仓契约**，逐条钉死。
    ///
    /// 派生出来的名字会跟着 Rust 侧改名悄悄变，而服务端那边不会跟着变——表现是某个能力
    /// 突然被判成「未知 capability」，且没有任何东西会在改名那一刻变红。
    #[test]
    fn wire_capability_names_match_the_server_contract() {
        // 服务端 `ip_domain::Capability::as_str` 的取值，逐字抄录。
        for (capability, wire) in [
            (Capability::Transcribe, "transcribe"),
            (Capability::Translate, "translate"),
            (Capability::Summarize, "summarize"),
            (Capability::Refine, "refine"),
            (Capability::Chat, "chat"),
            (Capability::Ocr, "ocr"),
            (Capability::Diarize, "diarize"),
            (Capability::Embed, "embed"),
            (Capability::Custom, "custom"),
        ] {
            assert_eq!(wire_capability(capability), wire);
        }
    }

    /// 每一档错误的**可重试性**都必须与「用户该做什么」一致。
    ///
    /// 归错档的代价在两个方向上都很实：把 401 判成可重试 = 反复重发一个必然失败的请求
    /// **服务端已经把错因写在响应体里了，字段名拼错的表现是「日志永远安静」**
    /// （ADR-103）。
    ///
    /// 判据钉在一份**逐字照服务端 `ErrorBody` 形状**的响应体上：三个可空字段各自
    /// 逐错误码确定在不在场（`error_description` 只随 `detail_is_public()` 白名单里的
    /// 码，`dimension` 只随 `quota_exhausted`），所以样本要同时覆盖「在场」与「缺席」
    /// 两种真实形态——只测在场的那一半，缺席时回落到什么样是没有判据的。
    #[test]
    fn the_detail_field_name_matches_the_server_error_body() {
        // 在场：上游全链拒绝。这句话逐字来自服务端 `upstream_status::map_status`。
        let rejected = serde_json::json!({
            "error": "invalid_input",
            "error_description":
                "上游拒绝了请求（HTTP 413）——内容超过了该上游的单请求上限；\
                 请在管理端为它登记 max_upload_bytes（超限的候选会自动让位），或缩短内容"
        });
        let detail = server_error_detail(&rejected).expect("服务端的说明被读丢了");
        assert!(detail.contains("413"), "取到的不是那句说明：{detail}");

        // 缺席：内部错误的细节只进服务端日志，响应体里**没有**这一格。
        let internal = serde_json::json!({ "error": "internal" });
        assert_eq!(server_error_detail(&internal), None);

        // 空串等同缺席——否则日志里会出现一行「说明=」，比明写「服务端未提供」更难读。
        let empty = serde_json::json!({ "error": "internal", "error_description": "" });
        assert_eq!(server_error_detail(&empty), None);

        // 与 `dimension` 是两格，不是一格：额度耗尽时两者可以同时在场，
        // 读串了的表现是日志里写着「说明=tokens_out」。
        let quota = serde_json::json!({
            "error": "quota_exhausted",
            "dimension": "tokens_out",
            "error_description": "本月度输出额度已用尽"
        });
        assert_eq!(server_error_detail(&quota), Some("本月度输出额度已用尽"));
    }

    /// （用户看到的是无声的卡死）；把限流判成不可重试 = 一次抖动就让任务永久失败。
    #[test]
    fn server_error_codes_map_to_actionable_job_errors() {
        let p = ProviderId::from(CLOUD_LLM_PROVIDER_ID);

        // 会话被拒 → 去账户页重新登录（TDD-01 §2.4）。
        let unauthorized = map_server_error(&p, SITE, "unauthorized", None);
        assert_eq!(unauthorized.code(), "SIGN_IN_REQUIRED");
        assert!(!unauthorized.is_retriable(), "凭据没换，重试还是 401");

        let quota = map_server_error(&p, SITE, "quota_exhausted", Some("tokens_out".into()));
        assert!(
            matches!(&quota, JobError::QuotaExhausted { dimension: Some(d) } if d == "tokens_out"),
            "丢掉维度 = 用户不知道该等周期重置还是找管理员加量：{quota:?}"
        );
        assert!(!quota.is_retriable());

        // 组织没开放（R4b）：不是「去登录」、不是「额度用尽」，可回落本机、不重试。
        let frozen = map_server_error(&p, SITE, ACCOUNT_FROZEN, None);
        assert_eq!(frozen.code(), "ACCOUNT_FROZEN");
        assert!(!frozen.is_retriable() && frozen.is_structural());
        // 本月预付上限（C4）：独立的码，不落成「额度用尽」（那一档给的是「购买」）。
        let capped = map_server_error(&p, SITE, SPEND_CAP_REACHED, None);
        assert_eq!(capped.code(), "SPEND_CAP_REACHED");
        assert!(!capped.is_retriable() && capped.is_structural());
        // 免费额度被扣下（C5）：独立的码，不落成「额度用尽」（那一档给的是「购买」）。
        let withheld = map_server_error(&p, SITE, FREE_QUOTA_WITHHELD, None);
        assert_eq!(withheld.code(), "FREE_QUOTA_WITHHELD");
        assert!(!withheld.is_retriable() && withheld.is_structural());
        let denied = map_server_error(&p, SITE, CAPABILITY_NOT_ALLOWED, None);
        assert_eq!(denied.code(), "CAPABILITY_NOT_ALLOWED");
        assert!(!denied.is_retriable());

        for retriable in ["rate_limited", "upstream_unavailable", "internal"] {
            let err = map_server_error(&p, SITE, retriable, None);
            assert!(
                err.is_retriable(),
                "{retriable} 是「稍后能好」的一类，判成不可重试会让一次抖动永久失败"
            );
        }
        for code in ["upstream_unavailable", "internal"] {
            let err = map_server_error(&p, SITE, code, None);
            assert!(
                matches!(
                    &err,
                    JobError::ServerSideFailure { code: actual, .. } if actual == code
                ),
                "{code} 是服务端已响应的业务失败，必须保留稳定码而不能伪装成链路不可达"
            );
            assert_eq!(
                err.code(),
                "SERVER_SIDE_FAILURE",
                "与 `UPSTREAM` 分家正是 ADR-097：那条说的是「我们直连的上游回了错」，\
                 用户手上有旋钮；这条说的是「服务端背后那一层出了问题」，他一个都没有"
            );
            assert_eq!(
                err.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
                luna_core::error::FailoverDisposition::Stop,
                "{code} 可重试不等于可静默换腿"
            );
        }
        // 服务端**配错了**的两个码：可重试是错的——重试永远得到同一个裁决，而队列会
        // 白烧候选链、界面写着「请重试」。它们此前落在 `other =>` 的 `Upstream`（可重试）里。
        for code in ["egress_blocked", "upstream_protocol"] {
            let err = map_server_error(&p, SITE, code, None);
            assert!(
                matches!(
                    &err,
                    JobError::ServerMisconfigured { code: actual, .. } if actual == code
                ),
                "{code} 必须归 ServerMisconfigured 并保留服务端码：{err:?}"
            );
            assert!(!err.is_retriable(), "{code} 重试永远不会成功");
            assert!(
                err.is_structural(),
                "{code} 换下一句也一样——逐句消费者要一次性停，而不是失败一百次"
            );
            assert_eq!(
                err.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
                luna_core::error::FailoverDisposition::Stop,
            );
        }
        let limited = map_server_error_with_retry(&p, SITE, "rate_limited", None, Some(9));
        assert!(matches!(
            limited,
            JobError::RateLimited {
                retry_after_secs: Some(9),
                ..
            }
        ));

        assert!(!map_server_error(&p, SITE, "invalid_input", None).is_retriable());
        assert!(matches!(
            map_server_error(&p, SITE, "cancelled", None),
            JobError::Cancelled
        ));

        // 认不出的码**保留原文**：新版服务端加一个码是正常演进，客户端不该假装认识它，
        // 但运维要能据此定位。
        assert!(matches!(
            map_server_error(&p, SITE, "brand_new_code", None),
            JobError::Upstream { code: Some(c), .. } if c == "brand_new_code"
        ));
    }

    /// 「字节有没有离开这台设备」由**客户端**填，且走到这里恒为真。
    ///
    /// 服务端把自己的运行时如实报成 `local_runtime`（对它而言确实是自有运行时）。
    /// 拿它当联网指示的判据，指示会在收到执行者回报的那一刻熄灭——而正文正在外发。
    /// 转写那条路上真机打出来过一次，这条防的是同一件事在对话路上重演。
    #[test]
    fn a_server_side_local_runtime_still_left_this_device() {
        let accepted = serde_json::json!({
            "job_id": "j1",
            "executed_by": {
                "upstream_name": "组织服务端",
                "kind": "local_runtime",
                "model": "qwen3:8b",
            },
        });
        let who = executor_from(&accepted).expect("应解析出执行者");
        assert_eq!(who.kind, ExecutorKind::LocalRuntime);
        assert!(
            who.leaves_device,
            "服务端的「本地运行时」被当成了「没出这台设备」——联网指示会误灭"
        );
        assert_eq!(who.model, "qwen3:8b");
    }

    /// 服务端报了我们不认识的执行者性质时，绝不能当成本地（同 `ExecutorKind` 的纪律）。
    #[test]
    fn unknown_executor_kind_stays_unknown() {
        let accepted = serde_json::json!({
            "executed_by": { "upstream_name": "x", "kind": "brand_new", "model": "m" },
        });
        assert_eq!(
            executor_from(&accepted).unwrap().kind,
            ExecutorKind::Unknown
        );
    }

    /// 声明的能力必须覆盖对话与翻译，且 `location` 必须是 `Cloud`。
    ///
    /// 空集 = 路由认为没有任何 Provider 支持对话 → 用户拿到 `NoProvider`，一条与真实
    /// 原因毫无关系、也无从下手的错误。`location` 标错则严格本地过滤不掉它。
    #[test]
    fn it_declares_the_capabilities_the_server_actually_serves() {
        struct NoBearer;
        #[async_trait]
        impl BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }
        let provider = CloudLlmProvider::new(CloudLlmConfig {
            api_base: "http://server.invalid".into(),
            display_name: "测试".into(),
            bearer: Arc::new(NoBearer),
            gate: Arc::new(luna_core::AllowOutbound),
            allowed: None,
        });
        let caps = provider.capabilities();
        for capability in SERVED_CAPABILITIES {
            assert!(
                caps.supports(capability),
                "少声明了 {capability:?}——那条路会以「无可用 Provider」失败"
            );
        }
        assert!(
            !caps.supports(Capability::Embed),
            "声明了本端点收不了的能力——服务端确实提供 `Embed`，但它走
             `/inference/v1/embeddings`（见 `Capability::CHAT_SHAPED`）。在这里声明它，
             路由就会把嵌入 Job 派给对话 Provider，然后拿回一句 invalid_input"
        );
        assert_eq!(
            caps.location,
            ProviderLocation::Cloud,
            "标错就拦不住严格本地"
        );
    }

    /// 知识管线（PRD-16）的每一种批处理 Job 都必须能在服务端跑。
    ///
    /// 它们是「点一次跑几十上百篇」的长任务，落点判错的代价不是一次失败而是整批中止；
    /// 而失败文案历来指向「检查本机引擎」，在服务端形态下把用户带向完全错误的方向。
    /// ADR-059 之前 `ExtractTags` 走 `Capability::Custom`——服务端不提供它，于是连着组织
    /// 服务端的用户，关系标签重扫**一篇都跑不成**，同屏对话却一切正常。
    ///
    /// 本用例与 `luna_core::job` 里那条「只有 `JobKind::Custom` 用 `Capability::Custom`」
    /// 成对：那条守住分类纪律，这条守住「分类之后服务端真的收」。
    #[test]
    fn every_knowledge_pipeline_job_is_servable_by_the_cloud_provider() {
        use luna_core::{EmbeddingPurpose, JobKind, SummaryStyle, TextRef};

        let knowledge_jobs = [
            // 关系标签重扫（graph.rs）
            JobKind::ExtractTags {
                text: TextRef::Inline(String::new()),
                instruction: luna_core::Instruction::default(),
            },
            // 深度体检 / 蒸馏 / 归入（checkup.rs、distill.rs、ingest.rs）
            JobKind::Summarize {
                text: TextRef::Inline(String::new()),
                style: SummaryStyle::Bullets,
                instruction: None,
            },
        ];
        for kind in &knowledge_jobs {
            assert!(
                SERVED_CAPABILITIES.contains(&kind.capability()),
                "{kind:?} 按 {:?} 路由，而本 Provider 不声明它——\
                 连上服务端的用户会看到这条知识链路整批中止，且看不出为什么",
                kind.capability()
            );
        }
        // 嵌入是知识管线的第四条腿，但它**刻意**不在这里：走 `/inference/v1/embeddings`
        // 与 `CloudEmbedProvider`，不是本 Provider 的责任（见上一条用例的 `Embed` 断言）。
        assert!(!SERVED_CAPABILITIES.contains(
            &JobKind::Embed {
                purpose: EmbeddingPurpose::Document,
                texts: Vec::new(),
            }
            .capability()
        ));
    }

    /// 只用于「401 → 刷新一次」那条路的替身：第一次给旧令牌，刷新后给新的。
    struct RefreshingBearer;

    #[async_trait]
    impl BearerSource for RefreshingBearer {
        async fn bearer(&self) -> Option<String> {
            Some("stale-token".to_owned())
        }

        async fn refresh(&self, rejected: &str) -> Option<String> {
            // 断言在替身里：调用方必须把**刚被拒的那一枚**递下来。
            // 真实现（`CloudBearer`）拿它跟钥匙链里的 access 比对来决定「轮换还是复用」，
            // 递错（例如回头重读一次凭据）会让它恒判「别人已经换好了」而永不轮换——
            // 401 就此死循环，而在只看重试次数的用例里完全看不出来。
            assert_eq!(
                rejected, "stale-token",
                "递下来的不是刚挨 401 的那一枚，二次检查会恒判「已被别人换走」"
            );
            Some("fresh-token".to_owned())
        }
    }

    /// 从一段原始 HTTP 请求里取一个头的值（大小写不敏感）。
    fn header_of(request: &str, name: &str) -> Option<String> {
        request.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    }

    /// **`route_lane_empty` 不可重试、不换腿，且把「哪一档的哪条链」原样带出来**
    /// （ADR-083 ⑧）。
    ///
    /// 落进 `other =>` 的 `Upstream`（可重试）会退回本批要消灭的形态：实时字幕逐句打一次
    /// 服务端、逐句失败、逐句回落本机——延迟翻倍，而用户看见的只是「译文慢」。而映射成
    /// `ProviderUnavailable` 更糟：那一条对「云 → 本机」明确回 `FallBackToLocal`。
    ///
    /// 反向验证：删掉 `map_server_error_with_retry` 里那条 `"route_lane_empty" =>` 臂，
    /// 三条断言里的前两条当场变红（它会掉进 `other =>` 的 `Upstream`）。
    #[test]
    fn an_empty_route_lane_is_not_a_transient_upstream_failure() {
        let provider = ProviderId::from(CLOUD_LLM_PROVIDER_ID);
        let mapped = map_server_error(&provider, SITE, "route_lane_empty", None);
        assert_eq!(mapped.code(), "ROUTE_LANE_EMPTY");
        assert!(
            !mapped.is_retriable(),
            "运营者排出来的名单不会因为再问一次就多出一个候选"
        );
        assert_eq!(
            mapped.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
            luna_core::error::FailoverDisposition::Stop,
            "逐句回落本机正是本条错误码要消灭的那个形态"
        );
        assert!(
            matches!(
                mapped,
                JobError::RouteLaneEmpty {
                    capability: Capability::Translate,
                    class: ServiceClass::Realtime
                }
            ),
            "两格都要原样带出来：说不清是哪一档空了，运营者不知道该去排哪条链"
        );
    }

    /// 服务端的幂等裁决必须落在**不可重试**的一侧，且带自己的稳定码。
    ///
    /// 落进 `other =>` 的 `Upstream` 是可重试的：队列会白烧候选链去重发一个**永远**
    /// 会得到同一裁决的请求，而界面显示一句无从下手的「上游出错」——前端读的是变体
    /// 投影出的稳定码，塞在 `Upstream.code` 里的字符串它根本看不到。同一个教训在
    /// `Unauthorized`（401 重试永远还是 401）与 `QuotaExhausted`（402）上各付过一次。
    #[test]
    fn idempotency_verdicts_are_not_retriable_and_carry_their_own_code() {
        let provider = ProviderId::from("lunarca-cloud");
        for code in ["idempotency_in_flight", "idempotency_replayed"] {
            let mapped = map_server_error_with_retry(&provider, SITE, code, None, None);
            assert_eq!(
                mapped.code(),
                "ALREADY_PROCESSED",
                "`{code}` 没有自己的稳定码，界面只能显示一句通用文案"
            );
            assert!(
                !mapped.is_retriable(),
                "`{code}` 落进了可重试桶——拿同一个幂等键重试永远不会成功"
            );
        }
        // 同键不同体是**我们这一侧**的 bug：重新发起一次也还是错的，要改的是键的生成。
        let reused =
            map_server_error_with_retry(&provider, SITE, "idempotency_key_reused", None, None);
        assert_eq!(reused.code(), "INVALID_INPUT");
        assert!(!reused.is_retriable());
    }

    /// **终态帧里的截断归属必须真的走完「解析 → 装进 `JobResult`」这条链。**
    ///
    /// 现网缺陷正是它的反面：生成中途撞上单请求 token 闸，整篇被判成额度耗尽并**丢掉已
    /// 生成的正文**，而用户的额度只用了四分之一。目标态是「正常收尾 + 显式标注」，那就
    /// 要求两件事同时成立——正文照常交付，且触顶这个事实有地方落。
    ///
    /// 这条起真 TCP 服务走整条流，而不是只喂 `stream_terminal` 一个手搓的 tally：
    /// 中间那一步（`done` 分支把解析结果赋回局部变量）是纯函数测试**够不到**的一行，
    /// 而它恰好是最容易漏写的一行——漏了照样编译、照样全绿，只是标注永远为空。
    #[tokio::test]
    async fn a_capped_generation_arrives_as_a_finished_result_that_says_which_cap_hit() {
        use futures::StreamExt as _;
        use luna_core::{JobKind, JobOrigin, SummaryStyle, TextRef};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8 * 1024];
            let _ = socket.read(&mut buf).await.unwrap();
            let body = concat!(
                "event: delta\ndata: {\"text\":\"半篇整理稿\"}\n\n",
                "event: done\ndata: {\"terminal\":\"finished\",\"output_truncated\":\"upstream\"}\n\n",
            );
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
        });

        let provider = CloudLlmProvider::new(CloudLlmConfig {
            api_base: format!("http://{address}"),
            display_name: "测试服务端".to_owned(),
            bearer: Arc::new(StaticBearer),
            gate: Arc::new(luna_core::AllowOutbound),
            allowed: None,
        });
        let job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline("很长的会议记录".to_owned()),
                style: SummaryStyle::Bullets,
                instruction: None,
            },
            JobOrigin::FloatingMenu,
        );
        let mut events = provider.execute(job, CancelToken::new()).await;
        let mut finished = None;
        while let Some(event) = events.next().await {
            if let JobEvent::Finished(result) = event {
                finished = Some(result);
            }
        }
        server.await.unwrap();

        let result = finished.expect("触顶被报成了失败——已生成的正文连同额度一起蒸发");
        assert_eq!(
            result.output_truncated,
            Some(TruncatedBy::Upstream),
            "终态帧说了是哪道闸，结果里却没有——标注与补齐入口都会因此永远不出现"
        );
        assert!(
            matches!(result.payload, ResultPayload::Text(ref t) if t == "半篇整理稿"),
            "触顶收尾必须保留已产出的正文，这正是现网缺陷丢掉的东西"
        );
    }

    /// **401 刷新后的那一发必须带同一个幂等键**（ADR-044 ②）。
    ///
    /// 这条路是幂等键存在的头号理由：一次长生成可能跨过 access token 的有效期，而**第一发
    /// 可能已经被服务端收下并开始计费**。两发换了键，服务端就把它当成两次操作——重复扣费，
    /// 且没有任何东西会变红。故这里断言的是「两发的键**相等**」，而不只是「带了这个头」。
    #[tokio::test]
    async fn the_retry_after_a_401_reuses_the_same_idempotency_key() {
        use futures::StreamExt as _;
        use luna_core::{JobKind, JobOrigin, Lang, TextRef};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let keys = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let probe = Arc::clone(&keys);

        let server = tokio::spawn(async move {
            for attempt in 0..2u8 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8 * 1024];
                let read = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..read]).into_owned();
                probe.lock().unwrap().push(
                    header_of(&request, "idempotency-key").expect("每一发都必须带 Idempotency-Key"),
                );
                if attempt == 0 {
                    // 第一发：401 —— 逼客户端刷新凭据后原样重发。
                    socket
                        .write_all(
                            b"HTTP/1.1 401 Unauthorized
Content-Length: 0

",
                        )
                        .await
                        .unwrap();
                } else {
                    let done = "event: done\ndata: {\"terminal\":\"finished\"}\n\n";
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK
Content-Type: text/event-stream
                                 Content-Length: {}

{done}",
                                done.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                }
                socket.flush().await.unwrap();
            }
        });

        let provider = CloudLlmProvider::new(CloudLlmConfig {
            api_base: format!("http://{address}"),
            display_name: "测试服务端".to_owned(),
            bearer: Arc::new(RefreshingBearer),
            gate: Arc::new(luna_core::AllowOutbound),
            allowed: None,
        });
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("你好".to_owned()),
                source: None,
                target: Lang::from("en"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        let mut stream = provider.execute(job.clone(), CancelToken::new()).await;
        while stream.next().await.is_some() {}
        server.await.unwrap();

        let keys = keys.lock().unwrap().clone();
        assert_eq!(keys.len(), 2, "应当发了两次（401 之后重发一次）");
        assert_eq!(
            keys[0], keys[1],
            "两发的幂等键必须相同——不同就是两次操作，服务端会重复计费"
        );
        assert_eq!(
            keys[0],
            job.id.to_string(),
            "键取 JobId：重发不换、重问才换（见 `crate::idempotency`）"
        );
    }

    /// 严格本地在流中开启后，客户端从那一刻起不能再发一个字节。
    ///
    /// 这条起真实 TCP 服务并让 Provider 先拿到服务端 job id；如果实现把隐私撤销复用成
    /// “用户取消”，它一定会再连一次 `DELETE /jobs/{id}`，用例会直接看到，而不是只验证
    /// 返回的错误枚举看起来正确。
    #[tokio::test]
    async fn privacy_revoke_mid_stream_sends_no_cleanup_request() {
        use futures::StreamExt as _;
        use luna_core::{JobKind, JobOrigin, Lang, TextRef};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let delete_seen = Arc::new(AtomicBool::new(false));
        let delete_probe = Arc::clone(&delete_seen);
        let server = tokio::spawn(async move {
            let (mut post, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 8 * 1024];
            let read = post.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..read]).starts_with("POST /inference/v1/jobs "),
                "首个请求必须是创建任务"
            );
            let accepted = "event: accepted\ndata: {\"job_id\":\"server-job-1\",\"executed_by\":{\"upstream_name\":\"stub\",\"kind\":\"local_runtime\",\"model\":\"m\"}}\n\n";
            let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n";
            post.write_all(headers.as_bytes()).await.unwrap();
            post.write_all(format!("{:X}\r\n", accepted.len()).as_bytes())
                .await
                .unwrap();
            post.write_all(accepted.as_bytes()).await.unwrap();
            post.write_all(b"\r\n").await.unwrap();
            post.flush().await.unwrap();

            // 隐私撤销后若错误地走“业务取消”，reqwest 会新建连接发送 DELETE。
            if let Ok(Ok((mut cleanup, _))) =
                tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept()).await
            {
                let read = cleanup.read(&mut request).await.unwrap_or(0);
                if String::from_utf8_lossy(&request[..read]).starts_with("DELETE ") {
                    delete_probe.store(true, Ordering::SeqCst);
                }
            }
        });

        let gate = Arc::new(MutableGate(AtomicBool::new(true)));
        let provider = CloudLlmProvider::new(CloudLlmConfig {
            api_base: format!("http://{address}"),
            display_name: "测试服务端".to_owned(),
            bearer: Arc::new(StaticBearer),
            gate: gate.clone(),
            allowed: None,
        });
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("private text".to_owned()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        let mut events = provider.execute(job, CancelToken::new()).await;
        assert!(matches!(events.next().await, Some(JobEvent::Started)));
        assert!(matches!(events.next().await, Some(JobEvent::ExecutedBy(_))));

        gate.0.store(false, Ordering::SeqCst);
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(150), events.next())
            .await
            .expect("隐私闸必须独立于上游分片，在控制周期内生效");
        assert!(matches!(
            blocked,
            Some(JobEvent::Failed(JobError::StrictLocalBlocked { .. }))
        ));
        server.await.unwrap();
        assert!(
            !delete_seen.load(Ordering::SeqCst),
            "隐私撤销后仍发送了 DELETE；网络语义虽是取消，隐私语义已经越界"
        );
    }

    #[tokio::test]
    async fn privacy_gate_is_rechecked_after_credential_await() {
        use luna_core::{JobKind, JobOrigin, Lang, TextRef};

        let gate = Arc::new(MutableGate(AtomicBool::new(true)));
        let provider = CloudLlmProvider::new(CloudLlmConfig {
            api_base: "http://127.0.0.1:1".to_owned(),
            display_name: "不得连接".to_owned(),
            bearer: Arc::new(ClosingBearer(gate.clone())),
            gate,
            allowed: None,
        });
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("private".to_owned()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        let events: Vec<_> = provider
            .execute(job, CancelToken::new())
            .await
            .collect()
            .await;
        assert!(matches!(events.first(), Some(JobEvent::Started)));
        assert!(matches!(
            events.last(),
            Some(JobEvent::Failed(JobError::StrictLocalBlocked { .. }))
        ));
    }
}
