//! 协议版本协商（TDD-16 §5.1.1）。
//!
//! # 为什么必须有
//!
//! SaaS 的服务端永远比客户端新，**私有化不是**：部署方按自己的窗口升级，而客户端会自动
//! 更新，于是「新客户端 + 旧服务端」是常态而非异常。没有协商时的表现是**神秘的 400/404
//! 与半可用界面**——用户看不出是自己的问题、组织的问题，还是产品坏了。
//!
//! # 三条规矩（逐条来自 TDD-16 §5.1.1）
//!
//! 1. **在连接时比对**，而不是等到首次调用失败；
//! 2. 不匹配时给**可执行提示**而非失败码（「联系管理员升级到 ≥ vN」/「更新客户端」）；
//! 3. **在登录之前说清楚**：两侧不兼容时不进入登录，直接告诉用户该升级哪一侧
//!    （v9 起登录方式只按新协议渲染，ADR-155）。
//!
//! 协商挂在 `luna-cloud`：它的 `api_base` 就是那台 Lunarca 服务端（账户、目录、用量、
//! 推理全走它）。

use serde::{Deserialize, Serialize};

/// 本客户端实现的协议版本。
///
/// **加新的线上契约时必须 +1**，并同步服务端的 `MIN_CLIENT_PROTOCOL` 判断。它不是
/// 应用版本号——应用可以发十个版本而协议一动不动；混用会让「要不要提示升级」变成
/// 跟着发版号乱跳。
/// **v2（ADR-049）**：批式转写请求体新增必填 `tier`，SSE 阶段流新增 `queued` 事件。
/// **v3（ADR-038 ③ 闭合）**：实时 ASR 的 WebSocket 上行二进制帧由 16-bit PCM 改为
/// **Opus**（一包一帧）。这是一次纯粹的破坏性切换——旧 PCM 路径两侧同批删除，不做
/// 双格式、不做格式协商回退（ADR-046：兼容分支不能代替版本化）。v2 客户端发上来的
/// PCM 会被 Opus 解码器当成非法包，故服务端的 `min_client_protocol` 必须同步抬到 3：
/// 让它连上来只会得到一条「非法帧」，那比一句「请升级客户端」难懂得多。
/// v4：原生嵌入请求以稳定 `profile` 约束向量空间，云端具体模型由服务端选择。
/// **v5（ADR-083 ②⑨）**：`POST /inference/v1/jobs`
/// 请求体新增**必填** `service_class`（`realtime` / `batch`），服务端据它在两条翻译泳道
/// 之间选链；同批新增错误码 `route_lane_empty`（那一档的链一个候选都没有，不可重试）。
/// 它是破坏性的：服务端那一侧**没有** `serde(default)`——v4 客户端发不出这一格，让它连上来
/// 只会在**每一次提交时**拿 400，而那比一句「请升级客户端」难懂得多。故 `MIN_CLIENT_PROTOCOL`
/// 同批抬到 5，把它挡在协商那一刻（ADR-046 ④：可选字段过渡是兼容层的最小形态，不做）。
/// **v6**：输出触顶不再判成额度耗尽——服务端把它当成**正常完成**，并在终态帧里带上
/// `output_truncated`（`caller` / `upstream` / `absolute`）说明是哪一道闸先到。
/// 语义变的是**终态本身**：同一次触顶，v5 发 `error`、v6 发 `done`，客户端据此决定
/// 「丢掉正文报失败」还是「保留正文加标注」。这就是它必须走版本号而不是可选字段的原因
/// （ADR-046 ④）——挂一个 `Option` 只能表达「有没有带这一格」，表达不了「同一件事的终态
/// 换了一种」，而客户端恰恰要按后者分流。
///
/// **本客户端先行**：服务端侧（含 `MIN_CLIENT_PROTOCOL`）在下一批落地，届时它仍停在 5。
/// 期间 v6 客户端连 v5 服务端 → `ServerTooOld`，按该枚举的约定**仍然可用**：触顶照旧表现为
/// 旧行为（那是服务端的事实，客户端改不了），其余能力一概不受影响。反过来先抬服务端才是
/// 真的会红——那一侧有条测试断言 `MIN_CLIENT_PROTOCOL ≤ CLIENT_PROTOCOL ≤ PROTOCOL_VERSION`，
/// 读的是 pin 住的这个常量。
/// **v7（ADR-116）**：`GET /inference/v1/capabilities` 新增 `deployment { id, display_name }`
/// 与 `auth { inference[], login { device_code, oidc, admission } }`——客户端据此写出
/// 「已连接 <部署名>」、决定「登录」这一步要不要出现、以及用什么口径提示准入。**只加字段**，
/// 服务端 `min_client_protocol` 不抬。对 v6 服务端这两块**如实缺席**（`None`），界面退回
/// 「按地址称呼、默认呈现登录」——这是 TDD-16 §5.1.1 第三条的降级子集，不是给自己旧版本
/// 留的兼容分支（ADR-046 ④ 针对的是**请求体**的可选字段过渡；响应里服务端旧于客户端
/// 本就是协商要覆盖的常态窗口）。
/// **v8（ADR-154）**：新增应用内登录三条端点 `POST /v1/auth/password`、`/v1/auth/password/change`、
/// `/v1/auth/invite`。
/// **v9（ADR-155，破坏性）**：capabilities 的 `auth.login` 重写为 `{ password, browser, sso: [名称],
/// admission }`，`deployment` / `auth` 恒在场；服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 9。客户端登录前
/// 只按这份答案渲染登录方式，不再靠「试一次密码登录、404 就退回浏览器」试错，故旧服务端
/// （`protocol < 9`）一律判为太旧、拒绝登录并提示升级——不留退回分支（ADR-046）。
/// **v10（纵切 3 C1/C2，SaaS 收单）**：capabilities 恒带 `billing { enabled }`（[`BillingDto`]），
/// 新增公开价目 `GET /v1/billing/plans` 与结账收 `plan_id`。服务端只加字段、`MIN_CLIENT_PROTOCOL`
/// 仍为 9；本客户端把 `billing` 读成**必填**，故对 v9 服务端如实判为太旧——判据在解析整份
/// 答案**之前**（[`ProtocolVersionsDto`]），否则缺字段会被报成「读不懂的响应」而不是「服务器太旧」。
/// **v11（纵切 3 C3，催缴与拒付，破坏性）**：`GET /v1/entitlements` 恒带 `billing_standing`
/// （`good` / `past_due { grace_ends_at }` / `frozen`，[`crate::billing::BillingStandingDto`]，私有化恒
/// `good`）；冻结时推理、转写、同步写入与结账回 403 `account_frozen`。服务端 `MIN_CLIENT_PROTOCOL`
/// 同批抬到 11，本客户端把 `billing_standing` 读成必填——不留可选字段过渡（ADR-046）。
/// **v12（纵切 3 C4，成本可预期性，破坏性）**：`GET /v1/usage` 每本账恒带 `alert_key`、每一维恒带
/// `alert`（`none` / `p80` / `p95` / `exhausted`），顶层恒带 `spend_cap`（`unset` / `set`，只管预付
/// credits，UTC 自然月）；新增 `PUT /v1/spend-cap` 与 `POST /v1/usage/estimate`；预留要从预付扣而撞上
/// 自设上限时回 402 `spend_cap_reached`（[`crate::usage`]）。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 12，
/// 本客户端把三格都读成必填——不留可选字段过渡（ADR-046）。
/// **v13（纵切 3 C5，免费档反滥用与设备上限，破坏性）**：每一条建设备会话的登录请求（密码、改初始
/// 密码、找回重设、兑换邀请、设备码的发起与轮询两帧）恒带 `device_fingerprint`（64 位小写十六进制，
/// [`crate::fingerprint`]）；登录可回 403 `device_limit_reached`（附设备列表与一次性 `revoke_token`），
/// 经 `POST /v1/devices/revoke-for-sign-in` 移除一台后原样重发（[`crate::device_limit`]）；
/// `GET /v1/entitlements` 恒带 `free_quota`（[`crate::billing::FreeQuotaDto`]）；新增
/// `POST /v1/billing/verify-card`；预留回 402 `free_quota_withheld`；注册回 400
/// `email_domain_not_allowed`。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 13，本客户端把这些都读成 /
/// 发成必填——不留可选字段过渡（ADR-046）。
/// **v14（纵切 3 C6，账户数据权利，破坏性）**：新增 `GET /v1/account/deletion-preview`（`subscription`
/// `none` / `active { tier }`、`prepaid_micro_credits`、`billing_retention_years`）与 `GET /v1/account/export`
/// （JSON 附件，流式，限流回 429）；`DELETE /v1/account` 的回执由一句服务端写死的文案改为
/// `{ deleted: true, retained: { billing_retention_years } }`（文案归界面），退不掉订阅时回 503
/// `billing_unavailable`、什么都不删（[`crate::account_data`]）。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 14，
/// 本客户端把这些都读成必填——不留可选字段过渡（ADR-046）。
/// **v15（同步库的删除与恢复，破坏性）**：同步面新增 `DELETE /sync/v1/vaults/{id}`（删除云端库，所有设备停止
/// 同步、30 天内可恢复、删除即不计容量）与 `POST /sync/v1/vaults/{id}/restore`；库列表每一项恒带
/// `last_changed_at`，`status` 新增 `deleted`（只在那时带必填的 `deleted_at` / `purge_after`，
/// [`crate::sync_client::SyncVaultStateDto`]）；已删除库的数据面一律 410 `vault_deleted`，提示流发终结帧
/// `vault_deleted` 后关流（[`crate::sync_client::SyncEventDto`]）；恢复没删的库回 409 `vault_not_deleted`、
/// 过期回 404、超容量回 402 `sync_quota_exceeded`。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 15，本客户端把
/// 新字段都读成必填——v14 客户端会把另一台设备删掉的库当成反复 410 的同步故障，而它该做的是断开这个库、
/// 告诉用户本地文件都在（ADR-046：不留可选字段过渡）。
/// **v16（ADR-158，应用内 OPAQUE 口令认证，破坏性）**：`POST /v1/auth/password`、`/password/change`、
/// `/invite`、`/password/reset` 下线，换成 `/v1/auth/opaque/login/{start,finish}`（未钉住的服务端上是唯一
/// 允许放宽 TLS 校验的两条，[`crate::first_contact`]）、`/opaque/session`（只走钉住的 TLS，领取凭据换设备
/// 会话或注册授权）与 `/opaque/register/{start,finish}`（带 Bearer 注册口令）；新增 `POST /v1/auth/signup/start`；
/// 新错误码 `410 login_expired`。口令不离开设备，服务端的 TLS 根证书与两把签名公钥随信任包经会话密钥送达
/// （`luna_opaque`）。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 16，新字段一律必填（ADR-046）。
/// **v17（债务批 D1，破坏性）**：`GET /v1/billing/plans` 的 `billing_rules` 去掉 `micro_credits_per_credit`
/// ——换算比是协议量纲（[`crate::billing::MICRO_CREDITS_PER_CREDIT`]），不随价目下发；
/// `GET /v1/account/deletion-preview` 恒带 `deletion`（`allowed` / `blocked { reason: last_org_owner }`，
/// [`crate::account_data::AccountDeletionDto`]），`DELETE /v1/account` 在账户是组织唯一所有者、组织还有其他
/// 成员时回 409 `last_org_owner`、什么都不删。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 17，`deletion` 读成
/// 必填（ADR-046）。
/// **v18（ADR-169 ⑥，目录签名密钥分层，破坏性）**：`GET /v1/catalog/*` 的响应恒带 `keys`——离线根密钥签发的
/// 在线目录密钥清单（[`crate::catalog::SignedCatalogKeys`]）；目录签名须来自清单里的某一把，清单本身按根公钥
/// 验、按版本防回滚（吊销）。`GET /v1/trust/keys` 与信任包里的 `catalog_pubkey` 从「在线目录公钥」改为
/// 「目录根公钥」。服务端 `MIN_CLIENT_PROTOCOL` 同批抬到 18，`keys` 读成必填（ADR-046）。
/// **v19（ADR-159 ③ ⑨，纵切 7a）**：新增主题目录 `GET /v1/catalog/themes`（公开只读，404 = 没发布过），
/// 条目是 `luna_content_schema::ThemeCatalogEntry`（主题包内联 + 最低客户端版本），登录后随另两份目录
/// 一起同步。按惯例与服务端 `MIN_CLIENT_PROTOCOL` 同升到 19。
/// **v20（ADR-159 ③ ⑥，纵切 7b）**：新增模板目录 `GET /v1/catalog/templates`（公开只读，404 = 没发布过），
/// 条目是 `luna_content_schema::TemplateCatalogEntry`（包的摘要、由内容推导的能力、包体 sha256 与大小、
/// 最低客户端版本），版本内容 `{ entries, revoked }`；包体按 sha256 取
/// `GET /v1/catalog/templates/packages/{sha256}`（只给当前生效目录里的包）。与服务端 `MIN_CLIENT_PROTOCOL`
/// 同升到 20。
/// **v21（ADR-171 ④，破坏性）**：capabilities 的 `auth.login.sso`（名称列表）换成
/// `providers: [{ id, name, kind }]`（`kind` = `org` 组织单点登录 / `google`、`facebook` 公共身份源）；登录页按 `kind`
/// 画按钮，点了用 `id` 直接打开那一家的浏览器登录（[`crate::CloudClient::login_start`]）。与服务端
/// `MIN_CLIENT_PROTOCOL` 同升到 21。
pub const CLIENT_PROTOCOL: u32 = 21;

/// 协商结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    /// 双方都在对方支持的区间内。
    Ok,
    /// **客户端太旧**：服务端要求的 `min_client_protocol` 高于本客户端。可执行动作 = 更新客户端。
    ClientTooOld,
    /// **服务端太旧**：服务端的 `protocol` 低于本客户端所需。可执行动作 = 联系管理员升级。
    ServerTooOld,
}

/// 部署的人类可读身份。`display_name` 由部署方在控制台设置，未设时为 `None`，
/// 界面退回用地址称呼——**不**拿 `id` 冒充名字（它是机器标识）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeploymentDto {
    pub id: String,
    pub display_name: Option<String>,
}

/// 服务端接受什么凭据、人怎么登录（协议 v9）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthDto {
    pub login: LoginModesDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LoginModesDto {
    /// 应用内邮箱 + 密码（控制台「登录与注册」开关）。
    pub password: bool,
    /// 浏览器设备码。
    pub browser: bool,
    /// 已启用的身份源（协议 v21）：组织单点登录与 Google。
    pub providers: Vec<LoginProviderDto>,
    /// 注册准入：`open` / `invite`。
    pub admission: String,
    /// 「忘记密码」能否自助完成（邮件真能投递时为真，统一登录 R5）。
    pub password_reset: bool,
}

/// 一个可用的身份源（`auth.login.providers` 的一项，协议 v21）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LoginProviderDto {
    /// 服务端的 provider id（发起浏览器登录时原样带回）。
    pub id: String,
    /// 按钮上的名字（组织单点登录是部署方填的展示名；Google 恒为 `Google`）。
    pub name: String,
    pub kind: LoginProviderKind,
}

/// 身份源的归属（ADR-171 ①）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LoginProviderKind {
    /// 组织的企业身份提供方（账户由组织托管）。
    Org,
    /// Google（公共身份源：同邮箱即同一账户）。
    Google,
    /// Facebook（公共身份源：不给「邮箱已验证」标记，同邮箱不合并，只在邮箱没有账户时新建）。
    Facebook,
}

/// 这台部署卖不卖（协议 v10）。为假 = 私有化形态：客户端不出现任何升级 / 购买 / 管理订阅入口，
/// 服务端的 billing 端点一律 503 `billing_disabled`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct BillingDto {
    pub enabled: bool,
}

/// `GET /inference/v1/capabilities` 里客户端要用的那几格。
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilitiesDto {
    pub protocol: u32,
    pub min_client_protocol: u32,
    /// 部署完成了没有（ADR-041 ①）。
    pub status: DeploymentStatus,
    /// 此刻**真有可用上游**的能力（wire 名：`chat` / `translate` / `embed` / `transcribe` …）。
    pub capabilities: Vec<String>,
    pub deployment: DeploymentDto,
    pub auth: AuthDto,
    pub billing: BillingDto,
}

/// 部署状态（ADR-041 ①）。`setup_incomplete` = 一个上游都没登记，是一等状态，不能由
/// 「`capabilities` 为空」推出来（空也可能是上游全挂了）——故单独读这一格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Ready,
    SetupIncomplete,
}

/// 一台部署对所有账户都一样的事实（取自 capabilities，按服务器桶记）。与账户权益分开：
/// 「组织给不给这个账户开放」和「这台服务端到底配没配上游」是两个人的两个决定，
/// 界面要说出是哪一个（ADR-041 ①、统一登录 R4b）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentFacts {
    /// 卖不卖（协议 v10）。
    pub billing_enabled: bool,
    /// 一个上游都没登记（ADR-041 ①）。
    pub setup_incomplete: bool,
    /// 真有可用上游的能力（wire 名）。
    pub served: Vec<String>,
}

impl DeploymentFacts {
    /// 这台部署此刻跑得了这项能力吗（wire 名）。
    #[must_use]
    pub fn serves(&self, capability: &str) -> bool {
        self.served.iter().any(|c| c == capability)
    }
}

impl From<&CapabilitiesDto> for DeploymentFacts {
    fn from(caps: &CapabilitiesDto) -> Self {
        Self {
            billing_enabled: caps.billing.enabled,
            setup_incomplete: caps.status == DeploymentStatus::SetupIncomplete,
            served: caps.capabilities.clone(),
        }
    }
}

/// capabilities 里**只取两侧版本**的那一读：协商必须先于按本客户端的形状解析整份答案。
///
/// 旧服务端缺的正是本客户端新读的字段；先整份解析，失败会被报成 `UnreadableResponse`
/// （「版本不一致，升级到同一版」），而事实是「服务器太旧，请管理员升级」——那一句该由协商说。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ProtocolVersionsDto {
    pub protocol: u32,
    pub min_client_protocol: u32,
}

/// 纯判定：给定两侧版本，得出兼容性。
#[must_use]
pub fn negotiate(server_protocol: u32, min_client_protocol: u32, client: u32) -> Compatibility {
    if client < min_client_protocol {
        return Compatibility::ClientTooOld;
    }
    if server_protocol < client {
        return Compatibility::ServerTooOld;
    }
    Compatibility::Ok
}

/// 服务端可用的一个模型（**用户视角**：不含上游身份）。
///
/// **刻意不带上游名字/id**：用户要回答的是「我能选哪些、能干什么、贵不贵」，
/// 而「这个模型背后是哪家第三方」是运营信息——发给每个终端用户就等于公开供应商清单。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ServiceModelDto {
    /// 服务端目录内的模型标识（与结果 `executed_by` 同一命名空间）。
    /// ADR-048 后客户端不再把它写进分配槽位或请求体。
    pub model: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    /// u32：specta 禁 64 位整数入 TS；上下文窗口远不到 42 亿。
    pub context_window: Option<u32>,
    /// 定价档位（自由文本，部署方自定义命名）。客户端**原样展示、不解释**——
    /// 把 `basic/standard/premium` 硬翻译成中文，部署方换套命名就全乱了。
    pub price_tier: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_version_is_ok() {
        assert_eq!(negotiate(1, 1, 1), Compatibility::Ok);
    }

    /// 服务端要求的最低客户端版本高于我们 → 提示更新客户端。
    #[test]
    fn client_below_server_minimum_is_client_too_old() {
        assert_eq!(negotiate(3, 3, 1), Compatibility::ClientTooOld);
    }

    /// 服务端比本客户端旧 → 判为太旧（v9 起登录方式只按新协议渲染，不留退回分支）。
    #[test]
    fn older_server_is_server_too_old() {
        assert_eq!(
            negotiate(8, 8, CLIENT_PROTOCOL),
            Compatibility::ServerTooOld
        );
    }

    /// 服务端更新但仍接受旧客户端 → 兼容（服务端向后兼容是常态，不该报警）。
    #[test]
    fn newer_server_accepting_old_client_is_ok() {
        assert_eq!(negotiate(5, 1, 2), Compatibility::Ok);
    }

    /// 边界：恰好等于最低要求即通过（`<` 而非 `<=`）。
    #[test]
    fn exactly_at_minimum_is_ok() {
        assert_eq!(negotiate(2, 2, 2), Compatibility::Ok);
    }
}
