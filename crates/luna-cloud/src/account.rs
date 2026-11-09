//! 账户：「登录了哪一台服务器、登录到哪一步」的唯一事实源（统一登录，ADR-155）。
//!
//! # 状态只有三种
//!
//! `未登录` → `登录中`（等浏览器批准）→ `已登录 <服务器>`；退出 / 会话失效回到 `未登录`。
//! 「填了地址但没登录」不是一个状态：服务器只在登录流程里选，退出后**记住**上一次那台
//! （[`AccountStore`]，落 `account.json`），下次登录一键选中；它也决定退出之后公开目录从哪
//! 来（沿用上一台的签名目录，从未登录过则是内置官方后端）。
//!
//! # 服务器与验签公钥从哪来
//!
//! - 服务器：本模块持久化的那一台；从未选过 = 内置官方地址（[`crate::config::official_api_base`]，调试构建可被
//!   `LUNARCA_API_BASE` 覆盖）。
//! - 两把验签公钥：信任锚钉住值（[`crate::trust`]）。官方后端的两把都随应用编入。
//! - 调试构建可用 env 整体覆盖（[`crate::config`]），发行构建不认。
//!
//! 设置里**没有**第二份地址或公钥：同一事实两处存放，迟早一处说 A、一处验 B。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::client::ProfileDto;

/// 应用数据目录下的账户状态文件名。
pub const ACCOUNT_STATE_FILE: &str = "account.json";

/// 记住的那一台服务器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountServer {
    /// 归一化 origin（`scheme://host[:port]`，无尾随 `/`）。
    pub origin: String,
    /// 部署方在控制台设的名字（连接链接或 capabilities 给的）；没有就用地址称呼。
    pub display_name: Option<String>,
}

impl AccountServer {
    /// 内置官方后端。
    #[must_use]
    pub fn official() -> Self {
        Self {
            origin: crate::config::official_api_base(),
            display_name: None,
        }
    }

    /// 是不是内置官方后端（判据只有地址，与目录公钥兜底同一口径）。
    #[must_use]
    pub fn is_official(&self) -> bool {
        self.origin == crate::config::official_api_base()
    }

    #[must_use]
    pub fn dto(&self) -> AccountServerDto {
        AccountServerDto {
            origin: self.origin.clone(),
            display_name: self.display_name.clone(),
            kind: if self.is_official() {
                ServerKind::Official
            } else {
                ServerKind::Organization
            },
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountFile {
    server: Option<AccountServer>,
}

/// 「上一次（或此刻）登录的是哪一台」的持久化。
#[derive(Default)]
pub struct AccountStore {
    path: Mutex<Option<PathBuf>>,
    server: Mutex<Option<AccountServer>>,
}

impl AccountStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入持久化文件并装载（插件 setup 调用）。读不出来 = 从未选过服务器。
    pub fn attach_path(&self, path: PathBuf) {
        let loaded = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AccountFile>(&bytes).ok())
            .and_then(|file| file.server);
        *self.server.lock().expect("account server mutex poisoned") = loaded;
        *self.path.lock().expect("account path mutex poisoned") = Some(path);
    }

    /// 记住的服务器；`None` = 从未选过。
    #[must_use]
    pub fn server(&self) -> Option<AccountServer> {
        self.server
            .lock()
            .expect("account server mutex poisoned")
            .clone()
    }

    /// 此刻生效的服务器：记住的那一台，否则内置官方后端。
    #[must_use]
    pub fn effective_server(&self) -> AccountServer {
        self.server().unwrap_or_else(AccountServer::official)
    }

    /// 选定一台服务器并落盘。写不下去只留一行日志：本次会话照常可用，只是重启后不记得。
    pub fn set_server(&self, server: AccountServer) {
        *self.server.lock().expect("account server mutex poisoned") = Some(server.clone());
        let path = self
            .path
            .lock()
            .expect("account path mutex poisoned")
            .clone();
        let Some(path) = path else { return };
        let Ok(bytes) = serde_json::to_vec(&AccountFile {
            server: Some(server),
        }) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("[luna-cloud] 账户状态写盘失败，重启后不记得上次的服务器：{err}");
        }
    }
}

// ---- 地址归一化 ----

/// 用户输入的服务器地址 → 归一化 origin；不是合法地址回 `None`。
///
/// 缺 scheme 可以无歧义补全：`443` / `8443` 端口补 `https://`；私网 / 环回主机补
/// `http://`（内网部署通常不配证书，补 `https` 等于把人推进 TLS 报错）；其余补 `https://`。
/// 只收 origin——路径由客户端按契约派生，用户填进来的路径只会变成静默的拼接错误。
#[must_use]
pub fn normalize_server_address(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        let authority = trimmed.split('/').next().unwrap_or(trimmed);
        let scheme = if uses_conventional_tls_port(authority) {
            "https"
        } else if is_private_host(authority) {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{trimmed}")
    };
    crate::connect_link::normalize_origin(&with_scheme)
}

/// 私网 / 环回主机（决定缺 scheme 时补哪一个）。
fn is_private_host(authority: &str) -> bool {
    let host = authority.split(':').next().unwrap_or(authority);
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        return true;
    }
    if let Some(last) = host.rsplit('.').next() {
        if matches!(
            last.to_ascii_lowercase().as_str(),
            "local" | "internal" | "lan" | "home"
        ) {
            return true;
        }
    }
    let octets: Vec<u8> = host.split('.').filter_map(|o| o.parse().ok()).collect();
    if octets.len() != 4 || host.split('.').count() != 4 {
        return false;
    }
    match octets[0] {
        10 | 127 => true,
        172 => (16..=31).contains(&octets[1]),
        192 => octets[1] == 168,
        169 => octets[1] == 254,
        _ => false,
    }
}

/// 缺 scheme 时可无歧义判成 TLS 的惯用端口（部署标准用 `:8443` 终止 TLS）。
fn uses_conventional_tls_port(authority: &str) -> bool {
    matches!(
        authority.rsplit_once(':').map(|(_, port)| port),
        Some("443" | "8443")
    )
}

// ---- 展示 DTO（tauri-specta 导出；判定全在这一侧，ADR-003）----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    Official,
    Organization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AccountServerDto {
    pub origin: String,
    pub display_name: Option<String>,
    pub kind: ServerKind,
}

/// 登录到了哪一步。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum AccountPhaseDto {
    SignedOut,
    /// 等用户在浏览器里批准（设备码在 Rust 里轮询，前端只显示与取消）。
    AwaitingBrowser {
        user_code: String,
        verification_uri: String,
    },
    /// 登录撞上了这个账户的设备上限（纵切 3 C5）：选一台移除即接着登录（密码类原样重发，设备码接着
    /// 轮询同一个配对）。令牌与要重发的请求只在 Rust 里，这里只有上限与设备列表。
    DeviceLimit {
        limit: crate::device_limit::DeviceLimitDto,
    },
    /// 已登录。`profile` 读不出来时为 `None`（会话在、档案缓存丢了，界面不编邮箱）。
    SignedIn {
        profile: Option<ProfileDto>,
    },
}

/// 经 `lunarca://connect` 送达、等用户确认的那条链接（原始链接与邀请只留在 Rust 里）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct IncomingLinkDto {
    pub server: String,
    pub display_name: Option<String>,
    pub has_invite: bool,
}

/// 账户全量状态（`account://changed` 每次推它）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct AccountStateDto {
    /// 当前（已登录）或上一次（未登录）的服务器；`None` = 从未登录过。
    pub server: Option<AccountServerDto>,
    pub phase: AccountPhaseDto,
    pub incoming_link: Option<IncomingLinkDto>,
}

/// 这台服务器可用的登录方式（来自 capabilities `auth.login`，登录前即可取到）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SignInMethodsDto {
    /// 应用内邮箱 + 密码。
    pub password: bool,
    /// 浏览器设备码。
    pub browser: bool,
    /// 已启用的身份源（经浏览器完成，ADR-171）：组织单点登录与 Google。
    pub providers: Vec<SignInProviderDto>,
    /// 注册准入：`open` / `invite`。
    pub admission: String,
    /// 「忘记密码」能否自助完成：真 = 应用里申请、重设链接发到邮箱；假 = 找管理员。
    pub password_reset: bool,
}

/// 登录页上的一颗身份源按钮（来自 capabilities `auth.login.providers`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SignInProviderDto {
    /// 发起浏览器登录时原样交回 `account_sign_in_browser`。
    pub id: String,
    pub name: String,
    pub kind: crate::protocol::LoginProviderKind,
}

/// 从哪开始登录。第一步只问邮箱（ADR-158 ①）；服务器默认由 Rust 按邮箱找，「登录到」那一格可改成
/// Lunarca 云或用户自己给的服务器。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignInTarget {
    /// 按邮箱找服务器：这台设备记住的 → 邮箱域名上的 `.well-known/lunarca` → Lunarca 云。
    Discover { email: String },
    /// 就是 Lunarca 云：越过记住的那台。记住的是测试或组织服务器时，这是不用知道 API 地址就能
    /// 回到 Lunarca 云的唯一一步（真机 2026-11-03）。
    Official { email: String },
    /// 「使用其他服务器」：一格输入既收地址也收连接链接。
    Server { email: String, input: String },
    /// 经 URL scheme 送达、此刻挂着的那条链接。
    IncomingLink { email: String },
}

/// `account_sign_in_begin` 的下一步。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SignInStepDto {
    /// 服务器已选定：显示「将登录到 X」，再按 `methods` 问口令。
    ///
    /// `methods = None`：这台还没钉住、系统信任库也不认它，登录前看不到它提供哪些登录方式——
    /// 只提供口令与一次性码（握手本身会建立信任，ADR-158 ③）。`invite` = 链接带了邀请码，
    /// 直接去设口令（码留在 Rust 里，不过 IPC）。
    Ready {
        server: AccountServerDto,
        /// 规整后的邮箱（界面回显用）。
        email: String,
        methods: Option<SignInMethodsDto>,
        invite: bool,
    },
    /// 这台服务器的身份与本机钉住的不一致：先核对、确认，再重新开始登录。
    NeedsTrustConfirmation { trust: crate::trust::TrustStatusDto },
}

/// 口令登录 / 设口令的结果。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PasswordSignInDto {
    SignedIn,
    /// 撞上了设备上限（纵切 3 C5）：账户状态转到 [`AccountPhaseDto::DeviceLimit`]，界面在那里选一台
    /// 移除；挂着的那一次领取只在 Rust 内存里留到选完为止（口令不在其中）。
    DeviceLimitReached,
    /// 口令对上了，而它送来的签名公钥与本机钉住的不一致：先核对、确认，再登录一次。
    NeedsTrustConfirmation {
        trust: Box<crate::trust::TrustStatusDto>,
    },
}

/// 一次性码登录的结果。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CodeSignInDto {
    /// 码对了、这台服务器的身份已钉住：下一步设口令（注册授权只在 Rust 内存里）。
    SetPassword,
    /// 同 [`PasswordSignInDto::NeedsTrustConfirmation`]。
    NeedsTrustConfirmation {
        trust: Box<crate::trust::TrustStatusDto>,
    },
}

/// 要哪一种一次性码（发到邮箱）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CodeKind {
    /// 忘记口令：重设码（24 小时有效）。
    Reset,
    /// 创建账户：注册验证码（准入 `open` 时才发）。
    Signup,
}

/// 账户变化的种类（装配方据此决定要不要重装 Provider）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountChange {
    /// 登录态或服务器变了：算力落点的判据变了，宿主必须重装。
    Session,
    /// 只是展示状态变了（等浏览器、收到链接）：宿主只需广播。
    Display,
    /// 这个账户的付款处境变了（权益 `billing_standing`，纵切 3 C3；新值读
    /// [`crate::client::CloudClient::billing_standing`]）：宿主据此发 / 收催缴与冻结通知，不重装。
    BillingStanding,
    /// 这一次取用量认出的预警轮次与上一次不同（PRD-13-040 ③，纵切 3 C4；新值读
    /// [`crate::client::CloudClient::usage_alerts`]）：宿主据此按落盘标记发 80% / 95% / 用尽 / 上限通知，
    /// 不重装。
    UsageAlerts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_the_scheme_their_shape_implies() {
        assert_eq!(
            normalize_server_address("10.0.0.62:8443").as_deref(),
            Some("https://10.0.0.62:8443")
        );
        assert_eq!(
            normalize_server_address("10.0.0.62:8090").as_deref(),
            Some("http://10.0.0.62:8090")
        );
        assert_eq!(
            normalize_server_address(" ai.corp.example/ ").as_deref(),
            Some("https://ai.corp.example")
        );
        assert_eq!(
            normalize_server_address("http://localhost:8090").as_deref(),
            Some("http://localhost:8090")
        );
    }

    #[test]
    fn only_bare_origins_are_accepted() {
        for bad in [
            "",
            "https://ai.corp.example/v1",
            "https://user:pw@ai.corp.example",
            "https://ai.corp.example?x=1",
            "ftp://ai.corp.example",
            "two words",
        ] {
            assert_eq!(normalize_server_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_remembered_server_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ACCOUNT_STATE_FILE);
        let store = AccountStore::new();
        store.attach_path(path.clone());
        assert_eq!(store.server(), None);
        assert!(store.effective_server().is_official());

        let org = AccountServer {
            origin: "https://ai.corp.example".into(),
            display_name: Some("Corp".into()),
        };
        store.set_server(org.clone());

        let reloaded = AccountStore::new();
        reloaded.attach_path(path);
        assert_eq!(reloaded.server(), Some(org.clone()));
        assert_eq!(
            reloaded.effective_server().dto().kind,
            ServerKind::Organization
        );
    }
}
