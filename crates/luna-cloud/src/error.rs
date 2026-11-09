//! 云命令统一错误。
//!
//! 前端只消费 `code`（映射 i18n key，禁硬编码文案）；`detail` 是给日志/诊断的
//! 英文技术细节，**绝不含** token、邮箱、密码等敏感值（日志零 PII 纪律）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CloudError {
    /// 本地无会话（未登录时调用了需登录的命令）。
    #[error("not logged in")]
    NotLoggedIn,
    /// 会话已失效且静默 refresh 失败——本地凭据已清除，需要重新登录。
    #[error("session expired; relogin required")]
    NeedsRelogin,
    /// 没有进行中的浏览器登录配对。
    #[error("no pending device login")]
    NoPendingLogin,
    /// 配对码已过期/失效（重新发起）。
    #[error("device login expired")]
    LoginExpired,
    /// 严格本地模式拦截了联网动作（发请求前阻断，TDD-14 §3.1 目录客户端状态机）。
    #[error("blocked by strict local mode")]
    StrictLocalBlocked,
    /// 网络层失败（连接/超时/DNS）。
    ///
    /// **必须带上打的是哪一台**。此前只有 `detail`（英文技术串，前端不展示），界面
    /// 渲染的是一句笼统的「网络连接失败，请检查网络后重试」——用户于是去查自己的网络，
    /// 而真实情况往往是"那一台服务器不可达"（他刚在引导里选了 Lunarca 云，而官方云地址
    /// 尚未上线；或组织服务端没起来）。**说出主机名，排查方向立刻就对了。**
    ///
    /// `kind` 把「连不上」再分一档（DNS / 拒绝 / 超时 / TLS / 其它）：每一档对应用户
    /// 一个不同的下一步（改地址 / 让服务端起来 / 查网络 / 装根证书），一句笼统的
    /// 「连不上」等于把这四个方向全推给他自己猜。
    #[error("network failure ({host}): {detail}")]
    Network {
        host: String,
        detail: String,
        kind: NetworkFailureKind,
    },
    /// 服务端地址本身不合法，请求**根本没发出去**（URL 构造阶段就失败）。
    ///
    /// 与 [`Self::Network`] 分开是这条错误存在的**全部意义**。此前二者同码：用户把
    /// 组织服务端地址填成 `10.0.0.62:8090`（缺 scheme），reqwest 在 builder 阶段失败，
    /// 界面却显示「网络连接失败，请检查网络后重试」——于是他去重启路由器，而问题在
    /// 那七个字符。登录时的地址已归一化（`account::normalize_server_address`）堵住主路径，
    /// 这一条兜住其余入口（调试构建的 `LUNARCA_API_BASE` 环境变量）。
    #[error("invalid endpoint: {detail}")]
    InvalidEndpoint { detail: String },
    /// 服务端答了，但这一版客户端读不懂它的形状。
    ///
    /// 与 [`Self::Network`] 分开是这条错误存在的**全部意义**：请求发出去了、连上了、
    /// 服务端回了 200——只是我们解不动那个响应体。归进 `Network` 的代价在真机上兑现过
    /// （2026-08-10）：`/v1/usage` 的出站字节额度是 5 GiB，超出当时 `u32` 的字段类型，
    /// 反序列化失败被报成「连不上 10.0.0.62:8443」，而同一页的权益、租约、设备**全都刚从
    /// 那台服务端拉回来**。用户按着那句话去查网络，而问题在两侧的类型上。
    ///
    /// 它同时是**版本偏斜**的正确出口：服务端比客户端新、多了一个读不懂的形状时，
    /// 该说的是「升级客户端」而不是「检查网络」。
    #[error("unreadable response from {host}: {detail}")]
    UnreadableResponse { host: String, detail: String },
    /// 服务端非预期响应。
    #[error("server error (status {status}): {error_code}")]
    Server { status: u16, error_code: String },
    /// 触发服务端限频。（u32：specta 禁 64 位整数入 TS，见生成管线；秒级等待值域足够。）
    #[error("rate limited")]
    RateLimited { retry_after_secs: Option<u32> },
    /// **信任锚变了**——端点自报的两把签名公钥与本机钉住的不一致（ADR-060 ②）。
    ///
    /// 这条错误存在的**全部意义**是不再说那句指错方向的「签名校验失败」。ADR-060 开头
    /// 点名的正是它：换过密钥的服务端会让目录验签失败，而用户拿着「签名校验失败」
    /// 只能去怀疑网络、怀疑服务端坏了，唯独想不到「这台服务端换了身份」——那恰恰是
    /// 唯一该做的判断。收到本码时界面必须**阻断**并要求一次显式的重新确认，
    /// 确认之前验签一律继续用旧钉住钥。
    #[error("trust anchor changed")]
    TrustAnchorChanged,
    /// 应用内登录 / 设口令被服务端拒绝（ADR-154、ADR-158）。
    #[error("login rejected: {reason:?}")]
    LoginRejected { reason: LoginRejection },
    /// 信任锚线格式版本不认识 → 如实拒绝并提示**升级客户端**（ADR-046 的诚实拒绝语义）。
    #[error("unsupported trust format version {found} (supported {supported})")]
    TrustUnsupportedFormat { found: u32, supported: u32 },
    /// 一次只登录一个账户：换服务器先退出（ADR-155）。
    #[error("already signed in")]
    AlreadySignedIn,
    /// 输入既不是邀请链接，也不是服务器地址。
    #[error("not a server address")]
    NotAnAddress,
    /// 填的地址上没有 Lunarca 服务端（能力声明 404 或读不懂），按主机名回查
    /// `.well-known/lunarca` 也没有——多半是填了官网或别的网站（ADR-158 ①）。
    #[error("no Lunarca server at {host}")]
    NoServerAtAddress { host: String },
    /// 登录第一步填的不是邮箱（缺 `@` 或域名）：发请求前在本地拒绝。
    #[error("not an email address")]
    NotAnEmail,
    /// 连接链接坏了（每一条原因对应一个用户能做的动作）。
    #[error("connect link rejected: {reason}")]
    LinkRejected {
        reason: crate::connect_link::ConnectLinkRejection,
    },
    /// 口令对上了，但服务端送来的身份不可信（ADR-158 ③）：信任包解不开、写的是另一台部署，
    /// 或与邮箱域名上发布的部署不一致。**什么都没钉住**；多半是连到了别的服务器，换一个地址再试。
    #[error("server identity rejected")]
    ServerIdentityRejected,
    /// 客户端太旧：服务器要求的最低协议版本高于本客户端 → 更新 Lunarca。
    #[error("client too old (server requires {min_client_protocol})")]
    ClientTooOld { min_client_protocol: u32 },
    /// 服务器太旧：它的协议版本低于本客户端所需 → 请管理员升级服务器。
    #[error("server too old (protocol {server_protocol})")]
    ServerTooOld { server_protocol: u32 },
    /// 这台服务器没有启用收单（billing 端点 503 `billing_disabled`，私有化的正常形态）。
    /// 界面据此收起升级 / 购买入口，而不是把它当成一次失败。
    #[error("billing is not enabled on this server")]
    BillingDisabled,
    /// 要买的那一项已不在这台服务器的在售价目里（结账 404）：价目列表过时了，重新取一次。
    #[error("plan is no longer offered")]
    PlanUnavailable,
    /// 这个账户在这台服务器上还没付过款，没有可管理的订阅（账单管理 404）。
    #[error("no billing account yet")]
    NoBillingAccount,
    /// 注销时服务端退不掉在扣费的订阅（`DELETE /v1/account` 回 503 `billing_unavailable`，协议 v14）：
    /// **什么都没删**，账户与会话原样还在。下一步是稍后再试——不能说成「删除失败了一半」。
    #[error("billing provider unavailable; nothing was deleted")]
    BillingUnavailable,
    /// 注销被拦下：这个账户是某个组织里唯一的所有者，而组织还有其他成员（`DELETE /v1/account` 回
    /// 409 `last_org_owner`，协议 v17）。**什么都没删**。预览本来就会把确认按钮收起，走到这里说明
    /// 预览之后处境变了；下一步是先把所有者转给别的成员，或请管理员处理。
    #[error("sole owner of an organization with other members; nothing was deleted")]
    LastOrgOwner,
    /// 账户数据导出写不进用户选的位置（建临时文件 / 写入 / 落盘 / 改名任一步失败，协议 v14）。
    /// 半份文件已删；`detail` 只带 I/O 错误种类，**不含路径**（路径里有用户名）。
    #[error("export write failed: {detail}")]
    ExportWriteFailed { detail: String },
    /// 账户因付款争议被冻结为只读（服务端 403 `account_frozen`，纵切 3 C3）：推理、转写、同步
    /// 上传与结账一律拒绝，登录、权益、用量、账单管理与同步下载照常。下一步是联系支持；
    /// 本地数据不受影响。
    #[error("account is frozen")]
    AccountFrozen,
    /// 网页访问只在启用网页版的官方云上提供（ADR-177 ①）。
    #[error("web access unavailable")]
    WebAccessUnavailable,
    /// 这次预留要从预付 credits 扣，而用户自设的本月预付上限已到（服务端 402 `spend_cap_reached`，
    /// 纵切 3 C4）。下一步是调整上限，不是购买；订阅包含的额度不受上限影响。
    #[error("monthly prepaid spend cap reached")]
    SpendCapReached,
    /// 要设的本月预付上限不是一个可用的 credits 数（负数、非有限值或大到存不下）。
    /// 发请求前在本地拒绝。
    #[error("invalid spend cap")]
    InvalidSpendCap,
    /// 这个账户此刻领不到免费额度（服务端 402 `free_quota_withheld`，纵切 3 C5）：这台设备上已有别的
    /// 账户领过，或要先做一次零扣费的付款方式验证。成因在权益 `free_quota` 里；预付 credits 与付费
    /// 订阅不受影响。
    #[error("free quota withheld")]
    FreeQuotaWithheld,
    /// 付款方式验证（`POST /v1/billing/verify-card`）回 409 `verification_not_required`：这个账户
    /// 已经不需要验证了（多半是刚验过，或运营已放行）。界面据此重取权益，而不是把它当成失败。
    #[error("card verification not required")]
    VerificationNotRequired,
    /// 这次登录撞上设备上限之后，「移除一台并登录」那一步的授权已失效（`revoke_token` 过期、已用过，
    /// 或这次登录已被取消 / 替换，纵切 3 C5）。挂着的选择已清掉，下一步是重新登录。
    #[error("device limit choice expired")]
    DeviceLimitExpired,
    /// 注册被拒：服务端不接受这个邮箱域名（400 `email_domain_not_allowed`，PRD-13-018 L0 的一次性
    /// 邮箱域名黑名单）。下一步是换一个常用邮箱。
    #[error("email domain not allowed")]
    EmailDomainNotAllowed,
    /// 没有可兑换的邀请（链接没带，或已经用掉）。
    #[error("no invite to redeem")]
    NoInvite,
    /// 系统钥匙链读写失败。
    #[error("keychain failure: {detail}")]
    Keychain { detail: String },
    /// 客户端内部错误。
    #[error("internal: {detail}")]
    Internal { detail: String },
}

/// 应用内登录被拒的原因（服务端稳定码，ADR-154）。每一条对应界面上的一句话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LoginRejection {
    /// 邮箱与口令（或一次性码）对不上（也覆盖未知邮箱、码已过期或已用、对端不是这个账户的
    /// 那台服务端：反枚举同一句）。
    InvalidCredentials,
    /// 账户已被停用。
    AccountDisabled,
    /// 账户所在组织已归档。
    OrganizationArchived,
    /// 新密码不合规（长度 8–128，或与当前密码相同）。
    PasswordInvalid,
}

impl LoginRejection {
    /// 服务端错误码 → 原因；认不出 → `None`（按通用服务端错误报）。
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "invalid_credentials" => Self::InvalidCredentials,
            "account_disabled" => Self::AccountDisabled,
            "organization_archived" => Self::OrganizationArchived,
            "password_invalid" => Self::PasswordInvalid,
            _ => return None,
        })
    }
}

impl CloudError {
    /// reqwest 错误 → 分类。`base` 是这次请求打的服务端基址（用于告诉用户"连不上哪一台"）。
    ///
    /// builder 错误 = 请求**没发出去**（URL 拼不出来）。它与"发出去了但连不上"是
    /// 两条完全不同的排查路径，映射成同一个 code 就是在指错方向。
    pub fn network(err: &reqwest::Error, base: &str) -> Self {
        // reqwest 错误串只含 URL/阶段信息，不含请求体或凭据。
        if err.is_builder() {
            return Self::InvalidEndpoint {
                detail: err.to_string(),
            };
        }
        Self::Network {
            host: crate::config::server_bucket(base),
            kind: NetworkFailureKind::classify(err),
            detail: err.to_string(),
        }
    }

    /// 读响应体失败 → 分类。用在每一处 `.json()` 之后。
    ///
    /// **`is_decode()` 必须先判**：`.json()` 的失败有两种来源——「读到一半连接断了」
    /// （确实是网络）与「读完了但形状不认识」（不是网络）。全归网络，就等于让每一次
    /// 两侧类型不一致都伪装成一次网络故障，而排查方向完全相反。
    pub fn body(err: &reqwest::Error, base: &str) -> Self {
        if err.is_decode() {
            return Self::UnreadableResponse {
                host: crate::config::server_bucket(base),
                // reqwest 的 decode 错误串只含类型/位置信息，不含响应体本身。
                detail: err.to_string(),
            };
        }
        Self::network(err, base)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal {
            detail: detail.into(),
        }
    }
}

/// 「连不上」的分档。判据取自 reqwest 错误链（`std::error::Error::source`）里最内层的
/// `io::Error` 种类与 TLS/DNS 错误的类型名——reqwest 自己只暴露 `is_connect/is_timeout`，
/// 分不出 DNS 与拒绝，而那两者的下一步完全不同（地址打错 vs 服务没起）。
///
/// 分不出来时如实 `Other`，不把猜测当结论：错档的指引比不指引更糟。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum NetworkFailureKind {
    /// 主机名解析不出来：地址多半打错，或本机 DNS 不认这个内网名。
    Dns,
    /// 对端拒绝连接：主机通、端口上没有服务（服务端没起，或端口填错）。
    Refused,
    /// 超时：主机不响应（不在同一网络 / 防火墙丢包）。
    Timeout,
    /// TLS 握手失败：多半是自签根证书没装进系统信任库。
    Tls,
    /// 没有通往对端的路由（`EHOSTUNREACH` / `ENETUNREACH`）：不在同一网络，或 macOS 的
    /// 「本地网络」权限没给本应用——后者连局域网地址时报的就是这一种，curl 却能通。
    Unreachable,
    /// 其余（连接被重置等）。
    Other,
}

impl NetworkFailureKind {
    #[must_use]
    pub fn classify(err: &reqwest::Error) -> Self {
        if err.is_timeout() {
            return Self::Timeout;
        }
        let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
        while let Some(current) = cause {
            // 先按类型认 TLS：错误文字随系统语言本地化（中文 Windows 的 Schannel 报
            // CERT_E_UNTRUSTEDROOT 时整句是中文），按英文关键字判会落进 `Other`，
            // 界面就指不到「用管理员发的连接链接钉住证书」这一步（Windows 真机 2026-10-15）。
            if current.downcast_ref::<native_tls::Error>().is_some() {
                return Self::Tls;
            }
            if let Some(io) = current.downcast_ref::<std::io::Error>() {
                return Self::from_io(io);
            }
            let name = format!("{current:?}");
            // hyper-util 的连接错误把 DNS 失败包成 `ConnectError("dns error", …)`；
            // rustls / native-tls 的错误类型名里带 "Tls" / "certificate"。
            let lower = name.to_ascii_lowercase();
            if lower.contains("dns error") || lower.contains("failed to lookup address") {
                return Self::Dns;
            }
            if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
                return Self::Tls;
            }
            cause = current.source();
        }
        if err.is_connect() {
            return Self::Other;
        }
        Self::Other
    }

    fn from_io(io: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        // tokio-native-tls 把握手失败包成 `io::Error::other(native_tls::Error)`。
        if io
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<native_tls::Error>().is_some())
        {
            return Self::Tls;
        }
        if io.raw_os_error().is_some_and(is_windows_certificate_error) {
            return Self::Tls;
        }
        match io.kind() {
            ErrorKind::ConnectionRefused => Self::Refused,
            ErrorKind::TimedOut => Self::Timeout,
            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => Self::Unreachable,
            _ => {
                let text = io.to_string().to_ascii_lowercase();
                if text.contains("dns")
                    || text.contains("nodename nor servname")
                    || text.contains("name resolution")
                {
                    Self::Dns
                } else if text.contains("certificate")
                    || text.contains("tls")
                    || text.contains("ssl")
                {
                    Self::Tls
                } else if text.contains("refused") {
                    Self::Refused
                } else {
                    Self::Other
                }
            }
        }
    }
}

/// Windows 证书校验失败的系统错误码（`CERT_E_*`、`SEC_E_*`）：Schannel 有时只交回裸码，
/// 文字又是本地化的，码是唯一不随语言变的判据。值是 HRESULT 按 `i32` 解释。
fn is_windows_certificate_error(code: i32) -> bool {
    const CODES: [u32; 8] = [
        0x800B_0109, // CERT_E_UNTRUSTEDROOT：自签根不在信任库
        0x800B_010A, // CERT_E_CHAINING
        0x800B_0101, // CERT_E_EXPIRED
        0x800B_010F, // CERT_E_CN_NO_MATCH
        0x8009_0325, // SEC_E_UNTRUSTED_ROOT
        0x8009_0322, // SEC_E_WRONG_PRINCIPAL
        0x8009_0328, // SEC_E_CERT_EXPIRED
        0x8009_0327, // SEC_E_CERT_UNKNOWN
    ];
    CODES.iter().any(|known| *known as i32 == code)
}

#[cfg(test)]
mod network_kind_tests {
    use super::*;

    #[test]
    fn io_kinds_map_to_actionable_buckets() {
        let refused = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            NetworkFailureKind::from_io(&refused),
            NetworkFailureKind::Refused
        );
        let timeout = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
        assert_eq!(
            NetworkFailureKind::from_io(&timeout),
            NetworkFailureKind::Timeout
        );
        let dns = std::io::Error::other(
            "failed to lookup address information: nodename nor servname provided",
        );
        assert_eq!(NetworkFailureKind::from_io(&dns), NetworkFailureKind::Dns);
        let tls = std::io::Error::other("invalid peer certificate: UnknownIssuer");
        assert_eq!(NetworkFailureKind::from_io(&tls), NetworkFailureKind::Tls);
        let unreachable = std::io::Error::new(std::io::ErrorKind::HostUnreachable, "no route");
        assert_eq!(
            NetworkFailureKind::from_io(&unreachable),
            NetworkFailureKind::Unreachable
        );
        // 中文 Windows：文字没有任何英文关键字，只剩错误码可认。
        let untrusted_root = std::io::Error::from_raw_os_error(0x800B_0109_u32 as i32);
        assert_eq!(
            NetworkFailureKind::from_io(&untrusted_root),
            NetworkFailureKind::Tls
        );
        let localized =
            std::io::Error::other("已处理证书链，但是在不受信任提供程序信任的根证书中终止。");
        assert_eq!(
            NetworkFailureKind::from_io(&localized),
            NetworkFailureKind::Other,
            "纯文字且无码时不猜（类型与码两条判据都没有）"
        );
        let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert_eq!(
            NetworkFailureKind::from_io(&reset),
            NetworkFailureKind::Other
        );
    }

    #[tokio::test]
    async fn refused_port_classifies_as_refused() {
        // 绑一个端口再立刻放掉：那一刻它必然是「主机通、无人监听」。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let client = reqwest::Client::new();
        let err = client
            .get(format!("http://127.0.0.1:{port}/x"))
            .send()
            .await
            .unwrap_err();
        assert_eq!(
            NetworkFailureKind::classify(&err),
            NetworkFailureKind::Refused
        );
    }
}
