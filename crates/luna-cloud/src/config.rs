//! 云客户端配置。
//!
//! - API 根地址：账户记住的那一台服务器（[`crate::account`]）；从未登录过 = 内置官方后端
//!   （release 为 `https://api.lunarca.app`，debug 为联调服务端，理由见 [`DEFAULT_API_BASE`]）。
//! - Lease / 目录两把验签公钥：该服务器的信任锚钉住值；官方后端的两把都随应用编入。
//!   公钥缺失时对应命令如实返回「未验签/无法验签」而不是假装验过。
//! - 调试构建可用 env（`LUNARCA_API_BASE` / `LUNARCA_LEASE_PUBKEY` / `LUNARCA_CATALOG_PUBKEY`）
//!   整体覆盖，供联调与排障；发行构建不认这三个变量。

use ed25519_dalek::VerifyingKey;

/// **生产**的官方 Control Plane 根地址。release 构建的默认值恒为它。
pub const PRODUCTION_API_BASE: &str = "https://api.lunarca.app";

/// **开发**默认根地址（当前的联调服务端，**经反代的 8443**）。
///
/// 之所以要有它：开发构建不该连生产（`api.lunarca.app` 已部署，但它是真实用户的服务端，
/// 联调账户、测试数据与协议硬切都不该落在那里）。debug 指向联调服务端，开发期的每条路
/// 都在一台可以随时重置的机器上走。
///
/// **必须是反代那一端**：部署机装了 Caddy 之后 `:8090` 只绑 `127.0.0.1`，从外部填它只会
/// 得到「连不上」（商业仓 deploy/README「客户端要配什么」）。这个常量曾长期停在 `:8090`，
/// 真机 2026-09-06 的表现是：连接面板写着「内置默认 http://10.0.0.62:8090」、测试必失败，
/// 用户以为是新面板坏了。
const DEV_API_BASE: &str = "https://10.0.0.62:8443";

/// 官方 Control Plane 默认根地址（用户不填地址时用它）。
///
/// **按构建档分开，且 release 恒为生产域名。** 把一个 RFC1918 私网地址当成"公有云"的
/// 默认值发出去，是灾难性的：每一个装了 release 包的用户，他的「Lunarca 云」都指向某人
/// 局域网里的一台机器——轻则登录永远失败，重则把请求发向一台他完全不了解的主机。
/// 这不是"记得改回来"能防住的事（没有任何东西会提醒你），故由
/// `production_default_is_never_a_lan_address` 机器钉死。
pub const DEFAULT_API_BASE: &str = if cfg!(debug_assertions) {
    DEV_API_BASE
} else {
    PRODUCTION_API_BASE
};

/// 联调后端（[`DEV_API_BASE`]）的**目录根公钥**（ADR-169 ⑥）：签在线目录密钥清单的那把离线根钥。
/// 日常换在线目录钥不动它，故编进构建是稳的。公钥不是秘密：它在服务端启动横幅与
/// `GET /v1/trust/keys` 上公开，编进构建与编进地址是同一回事。
const DEV_CATALOG_PUBKEY: &str = "a4489f36cf16efc7f76275d1b789d1c2da3c09f47f533d7b34b52c06c254b0dc";

/// 生产后端（[`PRODUCTION_API_BASE`]）的**目录根公钥**。根私钥只在运营者的加密离线映像里，
/// 生成于 2026-10-23；这串值以商业仓 `docs/operations/catalog-keys.md`「正式根密钥的现状」的
/// 离线记录为准，与生产 `GET /v1/trust/keys` 公布的一致。换根 = 发一个新版本客户端。
const PRODUCTION_CATALOG_PUBKEY: &str =
    "e4e8f62a8d536f70902b1f226f916091ccf7eff849164d2a89ebb2cdaa865aec";

/// 内置官方后端的目录根公钥（ADR-145 ⑥：未连接的纯本地用户在显式动作时**匿名**取官方
/// 签名目录，根公钥随应用编入；ADR-169 ⑥：它只验根签的在线密钥清单）。按构建档与
/// [`DEFAULT_API_BASE`] 成对：debug 指联调后端那把，release 指生产那把。
pub const OFFICIAL_CATALOG_PUBKEY: &str = if cfg!(debug_assertions) {
    DEV_CATALOG_PUBKEY
} else {
    PRODUCTION_CATALOG_PUBKEY
};

/// 联调后端（[`DEV_API_BASE`]）的 **Lease 验签公钥**：签离线租约 JWT 的那把（TDD-14 §3）。
/// 与 `GET /v1/trust/keys` 公布的一致。
const DEV_LEASE_PUBKEY: &str = "97e94e48506c0ba804e44404f11c1a95410fe8f867c1dd6e3ca664f3241c77b7";

/// 生产后端（[`PRODUCTION_API_BASE`]）的 **Lease 验签公钥**，取自生产 `GET /v1/trust/keys`
/// （2026-11-03）。官方后端不走钉住、也就不沿轮换链验（ADR-124），故生产换
/// `LEASE_SIGNING_KEY` = 已发出的客户端一律验不过租约、断网即不可用——换钥前先发一个
/// 编入新公钥的版本。
const PRODUCTION_LEASE_PUBKEY: &str =
    "990ecfc5a27374f874385c3c492695c4c54b0c96802e95a446baecc509b3a4a4";

/// 内置官方后端的 Lease 验签公钥（ADR-158 ②：Lunarca 云的信任锚随应用编入）。
/// 按构建档与 [`DEFAULT_API_BASE`] 成对，口径同 [`OFFICIAL_CATALOG_PUBKEY`]。
///
/// 它曾经缺席：官方后端跳过钉住，而派生只认 env 与钉住，于是正式包登录 Lunarca 云后
/// 租约恒为 `no_pubkey`（真机 2026-11-03：「离线可用」一行写着本版应用没有内置验签公钥）。
pub const OFFICIAL_LEASE_PUBKEY: &str = if cfg!(debug_assertions) {
    DEV_LEASE_PUBKEY
} else {
    PRODUCTION_LEASE_PUBKEY
};

/// 地址就是内置官方后端时，给出编入的 `(lease, catalog)` 两把公钥；别的地址一律不兜底——
/// 拿官方的钥去验组织服务端的租约或目录只会得到一句误导的 `bad_signature`。
fn official_keys(api_base: &str) -> Option<(&'static str, &'static str)> {
    (api_base == DEFAULT_API_BASE).then_some((OFFICIAL_LEASE_PUBKEY, OFFICIAL_CATALOG_PUBKEY))
}

/// env：覆盖 API 根地址（仅调试构建）。
pub const ENV_API_BASE: &str = "LUNARCA_API_BASE";
/// env：Lease 验签公钥（64 hex；仅调试构建）。
pub const ENV_LEASE_PUBKEY: &str = "LUNARCA_LEASE_PUBKEY";
/// env：目录根公钥（64 hex，ADR-169 ⑥；仅调试构建）。
pub const ENV_CATALOG_PUBKEY: &str = "LUNARCA_CATALOG_PUBKEY";

/// 内置官方后端此刻的地址：调试构建可被 `LUNARCA_API_BASE` 整体覆盖（与 [`CloudConfig::derive`] 同一口径——
/// 握手、领取与之后的每个请求都落在同一台上），发行构建恒为 [`DEFAULT_API_BASE`]。
#[must_use]
pub fn official_api_base() -> String {
    official_api_base_with(debug_env(ENV_API_BASE))
}

/// [`official_api_base`] 的纯逻辑：给了覆盖值就用它（去掉尾随 `/`），否则是编入的地址。
#[must_use]
pub fn official_api_base_with(override_base: Option<String>) -> String {
    override_base.map_or_else(
        || DEFAULT_API_BASE.to_owned(),
        |base| base.trim_end_matches('/').to_owned(),
    )
}

/// 读一个调试期覆盖变量；发行构建恒为 `None`（这三个变量不是用户配置入口）。
#[must_use]
pub fn debug_env(key: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(key)
        .ok()
        .map(|raw| raw.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Clone)]
pub struct CloudConfig {
    /// API 根地址（无尾随 `/`）。
    pub api_base: String,
    /// Lease JWT 离线验签公钥；`None` = 无法验签（如实上报，不降级为信任明文）。
    pub lease_pubkey: Option<VerifyingKey>,
    /// 目录 envelope 验签公钥；`None` = 无法验签（目录刷新如实拒绝，不落假验证）。
    pub catalog_pubkey: Option<VerifyingKey>,
}

impl CloudConfig {
    /// 由账户记住的服务器 + 该服务器的钉住公钥派生（调试构建下 env 整体优先）。
    ///
    /// `pinned` = 信任锚钉住的 `(lease, catalog)` 小写 hex；官方后端不钉住，两把都取编入的。
    #[must_use]
    pub fn derive(server_origin: &str, pinned: Option<(&str, &str)>) -> Self {
        let api_base = debug_env(ENV_API_BASE)
            .unwrap_or_else(|| server_origin.to_owned())
            .trim_end_matches('/')
            .to_owned();
        let official = official_keys(&api_base);
        let lease = debug_env(ENV_LEASE_PUBKEY)
            .or_else(|| pinned.map(|(l, _)| l.to_owned()))
            .or_else(|| official.map(|(l, _)| l.to_owned()));
        let catalog = debug_env(ENV_CATALOG_PUBKEY)
            .or_else(|| pinned.map(|(_, c)| c.to_owned()))
            .or_else(|| official.map(|(_, c)| c.to_owned()));
        Self {
            lease_pubkey: lease.as_deref().and_then(parse_pubkey_hex),
            catalog_pubkey: catalog.as_deref().and_then(parse_pubkey_hex),
            api_base,
        }
    }
}

/// 服务端**分桶名** —— 「这是哪一台服务器」的唯一判据。
///
/// 钥匙链凭据与目录状态都按它分桶（见 `client::CloudClient::scoped` 与
/// `catalog::Target::bucket`）。抽到这里是因为两处各写一遍必然分叉：一旦对
/// 「同一台服务端换 http/https 算不算同一台」判断不一致，用户就会遇到「切了个 https
/// 就被登出、目录也从头开始」——而两边都不会有任何东西变红。
///
/// 取 `scheme://` 之后的主机与端口：协议不同不算两台服务器，尾随斜杠也不算。
#[must_use]
pub fn server_bucket(api_base: &str) -> String {
    api_base
        .split("://")
        .nth(1)
        .unwrap_or(api_base)
        .trim_end_matches('/')
        .to_owned()
}

/// 64 位 hex → Ed25519 公钥；格式/曲线点非法一律 `None`（宁缺勿错验）。
pub fn parse_pubkey_hex(hex: &str) -> Option<VerifyingKey> {
    let bytes = decode_hex_32(hex)?;
    VerifyingKey::from_bytes(&bytes).ok()
}

fn decode_hex_32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_hex_rejects_malformed() {
        assert!(parse_pubkey_hex("zz").is_none());
        assert!(parse_pubkey_hex(&"0".repeat(63)).is_none());
        // 全零不是合法曲线点压缩编码之外的情况——ed25519-dalek 接受与否交给它裁决；
        // 这里只锁「长度/字符集非法必拒」。
    }

    /// **「这是不是同一台服务端」的判据只有这一个函数，而它此前没有一条直接用例。**
    ///
    /// 它同时决定三件互不相关的事：钥匙链凭据分桶、目录防回滚水位分桶，以及服务端
    /// 落点下的**嵌入索引身份**（`server:<桶>:<模型>`，TDD-14 §3.3.1 ⑥）。判断放宽一点
    /// （比如把端口也剥掉）就是把两台服务器的向量混进同一份索引——不报错、几乎无法归因；
    /// 收紧一点（比如认 scheme）就是「换了个 https 就被登出、目录也从头再来」。
    /// 两种错法都不会让任何别的用例变红，所以判据必须在这里被逐条钉住。
    #[test]
    fn server_bucket_is_the_single_judgment_of_same_server() {
        // 协议不算：同一台机器换 https 不是换了一台。
        assert_eq!(server_bucket("http://10.0.0.62:8443"), "10.0.0.62:8443");
        assert_eq!(server_bucket("https://10.0.0.62:8443"), "10.0.0.62:8443");
        // 尾随斜杠不算。
        assert_eq!(server_bucket("https://10.0.0.62:8443/"), "10.0.0.62:8443");
        // 端口**算**：反代 8443 与直连 8090 是两条不同的入口，凭据与水位不通用。
        assert_ne!(
            server_bucket("https://10.0.0.62:8443"),
            server_bucket("http://10.0.0.62:8090")
        );
        // 主机自然算。
        assert_ne!(
            server_bucket("https://a.internal"),
            server_bucket("https://b.internal")
        );
        // 没写协议时原样取——不得静默丢掉整个串。
        assert_eq!(server_bucket("10.0.0.62:8443"), "10.0.0.62:8443");
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    /// 账户记住的服务器即生效地址（尾随斜杠去掉）。
    #[test]
    fn the_account_server_is_the_address_in_use() {
        if std::env::var(ENV_API_BASE).is_ok() {
            return;
        }
        let cfg = CloudConfig::derive("http://10.0.0.62:8090/", None);
        assert_eq!(cfg.api_base, "http://10.0.0.62:8090");
    }

    /// 生效公钥就是钉住的那一组——设置里没有第二份可以分叉。
    #[test]
    fn pinned_keys_are_the_keys_in_use() {
        if std::env::var(ENV_LEASE_PUBKEY).is_ok() || std::env::var(ENV_CATALOG_PUBKEY).is_ok() {
            return;
        }
        let cfg = CloudConfig::derive(
            "https://org.example",
            Some((DEV_CATALOG_PUBKEY, DEV_CATALOG_PUBKEY)),
        );
        assert!(cfg.lease_pubkey.is_some() && cfg.catalog_pubkey.is_some());
        let bare = CloudConfig::derive("https://org.example", None);
        assert!(bare.lease_pubkey.is_none() && bare.catalog_pubkey.is_none());
    }

    /// **官方地址派生出的配置必须带两把公钥。** 官方后端跳过钉住（`adopt_handshake`），
    /// 编入值是它唯一的来源；缺了不会有任何别的用例变红，只会让正式包登录后
    /// 「离线可用」恒写 `no_pubkey`（真机 2026-11-03）。
    #[test]
    fn official_backend_derives_both_keys() {
        if std::env::var(ENV_API_BASE).is_ok()
            || std::env::var(ENV_LEASE_PUBKEY).is_ok()
            || std::env::var(ENV_CATALOG_PUBKEY).is_ok()
        {
            return;
        }
        let builtin = CloudConfig::derive(DEFAULT_API_BASE, None);
        assert_eq!(
            builtin.lease_pubkey,
            parse_pubkey_hex(OFFICIAL_LEASE_PUBKEY),
            "官方后端的租约公钥只能来自编入值"
        );
        assert!(builtin.lease_pubkey.is_some());
        assert!(builtin.catalog_pubkey.is_some());
        // 尾随斜杠不改变「这是官方后端」。
        let slashed = CloudConfig::derive(&format!("{DEFAULT_API_BASE}/"), None);
        assert!(slashed.lease_pubkey.is_some());
        // 别的地址不兜底：拿官方的钥验组织服务端的租约只会得到误导的 bad_signature。
        let org = CloudConfig::derive("https://org.example", None);
        assert!(org.lease_pubkey.is_none());
    }

    /// 调试构建用 `LUNARCA_API_BASE` 指向别的服务端时，「Lunarca 云」这个登录目标也跟着指过去：
    /// 否则握手发往编入的联调地址、领取发往覆盖地址，两台对不上，领取回 410（联调 2026-11-06）。
    #[test]
    fn the_official_backend_follows_the_debug_override() {
        assert_eq!(official_api_base_with(None), DEFAULT_API_BASE);
        assert_eq!(
            official_api_base_with(Some("http://127.0.0.1:18282/".into())),
            "http://127.0.0.1:18282"
        );
    }

    /// 官方租约公钥与地址按构建档成对，release 绝不带联调后端那把。
    #[test]
    fn official_lease_key_is_paired_with_the_build_profile() {
        if cfg!(debug_assertions) {
            assert_eq!(OFFICIAL_LEASE_PUBKEY, DEV_LEASE_PUBKEY);
        } else {
            assert_eq!(OFFICIAL_LEASE_PUBKEY, PRODUCTION_LEASE_PUBKEY);
        }
        assert_ne!(DEV_LEASE_PUBKEY, PRODUCTION_LEASE_PUBKEY);
        // 租约钥与目录根钥是两把独立的钥：抄串了会让一侧恒报 bad_signature。
        assert_ne!(PRODUCTION_LEASE_PUBKEY, PRODUCTION_CATALOG_PUBKEY);
        assert_ne!(DEV_LEASE_PUBKEY, DEV_CATALOG_PUBKEY);
        assert!(parse_pubkey_hex(DEV_LEASE_PUBKEY).is_some());
        assert!(parse_pubkey_hex(PRODUCTION_LEASE_PUBKEY).is_some());
    }

    /// 官方目录公钥只给内置官方后端兜底，且与地址按构建档成对。
    #[test]
    fn official_catalog_key_only_backs_the_builtin_backend() {
        // 别的地址不兜底：拿官方的钥去验组织服务端的目录只会得到误导的 bad_signature。
        let org = CloudConfig::derive("https://org.example", None);
        if std::env::var(ENV_CATALOG_PUBKEY).is_err() {
            assert!(org.catalog_pubkey.is_none());
            let builtin = CloudConfig::derive(DEFAULT_API_BASE, None);
            assert_eq!(
                builtin.catalog_pubkey.is_some(),
                std::env::var(ENV_API_BASE).is_err()
            );
        }
        // release 绝不带联调后端那把钥（与地址同一条防漏改）。
        if !cfg!(debug_assertions) {
            assert_eq!(OFFICIAL_CATALOG_PUBKEY, PRODUCTION_CATALOG_PUBKEY);
        }
        assert_ne!(DEV_CATALOG_PUBKEY, PRODUCTION_CATALOG_PUBKEY);
        // 两把都必须是合法的 Ed25519 点：解析失败 = 刷新目录恒报无法验签，不会有别的地方变红。
        assert!(parse_pubkey_hex(DEV_CATALOG_PUBKEY).is_some());
        assert!(parse_pubkey_hex(PRODUCTION_CATALOG_PUBKEY).is_some());
    }

    /// **release 的默认地址绝不能是私网/明文地址。**
    ///
    /// debug 默认指向联调服务端，而这类"记得发版前改回来"的约定从来没有守住过——没有任何
    /// 东西会提醒你。一旦漏改，每个装了 release 包的用户，他的「Lunarca 云」都指向某人局域网里的一台机器。
    #[test]
    fn production_default_is_never_a_lan_address() {
        assert!(
            PRODUCTION_API_BASE.starts_with("https://"),
            "生产地址必须是 https——公有云默认走明文是不可接受的"
        );
        let host = server_bucket(PRODUCTION_API_BASE);
        assert!(
            !host.starts_with(|c: char| c.is_ascii_digit()),
            "生产地址不能是裸 IP（{host}）——那多半是把联调地址漏改带出去了"
        );
        for private in ["10.", "192.168.", "127.", "172.16.", "localhost"] {
            assert!(
                !host.starts_with(private),
                "生产地址指向私网 {host}——release 包会让每个用户去连某人局域网里的机器"
            );
        }
        // release 构建下两者必须相等；debug 下允许分开。
        if !cfg!(debug_assertions) {
            assert_eq!(DEFAULT_API_BASE, PRODUCTION_API_BASE);
        }
    }
}
