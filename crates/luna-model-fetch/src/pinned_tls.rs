//! 按服务端钉住的 TLS 根证书（ADR-153）。
//!
//! 自签部署（反代用自建根证书）的服务端，系统信任库不认它。用户经连接链接或组织配置文件
//! 拿到这张根证书并核验指纹后，**只对这一台服务端**信任它：不改系统信任库、不跳过校验，
//! 而是给发往这台服务端的请求换一个「只认这张根」的客户端。别的主机照旧走系统信任库。
//!
//! 钉住只在这里登记一份（进程级），持久化与核验在 `luna-cloud::tls_root`。所有发往服务端的
//! HTTP / WebSocket 客户端都经 [`PinnedClient`] 或 [`native_tls_connector`] 取连接——
//! 漏一处，那条通道在自签部署上就是「证书不受信任」。
//!
//! 钉住之后**只认这张根**（关掉系统内置根）：服务端换了证书链，连接如实失败，而不是悄悄
//! 落回系统信任库——那等于根证书变了也不阻断。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// authority（`host[:port]`）→ 根证书 DER。
static PINS: RwLock<BTreeMap<String, Arc<[u8]>>> = RwLock::new(BTreeMap::new());

/// URL → 登记用的 authority。登记与查找走同一个函数，故默认端口写不写都落到同一格。
#[must_use]
pub fn authority_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(match parsed.port_or_known_default() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// 为 `origin` 钉住（`Some(der)`）或撤销（`None`）根证书。
pub fn set_pinned_root(origin: &str, der: Option<Vec<u8>>) {
    let Some(authority) = authority_of(origin) else {
        return;
    };
    let mut pins = PINS.write().unwrap_or_else(PoisonError::into_inner);
    match der {
        Some(der) => {
            pins.insert(authority, der.into());
        }
        None => {
            pins.remove(&authority);
        }
    }
}

/// 发往 `url` 的请求该用哪张根证书；`None` = 走系统信任库。
#[must_use]
pub fn pinned_root(url: &str) -> Option<Arc<[u8]>> {
    let authority = authority_of(url)?;
    PINS.read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&authority)
        .cloned()
}

/// 按钉住状态构建客户端。证书解析失败时**不落回系统信任库**：关掉内置根、不加任何根，
/// 连接如实失败。
fn build(
    configure: fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
    root: Option<&[u8]>,
) -> reqwest::Client {
    let mut builder = configure(reqwest::Client::builder());
    if let Some(der) = root {
        builder = builder.tls_built_in_root_certs(false);
        match reqwest::Certificate::from_der(der) {
            Ok(cert) => builder = builder.add_root_certificate(cert),
            Err(err) => eprintln!("[pinned-tls] 钉住的根证书解析失败，本次连接将失败：{err}"),
        }
    }
    builder
        .build()
        .expect("reqwest client 构造失败（TLS 后端初始化异常）")
}

/// 发往服务端的 HTTP 客户端：按请求地址取「钉住的根」或「系统信任库」那一个，按根证书缓存。
///
/// 钉住可以在运行中变（连上一台新服务端），故不在构造时定死——每次按 URL 取。
pub struct PinnedClient {
    configure: fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
    cache: Mutex<BTreeMap<Option<Arc<[u8]>>, reqwest::Client>>,
}

impl PinnedClient {
    /// `configure` 是各通道自己的超时、连接池等设置（每次重建都套用）。
    #[must_use]
    pub fn new(configure: fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder) -> Self {
        Self {
            configure,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    /// 发往 `url` 用的客户端（克隆廉价，内部共享连接池）。
    #[must_use]
    pub fn for_url(&self, url: &str) -> reqwest::Client {
        let root = pinned_root(url);
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .entry(root.clone())
            .or_insert_with(|| build(self.configure, root.as_deref()))
            .clone()
    }
}

impl Default for PinnedClient {
    fn default() -> Self {
        Self::new(|builder| builder)
    }
}

/// WebSocket 用的 TLS 连接器：`url` 的服务端钉了根证书 → 只认那张根的连接器；没钉 → `None`
/// （走 tungstenite 默认，即系统信任库）。
///
/// # Errors
/// 钉住的证书解析失败或 TLS 后端初始化失败——调用方如实报连接失败，不落回系统信任库。
pub fn native_tls_connector(url: &str) -> Result<Option<native_tls::TlsConnector>, String> {
    let Some(der) = pinned_root(url) else {
        return Ok(None);
    };
    let cert = native_tls::Certificate::from_der(&der).map_err(|e| e.to_string())?;
    native_tls::TlsConnector::builder()
        .disable_built_in_roots(true)
        .add_root_certificate(cert)
        .build()
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ports_and_case_land_in_the_same_slot() {
        assert_eq!(
            authority_of("https://Pin-A.test/v1/x").as_deref(),
            Some("pin-a.test:443")
        );
        assert_eq!(
            authority_of("https://pin-a.test:443").as_deref(),
            Some("pin-a.test:443")
        );
        assert_eq!(
            authority_of("wss://10.0.0.62:8443/v1/asr").as_deref(),
            Some("10.0.0.62:8443")
        );
        assert_eq!(authority_of("not a url"), None);
    }

    #[test]
    fn a_pin_applies_to_its_own_server_only() {
        set_pinned_root("https://pin-b.test:8443", Some(vec![1, 2, 3]));
        assert_eq!(
            pinned_root("https://pin-b.test:8443/v1/me").as_deref(),
            Some(&[1u8, 2, 3][..])
        );
        assert!(
            pinned_root("https://pin-b.test/v1/me").is_none(),
            "端口不同就是另一台"
        );
        assert!(pinned_root("https://huggingface.co/x").is_none());
        set_pinned_root("https://pin-b.test:8443", None);
        assert!(pinned_root("https://pin-b.test:8443/v1/me").is_none());
    }

    #[test]
    fn clients_are_cached_per_root() {
        let pinned = PinnedClient::default();
        let _ = pinned.for_url("https://pin-c.test/a");
        let _ = pinned.for_url("https://other.test/b");
        assert_eq!(
            pinned.cache.lock().unwrap().len(),
            1,
            "都没钉：共用系统信任库那一个"
        );
    }
}
