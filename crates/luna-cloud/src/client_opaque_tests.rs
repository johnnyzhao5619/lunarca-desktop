//! ADR-158 的客户端行为：邮箱优先的发现、OPAQUE 口令登录（口令不离开设备）、信任包钉住与重连、
//! 一次性码 → 设口令、连接链接带来的邀请码、登录状态下改口令、设备上限与各类拒绝。
//!
//! 对着真跑 `opaque-ke` 服务端一侧的假服务端（[`crate::opaque_mock`]）；断言看线上真实发出去的字节。

use super::*;
use crate::account::AccountPhaseDto;
use crate::opaque_mock::{real_pubkey, MockServer};
use crate::store::MemoryStore;

const EMAIL: &str = "Alice@Corp.Example";
const NORMALIZED: &str = "alice@corp.example";
const PASSWORD: &str = "correct horse battery staple";
const DEPLOYMENT: &str = "dep-corp-1";

const LIMIT_BODY: &str = r#"{"error":"device_limit_reached","max":2,
    "devices":[{"id":"d1","name":"Mac mini","platform":"macos","last_seen_at":"2026-10-01T08:00:00Z"}],
    "revoke_token":"rvk_secret_token","revoke_token_expires_at":"2026-10-14T12:10:00Z"}"#;

fn client() -> CloudClient {
    CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false))
}

/// 发现指向假服务端：`.well-known/lunarca` 就在它上面，并发布它自己。
fn discovering(mock: &MockServer) -> CloudClient {
    let client = client();
    *client.discovery_base.lock().unwrap() = Some(mock.base.clone());
    let doc = serde_json::json!({
        "format_version": 1, "server": mock.base, "deployment_id": DEPLOYMENT,
    })
    .to_string();
    mock.with(|state| state.well_known = Some(doc));
    client
}

/// 自签、未钉住的服务端那一刻的处境：经校验的 TLS 连不上，握手只能走首连。
/// 明文 HTTP 的假服务端上 capabilities 本来连得上，故直接摆好这一步的上下文。
fn at_first_contact(mock: &MockServer) -> CloudClient {
    let client = client();
    *client.sign_in.lock().unwrap() = Some(SignInContext {
        server: AccountServer {
            origin: mock.base.clone(),
            display_name: None,
        },
        email: NORMALIZED.to_owned(),
        expected_deployment: None,
        trusted_tls: false,
    });
    client
}

fn bucket_of(mock: &MockServer) -> String {
    crate::config::server_bucket(&mock.base)
}

/// 邮箱优先：没记住服务器 → 邮箱域名上的 `.well-known` 指向这台 → 「将登录到 X」→ 口令 → 进入。
/// 口令从不出现在线上任何一个字节里。
#[tokio::test]
async fn email_first_discovers_the_server_and_signs_in_without_sending_the_password() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = discovering(&mock);

    let step = client
        .sign_in_begin(SignInTarget::Discover {
            email: format!("  {EMAIL} "),
        })
        .await
        .unwrap();
    let SignInStepDto::Ready {
        server,
        email,
        methods,
        invite,
    } = step
    else {
        panic!("首次连这台不该要求确认变更");
    };
    assert_eq!(server.origin, mock.base);
    assert_eq!(email, NORMALIZED);
    assert!(
        methods.is_some_and(|m| m.password),
        "经可信 TLS 连得上就给出登录方式"
    );
    assert!(!invite);

    let outcome = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, PasswordSignInDto::SignedIn));
    assert!(client.logged_in());
    let profile = client.cached_profile().unwrap();
    assert_eq!(profile.email, NORMALIZED);
    assert_eq!(client.account().server().unwrap().origin, mock.base);

    for seen in mock.seen() {
        assert!(!seen.raw.contains(PASSWORD), "{} 里出现了口令", seen.path);
    }
    let session = mock.bodies_to("/v1/auth/opaque/session");
    assert_eq!(session.len(), 1);
    assert_eq!(session[0]["device_name"], "Laptop");
    assert_eq!(
        session[0]["device_fingerprint"].as_str(),
        Some(client.fingerprint().get().as_str())
    );
    assert!(
        client.sign_in.lock().unwrap().is_none(),
        "登录完成即丢掉这次的上下文"
    );
}

/// 首连（自签、未钉住）：信任包里的根证书为用户给的 origin 钉住，两把签名公钥以 `Password` 来源钉住。
#[tokio::test]
async fn first_contact_pins_the_bundles_tls_root_and_anchors_from_the_password() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let root_pem = "-----BEGIN CERTIFICATE-----\nMIIBbDCCARKgAwIBAgIUCaxSJZTtUFopdREqDXcVMPYi3J0wCgYIKoZIzj0EAwIw\n-----END CERTIFICATE-----\n";
    mock.with(|state| {
        state.bundle_tls = BundleTls::PinnedRoot {
            root_pem: root_pem.to_owned(),
        };
    });
    let client = at_first_contact(&mock);

    let outcome = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, PasswordSignInDto::SignedIn));
    let bucket = bucket_of(&mock);
    let pinned_root = client.tls_roots().pinned(&bucket).expect("根证书应已钉住");
    assert_eq!(pinned_root.origin, mock.base);
    let anchor = client.trust().pinned(&bucket).expect("两钥应已钉住");
    assert_eq!(anchor.provenance, TrustProvenance::Password);
    assert_eq!(anchor.lease_pubkey, real_pubkey(1));
    assert_eq!(anchor.deployment_id, DEPLOYMENT);
    assert_eq!(client.config().api_base, mock.base);
    luna_model_fetch::pinned_tls::set_pinned_root(&mock.base, None);
}

/// 口令不对：客户端解不开应答就停下（不发第二段），什么都不钉、不选、不存。
#[tokio::test]
async fn a_wrong_password_stops_before_finish_and_pins_nothing() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = at_first_contact(&mock);
    let err = client
        .sign_in_password("not it", "Laptop", "windows")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CloudError::LoginRejected {
            reason: LoginRejection::InvalidCredentials
        }
    ));
    assert!(mock.bodies_to("/v1/auth/opaque/login/finish").is_empty());
    assert!(client.trust().pinned(&bucket_of(&mock)).is_none());
    assert!(client.tls_roots().pinned(&bucket_of(&mock)).is_none());
    assert_eq!(client.account().server(), None);
    assert!(!client.logged_in());
    // 未知邮箱走伪造记录：同一句，对端不披露有没有这个账户。
    *client.sign_in.lock().unwrap() = Some(SignInContext {
        email: "nobody@corp.example".to_owned(),
        ..client.sign_in_context().unwrap()
    });
    assert!(matches!(
        client.sign_in_password(PASSWORD, "Laptop", "windows").await,
        Err(CloudError::LoginRejected {
            reason: LoginRejection::InvalidCredentials
        })
    ));
}

/// 邮箱域名发布的是另一台部署（握手被带去了别处）：整次丢弃，不钉、不选、不领会话。
#[tokio::test]
async fn a_handshake_with_another_deployment_than_published_is_discarded() {
    let mock = MockServer::start("dep-elsewhere");
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = at_first_contact(&mock);
    client
        .sign_in
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .expected_deployment = Some(DEPLOYMENT.to_owned());
    let err = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::ServerIdentityRejected), "{err:?}");
    assert!(client.trust().pinned(&bucket_of(&mock)).is_none());
    assert!(mock.bodies_to("/v1/auth/opaque/session").is_empty());
}

/// 信任包解得开、却写着另一台；或 capabilities 自报的部署与握手不一致：都不可信，什么都不留。
#[tokio::test]
async fn a_bundle_or_capabilities_naming_another_deployment_is_refused() {
    for lie in ["bundle", "capabilities"] {
        let mock = MockServer::start(DEPLOYMENT);
        mock.register(NORMALIZED, "password", PASSWORD);
        mock.with(|state| {
            state.bundle_tls = BundleTls::PinnedRoot {
                root_pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"
                    .to_owned(),
            };
            match lie {
                "bundle" => state.bundle_deployment_id = Some("dep-other".to_owned()),
                _ => state.caps_deployment_id = "dep-other".to_owned(),
            }
        });
        let client = at_first_contact(&mock);
        let err = client
            .sign_in_password(PASSWORD, "Laptop", "windows")
            .await
            .unwrap_err();
        assert!(
            matches!(err, CloudError::ServerIdentityRejected),
            "{lie}: {err:?}"
        );
        let bucket = bucket_of(&mock);
        assert!(client.trust().pinned(&bucket).is_none(), "{lie}");
        assert!(
            client.tls_roots().pinned(&bucket).is_none(),
            "{lie}：钉住应已撤回"
        );
        assert!(
            mock.bodies_to("/v1/auth/opaque/session").is_empty(),
            "{lie}"
        );
    }
}

/// 送来的两钥与本机钉住的不一致：服务器选定、两钥不改、不领会话，交给用户确认；确认后来源记为口令。
#[tokio::test]
async fn changed_anchors_from_a_password_sign_in_wait_for_confirmation() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = at_first_contact(&mock);
    let bucket = bucket_of(&mock);
    client.trust().pin(
        &bucket,
        PinnedAnchor {
            lease_pubkey: real_pubkey(7),
            catalog_pubkey: real_pubkey(8),
            pinned_at: 0,
            provenance: TrustProvenance::Imported,
            deployment_id: DEPLOYMENT.into(),
        },
    );
    let outcome = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        PasswordSignInDto::NeedsTrustConfirmation { .. }
    ));
    assert!(mock.bodies_to("/v1/auth/opaque/session").is_empty());
    assert_eq!(
        client.trust().pinned(&bucket).unwrap().lease_pubkey,
        real_pubkey(7)
    );
    let (_, repinned) = client.confirm_trust_change().unwrap();
    assert!(repinned);
    let anchor = client.trust().pinned(&bucket).unwrap();
    assert_eq!(anchor.provenance, TrustProvenance::Password);
    assert_eq!(anchor.lease_pubkey, real_pubkey(1));
}

/// 忘记口令：申请码 → 输码（规整后比对）→ 设口令 → 进入；之后新口令可登录，码用过即作废。
#[tokio::test]
async fn a_reset_code_leads_to_a_new_password_and_signs_in() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "code", "ABCDEFGH10");
    let client = at_first_contact(&mock);

    client.sign_in_request_code(CodeKind::Reset).await.unwrap();
    assert_eq!(
        mock.bodies_to("/v1/auth/password/forgot"),
        vec![serde_json::json!({ "email": NORMALIZED })]
    );
    let outcome = client
        .sign_in_code(Some("abcd-efgh 1o"), "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, CodeSignInDto::SetPassword));
    assert!(!client.logged_in(), "码只换来设口令的授权");
    let start = mock.bodies_to("/v1/auth/opaque/login/start");
    assert_eq!(start[0]["purpose"], "code");

    let outcome = client
        .sign_in_set_password("a brand new secret", "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, PasswordSignInDto::SignedIn));
    assert!(client.logged_in());
    assert!(
        client.registration.lock().unwrap().is_none(),
        "授权用完即丢"
    );
    let register = mock
        .seen()
        .into_iter()
        .find(|seen| seen.path == "/v1/auth/opaque/register/start")
        .unwrap();
    assert!(register
        .authorization
        .is_some_and(|auth| auth.starts_with("Bearer rg_")));
    for seen in mock.seen() {
        assert!(!seen.raw.contains("a brand new secret"), "{}", seen.path);
        assert!(!seen.raw.contains("ABCDEFGH10"), "{}", seen.path);
    }

    // 新口令能登录（同一台、同一个邮箱）。
    let again = at_first_contact(&mock);
    assert!(matches!(
        again
            .sign_in_password("a brand new secret", "Laptop", "windows")
            .await
            .unwrap(),
        PasswordSignInDto::SignedIn
    ));
}

/// 没有授权或已过期：设口令直接要求重来，不发请求。
#[tokio::test]
async fn setting_a_password_without_a_live_grant_says_start_again() {
    let mock = MockServer::start(DEPLOYMENT);
    let client = at_first_contact(&mock);
    assert!(matches!(
        client
            .sign_in_set_password("pw-new-1", "L", "windows")
            .await,
        Err(CloudError::LoginExpired)
    ));
    *client.registration.lock().unwrap() = Some(RegistrationGrant {
        grant: zeroize::Zeroizing::new("rg_old".to_owned()),
        email: NORMALIZED.to_owned(),
        deployment_id: DEPLOYMENT.to_owned(),
        expires_at: Instant::now() - Duration::from_secs(1),
    });
    assert!(matches!(
        client
            .sign_in_set_password("pw-new-1", "L", "windows")
            .await,
        Err(CloudError::LoginExpired)
    ));
    assert!(mock.seen().is_empty());
}

/// 连接链接（v2）带邀请码：只需邮箱 → 直接设口令；码留在 Rust 里，用过即丢。
#[tokio::test]
async fn an_invite_link_goes_straight_to_setting_a_password() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "code", "ABCDEFGH23");
    let client = client();
    let link = format!(
        "lunarca://connect?v=2&server={}&name=Acme&invite=abcd-efgh-23",
        crate::connect_link::encode_query(&mock.base)
    );
    client.stash_incoming_link(&link).unwrap();
    let step = client
        .sign_in_begin(SignInTarget::IncomingLink {
            email: EMAIL.to_owned(),
        })
        .await
        .unwrap();
    let SignInStepDto::Ready { server, invite, .. } = step else {
        panic!("应直接进入下一步");
    };
    assert!(invite);
    assert_eq!(
        server.display_name.as_deref(),
        Some("Acme"),
        "链接给的名字优先"
    );
    assert!(
        client.account_state().incoming_link.is_none(),
        "用掉的链接还挂着"
    );
    let state = serde_json::to_string(&client.account_state()).unwrap();
    assert!(
        !state.to_ascii_lowercase().contains("abcd-efgh"),
        "邀请码过了 IPC"
    );

    let outcome = client
        .sign_in_code(None, "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, CodeSignInDto::SetPassword));
    assert!(client.invite.lock().unwrap().is_none(), "兑换成功即用掉");
    assert!(matches!(
        client
            .sign_in_set_password(PASSWORD, "Laptop", "windows")
            .await
            .unwrap(),
        PasswordSignInDto::SignedIn
    ));
}

/// 领取会话撞上设备上限：选一台移除 → 同一条领取原样重发 → 进入。口令不在挂着的东西里。
#[tokio::test]
async fn a_device_limit_on_the_claim_resends_the_same_claim() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    mock.script("/v1/auth/opaque/session", vec![(403, LIMIT_BODY)]);
    let client = at_first_contact(&mock);
    let outcome = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(outcome, PasswordSignInDto::DeviceLimitReached));
    assert!(matches!(
        client.account_state().phase,
        AccountPhaseDto::DeviceLimit { .. }
    ));
    let pending = format!(
        "{:?}",
        client.device_limit.lock().unwrap().as_ref().unwrap()
    );
    assert!(!pending.contains(PASSWORD) && !pending.contains("rvk_secret_token"));

    let resolved = client.sign_in_resolve_device_limit("d1").await.unwrap();
    assert_eq!(resolved, DeviceLimitResolutionDto::SignedIn);
    assert!(client.logged_in());
    let claims = mock.bodies_to("/v1/auth/opaque/session");
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0], claims[1], "重发必须是同一条领取");
}

/// 设口令那一步撞上设备上限：重发同一条注册、带同一个授权。
#[tokio::test]
async fn a_device_limit_on_register_finish_resends_it_with_the_same_grant() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "code", "ABCDEFGH10");
    mock.script("/v1/auth/opaque/register/finish", vec![(403, LIMIT_BODY)]);
    let client = at_first_contact(&mock);
    client
        .sign_in_code(Some("ABCDEFGH10"), "Laptop", "windows")
        .await
        .unwrap();
    assert!(matches!(
        client
            .sign_in_set_password("pw-new-12345", "Laptop", "windows")
            .await
            .unwrap(),
        PasswordSignInDto::DeviceLimitReached
    ));
    assert_eq!(
        client.sign_in_resolve_device_limit("d1").await.unwrap(),
        DeviceLimitResolutionDto::SignedIn
    );
    let finishes: Vec<_> = mock
        .seen()
        .into_iter()
        .filter(|seen| seen.path == "/v1/auth/opaque/register/finish")
        .collect();
    assert_eq!(finishes.len(), 2);
    assert_eq!(finishes[0].body, finishes[1].body);
    assert_eq!(finishes[0].authorization, finishes[1].authorization);
}

/// 领取环节的拒绝各有各的码；钥匙链写不进时领取资格不花掉。
#[tokio::test]
async fn claim_rejections_map_to_their_own_codes() {
    for (status, body, expect) in [
        (410_u16, r#"{"error":"login_expired"}"#, "expired"),
        (403, r#"{"error":"account_disabled"}"#, "disabled"),
        (403, r#"{"error":"organization_archived"}"#, "archived"),
    ] {
        let mock = MockServer::start(DEPLOYMENT);
        mock.register(NORMALIZED, "password", PASSWORD);
        mock.script("/v1/auth/opaque/session", vec![(status, body)]);
        let client = at_first_contact(&mock);
        let err = client
            .sign_in_password(PASSWORD, "Laptop", "windows")
            .await
            .unwrap_err();
        let got = match err {
            CloudError::LoginExpired => "expired",
            CloudError::LoginRejected {
                reason: LoginRejection::AccountDisabled,
            } => "disabled",
            CloudError::LoginRejected {
                reason: LoginRejection::OrganizationArchived,
            } => "archived",
            other => panic!("{body}: {other:?}"),
        };
        assert_eq!(got, expect);
        assert!(!client.logged_in());
    }
}

/// 钥匙链写不进：握手与钉住照常，但领取请求不发（领取资格一次性，先用掉再存不下就白丢了）。
#[tokio::test]
async fn the_claim_is_not_spent_when_the_keychain_cannot_hold_the_session() {
    struct ReadOnlyStore;

    impl CredentialStore for ReadOnlyStore {
        fn get(&self, _key: &str) -> Result<Option<String>, CloudError> {
            Ok(None)
        }

        fn set(&self, _key: &str, _value: &str) -> Result<(), CloudError> {
            Err(CloudError::Keychain {
                detail: "write rejected".to_owned(),
            })
        }

        fn delete(&self, _key: &str) -> Result<(), CloudError> {
            Ok(())
        }
    }

    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = CloudClient::new(Box::new(ReadOnlyStore), Arc::new(|| false));
    *client.sign_in.lock().unwrap() = at_first_contact(&mock).sign_in.lock().unwrap().clone();
    let err = client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::Keychain { .. }), "{err:?}");
    assert!(mock.bodies_to("/v1/auth/opaque/session").is_empty());
}

/// 登录状态下改口令：带 access token 注册，新令牌写回；旧口令作废、新口令可登录。
#[tokio::test]
async fn changing_the_password_while_signed_in_keeps_this_device_signed_in() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    let client = at_first_contact(&mock);
    client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    let before = client.access_token().unwrap();
    client
        .change_password("another long secret", "Laptop", "windows")
        .await
        .unwrap();
    assert!(client.logged_in());
    assert_ne!(client.access_token().unwrap(), before, "新令牌写回钥匙链");
    let register = mock
        .seen()
        .into_iter()
        .find(|seen| seen.path == "/v1/auth/opaque/register/start")
        .unwrap();
    assert_eq!(register.authorization, Some(format!("Bearer {before}")));

    let old = at_first_contact(&mock);
    assert!(old
        .sign_in_password(PASSWORD, "L", "windows")
        .await
        .is_err());
    let new = at_first_contact(&mock);
    assert!(new
        .sign_in_password("another long secret", "L", "windows")
        .await
        .is_ok());
}

/// 账户键不变（ADR-158 ⑩）：换成 OPAQUE 登录之后，服务端分桶与账户 id 与旧机制下一模一样——
/// 本机库列表与同步绑定（`account_scope::account_key` 的两项输入）因此仍归原账户。
#[tokio::test]
async fn the_account_key_inputs_survive_the_switch_to_opaque() {
    let mock = MockServer::start(DEPLOYMENT);
    mock.register(NORMALIZED, "password", PASSWORD);
    mock.with(|state| state.account_id = "acc-legacy-42".to_owned());
    let legacy_bucket = crate::config::server_bucket(&mock.base);
    let client = at_first_contact(&mock);
    client
        .sign_in_password(PASSWORD, "Laptop", "windows")
        .await
        .unwrap();
    assert_eq!(client.server_bucket(), legacy_bucket);
    assert_eq!(client.cached_profile().unwrap().account_id, "acc-legacy-42");
}

/// 创建账户：一次性邮箱域名有自己的码；第一步填的不是邮箱，本地就拒。
#[tokio::test]
async fn sign_up_and_email_shape_have_their_own_codes() {
    let mock = MockServer::start(DEPLOYMENT);
    let client = at_first_contact(&mock);
    client.sign_in.lock().unwrap().as_mut().unwrap().email = "x@mailinator.test".to_owned();
    assert!(matches!(
        client.sign_in_request_code(CodeKind::Signup).await,
        Err(CloudError::EmailDomainNotAllowed)
    ));
    let fresh = discovering(&mock);
    for bad in ["", "alice", "alice@", "@corp.example"] {
        assert!(
            matches!(
                fresh
                    .sign_in_begin(SignInTarget::Discover {
                        email: bad.to_owned()
                    })
                    .await,
                Err(CloudError::NotAnEmail)
            ),
            "{bad}"
        );
    }
    assert!(!mock
        .seen()
        .iter()
        .any(|seen| seen.path == "/.well-known/lunarca"));
}

/// 发现顺序：记住的服务器优先于邮箱域名；都没有则是 Lunarca 云。
#[tokio::test]
async fn discovery_prefers_the_remembered_server_then_falls_back_to_official() {
    let remembered = MockServer::start(DEPLOYMENT);
    let published = MockServer::start("dep-published");
    let client = discovering(&published);
    client.account().set_server(AccountServer {
        origin: remembered.base.clone(),
        display_name: None,
    });
    let (server, expected) = client.discover("corp.example").await;
    assert_eq!(server.origin, remembered.base);
    assert_eq!(expected, None);

    let nothing = client_with_dead_discovery();
    let (server, expected) = nothing.discover("corp.example").await;
    assert!(server.is_official());
    assert_eq!(expected, None);
}

/// 一个只会回 404 的「网站」（官网那种）：每个连接都回 404，连接数不限。
fn website_that_404s() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    base
}

/// 「其他服务器」里填的是网站而不是服务端（真机 2026-11-03：填 `lunarca.app`）：按主机名回查
/// `.well-known/lunarca`，找到就去它发布的那台，并要求部署与发布的一致；找不到如实说「没有服务端」，
/// 而不是那句让人去问管理员的 404。
#[tokio::test]
async fn a_typed_website_address_falls_back_to_its_well_known() {
    let published = MockServer::start(DEPLOYMENT);
    let client = discovering(&published);
    let step = client
        .sign_in_begin(SignInTarget::Server {
            email: EMAIL.to_owned(),
            input: website_that_404s(),
        })
        .await
        .unwrap();
    let SignInStepDto::Ready { server, .. } = step else {
        panic!("回查到的是首次连的服务端，不该要求确认变更");
    };
    assert_eq!(server.origin, published.base, "要去 .well-known 发布的那台");
    assert_eq!(
        client
            .sign_in_context()
            .unwrap()
            .expected_deployment
            .as_deref(),
        Some(DEPLOYMENT),
        "发布物给的部署 id 要记成本次必须一致的那个"
    );

    let nothing = client_with_dead_discovery();
    let err = nothing
        .sign_in_begin(SignInTarget::Server {
            email: EMAIL.to_owned(),
            input: website_that_404s(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CloudError::NoServerAtAddress { host } if host == "127.0.0.1"),
        "{err:?}"
    );
}

fn client_with_dead_discovery() -> CloudClient {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = client();
    *client.discovery_base.lock().unwrap() = Some(format!("http://127.0.0.1:{port}"));
    client
}

/// 没开始登录就调后面几步：说「没有进行中的登录」，不发请求。
#[tokio::test]
async fn later_steps_without_a_begun_sign_in_say_so() {
    let client = client();
    assert!(matches!(
        client.sign_in_password(PASSWORD, "L", "windows").await,
        Err(CloudError::NoPendingLogin)
    ));
    assert!(matches!(
        client.sign_in_request_code(CodeKind::Reset).await,
        Err(CloudError::NoPendingLogin)
    ));
    assert!(matches!(
        client.sign_in_code(None, "L", "windows").await,
        Err(CloudError::NoInvite)
    ));
}

/// 正式构建的官方后端只认 `public` 信任包：送来自签根就丢弃并报 tls，什么都不钉、不领会话；
/// 调试构建（联调服务端本就自签）照常采用（ADR-158 ④ 纵深防御）。
#[tokio::test]
async fn a_release_official_backend_refuses_a_pinned_root_bundle_debug_accepts_it() {
    let root_pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    for release in [true, false] {
        let mock = MockServer::start(DEPLOYMENT);
        mock.register(NORMALIZED, "password", PASSWORD);
        mock.with(|state| {
            state.bundle_tls = BundleTls::PinnedRoot {
                root_pem: root_pem.to_owned(),
            };
        });
        let client = at_first_contact(&mock);
        *client.official_policy.lock().unwrap() = Some((mock.base.clone(), release));
        let outcome = client.sign_in_password(PASSWORD, "Laptop", "windows").await;
        let bucket = bucket_of(&mock);
        if release {
            assert!(
                matches!(
                    outcome,
                    Err(CloudError::Network {
                        kind: crate::error::NetworkFailureKind::Tls,
                        ..
                    })
                ),
                "{outcome:?}"
            );
            assert!(client.tls_roots().pinned(&bucket).is_none());
            assert!(mock.bodies_to("/v1/auth/opaque/session").is_empty());
            assert!(!client.logged_in());
        } else {
            assert!(
                matches!(outcome, Ok(PasswordSignInDto::SignedIn)),
                "{outcome:?}"
            );
            assert!(client.tls_roots().pinned(&bucket).is_some());
            luna_model_fetch::pinned_tls::set_pinned_root(&mock.base, None);
        }
    }
}

/// 公网证书的信任包两种构建都照常采用；判定本体是纯函数。
#[test]
fn only_a_release_official_backend_is_restricted_to_public_bundles() {
    let pinned = BundleTls::PinnedRoot {
        root_pem: String::new(),
    };
    assert!(!official_bundle_allowed(true, &pinned));
    assert!(official_bundle_allowed(true, &BundleTls::Public));
    assert!(official_bundle_allowed(false, &pinned));
    assert!(official_bundle_allowed(false, &BundleTls::Public));
}
