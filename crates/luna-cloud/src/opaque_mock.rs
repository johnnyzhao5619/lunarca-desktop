//! 测试用的假服务端：真跑 `opaque-ke` 的服务端一侧，按 TDD-16 §5.2a 作答（只给本 crate 的测试用；
//! 真实服务端在商业仓按同一份契约实现，互通靠 `luna-opaque` 的 `tests/fixtures/opaque-login-v16.json`）。
//!
//! 走明文 HTTP：首连的放宽客户端与钉住的客户端对 `http://` 都一样发，信任仍只来自握手与信任包。
//! 每一条请求按 `(方法, 路径, 头, 体)` 记下，断言看的是线上真实发出去的字节。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opaque_ke::{
    ClientRegistration, ClientRegistrationFinishParameters, CredentialFinalization,
    CredentialRequest, RegistrationRequest, RegistrationUpload, ServerLogin, ServerLoginParameters,
    ServerRegistration, ServerSetup,
};
use sha2::{Digest as _, Sha256};

use luna_opaque::{BundleTls, Suite, TrustBundle};

/// 一条记下的请求。
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: serde_json::Value,
    pub raw: String,
}

struct Login {
    state: Option<ServerLogin<Suite>>,
    email: String,
    purpose: String,
    claim_sha256: Option<[u8; 32]>,
    used: bool,
}

pub(crate) struct State {
    setup: ServerSetup<Suite>,
    /// 握手与信任包里的部署 id。
    pub deployment_id: String,
    /// capabilities 自报的部署 id（模拟两者不一致的服务端时单独改它）。
    pub caps_deployment_id: String,
    /// 信任包里写的部署 id（模拟「解开了、却写着另一台」）。
    pub bundle_deployment_id: Option<String>,
    pub bundle_tls: BundleTls,
    pub lease_pubkey: String,
    pub catalog_pubkey: String,
    pub account_id: String,
    /// `(邮箱, purpose)` → 注册记录。
    records: HashMap<(String, String), ServerRegistration<Suite>>,
    logins: HashMap<String, Login>,
    /// 注册授权 / access token → 它属于哪个邮箱。
    bearers: HashMap<String, String>,
    registrations: HashMap<String, String>,
    /// 某条路径的下一次（或几次）答复改成这些（用完回到正常逻辑）。
    pub scripted: HashMap<&'static str, VecDeque<(u16, String)>>,
    pub well_known: Option<String>,
    issued: u32,
    pub seen: Vec<Seen>,
}

pub(crate) struct MockServer {
    pub base: String,
    pub state: Arc<Mutex<State>>,
}

pub(crate) fn real_pubkey(seed: u8) -> String {
    ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl MockServer {
    pub fn start(deployment_id: &str) -> Self {
        let mut rng = aes_gcm::aead::OsRng;
        let state = Arc::new(Mutex::new(State {
            setup: ServerSetup::<Suite>::new(&mut rng),
            deployment_id: deployment_id.to_owned(),
            caps_deployment_id: deployment_id.to_owned(),
            bundle_deployment_id: None,
            bundle_tls: BundleTls::Public,
            lease_pubkey: real_pubkey(1),
            catalog_pubkey: real_pubkey(2),
            account_id: "acc-1".to_owned(),
            records: HashMap::new(),
            logins: HashMap::new(),
            bearers: HashMap::new(),
            registrations: HashMap::new(),
            scripted: HashMap::new(),
            well_known: None,
            issued: 0,
            seen: Vec::new(),
        }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let seen = read_request(&mut stream);
                let (status, body) = {
                    let mut state = shared.lock().unwrap();
                    state.seen.push(seen.clone());
                    state.answer(&seen)
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base, state }
    }

    /// 以 `secret` 为口令（或一次性码）给邮箱建一条记录，身份绑定这台部署。
    pub fn register(&self, email: &str, purpose: &str, secret: &str) {
        let mut state = self.state.lock().unwrap();
        let mut rng = aes_gcm::aead::OsRng;
        let ksf = luna_opaque::ksf();
        let start = ClientRegistration::<Suite>::start(&mut rng, secret.as_bytes()).unwrap();
        let response =
            ServerRegistration::<Suite>::start(&state.setup, start.message, email.as_bytes())
                .unwrap();
        let deployment = state.deployment_id.clone();
        let finish = start
            .state
            .finish(
                &mut rng,
                secret.as_bytes(),
                response.message,
                ClientRegistrationFinishParameters::new(
                    luna_opaque::identifiers(email, &deployment),
                    Some(&ksf),
                ),
            )
            .unwrap();
        state.records.insert(
            (email.to_owned(), purpose.to_owned()),
            ServerRegistration::<Suite>::finish(finish.message),
        );
    }

    /// 下一次（依次）请求这条路径时改用这些答复。
    pub fn script(&self, path: &'static str, answers: Vec<(u16, &str)>) {
        self.state.lock().unwrap().scripted.insert(
            path,
            answers
                .into_iter()
                .map(|(status, body)| (status, body.to_owned()))
                .collect(),
        );
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    pub fn bodies_to(&self, path: &str) -> Vec<serde_json::Value> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.path == path)
            .map(|seen| seen.body)
            .collect()
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }
}

fn json(value: &serde_json::Value) -> String {
    value.to_string()
}

fn error(code: &str) -> String {
    json(&serde_json::json!({ "error": code }))
}

impl State {
    fn tokens(&mut self, email: &str) -> String {
        self.issued += 1;
        let access = format!("acc_{}", self.issued);
        self.bearers.insert(access.clone(), email.to_owned());
        json(&serde_json::json!({
            "access_token": access,
            "refresh_token": format!("ref_{}", self.issued),
        }))
    }

    fn answer(&mut self, seen: &Seen) -> (u16, String) {
        if let Some(queue) = self.scripted.get_mut(seen.path.as_str()) {
            if let Some((status, body)) = queue.pop_front() {
                return (status, body);
            }
        }
        let body = &seen.body;
        let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
        match (seen.method.as_str(), seen.path.as_str()) {
            ("GET", "/.well-known/lunarca") => match &self.well_known {
                Some(doc) => (200, doc.clone()),
                None => (404, String::new()),
            },
            ("GET", "/inference/v1/capabilities") => (
                200,
                json(&serde_json::json!({
                    "protocol": crate::protocol::CLIENT_PROTOCOL,
                    "min_client_protocol": crate::protocol::CLIENT_PROTOCOL,
                    "status": "ready",
                    "capabilities": ["chat", "embed", "transcribe"],
                    "deployment": { "id": self.caps_deployment_id, "display_name": "Corp" },
                    "auth": { "login": { "password": true, "browser": false, "providers": [],
                                          "admission": "open", "password_reset": true } },
                    "billing": { "enabled": false },
                })),
            ),
            ("GET", "/v1/trust/keys") => (
                200,
                json(&serde_json::json!({
                    "format_version": 1,
                    "deployment_id": self.deployment_id,
                    "lease_pubkey": self.lease_pubkey,
                    "catalog_pubkey": self.catalog_pubkey,
                })),
            ),
            ("GET", "/v1/me") => {
                let email = seen
                    .authorization
                    .as_deref()
                    .and_then(|auth| auth.strip_prefix("Bearer "))
                    .and_then(|token| self.bearers.get(token).cloned())
                    .unwrap_or_default();
                (
                    200,
                    json(&serde_json::json!({ "account_id": self.account_id, "email": email })),
                )
            }
            ("POST", "/v1/auth/opaque/login/start") => self.login_start(
                &text("email"),
                &text("purpose"),
                &text("credential_request"),
            ),
            ("POST", "/v1/auth/opaque/login/finish") => {
                self.login_finish(&text("login_id"), &text("credential_finalization"))
            }
            ("POST", "/v1/auth/opaque/session") => self.session(&text("login_id"), &text("claim")),
            ("POST", "/v1/auth/opaque/register/start") => {
                self.register_start(seen.authorization.as_deref(), &text("registration_request"))
            }
            ("POST", "/v1/auth/opaque/register/finish") => self.register_finish(
                seen.authorization.as_deref(),
                &text("registration_id"),
                &text("registration_upload"),
            ),
            ("POST", "/v1/auth/password/forgot") => (202, String::new()),
            ("POST", "/v1/auth/signup/start") => {
                if text("email").ends_with("@mailinator.test") {
                    (400, error("email_domain_not_allowed"))
                } else {
                    (202, String::new())
                }
            }
            ("POST", "/v1/devices/revoke-for-sign-in") => (204, String::new()),
            _ => (404, String::new()),
        }
    }

    fn login_start(&mut self, email: &str, purpose: &str, request: &str) -> (u16, String) {
        let Some(request) = luna_opaque::from_b64u(request)
            .and_then(|bytes| CredentialRequest::<Suite>::deserialize(&bytes).ok())
        else {
            return (400, error("bad_request"));
        };
        let mut rng = aes_gcm::aead::OsRng;
        let record = self
            .records
            .get(&(email.to_owned(), purpose.to_owned()))
            .cloned();
        let deployment = self.deployment_id.clone();
        // 未知邮箱：`opaque-ke` 伪造记录，应答形状与真记录一致。
        let started = ServerLogin::start(
            &mut rng,
            &self.setup,
            record,
            request,
            email.as_bytes(),
            ServerLoginParameters {
                context: Some(luna_opaque::CONTEXT),
                identifiers: luna_opaque::identifiers(email, &deployment),
            },
        )
        .unwrap();
        let login_id = luna_opaque::b64u(&rand_bytes::<16>());
        self.logins.insert(
            login_id.clone(),
            Login {
                state: Some(started.state),
                email: email.to_owned(),
                purpose: purpose.to_owned(),
                claim_sha256: None,
                used: false,
            },
        );
        (
            200,
            json(&serde_json::json!({
                "login_id": login_id,
                "deployment_id": deployment,
                "credential_response": luna_opaque::b64u(&started.message.serialize()),
            })),
        )
    }

    fn login_finish(&mut self, login_id: &str, finalization: &str) -> (u16, String) {
        let deployment = self.deployment_id.clone();
        let Some(login) = self.logins.get_mut(login_id) else {
            return (401, error("invalid_credentials"));
        };
        let (Some(state), Some(finalization)) = (
            login.state.take(),
            luna_opaque::from_b64u(finalization)
                .and_then(|bytes| CredentialFinalization::<Suite>::deserialize(&bytes).ok()),
        ) else {
            return (401, error("invalid_credentials"));
        };
        let email = login.email.clone();
        let Ok(finished) = state.finish(
            finalization,
            ServerLoginParameters {
                context: Some(luna_opaque::CONTEXT),
                identifiers: luna_opaque::identifiers(&email, &deployment),
            },
        ) else {
            return (401, error("invalid_credentials"));
        };
        let login_id_bytes = luna_opaque::from_b64u(login_id).unwrap();
        let claim = luna_opaque::claim_key(&finished.session_key, &login_id_bytes);
        login.claim_sha256 = Some(Sha256::digest(claim.as_ref()).into());
        let bundle = TrustBundle {
            format_version: 1,
            deployment_id: self.bundle_deployment_id.clone().unwrap_or(deployment),
            tls: self.bundle_tls.clone(),
            lease_pubkey: self.lease_pubkey.clone(),
            catalog_pubkey: self.catalog_pubkey.clone(),
        };
        let sealed = seal_with_aad_of(
            &bundle,
            &self.deployment_id,
            &finished.session_key,
            &login_id_bytes,
        );
        (
            200,
            json(&serde_json::json!({ "sealed_bundle": luna_opaque::b64u(&sealed) })),
        )
    }

    fn session(&mut self, login_id: &str, claim: &str) -> (u16, String) {
        let Some(login) = self.logins.get_mut(login_id) else {
            return (410, error("login_expired"));
        };
        if login.used {
            return (410, error("login_expired"));
        }
        let presented =
            luna_opaque::from_b64u(claim).map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if login.claim_sha256.is_none() || presented != login.claim_sha256 {
            return (401, error("invalid_credentials"));
        }
        login.used = true;
        let (email, purpose) = (login.email.clone(), login.purpose.clone());
        if purpose == "code" {
            self.records.remove(&(email.clone(), "code".to_owned()));
            self.issued += 1;
            let grant = format!("rg_{}", self.issued);
            self.bearers.insert(grant.clone(), email);
            return (
                200,
                json(&serde_json::json!({ "registration_grant": grant, "expires_in": 600 })),
            );
        }
        (200, self.tokens(&email))
    }

    fn bearer_email(&self, authorization: Option<&str>) -> Option<String> {
        let token = authorization?.strip_prefix("Bearer ")?;
        self.bearers.get(token).cloned()
    }

    fn register_start(&mut self, authorization: Option<&str>, request: &str) -> (u16, String) {
        let Some(email) = self.bearer_email(authorization) else {
            return (401, error("unauthorized"));
        };
        let Some(request) = luna_opaque::from_b64u(request)
            .and_then(|bytes| RegistrationRequest::<Suite>::deserialize(&bytes).ok())
        else {
            return (400, error("bad_request"));
        };
        let response =
            ServerRegistration::<Suite>::start(&self.setup, request, email.as_bytes()).unwrap();
        let registration_id = luna_opaque::b64u(&rand_bytes::<16>());
        self.registrations.insert(registration_id.clone(), email);
        (
            200,
            json(&serde_json::json!({
                "registration_id": registration_id,
                "registration_response": luna_opaque::b64u(&response.message.serialize()),
            })),
        )
    }

    fn register_finish(
        &mut self,
        authorization: Option<&str>,
        registration_id: &str,
        upload: &str,
    ) -> (u16, String) {
        if self.bearer_email(authorization).is_none() {
            return (401, error("unauthorized"));
        }
        let (Some(email), Some(upload)) = (
            self.registrations.get(registration_id).cloned(),
            luna_opaque::from_b64u(upload)
                .and_then(|bytes| RegistrationUpload::<Suite>::deserialize(&bytes).ok()),
        ) else {
            return (400, error("bad_request"));
        };
        self.records.insert(
            (email.clone(), "password".to_owned()),
            ServerRegistration::<Suite>::finish(upload),
        );
        (200, self.tokens(&email))
    }
}

/// 封包：AAD 恒用握手那台部署（模拟「解开了、明文却写另一台」时只改明文）。
fn seal_with_aad_of(
    bundle: &TrustBundle,
    handshake_deployment: &str,
    session_key: &[u8],
    login_id: &[u8],
) -> Vec<u8> {
    luna_opaque::seal_bundle_for(
        bundle,
        handshake_deployment,
        session_key,
        login_id,
        rand_bytes::<12>(),
    )
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    use aes_gcm::aead::rand_core::RngCore as _;
    let mut out = [0_u8; N];
    aes_gcm::aead::OsRng.fill_bytes(&mut out);
    out
}

fn read_request(stream: &mut TcpStream) -> Seen {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break raw.len();
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let header = |wanted: &str| {
        head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_owned())
        })
    };
    let length = header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while raw.len() < header_end + length {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let body = serde_json::from_slice(&raw[header_end..]).unwrap_or(serde_json::Value::Null);
    Seen {
        method,
        path,
        authorization: header("authorization"),
        body,
        raw: String::from_utf8_lossy(&raw).to_string(),
    }
}
