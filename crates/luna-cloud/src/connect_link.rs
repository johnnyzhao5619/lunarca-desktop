//! 连接链接（ADR-116，ADR-158 ⑦ 改形）：`lunarca://connect?…`。
//!
//! # 它现在只是一份便捷投递
//!
//! ADR-158 之后，信任不再来自链接：口令（或一次性码）的 OPAQUE 握手会把服务端的 TLS 根证书与
//! 两把签名公钥经会话密钥送达（`crate::first_contact`）。链接只省掉两件事——手填服务器地址、
//! 手抄邀请码；`anchor` / `tls` / `deployment` 随之删除（不再被读）。
//!
//! # 线格式（与商业仓控制台逐字对齐）
//!
//! ```text
//! lunarca://connect?v=2&server=<origin>[&name=<显示名>][&invite=<一次性邀请码>]
//! ```
//!
//! `server` 只到 origin（scheme + host[:port]），路径由客户端按 TDD-16 §5.1 自动拼——与
//! 登录时手填的地址同一约束（`crate::account::normalize_server_address`）。
//!
//! # 本模块只做纯解析
//!
//! 没有 IO，全部可单测。

use serde::{Deserialize, Serialize};

/// 当前支持的链接格式版本。
pub const LINK_VERSION: &str = "2";
/// URL scheme（同时登记在 `tauri.conf.json` 的 deep-link 插件里）。
pub const SCHEME: &str = "lunarca";
/// `host` 段：`lunarca://connect`。
pub const ACTION_CONNECT: &str = "connect";

/// 解析成功的连接链接。`invite` 是一次性邀请码——**不落日志、不进任何持久化**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ConnectLink {
    /// 归一化后的服务端 origin（无尾随 `/`）。
    pub server: String,
    pub display_name: Option<String>,
    pub invite: Option<String>,
}

impl ConnectLink {
    /// 链接指向的那台服务器（名字先用链接给的）。
    #[must_use]
    pub fn server_account(&self) -> crate::account::AccountServer {
        crate::account::AccountServer {
            origin: self.server.clone(),
            display_name: self.display_name.clone(),
        }
    }
}

/// 链接被拒的原因。每一条都指向一个用户能做的动作（同 `OrgConfigRejection` 的纪律）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ConnectLinkRejection {
    /// 不是 `lunarca://connect` 链接（多半粘错了东西）。
    #[error("not a connect link")]
    NotConnectLink,
    /// 链接版本比本客户端新：升级 Lunarca。
    #[error("unsupported link version {found}")]
    UnsupportedVersion { found: String },
    /// 缺必填参数。
    #[error("missing parameter {name}")]
    Missing { name: String },
    /// `server` 不是合法 origin。
    #[error("bad server origin")]
    BadServer,
}

impl ConnectLinkRejection {
    /// 稳定错误码（前端映射文案）。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConnectLink => "CONNECT_LINK_NOT_LINK",
            Self::UnsupportedVersion { .. } => "CONNECT_LINK_UNSUPPORTED_VERSION",
            Self::Missing { .. } => "CONNECT_LINK_MISSING_PARAM",
            Self::BadServer => "CONNECT_LINK_BAD_SERVER",
        }
    }
}

/// 输入看起来是不是一条连接链接（只看前缀；用于「一格输入既收地址也收链接」的分流）。
#[must_use]
pub fn looks_like_link(input: &str) -> bool {
    input
        .trim()
        .to_ascii_lowercase()
        .starts_with(&format!("{SCHEME}://"))
}

/// 解析并校验一条链接。
///
/// # Errors
/// 见 [`ConnectLinkRejection`]。
pub fn parse(input: &str) -> Result<ConnectLink, ConnectLinkRejection> {
    let raw = input.trim();
    let url = reqwest::Url::parse(raw).map_err(|_| ConnectLinkRejection::NotConnectLink)?;
    if url.scheme() != SCHEME || url.host_str() != Some(ACTION_CONNECT) {
        return Err(ConnectLinkRejection::NotConnectLink);
    }
    let mut version = None;
    let mut server = None;
    let mut name = None;
    let mut invite = None;
    for (key, value) in url.query_pairs() {
        let value = value.trim().to_owned();
        match key.as_ref() {
            "v" => version = Some(value),
            "server" => server = Some(value),
            "name" => name = Some(value),
            "invite" => invite = Some(value),
            // 未知参数忽略：将来的链接可以多带东西而不让旧客户端整条拒掉（协议向前兼容
            // 的常规做法；版本号只在**语义**变化时抬）。
            _ => {}
        }
    }
    let version = version.ok_or_else(|| ConnectLinkRejection::Missing { name: "v".into() })?;
    if version != LINK_VERSION {
        return Err(ConnectLinkRejection::UnsupportedVersion { found: version });
    }
    let server = server
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ConnectLinkRejection::Missing {
            name: "server".into(),
        })?;
    let server = normalize_origin(&server).ok_or(ConnectLinkRejection::BadServer)?;
    Ok(ConnectLink {
        server,
        display_name: name.filter(|s| !s.is_empty()),
        invite: invite.filter(|s| !s.is_empty()),
    })
}

/// `server` → 归一化 origin：只认 `http`/`https`，不带路径 / 查询 / 凭据，去尾随 `/`。
pub(crate) fn normalize_origin(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.username() != "" || url.password().is_some() {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    if !matches!(url.path(), "" | "/") {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// 查询串编码（测试用来拼链接；客户端自己从不生成链接）。
#[cfg(test)]
#[must_use]
pub(crate) fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(extra: &str) -> String {
        format!("lunarca://connect?v=2&server=https%3A%2F%2Fai.corp.example%3A8443{extra}")
    }

    #[test]
    fn parses_full_link() {
        let parsed = parse(&link("&name=Acme%20AI&invite=ABCD-1234")).unwrap();
        assert_eq!(parsed.server, "https://ai.corp.example:8443");
        assert_eq!(parsed.display_name.as_deref(), Some("Acme AI"));
        assert_eq!(parsed.invite.as_deref(), Some("ABCD-1234"));
        assert_eq!(parsed.server_account().origin, parsed.server);
    }

    #[test]
    fn optional_parts_absent_are_none_and_old_params_are_ignored() {
        // v1 的 `anchor` / `tls` / `deployment` 不再被读：信任来自握手，不来自链接。
        let parsed = parse(&link("&anchor=zz&tls=xyz&deployment=d&future=1")).unwrap();
        assert_eq!(parsed.display_name, None);
        assert_eq!(parsed.invite, None);
    }

    #[test]
    fn rejects_non_link_and_wrong_action() {
        assert_eq!(
            parse("https://ai.corp.example"),
            Err(ConnectLinkRejection::NotConnectLink)
        );
        assert_eq!(
            parse("lunarca://other?v=2"),
            Err(ConnectLinkRejection::NotConnectLink)
        );
        assert!(!looks_like_link("https://x"));
        assert!(looks_like_link("  LUNARCA://connect?v=2"));
    }

    #[test]
    fn rejects_other_versions_and_missing_params() {
        assert_eq!(
            parse("lunarca://connect?v=1&server=https://a"),
            Err(ConnectLinkRejection::UnsupportedVersion { found: "1".into() })
        );
        assert_eq!(
            parse("lunarca://connect?v=2&name=x"),
            Err(ConnectLinkRejection::Missing {
                name: "server".into()
            })
        );
    }

    #[test]
    fn rejects_bad_server() {
        assert_eq!(
            parse("lunarca://connect?v=2&server=ftp%3A%2F%2Fx"),
            Err(ConnectLinkRejection::BadServer)
        );
        assert_eq!(
            parse("lunarca://connect?v=2&server=https%3A%2F%2Fx%2Fapi"),
            Err(ConnectLinkRejection::BadServer)
        );
    }

    #[test]
    fn origin_is_normalized_without_trailing_slash() {
        assert_eq!(
            normalize_origin("http://10.0.0.62:8090/").as_deref(),
            Some("http://10.0.0.62:8090")
        );
        assert_eq!(
            normalize_origin("https://x.example").as_deref(),
            Some("https://x.example")
        );
    }

    #[test]
    fn query_encoding_covers_invite_alphabet() {
        assert_eq!(encode_query("AB+C=1 x"), "AB%2BC%3D1%20x");
        assert_eq!(encode_query("abc-1_2.3~"), "abc-1_2.3~");
    }
}
