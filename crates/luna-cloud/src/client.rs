//! Control Plane HTTP 客户端（业务层，前端零业务的“业务”都在这里）。
//!
//! 会话语义与服务端对齐（cp-api M1）：
//! - access = 短 TTL EdDSA JWT；refresh = 不透明串，**轮换 + 复用即家族全吊销**。
//! - 任一鉴权请求 401 → **静默 refresh 一次**并重试；仍失败 → 清空本地会话并上抛
//!   [`CloudError::NeedsRelogin`]（UI 引导重新登录）。
//! - device-code 配对流：`login_start` 发起并持有 device_code（**秘密不过前端**），
//!   `login_poll` 单步轮询——`slow_down` 时按 RFC 8628 §3.5 把间隔 +5s，
//!   下次轮询时机由 Rust 计算（`next_poll_ms`），前端只负责定时器与渲染。

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::account::{
    AccountPhaseDto, AccountServer, AccountStateDto, AccountStore, CodeKind, CodeSignInDto,
    IncomingLinkDto, PasswordSignInDto, SignInMethodsDto, SignInProviderDto, SignInStepDto,
    SignInTarget,
};
use crate::account_data::{AccountDeleteDto, AccountDeletionPreviewDto};
use crate::billing::{
    map_billing_error, map_verify_card_error, BillingOfferDto, BillingStandingDto, FreeQuotaDto,
};
use crate::config::CloudConfig;
use crate::device_limit::{DeviceLimitPending, DeviceLimitResolutionDto, SignInRetry};
use crate::error::CloudError;
use crate::error::LoginRejection;
use crate::fingerprint::FingerprintProvider;
use crate::first_contact::{self, Channel, Purpose};
use crate::lease::{self, LeaseError};
use crate::opaque_auth::{self, BundleTls, Suite};
use crate::protocol::{self, DeploymentFacts, ServiceModelDto};
use crate::store::{CredentialStore, KEY_ACCESS, KEY_PROFILE, KEY_REFRESH, KEY_WRITE_PROBE};
use crate::tls_root::TlsRootStore;
use crate::trust::{
    OfferedAnchor, PinnedAnchor, RotationStatement, TrustProvenance, TrustStatusDto, TrustStore,
    TrustVerdict,
};

/// `slow_down` 时的间隔增量（RFC 8628 §3.5：轮询过快须把 interval 增加 5 秒）。
const SLOW_DOWN_STEP_SECS: u64 = 5;

// ---- typed DTO（tauri-specta 导出为 TS；也是集成测试断言的契约面）----

#[derive(Debug, Clone)]
pub struct LoginStartDto {
    /// 展示给用户的配对码（如 `ABCD-EFGH`）。
    pub user_code: String,
    /// 浏览器批准页地址。
    pub verification_uri: String,
    /// 带 user_code 预填参数的批准页地址（自动开浏览器用）。
    pub verification_uri_complete: String,
    /// 建议轮询间隔（毫秒）。
    pub next_poll_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginPollStatus {
    /// 等待用户在浏览器批准；按 `next_poll_ms` 继续轮询。
    Pending,
    /// 服务端要求放慢；间隔已 +5s，按 `next_poll_ms` 继续轮询。
    SlowDown,
    /// 登录完成，凭据已入钥匙链。
    Authorized,
    /// 配对码过期/失效；需重新发起 `login_start`。
    Expired,
    /// 浏览器那边批准了，但这个账户的设备已满（403 `device_limit_reached`，纵切 3 C5）：配对**留着**，
    /// 停止轮询，等用户在应用里移除一台（[`CloudClient::sign_in_resolve_device_limit`]）后接着轮询。
    DeviceLimitReached,
}

#[derive(Debug, Clone)]
pub struct LoginPollDto {
    pub status: LoginPollStatus,
    /// 下次轮询的等待毫秒数（终态为 `None`）。
    pub next_poll_ms: Option<u32>,
    /// `Authorized` 时的账户邮箱（来自 /v1/me）。
    pub email: Option<String>,
}

/// 账户档案（登录成功时缓存进钥匙链，供账户状态离线读取）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ProfileDto {
    pub account_id: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DeviceDto {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    /// `active` / `revoked`（服务端枚举的线上编码）。
    pub status: String,
    /// RFC 3339 时间戳（渲染归前端，本地化在 UI 层）。
    pub created_at: String,
    pub last_seen_at: Option<String>,
    /// 是否为当前设备（服务端按请求凭据判定）。
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct EntitlementDto {
    pub key: String,
    /// 权益值的原始 JSON 文本（服务端数据驱动、值形状任意——PRD-13 §1.2；
    /// specta 无法导出 `serde_json::Value`，前端按需 `JSON.parse`）。
    pub value_json: String,
}

/// 一维的额度与已用。
///
/// `limit = None` **即不限**——客户端据此渲染「不限」而不是一根满进度条（把不限画成
/// 满进度条，用户会以为自己快用完了）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct UsageDimensionDto {
    /// 稳定维度名（`tokens_in` / `tokens_out` / `egress_bytes` / `requests` / …）。
    pub dimension: String,
    /// **f64 而不是 u32**：账本是**物理量**（ADR-039），
    /// 出站字节这一维天然超过 42 亿——默认策略的出站上限就是 5 GiB = 5,368,709,120。
    ///
    /// 此前这里写的是 `u32`，理由是「specta 禁 64 位整数入 TS」，并**假定**服务端会在
    /// 溢出时改用更粗的单位下发。那个假定从未在服务端侧落实，也没有任何东西检查它，
    /// 于是真机上出现了这样一条链路：`/v1/usage` 正常返回 200 → 客户端反序列化失败 →
    /// 失败被当成网络错误 → 界面说「连不上这台服务端」，**而同一页的权益、租约、
    /// 设备全都刚从那台服务端拉回来**（真机坐实 2026-08-10）。
    ///
    /// 正确的类型是 f64：TS 的 `number` 本来就是 f64，整数在 2^53 内精确——覆盖到 PB 级
    /// 字节。u32 是在 TS 之上**凭空更窄**的一道约束，而收窄的代价由用户承担。
    pub limit: Option<f64>,
    pub used: f64,
    /// 这一维到了哪一档（协议 v12，**服务端判定、必填**）。这是客户端唯一的一份档位：账户页的徽标
    /// 色调、阈值通知（`crate::usage::UsageAlertEpisode`）、同步存储容量的「快满了 / 满了」读的都是它，
    /// 客户端不再拿阈值自己比一遍（ADR-121 ⑦）。
    pub alert: UsageAlertDto,
}

/// 用量档位（协议 v12 线上原样：`none` / `p80` / `p95` / `exhausted`；阈值是 `PRD-13-040` ③ 的
/// 80% / 95%，**只在服务端**）。顺序即严重程度（派生 `Ord`）：同一轮预警只在「升到更高一档」时提醒，
/// 靠的就是这个顺序。认不出的档位是读不懂，不猜。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum UsageAlertDto {
    None,
    P80,
    P95,
    /// 已用尽。不与「即将耗尽」混说：一个是快没了，一个是已经没了，下一步不同。
    Exhausted,
}

impl UsageAlertDto {
    /// 稳定名（与线上一致；通知种类名由它拼出）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::P80 => "p80",
            Self::P95 => "p95",
            Self::Exhausted => "exhausted",
        }
    }
}

/// 已用占上限的比例；无上限或上限为 0 时没有比例可言。**只用来排序**（最紧的一维排最前，
/// ADR-121 ⑥），不用来判档——档位只认服务端那一格。
///
/// 上限 0 不当成「已经 100%」：那一档是「不包含」，画成满条就是把「你没有这项」
/// 显示成「你快用完了」——与「不限画成满条」同一类误导，方向相反。
pub(crate) fn usage_ratio(dim: &UsageDimensionDto) -> Option<f64> {
    dim.limit
        .filter(|limit| *limit > 0.0)
        .map(|limit| dim.used / limit)
}

/// 一本账（PRD-13-038）。
///
/// # 用量是**账本的列表**，不是「一份额度 + 一个可选的余额字段」
///
/// 订阅额度按周期重置，预付余额不重置——两者性质不同，用户必须能**分别**看到。
/// 一个混合数字既回答不了「这个月还剩多少」，也回答不了「我买的流量包用掉多少」。
///
/// 预付账本随计费纵切才有生产者，故今天**列表里没有这一条**（而不是有一条值为 0 的
/// ——那会让用户以为自己有一个空的流量包）。列表让「预付上线」不改 wire 类型：
/// 多一个元素而已，两端都不需要兼容分支。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct UsageLedgerDto {
    /// `subscription`（周期额度）/ `prepaid`（预付余额）。
    pub kind: String,
    /// 周期（`none` / `daily` / `weekly` / `monthly`）——用户关心「什么时候重置」。
    /// 预付账本恒 `none`：它不重置。**字段恒在**，故消费者不必分辨「没有周期」与
    /// 「周期未知」两种缺失。
    pub period: String,
    pub dimensions: Vec<UsageDimensionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct EntitlementsDto {
    pub tier: String,
    pub entitlements: Vec<EntitlementDto>,
    /// Lease 是否通过本地 Ed25519 验签 + 过期检查。
    pub lease_verified: bool,
    /// Lease 过期时刻（Unix 秒；验签通过时给出）。
    /// u32：specta 禁 64 位整数入 TS；2106 年前值域足够，短 TTL 租约远够。
    pub lease_expires_at: Option<u32>,
    /// 未验签原因（`no_pubkey` / `bad_signature` / `expired` / …；给诊断与 i18n 映射）。
    pub lease_error: Option<String>,
    /// 这个账户可用的 AI 能力（权益 `inference.capabilities`，已解析；`None` = 服务器没给）。
    /// 账户页「AI 能力」一段直接渲染它，不再从 `entitlements` 的 JSON 里自己挖。
    pub capabilities: Option<Vec<String>>,
    /// 付款处境（协议 v11 必填，纵切 3 C3）。
    pub billing_standing: BillingStandingDto,
    /// 免费额度处境（协议 v13 必填，纵切 3 C5）：账户页「订阅与用量」按它说明为什么领不到、该做什么。
    pub free_quota: FreeQuotaDto,
}

/// 权益键：账户可用的 AI 能力（与服务端 `cp_domain::entitlement::INFERENCE_CAPABILITIES` 一致）。
pub const ENTITLEMENT_INFERENCE_CAPABILITIES: &str = "inference.capabilities";

/// 权益键：同步的单文件上限（字节，与服务端 `cp_domain::entitlement::SYNC_MAX_FILE_BYTES` 一致；
/// ADR-156 ④）。没有生效策略时为 null。
pub const ENTITLEMENT_SYNC_MAX_FILE_BYTES: &str = "sync.max_file_bytes";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct LogoutDto {
    /// 服务端会话是否成功吊销（网络不可达时为 false——本地凭据仍已清除，如实上报）。
    pub server_revoked: bool,
}

// ---- 内部：服务端响应形状 ----

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: i64,
    expires_in: i64,
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: String,
}

/// `GET /inference/v1/models` 响应外壳。
#[derive(Debug, Deserialize)]
struct ServiceModelsResponse {
    models: Vec<ServiceModelDto>,
}

/// `GET /v1/trust/keys` 响应（公开只读；线格式逐字见 ADR-071）。
#[derive(Debug, Deserialize)]
struct TrustKeysResponse {
    format_version: u32,
    deployment_id: String,
    lease_pubkey: String,
    catalog_pubkey: String,
    /// 轮换声明链（ADR-124）。**旧服务端没有这一格**，`default` 即空链——空链验不出
    /// 任何东西，于是行为原样退回阻断 + 人眼核对。这正是「不抬协议号」成立的原因。
    #[serde(default)]
    rotations: Vec<crate::trust::RotationStatement>,
}

/// 某一维在**全部账本上**还剩多少（PRD-13-040 ② 的前置提示要它）。
///
/// # `None` 的唯一含义是「读不到 / 不设限」，调用方一律据此**放行**
///
/// 前置预检是善意提醒，绝不能变成新的卡点：未登录、离线、私有化部署、本机模型、
/// 服务端根本不按这一维计量、以及任何一本账写着「不限」——这些都必须让任务照常起跑。
/// 把「不知道」当成「不够」，代价是给一整批完全没有额度概念的用户造一道假门。
///
/// 有限的那些账本**求和**：订阅额度与预付余额性质不同但都能花在同一件事上，
/// 分别看会让"订阅用完了、可我刚买了流量包"的用户被拦下。
///
/// **按 credit 计的预付余额还有剩时同样读不到**：预付账本按 credit 计（一维 `credits`），它能折成多少个
/// `tokens_out` 只有服务端的费率表知道。此时把订阅那一本的余量当成全部，会把「订阅用完了、
/// 可我刚买了流量包」的用户拦在门外——够不够交给任务前预估（`POST /v1/usage/estimate`）与服务端预留。
#[must_use]
pub fn remaining_in_dimension(ledgers: &[UsageLedgerDto], dimension: &str) -> Option<f64> {
    let prepaid_left = ledgers
        .iter()
        .filter(|ledger| ledger.kind == crate::usage::PREPAID_LEDGER)
        .filter(|ledger| !ledger.dimensions.iter().any(|d| d.dimension == dimension))
        .flat_map(|ledger| &ledger.dimensions)
        .any(|dim| dim.limit.is_none_or(|limit| limit > dim.used));
    if prepaid_left {
        return None;
    }
    let dims: Vec<&UsageDimensionDto> = ledgers
        .iter()
        .flat_map(|ledger| &ledger.dimensions)
        .filter(|dim| dim.dimension == dimension)
        .collect();
    if dims.is_empty() {
        return None; // 服务端不按这一维计量：没有可比的数。
    }
    let mut total = 0.0f64;
    for dim in dims {
        // 一本账写着「不限」就整体不限：另几本再有限也拦不住这一次。
        let limit = dim.limit?;
        total += (limit - dim.used).max(0.0);
    }
    Some(total)
}

#[derive(Deserialize)]
struct MeResponse {
    account_id: String,
    email: String,
}

#[derive(Deserialize)]
struct EntitlementsResponse {
    tier: String,
    entitlements: Vec<RawEntitlement>,
    lease: String,
    billing_standing: BillingStandingDto,
    free_quota: FreeQuotaDto,
}

/// 服务端权益条目的线上形状（value = 任意 JSON；进 DTO 前转为原始 JSON 文本）。
#[derive(Deserialize)]
struct RawEntitlement {
    key: String,
    value: serde_json::Value,
}

struct PendingLogin {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    interval: Duration,
    expires_at: Instant,
}

/// 登录第一步定下来的事：哪台服务器、哪个邮箱、要核对的部署、走哪条 TLS。
#[derive(Debug, Clone)]
struct SignInContext {
    server: AccountServer,
    /// 规整后的邮箱（[`opaque_auth::normalize_email`]）。
    email: String,
    /// 邮箱域名上 `.well-known/lunarca` 发布的部署：握手与信任包都必须是它。
    expected_deployment: Option<String>,
    /// 经校验的 TLS（钉住的根或系统信任库）已经连得上这台：握手就走它，不放宽。
    trusted_tls: bool,
}

/// 一次性码换来的注册授权（服务端 `expires_in` 秒内有效）。
struct RegistrationGrant {
    grant: zeroize::Zeroizing<String>,
    email: String,
    deployment_id: String,
    expires_at: Instant,
}

/// 握手并领取之后的两种结局。
enum ClaimOutcome {
    /// 领到了：`answer = None` 表示撞上设备上限（已挂起等用户选）。
    Claimed {
        answer: Option<serde_json::Value>,
        deployment_id: String,
        email: String,
    },
    /// 两把签名公钥与本机钉住的不一致：先确认。
    NeedsTrust(TrustStatusDto),
}

/// `.well-known/lunarca` 的形状（TDD-16 §5.2a）。
#[derive(Deserialize)]
struct WellKnown {
    format_version: u32,
    server: String,
    deployment_id: String,
}

/// 一次性码换来的注册授权（`POST /v1/auth/opaque/session`，`purpose=code`）。
#[derive(Deserialize)]
struct RegistrationGrantResponse {
    registration_grant: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct RegisterStartResponse {
    registration_id: String,
    registration_response: String,
}

/// 领取设备会话（只走钉住的 TLS）。
const SESSION_PATH: &str = "/v1/auth/opaque/session";
/// 注册口令第一段（带 Bearer）。
const REGISTER_START_PATH: &str = "/v1/auth/opaque/register/start";
/// 注册口令第二段（带 Bearer）：建设备会话。
const REGISTER_FINISH_PATH: &str = "/v1/auth/opaque/register/finish";
/// 邮箱域名上的发现文档。
const WELL_KNOWN_PATH: &str = "/.well-known/lunarca";
/// 取发现文档最多等多久：取不到就跳过，不该让「下一步」卡住。
const WELL_KNOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ---- 客户端 ----

pub struct CloudClient {
    /// 按服务端取客户端：钉了根证书的服务端只认那张根（ADR-153），其余走系统信任库。
    http: luna_model_fetch::pinned_tls::PinnedClient,
    /// 当前生效配置：由账户记住的服务器 + 该服务器的钉住公钥派生（[`Self::reconfigure`]）。
    cfg: RwLock<Arc<CloudConfig>>,
    /// 登录了（或上一次登录的）哪一台服务器（[`crate::account`]）。
    account: Arc<AccountStore>,
    /// 连接链接带来的一次性邀请码：只活在内存里，兑换成功即丢（不过 IPC、不落盘）。
    invite: Mutex<Option<String>>,
    /// 「邮箱 → 将登录到 X」之后定下来的这次登录（ADR-158 ①；只在内存里）。
    sign_in: Mutex<Option<SignInContext>>,
    /// 一次性码换来的注册授权（ADR-158 ⑥）：设完口令即丢；不过 IPC、不落盘。
    registration: Mutex<Option<RegistrationGrant>>,
    /// 测试把 `.well-known/lunarca` 指到本地假服务端（生产恒为邮箱域名上的 https）。
    #[cfg(test)]
    discovery_base: Mutex<Option<String>>,
    /// 测试注入的「官方后端」判定：`(origin, 是否正式构建)`。生产恒按 [`AccountServer::is_official`]
    /// 与 `cfg!(debug_assertions)` 判（[`Self::strict_official`]）。
    #[cfg(test)]
    official_policy: Mutex<Option<(String, bool)>>,
    /// URL scheme 送达、尚未处理的那条连接链接（原文只留在这里）。
    incoming_link: Mutex<Option<crate::connect_link::ConnectLink>>,
    store: Box<dyn CredentialStore>,
    pending: Mutex<Option<PendingLogin>>,
    /// 会话**被动失效**（refresh 被 401 拒、设备被撤销）时的回调。与 `logout` 那条主动路径
    /// 不同：主动登出由命令层通知观察者，被动失效发生在任意一次鉴权请求内部，没有这个钩子
    /// 界面上什么都不会发生——卡片继续写「AI 发往服务端」，而每次对话都在 401 上失败。
    session_lost: Mutex<Option<crate::SessionObserver>>,
    /// 严格本地硬闸。所有控制面 HTTP 都必须先过这一处，不能由各命令自行约定例外。
    probe: crate::StrictLocalProbe,
    /// refresh token 是轮换型一次性凭据；并发轮换会被服务端判为重用并吊销整个家族。
    refresh_gate: AsyncMutex<()>,
    /// 信任锚钉住状态（按服务端分桶，见 [`crate::trust`]）。
    ///
    /// **住在这里而不是另起一个托管状态**：钉住的判据是「这是哪一台服务端」，而
    /// 那个判据（`server_bucket(api_base)`）的事实源就是本结构里的 `cfg`。分开放的话，
    /// 换服务端时两者的切换时机会分叉，钉住就会被记到上一台的桶上。
    trust: Arc<TrustStore>,
    tls_roots: Arc<TlsRootStore>,
    /// 轮换验过之后把它落地的那只手（装配层注入，见 [`Self::set_rotation_applier`]）。
    /// 没注入 = 这台上没有能原子写设置+钉住的那一层（测试与纯库用法），轮换于是**不生效**，
    /// 行为退回阻断——安全默认在「不自动换钥」这一侧。
    rotation_applier: Mutex<Option<RotationApplier>>,
    /// 当前账户可用的 AI 能力（权益 `inference.capabilities`，统一登录 R4b）。`None` = 还不知道
    /// （没取过权益 / 刚换服务器 / 刚退出），此时按「服务器全都提供」路由——真被拒时服务端
    /// 回 403 `capability_not_allowed`，路由当场回落本机。只活在内存里：权益每次登录态变化
    /// 都会重取（装配层），落盘只会多一份会过时的副本。
    allowed_capabilities: Mutex<Option<Vec<String>>>,
    /// 同步的单文件上限（权益 `sync.max_file_bytes`，ADR-156 ④）。`None` = 还不知道或不限：
    /// 同步引擎此时不预检，超了的由服务端在提交时拒（`too_large`）。与能力清单同一生命周期。
    sync_max_file_bytes: Mutex<Option<u64>>,
    /// 某台服务器的部署事实（capabilities 的卖不卖、配没配上游、真有上游的能力，ADR-041 ①），
    /// 按服务器桶记。只活在内存里：登录、取权益、账户页取价目时各记一次；失败卡片据「卖不卖」
    /// 决定给「升级或购买」还是「打开账户设置」，路由据另两格决定服务端跑不跑得了——都不为它再发
    /// 一次请求。按桶记而不是随换服务器清空：桶名就是判据，记错台的可能性为零。
    deployment: Mutex<Option<(String, DeploymentFacts)>>,
    /// 这个账户的付款处境（权益 `billing_standing`，纵切 3 C3）。与能力清单同一生命周期：取到权益
    /// 时记、退出 / 换服务器时清。`None` = 还不知道。[`crate::commands::fetch_entitlements`] 据它
    /// 判「变没变」并通知装配层发通知；只是展示与通知的依据，**不**拿它预先拦请求——服务端才是
    /// 判官，运营解冻后不该等客户端重取权益才恢复。
    billing_standing: Mutex<Option<BillingStandingDto>>,
    /// 上一次取用量时认出的预警轮次（PRD-13-040 ③，纵切 3 C4）。与能力清单同一生命周期：退出 /
    /// 换服务器时清。[`crate::commands::fetch_usage`] 据它判「变没变」并通知装配层按落盘标记发通知。
    usage_alerts: Mutex<Option<Vec<crate::usage::UsageAlertEpisode>>>,
    /// 这台设备的指纹（纵切 3 C5，[`crate::fingerprint`]）：只随建设备会话的登录请求发给登录的那一台。
    fingerprint: FingerprintProvider,
    /// 登录撞上设备上限之后、用户选好移除哪一台之前挂着的状态（[`crate::device_limit`]）。只在内存里；
    /// 选完、取消、过期或开始另一次登录即清。
    device_limit: Mutex<Option<DeviceLimitPending>>,
}

/// 把一次验过的轮换落地：写地址两钥 + 重钉 + 三重重置，原子做完。
///
/// 返回 `Err` 时**不算换成**，调用方原样退回阻断——落地失败而界面说「已自动更新」，
/// 会让用户以为信任锚已经更新，而实际上验签仍在用旧钥。
pub type RotationApplier =
    Arc<dyn Fn(&OfferedAnchor) -> Result<(), String> + Send + Sync + 'static>;

impl CloudClient {
    pub fn new(store: Box<dyn CredentialStore>, probe: crate::StrictLocalProbe) -> Self {
        Self {
            http: luna_model_fetch::pinned_tls::PinnedClient::default(),
            cfg: RwLock::new(Arc::new(CloudConfig::derive(
                crate::config::DEFAULT_API_BASE,
                None,
            ))),
            account: Arc::new(AccountStore::new()),
            invite: Mutex::new(None),
            sign_in: Mutex::new(None),
            registration: Mutex::new(None),
            #[cfg(test)]
            discovery_base: Mutex::new(None),
            #[cfg(test)]
            official_policy: Mutex::new(None),
            incoming_link: Mutex::new(None),
            store,
            pending: Mutex::new(None),
            session_lost: Mutex::new(None),
            probe,
            refresh_gate: AsyncMutex::new(()),
            trust: Arc::new(TrustStore::new()),
            tls_roots: Arc::new(TlsRootStore::new()),
            rotation_applier: Mutex::new(None),
            allowed_capabilities: Mutex::new(None),
            sync_max_file_bytes: Mutex::new(None),
            deployment: Mutex::new(None),
            billing_standing: Mutex::new(None),
            usage_alerts: Mutex::new(None),
            fingerprint: FingerprintProvider::platform(),
            device_limit: Mutex::new(None),
        }
    }

    /// 设备指纹的提供者（插件 setup 经它注入退路 id 的落盘位置；测试经它换掉硬件标识来源）。
    #[must_use]
    pub fn fingerprint(&self) -> &FingerprintProvider {
        &self.fingerprint
    }

    /// 换一个硬件标识来源（生产恒为平台来源）。给测试与下游契约测试模拟「多台设备」用——同一台
    /// 机器上起的两个客户端指纹相同，服务端会正确地把它们认成同一台设备。
    #[must_use]
    pub fn with_fingerprint(mut self, provider: FingerprintProvider) -> Self {
        self.fingerprint = provider;
        self
    }

    /// 挂着的「已达到设备上限」那一步（界面渲染用，不含令牌）；没有为 `None`。零网络。
    #[must_use]
    pub fn device_limit(&self) -> Option<crate::device_limit::DeviceLimitDto> {
        self.device_limit
            .lock()
            .expect("device limit mutex poisoned")
            .as_ref()
            .map(|pending| pending.limit.clone())
    }

    /// 丢掉挂着的设备上限选择（取消、开始另一次登录、登录成功、令牌失效）。
    fn clear_device_limit(&self) {
        *self
            .device_limit
            .lock()
            .expect("device limit mutex poisoned") = None;
    }

    fn hold_device_limit(&self, pending: DeviceLimitPending) {
        *self
            .device_limit
            .lock()
            .expect("device limit mutex poisoned") = Some(pending);
    }

    /// 上一次取用量时认出的预警轮次（`None` = 还没取过）。零网络。
    #[must_use]
    pub fn usage_alerts(&self) -> Option<Vec<crate::usage::UsageAlertEpisode>> {
        self.usage_alerts
            .lock()
            .expect("usage alerts mutex poisoned")
            .clone()
    }

    fn set_usage_alerts(&self, value: Option<Vec<crate::usage::UsageAlertEpisode>>) {
        *self
            .usage_alerts
            .lock()
            .expect("usage alerts mutex poisoned") = value;
    }

    /// 上一次取到的付款处境（`None` = 还不知道）。零网络。
    #[must_use]
    pub fn billing_standing(&self) -> Option<BillingStandingDto> {
        self.billing_standing
            .lock()
            .expect("billing standing mutex poisoned")
            .clone()
    }

    fn set_billing_standing(&self, value: Option<BillingStandingDto>) {
        *self
            .billing_standing
            .lock()
            .expect("billing standing mutex poisoned") = value;
    }

    /// 同步的单文件上限（字节；`None` = 还不知道或不限）。零网络。
    #[must_use]
    pub fn sync_max_file_bytes(&self) -> Option<u64> {
        *self
            .sync_max_file_bytes
            .lock()
            .expect("sync max file mutex poisoned")
    }

    /// 当前账户可用的 AI 能力（`None` = 还不知道，按全部提供处理）。零网络。
    #[must_use]
    pub fn allowed_capabilities(&self) -> Option<Vec<String>> {
        self.allowed_capabilities
            .lock()
            .expect("allowed capabilities mutex poisoned")
            .clone()
    }

    /// 服务器提不提供某项能力（按上一次取到的权益；不知道 = 提供）。
    #[must_use]
    pub fn capability_allowed(&self, capability: &str) -> bool {
        self.allowed_capabilities
            .lock()
            .expect("allowed capabilities mutex poisoned")
            .as_ref()
            .is_none_or(|list| list.iter().any(|c| c == capability))
    }

    /// 记下 / 忘掉这个账户的权益派生值（能力清单与同步单文件上限同进同退：退出、换服务器时
    /// 一起清，取到权益时一起记）。
    fn set_allowed_capabilities(&self, value: Option<Vec<String>>) {
        *self
            .allowed_capabilities
            .lock()
            .expect("allowed capabilities mutex poisoned") = value;
        if self.allowed_capabilities().is_none() {
            self.set_sync_max_file_bytes(None);
            self.set_billing_standing(None);
            self.set_usage_alerts(None);
        }
    }

    fn set_sync_max_file_bytes(&self, value: Option<u64>) {
        *self
            .sync_max_file_bytes
            .lock()
            .expect("sync max file mutex poisoned") = value;
    }

    /// 信任锚钉住状态（装配层注入持久化路径、命令层读状态）。
    #[must_use]
    pub fn trust(&self) -> Arc<TrustStore> {
        Arc::clone(&self.trust)
    }

    /// 账户记住的服务器（装配层注入持久化路径）。
    #[must_use]
    pub fn account(&self) -> Arc<AccountStore> {
        Arc::clone(&self.account)
    }

    /// 自签部署的根证书钉住（ADR-153；装配层注入持久化路径，连接链接与配置文件导入写入）。
    #[must_use]
    pub fn tls_roots(&self) -> Arc<TlsRootStore> {
        Arc::clone(&self.tls_roots)
    }

    /// 发往当前服务端的 HTTP 客户端。
    fn http(&self) -> reqwest::Client {
        self.http.for_url(&self.config().api_base)
    }

    /// 注入「轮换验过之后由谁把它落地」（装配层调用一次，`reconfigure` 不清它）。
    ///
    /// 写设置 + 钉住 + 三重重置只有装配层做得到（`luna-cloud` 刻意不知道设置存在哪），
    /// 而**验链必须在这一层**——它是密码学判断，不能散到界面或命令层去。同 `StrictLocalProbe`
    /// 的分工：判据在这里，能力由宿主注入。
    pub fn set_rotation_applier(&self, applier: RotationApplier) {
        *self
            .rotation_applier
            .lock()
            .expect("rotation applier mutex poisoned") = Some(applier);
    }

    /// 当前服务端桶名 —— 钉住 / 凭据 / 目录水位共用的那一个判据。
    #[must_use]
    pub fn server_bucket(&self) -> String {
        crate::config::server_bucket(&self.config().api_base)
    }

    /// 账户页「服务器身份」那一块的状态（零网络）。
    ///
    /// **展示「此刻要你处理的那一台」，而不是死盯当前那一桶**：一次导入的冲突记在文件里
    /// 那台的桶上（可能不是当前那台），而确认按钮长在这份状态上。`server` 字段说明是哪一台。
    /// 内置官方后端不走钉住（登录对它跳过核对），未钉住时报 `Official` 而不是「还没核验」。
    #[must_use]
    pub fn trust_status(&self) -> crate::trust::TrustStatusDto {
        let current = self.server_bucket();
        if let Some(pending) = self.trust.pending_bucket(&current) {
            return self.trust.status(&pending);
        }
        let mut status = self.trust.status(&current);
        if status.state == crate::trust::TrustStateDto::Unpinned
            && self.account.effective_server().is_official()
        {
            status.state = crate::trust::TrustStateDto::Official;
        }
        status
    }

    /// 所有出站请求共用的字节前硬闸。
    fn ensure_outbound_allowed(&self) -> Result<(), CloudError> {
        if (self.probe)() {
            Err(CloudError::StrictLocalBlocked)
        } else {
            Ok(())
        }
    }

    /// 当前配置快照。
    #[must_use]
    pub fn config(&self) -> Arc<CloudConfig> {
        Arc::clone(&self.cfg.read().expect("cloud cfg rwlock poisoned"))
    }

    /// 按账户记住的服务器与它的钉住公钥重新派生配置（换服务器 / 钉住变了之后调用）。
    ///
    /// **地址与公钥一次换齐**：只换地址的话，新服务端签发的租约与目录会拿旧服务端的公钥
    /// 去验，结果必然是「未验签」与「目录刷新被拒」。进行中的设备码配对一并作废——那串
    /// 配对码是旧服务端发的。
    pub fn reconfigure(&self) {
        let server = self.account.effective_server();
        let bucket = crate::config::server_bucket(&server.origin);
        let pinned = self.trust.pinned(&bucket);
        let cfg = CloudConfig::derive(
            &server.origin,
            pinned
                .as_ref()
                .map(|p| (p.lease_pubkey.as_str(), p.catalog_pubkey.as_str())),
        );
        let changed_server = self.config().api_base != cfg.api_base;
        *self.cfg.write().expect("cloud cfg rwlock poisoned") = Arc::new(cfg);
        if changed_server {
            self.login_cancel();
            // 上一台服务器的能力清单不适用于这一台。
            self.set_allowed_capabilities(None);
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config().api_base)
    }

    // ---- 凭据的服务端命名空间 ----

    /// 钥匙链条目前缀 —— **按服务端分桶**。
    ///
    /// 此前三个条目（access / refresh / profile）是全局唯一的。于是把地址从服务端 A
    /// 改到 B 之后，账户状态（零网络，只读钥匙链）照旧报「已登录」并显示 A 的邮箱，
    /// 直到第一次真实请求撞上 401 才把人踹出去。用户看到的是「明明已登录，改个地址就
    /// 被登出了，而且刚才那个邮箱是哪来的」。
    ///
    /// 分桶之后，切回 A 仍是登录态——这正是分桶的价值，而不只是修一个错误显示。
    fn scoped(&self, key: &str) -> String {
        // 桶名判据只有一份（`config::server_bucket`）——与目录客户端共用，各写一遍必然分叉。
        format!(
            "{}/{key}",
            crate::config::server_bucket(&self.config().api_base)
        )
    }

    fn cred_get(&self, key: &str) -> Result<Option<String>, CloudError> {
        self.store.get(&self.scoped(key))
    }

    fn cred_set(&self, key: &str, value: &str) -> Result<(), CloudError> {
        self.store.set(&self.scoped(key), value)
    }

    /// 试写一条再删掉：回答「拿到会话后存得下吗」，不碰已有条目。
    fn ensure_credentials_writable(&self) -> Result<(), CloudError> {
        let probe = self.scoped(KEY_WRITE_PROBE);
        self.store.set(&probe, "1")?;
        self.store.delete(&probe)
    }

    /// 清空**当前服务端**的会话条目（别的服务端的凭据不动）。
    fn cred_clear_session(&self) -> Result<(), CloudError> {
        self.set_allowed_capabilities(None);
        self.cred_clear_bucket(&self.server_bucket())
    }

    /// 清空**指定服务端**的会话条目（重钉一台未必是当前那台的服务端时用）。
    fn cred_clear_bucket(&self, bucket: &str) -> Result<(), CloudError> {
        for key in [KEY_ACCESS, KEY_REFRESH, KEY_PROFILE] {
            self.store.delete(&format!("{bucket}/{key}"))?;
        }
        Ok(())
    }

    /// 登记会话被动失效的观察者（装配层用它重装云栈并通知用户重新登录）。
    pub fn on_session_lost(&self, observer: crate::SessionObserver) {
        *self
            .session_lost
            .lock()
            .expect("session_lost mutex poisoned") = Some(observer);
    }

    /// 会话被动失效的唯一出口：清凭据 + 通知观察者 + `NeedsRelogin`。
    /// 两处 401 收敛点都走这里，免得一处记得通知、另一处忘了。
    fn session_lost(&self) -> CloudError {
        if let Err(err) = self.cred_clear_session() {
            return err;
        }
        let observer = self
            .session_lost
            .lock()
            .expect("session_lost mutex poisoned")
            .clone();
        if let Some(observer) = observer {
            observer();
        }
        CloudError::NeedsRelogin
    }

    // ---- device-code 登录 ----

    /// 发起配对：POST /v1/device/code。device_code 只存进程内，不出 Rust。
    pub async fn login_start(
        &self,
        device_name: String,
        platform: String,
    ) -> Result<LoginStartDto, CloudError> {
        self.ensure_outbound_allowed()?;
        self.clear_device_limit();
        let fingerprint = self.fingerprint.get();
        let response = self
            .http()
            .post(self.url("/v1/device/code"))
            .json(&serde_json::json!({
                "device_name": device_name,
                "platform": platform,
                "device_fingerprint": fingerprint.as_str(),
            }))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
        let response = check_status(response).await?;
        let body: DeviceCodeResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;

        let interval = Duration::from_secs(body.interval.max(0) as u64);
        let expires_in = body.expires_in.max(0) as u64;
        let verification_uri_complete = format!(
            "{}?user_code={}",
            body.verification_uri,
            // user_code 是服务端生成的白名单字符集（字母数字 + '-'），无需完整 urlencode。
            body.user_code
        );
        *self.pending.lock().expect("pending mutex poisoned") = Some(PendingLogin {
            device_code: body.device_code,
            user_code: body.user_code.clone(),
            verification_uri_complete: verification_uri_complete.clone(),
            interval,
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });

        Ok(LoginStartDto {
            user_code: body.user_code,
            verification_uri: body.verification_uri,
            verification_uri_complete,
            next_poll_ms: poll_ms(interval),
        })
    }

    /// 把进行中的配对改由某个身份源完成（「用 Google 继续」、「用 <组织> 登录」，ADR-171 ④）：浏览器直接
    /// 打开 `/v1/device/oidc/start?provider=<id>&user_code=<码>`，跳过批准页上的选择；回来后仍是同一个
    /// 设备码轮询。返回要打开的地址，并记为这次配对的地址（「重新打开浏览器」打开的也是它）。
    ///
    /// `provider_id` 来自 capabilities，但仍按 UUID 校验——它要拼进地址。
    pub fn route_pending_to_provider(&self, provider_id: &str) -> Result<String, CloudError> {
        let provider_id =
            uuid::Uuid::parse_str(provider_id).map_err(|_| CloudError::InvalidEndpoint {
                detail: "provider id is not a uuid".to_owned(),
            })?;
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        let pending = pending.as_mut().ok_or(CloudError::NoPendingLogin)?;
        let url = reqwest::Url::parse_with_params(
            &self.url("/v1/device/oidc/start"),
            &[
                ("provider", provider_id.to_string()),
                ("user_code", pending.user_code.clone()),
            ],
        )
        .map_err(|err| CloudError::InvalidEndpoint {
            detail: err.to_string(),
        })?;
        pending.verification_uri_complete = url.to_string();
        Ok(pending.verification_uri_complete.clone())
    }

    /// 进行中配对的批准页地址（「重新打开浏览器」入口）。
    pub fn pending_verification_uri(&self) -> Result<String, CloudError> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .as_ref()
            .map(|p| p.verification_uri_complete.clone())
            .ok_or(CloudError::NoPendingLogin)
    }

    /// 进行中配对的 `(配对码, 批准页地址)`；没有配对为 `None`。
    #[must_use]
    pub fn pending_login(&self) -> Option<(String, String)> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .as_ref()
            .map(|p| (p.user_code.clone(), p.verification_uri_complete.clone()))
    }

    /// 进行中配对现行的轮询间隔（毫秒，含 `slow_down` 放宽过的）；没有配对为 `None`。
    #[must_use]
    pub fn pending_poll_ms(&self) -> Option<u32> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .as_ref()
            .map(|p| poll_ms(p.interval))
    }

    /// 取消进行中的配对（本地丢弃 device_code；服务端 grant 自然过期）。
    pub fn login_cancel(&self) {
        *self.pending.lock().expect("pending mutex poisoned") = None;
    }

    /// 单步轮询：POST /v1/device/token。
    pub async fn login_poll(&self) -> Result<LoginPollDto, CloudError> {
        self.ensure_outbound_allowed()?;
        let (device_code, interval, expired) = {
            let pending = self.pending.lock().expect("pending mutex poisoned");
            let Some(p) = pending.as_ref() else {
                return Err(CloudError::NoPendingLogin);
            };
            (
                p.device_code.clone(),
                p.interval,
                Instant::now() >= p.expires_at,
            )
        };
        if expired {
            self.login_cancel();
            return Ok(LoginPollDto {
                status: LoginPollStatus::Expired,
                next_poll_ms: None,
                email: None,
            });
        }

        // 指纹只随发起帧登记（服务端存在配对上），轮询帧只带 device_code（协议 v13）。
        let response = self
            .http()
            .post(self.url("/v1/device/token"))
            .json(&serde_json::json!({
                "device_code": device_code,
            }))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;

        if response.status().is_success() {
            let tokens: TokenResponse = response
                .json()
                .await
                .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
            self.login_cancel();
            self.clear_device_limit();
            let profile = self.activate_session(tokens).await?;
            return Ok(LoginPollDto {
                status: LoginPollStatus::Authorized,
                next_poll_ms: None,
                email: Some(profile.email),
            });
        }

        let status_code = response.status();
        let status = status_code.as_u16();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        // 浏览器那边批准了、设备却满了（纵切 3 C5）：配对留着，停轮询，等用户选好移除哪一台。
        if let Some(wire) = crate::device_limit::decode(status_code, &body) {
            self.hold_device_limit(DeviceLimitPending::new(wire, SignInRetry::DeviceCode));
            return Ok(LoginPollDto {
                status: LoginPollStatus::DeviceLimitReached,
                next_poll_ms: None,
                email: None,
            });
        }
        let oauth: OAuthErrorResponse =
            serde_json::from_value(body).map_err(|err| CloudError::UnreadableResponse {
                host: crate::config::server_bucket(&self.config().api_base),
                detail: err.to_string(),
            })?;
        match oauth.error.as_str() {
            "authorization_pending" => Ok(LoginPollDto {
                status: LoginPollStatus::Pending,
                next_poll_ms: Some(poll_ms(interval)),
                email: None,
            }),
            "slow_down" => {
                // RFC 8628 §3.5：间隔 +5s 并持久到 pending，之后每次轮询都用新间隔。
                let widened = interval + Duration::from_secs(SLOW_DOWN_STEP_SECS);
                if let Some(p) = self
                    .pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .as_mut()
                {
                    p.interval = widened;
                }
                Ok(LoginPollDto {
                    status: LoginPollStatus::SlowDown,
                    next_poll_ms: Some(poll_ms(widened)),
                    email: None,
                })
            }
            "expired_token" | "invalid_grant" => {
                self.login_cancel();
                Ok(LoginPollDto {
                    status: LoginPollStatus::Expired,
                    next_poll_ms: None,
                    email: None,
                })
            }
            other => Err(CloudError::Server {
                status,
                error_code: other.to_owned(),
            }),
        }
    }

    async fn fetch_and_cache_profile(&self) -> Result<ProfileDto, CloudError> {
        let response = self.authed(reqwest::Method::GET, "/v1/me").await?;
        let me: MeResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        let profile = ProfileDto {
            account_id: me.account_id,
            email: me.email,
        };
        let json = serde_json::to_string(&profile)
            .map_err(|_| CloudError::internal("profile serialize"))?;
        self.cred_set(KEY_PROFILE, &json)?;
        Ok(profile)
    }

    /// 把令牌对与账户档案作为一个会话整体激活。
    ///
    /// Keychain 没有跨条目事务；任何一步失败都清掉三项，绝不暴露“有 refresh、没档案”或
    /// “只有 access”这种半登录态。服务端授权码已消费时，唯一诚实的恢复动作是重新登录。
    async fn activate_session(&self, tokens: TokenResponse) -> Result<ProfileDto, CloudError> {
        let result = async {
            self.store_token_pair(&tokens)?;
            self.fetch_and_cache_profile().await
        }
        .await;
        if result.is_err() {
            let _ = self.cred_clear_session();
        }
        result
    }

    /// 同批写入一次轮换返回的 access/refresh；部分写入时回滚为无会话。
    pub(crate) fn store_token_pair(&self, tokens: &TokenResponse) -> Result<(), CloudError> {
        let result = (|| {
            self.cred_set(KEY_ACCESS, &tokens.access_token)?;
            self.cred_set(KEY_REFRESH, &tokens.refresh_token)
        })();
        if result.is_err() {
            let _ = self.cred_clear_session();
        }
        result
    }

    // ---- 会话 ----

    /// 只回答「登录了没有」——**只碰 refresh 这一条**。
    ///
    /// 与读档案（[`Self::cached_profile`]）分开是最小读取：「此刻连着服务端吗」这个判断在装配时
    /// 问一次、后台目录同步每 15 分钟问一次，而它**从来不看档案**。凭据现在逐条加密在本机保险箱里
    /// （TDD-12 §2），读哪一条只解哪一条；钥匙串只在本进程第一次取主密钥时碰一次。
    ///
    /// 读不到一律当成未登录：这与既有调用点的 `.ok()` 同一口径，且安全默认在这一侧
    /// （不确定时不发网络请求）。
    #[must_use]
    pub fn logged_in(&self) -> bool {
        self.cred_get(KEY_REFRESH).ok().flatten().is_some()
    }

    /// 缓存的账户档案（零网络）；读不出来为 `None`。
    ///
    /// **必须走 `cred_get`**（按服务端分桶）：档案写进 `<桶>/profile`，读裸 `profile` 恒为
    /// `None`——界面上就是「已登录、而邮箱永远是一个破折号」（真机 2026-08-08 坐实）。
    #[must_use]
    pub fn cached_profile(&self) -> Option<ProfileDto> {
        self.cred_get(KEY_PROFILE)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    // ---- 应用内登录（ADR-158：OPAQUE，口令不离开设备）----

    /// 登录最后一步（领取会话 / 注册口令）共用的发送与结果映射；撞上设备上限的重发也走这里。
    ///
    /// 成功 → `Some(响应体)`（调用方按路径解读）；撞上设备上限 → 这一条请求原样挂进
    /// [`crate::device_limit`]，返回 `None`。请求体含一次性的领取凭据或注册授权：只进钉住的 TLS，
    /// 不进日志。
    async fn send_grant(
        &self,
        path: &'static str,
        body: serde_json::Value,
        bearer: Option<zeroize::Zeroizing<String>>,
    ) -> Result<Option<serde_json::Value>, CloudError> {
        let mut request = self.http().post(self.url(path)).json(&body);
        if let Some(token) = &bearer {
            request = request.bearer_auth(token.as_str());
        }
        let response = request
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .map(Some)
                .map_err(|err| CloudError::body(&err, &self.config().api_base));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(check_status(response)
                .await
                .err()
                .unwrap_or(CloudError::RateLimited {
                    retry_after_secs: None,
                }));
        }
        let answer: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
        // 撞上设备上限（纵切 3 C5）：这一条请求原样留着，等用户选好移除哪一台再发一次。
        if let Some(wire) = crate::device_limit::decode(status, &answer) {
            self.hold_device_limit(DeviceLimitPending::new(
                wire,
                SignInRetry::Grant { path, body, bearer },
            ));
            return Ok(None);
        }
        // 领取资格 / 注册授权过了时效（领取 120 秒；码换来的授权按 `expires_in`）：从登录重来一次。
        if status == reqwest::StatusCode::GONE
            || (status == reqwest::StatusCode::UNAUTHORIZED && bearer.is_some())
        {
            return Err(CloudError::LoginExpired);
        }
        let code = answer
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match code.as_str() {
            EMAIL_DOMAIN_NOT_ALLOWED if status == reqwest::StatusCode::BAD_REQUEST => {
                Err(CloudError::EmailDomainNotAllowed)
            }
            other => match LoginRejection::from_code(other) {
                Some(reason) => Err(CloudError::LoginRejected { reason }),
                None => Err(CloudError::Server {
                    status: status.as_u16(),
                    error_code: if other.is_empty() {
                        "unknown".to_owned()
                    } else {
                        other.to_owned()
                    },
                }),
            },
        }
    }

    /// 领到的令牌对 → 激活会话（应用内登录完成的唯一出口）。
    async fn finish_with_tokens(&self, answer: serde_json::Value) -> Result<(), CloudError> {
        let tokens: TokenResponse =
            serde_json::from_value(answer).map_err(|err| CloudError::UnreadableResponse {
                host: self.server_bucket(),
                detail: err.to_string(),
            })?;
        self.login_cancel();
        self.activate_session(tokens).await?;
        self.forget_sign_in();
        Ok(())
    }

    /// 在钉住的通道上注册口令（`register/start` → `register/finish`，ADR-158 ⑤⑥）。`bearer` 是
    /// 一次性码换来的注册授权，或登录状态下的 access token。
    ///
    /// 结果同 [`Self::send_grant`]。
    async fn register_password(
        &self,
        bearer: zeroize::Zeroizing<String>,
        email: &str,
        deployment_id: &str,
        password: &str,
        device_name: &str,
        platform: &str,
    ) -> Result<Option<serde_json::Value>, CloudError> {
        use opaque_ke::{
            ClientRegistration, ClientRegistrationFinishParameters, RegistrationResponse,
        };

        let mut rng = aes_gcm::aead::OsRng;
        let start = ClientRegistration::<Suite>::start(&mut rng, password.as_bytes())
            .map_err(|_| CloudError::internal("opaque registration start"))?;
        let response = self
            .http()
            .post(self.url(REGISTER_START_PATH))
            .bearer_auth(bearer.as_str())
            .json(&serde_json::json!({
                "registration_request": opaque_auth::b64u(&start.message.serialize()),
            }))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(CloudError::LoginExpired);
        }
        let started: RegisterStartResponse = check_status(response)
            .await?
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        let registration_response = opaque_auth::from_b64u(&started.registration_response)
            .and_then(|bytes| RegistrationResponse::<Suite>::deserialize(&bytes).ok())
            .ok_or_else(|| CloudError::UnreadableResponse {
                host: self.server_bucket(),
                detail: "registration_response is not a valid opaque message".to_owned(),
            })?;
        let ksf = opaque_auth::ksf();
        let finished = start
            .state
            .finish(
                &mut rng,
                password.as_bytes(),
                registration_response,
                ClientRegistrationFinishParameters::new(
                    opaque_auth::identifiers(email, deployment_id),
                    Some(&ksf),
                ),
            )
            .map_err(|_| CloudError::internal("opaque registration finish"))?;
        self.send_grant(
            REGISTER_FINISH_PATH,
            serde_json::json!({
                "registration_id": started.registration_id,
                "registration_upload": opaque_auth::b64u(&finished.message.serialize()),
                "device_name": device_name,
                "platform": platform,
                "device_fingerprint": self.fingerprint.get().as_str(),
            }),
            Some(bearer),
        )
        .await
    }

    /// 登录状态下改口令（ADR-158 ⑤，TDD-16 §5.2a）：在当前会话上重新注册 OPAQUE 记录。服务端保留
    /// 这台设备的会话、吊销其余设备的 refresh token；回来的令牌对写回钥匙链。
    ///
    /// # Errors
    /// 未登录 → [`CloudError::NotLoggedIn`]；新口令不合规 → `LoginRejected { password_invalid }`；
    /// 会话失效 → [`CloudError::LoginExpired`]（重新登录）。
    pub async fn change_password(
        &self,
        new_password: &str,
        device_name: &str,
        platform: &str,
    ) -> Result<(), CloudError> {
        self.ensure_outbound_allowed()?;
        self.ensure_access_token().await?;
        let access = self.access_token().ok_or(CloudError::NotLoggedIn)?;
        let email = self
            .cached_profile()
            .map(|profile| opaque_auth::normalize_email(&profile.email))
            .ok_or(CloudError::NotLoggedIn)?;
        // 部署 id 经钉住的通道现取（与登录时 `server_identity` 同一个值）。
        let deployment_id = self
            .fetch_capabilities_at(&self.config().api_base)
            .await?
            .deployment
            .id;
        let answer = self
            .register_password(
                zeroize::Zeroizing::new(access),
                &email,
                &deployment_id,
                new_password,
                device_name,
                platform,
            )
            .await?
            .ok_or_else(|| CloudError::internal("device limit while changing password"))?;
        let tokens: TokenResponse =
            serde_json::from_value(answer).map_err(|err| CloudError::UnreadableResponse {
                host: self.server_bucket(),
                detail: err.to_string(),
            })?;
        self.store_token_pair(&tokens)
    }

    /// refresh 轮换：旧 refresh 换新 access + 新 refresh（两条都写回钥匙链）。
    /// 401（家族吊销/过期/设备撤销）→ 清空会话 + `NeedsRelogin`。
    ///
    /// 调用方已持有 [`Self::refresh_gate`] 时执行一次**真正的**轮换。
    ///
    /// **本方法没有 pub 的无参兄弟，这是刻意的。** 曾经有过一个
    /// `pub async fn refresh_session(&self)`——只加锁、不做二次检查——于是仓库里存在
    /// 两条续期入口：一条（`refresh_after_rejection`）会问「等锁期间别人是不是已经换好了」，
    /// 另一条不问。Provider 侧当时接的正是不问的那条，代价是 N 条 Provider 同时 401
    /// 会串行轮换 N 次（TDD-14 §3.3.1 ⑩ 记录的偏差）。留着那个无参入口，
    /// 下一个人还会接错——**能进这里的路只有一条，就是二次检查的下游**。
    async fn refresh_session_locked(&self) -> Result<(), CloudError> {
        self.ensure_outbound_allowed()?;
        let Some(refresh) = self.cred_get(KEY_REFRESH)? else {
            return Err(CloudError::NotLoggedIn);
        };
        let response = self
            .http()
            .post(self.url("/v1/auth/refresh"))
            .json(&serde_json::json!({ "refresh_token": refresh }))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(self.session_lost());
        }
        let response = check_status(response).await?;
        let tokens: TokenResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        self.store_token_pair(&tokens)
    }

    /// 某个 access 收到 401 后刷新；若等锁期间别的请求已经换好了 access，就直接复用。
    async fn refresh_after_rejection(&self, rejected_access: &str) -> Result<(), CloudError> {
        let _guard = self.refresh_gate.lock().await;
        if self.cred_get(KEY_ACCESS)?.as_deref() != Some(rejected_access) {
            return Ok(());
        }
        self.refresh_session_locked().await
    }

    /// 仅存 refresh、没有 access 的降级态也只允许一个请求补齐会话。
    async fn ensure_access_token(&self) -> Result<(), CloudError> {
        if self.cred_get(KEY_ACCESS)?.is_some() {
            return Ok(());
        }
        let _guard = self.refresh_gate.lock().await;
        if self.cred_get(KEY_ACCESS)?.is_some() {
            return Ok(());
        }
        self.refresh_session_locked().await
    }

    /// 鉴权请求（401 → 静默 refresh 一次 → 重试；仍 401 → 清会话 + NeedsRelogin）。
    /// 当前可用的 access token（**零网络**：只读本地凭据）。
    ///
    /// `None` = 未登录。与 `authed` 的前半段同规则：无 refresh token 即视为未登录，
    /// 一个字节都不发（ADR-037 不变量）。供 Provider 侧的 `BearerSource` 适配。
    pub fn access_token(&self) -> Option<String> {
        // 先看 refresh：没有它就是未登录，此时**不返回 access**——哪怕本地还残留
        // 一个过期的 access token。这与 `authed` 的前半段是同一条判定。
        self.cred_get(KEY_REFRESH).ok().flatten()?;
        self.cred_get(KEY_ACCESS).ok().flatten()
    }

    /// **被 `rejected_access` 那枚 token 换来的**新 access（收到 401 之后调用）。
    ///
    /// Provider 侧（`BearerSource::refresh`）唯一的续期入口，与控制面的 `authed` 共用
    /// [`Self::refresh_after_rejection`] 那一份二次检查——**二次检查只有一份**。
    ///
    /// **必须传那枚真正被拒的 token，而不是"当前的" token。** 二次检查靠
    /// 「钥匙链里的 access 还是不是你手上这枚」来分辨两件事：还是 = 确实过期了，
    /// 该我去轮换；已经不是 = 等锁那几百毫秒里别人换好了，直接复用。传错（比如重新读一次
    /// 钥匙链再传进来）会让它恒判「别人换好了」而**永不轮换**，401 就此死循环。
    ///
    /// 失败即视为需要重新登录——轮换内部对 401 已做清会话 + `NeedsRelogin`。
    pub async fn refreshed_access_token(&self, rejected_access: &str) -> Option<String> {
        self.refresh_after_rejection(rejected_access).await.ok()?;
        // 走 `access_token()` 而不是裸读 `KEY_ACCESS`：二次检查的提前返回那一支没有
        // 碰过网络，此刻可能处在「有残留 access、无 refresh」的未登录态（例如 Bearer 其实是
        // 一枚组织 API Key）。裸读会把一枚过期 access 当成有效会话交出去。
        self.access_token()
    }

    /// 鉴权请求的发前判定（零网络）：严格本地 → [`CloudError::StrictLocalBlocked`]；本机没有会话 →
    /// [`CloudError::NotLoggedIn`]。[`Self::authed_with`] 每次都先过它；要在发请求**之前**先做一件
    /// 用户可见的事（导出前弹保存对话框）的命令也先过它——不让用户选完位置才被告知「没登录」。
    ///
    /// # Errors
    /// 见上；钥匙链读失败照原样上抛。
    pub(crate) fn require_session(&self) -> Result<(), CloudError> {
        self.ensure_outbound_allowed()?;
        if self.cred_get(KEY_REFRESH)?.is_none() {
            return Err(CloudError::NotLoggedIn);
        }
        Ok(())
    }

    async fn authed(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::Response, CloudError> {
        self.authed_with(method, path, |request| request).await
    }

    /// 带请求体 / 请求头的鉴权请求（同 [`Self::authed`] 的 401 处置）。`build` 每次尝试都会
    /// 被调用一次——401 之后重发要带同样的请求体，所以它必须能重建（别在里面消耗一次性数据）。
    pub(crate) async fn authed_with(
        &self,
        method: reqwest::Method,
        path: &str,
        build: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, CloudError> {
        self.require_session()?;
        // 无 access（如仅存 refresh 的降级态）→ 先补一次 refresh。
        self.ensure_access_token().await?;

        for attempt in 0..2u8 {
            let access = self.cred_get(KEY_ACCESS)?.ok_or(CloudError::NotLoggedIn)?;
            let response = build(
                self.http()
                    .request(method.clone(), self.url(path))
                    .bearer_auth(&access),
            )
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
            if response.status() != reqwest::StatusCode::UNAUTHORIZED {
                return check_status(response).await;
            }
            if attempt == 0 {
                // 第一次 401：静默 refresh（其内部对 401 已做清会话 + NeedsRelogin）。
                self.refresh_after_rejection(&access).await?;
            } else {
                // refresh 成功但新 access 仍被拒（如设备刚被撤销的竞态）：同样收敛到重登。
                return Err(self.session_lost());
            }
        }
        unreachable!("authed loop returns within two attempts");
    }

    // ---- 设备管理 / 权益 / 注销 ----

    pub async fn devices(&self) -> Result<Vec<DeviceDto>, CloudError> {
        let response = self.authed(reqwest::Method::GET, "/v1/devices").await?;
        response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))
    }

    pub async fn revoke_device(&self, device_id: &str) -> Result<(), CloudError> {
        self.authed(
            reqwest::Method::POST,
            &format!("/v1/devices/{device_id}/revoke"),
        )
        .await?;
        Ok(())
    }

    /// 匿名取指定服务器的 capabilities 并协商协议（登录前要知道它支持哪几种登录方式）。
    ///
    /// 只在用户显式开始登录之后调用（ADR-037 例外清单的「登录」那一条）。
    ///
    /// # Errors
    /// 连不上 / 非 2xx 照常；协议不兼容 → [`CloudError::ClientTooOld`] /
    /// [`CloudError::ServerTooOld`]（不进入登录，告诉用户该升级哪一侧）。
    pub async fn fetch_capabilities_at(
        &self,
        api_base: &str,
    ) -> Result<protocol::CapabilitiesDto, CloudError> {
        self.ensure_outbound_allowed()?;
        let base = api_base.trim_end_matches('/');
        let response = self
            .http
            .for_url(base)
            .get(format!("{base}/inference/v1/capabilities"))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, base))?;
        let response = check_status(response).await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|err| CloudError::body(&err, base))?;
        let unreadable = |err: serde_json::Error| CloudError::UnreadableResponse {
            host: crate::config::server_bucket(base),
            // serde_json 的错误串只含位置与期望的类型，不含响应体本身。
            detail: err.to_string(),
        };
        // 先只读两侧版本、协商，再按本客户端的形状解析整份（见 `ProtocolVersionsDto`）。
        let versions: protocol::ProtocolVersionsDto =
            serde_json::from_slice(&bytes).map_err(unreadable)?;
        match protocol::negotiate(
            versions.protocol,
            versions.min_client_protocol,
            protocol::CLIENT_PROTOCOL,
        ) {
            protocol::Compatibility::Ok => serde_json::from_slice(&bytes).map_err(unreadable),
            protocol::Compatibility::ClientTooOld => Err(CloudError::ClientTooOld {
                min_client_protocol: versions.min_client_protocol,
            }),
            protocol::Compatibility::ServerTooOld => Err(CloudError::ServerTooOld {
                server_protocol: versions.protocol,
            }),
        }
    }

    /// 取端点自报的两把签名公钥并与钉住值比对。
    ///
    /// # 为什么让服务端自报公钥不是安全剧场
    ///
    /// 单看首连这一次，它确实挡不住中间人——与 SSH 首连相同。它的价值全部在**之后**：
    /// 钉住之后任何变更都会被这条比对发现并阻断（ADR-060 ②）。真正锚住防回滚水位的
    /// 是这两把钥本身：MITM 篡改目录会在验签这一步被发现，而他拿不到私钥就重签不出
    /// 更大的版本。有 MDM 的组织走主路径（组织配置文件），根本不经过首连窗口。
    ///
    /// # Errors
    /// 与钉住不一致 → [`CloudError::TrustAnchorChanged`]（阻断态）。其余失败按原因分类
    /// 上抛，并同时记进钉住状态供界面如实展示。
    pub async fn check_trust_anchor(&self) -> Result<TrustVerdict, CloudError> {
        let bucket = self.server_bucket();
        let (offered, rotations) = match self.fetch_trust_keys().await {
            Ok(payload) => payload,
            Err(err) => {
                self.trust
                    .record_error(&bucket, Some(trust_error_code(&err)));
                return Err(err);
            }
        };
        self.trust.record_error(&bucket, None);
        let verdict = self.trust.record_offer(&bucket, offered.clone());
        if verdict == TrustVerdict::Changed {
            // 阻断之前先给轮换链一次机会（ADR-124 ③）。验得通才自动重钉；**任何一步不成立
            // 都原样落回阻断**——轮换只能让一次本来就要打断用户的事变得不打断，永远不能
            // 让一次本来会被拦下的变更通过。
            if self.rotation_settles(&bucket, &offered, &rotations) {
                return Ok(TrustVerdict::Match);
            }
            return Err(CloudError::TrustAnchorChanged);
        }
        Ok(verdict)
    }

    /// 沿链验证并落地；**落不了地就当没验过**。
    fn rotation_settles(
        &self,
        bucket: &str,
        offered: &OfferedAnchor,
        rotations: &[RotationStatement],
    ) -> bool {
        let Some(pinned) = self.trust.pinned(bucket) else {
            return false;
        };
        if !crate::trust::follow_rotation_chain(&pinned, offered, rotations) {
            return false;
        }
        let applier = self
            .rotation_applier
            .lock()
            .expect("rotation applier mutex poisoned")
            .clone();
        let Some(applier) = applier else {
            return false;
        };
        match applier(offered) {
            Ok(()) => true,
            Err(detail) => {
                // 如实落回阻断。这一行是排障时唯一的线索——验过了却没写下去，
                // 与「没验过」在界面上长得一模一样。
                eprintln!("[信任锚] 轮换链验过但落地失败，退回变更阻断：{detail}");
                false
            }
        }
    }

    /// `GET /v1/trust/keys` —— 公开只读、无鉴权（公钥非秘密）。
    ///
    /// # Errors
    /// 格式版本不认识 → [`CloudError::TrustUnsupportedFormat`]（提示升级客户端，不猜）；
    /// 公钥形状不合格 → [`CloudError::UnreadableResponse`]（服务端答了，但这串不是公钥）。
    pub async fn fetch_trust_keys(
        &self,
    ) -> Result<(OfferedAnchor, Vec<RotationStatement>), CloudError> {
        let base = self.config().api_base.clone();
        self.fetch_trust_keys_at(&base).await
    }

    /// 同 [`Self::fetch_trust_keys`]，但打**指定地址**而不是当前配置的那一台。
    ///
    /// 连接链接（ADR-116）要在**切换地址之前**先核验那台服务端自报的公钥与链接里的摘要
    /// 是否一致——先切再验的话，摘要对不上时地址已经换过去了，六个消费者也都跟着换了。
    /// 零云网络行为的不变量不变：它仍只在用户显式动作（打开/粘贴链接并确认）之后调用。
    pub async fn fetch_trust_keys_at(
        &self,
        api_base: &str,
    ) -> Result<(OfferedAnchor, Vec<RotationStatement>), CloudError> {
        self.ensure_outbound_allowed()?;
        let base = api_base.trim_end_matches('/').to_owned();
        let response = self
            .http
            .for_url(&base)
            .get(format!("{base}{}", crate::trust::TRUST_KEYS_PATH))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &base))?;
        let response = check_status(response).await?;
        let body: TrustKeysResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &base))?;
        if body.format_version != crate::trust::TRUST_KEYS_FORMAT_VERSION {
            return Err(CloudError::TrustUnsupportedFormat {
                found: body.format_version,
                supported: crate::trust::TRUST_KEYS_FORMAT_VERSION,
            });
        }
        // 形状不合格的串**在这里就拒**，不让它进钉住状态。让一个「不是公钥的东西」
        // 被钉住，用户此后每一次核对都会通过，而验签永远失败——两句话互相矛盾且都不可执行。
        let (Some(lease_pubkey), Some(catalog_pubkey)) = (
            normalized_pubkey(&body.lease_pubkey),
            normalized_pubkey(&body.catalog_pubkey),
        ) else {
            return Err(CloudError::UnreadableResponse {
                host: crate::config::server_bucket(&base),
                detail: "trust keys are not valid ed25519 public keys".to_owned(),
            });
        };
        Ok((
            OfferedAnchor {
                lease_pubkey,
                catalog_pubkey,
                deployment_id: body.deployment_id,
            },
            body.rotations,
        ))
    }

    /// 清空**当前服务端**的会话凭据（重钉三重重置的第三项，修 2）。
    ///
    /// 为什么换钥必须清凭据：用户确认换钥 = 承认「这是一台新身份的服务端」。不清的话，
    /// 误确认场景下客户端会拿着旧 refresh token 继续对新身份的服务端发请求——等于把会话
    /// 凭据主动送给攻击者（SSH「对已换钥主机继续转发 agent」的同款教训）。合法重部署下
    /// 用户本就要重新登录，零额外代价。
    ///
    /// # Errors
    /// 钥匙链写失败时上抛——**不吞**：吞掉的话调用方会以为凭据已经清了。
    pub fn clear_session_credentials(&self) -> Result<(), CloudError> {
        self.cred_clear_session()
    }

    /// 服务端可用模型清单（用户视角，不含上游身份）。
    ///
    /// ADR-048 后它不再投影成客户端的模型选择器；桌面侧唯一消费者是对话上下文预算
    /// （托管路由下取 chat 能力模型的最小窗，`refresh_chat_context_window`）。
    /// 服务端保留该目录供管理、自检与第三方生态使用（TDD-16 §5.1）。
    pub async fn service_models(&self) -> Result<Vec<ServiceModelDto>, CloudError> {
        let response = self
            .authed(reqwest::Method::GET, "/inference/v1/models")
            .await?;
        let body: ServiceModelsResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        Ok(body.models)
    }

    /// 当前账户的账本列表与本月预付上限（PRD-13-038 / PRD-13-040 ④），并记下这一次认出的预警轮次
    /// （③）。**只经 [`crate::commands::fetch_usage`] 调用**：「轮次变没变 → 通知装配层」只在那里判，
    /// 别处直接调就会让一次取数的预警漏掉通知。
    ///
    /// 只回自己的、不含策略内部标识与任务明细——用量是「花了多少」而不是「做过什么」
    /// （后者会让服务端凭空多一份「用户干了啥」的可读记录）。
    ///
    /// 空列表 = 尚未触碰过配额，**不是**错误：客户端据此显示「尚未使用」，而不是伪造
    /// 一份可能与真实默认不符的额度视图。
    pub async fn usage(&self) -> Result<crate::usage::UsageDto, CloudError> {
        let response = self.authed(reqwest::Method::GET, "/v1/usage").await?;
        let wire: crate::usage::UsageWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        let (dto, episodes) = wire.into_parts();
        self.set_usage_alerts(Some(episodes));
        Ok(dto)
    }

    /// 设 / 改 / 清本月预付上限（`PUT /v1/spend-cap`，纵切 3 C4）。`None` = 清除。金额已由调用方
    /// 换算成微 credit（[`crate::billing::credits_to_micro`]）。只管预付 credits，订阅额度不受影响。
    ///
    /// # Errors
    /// 严格本地 / 未登录在发请求前返回；其余同 [`Self::authed`]。
    pub async fn set_spend_cap(
        &self,
        limit_micro_credits: Option<i64>,
    ) -> Result<crate::usage::SpendCapDto, CloudError> {
        let body = serde_json::json!({ "monthly_limit_micro_credits": limit_micro_credits });
        let response = self
            .authed_with(reqwest::Method::PUT, "/v1/spend-cap", |request| {
                request.json(&body)
            })
            .await?;
        let wire: crate::usage::SpendCapWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        Ok(wire.into_dto())
    }

    /// 任务前预估（`POST /v1/usage/estimate`，PRD-13-040 ②）。结果是「约」，最终账单以服务端计量为准；
    /// 预估失败绝不该挡住任务——调用方据此只做提示（服务端预留才是真正的闸）。
    ///
    /// # Errors
    /// 严格本地 / 未登录在发请求前返回；其余同 [`Self::authed`]。
    pub async fn usage_estimate(
        &self,
        capability: &str,
        quantity: crate::usage::EstimateQuantity,
    ) -> Result<crate::usage::UsageEstimateDto, CloudError> {
        let body = serde_json::json!({ "capability": capability, "quantity": quantity.wire() });
        let response = self
            .authed_with(reqwest::Method::POST, "/v1/usage/estimate", |request| {
                request.json(&body)
            })
            .await?;
        let wire: crate::usage::EstimateWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        Ok(wire.into_dto())
    }

    /// 取 Entitlement + Lease，并**本地**验签/验过期
    /// （TDD-14 §3：离线租约由客户端持公钥验证，服务端说了不算）。
    pub async fn entitlements(&self) -> Result<EntitlementsDto, CloudError> {
        let response = self
            .authed(reqwest::Method::GET, "/v1/entitlements")
            .await?;
        let body: EntitlementsResponse = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;

        let (lease_verified, lease_expires_at, lease_error) = match &self.config().lease_pubkey {
            None => (false, None, Some("no_pubkey".to_owned())),
            Some(key) => match lease::verify_lease(&body.lease, key, lease::now_unix()) {
                Ok(claims) => (true, u32::try_from(claims.exp).ok(), None),
                Err(err) => (false, None, Some(lease_error_code(&err).to_owned())),
            },
        };
        let capabilities = body
            .entitlements
            .iter()
            .find(|e| e.key == ENTITLEMENT_INFERENCE_CAPABILITIES)
            .and_then(|e| serde_json::from_value::<Vec<String>>(e.value.clone()).ok());
        self.set_allowed_capabilities(capabilities.clone());
        self.set_sync_max_file_bytes(
            body.entitlements
                .iter()
                .find(|e| e.key == ENTITLEMENT_SYNC_MAX_FILE_BYTES)
                .and_then(|e| e.value.as_u64()),
        );
        self.set_billing_standing(Some(body.billing_standing.clone()));
        Ok(EntitlementsDto {
            billing_standing: body.billing_standing,
            free_quota: body.free_quota,
            capabilities,
            tier: body.tier,
            entitlements: body
                .entitlements
                .into_iter()
                .map(|e| EntitlementDto {
                    key: e.key,
                    value_json: e.value.to_string(),
                })
                .collect(),
            lease_verified,
            lease_expires_at,
            lease_error,
        })
    }

    // ---- 收单（纵切 3 C2，协议 v10；形状见 [`crate::billing`]）----

    /// 收单相关命令的发前判定：严格本地硬闸，再要求有会话。
    ///
    /// 价目与 capabilities 本是公开端点，但账户页只在登录后呈现它们——未登录时一个字节都不发，
    /// 与 [`Self::authed`] 的前半段同一判定（ADR-037 不变量）。
    fn ensure_billing_reach(&self) -> Result<(), CloudError> {
        self.ensure_outbound_allowed()?;
        if !self.logged_in() {
            return Err(CloudError::NotLoggedIn);
        }
        Ok(())
    }

    fn remember_deployment(&self, base: &str, caps: &protocol::CapabilitiesDto) {
        *self.deployment.lock().expect("deployment mutex poisoned") = Some((
            crate::config::server_bucket(base),
            DeploymentFacts::from(caps),
        ));
    }

    /// 价目回了 503 `billing_disabled`（两次请求之间运营关了收单）：只改「卖不卖」这一格，
    /// 部署的其余事实是同一次 capabilities 取到的，仍然成立。
    fn forget_billing(&self, base: &str) {
        let bucket = crate::config::server_bucket(base);
        if let Some((at, facts)) = self
            .deployment
            .lock()
            .expect("deployment mutex poisoned")
            .as_mut()
        {
            if *at == bucket {
                facts.billing_enabled = false;
            }
        }
    }

    /// 当前服务器上一次取到的部署事实（`None` = 还不知道）。零网络。
    #[must_use]
    pub fn deployment_facts(&self) -> Option<DeploymentFacts> {
        let bucket = self.server_bucket();
        self.deployment
            .lock()
            .expect("deployment mutex poisoned")
            .as_ref()
            .filter(|(at, _)| *at == bucket)
            .map(|(_, facts)| facts.clone())
    }

    /// 重取当前服务器的 capabilities 并记下部署事实（也就顺带协商了一次协议）。
    ///
    /// # Errors
    /// 同 [`Self::fetch_capabilities_at`]。
    pub async fn refresh_deployment_facts(&self) -> Result<DeploymentFacts, CloudError> {
        let base = self.config().api_base.clone();
        let caps = self.fetch_capabilities_at(&base).await?;
        self.remember_deployment(&base, &caps);
        Ok(DeploymentFacts::from(&caps))
    }

    /// 当前服务器卖不卖。**记得就不发请求**；不记得（重启后还没进过账户页）才取一次 capabilities。
    ///
    /// # Errors
    /// 严格本地 / 未登录在发请求前返回；其余同 [`Self::fetch_capabilities_at`]。
    pub async fn billing_enabled(&self) -> Result<bool, CloudError> {
        self.ensure_billing_reach()?;
        let facts = match self.deployment_facts() {
            Some(facts) => facts,
            None => self.refresh_deployment_facts().await?,
        };
        Ok(facts.billing_enabled)
    }

    /// 账户页「升级与购买」一节要的全部事实：卖不卖（**每次都重取**——这是用户打开 / 刷新账户页
    /// 的时刻，运营刚开了收单也该立刻看得到），卖的话在售什么、账怎么算，以及这个账户此刻的
    /// 订阅处境（按权益 `tier`，见 [`crate::billing::subscription_standing`]）。
    ///
    /// 权益由调用方给（`entitlements`，只在卖的时候才等它；档位与付款处境都从它读）：取权益会顺手
    /// 刷新本客户端缓存的「可用 AI 能力」，而「变没变 → 通知装配层」只在
    /// [`crate::commands::fetch_entitlements`] 里判。
    /// 这里若自己取，付款回来账户页并行刷新时先到的一路就把缓存改掉，另一路比不出变化——
    /// 刚买的云推理要等重启才生效。故命令层传入经那条路径取到的权益；取不到为 `None`。
    ///
    /// capabilities 说卖、价目却回 503 `billing_disabled`（两次请求之间运营关了收单）时，按价目
    /// 那一句算：它更新。
    ///
    /// # Errors
    /// 严格本地 / 未登录在发请求前返回；连不上、读不懂、协议不兼容各有各的码。
    pub async fn billing_offer(
        &self,
        entitlements: impl std::future::Future<Output = Option<EntitlementsDto>>,
    ) -> Result<BillingOfferDto, CloudError> {
        self.ensure_billing_reach()?;
        if !self.refresh_deployment_facts().await?.billing_enabled {
            return Ok(BillingOfferDto::Disabled);
        }
        let base = self.config().api_base.clone();
        let response = self
            .http()
            .get(self.url("/v1/billing/plans"))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &base))?;
        let response = match check_status(response).await {
            Ok(response) => response,
            Err(err) => {
                // 价目没有「这一项」可言，404 不改写，照通用服务端错误说。
                let unchanged = err.clone();
                return match map_billing_error(err, unchanged) {
                    CloudError::BillingDisabled => {
                        self.forget_billing(&base);
                        Ok(BillingOfferDto::Disabled)
                    }
                    other => Err(other),
                };
            }
        };
        let wire: crate::billing::PlansWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &base))?;
        let mut plans = wire.into_dto();
        // 订阅处境要当前档位：取不到权益不让整节失败，处境如实为「不知道」。
        let entitlements = entitlements.await;
        let standing = crate::billing::subscription_standing(
            &mut plans,
            entitlements.as_ref().map(|ents| ents.tier.as_str()),
        );
        let payment = entitlements.map(|ents| ents.billing_standing);
        Ok(BillingOfferDto::Enabled {
            plans,
            standing,
            purchasable: crate::billing::purchasable(payment.as_ref()),
            payment,
        })
    }

    /// 为价目里的一项建 Stripe 托管付款页，返回它的地址（交给系统浏览器；**不进日志**）。
    ///
    /// # Errors
    /// 那一项已不在售 → [`CloudError::PlanUnavailable`]；本部署未启用收单 →
    /// [`CloudError::BillingDisabled`]；账户被冻结 → [`CloudError::AccountFrozen`]（`check_status`
    /// 统一映射）；其余同 [`Self::authed`]。
    pub async fn billing_checkout_url(&self, plan_id: &str) -> Result<String, CloudError> {
        let body = serde_json::json!({ "plan_id": plan_id });
        let response = self
            .authed_with(reqwest::Method::POST, "/v1/billing/checkout", |request| {
                request.json(&body)
            })
            .await
            .map_err(|err| map_billing_error(err, CloudError::PlanUnavailable))?;
        self.hosted_session_url(response).await
    }

    /// 建 Stripe 托管账单管理页（改付款方式 / 改档 / 退订 / 发票），返回它的地址。
    ///
    /// # Errors
    /// 这个账户还没付过款 → [`CloudError::NoBillingAccount`]；本部署未启用收单 →
    /// [`CloudError::BillingDisabled`]；其余同 [`Self::authed`]。
    pub async fn billing_portal_url(&self) -> Result<String, CloudError> {
        let response = self
            .authed(reqwest::Method::POST, "/v1/billing/portal")
            .await
            .map_err(|err| map_billing_error(err, CloudError::NoBillingAccount))?;
        self.hosted_session_url(response).await
    }

    /// 建零扣费的付款方式验证页（免费额度因风险信号被扣下时，纵切 3 C5 / PRD-13-018 L2），返回它的
    /// 地址（交给系统浏览器；**不进日志、不过 IPC**，同结账）。
    ///
    /// # Errors
    /// 这个账户已不需要验证 → [`CloudError::VerificationNotRequired`]（界面重取权益）；本部署未启用收单
    /// → [`CloudError::BillingDisabled`]；其余同 [`Self::authed`]。
    pub async fn billing_verify_card_url(&self) -> Result<String, CloudError> {
        let response = self
            .authed(reqwest::Method::POST, "/v1/billing/verify-card")
            .await
            .map_err(map_verify_card_error)?;
        self.hosted_session_url(response).await
    }

    async fn hosted_session_url(&self, response: reqwest::Response) -> Result<String, CloudError> {
        let session: crate::billing::HostedSessionWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        Ok(session.url)
    }

    /// 登出：尽力吊销服务端会话，**无论如何**清空本地凭据。
    ///
    /// 「无论如何」此前只覆盖了四种错误（会话已失效 / 未登录 / 严格本地 / 连不上），其余
    /// ——服务端 5xx、限流、读不懂的响应——一律原样返回、**本地凭据不清**。2026-09-27
    /// 真机：部署机数据库盘满，`/v1/auth/refresh` 卡满 30 秒后回 500，于是「退出登录」
    /// 点下去先是半分钟毫无动静，然后仍是登录态——用户想登出重登，这一步本身就走不通，
    /// 而服务端坏掉正是他最需要登出重登的时候。
    ///
    /// 登出的意图是**这台设备不再以这个账户行事**，它只取决于本地凭据；服务端吊销是
    /// 附带的尽力而为，结果如实放进 `server_revoked`（前端据此说「服务端那边可能仍在」）。
    /// 故吊销的任何失败都不阻止本地清除，且整个吊销**有时限**（[`LOGOUT_REVOKE_DEADLINE`]）：
    /// `authed` 里那次 401 → refresh 在服务端卡住时，不能把一次登出拖成半分钟。
    /// 唯一会让登出失败的是本地凭据清不掉（钥匙链写失败）——那才是真的没登出。
    pub async fn logout(&self) -> Result<LogoutDto, CloudError> {
        let revoke = self.authed(reqwest::Method::POST, "/v1/auth/logout");
        let server_revoked = matches!(
            tokio::time::timeout(LOGOUT_REVOKE_DEADLINE, revoke).await,
            Ok(Ok(_))
        );
        self.cred_clear_session()?;
        Ok(LogoutDto { server_revoked })
    }

    /// GDPR 注销（确认在前端；后端一次调用，协议 v14）。成功后清空本地凭据。
    /// 「本地数据不受影响」是产品承诺：本命令绝不触碰 Vault/设置等本地文件。
    ///
    /// 回执只带**保留了什么**（计费与用量记录保留几年，[`AccountDeleteDto`]）——文案由界面按语言
    /// 渲染，服务端不再下发一句中文。回执说的不是「已删除」时如实报读不懂，**不清会话**：
    /// 没有确认删掉，就不能让这台设备表现得像删掉了。
    ///
    /// # Errors
    /// 503 `billing_unavailable`（订阅退不掉，什么都没删）→ [`CloudError::BillingUnavailable`]；
    /// 其余同 [`Self::authed`]。
    pub async fn delete_account(&self) -> Result<AccountDeleteDto, CloudError> {
        let response = self
            .authed(reqwest::Method::DELETE, "/v1/account")
            .await
            .map_err(crate::account_data::map_delete_error)?;
        let wire: crate::account_data::DeleteWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        let dto = wire.into_dto(&self.server_bucket())?;
        self.cred_clear_session()?;
        Ok(dto)
    }

    /// 注销前的后果预览（`GET /v1/account/deletion-preview`，协议 v14）：有没有在扣费的订阅、预付
    /// 余额（已换算成 credits）、计费记录保留几年。只读，不改任何东西。
    ///
    /// # Errors
    /// 同 [`Self::authed`]。
    pub async fn deletion_preview(&self) -> Result<AccountDeletionPreviewDto, CloudError> {
        let response = self
            .authed(reqwest::Method::GET, "/v1/account/deletion-preview")
            .await?;
        let wire: crate::account_data::DeletionPreviewWire = response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))?;
        Ok(wire.into_dto())
    }

    /// 把账户数据导出（`GET /v1/account/export`，协议 v14）**流式**写进 `target`：边收边写到同目录
    /// 的临时文件，收完、落盘之后才改名成 `target`。整份响应体从不整块进内存（它可能很大）。
    ///
    /// 任何一步失败（连接中断、写盘失败），或这个 future 被丢弃（取消），临时文件随之删除——
    /// `target` 要么是一份完整的导出，要么保持原样（原先有同名文件时不被半份内容覆盖）。
    ///
    /// # Errors
    /// 429 → [`CloudError::RateLimited`]；写盘失败 → [`CloudError::ExportWriteFailed`]；其余同
    /// [`Self::authed`]。
    pub async fn export_account_to(&self, target: &std::path::Path) -> Result<(), CloudError> {
        let response = self
            .authed(reqwest::Method::GET, "/v1/account/export")
            .await?;
        crate::account_data::stream_to_file(response, target, &self.config().api_base).await
    }
}

// ---- 账户与登录（ADR-155）----

impl CloudClient {
    /// 账户全量状态（**零网络**：读本地记住的服务器、钥匙链会话与进程内的配对状态）。
    #[must_use]
    pub fn account_state(&self) -> AccountStateDto {
        let phase = if self.logged_in() {
            AccountPhaseDto::SignedIn {
                profile: self.cached_profile(),
            }
        } else if let Some(limit) = self.device_limit() {
            // 先于「等浏览器」：设备码撞上限时配对还留着，此刻要渲染的是选哪一台。
            AccountPhaseDto::DeviceLimit { limit }
        } else if let Some((user_code, verification_uri)) = self.pending_login() {
            AccountPhaseDto::AwaitingBrowser {
                user_code,
                verification_uri,
            }
        } else {
            AccountPhaseDto::SignedOut
        };
        let incoming_link = self
            .incoming_link
            .lock()
            .expect("incoming link mutex poisoned")
            .as_ref()
            .map(|link| IncomingLinkDto {
                server: link.server.clone(),
                display_name: link.display_name.clone(),
                has_invite: link.invite.is_some(),
            });
        AccountStateDto {
            server: self.account.server().map(|server| server.dto()),
            phase,
            incoming_link,
        }
    }

    /// URL scheme 送达一条连接链接：**只存不发**（ADR-037），等用户在登录界面确认。
    ///
    /// # Errors
    /// 链接坏了 → 原因（调用方只记日志：送达时没有界面可以说）。
    pub fn stash_incoming_link(
        &self,
        raw: &str,
    ) -> Result<(), crate::connect_link::ConnectLinkRejection> {
        let link = crate::connect_link::parse(raw)?;
        *self
            .incoming_link
            .lock()
            .expect("incoming link mutex poisoned") = Some(link);
        Ok(())
    }

    /// 放弃挂着的链接、进行中的这次登录、浏览器配对与撞上设备上限后挂着的选择（「取消」）。
    pub fn sign_in_cancel(&self) {
        *self
            .incoming_link
            .lock()
            .expect("incoming link mutex poisoned") = None;
        self.forget_sign_in();
        self.login_cancel();
        self.clear_device_limit();
    }

    /// 丢掉这次登录在内存里留下的一切（选定的服务器与邮箱、邀请码、注册授权）。
    fn forget_sign_in(&self) {
        *self.sign_in.lock().expect("sign-in mutex poisoned") = None;
        *self
            .registration
            .lock()
            .expect("registration mutex poisoned") = None;
        *self.invite.lock().expect("invite mutex poisoned") = None;
    }

    fn sign_in_context(&self) -> Result<SignInContext, CloudError> {
        self.sign_in
            .lock()
            .expect("sign-in mutex poisoned")
            .clone()
            .ok_or(CloudError::NoPendingLogin)
    }

    /// 开始登录（ADR-158 ①）：规整邮箱 → 定服务器（记住的 → 邮箱域名上的 `.well-known/lunarca` →
    /// Lunarca 云；或用户给的地址 / 链接）→ 能经校验的 TLS 连上就先取 capabilities 协商协议与登录方式。
    ///
    /// 自签且还没钉住的服务器此刻连不上（系统信任库不认它）：不报错，返回 `methods = None`，
    /// 信任在口令那一步随握手建立。服务器在登录成功（或信任已钉住）时才记住。
    ///
    /// # Errors
    /// 已登录 → [`CloudError::AlreadySignedIn`]；不是邮箱、不是地址、链接坏了、身份需要确认、
    /// 协议不兼容、连不上，各有各的码。
    pub async fn sign_in_begin(&self, target: SignInTarget) -> Result<SignInStepDto, CloudError> {
        if self.logged_in() {
            return Err(CloudError::AlreadySignedIn);
        }
        self.ensure_outbound_allowed()?;
        // 重新开始登录：上一次的选择、授权与撞上限挂着的选择一并作废。
        self.clear_device_limit();
        self.forget_sign_in();
        let raw_email = match &target {
            SignInTarget::Discover { email }
            | SignInTarget::Server { email, .. }
            | SignInTarget::IncomingLink { email } => email,
        };
        let email = opaque_auth::normalize_email(raw_email);
        let domain = opaque_auth::email_domain(&email)
            .ok_or(CloudError::NotAnEmail)?
            .to_owned();
        let mut from_incoming = false;
        let (server, expected_deployment, invite) = match target {
            SignInTarget::Discover { .. } => {
                let (server, expected) = self.discover(&domain).await;
                (server, expected, None)
            }
            SignInTarget::Server { input, .. } => {
                if crate::connect_link::looks_like_link(&input) {
                    let link = crate::connect_link::parse(&input)
                        .map_err(|reason| CloudError::LinkRejected { reason })?;
                    (link.server_account(), None, link.invite)
                } else {
                    let origin = crate::account::normalize_server_address(&input)
                        .ok_or(CloudError::NotAnAddress)?;
                    (
                        AccountServer {
                            origin,
                            display_name: None,
                        },
                        None,
                        None,
                    )
                }
            }
            SignInTarget::IncomingLink { .. } => {
                let link = self
                    .incoming_link
                    .lock()
                    .expect("incoming link mutex poisoned")
                    .clone()
                    .ok_or(CloudError::NotAnAddress)?;
                from_incoming = true;
                (link.server_account(), None, link.invite)
            }
        };
        let step = self
            .sign_in_at(server, email, expected_deployment, invite.is_some())
            .await?;
        if matches!(step, SignInStepDto::Ready { .. }) {
            *self.invite.lock().expect("invite mutex poisoned") = invite;
            if from_incoming {
                *self
                    .incoming_link
                    .lock()
                    .expect("incoming link mutex poisoned") = None;
            }
        }
        Ok(step)
    }

    /// 按邮箱找服务器：这台设备记住的 → 邮箱域名上发布的 → Lunarca 云。
    async fn discover(&self, domain: &str) -> (AccountServer, Option<String>) {
        if let Some(server) = self.account.server() {
            return (server, None);
        }
        if let Some(found) = self.well_known(domain).await {
            return (
                AccountServer {
                    origin: found.server,
                    display_name: None,
                },
                Some(found.deployment_id),
            );
        }
        (AccountServer::official(), None)
    }

    /// 取 `https://<域名>/.well-known/lunarca`。只认系统信任库、不跟随跳转；取不到、读不懂一律跳过。
    async fn well_known(&self, domain: &str) -> Option<WellKnown> {
        #[cfg(test)]
        let base = self
            .discovery_base
            .lock()
            .expect("discovery mutex poisoned")
            .clone()
            .unwrap_or_else(|| format!("https://{domain}"));
        #[cfg(not(test))]
        let base = format!("https://{domain}");
        let url = reqwest::Url::parse(&format!("{base}{WELL_KNOWN_PATH}")).ok()?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(WELL_KNOWN_TIMEOUT)
            .build()
            .ok()?;
        let response = client.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let found: WellKnown = response.json().await.ok()?;
        if found.format_version != 1 || found.deployment_id.is_empty() {
            return None;
        }
        let server = crate::connect_link::normalize_origin(&found.server)?;
        Some(WellKnown { server, ..found })
    }

    async fn sign_in_at(
        &self,
        server: AccountServer,
        email: String,
        expected_deployment: Option<String>,
        invite: bool,
    ) -> Result<SignInStepDto, CloudError> {
        let caps = match self.fetch_capabilities_at(&server.origin).await {
            Ok(caps) => Some(caps),
            // 系统信任库不认它（自签、还没钉住），或钉住的根已经换掉：登录前看不到登录方式，
            // 口令握手会建立（或重建）信任。别的失败照常报。
            Err(CloudError::Network {
                kind: crate::error::NetworkFailureKind::Tls,
                ..
            }) => None,
            Err(err) => return Err(err),
        };
        let mut server = server;
        if let Some(caps) = &caps {
            if expected_deployment
                .as_deref()
                .is_some_and(|expected| expected != caps.deployment.id)
            {
                return Err(CloudError::ServerIdentityRejected);
            }
            if !server.is_official() {
                if let Some(trust) = self.check_offered_anchor(&server.origin).await? {
                    return Ok(SignInStepDto::NeedsTrustConfirmation { trust });
                }
            }
            self.remember_deployment(&server.origin, caps);
            server.display_name = server
                .display_name
                .or_else(|| caps.deployment.display_name.clone());
            self.account.set_server(server.clone());
            self.reconfigure();
        }
        *self.sign_in.lock().expect("sign-in mutex poisoned") = Some(SignInContext {
            server: server.clone(),
            email: email.clone(),
            expected_deployment,
            trusted_tls: caps.is_some(),
        });
        Ok(SignInStepDto::Ready {
            server: server.dto(),
            email,
            methods: caps.map(|caps| {
                let login = caps.auth.login;
                SignInMethodsDto {
                    password: login.password,
                    browser: login.browser,
                    providers: login
                        .providers
                        .into_iter()
                        .map(|p| SignInProviderDto {
                            id: p.id,
                            name: p.name,
                            kind: p.kind,
                        })
                        .collect(),
                    admission: login.admission,
                    password_reset: login.password_reset,
                }
            }),
            invite,
        })
    }

    /// 经可信 TLS 连得上的组织服务器：核对它自报的两把签名公钥（ADR-060 ②）。没钉过就钉
    /// （来源 `Tofu`：浏览器 / 单点登录那一路靠它）；与钉住不一致 → 交给用户确认。
    async fn check_offered_anchor(
        &self,
        origin: &str,
    ) -> Result<Option<TrustStatusDto>, CloudError> {
        let bucket = crate::config::server_bucket(origin);
        // 轮换链在这条路上用不上：核的是「这台是不是我要连的那台」。
        let (offered, _rotations) = self.fetch_trust_keys_at(origin).await?;
        match self
            .trust
            .record_import_conflict(&bucket, offered.clone(), origin)
        {
            TrustVerdict::Changed => return Ok(Some(self.trust.status(&bucket))),
            TrustVerdict::Unpinned => self.trust.pin(
                &bucket,
                PinnedAnchor {
                    lease_pubkey: offered.lease_pubkey,
                    catalog_pubkey: offered.catalog_pubkey,
                    pinned_at: crate::catalog::now_unix(),
                    provenance: TrustProvenance::Tofu,
                    deployment_id: offered.deployment_id,
                },
            ),
            TrustVerdict::Match | TrustVerdict::Bypassed => {}
        }
        Ok(None)
    }

    /// 正式构建里的官方后端（ADR-158 ④ 纵深防御）：握手只走系统信任库，信任包只认 `public`。
    /// 调试构建放过——那里的官方后端（`DEV_API_BASE`）就是自签的联调服务端。
    fn strict_official(&self, server: &AccountServer) -> bool {
        #[cfg(test)]
        if let Some((origin, release)) = self
            .official_policy
            .lock()
            .expect("official policy mutex poisoned")
            .clone()
        {
            return release && origin == server.origin;
        }
        server.is_official() && !cfg!(debug_assertions)
    }

    /// 握手走哪条 TLS：可信 TLS 连得上就走它（不放宽）；否则走首连（[`crate::first_contact`]）。
    fn channel_for(&self, ctx: &SignInContext) -> Channel {
        // 正式构建的官方后端是公网证书：握手只走系统信任库，连不上就如实报错，不放宽。
        if ctx.trusted_tls || self.strict_official(&ctx.server) {
            Channel::Trusted(self.http.for_url(&ctx.server.origin))
        } else {
            Channel::FirstContact
        }
    }

    /// 握手完成之后：钉住信任包里的 TLS 身份 → **改走钉住的 TLS 重连**（主机名照常对着用户给的
    /// origin 校验，中继攻击的最后一道防线，ADR-158 ③）→ 协商协议 → 选定服务器 → 钉住两把签名公钥。
    ///
    /// 重连失败 → 撤回这一次的根证书钉住，什么都不留。两钥与既有钉住不一致 → 返回待确认的状态
    /// （服务器已选定，确认流程能找到它），不领会话。
    async fn adopt_handshake(
        &self,
        ctx: &SignInContext,
        handshake: &first_contact::Handshake,
    ) -> Result<Option<TrustStatusDto>, CloudError> {
        let origin = ctx.server.origin.as_str();
        let bucket = crate::config::server_bucket(origin);
        if !official_bundle_allowed(self.strict_official(&ctx.server), &handshake.bundle.tls) {
            eprintln!("[account] 官方后端送来自签根证书的信任包，已丢弃");
            return Err(CloudError::Network {
                host: bucket,
                detail: "official backend sent a pinned_root trust bundle".to_owned(),
                kind: crate::error::NetworkFailureKind::Tls,
            });
        }
        let previous = self.tls_roots.pinned(&bucket);
        match &handshake.bundle.tls {
            BundleTls::PinnedRoot { root_pem } => {
                let same = previous.as_ref().map(|p| p.sha256.as_str())
                    == crate::tls_root::fingerprint(root_pem).as_deref();
                if !same {
                    self.tls_roots
                        .pin(origin, root_pem, crate::catalog::now_unix())
                        .map_err(|_| CloudError::ServerIdentityRejected)?;
                }
            }
            BundleTls::Public => {
                if previous.is_some() {
                    self.tls_roots.unpin(&bucket);
                }
            }
        }
        let caps = match self.fetch_capabilities_at(origin).await {
            Ok(caps) => caps,
            Err(err) => {
                eprintln!("[account] 握手之后经钉住的 TLS 重连失败，已撤回这次钉住：{err}");
                self.tls_roots.restore(&bucket, previous);
                return Err(err);
            }
        };
        if caps.deployment.id != handshake.deployment_id {
            self.tls_roots.restore(&bucket, previous);
            return Err(CloudError::ServerIdentityRejected);
        }
        self.remember_deployment(origin, &caps);
        self.account.set_server(AccountServer {
            origin: origin.to_owned(),
            display_name: ctx
                .server
                .display_name
                .clone()
                .or(caps.deployment.display_name),
        });
        self.reconfigure();
        if ctx.server.is_official() {
            // 官方后端的验签公钥随应用编入，不走钉住。
            return Ok(None);
        }
        let bundle = &handshake.bundle;
        let (Some(lease_pubkey), Some(catalog_pubkey)) = (
            normalized_pubkey(&bundle.lease_pubkey),
            normalized_pubkey(&bundle.catalog_pubkey),
        ) else {
            return Err(CloudError::ServerIdentityRejected);
        };
        let offered = OfferedAnchor {
            lease_pubkey,
            catalog_pubkey,
            deployment_id: handshake.deployment_id.clone(),
        };
        match self
            .trust
            .record_password_conflict(&bucket, offered.clone())
        {
            TrustVerdict::Changed => return Ok(Some(self.trust.status(&bucket))),
            TrustVerdict::Unpinned => self.trust.pin(
                &bucket,
                PinnedAnchor {
                    lease_pubkey: offered.lease_pubkey,
                    catalog_pubkey: offered.catalog_pubkey,
                    pinned_at: crate::catalog::now_unix(),
                    provenance: TrustProvenance::Password,
                    deployment_id: offered.deployment_id,
                },
            ),
            TrustVerdict::Match | TrustVerdict::Bypassed => {}
        }
        // 钉住的两把钥刚换上：让配置一起换。
        self.reconfigure();
        Ok(None)
    }

    /// 握手 + 钉住 + 改走钉住的通道领取（口令与一次性码共用）。
    async fn handshake_and_claim(
        &self,
        secret: &[u8],
        purpose: Purpose,
        device_name: &str,
        platform: &str,
    ) -> Result<ClaimOutcome, CloudError> {
        self.ensure_outbound_allowed()?;
        // 开始另一次登录 = 上一次撞上限的那一步作废。
        self.clear_device_limit();
        let ctx = self.sign_in_context()?;
        let handshake = first_contact::login(
            &ctx.server.origin,
            &ctx.email,
            secret,
            purpose,
            ctx.expected_deployment.as_deref(),
            self.channel_for(&ctx),
        )
        .await?;
        if let Some(trust) = self.adopt_handshake(&ctx, &handshake).await? {
            return Ok(ClaimOutcome::NeedsTrust(trust));
        }
        // 领到会话之后存不下的话，领取资格已经用掉、本机却没有会话：先试写，写不进就在这里停下。
        self.ensure_credentials_writable()?;
        let answer = self
            .send_grant(
                SESSION_PATH,
                serde_json::json!({
                    "login_id": handshake.login_id,
                    "claim": handshake.claim.as_str(),
                    "device_name": device_name,
                    "platform": platform,
                    "device_fingerprint": self.fingerprint.get().as_str(),
                }),
                None,
            )
            .await?;
        Ok(ClaimOutcome::Claimed {
            answer,
            deployment_id: handshake.deployment_id,
            email: ctx.email,
        })
    }

    /// 口令登录（ADR-158 ②③）：OPAQUE 握手 → 钉住信任包 → 经钉住的通道领取设备会话。
    ///
    /// # Errors
    /// 没有开始登录 → [`CloudError::NoPendingLogin`]；邮箱或口令不对 → `LoginRejected
    /// { invalid_credentials }`；信任包不可信 → [`CloudError::ServerIdentityRejected`]；其余照常。
    pub async fn sign_in_password(
        &self,
        password: &str,
        device_name: &str,
        platform: &str,
    ) -> Result<PasswordSignInDto, CloudError> {
        let claimed = self
            .handshake_and_claim(
                password.as_bytes(),
                Purpose::Password,
                device_name,
                platform,
            )
            .await?;
        let answer = match claimed {
            ClaimOutcome::Claimed { answer, .. } => answer,
            ClaimOutcome::NeedsTrust(trust) => {
                return Ok(PasswordSignInDto::NeedsTrustConfirmation {
                    trust: Box::new(trust),
                })
            }
        };
        match answer {
            Some(answer) => {
                self.finish_with_tokens(answer).await?;
                Ok(PasswordSignInDto::SignedIn)
            }
            None => Ok(PasswordSignInDto::DeviceLimitReached),
        }
    }

    /// 一次性码登录（ADR-158 ⑥：重设 / 邀请 / 激活 / 注册验证）：握手同口令登录，领到的是只够设口令的
    /// 注册授权（留在 Rust 内存里）。`code = None` → 用连接链接带来的邀请码。
    ///
    /// # Errors
    /// 没有码可用 → [`CloudError::NoInvite`]；码不对或已过期 → `LoginRejected { invalid_credentials }`；
    /// 其余同 [`Self::sign_in_password`]。
    pub async fn sign_in_code(
        &self,
        code: Option<&str>,
        device_name: &str,
        platform: &str,
    ) -> Result<CodeSignInDto, CloudError> {
        let raw = match code {
            Some(code) => code.to_owned(),
            None => self
                .invite
                .lock()
                .expect("invite mutex poisoned")
                .clone()
                .ok_or(CloudError::NoInvite)?,
        };
        // 形状不对的码不发请求：服务端那边也只认规整后恰为 10 位的 Crockford 码。
        let code = zeroize::Zeroizing::new(opaque_auth::normalize_code(&raw).ok_or(
            CloudError::LoginRejected {
                reason: LoginRejection::InvalidCredentials,
            },
        )?);
        let claimed = self
            .handshake_and_claim(code.as_bytes(), Purpose::Code, device_name, platform)
            .await?;
        let (answer, deployment_id, email) = match claimed {
            ClaimOutcome::Claimed {
                answer,
                deployment_id,
                email,
            } => (answer, deployment_id, email),
            ClaimOutcome::NeedsTrust(trust) => {
                return Ok(CodeSignInDto::NeedsTrustConfirmation {
                    trust: Box::new(trust),
                })
            }
        };
        let answer = answer.ok_or_else(|| CloudError::internal("device limit on a code claim"))?;
        let granted: RegistrationGrantResponse =
            serde_json::from_value(answer).map_err(|err| CloudError::UnreadableResponse {
                host: self.server_bucket(),
                detail: err.to_string(),
            })?;
        *self
            .registration
            .lock()
            .expect("registration mutex poisoned") = Some(RegistrationGrant {
            grant: zeroize::Zeroizing::new(granted.registration_grant),
            email,
            deployment_id,
            expires_at: Instant::now() + Duration::from_secs(granted.expires_in),
        });
        // 码已兑换成授权：链接带来的邀请用掉了。
        *self.invite.lock().expect("invite mutex poisoned") = None;
        Ok(CodeSignInDto::SetPassword)
    }

    /// 凭一次性码换来的注册授权设口令，随即登录（ADR-158 ⑤⑥；服务端让这个账户的其他设备退出）。
    ///
    /// # Errors
    /// 没有授权或已过期 → [`CloudError::LoginExpired`]；口令不合规 → `LoginRejected
    /// { password_invalid }`（授权留着，改一个再试）；其余照常。
    pub async fn sign_in_set_password(
        &self,
        new_password: &str,
        device_name: &str,
        platform: &str,
    ) -> Result<PasswordSignInDto, CloudError> {
        self.ensure_outbound_allowed()?;
        self.clear_device_limit();
        let (grant, email, deployment_id) = {
            let registration = self
                .registration
                .lock()
                .expect("registration mutex poisoned");
            match registration.as_ref() {
                Some(r) if r.expires_at > Instant::now() => {
                    (r.grant.clone(), r.email.clone(), r.deployment_id.clone())
                }
                _ => return Err(CloudError::LoginExpired),
            }
        };
        self.ensure_credentials_writable()?;
        let answer = self
            .register_password(
                grant,
                &email,
                &deployment_id,
                new_password,
                device_name,
                platform,
            )
            .await?;
        match answer {
            Some(answer) => {
                self.finish_with_tokens(answer).await?;
                Ok(PasswordSignInDto::SignedIn)
            }
            // 授权随挂着的重发一起留在设备上限那一步里；这里不再需要它。
            None => {
                *self
                    .registration
                    .lock()
                    .expect("registration mutex poisoned") = None;
                Ok(PasswordSignInDto::DeviceLimitReached)
            }
        }
    }

    /// 请这次登录选定的服务器往邮箱发一枚一次性码（忘记口令 / 创建账户）。服务端一律回 202，不说
    /// 这个邮箱有没有账户（反枚举）；界面因此只能说「若可用，码已发出」，并当场给输入框。
    ///
    /// # Errors
    /// 没有开始登录 → [`CloudError::NoPendingLogin`]；一次性邮箱域名 →
    /// [`CloudError::EmailDomainNotAllowed`]；限流、网络照常。
    pub async fn sign_in_request_code(&self, kind: CodeKind) -> Result<(), CloudError> {
        self.ensure_outbound_allowed()?;
        let ctx = self.sign_in_context()?;
        let path = match kind {
            CodeKind::Reset => first_contact::FORGOT_PATH,
            CodeKind::Signup => first_contact::SIGNUP_START_PATH,
        };
        first_contact::request_code(&ctx.server.origin, path, &ctx.email, self.channel_for(&ctx))
            .await
    }

    /// 撞上设备上限之后「移除这台并登录」：`POST /v1/devices/revoke-for-sign-in`，成功即把挂着的那次
    /// 登录原样再来一次——应用内登录重发领取会话（或注册口令）那一条，设备码接着轮询同一个配对（纵切 3 C5）。
    ///
    /// 令牌是一次性的：撤销请求一旦被服务端受理（204）或明确拒绝（4xx），挂着的选择即清掉；连不上 /
    /// 限频 / 5xx 时留着，用户可以再点一次。
    ///
    /// 结果 [`DeviceLimitResolutionDto::AwaitingBrowser`] 时，设备码配对还在等浏览器——调用方接着轮询。
    ///
    /// # Errors
    /// 没有挂着的选择、令牌过期或已用、服务端不认这台 → [`CloudError::DeviceLimitExpired`]（设备码配对
    /// 一并作废：重新登录）；交来的 id 不在服务端列出的设备里 → [`CloudError::Internal`]（不发请求，
    /// 选择留着）；配对或领取资格过期 → [`CloudError::LoginExpired`]；其余同应用内登录。
    pub async fn sign_in_resolve_device_limit(
        &self,
        device_id: &str,
    ) -> Result<DeviceLimitResolutionDto, CloudError> {
        self.ensure_outbound_allowed()?;
        let (revoke_token, listed) = {
            let pending = self
                .device_limit
                .lock()
                .expect("device limit mutex poisoned");
            let Some(pending) = pending.as_ref() else {
                return Err(CloudError::DeviceLimitExpired);
            };
            (
                pending.revoke_token.clone(),
                pending
                    .limit
                    .devices
                    .iter()
                    .any(|device| device.id == device_id),
            )
        };
        // 只认服务端列出来的那几台：界面交来别的 id 不发请求（令牌只为这份列表签发）。
        if !listed {
            return Err(CloudError::internal("device not offered for removal"));
        }
        let response = self
            .http()
            .post(self.url("/v1/devices/revoke-for-sign-in"))
            .json(&serde_json::json!({
                "revoke_token": revoke_token,
                "device_id": device_id,
            }))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, &self.config().api_base))?;
        let status = response.status();
        if !status.is_success() {
            if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
                // 令牌过期 / 已用 / 不认这台：这次选择作废，重新登录（设备码配对一并丢掉，免得
                // 界面退回一个永远停在「设备已满」的配对上）。
                self.abandon_device_limit();
                return Err(CloudError::DeviceLimitExpired);
            }
            return Err(check_status(response)
                .await
                .err()
                .unwrap_or_else(|| CloudError::internal("revoke-for-sign-in")));
        }
        let Some(pending) = self
            .device_limit
            .lock()
            .expect("device limit mutex poisoned")
            .take()
        else {
            // 撤销期间用户取消了这次登录：设备已移除，但没有要重发的登录了。
            return Err(CloudError::DeviceLimitExpired);
        };
        match pending.retry {
            SignInRetry::Grant { path, body, bearer } => {
                match self.send_grant(path, body, bearer).await? {
                    Some(answer) => {
                        self.finish_with_tokens(answer).await?;
                        Ok(DeviceLimitResolutionDto::SignedIn)
                    }
                    None => Ok(DeviceLimitResolutionDto::DeviceLimitReached),
                }
            }
            SignInRetry::DeviceCode => {
                let poll = self.login_poll().await?;
                match poll.status {
                    LoginPollStatus::Authorized => Ok(DeviceLimitResolutionDto::SignedIn),
                    LoginPollStatus::Pending | LoginPollStatus::SlowDown => {
                        Ok(DeviceLimitResolutionDto::AwaitingBrowser)
                    }
                    LoginPollStatus::DeviceLimitReached => {
                        Ok(DeviceLimitResolutionDto::DeviceLimitReached)
                    }
                    LoginPollStatus::Expired => Err(CloudError::LoginExpired),
                }
            }
        }
    }

    /// 这次撞上限的选择整体作废：挂着的令牌与重发内容丢掉；若是设备码那一路，配对也一并丢掉。
    fn abandon_device_limit(&self) {
        let pending = self
            .device_limit
            .lock()
            .expect("device limit mutex poisoned")
            .take();
        if matches!(
            pending.map(|pending| pending.retry),
            Some(SignInRetry::DeviceCode)
        ) {
            self.login_cancel();
        }
    }

    /// 退出登录：服务端吊销尽力而为，本地凭据必清；记住的服务器保留（下次一键选中）。
    ///
    /// # Errors
    /// 钥匙链清不掉时上抛。
    pub async fn sign_out(&self) -> Result<LogoutDto, CloudError> {
        self.forget_sign_in();
        self.logout().await
    }

    /// 把一组钥钉为某台服务器的身份（导入 / 用户确认变更 / 轮换的同一出口）。
    ///
    /// `is_repin` = 这台已经钉过别的钥：承认它是「一台新身份的服务器」，该桶的会话凭据
    /// 一并清掉（拿旧 refresh token 对新身份发请求，正是重钉要防的事）。目录的防回滚水位
    /// 与缓存由调用方重置（两份目录客户端不归这里）。
    ///
    /// # Errors
    /// 凭据清不掉时整体失败、钉住不改。
    pub fn adopt_anchor(
        &self,
        bucket: &str,
        offered: &OfferedAnchor,
        provenance: TrustProvenance,
    ) -> Result<bool, CloudError> {
        let is_repin = self.trust.pinned(bucket).is_some();
        if is_repin {
            self.cred_clear_bucket(bucket)?;
        }
        self.trust.pin(
            bucket,
            PinnedAnchor {
                lease_pubkey: offered.lease_pubkey.clone(),
                catalog_pubkey: offered.catalog_pubkey.clone(),
                pinned_at: crate::catalog::now_unix(),
                provenance,
                deployment_id: offered.deployment_id.clone(),
            },
        );
        self.reconfigure();
        Ok(is_repin)
    }

    /// 用户核对后确认一次信任锚变更（首连之外的换钥、导入撞上既有钉住）。
    ///
    /// 返回 `(这一桶的新状态, 是否重钉)`；重钉时调用方还要重置两份目录的水位与缓存。
    ///
    /// # Errors
    /// 没有待确认的观察 → [`CloudError::Internal`]（界面不该走到这里，但绝不静默成功）。
    pub fn confirm_trust_change(&self) -> Result<(TrustStatusDto, bool), CloudError> {
        let bucket = self
            .trust
            .pending_bucket(&self.server_bucket())
            .ok_or_else(|| CloudError::internal("nothing to confirm"))?;
        let offered = self
            .trust
            .offered(&bucket)
            .ok_or_else(|| CloudError::internal("nothing to confirm"))?;
        let provenance = if self.trust.pending_import(&bucket).is_some() {
            TrustProvenance::Imported
        } else if self.trust.pending_password(&bucket) {
            TrustProvenance::Password
        } else {
            TrustProvenance::Tofu
        };
        let is_repin = self.adopt_anchor(&bucket, &offered, provenance)?;
        Ok((self.trust.status(&bucket), is_repin))
    }

    /// 选定一台服务器（导入组织配置文件之后预选它）：已登录时不动——一次一个账户。
    pub fn preselect_server(&self, server: AccountServer) {
        if self.logged_in() {
            return;
        }
        self.account.set_server(server);
        self.reconfigure();
    }
}

/// 信任包的 TLS 身份能不能被采用：正式构建的官方后端只认 `public`（它用公网证书，送来一张
/// 自签根只可能是出了岔子；口令认证挡住了伪造，这一条是纵深防御，ADR-158 ④）。
fn official_bundle_allowed(strict_official: bool, tls: &BundleTls) -> bool {
    !strict_official || matches!(tls, BundleTls::Public)
}

/// 取公钥失败 → 稳定原因码（与目录 `last_error` 同口径，前端映射 i18n）。
///
/// `server_error` 与 `network` 必须分开：一台**没有** `/v1/trust/keys` 的服务端
/// （旧版本、或反代把这条路挡了）与一台连不上的服务端，下一步动作完全不同。
pub fn trust_error_code(err: &CloudError) -> &'static str {
    match err {
        CloudError::StrictLocalBlocked => "strict_local",
        CloudError::InvalidEndpoint { .. } => "invalid_endpoint",
        CloudError::Network { .. } => "network",
        CloudError::UnreadableResponse { .. } => "malformed",
        CloudError::TrustUnsupportedFormat { .. } => "unsupported_format",
        CloudError::RateLimited { .. } => "rate_limited",
        _ => "server_error",
    }
}

/// 见 [`crate::trust::normalized_pubkey`]（同一份形状判据，不另写一遍）。
fn normalized_pubkey(raw: &str) -> Option<String> {
    crate::trust::normalized_pubkey(raw)
}

fn lease_error_code(err: &LeaseError) -> &'static str {
    match err {
        LeaseError::Malformed => "malformed",
        LeaseError::UnsupportedAlg => "unsupported_alg",
        LeaseError::BadSignature => "bad_signature",
        LeaseError::WrongUse => "wrong_use",
        LeaseError::Expired => "expired",
    }
}

/// 登出时服务端吊销（含 `authed` 里可能的一次 refresh）最多等多久。
///
/// 健康的服务端一个往返就够；超过这个时长只说明它此刻办不成这件事，而本地清除不依赖它
/// ——再等下去，用户看到的只是一颗点了没反应的按钮。
const LOGOUT_REVOKE_DEADLINE: Duration = Duration::from_secs(5);

fn poll_ms(interval: Duration) -> u32 {
    // interval=0（测试配置）也至少 250ms，避免前端热轮询。
    (interval.as_millis() as u32).max(250)
}

/// 非 2xx（且非上文特判的 401/OAuth 流）→ 统一映射 CloudError。
pub(crate) async fn check_status(
    response: reqwest::Response,
) -> Result<reqwest::Response, CloudError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_secs = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u32>().ok());
        return Err(CloudError::RateLimited { retry_after_secs });
    }
    let error_code = response
        .json::<OAuthErrorResponse>()
        .await
        .map(|body| body.error)
        .unwrap_or_else(|_| "unknown".to_owned());
    // 账户被冻结（纵切 3 C3）：推理之外的云请求（同步写入、结账）都从这里出。它不是通用服务端
    // 错误——那一档说的是「服务端出了问题」，而这里服务端好好的，是账户被冻结、该联系支持。
    if status == reqwest::StatusCode::FORBIDDEN && error_code == ACCOUNT_FROZEN {
        return Err(CloudError::AccountFrozen);
    }
    // 撞上用户自设的本月预付上限（纵切 3 C4）：402 有两种，单纯额度用尽照旧是通用服务端错误。
    if status == reqwest::StatusCode::PAYMENT_REQUIRED && error_code == SPEND_CAP_REACHED {
        return Err(CloudError::SpendCapReached);
    }
    // 领不到免费额度（纵切 3 C5）：同样是 402 的一种，成因在权益 `free_quota` 里。
    if status == reqwest::StatusCode::PAYMENT_REQUIRED && error_code == FREE_QUOTA_WITHHELD {
        return Err(CloudError::FreeQuotaWithheld);
    }
    // 注册 / 兑换被拒：一次性邮箱域名（PRD-13-018 L0）。
    if status == reqwest::StatusCode::BAD_REQUEST && error_code == EMAIL_DOMAIN_NOT_ALLOWED {
        return Err(CloudError::EmailDomainNotAllowed);
    }
    Err(CloudError::Server {
        status: status.as_u16(),
        error_code,
    })
}

/// 服务端 403 稳定码：账户被冻结（与 `luna_provider::cloud_llm::ACCOUNT_FROZEN` 逐字一致；
/// 两个 crate 互不依赖，故各持一份字面量，由各自的映射用例钉住）。
pub const ACCOUNT_FROZEN: &str = "account_frozen";

/// 服务端 402 稳定码：本月预付上限已到（与 `luna_provider::cloud_llm::SPEND_CAP_REACHED` 逐字一致；
/// 理由同 [`ACCOUNT_FROZEN`]）。
pub const SPEND_CAP_REACHED: &str = "spend_cap_reached";

/// 服务端 402 稳定码：这个账户此刻领不到免费额度（与 `luna_provider::cloud_llm::FREE_QUOTA_WITHHELD`
/// 逐字一致；理由同 [`ACCOUNT_FROZEN`]）。
pub const FREE_QUOTA_WITHHELD: &str = "free_quota_withheld";

/// 服务端 400 稳定码：注册用的邮箱域名不被接受（一次性邮箱，PRD-13-018 L0）。
pub const EMAIL_DOMAIN_NOT_ALLOWED: &str = "email_domain_not_allowed";

#[cfg(test)]
mod session_scope_tests {
    use super::*;
    use crate::store::MemoryStore;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn client_for(api_base: &str, store: Box<dyn CredentialStore>) -> CloudClient {
        {
            let client = CloudClient::new(store, Arc::new(|| false));
            client.account().set_server(crate::account::AccountServer {
                origin: api_base.to_owned(),
                display_name: None,
            });
            client.reconfigure();
            client
        }
    }

    #[test]
    fn a_partial_token_write_rolls_back_to_no_session() {
        struct FailRefreshStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

        impl CredentialStore for FailRefreshStore {
            fn get(&self, key: &str) -> Result<Option<String>, CloudError> {
                Ok(self.0.lock().unwrap().get(key).cloned())
            }

            fn set(&self, key: &str, value: &str) -> Result<(), CloudError> {
                if key.ends_with(KEY_REFRESH) {
                    return Err(CloudError::Keychain {
                        detail: "refresh write rejected".to_owned(),
                    });
                }
                self.0
                    .lock()
                    .unwrap()
                    .insert(key.to_owned(), value.to_owned());
                Ok(())
            }

            fn delete(&self, key: &str) -> Result<(), CloudError> {
                self.0.lock().unwrap().remove(key);
                Ok(())
            }
        }

        let client = client_for(
            "https://cloud.example",
            Box::new(FailRefreshStore(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            ))),
        );
        let error = client
            .store_token_pair(&TokenResponse {
                access_token: "access".to_owned(),
                refresh_token: "refresh".to_owned(),
            })
            .expect_err("第二个条目写失败必须让整批失败");
        assert!(matches!(error, CloudError::Keychain { .. }));
        assert_eq!(client.cred_get(KEY_ACCESS).unwrap(), None);
        assert_eq!(client.cred_get(KEY_REFRESH).unwrap(), None);
        assert_eq!(client.cred_get(KEY_PROFILE).unwrap(), None);
    }

    #[test]
    fn connectivity_check_reads_one_keychain_entry() {
        type ReadLog = Arc<std::sync::Mutex<Vec<String>>>;

        struct CountingStore {
            inner: MemoryStore,
            reads: ReadLog,
        }

        impl CredentialStore for CountingStore {
            fn get(&self, key: &str) -> Result<Option<String>, CloudError> {
                self.reads.lock().unwrap().push(key.to_owned());
                self.inner.get(key)
            }

            fn set(&self, key: &str, value: &str) -> Result<(), CloudError> {
                self.inner.set(key, value)
            }

            fn delete(&self, key: &str) -> Result<(), CloudError> {
                self.inner.delete(key)
            }
        }

        let reads: ReadLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = client_for(
            "http://a.test",
            Box::new(CountingStore {
                inner: MemoryStore::new(),
                reads: Arc::clone(&reads),
            }),
        );
        client.cred_set(KEY_REFRESH, "refresh").unwrap();
        client
            .cred_set(
                KEY_PROFILE,
                r#"{"account_id":"acc","email":"a@example.test"}"#,
            )
            .unwrap();

        reads.lock().unwrap().clear();
        assert!(client.logged_in());
        assert_eq!(
            reads.lock().unwrap().len(),
            1,
            "logged_in 只该读 refresh 那一条：每多一条就是真机上每次多弹一个钥匙串密码框"
        );

        // 反过来，账户界面要的确实是两条——本用例不是在说「少读总是对的」。
        reads.lock().unwrap().clear();
        assert!(client.logged_in() && client.cached_profile().is_some());
        assert_eq!(reads.lock().unwrap().len(), 2);
    }

    /// **会话凭据必须按服务端分桶**。
    ///
    /// 不分桶时，把服务器从 A 换到 B 之后，账户状态（零网络，只读钥匙链）
    /// 照旧报「已登录」并显示 A 的邮箱，直到第一次真实请求撞上 401 才把人踹出去。
    /// 用户看到的是「明明已登录，改个地址就被登出了，而且刚才那个邮箱是哪来的」。
    /// 热切把这段窗口从"重启之后"提前到"改完立刻"，故必须同批修。
    #[test]
    fn session_is_scoped_per_server() {
        let client = client_for("http://a.test", Box::new(MemoryStore::new()));

        // 在 A 上"登录"（直接写凭据，绕开需要服务端的握手）。
        //
        // **三个条目逐个写、逐个断言**，而不是只测 refresh。上一版只测了 refresh，
        // 于是读档案那一句漏掉分桶（跨行写法躲过了批量替换）
        // 完全没被发现——真机表现是「已登录，但邮箱永远是一个破折号」：会话在、档案没了，
        // 比整个登不上更难查，因为其余一切都正常。
        client.cred_set(KEY_REFRESH, "refresh-for-a").unwrap();
        client.cred_set(KEY_ACCESS, "access-for-a").unwrap();
        client
            .cred_set(
                KEY_PROFILE,
                r#"{"account_id":"acc-a","email":"a@example.test"}"#,
            )
            .unwrap();
        let on_a = (client.logged_in(), client.cached_profile());
        assert!(on_a.0);
        assert_eq!(
            on_a.1.as_ref().map(|p| p.email.as_str()),
            Some("a@example.test"),
            "档案读不回来 = 界面上「已登录但邮箱是破折号」"
        );

        // 换到 B：B 没有会话。
        client.account().set_server(crate::account::AccountServer {
            origin: "http://b.test".to_owned(),
            display_name: None,
        });
        client.reconfigure();
        let on_b = (client.logged_in(), client.cached_profile());
        assert!(
            !on_b.0,
            "B 继承了 A 的会话——界面会显示 A 的账号，直到第一次请求 401 才现形"
        );
        assert!(
            on_b.1.is_none(),
            "B 显示着 A 的邮箱——比「未登录」更糟：它是一句具体而错误的陈述"
        );

        // 切回 A：仍是登录态。分桶的价值不只是隔离，也是"切回去仍然认得你"。
        client.account().set_server(crate::account::AccountServer {
            origin: "http://a.test".to_owned(),
            display_name: None,
        });
        client.reconfigure();
        let back_on_a = (client.logged_in(), client.cached_profile());
        assert!(
            back_on_a.0,
            "切回原服务端后要重新登录一次——分桶写成了「换一次清一次」"
        );
        assert_eq!(
            back_on_a.1.map(|p| p.email),
            Some("a@example.test".to_owned()),
            "会话切回来了、档案没跟着回来——又是那个「已登录但邮箱是破折号」"
        );
    }

    /// 「用 Google 继续」（ADR-171 ④）：进行中的配对改由身份源完成，浏览器直接去 oidc/start，
    /// 「重新打开浏览器」打开的也是它；id 不是 UUID 一律拒（它要拼进地址）。
    #[test]
    fn pending_login_can_be_routed_to_a_provider() {
        let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false));
        client.account().set_server(crate::account::AccountServer {
            origin: "https://api.example.test".to_owned(),
            display_name: None,
        });
        client.reconfigure();
        let provider = "00000000-0000-0000-0000-0000000000b2";
        assert!(matches!(
            client.route_pending_to_provider(provider),
            Err(CloudError::NoPendingLogin)
        ));
        *client.pending.lock().unwrap() = Some(PendingLogin {
            device_code: "dc".to_owned(),
            user_code: "BCDF-GHJK".to_owned(),
            verification_uri_complete:
                "https://api.example.test/v1/device/approve?user_code=BCDF-GHJK".to_owned(),
            interval: Duration::from_secs(5),
            expires_at: Instant::now() + Duration::from_secs(600),
        });
        assert!(matches!(
            client.route_pending_to_provider("../evil?x="),
            Err(CloudError::InvalidEndpoint { .. })
        ));
        let url = client.route_pending_to_provider(provider).unwrap();
        assert_eq!(
            url,
            format!("https://api.example.test/v1/device/oidc/start?provider={provider}&user_code=BCDF-GHJK")
        );
        assert_eq!(client.pending_verification_uri().unwrap(), url);
    }

    /// 严格本地不是 UI 状态，而是控制面所有 HTTP 的字节前硬闸。
    #[tokio::test]
    async fn strict_local_blocks_control_plane_before_network() {
        let client = {
            let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| true));
            client.account().set_server(crate::account::AccountServer {
                origin: "http://127.0.0.1:1".to_owned(),
                display_name: None,
            });
            client.reconfigure();
            client
        };
        assert!(matches!(
            client
                .login_start("device".to_owned(), "test".to_owned())
                .await,
            Err(CloudError::StrictLocalBlocked)
        ));
        assert!(matches!(
            client.fetch_capabilities_at("http://127.0.0.1:1").await,
            Err(CloudError::StrictLocalBlocked)
        ));
    }

    /// 三条鉴权请求同时撞上过期 access 时，只能轮换一次 refresh。
    ///
    /// 服务端把 refresh 设计成一次性凭据；这里若没有 single-flight，第二次提交同一枚旧值
    /// 就会触发「重用即家族全吊销」，不是普通的多发一次请求。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_unauthorized_requests_share_one_refresh_rotation() {
        const CALLS: usize = 3;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let expired_seen = Arc::new(AtomicUsize::new(0));
        let refresh_seen = Arc::new(AtomicUsize::new(0));
        let expired_for_server = Arc::clone(&expired_seen);
        let refresh_for_server = Arc::clone(&refresh_seen);

        let server = std::thread::spawn(move || {
            for _ in 0..(CALLS * 2 + 1) {
                let (mut stream, _) = listener.accept().unwrap();
                let expired_seen = Arc::clone(&expired_for_server);
                let refresh_seen = Arc::clone(&refresh_for_server);
                std::thread::spawn(move || {
                    let mut buffer = [0_u8; 4096];
                    let size = stream.read(&mut buffer).unwrap();
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let request_lower = request.to_ascii_lowercase();
                    let (status, body) = if request.starts_with("POST /v1/auth/refresh ") {
                        refresh_seen.fetch_add(1, Ordering::SeqCst);
                        (
                            "200 OK",
                            r#"{"access_token":"fresh-access","refresh_token":"fresh-refresh"}"#,
                        )
                    } else if request_lower.contains("authorization: bearer expired-access") {
                        expired_seen.fetch_add(1, Ordering::SeqCst);
                        while expired_seen.load(Ordering::SeqCst) < CALLS {
                            std::thread::yield_now();
                        }
                        ("401 Unauthorized", r#"{"error":"expired"}"#)
                    } else {
                        ("200 OK", "[]")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                });
            }
        });

        let client = Arc::new(client_for(
            &format!("http://{address}"),
            Box::new(MemoryStore::new()),
        ));
        client.cred_set(KEY_ACCESS, "expired-access").unwrap();
        client.cred_set(KEY_REFRESH, "old-refresh").unwrap();

        let mut tasks = Vec::new();
        for _ in 0..CALLS {
            let client = Arc::clone(&client);
            tasks.push(tokio::spawn(async move { client.devices().await }));
        }
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
        server.join().unwrap();

        assert_eq!(expired_seen.load(Ordering::SeqCst), CALLS);
        assert_eq!(
            refresh_seen.load(Ordering::SeqCst),
            1,
            "并发 401 轮换了不止一次 refresh——线上会触发令牌家族吊销"
        );
        assert_eq!(
            client.cred_get(KEY_REFRESH).unwrap().as_deref(),
            Some("fresh-refresh")
        );
    }

    /// 起一台只回一份固定 200 JSON 的假服务端；**join 交出「有没有真的被访问过」**。
    ///
    /// **accept 必须带死线**：客户端在字节前就返回错误时（`authed` 少一枚凭据即如此），
    /// 无死线的 `accept` 会让 `join` 永远挂住——而挂死在 CI 上的表现是「跑了一小时、
    /// 零输出、没有任何一条失败断言」，比任何失败都难查。这条不是防御性编程：本 helper
    /// 的头两个使用者就同时踩了它，其中一个还因为 `expect_err` 接受任意错误而**假通过**。
    /// 故死线到期即收摊，把「是否被访问」变成调用方必须断言的返回值。
    fn serve_json_once(body: &'static str) -> (String, std::thread::JoinHandle<bool>) {
        const DEADLINE: Duration = Duration::from_secs(5);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + DEADLINE;
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // accept 出来的 socket 在 BSD 上会继承监听套接字的非阻塞位，
                        // 这里显式关掉，读写照常按阻塞语义走。
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(DEADLINE)).unwrap();
                        let mut buffer = [0_u8; 4096];
                        let _ = stream.read(&mut buffer).unwrap();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                             Connection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).unwrap();
                        return true;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("假服务端 accept 失败：{err}"),
                }
            }
        });
        (format!("http://{address}"), handle)
    }

    /// 起一个按路径作答的假服务端（登录前的两次匿名请求：取公钥、取 capabilities）。
    /// 未登记的路径回 404。线程随测试进程结束。
    fn serve_paths(routes: Vec<(&'static str, String)>) -> String {
        serve_answers(
            routes
                .into_iter()
                .map(|(path, body)| (path, 200, body))
                .collect(),
        )
    }

    /// 同 [`serve_paths`]，但每条路径带自己的状态码（收单端点的 404 / 503 各有含义）。
    fn serve_answers(routes: Vec<(&'static str, u16, String)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let response = match routes.iter().find(|(route, _, _)| *route == path) {
                    Some((_, status, body)) => format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned(),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}")
    }

    /// 合法的 Ed25519 公钥（全零字节不是合法点，故取一把真实生成的钥）。
    fn real_pubkey(seed: u8) -> String {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        signing
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn trust_keys_body(lease: &str, catalog: &str) -> String {
        format!(
            r#"{{"format_version":1,"deployment_id":"dep-1","lease_pubkey":"{lease}","catalog_pubkey":"{catalog}"}}"#
        )
    }

    fn capabilities_body(protocol: u32) -> String {
        capabilities_with_billing(protocol, false)
    }

    fn capabilities_with_billing(protocol: u32, billing: bool) -> String {
        format!(
            r#"{{"protocol":{protocol},"min_client_protocol":9,"status":"ready","capabilities":["chat","embed","transcribe"],"deployment":{{"id":"dep-1","display_name":"Corp"}},"auth":{{"login":{{"password":true,"browser":false,"providers":[{{"id":"00000000-0000-0000-0000-0000000000a1","name":"Okta","kind":"org"}}],"admission":"invite","password_reset":true}}}},"billing":{{"enabled":{billing}}}}}"#
        )
    }

    /// v9 服务端的真实形状：没有 `billing`。
    fn capabilities_v9_body() -> String {
        r#"{"protocol":9,"min_client_protocol":9,"deployment":{"id":"dep-1","display_name":"Corp"},"auth":{"login":{"password":true,"browser":false,"providers":[],"admission":"invite","password_reset":true}}}"#.to_owned()
    }

    const PLANS_BODY: &str = r#"{"subscriptions":[{"id":"p-pro","tier":"pro","display_name":"Pro",
        "price":{"unit_amount":999,"currency":"usd"},"interval":"month",
        "included":{"period":"monthly","dimensions":[{"dimension":"tokens_out","limit":2000000}]}}],
        "prepaid_packs":[{"id":"p-pack","display_name":"Pack","price":{"unit_amount":500,"currency":"usd"},
        "grant_micro_credits":600000000}],
        "billing_rules":{"ledger_order":["subscription","prepaid"],"splits_across_ledgers":false,
        "prepaid_credits_expire":false,
        "minimum_billable_units":[{"dimension":"asr_seconds","unit":60}]}}"#;

    /// v9 服务端缺本客户端必读的 `billing`：必须说「服务器太旧」，而不是「读不懂的响应」——
    /// 先整份解析再协商的话，缺字段会抢在协商之前失败，界面就指向了错误的一侧。
    #[tokio::test]
    async fn a_v9_server_without_billing_is_too_old_not_unreadable() {
        let base = serve_paths(vec![
            (
                "/v1/trust/keys",
                trust_keys_body(&real_pubkey(1), &real_pubkey(2)),
            ),
            ("/inference/v1/capabilities", capabilities_v9_body()),
        ]);
        let err = signed_out_client()
            .sign_in_begin(SignInTarget::Server {
                email: "a@b.test".into(),
                input: base,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, CloudError::ServerTooOld { server_protocol: 9 }),
            "得到 {err:?}"
        );
    }

    /// 私有化（未启用收单）：答案是 `Disabled`，**不去取价目**——价目路径没登记，取了就是 404 错误。
    #[tokio::test]
    async fn billing_disabled_is_an_answer_and_skips_the_price_list() {
        let base = serve_paths(vec![(
            "/inference/v1/capabilities",
            capabilities_with_billing(protocol::CLIENT_PROTOCOL, false),
        )]);
        let offer = signed_in_client(&base)
            .billing_offer(async { panic!("不卖就不该去取档位") })
            .await
            .unwrap();
        assert!(matches!(offer, BillingOfferDto::Disabled));
    }

    #[tokio::test]
    async fn billing_enabled_returns_plans_and_rules() {
        let base = serve_paths(vec![
            (
                "/inference/v1/capabilities",
                capabilities_with_billing(protocol::CLIENT_PROTOCOL, true),
            ),
            ("/v1/billing/plans", PLANS_BODY.to_owned()),
        ]);
        let client = signed_in_client(&base);
        let BillingOfferDto::Enabled {
            plans,
            standing,
            payment,
            purchasable,
        } = client.billing_offer(async { None }).await.unwrap()
        else {
            panic!("收单已启用，应当拿到价目");
        };
        assert_eq!(payment, None, "权益取不到：付款处境不知道，不出横幅");
        assert!(purchasable, "不知道时照常可买，服务端拒了再说明");
        // 档位取不到：处境如实为「不知道」，价目照常给出。
        assert_eq!(
            standing,
            crate::billing::SubscriptionStandingDto::Unknown,
            "取不到档位不能让整节失败，也不能猜成「未订阅」"
        );
        assert_eq!(plans.subscriptions[0].id, "p-pro");
        assert_eq!(plans.prepaid_packs[0].credits, 600.0);
        assert_eq!(plans.rules.minimum_billable_units[0].unit, 60.0);
        assert!(
            client.billing_enabled().await.unwrap(),
            "取价目时已记下「卖」，失败卡片不必再问"
        );
    }

    /// 订阅处境按调用方给的权益 `tier` 由 Rust 判定：在售档上 = 已订阅，并标出那一档。
    #[tokio::test]
    async fn billing_offer_marks_the_current_plan_from_entitlements() {
        let base = serve_paths(vec![
            (
                "/inference/v1/capabilities",
                capabilities_with_billing(protocol::CLIENT_PROTOCOL, true),
            ),
            ("/v1/billing/plans", PLANS_BODY.to_owned()),
        ]);
        let BillingOfferDto::Enabled {
            plans,
            standing,
            payment,
            purchasable,
        } = signed_in_client(&base)
            .billing_offer(async { Some(entitlements_for("pro", BillingStandingDto::Good)) })
            .await
            .unwrap()
        else {
            panic!("收单已启用，应当拿到价目");
        };
        assert_eq!(
            standing,
            crate::billing::SubscriptionStandingDto::Subscribed
        );
        assert!(plans.subscriptions[0].current);
        assert_eq!(payment, Some(BillingStandingDto::Good));
        assert!(purchasable);
    }

    fn entitlements_for(tier: &str, billing_standing: BillingStandingDto) -> EntitlementsDto {
        EntitlementsDto {
            tier: tier.to_owned(),
            entitlements: Vec::new(),
            lease_verified: false,
            lease_expires_at: None,
            lease_error: None,
            capabilities: None,
            billing_standing,
            free_quota: FreeQuotaDto::NotApplicable,
        }
    }

    /// 付款处境由 Rust 带进收单答案（纵切 3 C3）：催缴照常可买；冻结时不可下单——服务端那时
    /// 对结账回 403，界面不该摆一排点了就失败的按钮。
    #[tokio::test]
    async fn billing_offer_carries_the_payment_standing_and_a_frozen_account_cannot_buy() {
        let base = serve_paths(vec![
            (
                "/inference/v1/capabilities",
                capabilities_with_billing(protocol::CLIENT_PROTOCOL, true),
            ),
            ("/v1/billing/plans", PLANS_BODY.to_owned()),
        ]);
        let client = signed_in_client(&base);
        let past_due = BillingStandingDto::PastDue {
            grace_ends_at: "2026-10-28T00:00:00Z".into(),
        };
        for (standing, can_buy) in [(past_due, true), (BillingStandingDto::Frozen, false)] {
            let ents = entitlements_for("pro", standing.clone());
            let BillingOfferDto::Enabled {
                payment,
                purchasable,
                ..
            } = client.billing_offer(async { Some(ents) }).await.unwrap()
            else {
                panic!("收单已启用，应当拿到价目");
            };
            assert_eq!(payment, Some(standing));
            assert_eq!(purchasable, can_buy);
        }
    }

    /// 冻结的账户结账：服务端 403 `account_frozen` → `AccountFrozen`，不是「这一项已不在售」，
    /// 也不是通用服务端错误。
    #[tokio::test]
    async fn checkout_on_a_frozen_account_says_frozen() {
        let base = serve_answers(vec![(
            "/v1/billing/checkout",
            403,
            r#"{"error":"account_frozen"}"#.to_owned(),
        )]);
        assert!(matches!(
            signed_in_client(&base).billing_checkout_url("p-pro").await,
            Err(CloudError::AccountFrozen)
        ));
        // 别的 403 照旧是通用服务端错误：只认这一个稳定码。
        let other = serve_answers(vec![(
            "/v1/billing/checkout",
            403,
            r#"{"error":"forbidden"}"#.to_owned(),
        )]);
        assert!(matches!(
            signed_in_client(&other).billing_checkout_url("p-pro").await,
            Err(CloudError::Server { status: 403, .. })
        ));
    }

    /// capabilities 说卖、价目却说 `billing_disabled`（两次请求之间运营关了收单）：按价目那句算。
    #[tokio::test]
    async fn a_price_list_that_says_disabled_wins() {
        let base = serve_answers(vec![
            (
                "/inference/v1/capabilities",
                200,
                capabilities_with_billing(protocol::CLIENT_PROTOCOL, true),
            ),
            (
                "/v1/billing/plans",
                503,
                r#"{"error":"billing_disabled"}"#.to_owned(),
            ),
        ]);
        let client = signed_in_client(&base);
        assert!(matches!(
            client.billing_offer(async { None }).await.unwrap(),
            BillingOfferDto::Disabled
        ));
        assert!(!client.billing_enabled().await.unwrap());
    }

    /// 记得就不再发请求：第二次问的时候假服务端已经收摊，真发了就是网络错误。
    #[tokio::test]
    async fn billing_availability_is_remembered_per_server() {
        let (base, server) = serve_json_once(Box::leak(
            capabilities_with_billing(protocol::CLIENT_PROTOCOL, true).into_boxed_str(),
        ));
        let client = signed_in_client(&base);
        assert!(client.billing_enabled().await.unwrap());
        assert!(server.join().unwrap(), "第一次必须真的去问");
        assert!(
            client.billing_enabled().await.unwrap(),
            "第二次应当直接用记下的答案"
        );
    }

    /// 未登录时收单命令一个字节都不发（地址指向没人听的端口：真发了会是网络错误）。
    #[tokio::test]
    async fn billing_needs_a_session_before_any_byte() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let client = client_for(&base, Box::new(MemoryStore::new()));
        assert!(matches!(
            client.billing_offer(async { None }).await,
            Err(CloudError::NotLoggedIn)
        ));
        assert!(matches!(
            client.billing_enabled().await,
            Err(CloudError::NotLoggedIn)
        ));
        assert!(matches!(
            client.billing_checkout_url("p-pro").await,
            Err(CloudError::NotLoggedIn)
        ));
    }

    /// 严格本地：收单与其余控制面同一道硬闸，在发请求前拦下。
    #[tokio::test]
    async fn billing_is_blocked_by_strict_local() {
        let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| true));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        client.cred_set(KEY_REFRESH, "refresh").unwrap();
        assert!(matches!(
            client.billing_offer(async { None }).await,
            Err(CloudError::StrictLocalBlocked)
        ));
        assert!(matches!(
            client.billing_portal_url().await,
            Err(CloudError::StrictLocalBlocked)
        ));
    }

    #[tokio::test]
    async fn checkout_returns_the_hosted_page_and_a_stale_plan_is_named() {
        let base = serve_paths(vec![(
            "/v1/billing/checkout",
            r#"{"url":"https://checkout.stripe.com/c/pay/cs_test_1"}"#.to_owned(),
        )]);
        let client = signed_in_client(&base);
        assert_eq!(
            client.billing_checkout_url("p-pro").await.unwrap(),
            "https://checkout.stripe.com/c/pay/cs_test_1"
        );

        let stale = serve_answers(vec![(
            "/v1/billing/checkout",
            404,
            r#"{"error":"not_found"}"#.to_owned(),
        )]);
        assert!(matches!(
            signed_in_client(&stale).billing_checkout_url("gone").await,
            Err(CloudError::PlanUnavailable)
        ));
    }

    /// 从没付过款就没有账单管理页可开：如实说「还没有可管理的订阅」，而不是「这个地址上没有
    /// Lunarca 服务端」（通用 404 的说法）。
    #[tokio::test]
    async fn portal_without_a_stripe_customer_is_no_billing_account() {
        let base = serve_answers(vec![(
            "/v1/billing/portal",
            404,
            r#"{"error":"not_found"}"#.to_owned(),
        )]);
        assert!(matches!(
            signed_in_client(&base).billing_portal_url().await,
            Err(CloudError::NoBillingAccount)
        ));
        let disabled = serve_answers(vec![(
            "/v1/billing/portal",
            503,
            r#"{"error":"billing_disabled"}"#.to_owned(),
        )]);
        assert!(matches!(
            signed_in_client(&disabled).billing_portal_url().await,
            Err(CloudError::BillingDisabled)
        ));
    }

    fn signed_out_client() -> CloudClient {
        CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false))
    }

    /// 「使用其他服务器」填地址：经可信 TLS 连得上就钉住它自报的两钥（TOFU，浏览器 / 单点登录那一路
    /// 靠它）、选定并记住这台、按 capabilities 给出登录方式。
    #[tokio::test]
    async fn signing_in_at_an_address_pins_selects_and_reports_methods() {
        let (lease, catalog) = (real_pubkey(1), real_pubkey(2));
        let base = serve_paths(vec![
            ("/v1/trust/keys", trust_keys_body(&lease, &catalog)),
            (
                "/inference/v1/capabilities",
                capabilities_body(protocol::CLIENT_PROTOCOL),
            ),
        ]);
        let client = signed_out_client();
        let step = client
            .sign_in_begin(SignInTarget::Server {
                email: "a@b.test".into(),
                input: base.clone(),
            })
            .await
            .expect("登录第一步应成功");
        let SignInStepDto::Ready {
            server,
            methods,
            invite,
            ..
        } = step
        else {
            panic!("首连不该要求确认变更");
        };
        let methods = methods.expect("可信 TLS 连得上就有登录方式");
        assert_eq!(server.origin, base);
        assert_eq!(server.display_name.as_deref(), Some("Corp"));
        assert!(methods.password && !methods.browser);
        assert_eq!(
            methods.providers,
            vec![SignInProviderDto {
                id: "00000000-0000-0000-0000-0000000000a1".to_owned(),
                name: "Okta".to_owned(),
                kind: crate::protocol::LoginProviderKind::Org,
            }]
        );
        assert!(
            methods.password_reset,
            "忘记密码能否自助完成随 capabilities 走"
        );
        assert!(!invite);
        // 选定即记住，配置随之派生：地址与钉住的两把钥一起换。
        assert_eq!(client.config().api_base, base);
        assert!(client.config().catalog_pubkey.is_some());
        let bucket = crate::config::server_bucket(&base);
        assert_eq!(
            client.trust().pinned(&bucket).map(|p| p.provenance),
            Some(TrustProvenance::Tofu)
        );
    }

    /// 已钉住的服务器换了钥：不切服务器、不改钉住，交给用户核对确认。
    #[tokio::test]
    async fn a_changed_identity_needs_confirmation_before_anything_switches() {
        let base = serve_paths(vec![
            (
                "/v1/trust/keys",
                trust_keys_body(&real_pubkey(3), &real_pubkey(4)),
            ),
            (
                "/inference/v1/capabilities",
                capabilities_body(protocol::CLIENT_PROTOCOL),
            ),
        ]);
        let client = signed_out_client();
        let bucket = crate::config::server_bucket(&base);
        client.trust().pin(
            &bucket,
            PinnedAnchor {
                lease_pubkey: real_pubkey(1),
                catalog_pubkey: real_pubkey(2),
                pinned_at: 0,
                provenance: TrustProvenance::Tofu,
                deployment_id: "dep-1".into(),
            },
        );
        let step = client
            .sign_in_begin(SignInTarget::Server {
                email: "a@b.test".into(),
                input: base.clone(),
            })
            .await
            .unwrap();
        assert!(matches!(step, SignInStepDto::NeedsTrustConfirmation { .. }));
        assert_eq!(client.account().server(), None, "身份没确认就先切了服务器");
        assert_eq!(
            client.trust().pinned(&bucket).map(|p| p.lease_pubkey),
            Some(real_pubkey(1))
        );
    }

    /// 服务器协议比本客户端旧：不进入登录，如实说「服务器太旧」。
    #[tokio::test]
    async fn an_older_server_is_refused_before_sign_in() {
        let base = serve_paths(vec![
            (
                "/v1/trust/keys",
                trust_keys_body(&real_pubkey(1), &real_pubkey(2)),
            ),
            ("/inference/v1/capabilities", capabilities_body(8)),
        ]);
        let client = signed_out_client();
        let err = client
            .sign_in_begin(SignInTarget::Server {
                email: "a@b.test".into(),
                input: base,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CloudError::ServerTooOld { server_protocol: 8 }
        ));
        assert_eq!(client.account().server(), None);
    }

    /// 一次一个账户：已登录时不能开始另一次登录。
    #[tokio::test]
    async fn signing_in_again_while_signed_in_is_refused() {
        let client = signed_in_client("http://127.0.0.1:1");
        let err = client
            .sign_in_begin(SignInTarget::Discover {
                email: "a@b.test".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, CloudError::AlreadySignedIn));
    }

    /// 起一个只接一次连接的假服务端：读完请求后按 `respond` 回写（`None` = 永不回答）。
    fn serve_raw_once(respond: Option<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            match respond {
                Some(response) => {
                    let _ = stream.write_all(response.as_bytes());
                }
                // 连接挂住不回：模拟服务端卡在数据库上。持有 stream 直到测试进程结束。
                None => std::thread::sleep(Duration::from_secs(60)),
            }
        });
        format!("http://{address}")
    }

    fn signed_in_client(base: &str) -> CloudClient {
        let client = client_for(base, Box::new(MemoryStore::new()));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        client.cred_set(KEY_REFRESH, "refresh").unwrap();
        client
    }

    /// **服务端出错也必须登出本地**（2026-09-27 真机：部署机数据库盘满，吊销请求回 500，
    /// 「退出登录」点了仍是登录态）。登出只取决于本地凭据；服务端没吊销成要如实说出来，
    /// 但不能拦住登出本身。
    #[tokio::test]
    async fn logout_clears_the_local_session_even_when_the_server_errors() {
        let base = serve_raw_once(Some(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\
             Content-Length: 18\r\nConnection: close\r\n\r\n{\"error\":\"internal\"}",
        ));
        let client = signed_in_client(&base);

        let result = client
            .logout()
            .await
            .expect("服务端 500 不是登出失败的理由");
        assert!(!result.server_revoked, "服务端回了 500，却报告已吊销");
        assert!(
            !client.logged_in(),
            "登出返回了，本地却仍是登录态——用户点了「退出登录」什么都没发生"
        );
    }

    /// 可用的 AI 能力（R4b）：取权益时记下，退出即忘——下一个账户的组织策略可能不同。
    #[tokio::test]
    async fn allowed_capabilities_follow_entitlements_and_are_forgotten_on_logout() {
        let body = serde_json::json!({
            "tier": "free",
            "entitlements": [
                { "key": "devices.max", "value": 2 },
                { "key": "inference.capabilities", "value": ["chat", "transcribe"] },
                { "key": "sync.max_file_bytes", "value": 2_147_483_648_u64 },
            ],
            "lease": "",
            "lease_ttl_secs": 0,
            "billing_standing": { "state": "past_due", "grace_ends_at": "2026-10-28T00:00:00Z" },
            "free_quota": { "state": "withheld", "reason": "shared_device" },
        })
        .to_string();
        let base = serve_paths(vec![
            ("/v1/entitlements", body),
            ("/v1/auth/logout", "{}".to_owned()),
        ]);
        let client = signed_in_client(&base);
        assert_eq!(client.allowed_capabilities(), None, "没取过 = 不知道");
        assert!(
            client.capability_allowed("translate"),
            "不知道时按全部提供处理"
        );

        let dto = client.entitlements().await.expect("取权益");
        let expected = Some(vec!["chat".to_owned(), "transcribe".to_owned()]);
        assert_eq!(dto.capabilities, expected);
        assert_eq!(client.allowed_capabilities(), expected);
        assert!(client.capability_allowed("chat"));
        assert!(!client.capability_allowed("translate"));
        assert_eq!(client.sync_max_file_bytes(), Some(2_147_483_648));
        let past_due = BillingStandingDto::PastDue {
            grace_ends_at: "2026-10-28T00:00:00Z".into(),
        };
        assert_eq!(dto.billing_standing, past_due);
        assert_eq!(
            dto.free_quota,
            FreeQuotaDto::Withheld {
                reason: crate::billing::FreeQuotaWithheldReasonDto::SharedDevice
            }
        );
        assert_eq!(client.billing_standing(), Some(past_due));

        client.logout().await.expect("登出");
        assert_eq!(client.billing_standing(), None, "付款处境随账户一起忘掉");
        assert_eq!(
            client.sync_max_file_bytes(),
            None,
            "单文件上限随账户一起忘掉"
        );
        assert_eq!(
            client.allowed_capabilities(),
            None,
            "退出后不该带着上一个账户的清单"
        );
    }

    /// 付款处境变了才通知（纵切 3 C3）：`fetch_entitlements` 是唯一判定处；同一处境重取不再通知，
    /// 换了处境（含从「不知道」到第一次取到）通知一次 `BillingStanding`，不触发重装。
    #[tokio::test]
    async fn fetch_entitlements_notifies_only_when_the_billing_standing_changes() {
        let body = serde_json::json!({
            "tier": "pro",
            "entitlements": [],
            "lease": "",
            "billing_standing": { "state": "frozen" },
            "free_quota": { "state": "not_applicable" },
        })
        .to_string();
        let base = serve_paths(vec![("/v1/entitlements", body)]);
        let client = signed_in_client(&base);
        let seen: Arc<Mutex<Vec<crate::account::AccountChange>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let observer = crate::commands::AccountObserverState(Arc::new(move |change| {
            sink.lock().unwrap().push(change);
        }));

        crate::commands::fetch_entitlements(&client, &observer)
            .await
            .unwrap();
        crate::commands::fetch_entitlements(&client, &observer)
            .await
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![crate::account::AccountChange::BillingStanding],
            "第一次取到通知一次，同一处境再取不再通知，且不重装"
        );

        // 处境在别处变了（例如上一次取到的是 good）：再取到 frozen 又是一次变化。
        client.set_billing_standing(Some(BillingStandingDto::Good));
        crate::commands::fetch_entitlements(&client, &observer)
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(client.billing_standing(), Some(BillingStandingDto::Frozen));
    }

    /// 部署事实随权益一起重取（ADR-041 ①）：第一次知道「这台服务端没配上游」通知一次重装，
    /// 路由据此改走本机；同样的事实再取不再通知。
    #[tokio::test]
    async fn fetch_entitlements_learns_an_unconfigured_deployment_once() {
        let entitlements = serde_json::json!({
            "tier": "free",
            "entitlements": [],
            "lease": "",
            "billing_standing": { "state": "good" },
            "free_quota": { "state": "not_applicable" },
        })
        .to_string();
        let capabilities = serde_json::json!({
            "protocol": crate::protocol::CLIENT_PROTOCOL,
            "min_client_protocol": crate::protocol::CLIENT_PROTOCOL,
            "status": "setup_incomplete",
            "capabilities": [],
            "deployment": { "id": "dep-1", "display_name": null },
            "auth": { "login": { "password": true, "browser": false, "providers": [],
                                  "admission": "invite", "password_reset": false } },
            "billing": { "enabled": false },
        })
        .to_string();
        let base = serve_paths(vec![
            ("/v1/entitlements", entitlements),
            ("/inference/v1/capabilities", capabilities),
        ]);
        let client = signed_in_client(&base);
        let seen: Arc<Mutex<Vec<crate::account::AccountChange>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let observer = crate::commands::AccountObserverState(Arc::new(move |change| {
            sink.lock().unwrap().push(change);
        }));

        crate::commands::fetch_entitlements(&client, &observer)
            .await
            .unwrap();
        let facts = client.deployment_facts().expect("取权益时一并记下部署事实");
        assert!(facts.setup_incomplete);
        assert!(!facts.serves("chat"));
        let sessions = |seen: &[crate::account::AccountChange]| {
            seen.iter()
                .filter(|c| **c == crate::account::AccountChange::Session)
                .count()
        };
        assert_eq!(
            sessions(&seen.lock().unwrap()),
            1,
            "从不知道到知道没配上游：重装一次"
        );

        crate::commands::fetch_entitlements(&client, &observer)
            .await
            .unwrap();
        assert_eq!(sessions(&seen.lock().unwrap()), 1, "事实没变不再重装");
    }

    /// **服务端卡住时登出不能跟着卡住**：吊销有时限，过时即按「未吊销」处理并清本地。
    #[tokio::test]
    async fn logout_does_not_wait_on_a_hung_server() {
        let base = serve_raw_once(None);
        let client = signed_in_client(&base);

        let result = tokio::time::timeout(
            LOGOUT_REVOKE_DEADLINE + Duration::from_secs(3),
            client.logout(),
        )
        .await
        .expect("登出被卡住的服务端拖过了时限——按钮在用户那里就是「点了没反应」")
        .expect("超时不是登出失败的理由");
        assert!(!result.server_revoked);
        assert!(!client.logged_in());
    }

    /// 出站字节这一维会超过 u32——默认策略的上限就是 5 GiB。
    ///
    /// 载荷逐字取自部署机（2026-08-10）。字段此前是 `u32`，于是这份**完全正常**的响应
    /// 反序列化失败，而失败被报成「连不上这台服务端」：用户按那句话去查网络，
    /// 而同一页的权益、租约、设备刚从同一台机器拉回来。
    #[tokio::test]
    async fn usage_survives_an_egress_limit_beyond_u32() {
        const FIVE_GIB: f64 = 5_368_709_120.0;
        let (base, server) = serve_json_once(
            r#"{"ledgers":[{"kind":"subscription","period":"monthly","alert_key":"2026-08-01T00:00:00Z","dimensions":[
                {"dimension":"tokens_in","limit":1000000,"used":1881,"alert":"none"},
                {"dimension":"storage_byte_hours","limit":null,"used":0,"alert":"none"},
                {"dimension":"egress_bytes","limit":5368709120,"used":1024591,"alert":"none"}]}],
                "spend_cap":{"state":"unset","spent_micro_credits":0,"period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z"}}"#,
        );
        let client = client_for(&base, Box::new(MemoryStore::new()));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        // refresh 也必须写：`authed` 少了它会在发出**任何字节之前**返回 `NotLoggedIn`，
        // 于是这个测试测的就不是解码，而是它自己的前置条件没配齐。
        client.cred_set(KEY_REFRESH, "refresh").unwrap();

        let ledgers = client
            .usage()
            .await
            .expect("这份响应完全合法，必须解得动")
            .ledgers;
        assert!(
            server.join().unwrap(),
            "假服务端从未被访问——请求在客户端内部就失败了，这份载荷根本没被解析过"
        );

        let egress = ledgers[0]
            .dimensions
            .iter()
            .find(|d| d.dimension == "egress_bytes")
            .expect("出站维度");
        assert_eq!(egress.limit, Some(FIVE_GIB), "5 GiB 上限被截断或丢失");
        assert_eq!(egress.used, 1_024_591.0);
        let storage = ledgers[0]
            .dimensions
            .iter()
            .find(|d| d.dimension == "storage_byte_hours")
            .expect("存储维度");
        assert_eq!(
            storage.limit, None,
            "「不限」必须仍是 None，而不是 0——0 会被画成一根满进度条"
        );
        // 顺手钉住 ADR-121 ⑥ 走完整条命令：无上限的沉到最后，不占「第一行就是答案」那个位置。
        assert_eq!(
            ledgers[0].dimensions.last().map(|d| d.dimension.as_str()),
            Some("storage_byte_hours")
        );
    }

    /// 读不懂的响应体**不是**网络故障。
    ///
    /// 两者的排查方向完全相反：一个去看网线与服务器，一个去看两侧版本与类型。
    /// 这条纪律此前只写在 `Network` 的文档里（"必须说出打的是哪一台"），
    /// 而 `.json()` 的失败一律走了那条路——于是类型不一致永远伪装成网络问题。
    #[tokio::test]
    async fn an_unreadable_body_is_not_reported_as_a_network_failure() {
        // 形状对不上：`used` 是字符串。服务端答了 200，只是我们读不懂。
        let (base, server) = serve_json_once(
            r#"{"ledgers":[{"kind":"subscription","period":"monthly","alert_key":"unstarted",
                "dimensions":[{"dimension":"tokens_in","limit":1,"used":"很多","alert":"none"}]}],
                "spend_cap":{"state":"unset","spent_micro_credits":0,"period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z"}}"#,
        );
        let client = client_for(&base, Box::new(MemoryStore::new()));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        // 见上一条：少了 refresh，`expect_err` 会被 `NotLoggedIn` 满足，
        // 这个测试便在「请求从未发出」的情况下绿着——它要验的分类一次都没跑到。
        client.cred_set(KEY_REFRESH, "refresh").unwrap();

        let err = client.usage().await.expect_err("这份响应读不懂");
        assert!(
            server.join().unwrap(),
            "假服务端从未被访问——错误来自客户端内部，而不是这份读不懂的响应"
        );

        assert!(
            matches!(err, CloudError::UnreadableResponse { .. }),
            "读不懂的响应被归成了 {err:?}——界面会让用户去查网络，而问题在两侧的类型上"
        );
    }

    /// **双账本是列表契约，不是"订阅那一本外加可选的预付字段"**（PRD-13-038 / ADR-046）。
    ///
    /// 此前两条用例的载荷都只有一个 `subscription` 元素，于是「多本账」这件事在 Rust 侧
    /// 一次都没被解过：把 `ledgers` 读成"取第一个"、或把 `kind`/`period` 漏进
    /// `#[serde(default)]`，两种写法都能让那两条继续全绿，而账户页只会显示半张账单——
    /// 少的那一本没有任何提示，看起来就像"这个月没用预付额度"。
    #[tokio::test]
    async fn usage_decodes_both_ledgers_not_just_the_first() {
        let (base, server) = serve_json_once(
            r#"{"ledgers":[
                {"kind":"subscription","period":"monthly","alert_key":"2026-08-01T00:00:00Z","dimensions":[
                    {"dimension":"tokens_in","limit":1000000,"used":1881,"alert":"none"}]},
                {"kind":"prepaid","period":"none","alert_key":"500000","dimensions":[
                    {"dimension":"tokens_out","limit":500000,"used":0,"alert":"none"},
                    {"dimension":"asr_seconds","limit":null,"used":804,"alert":"none"}]}],
                "spend_cap":{"state":"unset","spent_micro_credits":0,"period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z"}}"#,
        );
        let client = client_for(&base, Box::new(MemoryStore::new()));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        client.cred_set(KEY_REFRESH, "refresh").unwrap();

        let ledgers = client.usage().await.expect("两本账的载荷完全合法").ledgers;
        assert!(server.join().unwrap(), "假服务端从未被访问");

        let kinds: Vec<&str> = ledgers.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["subscription", "prepaid"],
            "只解出 {kinds:?} —— 账户页会少显示一整本账，而缺的那本不会报错"
        );
        let prepaid = &ledgers[1];
        assert_eq!(
            prepaid.period, "none",
            "预付账本的 `period` 恒为 `none` 且恒在场——缺省成空串会让界面渲染一个没有周期的周期行"
        );
        assert_eq!(prepaid.dimensions.len(), 2);
        assert_eq!(
            prepaid.dimensions[1].limit, None,
            "「不限」必须仍是 None，而不是 0"
        );
    }

    /// **空列表是「还没用过」，不是失败**——这条规则写在 `usage()` 的文档里，却从未被验过。
    ///
    /// 反过来写（空列表当错误、或把它读成 `None`）不会让任何现有用例变红，而前端据此
    /// 分辨「取不到」与「真的没用过」：那正是 2026-08-09 验收抓到的第三处同型缺陷。
    #[tokio::test]
    async fn an_empty_ledger_list_is_a_fact_not_a_failure() {
        let (base, server) = serve_json_once(
            r#"{"ledgers":[],"spend_cap":{"state":"unset","spent_micro_credits":0,"period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z"}}"#,
        );
        let client = client_for(&base, Box::new(MemoryStore::new()));
        client.cred_set(KEY_ACCESS, "access").unwrap();
        client.cred_set(KEY_REFRESH, "refresh").unwrap();

        let usage = client
            .usage()
            .await
            .expect("空列表是合法答复：这个账户还没用过服务端算力");
        assert!(server.join().unwrap(), "假服务端从未被访问");
        assert!(usage.ledgers.is_empty());
        assert_eq!(
            usage.spend_cap, None,
            "没有预付也没设上限：上限那一块不出现"
        );
        let episodes = client
            .usage_alerts()
            .expect("取过一次就记下了这一次的预警轮次");
        assert_eq!(episodes.len(), 1, "只有月度上限那一轮");
        assert_eq!(episodes[0].id, "spend_cap@2026-10");
    }

    /// 月度上限与任务前预估（纵切 3 C4）：两条端点往返、换算成 credit / 分钟。
    #[tokio::test]
    async fn spend_cap_and_estimate_round_trip_in_credits() {
        let base = serve_answers(vec![
            (
                "/v1/spend-cap",
                200,
                r#"{"state":"set","limit_micro_credits":25000000,"spent_micro_credits":1000000,
                    "period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z","alert":"none"}"#
                    .to_owned(),
            ),
            (
                "/v1/usage/estimate",
                200,
                r#"{"dimensions":[{"dimension":"asr_seconds","amount":125}],
                    "outcome":{"state":"insufficient","reason":"spend_cap"}}"#
                    .to_owned(),
            ),
        ]);
        let client = signed_in_client(&base);
        let cap = client.set_spend_cap(Some(25_000_000)).await.unwrap();
        assert_eq!(
            cap,
            crate::usage::SpendCapDto::Set {
                limit_credits: 25.0,
                spent_credits: 1.0,
                period_ends_at: "2026-11-01T00:00:00Z".into(),
                alert: UsageAlertDto::None,
            }
        );
        let estimate = client
            .usage_estimate(
                "transcribe",
                crate::usage::EstimateQuantity::AsrSeconds(125),
            )
            .await
            .unwrap();
        assert_eq!(
            estimate.amounts,
            [crate::usage::EstimateAmountDto::Minutes { minutes: 3.0 }]
        );
        assert_eq!(
            estimate.outcome,
            crate::usage::EstimateOutcomeDto::Insufficient {
                reason: crate::usage::EstimateShortfallDto::SpendCap
            }
        );
    }

    /// 402 有两种（纵切 3 C4）：`spend_cap_reached` 是自设上限到了（调整上限），其余照旧是通用
    /// 服务端错误——不把额度用尽说成「你设的上限到了」。
    #[tokio::test]
    async fn a_402_spend_cap_reached_is_its_own_error() {
        for (body, capped) in [
            (r#"{"error":"spend_cap_reached"}"#, true),
            (r#"{"error":"quota_exhausted"}"#, false),
        ] {
            let base = serve_answers(vec![("/v1/usage", 402, body.to_owned())]);
            let err = signed_in_client(&base).usage().await.unwrap_err();
            if capped {
                assert!(matches!(err, CloudError::SpendCapReached), "{err:?}");
            } else {
                assert!(
                    matches!(err, CloudError::Server { status: 402, .. }),
                    "{err:?}"
                );
            }
        }
    }

    /// 上限的发前判定：未登录 / 严格本地一个字节都不发（与其余 `RequiresSession` 命令同一口径）。
    #[tokio::test]
    async fn spend_cap_and_estimate_need_a_session_and_respect_strict_local() {
        let client = client_for("http://127.0.0.1:9", Box::new(MemoryStore::new()));
        assert!(matches!(
            client.set_spend_cap(None).await,
            Err(CloudError::NotLoggedIn)
        ));
        let strict = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| true));
        assert!(matches!(
            strict
                .usage_estimate("chat", crate::usage::EstimateQuantity::AsrSeconds(1))
                .await,
            Err(CloudError::StrictLocalBlocked)
        ));
    }

    /// 起跑线上的余量口径（PRD-13-040 ②）。
    ///
    /// **两个方向都要验**：算得出缺口固然要紧，但「读不到就放行」是这条特性能不能
    /// 上线的前提——把「不知道」当成「不够」，等于给未登录 / 离线 / 私有化部署 / 本机
    /// 模型那一整批完全没有额度概念的用户造一道假门。
    #[test]
    fn headroom_sums_the_bounded_ledgers_and_stays_silent_when_it_cannot_know() {
        fn dim(name: &str, limit: Option<f64>, used: f64) -> UsageDimensionDto {
            UsageDimensionDto {
                dimension: name.to_owned(),
                limit,
                used,
                alert: UsageAlertDto::None,
            }
        }
        fn ledger(kind: &str, dims: Vec<UsageDimensionDto>) -> UsageLedgerDto {
            UsageLedgerDto {
                kind: kind.to_owned(),
                period: "monthly".to_owned(),
                dimensions: dims,
            }
        }

        // ① 单本账：剩下的就是 limit - used。
        let one = vec![ledger(
            "subscription",
            vec![
                dim("tokens_in", Some(9.0), 9.0),
                dim("tokens_out", Some(1000.0), 250.0),
            ],
        )];
        assert_eq!(remaining_in_dimension(&one, "tokens_out"), Some(750.0));

        // ② 订阅 + 预付**求和**：订阅用完了、可我刚买了流量包——分别看会把他拦下。
        let two = vec![
            ledger(
                "subscription",
                vec![dim("tokens_out", Some(1000.0), 1000.0)],
            ),
            ledger("prepaid", vec![dim("tokens_out", Some(500.0), 100.0)]),
        ];
        assert_eq!(remaining_in_dimension(&two, "tokens_out"), Some(400.0));

        // ③ 超支（服务端按 min(actual, reserved) 结算，账面仍可能贴着上限）不产生负数。
        let over = vec![ledger(
            "subscription",
            vec![dim("tokens_out", Some(10.0), 42.0)],
        )];
        assert_eq!(remaining_in_dimension(&over, "tokens_out"), Some(0.0));

        // ④ 任一本账「不限」→ 整体不限 → None → 放行。
        let unlimited = vec![
            ledger("subscription", vec![dim("tokens_out", None, 12_345.0)]),
            ledger("prepaid", vec![dim("tokens_out", Some(500.0), 100.0)]),
        ];
        assert_eq!(remaining_in_dimension(&unlimited, "tokens_out"), None);

        // ⑥ 预付余额还有剩：它按 credit 计，折成多少 token 只有服务端知道 → None → 放行。
        //    订阅用完了、可我刚买了流量包——不能拿订阅那一本的 0 把他拦下。
        let topped_up = vec![
            ledger(
                "subscription",
                vec![dim("tokens_out", Some(1000.0), 1000.0)],
            ),
            ledger("prepaid", vec![dim("credits", Some(600.0), 10.0)]),
        ];
        assert_eq!(remaining_in_dimension(&topped_up, "tokens_out"), None);
        // 预付用光了：照旧只看订阅那一本。
        let drained = vec![
            ledger(
                "subscription",
                vec![dim("tokens_out", Some(1000.0), 1000.0)],
            ),
            ledger("prepaid", vec![dim("credits", Some(600.0), 600.0)]),
        ];
        assert_eq!(remaining_in_dimension(&drained, "tokens_out"), Some(0.0));

        // ⑤ 服务端不按这一维计量 / 还没用过 → None → 放行（不是 0，不是"用完了"）。
        assert_eq!(remaining_in_dimension(&[], "tokens_out"), None);
        let other = vec![ledger(
            "subscription",
            vec![dim("requests", Some(10.0), 1.0)],
        )];
        assert_eq!(remaining_in_dimension(&other, "tokens_out"), None);
    }

    /// **Provider 侧的并发 401 同样不得重放一枚已被换走的 refresh。**
    ///
    /// `authed`（控制面）那条路由 `refresh_after_rejection` 守着，已有用例。而
    /// `CloudLlmProvider` / `CloudEmbedProvider` / `RemoteAsrProvider`（**批式**那条）
    /// 走的是**另一个入口**——`refreshed_access_token`（经 `src-tauri/src/cloud_bearer.rs`
    /// 的 `BearerSource::refresh`）。
    ///
    /// **`RealtimeAsrProvider` 不在这份名单里**，尽管它同样持有一个 `BearerSource`：
    /// 实时通道只在建流时 `bearer()` 取一次 token，整场会话（含每一次重连）复用那一枚，
    /// **从不调 `refresh()`**。此处此前把它一并列上，是一句对不上实现的话。
    ///
    /// 那条路此前零覆盖：一次长回答跨过 access 有效期时，
    /// 几路 Provider 会同时撞 401，而服务端把 refresh 设计成一次性凭据——**重放即家族全吊销**，
    /// 用户被踹回登录页，且没有任何一条错误指向"我们自己把它重放了"。
    ///
    /// 本用例的假服务端因此**真的实现重放检测**：只认当前那一枚 refresh，认过就作废。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn provider_side_concurrent_refresh_never_replays_a_spent_token() {
        const CALLS: usize = 4;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let replays = Arc::new(AtomicUsize::new(0));
        let rotations = Arc::new(AtomicUsize::new(0));
        let replays_for_server = Arc::clone(&replays);
        let rotations_for_server = Arc::clone(&rotations);

        // **accept 必须带死线，且服务端不得假设"一定来 CALLS 次"**：单飞一旦补上，
        // 真正到达的续期就只有一次，而一个死等 CALLS 次的 accept 会让 `join` 永远挂住
        // ——挂死在 CI 上的表现是「跑了一小时、零输出、没有任何失败断言」。同一条教训见
        // `serve_json_once`。故按死线收摊，请求数由断言去读，不由循环去假设。
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            const DEADLINE: Duration = Duration::from_secs(5);
            // 服务端侧的"当前有效 refresh"：换一次就作废上一枚。
            let mut live = "refresh-0".to_owned();
            let deadline = Instant::now() + DEADLINE;
            let mut served = 0_usize;
            while served < CALLS && Instant::now() < deadline {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                served += 1;
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(DEADLINE)).unwrap();
                let mut buffer = [0_u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                let (status, body) = if request.contains(&format!("\"{live}\"")) {
                    let issued = rotations_for_server.fetch_add(1, Ordering::SeqCst) + 1;
                    live = format!("refresh-{issued}");
                    (
                        "200 OK",
                        format!(
                            r#"{{"access_token":"access-{issued}","refresh_token":"refresh-{issued}"}}"#
                        ),
                    )
                } else {
                    replays_for_server.fetch_add(1, Ordering::SeqCst);
                    (
                        "401 Unauthorized",
                        r#"{"error":"reuse_detected"}"#.to_owned(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let client = Arc::new(client_for(
            &format!("http://{address}"),
            Box::new(MemoryStore::new()),
        ));
        client.cred_set(KEY_ACCESS, "access-0").unwrap();
        client.cred_set(KEY_REFRESH, "refresh-0").unwrap();

        let mut tasks = Vec::new();
        for _ in 0..CALLS {
            let client = Arc::clone(&client);
            tasks.push(tokio::spawn(async move {
                // 四路都握着同一枚 access-0——一次真实过期就是这样被几路 Provider 同时撞上的。
                client.refreshed_access_token("access-0").await
            }));
        }
        for task in tasks {
            assert!(
                task.await.unwrap().is_some(),
                "有一路续期拿不到 access——重放被服务端判成家族吊销后，会话已被清空"
            );
        }
        server.join().unwrap();

        assert_eq!(
            replays.load(Ordering::SeqCst),
            0,
            "有 {} 次续期重放了一枚已被换走的 refresh——线上这就是令牌家族全吊销",
            replays.load(Ordering::SeqCst)
        );
        assert_eq!(
            client.cred_get(KEY_REFRESH).unwrap().as_deref(),
            Some(format!("refresh-{}", rotations.load(Ordering::SeqCst)).as_str()),
            "本地留下的必须是服务端最后签发的那一枚，否则下一次请求必然被判重放"
        );
        // **单飞：{CALLS} 路同时 401，只许发生一次轮换。**
        //
        // 这条此前**刻意没断言**——`refreshed_access_token` 当时走的是不做二次检查的
        // `refresh_session`，于是 N 路依次抢锁、依次各轮换一次：不重放（每一路都用当时
        // 最新的 refresh），但白白多 N-1 个往返，且每一轮都在服务端那边作废一枚凭据。
        // 那处偏差记在 TDD-14 §3.3.1 ⑩，现已接上与控制面 `authed` 同一份二次检查。
        //
        // 断言写成"恰好 1"而不是"≤ N"：退回串行不会让任何别的断言变红。
        assert_eq!(
            rotations.load(Ordering::SeqCst),
            1,
            "{CALLS} 路并发 401 触发了 {} 次轮换——二次检查没生效，等锁期间别人已经换好了却还是又换了一次",
            rotations.load(Ordering::SeqCst)
        );
    }

    /// **等锁期间别人已经换好了 ⇒ 复用，一个字节都不再发。**
    ///
    /// 上一条用例证明「并发只轮换一次」，但它靠计数，说不清剩下 N-1 路究竟是**没发请求**
    /// 还是发了却被服务端合并。这条把判据落在「有没有建立连接」上：`api_base` 指向一个
    /// **已经没人监听**的端口——只要还有任何一路真去轮换，就是 connection refused，
    /// `refreshed_access_token` 返回 `None`，断言当场倒。
    ///
    /// 场景即真实时序：Provider A 手里的 `access-0` 挨了 401，等锁那几百毫秒里 Provider B
    /// 已经把钥匙链换成了 `access-1`；A 醒来该做的是**直接用 B 换好的那枚**。
    #[tokio::test]
    async fn a_token_already_rotated_by_someone_else_is_reused_without_a_request() {
        // 绑了就放掉：拿到一个几乎可以肯定没人监听的端口。
        let dead_port = {
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let client = client_for(
            &format!("http://127.0.0.1:{dead_port}"),
            Box::new(MemoryStore::new()),
        );
        // 钥匙链里已是别人换好的新一代凭据。
        client.cred_set(KEY_ACCESS, "access-1").unwrap();
        client.cred_set(KEY_REFRESH, "refresh-1").unwrap();

        let fresh = client.refreshed_access_token("access-0").await;

        assert_eq!(
            fresh.as_deref(),
            Some("access-1"),
            "没复用别人换好的 access——端口上没人监听，说明它真去发了一次多余的轮换请求"
        );
        assert_eq!(
            client.cred_get(KEY_REFRESH).unwrap().as_deref(),
            Some("refresh-1"),
            "复用这条路上不该动 refresh：动了就意味着确实轮换过一轮"
        );
    }

    /// **复用不是「凡是对不上就交出本地 access」**：无会话时仍须是 `None`。
    ///
    /// 组织 API Key 那一档走的正是这条：`rejected` 是一枚 API Key，与钥匙链里的 access
    /// 当然对不上，二次检查随即提前返回——若此时裸读 `KEY_ACCESS` 交差，就会把一枚**残留的
    /// 过期 access** 当成有效会话递回给 Provider，它拿着再撞一次 401，循环往复。
    /// 判据是「没有 refresh 就是未登录」（与 `access_token` / `authed` 前半段同一条）。
    #[tokio::test]
    async fn a_leftover_access_without_a_refresh_is_still_not_a_session() {
        let dead_port = {
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let client = client_for(
            &format!("http://127.0.0.1:{dead_port}"),
            Box::new(MemoryStore::new()),
        );
        // 退出登录清掉了 refresh，只剩一枚早已过期的 access（或者 Bearer 本就是 API Key）。
        client.cred_set(KEY_ACCESS, "stale-access").unwrap();

        assert!(
            client.refreshed_access_token("an-api-key").await.is_none(),
            "无 refresh 却交出了本地 access——Provider 会拿着它无限撞 401"
        );
    }

    /// 档位是线上必填的一格：认得四种，缺了或认不出都是读不懂（不在客户端补一个）。
    #[test]
    fn the_dimension_alert_is_the_servers_required_level() {
        let decode = |raw: &str| serde_json::from_str::<UsageDimensionDto>(raw);
        for (raw, level) in [
            ("none", UsageAlertDto::None),
            ("p80", UsageAlertDto::P80),
            ("p95", UsageAlertDto::P95),
            ("exhausted", UsageAlertDto::Exhausted),
        ] {
            let dim = decode(&format!(
                r#"{{"dimension":"tokens_out","limit":10,"used":1,"alert":"{raw}"}}"#
            ))
            .unwrap();
            assert_eq!(dim.alert, level);
            assert_eq!(level.as_str(), raw);
        }
        assert!(decode(r#"{"dimension":"tokens_out","limit":10,"used":1}"#).is_err());
        assert!(
            decode(r#"{"dimension":"tokens_out","limit":10,"used":1,"alert":"warning"}"#).is_err()
        );
        assert!(
            UsageAlertDto::Exhausted > UsageAlertDto::P95
                && UsageAlertDto::P80 > UsageAlertDto::None
        );
    }
}

#[cfg(test)]
mod rotation_settle_tests {
    use super::*;
    use crate::store::MemoryStore;
    use crate::trust::rotation_fixtures::{key, offered_at, pinned_at, pubkey, statement};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn client() -> CloudClient {
        {
            let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false));
            client.account().set_server(crate::account::AccountServer {
                origin: "https://a.test".to_owned(),
                display_name: None,
            });
            client.reconfigure();
            client
        }
    }

    /// 官方后端不钉公钥，「还没有核验过」对它永远不会变成真的（2026-10-13 真机）。
    #[test]
    fn the_official_server_reports_official_instead_of_unpinned() {
        let official = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false));
        official.reconfigure();
        assert_eq!(
            official.trust_status().state,
            crate::trust::TrustStateDto::Official
        );
        assert_eq!(
            client().trust_status().state,
            crate::trust::TrustStateDto::Unpinned
        );
    }

    /// 验过链 + 落地成功 = 自动换钥；交给落地那只手的必须是**服务端自报的那一组**。
    #[test]
    fn a_verified_chain_settles_with_the_offered_keys() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let client = client();
        let bucket = client.server_bucket();
        client.trust().pin(&bucket, pinned_at(&l1, &c1));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        client.set_rotation_applier(Arc::new(move |offered| {
            sink.lock().unwrap().push(offered.lease_pubkey.clone());
            Ok(())
        }));

        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(client.rotation_settles(&bucket, &offered_at(&l2, &c2), &chain));
        assert_eq!(seen.lock().unwrap().as_slice(), &[pubkey(&l2)]);
    }

    /// **没注入落地那只手时不许自动换钥**，且**一次都不许调用**它。
    ///
    /// 安全默认必须在「不换」这一侧：装配不全的进程（测试、纯库用法）如果把「没人落地」
    /// 读成「换成了」，界面会显示已核验而验签仍在用旧钥。
    #[test]
    fn without_an_applier_nothing_settles() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let client = client();
        let bucket = client.server_bucket();
        client.trust().pin(&bucket, pinned_at(&l1, &c1));
        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(!client.rotation_settles(&bucket, &offered_at(&l2, &c2), &chain));
    }

    /// 链验不过时**连落地那只手都不该被碰一下**——它会清凭据、重置水位，
    /// 而那些动作对一次没被背书的换钥是实实在在的破坏。
    #[test]
    fn an_unverifiable_chain_never_reaches_the_applier() {
        let (l1, c1, l2, c2, lx, cx) = (key(1), key(2), key(3), key(4), key(7), key(8));
        let client = client();
        let bucket = client.server_bucket();
        client.trust().pin(&bucket, pinned_at(&l1, &c1));
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        client.set_rotation_applier(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        // 链首接的是别人那一代。
        let chain = [statement(&lx, 1, (&lx, &cx), (&l2, &c2))];
        assert!(!client.rotation_settles(&bucket, &offered_at(&l2, &c2), &chain));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// 落地失败 = 没换成，原样退回阻断。写下来是因为它的反面很诱人：
    /// 「验都验过了，界面先说已更新吧」——那句话会在验签仍用旧钥时出现。
    #[test]
    fn a_failing_applier_falls_back_to_blocking() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let client = client();
        let bucket = client.server_bucket();
        client.trust().pin(&bucket, pinned_at(&l1, &c1));
        client.set_rotation_applier(Arc::new(
            |_| Err("TRUST_CREDENTIAL_CLEAR_FAILED".to_owned()),
        ));
        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(!client.rotation_settles(&bucket, &offered_at(&l2, &c2), &chain));
    }

    /// 从未钉住过就没有「从哪一代开始」——首连走 TOFU，不走轮换。
    #[test]
    fn an_unpinned_bucket_never_rotates() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let client = client();
        let bucket = client.server_bucket();
        client.set_rotation_applier(Arc::new(|_| Ok(())));
        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(!client.rotation_settles(&bucket, &offered_at(&l2, &c2), &chain));
    }
}

#[cfg(test)]
#[path = "client_device_limit_tests.rs"]
mod device_limit_tests;

#[cfg(test)]
#[path = "client_account_data_tests.rs"]
mod account_data_tests;

#[cfg(test)]
#[path = "client_opaque_tests.rs"]
mod opaque_tests;
