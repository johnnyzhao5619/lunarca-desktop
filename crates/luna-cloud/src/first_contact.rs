//! 首连：还没钉住任何东西的服务端上，靠口令（或一次性码）建立信任（ADR-158 ③④，线协议
//! TDD-16 §5.2a）。
//!
//! # 全程唯一放宽 TLS 校验的地方
//!
//! 自签部署在钉住根证书之前，系统信任库不认它；这一阶段的信任不来自 TLS，而来自 OPAQUE：
//! 对端拿不到这个账户的注册记录，就算不出同一把会话密钥，信任包解不开、3DH 校验不过。
//! 故放宽校验的客户端只在这里构造一次，且只发 [`ALLOWED_PATHS`] 里的四条：
//!
//! - `login/start`、`login/finish`：OPAQUE 两段式，结果当场用会话密钥解开信任包并核对
//!   `deployment_id`（[`opaque_auth::open_bundle`] / [`opaque_auth::ensure_same_deployment`]）；
//! - `password/forgot`、`signup/start`：只带邮箱（`login/start` 本就要发）、只看状态码，
//!   响应体一个字节都不读——中间人最多吞掉这封信，拿不到任何秘密。
//!
//! 钉住之后的一切（领取会话、注册口令、取 capabilities）都在调用方改走钉住的 TLS，主机名校验
//! 照常针对用户输入的 origin（中继攻击的最后一道防线）。`trust.rs` 的源码守护按这几条核对本文件。
//!

use std::time::Duration;

use opaque_ke::{ClientLogin, ClientLoginFinishParameters, CredentialResponse};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::{CloudError, LoginRejection};
use crate::opaque_auth::{self, Suite, TrustBundle};

/// OPAQUE 第一段。
pub const LOGIN_START_PATH: &str = "/v1/auth/opaque/login/start";
/// OPAQUE 第二段。
pub const LOGIN_FINISH_PATH: &str = "/v1/auth/opaque/login/finish";
/// 申请重设口令的一次性码（恒 202）。
pub const FORGOT_PATH: &str = "/v1/auth/password/forgot";
/// 申请注册验证码（202；一次性邮箱域名 400）。
pub const SIGNUP_START_PATH: &str = "/v1/auth/signup/start";

/// 放宽校验的客户端只许发这几条（源码守护逐字核对）。
pub const ALLOWED_PATHS: [&str; 4] = [
    LOGIN_START_PATH,
    LOGIN_FINISH_PATH,
    FORGOT_PATH,
    SIGNUP_START_PATH,
];

/// 首连请求的超时：KSF 在客户端跑，服务端这两段都只是一次查库。
const TIMEOUT: Duration = Duration::from_secs(20);

/// 走哪条 TLS。
pub enum Channel {
    /// 已钉住根证书或公网证书：调用方给的、照常校验的客户端。
    Trusted(reqwest::Client),
    /// 还没钉住、系统信任库也不认：本文件构造的放宽客户端，只发 [`ALLOWED_PATHS`]。
    FirstContact,
}

impl Channel {
    fn client(self) -> Result<reqwest::Client, CloudError> {
        match self {
            Self::Trusted(client) => Ok(client),
            Self::FirstContact => reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                // 不跟随跳转：被带去别的主机就不是用户要登录的那一台。
                .redirect(reqwest::redirect::Policy::none())
                .timeout(TIMEOUT)
                .build()
                .map_err(|err| CloudError::internal(format!("first-contact client: {err}"))),
        }
    }
}

/// 这次握手证明的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// 用户自己的口令：领取到的是设备会话。
    Password,
    /// 一次性码（重设 / 邀请 / 激活 / 注册验证）：领取到的是只够设口令的注册授权。
    Code,
}

impl Purpose {
    fn wire(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Code => "code",
        }
    }
}

/// 一次成功的握手：信任包已解开、部署已核对。
pub struct Handshake {
    /// 服务端给的握手 id（线上原样的 b64u，领取会话时回送）。
    pub login_id: String,
    pub deployment_id: String,
    /// 领取凭据 `b64u(K_claim)`：只许经钉住的 TLS 发出。
    pub claim: Zeroizing<String>,
    pub bundle: TrustBundle,
}

impl std::fmt::Debug for Handshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handshake")
            .field("deployment_id", &self.deployment_id)
            .field("claim", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct StartResponse {
    login_id: String,
    deployment_id: String,
    credential_response: String,
}

#[derive(Deserialize)]
struct FinishResponse {
    sealed_bundle: String,
}

/// 跑一次 OPAQUE 登录并解开信任包。
///
/// `email` 须已规整（[`opaque_auth::normalize_email`]）；`secret` 是口令或规整后的一次性码。
/// `expected_deployment` = `.well-known/lunarca` 给的部署（若有）：握手与信任包都必须是它。
///
/// # Errors
/// 口令 / 码不对（含对端不是这个账户的那台服务端）→ `LoginRejected { invalid_credentials }`；
/// 信任包不可信 → [`CloudError::ServerIdentityRejected`]；网络、限流照常。
pub async fn login(
    origin: &str,
    email: &str,
    secret: &[u8],
    purpose: Purpose,
    expected_deployment: Option<&str>,
    channel: Channel,
) -> Result<Handshake, CloudError> {
    let client = channel.client()?;
    let mut rng = aes_gcm::aead::OsRng;
    let start = ClientLogin::<Suite>::start(&mut rng, secret)
        .map_err(|_| CloudError::internal("opaque login start"))?;
    let response = post(
        &client,
        origin,
        LOGIN_START_PATH,
        &serde_json::json!({
            "email": email,
            "credential_request": opaque_auth::b64u(&start.message.serialize()),
            "purpose": purpose.wire(),
        }),
    )
    .await?;
    let started: StartResponse = response
        .json()
        .await
        .map_err(|err| CloudError::body(&err, origin))?;
    if expected_deployment.is_some_and(|expected| expected != started.deployment_id) {
        return Err(CloudError::ServerIdentityRejected);
    }
    let credential_response = opaque_auth::from_b64u(&started.credential_response)
        .and_then(|bytes| CredentialResponse::<Suite>::deserialize(&bytes).ok())
        .ok_or_else(|| unreadable(origin, "credential_response"))?;
    let ksf = opaque_auth::ksf();
    // 口令不对、或对端不是这个账户注册时的那台部署（身份绑定不同）：客户端在这里就解不开。
    let finished = start
        .state
        .finish(
            &mut rng,
            secret,
            credential_response,
            ClientLoginFinishParameters::new(
                Some(opaque_auth::CONTEXT),
                opaque_auth::identifiers(email, &started.deployment_id),
                Some(&ksf),
            ),
        )
        .map_err(|_| CloudError::LoginRejected {
            reason: LoginRejection::InvalidCredentials,
        })?;
    let response = post(
        &client,
        origin,
        LOGIN_FINISH_PATH,
        &serde_json::json!({
            "login_id": started.login_id,
            "credential_finalization": opaque_auth::b64u(&finished.message.serialize()),
        }),
    )
    .await?;
    let sealed = response
        .json::<FinishResponse>()
        .await
        .map_err(|err| CloudError::body(&err, origin))?;
    let sealed = opaque_auth::from_b64u(&sealed.sealed_bundle)
        .ok_or_else(|| unreadable(origin, "sealed_bundle"))?;
    let login_id =
        opaque_auth::from_b64u(&started.login_id).ok_or_else(|| unreadable(origin, "login_id"))?;
    let session_key = Zeroizing::new(finished.session_key.to_vec());
    // 信任包用会话密钥解开、且写的正是握手那台部署，才算数；任一不成立整次丢弃。
    let bundle = opaque_auth::open_bundle(&sealed, &session_key, &login_id, &started.deployment_id)
        .map_err(|err| {
            eprintln!("[account] 信任包不可信，已丢弃：{err}");
            CloudError::ServerIdentityRejected
        })?;
    if let Some(expected) = expected_deployment {
        opaque_auth::ensure_same_deployment(&bundle, expected)
            .map_err(|_| CloudError::ServerIdentityRejected)?;
    }
    let claim = Zeroizing::new(opaque_auth::b64u(
        opaque_auth::claim_key(&session_key, &login_id).as_ref(),
    ));
    Ok(Handshake {
        login_id: started.login_id,
        deployment_id: started.deployment_id,
        claim,
        bundle,
    })
}

/// 请服务端往这个邮箱发一次性码（重设口令 / 注册验证）。只看状态码。
///
/// # Errors
/// 一次性邮箱域名 → [`CloudError::EmailDomainNotAllowed`]；限流、网络照常。
pub async fn request_code(
    origin: &str,
    path: &'static str,
    email: &str,
    channel: Channel,
) -> Result<(), CloudError> {
    debug_assert!(path == FORGOT_PATH || path == SIGNUP_START_PATH);
    let client = channel.client()?;
    post(
        &client,
        origin,
        path,
        &serde_json::json!({ "email": email }),
    )
    .await
    .map(|_| ())
}

async fn post(
    client: &reqwest::Client,
    origin: &str,
    path: &'static str,
    body: &serde_json::Value,
) -> Result<reqwest::Response, CloudError> {
    let response = client
        .post(format!("{}{path}", origin.trim_end_matches('/')))
        .json(body)
        .send()
        .await
        .map_err(|err| CloudError::network(&err, origin))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        // 口令错 / 伪造记录 / 握手过期，服务端一律同码（反枚举）。
        return Err(CloudError::LoginRejected {
            reason: LoginRejection::InvalidCredentials,
        });
    }
    crate::client::check_status(response).await
}

fn unreadable(origin: &str, field: &str) -> CloudError {
    CloudError::UnreadableResponse {
        host: crate::config::server_bucket(origin),
        detail: format!("{field} is not a valid opaque message"),
    }
}
