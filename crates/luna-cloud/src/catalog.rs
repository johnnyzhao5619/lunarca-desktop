//! 目录客户端状态机（TDD-14 §3.1，PRD-13-031）：
//! `Bundled → Refreshing → Verified(version) | Rejected(reason)`。
//!
//! # 一套基础设施，多份目录（ADR-017）
//!
//! [`CatalogClient<T>`] 对**条目类型**泛型：本地模型目录（通用 JSON，验签后由消费方逐条解析成
//! `luna_model_catalog::ModelEntry`，ADR-145）与能力提示词
//! 目录（`luna_core::CapabilityPromptSet`，ADR-050）
//! 共用同一条验签 / 过期 / 防回滚 / 缓存回退链，只有**端点**与**条目形状**不同。
//! 这正是 ADR-017 说的那件事：不为第二份目录写第二套远程更新逻辑。
//!
//! **两份目录各自持有防回滚水位与缓存**（持久化分桶键 = `服务端桶#目录类别`）。
//! 共用一个水位的话，提示词发到 v9 会让此后任何 v≤9 的模型目录被判 `rollback` 而永久
//! 拒绝——两份目录的版本本来就各走各的，比大小从一开始就是错的（同 `reconfigure`
//! 里那条按服务端分桶的教训，只是维度换成了「哪一份目录」）。
//!
//! 职责边界：刷新只改变「可发现目录」的**已验副本**，不下载、不删除、不分配、
//! 不重建索引（PRD-13-031）；upstream 模型管理 UI 与随包目录（`luna-model-catalog`）
//! 不受影响——M3 的 fetcher 替换只改「字节从哪来」。
//!
//! 校验链（顺序即失败原因优先级）：
//! 1. **严格本地拦截**：发请求前经注入的 [`StrictLocalProbe`] 阻断（Rust 侧硬闸）；
//! 2. Ed25519 验签（`LUNARCA_CATALOG_PUBKEY`，独立于 Lease 公钥）；
//! 3. 过期校验（envelope `expires_at`）；
//! 4. **版本防回滚**：新版本 < 本地已验最高版本 → 拒绝。已验版本持久化在应用数据
//!    目录小文件（经插件 setup 拿 app handle 路径），**即使缓存目录过期也保留**
//!    ——防回滚记忆比缓存有效期长寿。
//!
//! 失败回退：验证失败沿用最近一份仍有效（签名通过且未过期）的本地缓存目录；
//! 没有任何有效缓存 → `Bundled`（用随应用发布的内置目录）。fallback 只发生在
//! 同一签名当前协议内，不解析旧 schema。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::CloudError;

/// 严格本地探针：返回 `true` = 处于严格本地模式，联网动作必须拦截。
/// 由 app-official 装配时注入（从 upstream 设备级设置读取）；luna-cloud 不自造
/// 一套设置系统（业务事实源在 upstream AppState/config，DRY）。
pub type StrictLocalProbe = Arc<dyn Fn() -> bool + Send + Sync>;

/// 应用数据目录下的状态文件名（缓存目录 + 防回滚水位）。
pub const CATALOG_STATE_FILE: &str = "cloud-catalog.json";

// ---- envelope 线格式（镜像服务端 cp-api catalog_envelope，客户端只读不签）----

/// 目录端点（`GET /v1/catalog/local-models`、`/v1/catalog/prompts`）的响应：分离签名。签名对象 = `payload` 解码后的原始 JSON 字节。
///
/// 签名的是服务端的**在线目录密钥**；它是不是可信的那一把，由同一份响应里附带的、离线根密钥签发的
/// 密钥清单（[`SignedCatalogKeys`]）回答（ADR-169 ⑥）。客户端只认根公钥。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCatalog {
    pub payload: String,
    pub signature: String,
    pub keys: SignedCatalogKeys,
}

/// 离线根密钥签发的在线目录密钥清单（ADR-169 ⑥，对应 TUF 的根元数据）：分离签名，签名对象 =
/// `payload` 解码后的原始 JSON 字节（[`CatalogKeysStatement`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCatalogKeys {
    pub payload: String,
    pub signature: String,
}

/// [`CatalogKeysStatement::kind`] 的固定值：根密钥签的别的东西不能被当成密钥清单。
pub const CATALOG_KEYS_KIND: &str = "lunarca-catalog-keys";

/// 密钥清单的内容：此刻被授权签目录的在线公钥。
///
/// **换钥**：根密钥签一份版本更高、同时列新旧两把的清单，过渡后再签一份只列新钥的。
/// **吊销**：签一份版本更高、不再列那把钥的清单——客户端记住见过的最高版本（防回滚水位），
/// 此后有人拿旧清单配那把钥来骗，按版本倒退拒绝。都不需要用户做任何事。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogKeysStatement {
    pub kind: String,
    /// 单调递增；客户端拒收比见过的更低的版本。
    pub version: u64,
    pub issued_at: i64,
    pub expires_at: i64,
    /// 被授权的在线目录公钥（64 位小写 hex）。
    pub keys: Vec<String>,
}

/// 验根密钥签的密钥清单：签名 + 种类 + 过期 + 防回滚（**纯函数**）。
///
/// # Errors
/// `bad_keys`（解不开、根签名不对、种类不对）、`keys_expired`、`keys_revoked`（版本低于水位）。
pub fn verify_catalog_keys(
    signed: &SignedCatalogKeys,
    root: &VerifyingKey,
    now_unix: i64,
    min_version: u64,
) -> Result<CatalogKeysStatement, &'static str> {
    let payload = B64.decode(&signed.payload).map_err(|_| "bad_keys")?;
    let sig_raw = B64.decode(&signed.signature).map_err(|_| "bad_keys")?;
    let sig_bytes: [u8; 64] = sig_raw.try_into().map_err(|_| "bad_keys")?;
    root.verify_strict(&payload, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| "bad_keys")?;
    let statement: CatalogKeysStatement =
        serde_json::from_slice(&payload).map_err(|_| "bad_keys")?;
    if statement.kind != CATALOG_KEYS_KIND {
        return Err("bad_keys");
    }
    if statement.expires_at <= now_unix {
        return Err("keys_expired");
    }
    if statement.version < min_version {
        return Err("keys_revoked");
    }
    Ok(statement)
}

/// 目录类别：端点 + 持久化分桶后缀。
///
/// 逐条写死 wire 值而不是靠派生：它同时是**跨仓契约**（服务端按同一组字符串把发布
/// 归类）与**持久化键**，跟着字段改名悄悄变的话，用户的防回滚水位会在某次升级后
/// 集体归零，而没有任何东西会报错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogKind {
    /// 本地模型目录（`GET /v1/catalog/local-models`，ADR-145）。条目是 `luna-model-catalog` 的
    /// `ModelEntry`（多文件、GGUF 参数、版本区间）。
    LocalModels,
    /// 能力提示词目录（`GET /v1/catalog/prompts`，ADR-050）。
    Prompts,
    /// 主题目录（`GET /v1/catalog/themes`，ADR-159 ③，纵切 7a）。条目是 `luna-content-schema` 的
    /// `ThemeCatalogEntry`，由宿主逐条解析——本 crate 不认识主题。
    Themes,
    /// 模板目录（`GET /v1/catalog/templates`，ADR-159 ③ ⑥，纵切 7b）。条目是 `luna-content-schema` 的
    /// `TemplateCatalogEntry`（包的摘要、能力、包体 sha256 与大小），由宿主逐条解析；包体按 sha256 另取
    /// （[`template_package_path`]）。
    Templates,
}

/// 模板包包体的下载路径（相对服务端根，内容寻址）：目录签名覆盖 sha256，下回来的字节由宿主重算比对，
/// 所以条目里不写下载地址——也就没有「把人引到别的主机」的余地。
#[must_use]
pub fn template_package_path(sha256: &str) -> String {
    format!("/v1/catalog/templates/packages/{sha256}")
}

impl CatalogKind {
    /// 下发端点路径。
    ///
    #[must_use]
    pub fn path(self) -> &'static str {
        match self {
            Self::LocalModels => "/v1/catalog/local-models",
            Self::Prompts => "/v1/catalog/prompts",
            Self::Themes => "/v1/catalog/themes",
            Self::Templates => "/v1/catalog/templates",
        }
    }

    /// 持久化分桶后缀。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalModels => "local_models",
            Self::Prompts => "prompts",
            Self::Themes => "themes",
            Self::Templates => "templates",
        }
    }
}

/// envelope 载荷（验签通过后才解析）。条目类型由目录类别决定。
///
/// `variants`（ADR-050 修订 2026-09-05 ④，灰度）：同一份签名发布里除对照组 `entries` 外的
/// 若干变体，各带权重与受众规则，由客户端**确定性**选一份。它在 payload 里、随签名一起
/// 下发，故版本号仍是一个单调整数，防回滚不变量不动；老客户端不认识这个字段就只用
/// 对照组——`serde(default)` 在这里是「新字段对老读者不可见」的正确形态，不是兼容层。
#[derive(Debug, Deserialize)]
// `bound` 钉住：serde 派生会给带 `T` 的 default 字段附加 `T: Default`，而条目类型不该背它。
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct CatalogEnvelope<T> {
    pub catalog_version: u64,
    pub issued_at: i64,
    pub expires_at: i64,
    pub entries: Vec<T>,
    /// 安全封禁（ADR-159 ⑦）：本机停用这些 id 的已装内容。只有内容类目录（主题）会下发；另两份目录恒空。
    #[serde(default)]
    pub revoked: Vec<CatalogRevocation>,
    #[serde(default)]
    pub variants: Vec<CatalogVariant<T>>,
}

/// 目录信封里的一条安全封禁（`revoked`）。本 crate 不认识内容的 id 写法，原样交给宿主解析。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CatalogRevocation {
    pub id: String,
    pub reason: String,
}

/// 一份灰度变体：`weight` 是**百分比**（全部变体之和 ≤ 100，余下的落对照组）。
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogVariant<T> {
    pub id: String,
    pub weight: u32,
    #[serde(default)]
    pub rules: AudienceRules,
    pub entries: Vec<T>,
}

/// 受众规则：全空 = 对所有人生效。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AudienceRules {
    /// 客户端最低版本（`x.y.z`）；低于它的客户端不进这一变体。
    #[serde(default)]
    pub min_client_version: Option<String>,
    /// 界面语言白名单（`zh-CN` 或主语言 `zh`）；空 = 不限。
    #[serde(default)]
    pub locales: Vec<String>,
}

/// 客户端一侧参与灰度判定的事实（装配方提供）。
#[derive(Debug, Clone)]
pub struct AudienceContext {
    /// 应用版本（`x.y.z`）。
    pub client_version: String,
    /// 界面语言（`zh-CN` 等）；`None` = 未知。
    pub locale: Option<String>,
    /// 灰度桶 `0..100`：**按设备稳定**（同一台机器每次都落同一桶），否则用户每次启动
    /// 换一份提示词，而运营按桶看到的指标也拼不回来。
    pub bucket: u8,
}

/// 这台设备的灰度桶：主机名 + 装配方给的一段稳定盐（如用户目录）→ [`rollout_bucket`]。
///
/// 不联网、不持久化、不含任何可反推身份的东西（sha256 后只取两字节）。
#[must_use]
pub fn device_rollout_bucket(salt: &str) -> u8 {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    rollout_bucket(&format!("{host}|{salt}"))
}

/// 灰度桶的确定性派生：对一段稳定的设备标识取 sha256，首两字节模 100。
#[must_use]
pub fn rollout_bucket(stable_device_key: &str) -> u8 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(stable_device_key.as_bytes());
    let n = u16::from_be_bytes([digest[0], digest[1]]);
    (n % 100) as u8
}

/// `x.y.z` → 可比较的三元组；缺段按 0，非数字整段视为 0（宁可把它当旧版本也不当新版本）。
fn version_tuple(raw: &str) -> (u64, u64, u64) {
    let mut parts = raw
        .trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

impl AudienceRules {
    /// 这份规则对该客户端是否成立。
    #[must_use]
    pub fn matches(&self, ctx: &AudienceContext) -> bool {
        if let Some(min) = &self.min_client_version {
            if version_tuple(&ctx.client_version) < version_tuple(min) {
                return false;
            }
        }
        if !self.locales.is_empty() {
            let Some(locale) = ctx.locale.as_deref() else {
                return false;
            };
            let primary = locale.split(['-', '_']).next().unwrap_or(locale);
            let hit = self.locales.iter().any(|want| {
                want.eq_ignore_ascii_case(locale) || want.eq_ignore_ascii_case(primary)
            });
            if !hit {
                return false;
            }
        }
        true
    }
}

/// 从一份 envelope 里**确定性**选出这台设备该用的条目：变体按声明顺序各占一段**固定**的
/// 桶区间（`[累计, 累计 + weight)`），桶落在哪一段就属于哪一份变体；那一份的受众规则不成立
/// 时**落对照组而不是顺延到下一份**。都不中 → 对照组。返回（条目, 变体 id）。
///
/// 为什么规则不成立时不顺延：顺延会让「变体 b 覆盖哪些设备」随 a 的规则而变——老版本
/// 客户端里 b 占桶 0..20，新版本里 b 占桶 10..30，运营按变体看到的两组人不是同一批设备，
/// 指标从此拼不回来。固定区间下每一份变体的受众恒是「那一段桶 ∩ 规则成立」，
/// 与 LaunchDarkly 的「先 targeting 再按固定桶 rollout」同一语义。
///
/// 纯函数：同一份 envelope × 同一个 [`AudienceContext`] 永远同一结果，故不需要持久化「上次
/// 选了谁」——桶按设备稳定即可。
#[must_use]
pub fn select_variant<T>(
    envelope: CatalogEnvelope<T>,
    ctx: &AudienceContext,
) -> (Vec<T>, Option<String>) {
    let mut cumulative: u32 = 0;
    let bucket = u32::from(ctx.bucket);
    for variant in envelope.variants {
        let upper = cumulative.saturating_add(variant.weight);
        if bucket < upper {
            return if variant.rules.matches(ctx) {
                (variant.entries, Some(variant.id))
            } else {
                (envelope.entries, None)
            };
        }
        cumulative = upper;
    }
    (envelope.entries, None)
}

/// 受众上下文的提供者（装配方注入；`luna-cloud` 不知道应用版本与界面语言存在哪）。
pub type AudienceProvider = Arc<dyn Fn() -> AudienceContext + Send + Sync + 'static>;

// ---- 持久化状态（应用数据目录小文件）----

#[derive(Debug, Serialize, Deserialize)]
struct PersistedCatalog {
    /// 防回滚水位：本地验证过的最高版本（比缓存长寿——缓存过期不清水位）。
    last_verified_version: u64,
    /// 最近一份验签通过的 envelope 原文（原样存取，不重新序列化）。
    payload: String,
    signature: String,
    /// 随它下发的根签密钥清单（原样存取）。缺了它缓存副本重验不过、退回随包目录，水位照样保留。
    #[serde(default)]
    keys: SignedCatalogKeys,
    /// 密钥清单的防回滚水位：验过的最高清单版本（吊销靠它生效，比缓存长寿）。
    #[serde(default)]
    keys_floor: u64,
    /// 该份目录通过验证的时刻（Unix 秒）。
    verified_at: i64,
    /// 上次刷新**尝试**的时刻（成败都记）。
    ///
    /// 此前它只活在内存里，于是每次重启后 `status()` 都报 `last_refresh_at = None`，
    /// 而缓存重验又把状态置成 `Verified` —— 账户页三行当场自相矛盾：
    /// 「目录版本 v3（已验签）／上次刷新 尚未刷新／状态 已验签 v3」。这不是偶发，
    /// 是**每次重启必现**（真机 2026-08-08 坐实）。
    #[serde(default)]
    last_refresh_at: Option<i64>,
    /// 上次刷新的失败原因码；`None` = 上次刷新成功。同样要跨重启保留——
    /// 「上次刷新被拒了，原因是签名不对」这句话重启后消失，用户就再也查不到了。
    #[serde(default)]
    last_error: Option<String>,
}

// ---- 状态机 ----

/// 客户端目录状态（DTO 枚举；`refreshing` 只在刷新进行中可观察到）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogClientState {
    /// 无有效远端目录：使用随应用发布的内置目录。
    #[default]
    Bundled,
    /// 刷新进行中。
    Refreshing,
    /// 持有验签通过、未过期的目录副本。
    Verified,
    /// 上次刷新被拒（原因见 `last_error`）；本地仍沿用最近有效副本或内置目录。
    Rejected,
}

/// 目录状态 DTO（PRD-13-031 桌面展示面：版本 / 上次刷新 / 来源 / 失败原因）。
///
/// u32 时间戳/版本：specta 禁 64 位整数入 TS（与 lease_expires_at 同口径）；
/// 版本为服务端小步单调计数、时间戳 2106 年前值域足够。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CatalogStatusDto {
    pub state: CatalogClientState,
    /// 当前生效（已验签且未过期）目录版本；`None` = 内置目录。
    pub verified_version: Option<u32>,
    /// 当前生效目录通过验证的时刻（Unix 秒）。
    pub verified_at: Option<u32>,
    /// 上次刷新尝试时刻（Unix 秒；成败都记）。
    pub last_refresh_at: Option<u32>,
    /// 上次刷新的失败原因码（`bad_keys` / `keys_expired` / `keys_revoked` / `bad_signature` /
    /// `expired` / `rollback` / `malformed` / `no_pubkey` / `no_active_catalog` /
    /// `invalid_endpoint` / `network` / `server_error`）。
    pub last_error: Option<String>,
    /// 当前是否处于严格本地模式（刷新入口须禁用并说明，PRD-13-031）。
    pub strict_local: bool,
    /// 当前生效目录里这台设备选中的灰度变体 id（ADR-050 修订 ④）；`None` = 对照组。
    pub variant: Option<String>,
}

/// 纯校验函数：根签的密钥清单 → 在线密钥签的目录 → 过期 → 防回滚（状态机与测试共用同一裁决点）。
/// 返回目录与这次用到的密钥清单版本（调用方据此抬水位）。
///
/// # Errors
/// 返回稳定原因码（与 [`CatalogStatusDto::last_error`] 同口径）：先是 [`verify_catalog_keys`] 的
/// 那三个，再是 `malformed` / `bad_signature`（签目录的不是清单里的任何一把）/ `expired` / `rollback`。
pub fn verify_signed_catalog<T: DeserializeOwned>(
    signed: &SignedCatalog,
    root: &VerifyingKey,
    now_unix: i64,
    min_version: u64,
    min_keys_version: u64,
) -> Result<(CatalogEnvelope<T>, u64), &'static str> {
    let keys = verify_catalog_keys(&signed.keys, root, now_unix, min_keys_version)?;
    let payload_bytes = B64.decode(&signed.payload).map_err(|_| "malformed")?;
    let sig_raw = B64.decode(&signed.signature).map_err(|_| "malformed")?;
    let sig_bytes: [u8; 64] = sig_raw.try_into().map_err(|_| "malformed")?;
    let signature = Signature::from_bytes(&sig_bytes);
    let signed_by_listed_key = keys.keys.iter().any(|hex| {
        crate::config::parse_pubkey_hex(hex)
            .is_some_and(|key| key.verify(&payload_bytes, &signature).is_ok())
    });
    if !signed_by_listed_key {
        return Err("bad_signature");
    }
    let envelope: CatalogEnvelope<T> =
        serde_json::from_slice(&payload_bytes).map_err(|_| "malformed")?;
    if envelope.expires_at <= now_unix {
        return Err("expired");
    }
    if envelope.catalog_version < min_version {
        return Err("rollback");
    }
    Ok((envelope, keys.version))
}

#[derive(Default)]
struct Inner {
    state: CatalogClientState,
    /// 防回滚水位（持久化镜像）。
    floor_version: u64,
    /// 密钥清单的防回滚水位（持久化镜像；吊销靠它生效）。
    keys_floor: u64,
    /// 当前生效目录（版本，验证时刻）；缓存过期/缺失时为 `None`。
    verified: Option<(u64, i64)>,
    /// 当前生效目录的原文副本——持久化时原样写回。
    ///
    /// 留在内存里是为了让**刷新失败**这一支也能落盘：失败时没有新 payload，而
    /// 「上次刷新时刻/失败原因」必须跨重启保留。没有它就只能读-改-写文件，
    /// 那是第二处事实源。
    cached: Option<SignedCatalog>,
    /// 本桶**最近一次验签通过**的那一份原文，连同验签时刻——**不随过期作废**。
    ///
    /// `cached` 回答「此刻能拿来装东西的是哪一份」，过期即空；它回答「这台服务端最近一次签给我们的是
    /// 什么」，安全封禁（[`CatalogClient::revoked`]）只认它：封禁若随目录过期失效，一套因安全原因下架的
    /// 主题会在离线几天后悄悄回来，正好反了。持久化写的也是它，故过期的那一份重启后仍在盘上。
    last_signed: Option<(SignedCatalog, i64)>,
    last_refresh_at: Option<i64>,
    last_error: Option<String>,
    /// 当前生效目录里这台设备选中的灰度变体；`None` = 对照组。
    variant: Option<String>,
}

/// 目标服务端（地址 + 目录根公钥）——两者永远成对更换。
#[derive(Clone)]
struct Target {
    api_base: String,
    pubkey: Option<VerifyingKey>,
}

impl Target {
    /// 持久化分桶键 = `服务端桶#目录类别`。「算不算同一台服务器」这一半的判据与钥匙链
    /// 凭据分桶**同一份**（`config::server_bucket`）：两处判断不一致的话，会出现
    /// 「会话还在、目录却从头开始」这种谁也解释不了的半状态。类别后缀见 [`CatalogKind`]。
    fn bucket(&self, kind: CatalogKind) -> String {
        format!(
            "{}#{}",
            crate::config::server_bucket(&self.api_base),
            kind.as_str()
        )
    }
}

/// 目录条目消费者。`Some((version, entries))` = 这一份已验签生效；**`None` = 回到无覆盖**。
///
/// 与 [`StrictLocalProbe`] / `SessionObserver` 同一形态的注入接缝：`luna-cloud` 不知道
/// 「能力提示词」是什么，也不该知道——它只负责在一份目录生效或失效时说一声。
///
/// **`None` 那一支不是可有可无的**：换服务端时新桶可能根本没有缓存，不发这一声的话，
/// 上一台服务端的提示词会静默留在进程里继续生效——屏幕上写的服务器与实际用的模板对不上，
/// 而这正是本仓在会话凭据与目录水位上已经付过两次代价的那类错。
pub type CatalogObserver<T> = Arc<dyn Fn(Option<CatalogDelivery<T>>) + Send + Sync + 'static>;

/// 交给观察者的一份已验签、已选好变体的目录。
///
/// `variant` 与 `entries` **同一次**交出：条目来自哪一份变体，是消费方（提示词注册表）
/// 要随版本号一起记进账本的事实，事后再问目录客户端拿到的可能已经是下一次刷新的答案。
#[derive(Debug, Clone)]
pub struct CatalogDelivery<T> {
    pub version: u64,
    /// 选中的灰度变体 id；`None` = 对照组。
    pub variant: Option<String>,
    pub entries: Vec<T>,
}

/// 目录客户端（tauri 插件托管；`state_path` 在插件 setup 拿到 app 数据目录后注入）。
pub struct CatalogClient<T> {
    kind: CatalogKind,
    http: luna_model_fetch::pinned_tls::PinnedClient,
    /// 当前目标。**可换**（[`Self::reconfigure`]），故是 `RwLock` 而不是裸字段。
    target: RwLock<Target>,
    probe: StrictLocalProbe,
    state_path: Mutex<Option<PathBuf>>,
    inner: Mutex<Inner>,
    observer: Option<CatalogObserver<T>>,
    /// 灰度受众（装配方注入；缺省 = 不参与灰度，只用对照组）。
    audience: AudienceProvider,
}

impl<T: DeserializeOwned + Send + Sync + 'static> CatalogClient<T> {
    #[must_use]
    pub fn new(
        kind: CatalogKind,
        api_base: String,
        pubkey: Option<VerifyingKey>,
        probe: StrictLocalProbe,
    ) -> Self {
        Self {
            kind,
            http: luna_model_fetch::pinned_tls::PinnedClient::default(),
            target: RwLock::new(Target { api_base, pubkey }),
            probe,
            state_path: Mutex::new(None),
            inner: Mutex::new(Inner::default()),
            observer: None,
            // 缺省桶取 100：任何变体的累计权重都到不了它，故没注入受众的客户端恒用
            // 对照组——「不参与灰度」必须是默认态，而不是碰巧落进第一份变体。
            audience: Arc::new(|| AudienceContext {
                client_version: "0.0.0".to_owned(),
                locale: None,
                bucket: 100,
            }),
        }
    }

    /// 注入灰度受众（装配期一次性）。
    #[must_use]
    pub fn with_audience(mut self, audience: AudienceProvider) -> Self {
        self.audience = audience;
        self
    }

    /// 注入条目消费者（装配期一次性，故按值构造而不是运行期可换）。
    #[must_use]
    pub fn with_observer(mut self, observer: CatalogObserver<T>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn notify(&self, payload: Option<CatalogDelivery<T>>) {
        if let Some(observer) = &self.observer {
            observer(payload);
        }
    }

    fn target(&self) -> Target {
        self.target
            .read()
            .expect("catalog target rwlock poisoned")
            .clone()
    }

    /// 换服务端：切目标 + **切分桶**（新服务端的历史状态从它自己的桶里装载）。
    ///
    /// 分桶是热切逼出来的**真缺陷修复**，不是锦上添花。防回滚水位此前存在单一记录里
    /// 且"比缓存长寿、永不清除"——从目录 v3 的服务端切到 v1 的服务端，新目录会被
    /// `verify_signed_catalog` 判成 `rollback` 而**永久拒绝**，用户只能手删状态文件。
    /// 防回滚的语义本来就是"同一台服务端不得倒退"，跨服务端比大小从一开始就是错的。
    pub fn reconfigure(&self, api_base: String, pubkey: Option<VerifyingKey>) {
        {
            let mut guard = self.target.write().expect("catalog target rwlock poisoned");
            *guard = Target { api_base, pubkey };
        }
        {
            let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
            *inner = Inner::default();
        }
        // 先撤销旧服务端那一份，再装载新桶。反过来（或干脆不撤销）的话，新桶没有缓存
        // 时上一台服务端的条目会静默继续生效——见 [`CatalogObserver`] 的 `None` 那条注释。
        self.notify(None);
        let path = self
            .state_path
            .lock()
            .expect("catalog state_path mutex poisoned")
            .clone();
        if let Some(path) = path {
            self.load_bucket(&path);
        }
    }

    /// 注入持久化文件路径并装载既有状态（插件 setup 调用；测试直接调）。
    /// 装载即重验：缓存签名/过期不合格只降级展示，不阻塞启动。
    pub fn attach_state_path(&self, path: PathBuf) {
        *self
            .state_path
            .lock()
            .expect("catalog state_path mutex poisoned") = Some(path.clone());
        self.load_bucket(&path);
    }

    /// 从状态文件里装载**当前服务端那一桶**。
    fn load_bucket(&self, path: &PathBuf) {
        let target = self.target();
        let Some(persisted) =
            read_state_file(path).and_then(|mut file| file.remove(&target.bucket(self.kind)))
        else {
            return;
        };
        let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
        // 防回滚水位无条件恢复（比缓存有效期长寿）——**在本服务端这一桶内**。
        inner.floor_version = persisted.last_verified_version;
        inner.keys_floor = persisted.keys_floor;
        // 刷新历史（时刻 + 失败原因）**无条件恢复**，且与缓存是否重验通过无关：
        // 「上次什么时候刷的」是一件已经发生的事实，不该因为缓存这一刻恰好过期就消失。
        // 不恢复它正是三行自相矛盾的来源（见 `PersistedCatalog::last_refresh_at`）。
        inner.last_refresh_at = persisted.last_refresh_at;
        inner.last_error = persisted.last_error;
        // 缓存目录重验：通过 → Verified；不通过（过期/公钥换了）→ 保持 Bundled。
        if let Some(pubkey) = &target.pubkey {
            let signed = SignedCatalog {
                payload: persisted.payload,
                signature: persisted.signature,
                keys: persisted.keys,
            };
            // 过期与否都留着（封禁不随过期失效）；它到底验不验得过，由读它的 `revoked()` 按当时的时刻判。
            if !signed.payload.is_empty() {
                inner.last_signed = Some((signed.clone(), persisted.verified_at));
            }
            // 密钥清单的水位照用：被吊销的那一版清单配的缓存，重启后同样不认。
            if let Ok((envelope, _)) =
                verify_signed_catalog::<T>(&signed, pubkey, now_unix(), 0, inner.keys_floor)
            {
                let version = envelope.catalog_version;
                inner.verified = Some((version, persisted.verified_at));
                inner.state = CatalogClientState::Verified;
                inner.cached = Some(signed);
                // **缓存重验通过也要交条目**：不交的话，离线启动时提示词会静默退回
                // 随包默认，而界面（与 `status()`）却说「已验签 vN」——两句话不可能同时为真。
                let (entries, variant) = select_variant(envelope, &(self.audience)());
                inner.variant = variant.clone();
                self.notify(Some(CatalogDelivery {
                    version,
                    variant,
                    entries,
                }));
            }
        }
    }

    /// 受众变了（界面语言切换）→ 用缓存的那份目录**重选一次**变体并重新交付。
    ///
    /// 不联网：只对本桶已验签的副本重跑 [`select_variant`]。没有缓存、验签不过或选中的
    /// 变体没变时什么都不做——「换语言」不该表现成一次目录刷新。
    pub fn reselect(&self) {
        let target = self.target();
        let Some(pubkey) = &target.pubkey else {
            return;
        };
        let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
        let Some(signed) = inner.cached.clone() else {
            return;
        };
        let Ok((envelope, _)) =
            verify_signed_catalog::<T>(&signed, pubkey, now_unix(), 0, inner.keys_floor)
        else {
            return;
        };
        let version = envelope.catalog_version;
        let (entries, variant) = select_variant(envelope, &(self.audience)());
        if variant == inner.variant {
            return;
        }
        inner.variant = variant.clone();
        drop(inner);
        self.notify(Some(CatalogDelivery {
            version,
            variant,
            entries,
        }));
    }

    /// 此刻生效的那份目录（**零网络**）：对本桶已验签的副本重验一次、按当前受众选变体。
    ///
    /// 给只在用户打开界面时才要条目的消费者（主题目录的「获取更多」）用：不必注入观察者、另存一份
    /// 条目——缓存只有这一份事实源。没有缓存、验不过或已过期时为 `None`：过期的目录不该再拿来装东西。
    pub fn current(&self) -> Option<CatalogDelivery<T>> {
        let target = self.target();
        let pubkey = target.pubkey.as_ref()?;
        let inner = self.inner.lock().expect("catalog inner mutex poisoned");
        let signed = inner.cached.clone()?;
        let (envelope, _) =
            verify_signed_catalog::<T>(&signed, pubkey, now_unix(), 0, inner.keys_floor).ok()?;
        drop(inner);
        let version = envelope.catalog_version;
        let (entries, variant) = select_variant(envelope, &(self.audience)());
        Some(CatalogDelivery {
            version,
            variant,
            entries,
        })
    }

    /// 这台服务端最近一次签给我们的安全封禁（**零网络**，**不随目录过期失效**）：按当时验签的那一刻重验
    /// [`Inner::last_signed`]——签名、密钥清单与它的防回滚水位照验，只是「此刻是否已过期」不再作为条件。
    /// 没有验过任何一份、或换了服务端 / 换钥重钉之后为空。
    pub fn revoked(&self) -> Vec<CatalogRevocation> {
        let target = self.target();
        let Some(pubkey) = target.pubkey.as_ref() else {
            return Vec::new();
        };
        let inner = self.inner.lock().expect("catalog inner mutex poisoned");
        let Some((signed, verified_at)) = inner.last_signed.clone() else {
            return Vec::new();
        };
        verify_signed_catalog::<serde_json::Value>(
            &signed,
            pubkey,
            verified_at,
            0,
            inner.keys_floor,
        )
        .map(|(envelope, _)| envelope.revoked)
        .unwrap_or_default()
    }

    /// **换钥重钉时清空本桶的防回滚水位与缓存**（三重重置的前两项，ADR-071）。
    ///
    /// 不重置水位的话，一次**合法**的重部署（新机器、新密钥、目录版本从 1 重新开始）
    /// 会让此后每一份目录都被判成 `rollback` 而**永久拒绝**——用户确认了换钥、界面说
    /// 信任已重建，而目录再也刷不动，且那句 `rollback` 完全指不到这里。
    /// 缓存同样要清：它是**用旧身份的钥**验过的一份副本，留着就是让新身份继承旧断言。
    ///
    /// 与 [`Self::reconfigure`] 的分工：那个是「换了一台服务器」（切桶，旧桶原样留着，
    /// 切回去仍认得）；这个是「同一台服务器换了身份」（同一个桶，就地清零）。
    pub fn reset_for_repin(&self) {
        {
            let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
            *inner = Inner::default();
        }
        // 先撤销旧身份验过的那一份条目，理由同 `reconfigure`：不撤销的话，上一份用旧钥
        // 验过的提示词会静默继续生效。
        self.notify(None);
        let inner = self.inner.lock().expect("catalog inner mutex poisoned");
        self.persist(&inner);
    }

    /// 当前状态快照（`catalog_status` 命令）。
    pub fn status(&self) -> CatalogStatusDto {
        let inner = self.inner.lock().expect("catalog inner mutex poisoned");
        CatalogStatusDto {
            state: inner.state,
            verified_version: inner.verified.map(|(v, _)| clamp_u32(v)),
            verified_at: inner.verified.map(|(_, at)| clamp_u32_i64(at)),
            last_refresh_at: inner.last_refresh_at.map(clamp_u32_i64),
            last_error: inner.last_error.clone(),
            strict_local: (self.probe)(),
            variant: inner.variant.clone(),
        }
    }

    /// 显式刷新（「应用」页头「检查更新」经 [`crate::commands::Catalogs::refresh_checking_trust`]，ADR-170 ③）。
    ///
    /// # Errors
    /// 严格本地模式 → [`CloudError::StrictLocalBlocked`]（发请求前阻断，状态不变）。
    /// 其余失败**不是 Err**：进入 `Rejected(reason)` 并沿用本地有效副本——
    /// 拒绝是状态机的合法终点，不是命令错误。
    pub async fn refresh(&self) -> Result<CatalogStatusDto, CloudError> {
        if (self.probe)() {
            return Err(CloudError::StrictLocalBlocked);
        }
        {
            let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
            inner.state = CatalogClientState::Refreshing;
        }

        let outcome = self.fetch_and_verify().await;
        let now = now_unix();
        let mut inner = self.inner.lock().expect("catalog inner mutex poisoned");
        inner.last_refresh_at = Some(now);
        match outcome {
            Ok((envelope, keys_version, signed)) => {
                let version = envelope.catalog_version;
                inner.floor_version = version;
                inner.keys_floor = inner.keys_floor.max(keys_version);
                inner.verified = Some((version, now));
                inner.state = CatalogClientState::Verified;
                inner.last_error = None;
                inner.last_signed = Some((signed.clone(), now));
                inner.cached = Some(signed);
                let (entries, variant) = select_variant(envelope, &(self.audience)());
                inner.variant = variant.clone();
                self.notify(Some(CatalogDelivery {
                    version,
                    variant,
                    entries,
                }));
            }
            Err(reason) => {
                inner.state = CatalogClientState::Rejected;
                inner.last_error = Some(reason);
                // 缓存副本与水位不动：沿用最近一份有效目录（TDD-14 §3.1 fallback）。
            }
        }
        // 成败**都落盘**。只在成功时写的话，「上次刷新」与失败原因活不过一次重启，
        // 于是重启后界面会说「已验签 v3 / 尚未刷新」——两句话不可能同时为真。
        self.persist(&inner);
        drop(inner);
        Ok(self.status())
    }

    /// 后台同步（ADR-050 修订 2026-09-05 ③）：上一次刷新距今超过 `max_age_secs` 才真的去拉，
    /// 否则什么都不做——stale-while-revalidate 的「revalidate」那一半；「stale 照用」那一半
    /// 本就是状态机的既定语义（本地副本在下一份验签通过之前一直生效）。
    ///
    /// **联网姿态由调用方守**：本函数不判「有没有连接服务端」，那是宿主的门控
    /// （`ConnectedProbe`）；严格本地照旧在发请求前拦下（返回 `None`，不进 `Rejected`——
    /// 后台自发的一次尝试不该改写用户看到的「上次刷新」）。`max_age_secs = 0` = 立刻刷。
    ///
    /// 返回 `Some(status)` = 这一次真的刷了；`None` = 本次没有动作。
    pub async fn refresh_if_stale(&self, max_age_secs: i64) -> Option<CatalogStatusDto> {
        if (self.probe)() {
            return None;
        }
        let last = self
            .inner
            .lock()
            .expect("catalog inner mutex poisoned")
            .last_refresh_at;
        if !is_stale(last, now_unix(), max_age_secs) {
            return None;
        }
        self.refresh().await.ok()
    }

    /// 从这一份目录所在的服务端整块取回一个附属包体（模板包，[`template_package_path`]）：同一个 pinned TLS
    /// 客户端、同一个目标服务端，严格本地在发请求前拦下。读到 `max_bytes` 以上即停——上限按真读到的字节数
    /// 裁决，不信响应头。完整性由调用方拿目录签名覆盖的 sha256 比对，这里只管把字节取回来。
    ///
    /// # Errors
    /// 严格本地 → [`CloudError::StrictLocalBlocked`]；连不上 → [`CloudError::Network`]；非 2xx（含 404：包不在
    /// 当前生效目录里）→ [`CloudError::Server`]；超过上限 → [`CloudError::UnreadableResponse`]。
    pub async fn fetch_package(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, CloudError> {
        if (self.probe)() {
            return Err(CloudError::StrictLocalBlocked);
        }
        let target = self.target();
        let base = target.api_base.as_str();
        let mut response = self
            .http
            .for_url(base)
            .get(format!("{base}{path}"))
            .send()
            .await
            .map_err(|err| CloudError::network(&err, base))?;
        if !response.status().is_success() {
            return Err(CloudError::Server {
                status: response.status().as_u16(),
                error_code: String::new(),
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| CloudError::body(&err, base))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > max_bytes {
                return Err(CloudError::UnreadableResponse {
                    host: crate::config::server_bucket(base),
                    detail: format!("package is larger than {max_bytes} bytes"),
                });
            }
        }
        Ok(bytes)
    }

    /// 网络取回 + 校验链。返回 Err(原因码字符串)。
    async fn fetch_and_verify(&self) -> Result<(CatalogEnvelope<T>, u64, SignedCatalog), String> {
        // 无公钥则不发请求：取回也无法验证，如实拒绝（不落「取回但没验」的假状态）。
        let target = self.target();
        let Some(pubkey) = &target.pubkey else {
            return Err("no_pubkey".to_owned());
        };
        let response = self
            .http
            .for_url(&target.api_base)
            .get(format!("{}{}", target.api_base, self.kind.path()))
            .send()
            .await
            // builder 失败 = 地址拼不成 URL，请求没发出去。与"连不上"分开报，
            // 否则用户拿着一句「网络连接失败」去查网络，而问题在地址少了 scheme。
            .map_err(|err| {
                if err.is_builder() {
                    "invalid_endpoint".to_owned()
                } else {
                    "network".to_owned()
                }
            })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err("no_active_catalog".to_owned());
        }
        if !response.status().is_success() {
            return Err("server_error".to_owned());
        }
        let signed: SignedCatalog = response.json().await.map_err(|_| "malformed".to_owned())?;
        let (floor, keys_floor) = {
            let inner = self.inner.lock().expect("catalog inner mutex poisoned");
            (inner.floor_version, inner.keys_floor)
        };
        let (envelope, keys_version) =
            verify_signed_catalog::<T>(&signed, pubkey, now_unix(), floor, keys_floor)
                .map_err(|reason| reason.to_owned())?;
        Ok((envelope, keys_version, signed))
    }

    /// 写状态文件（尽力而为：写失败只丢持久化，不影响本次内存态；下次刷新重写）。
    ///
    /// 参数是**整个内存态**而不是"这次成功的那份目录"：刷新失败时同样要落盘
    /// （见 [`Self::refresh`]），那一支手上没有新 payload，只能从内存里取上一份。
    fn persist(&self, inner: &Inner) {
        let path = self
            .state_path
            .lock()
            .expect("catalog state_path mutex poisoned")
            .clone();
        let Some(path) = path else { return };
        // 从未验成功过（首次刷新即失败）：没有 payload 可写，也就没有"已验副本"可言。
        // 此时仍写一条只含刷新历史的记录——用户问的「上次刷新是什么时候、为什么被拒」
        // 恰恰是这一档最需要的答案。
        let (payload, signature, keys) = match inner.last_signed.as_ref().map(|(signed, _)| signed)
        {
            Some(signed) => (
                signed.payload.clone(),
                signed.signature.clone(),
                signed.keys.clone(),
            ),
            None => (String::new(), String::new(), SignedCatalogKeys::default()),
        };
        let persisted = PersistedCatalog {
            last_verified_version: inner.floor_version,
            payload,
            signature,
            keys,
            keys_floor: inner.keys_floor,
            verified_at: inner.last_signed.as_ref().map_or(0, |(_, at)| *at),
            last_refresh_at: inner.last_refresh_at,
            last_error: inner.last_error.clone(),
        };
        // 读-改-写：只替换当前服务端那一桶，别的服务端的水位与缓存原样留着
        // （切回去时仍是"已验签 v3"，而不是从零开始又被判一次回滚）。
        let mut file = read_state_file(&path).unwrap_or_default();
        file.insert(self.target().bucket(self.kind), persisted);
        if let Ok(bytes) = serde_json::to_vec(&file) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // 写失败静默降级（下次刷新重写；防回滚水位丢失的最坏后果 = 退回内置目录，
            // 不产生任何错误的「已验」声明）。
            let _ = std::fs::write(&path, bytes);
        }
    }
}

/// 读状态文件 → 「服务端 → 状态」映射。
///
/// 解析失败（文件缺失/损坏/格式不认）一律当作"没有状态"：目录状态是**可重建的缓存**
/// （下一次刷新即恢复），不值得为它写恢复逻辑，更不值得为它留一条只跑一次的旧格式分支。
fn read_state_file(path: &PathBuf) -> Option<BTreeMap<String, PersistedCatalog>> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<BTreeMap<String, PersistedCatalog>>(&bytes).ok()
}

fn clamp_u32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

fn clamp_u32_i64(v: i64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// 当前 Unix 秒。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 后台同步的默认新鲜度：距上次刷新超过这么久才再去拉（6 小时）。
///
/// 目录是运营侧低频变更的配置；同类工具（Langfuse SDK 60 秒 TTL、Firebase Remote Config
/// 默认 12 小时）取值差两个量级，我们取中间：一天最多四次、一次一个 GET，任何用户都
/// 感觉不到，而运营发布后半个工作日内全体生效。
pub const AUTO_REFRESH_MAX_AGE_SECS: i64 = 6 * 60 * 60;

/// 打开「应用」或「本地模型」页时的重验阈值（ADR-170 ②）：距上次刷新超过 5 分钟才再拉。界面先用缓存渲染，
/// 来回切页签不至于变成一串请求；运营刚发布的内容，用户重新打开一次就能看到。
pub const ON_OPEN_MAX_AGE_SECS: i64 = 5 * 60;

/// 「该不该再拉一次」的纯判定：从未刷新过 = 陈旧；否则看距上次刷新是否超过 `max_age`。
/// 时钟倒拨（`last > now`）按陈旧处理——宁可多拉一次也不要一个永远「新鲜」的副本。
#[must_use]
pub fn is_stale(last_refresh_at: Option<i64>, now: i64, max_age_secs: i64) -> bool {
    match last_refresh_at {
        None => true,
        Some(last) => last > now || now - last >= max_age_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    /// 测试用的目录根密钥（离线那一把的替身）。
    fn test_root() -> SigningKey {
        SigningKey::from_bytes(&[11u8; 32])
    }

    /// 根签一份密钥清单。
    fn keys_signed(
        root: &SigningKey,
        keys: &[&SigningKey],
        version: u64,
        expires_at: i64,
    ) -> SignedCatalogKeys {
        let statement = CatalogKeysStatement {
            kind: CATALOG_KEYS_KIND.to_owned(),
            version,
            issued_at: 1000,
            expires_at,
            keys: keys
                .iter()
                .map(|key| {
                    key.verifying_key()
                        .to_bytes()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect()
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&statement).unwrap();
        SignedCatalogKeys {
            payload: B64.encode(&bytes),
            signature: B64.encode(root.sign(&bytes).to_bytes()),
        }
    }

    fn signed_catalog(key: &SigningKey, version: u64, expires_at: i64) -> SignedCatalog {
        let payload = serde_json::json!({
            "catalog_version": version,
            "issued_at": 1000,
            "expires_at": expires_at,
            "entries": [{
                "id": "m", "name": "M", "task": "asr",
                "url": "https://example.com/m.bin",
                "sha256": "0".repeat(64), "size_bytes": 1, "est_on_disk_bytes": 1,
                "license_label": "MIT"
            }],
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let sig = key.sign(&bytes);
        SignedCatalog {
            payload: B64.encode(&bytes),
            signature: B64.encode(sig.to_bytes()),
            keys: keys_signed(&test_root(), &[key], 1, 10_000),
        }
    }

    #[test]
    fn signed_prompt_catalog_carries_every_manifest_domain_into_the_registry() {
        use luna_core::{CapabilityPromptSet, PromptRegistry};

        let registry = PromptRegistry::bundled();
        let entries: Vec<CapabilityPromptSet> = luna_core::prompt::manifest()
            .scopes
            .into_iter()
            .map(|scope| CapabilityPromptSet {
                scope: scope.scope,
                locale: None,
                templates: scope
                    .templates
                    .into_iter()
                    .map(|template| {
                        let body = registry.template(scope.scope, &template.name).unwrap();
                        (template.name, format!("{body} [published]"))
                    })
                    .collect(),
            })
            .collect();
        let payload = serde_json::json!({
            "catalog_version": 7, "issued_at": 1000, "expires_at": 2000,
            "entries": entries,
        });
        let key = SigningKey::from_bytes(&[42; 32]);
        let bytes = serde_json::to_vec(&payload).unwrap();
        let signed = SignedCatalog {
            payload: B64.encode(&bytes),
            signature: B64.encode(key.sign(&bytes).to_bytes()),
            keys: keys_signed(&test_root(), &[&key], 1, 10_000),
        };
        let (envelope, _) = verify_signed_catalog::<CapabilityPromptSet>(
            &signed,
            &test_root().verifying_key(),
            1500,
            6,
            0,
        )
        .unwrap();
        let installed = registry
            .install_overlay(
                envelope.catalog_version.try_into().unwrap(),
                None,
                envelope.entries,
            )
            .unwrap();
        assert_eq!(
            installed.manifest_version,
            luna_core::prompt::PROMPT_MANIFEST_VERSION
        );
        assert!(installed.missing.is_empty());
        for scope in luna_core::prompt::manifest().scopes {
            for template in scope.templates {
                assert!(registry
                    .template(scope.scope, &template.name)
                    .unwrap()
                    .ends_with(" [published]"));
            }
        }
        assert_eq!(registry.version(), 7);
    }

    /// **刷新历史必须跨重启存活**。
    ///
    /// 回归钉子：`last_refresh_at` 此前只活在内存里，于是每次重启后账户页都会同时显示
    /// 「状态：已验签 v3」与「上次刷新：尚未刷新」——两句话不可能同时为真，而用户无从
    /// 判断该信哪一句（真机 2026-08-08 坐实，每次重启必现）。
    /// 灰度选择是确定性的：变体按声明顺序各占一段固定桶区间；落在区间里但规则不成立
    /// → 对照组（**不顺延**到下一份）；都不中回对照组；缺省桶（100）永远只用对照组。
    #[test]
    fn variant_selection_is_deterministic_and_rule_gated() {
        fn envelope() -> CatalogEnvelope<u8> {
            CatalogEnvelope {
                catalog_version: 3,
                issued_at: 0,
                expires_at: 0,
                entries: vec![0],
                revoked: Vec::new(),
                variants: vec![
                    CatalogVariant {
                        id: "a".into(),
                        weight: 10,
                        rules: AudienceRules {
                            min_client_version: Some("1.2.0".into()),
                            locales: vec![],
                        },
                        entries: vec![1],
                    },
                    CatalogVariant {
                        id: "b".into(),
                        weight: 20,
                        rules: AudienceRules {
                            min_client_version: None,
                            locales: vec!["zh".into()],
                        },
                        entries: vec![2],
                    },
                ],
            }
        }
        let ctx = |version: &str, locale: Option<&str>, bucket: u8| AudienceContext {
            client_version: version.to_owned(),
            locale: locale.map(str::to_owned),
            bucket,
        };
        // 桶 5：a 的规则成立（1.3 ≥ 1.2），累计 10 > 5 → a。
        assert_eq!(
            select_variant(envelope(), &ctx("1.3.0", Some("zh-CN"), 5)),
            (vec![1], Some("a".into()))
        );
        // 版本不够：桶 5 仍属 a 的区间，但 a 的规则不成立 → 对照组，**不**顺延给 b。
        assert_eq!(
            select_variant(envelope(), &ctx("1.1.9", Some("zh-CN"), 5)),
            (vec![0], None)
        );
        // 同一台老客户端落在 b 的区间（10..30）→ b：b 的受众不因 a 的规则而挪动。
        assert_eq!(
            select_variant(envelope(), &ctx("1.1.9", Some("zh-CN"), 12)),
            (vec![2], Some("b".into()))
        );
        // 桶 25：a(10) + b(20) = 30 > 25 → b。
        assert_eq!(
            select_variant(envelope(), &ctx("1.3.0", Some("zh-CN"), 25)),
            (vec![2], Some("b".into()))
        );
        // 桶 40：都不中 → 对照组。
        assert_eq!(
            select_variant(envelope(), &ctx("1.3.0", Some("zh-CN"), 40)),
            (vec![0], None)
        );
        // 英文界面：桶 15 属 b 的区间，b 的语言规则不成立 → 对照组。
        assert_eq!(
            select_variant(envelope(), &ctx("1.3.0", Some("en-US"), 15)),
            (vec![0], None)
        );
        // 缺省桶 100：恒对照组。
        assert_eq!(
            select_variant(envelope(), &ctx("9.9.9", Some("zh-CN"), 100)),
            (vec![0], None)
        );
        // 同一设备键恒同一桶，且落在 0..100。
        assert_eq!(rollout_bucket("host|user"), rollout_bucket("host|user"));
        assert!(rollout_bucket("host|user") < 100);
    }

    /// 后台同步的新鲜度判定：从未刷 / 超龄 / 时钟倒拨 → 拉；刚刷过 → 不拉；`0` = 立刻。
    #[test]
    fn staleness_gates_the_background_refresh() {
        let now = 1_000_000;
        assert!(is_stale(None, now, AUTO_REFRESH_MAX_AGE_SECS));
        assert!(!is_stale(Some(now - 60), now, AUTO_REFRESH_MAX_AGE_SECS));
        assert!(is_stale(
            Some(now - AUTO_REFRESH_MAX_AGE_SECS),
            now,
            AUTO_REFRESH_MAX_AGE_SECS
        ));
        assert!(is_stale(Some(now + 60), now, AUTO_REFRESH_MAX_AGE_SECS));
        assert!(is_stale(Some(now), now, 0), "max_age = 0 必须立刻刷");
    }

    /// 后台同步在严格本地下**一个字节都不发、也不改写状态**：与显式刷新不同，它没有
    /// 用户动作可解释，故连 `Rejected(strict_local)` 都不该留下——那会把一次用户没发起的
    /// 尝试写进「上次刷新」。
    #[test]
    fn background_refresh_is_silent_under_strict_local() {
        let client = CatalogClient::<serde_json::Value>::new(
            CatalogKind::LocalModels,
            "http://example.invalid".into(),
            None,
            Arc::new(|| true),
        );
        let outcome = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(client.refresh_if_stale(0));
        assert!(outcome.is_none());
        assert_eq!(client.status().last_refresh_at, None);
    }

    #[test]
    fn refresh_history_survives_restart() {
        let dir = std::env::temp_dir().join(format!("luna-catalog-history-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");
        let _ = std::fs::remove_file(&path);

        // 无公钥 → 刷新在发请求前就以 `no_pubkey` 收尾（零网络，测试可离线跑）。
        let client = CatalogClient::<serde_json::Value>::new(
            CatalogKind::LocalModels,
            "http://example.invalid".into(),
            None,
            Arc::new(|| false),
        );
        client.attach_state_path(path.clone());
        let status = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(client.refresh())
            .expect("非严格本地下刷新不应返回 Err");
        assert_eq!(status.state, CatalogClientState::Rejected);
        assert_eq!(status.last_error.as_deref(), Some("no_pubkey"));
        let refreshed_at = status
            .last_refresh_at
            .expect("刷新尝试过就必须记下时刻，成败都记");

        // 重启：新客户端装载同一份状态文件。
        let reloaded = CatalogClient::<serde_json::Value>::new(
            CatalogKind::LocalModels,
            "http://example.invalid".into(),
            None,
            Arc::new(|| false),
        );
        reloaded.attach_state_path(path.clone());
        let after = reloaded.status();
        assert_eq!(
            after.last_refresh_at,
            Some(refreshed_at),
            "重启后「上次刷新」丢了——界面会退回「尚未刷新」，与状态行自相矛盾"
        );
        assert_eq!(
            after.last_error.as_deref(),
            Some("no_pubkey"),
            "失败原因跨重启丢失 = 用户再也查不到上次为什么被拒"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// **防回滚水位必须按服务端分桶**。
    ///
    /// 水位的语义是"同一台服务端不得倒退"，跨服务端比大小从一开始就是错的。不分桶时，
    /// 从目录 v9 的服务端切到目录 v1 的服务端，新目录会被判 `rollback` 而**永久拒绝**
    /// ——用户只能手删状态文件，而界面只会反复说「服务端目录版本低于本地已验版本」。
    /// 热切让这条从"换了地址重启一次才撞上"变成"当场撞上"，故必须同批修。
    #[test]
    fn rollback_floor_is_scoped_per_server() {
        let dir = std::env::temp_dir().join(format!("luna-catalog-bucket-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");
        let _ = std::fs::remove_file(&path);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let client = CatalogClient::<serde_json::Value>::new(
            CatalogKind::LocalModels,
            "http://a.test".into(),
            None,
            Arc::new(|| false),
        );
        client.attach_state_path(path.clone());
        // 在 A 上刷一次（无公钥 → `no_pubkey`，零网络），把刷新历史落进 A 的桶。
        runtime.block_on(client.refresh()).unwrap();
        assert!(client.status().last_refresh_at.is_some());

        // 换到 B：B 从**自己的空桶**开始，不继承 A 的任何东西。
        client.reconfigure("http://b.test".into(), None);
        let on_b = client.status();
        assert_eq!(
            on_b.last_refresh_at, None,
            "B 继承了 A 的刷新历史——界面会说「上次刷新 …」，而那次刷的是另一台服务器"
        );
        assert_eq!(on_b.state, CatalogClientState::Bundled);

        // 切回 A：A 自己的历史还在（分桶的价值不只是隔离，也是"切回去仍然认得你"）。
        client.reconfigure("http://a.test".into(), None);
        assert!(
            client.status().last_refresh_at.is_some(),
            "切回原服务端后历史丢了——分桶写成了「换一次清一次」"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_chain_orders_failures() {
        let root = test_root().verifying_key();
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let good = signed_catalog(&key, 5, 2000);
        let (envelope, keys_version) =
            verify_signed_catalog::<serde_json::Value>(&good, &root, 1500, 5, 1).unwrap();
        assert_eq!((envelope.catalog_version, keys_version), (5, 1));

        // 根公钥不对 → 密钥清单本身不可信。
        let other_root = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&good, &other_root, 1500, 0, 0).unwrap_err(),
            "bad_keys"
        );
        // 目录由一把清单里没有的钥签 → bad_signature。
        let rogue = SigningKey::from_bytes(&[8u8; 32]);
        let mut by_rogue = signed_catalog(&rogue, 5, 2000);
        by_rogue.keys = keys_signed(&test_root(), &[&key], 1, 10_000);
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&by_rogue, &root, 1500, 0, 0).unwrap_err(),
            "bad_signature"
        );
        // 密钥清单过期 / 版本低于水位（被吊销）。
        let mut stale_keys = signed_catalog(&key, 5, 2000);
        stale_keys.keys = keys_signed(&test_root(), &[&key], 1, 1200);
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&stale_keys, &root, 1500, 0, 0).unwrap_err(),
            "keys_expired"
        );
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&good, &root, 1500, 0, 2).unwrap_err(),
            "keys_revoked"
        );
        // 换钥过渡期：清单同时列新旧两把，两把签的目录都认。
        let next = SigningKey::from_bytes(&[4u8; 32]);
        let mut by_next = signed_catalog(&next, 6, 2000);
        by_next.keys = keys_signed(&test_root(), &[&key, &next], 2, 10_000);
        assert!(verify_signed_catalog::<serde_json::Value>(&by_next, &root, 1500, 5, 1).is_ok());
        // 根签的别的东西不能当密钥清单（种类不对）。
        let mut wrong_kind = good.clone();
        let bytes = br#"{"kind":"other","version":9,"issued_at":0,"expires_at":99999,"keys":[]}"#;
        wrong_kind.keys = SignedCatalogKeys {
            payload: B64.encode(bytes),
            signature: B64.encode(test_root().sign(bytes).to_bytes()),
        };
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&wrong_kind, &root, 1500, 0, 0).unwrap_err(),
            "bad_keys"
        );
        // 过期。
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&good, &root, 2000, 0, 0).unwrap_err(),
            "expired"
        );
        // 回滚：floor 6 > 5。
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&good, &root, 1500, 6, 0).unwrap_err(),
            "rollback"
        );
        // 篡改 payload → 签名失败（先于 schema 解析）。
        let mut tampered = signed_catalog(&key, 5, 2000);
        tampered.payload = B64.encode(br#"{"catalog_version":99}"#);
        assert_eq!(
            verify_signed_catalog::<serde_json::Value>(&tampered, &root, 1500, 0, 0).unwrap_err(),
            "bad_signature"
        );
    }

    /// 吊销跨重启生效：密钥清单的水位与缓存一起落盘，水位以上才认——被吊销那一版清单配的缓存，
    /// 重启后同样退回随包目录（攻击者把旧状态文件塞回来也一样）。
    #[test]
    fn a_retired_key_list_stays_refused_after_restart() {
        let dir = std::env::temp_dir().join(format!("luna-catalog-keys-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let mut cached = signed_catalog(&key, 5, i64::MAX);
        // 重启装载按真实时钟验：清单也要远未过期。
        cached.keys = keys_signed(&test_root(), &[&key], 1, i64::MAX);
        let bucket = Target {
            api_base: "http://keys.test".into(),
            pubkey: None,
        }
        .bucket(CatalogKind::LocalModels);
        let state = |keys_floor: u64| {
            let mut file = BTreeMap::new();
            file.insert(
                bucket.clone(),
                PersistedCatalog {
                    last_verified_version: 5,
                    payload: cached.payload.clone(),
                    signature: cached.signature.clone(),
                    keys: cached.keys.clone(),
                    keys_floor,
                    verified_at: 1000,
                    last_refresh_at: None,
                    last_error: None,
                },
            );
            std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        };
        let load = || {
            let client = CatalogClient::<serde_json::Value>::new(
                CatalogKind::LocalModels,
                "http://keys.test".into(),
                Some(test_root().verifying_key()),
                Arc::new(|| false),
            );
            client.attach_state_path(path.clone());
            client.status().state
        };
        state(1);
        assert_eq!(
            load(),
            CatalogClientState::Verified,
            "清单版本 = 水位：照常可用"
        );
        state(2);
        assert_eq!(
            load(),
            CatalogClientState::Bundled,
            "清单版本低于水位（已被吊销）：缓存不能再生效"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 安全封禁**不随目录过期失效**：重启时缓存已过期，`current()` 没有可装的，`revoked()` 仍交出最近一次签给
    /// 我们的封禁；换一台服务端则不再适用（别的服务端的封禁管不到这里）。
    #[test]
    fn revocations_outlive_catalog_expiry_but_not_a_server_switch() {
        let dir = std::env::temp_dir().join(format!("luna-catalog-revoked-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let payload = serde_json::json!({
            "catalog_version": 3, "issued_at": 1000, "expires_at": 2000, "entries": [],
            "revoked": [{ "id": "com.example.bad", "reason": "misleading name" }],
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let signed = SignedCatalog {
            payload: B64.encode(&bytes),
            signature: B64.encode(key.sign(&bytes).to_bytes()),
            keys: keys_signed(&test_root(), &[&key], 1, i64::MAX),
        };
        let bucket = Target {
            api_base: "http://revoked.test".into(),
            pubkey: None,
        }
        .bucket(CatalogKind::Themes);
        let mut file = BTreeMap::new();
        file.insert(
            bucket,
            PersistedCatalog {
                last_verified_version: 3,
                payload: signed.payload,
                signature: signed.signature,
                keys: signed.keys,
                keys_floor: 1,
                verified_at: 1500,
                last_refresh_at: None,
                last_error: None,
            },
        );
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let client = CatalogClient::<serde_json::Value>::new(
            CatalogKind::Themes,
            "http://revoked.test".into(),
            Some(test_root().verifying_key()),
            Arc::new(|| false),
        );
        client.attach_state_path(path.clone());
        assert!(client.current().is_none(), "目录早已过期：没有可装的");
        assert_eq!(
            client.revoked(),
            vec![CatalogRevocation {
                id: "com.example.bad".into(),
                reason: "misleading name".into(),
            }]
        );
        // 过期的那一份不因重启后的一次落盘而丢：再装载一次仍在。
        client.reset_for_repin();
        assert!(
            client.revoked().is_empty(),
            "换钥重钉：旧身份签的封禁不再适用"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// `current()` 读的是**本桶缓存重验后**的那一份：没有缓存、换了服务端、目录过期都给 `None`
    /// ——过期的目录不该再拿来装东西（主题目录的「获取更多」靠它取条目）。
    #[test]
    fn current_is_the_cached_catalog_reverified_and_nothing_once_it_expires() {
        let dir = std::env::temp_dir().join(format!("luna-catalog-current-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.json");
        let key = SigningKey::from_bytes(&[4u8; 32]);
        let bucket = Target {
            api_base: "http://themes.test".into(),
            pubkey: None,
        }
        .bucket(CatalogKind::Themes);
        let write = |expires_at: i64| {
            let mut cached = signed_catalog(&key, 7, expires_at);
            cached.keys = keys_signed(&test_root(), &[&key], 1, i64::MAX);
            let mut file = BTreeMap::new();
            file.insert(
                bucket.clone(),
                PersistedCatalog {
                    last_verified_version: 7,
                    payload: cached.payload,
                    signature: cached.signature,
                    keys: cached.keys,
                    keys_floor: 1,
                    verified_at: 1000,
                    last_refresh_at: None,
                    last_error: None,
                },
            );
            std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        };
        let client = || {
            let client = CatalogClient::<serde_json::Value>::new(
                CatalogKind::Themes,
                "http://themes.test".into(),
                Some(test_root().verifying_key()),
                Arc::new(|| false),
            );
            client.attach_state_path(path.clone());
            client
        };

        write(i64::MAX);
        let live = client();
        let delivery = live.current().expect("缓存验得过就有一份");
        assert_eq!((delivery.version, delivery.entries.len()), (7, 1));
        live.reconfigure(
            "http://elsewhere.test".into(),
            Some(test_root().verifying_key()),
        );
        assert!(
            live.current().is_none(),
            "换了服务端：上一台的条目不能再拿来装"
        );

        write(now_unix() - 1);
        assert!(client().current().is_none(), "目录过期：不再提供");
        let _ = std::fs::remove_file(&path);
    }

    /// **严格本地是字节前硬闸，不是"刷新完再扔掉结果"**（TDD-14 §3.3.1 ⑩）。
    ///
    /// 这一支此前**零覆盖**：`refresh()` 开头那三行删掉之后，全仓没有任何东西会变红——
    /// 而症状恰恰是这个特性存在的目的的反面：用户按下「仅本地」，界面照常写着"已阻断"，
    /// 一次指向服务端的出站请求却已经发出去了。判据因此必须落在**有没有连接**上，
    /// 而不是落在返回值上。
    ///
    /// 同批钉住 `status().strict_local`：界面据它禁用刷新按钮并给出说明（PRD-13-031），
    /// 它报错了的话，用户会得到一个按下去总是失败的按钮。
    /// 起一个只答一次的本地 HTTP 服务：回 `status` 与 `body`，交回地址与「有没有人连过」。
    fn one_shot_http(
        status: &'static str,
        body: Vec<u8>,
    ) -> (String, std::thread::JoinHandle<bool>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
            while std::time::Instant::now() < deadline {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_nonblocking(false).ok();
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    let head = format!(
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        });
        (format!("http://{address}"), handle)
    }

    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// 模板包包体（纵切 7b）：2xx 的正文原样交回；非 2xx（含 404：不在生效目录里）是服务端错误；超过上限即停；
    /// 严格本地在发请求前拦下——一个字节都不出去。
    #[test]
    fn packages_come_back_whole_capped_and_never_under_strict_local() {
        let client = |base: String, strict: bool| {
            CatalogClient::<serde_json::Value>::new(
                CatalogKind::Templates,
                base,
                None,
                Arc::new(move || strict),
            )
        };
        let (base, served) = one_shot_http("200 OK", b"PK\x03\x04pack".to_vec());
        let bytes = run(client(base, false).fetch_package("/v1/catalog/templates/packages/x", 64));
        assert_eq!(bytes.unwrap(), b"PK\x03\x04pack".to_vec());
        assert!(served.join().unwrap());

        let (base, _) = one_shot_http("404 Not Found", b"{}".to_vec());
        let outcome = run(client(base, false).fetch_package("/missing", 64));
        assert!(
            matches!(outcome, Err(CloudError::Server { status: 404, .. })),
            "{outcome:?}"
        );

        let (base, _) = one_shot_http("200 OK", vec![0; 65]);
        let outcome = run(client(base, false).fetch_package("/big", 64));
        assert!(
            matches!(outcome, Err(CloudError::UnreadableResponse { .. })),
            "{outcome:?}"
        );

        let (base, connected) = one_shot_http("200 OK", b"x".to_vec());
        let outcome = run(client(base, true).fetch_package("/x", 64));
        assert!(
            matches!(outcome, Err(CloudError::StrictLocalBlocked)),
            "{outcome:?}"
        );
        assert!(!connected.join().unwrap(), "严格本地下仍然连上了服务端");
    }

    #[test]
    fn strict_local_blocks_catalog_refresh_before_any_byte() {
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        // 真起一个监听口：它交出的是「有没有人连过我」，而不是任何客户端自述。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("addr");
        let watcher = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                if listener.accept().is_ok() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        });

        // 根公钥配齐：唯一可能拦住这次刷新的理由只剩严格本地。
        let client = CatalogClient::<serde_json::Value>::new(
            CatalogKind::LocalModels,
            format!("http://{address}"),
            Some(test_root().verifying_key()),
            Arc::new(|| true),
        );

        let outcome = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(client.refresh());
        assert!(
            matches!(outcome, Err(CloudError::StrictLocalBlocked)),
            "严格本地下刷新必须以 StrictLocalBlocked 收尾，实际 {outcome:?}"
        );
        assert!(
            !watcher.join().expect("watcher"),
            "严格本地下仍然连上了服务端——字节已经出去了，之后怎么处理结果都无所谓了"
        );

        let status = client.status();
        assert!(
            status.strict_local,
            "状态没报严格本地——界面不会禁用刷新按钮，用户得到一个按下去总是失败的按钮"
        );
        assert_eq!(
            status.state,
            CatalogClientState::Bundled,
            "被拦下的刷新不得改变目录状态：它连 Refreshing 都不该进"
        );
        assert!(
            status.last_refresh_at.is_none(),
            "记了一次「上次刷新」——那会让用户以为我们真的去问过服务端"
        );
    }
}
