//! 登录撞上设备上限（纵切 3 C5，协议 v13；用户可见规则见
//! PRD-13-021，客户端形态见
//! TDD-14 §3.9）。
//!
//! # 线上契约
//!
//! 任何一条建设备会话的登录请求都可能回 403：
//! `{"error":"device_limit_reached","max","devices":[{"id","name","platform","last_seen_at"}],
//! "revoke_token","revoke_token_expires_at"}`。`POST /v1/devices/revoke-for-sign-in`
//! `{"revoke_token","device_id"}` → 204 之后，**同一次登录原样再来一次**即成功：应用内登录原样重发
//! 领取会话（或注册口令）那一条请求，设备码接着轮询同一个配对。
//!
//! # 秘密留在 Rust
//!
//! `revoke_token` 与重发要用的请求体（含一次性的领取凭据或注册授权）只活在进程内存里的
//! [`DeviceLimitPending`]：不落盘、不打日志（`Debug` 一律打码）、不过 IPC。界面只拿 [`DeviceLimitDto`]
//! （上限与设备列表）。口令从不在这里（ADR-158：口令只进 OPAQUE 握手）。领取凭据有 120 秒的服务端
//! 时效，选得太久重发会回 `login_expired`，界面回到登录重来一次；选完（成败）、取消或开始另一次登录即丢。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 服务端 403 稳定码：登录撞上了这个账户的设备上限。
pub const DEVICE_LIMIT_REACHED: &str = "device_limit_reached";

/// 设备上限的线上形状（403 体，除 `error` 之外的部分）。
#[derive(Deserialize)]
pub(crate) struct DeviceLimitWire {
    pub(crate) max: u32,
    pub(crate) devices: Vec<LimitedDeviceDto>,
    pub(crate) revoke_token: String,
    pub(crate) revoke_token_expires_at: String,
}

impl fmt::Debug for DeviceLimitWire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceLimitWire")
            .field("max", &self.max)
            .field("devices", &self.devices.len())
            .field("revoke_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// 占着名额的一台设备（界面渲染名称、平台、最近使用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct LimitedDeviceDto {
    pub id: String,
    pub name: String,
    /// `std::env::consts::OS` 口径（同 [`crate::client::DeviceDto::platform`]）；服务端没记为 `null`。
    pub platform: Option<String>,
    /// RFC 3339；从没用过为 `null`。本地化在界面层。
    pub last_seen_at: Option<String>,
}

/// 「已达到设备上限」那一步要渲染的全部事实（不含 `revoke_token`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DeviceLimitDto {
    /// 这个账户最多几台设备。
    pub max: u32,
    pub devices: Vec<LimitedDeviceDto>,
    /// 「移除一台并登录」在这之前有效（RFC 3339，界面只用来提示）。
    pub expires_at: String,
}

/// 撞上限之后要原样再来一次的那次登录。
pub(crate) enum SignInRetry {
    /// 应用内登录的最后一步（领取会话 / 注册口令）：同一条请求原样重发。领取凭据与注册授权是
    /// 一次性的，服务端在签发会话之前停下时不消费它们；口令本身从不在这里——OPAQUE 下它不离开握手。
    Grant {
        path: &'static str,
        body: serde_json::Value,
        /// 注册口令那一条带的 `Authorization: Bearer`（一次性码换来的注册授权）。
        bearer: Option<zeroize::Zeroizing<String>>,
    },
    /// 浏览器设备码：同一个配对接着轮询。
    DeviceCode,
}

/// 撞上限之后、用户选好移除哪一台之前挂着的状态（只在内存里）。
pub(crate) struct DeviceLimitPending {
    pub(crate) revoke_token: String,
    pub(crate) retry: SignInRetry,
    pub(crate) limit: DeviceLimitDto,
}

impl fmt::Debug for DeviceLimitPending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let retry = match &self.retry {
            SignInRetry::Grant { path, .. } => *path,
            SignInRetry::DeviceCode => "device_code",
        };
        f.debug_struct("DeviceLimitPending")
            .field("retry", &retry)
            .field("max", &self.limit.max)
            .field("revoke_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl DeviceLimitPending {
    pub(crate) fn new(wire: DeviceLimitWire, retry: SignInRetry) -> Self {
        Self {
            revoke_token: wire.revoke_token,
            retry,
            limit: DeviceLimitDto {
                max: wire.max,
                devices: wire.devices,
                expires_at: wire.revoke_token_expires_at,
            },
        }
    }
}

/// 一个非 2xx 响应体是不是「撞上设备上限」；是就解出它。别的码、或体读不懂 → `None`（照原处置）。
pub(crate) fn decode(
    status: reqwest::StatusCode,
    body: &serde_json::Value,
) -> Option<DeviceLimitWire> {
    if status != reqwest::StatusCode::FORBIDDEN
        || body.get("error").and_then(serde_json::Value::as_str) != Some(DEVICE_LIMIT_REACHED)
    {
        return None;
    }
    serde_json::from_value(body.clone()).ok()
}

/// 「移除这台并登录」之后的下一步（界面只按它渲染；会话与账户状态照常经 `account://changed` 推）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeviceLimitResolutionDto {
    /// 移除成功、再登录成功。
    SignedIn,
    /// 移除成功；设备码配对还在等浏览器那边（轮询已在 Rust 里接着跑）。
    AwaitingBrowser,
    /// 又撞上了上限（别的设备抢先占了名额）：账户状态里是新的设备列表，再选一次。
    DeviceLimitReached,
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"error":"device_limit_reached","max":2,
        "devices":[{"id":"d1","name":"Mac mini","platform":"macos","last_seen_at":"2026-10-01T08:00:00Z"},
                   {"id":"d2","name":"Old PC","platform":null,"last_seen_at":null}],
        "revoke_token":"rvk_secret","revoke_token_expires_at":"2026-10-14T12:10:00Z"}"#;

    #[test]
    fn a_403_device_limit_body_decodes_into_a_pending_choice() {
        let body: serde_json::Value = serde_json::from_str(BODY).unwrap();
        let wire = decode(reqwest::StatusCode::FORBIDDEN, &body).expect("应认出设备上限");
        let pending = DeviceLimitPending::new(wire, SignInRetry::DeviceCode);
        assert_eq!(pending.limit.max, 2);
        assert_eq!(pending.limit.devices.len(), 2);
        assert_eq!(pending.limit.devices[1].platform, None);
        assert_eq!(pending.limit.expires_at, "2026-10-14T12:10:00Z");
        assert_eq!(pending.revoke_token, "rvk_secret");
        // 打印出来不带令牌。
        assert!(!format!("{pending:?}").contains("rvk_secret"));
        // 界面那一份里没有令牌。
        let dto = serde_json::to_string(&pending.limit).unwrap();
        assert!(!dto.contains("rvk_secret") && !dto.contains("revoke_token"));
    }

    #[test]
    fn only_the_403_with_that_code_and_a_complete_body_counts() {
        let body: serde_json::Value = serde_json::from_str(BODY).unwrap();
        assert!(decode(reqwest::StatusCode::UNAUTHORIZED, &body).is_none());
        let other: serde_json::Value =
            serde_json::from_str(r#"{"error":"account_disabled"}"#).unwrap();
        assert!(decode(reqwest::StatusCode::FORBIDDEN, &other).is_none());
        // 缺令牌（必填）：认不出就照原处置，不编一个能用的令牌出来。
        let incomplete: serde_json::Value = serde_json::from_str(
            r#"{"error":"device_limit_reached","max":2,"devices":[],"revoke_token_expires_at":"x"}"#,
        )
        .unwrap();
        assert!(decode(reqwest::StatusCode::FORBIDDEN, &incomplete).is_none());
    }
}
