//! 纵切 3 C5 的客户端行为：设备指纹随每一条建设备会话的登录请求发出、撞上设备上限后「移除一台并登录」
//! （应用内登录的领取会话 / 注册口令 + 设备码）、令牌失效、免费额度被扣下与付款方式验证的错误映射。
//!
//! 假服务端按路径**依次**作答并逐条记下请求体——断言看的是线上真实发出去的字节，而不是某个返回值。

use super::*;
use crate::account::AccountPhaseDto;
use crate::fingerprint::{derive, FingerprintProvider};
use crate::store::MemoryStore;
use std::io::{Read, Write};
use std::net::TcpListener;

const HARDWARE_ID: &str = "TEST-HARDWARE-UUID-0001";

const LIMIT_BODY: &str = r#"{"error":"device_limit_reached","max":2,
    "devices":[{"id":"d1","name":"Mac mini","platform":"macos","last_seen_at":"2026-10-01T08:00:00Z"},
               {"id":"d2","name":"Old PC","platform":"windows","last_seen_at":null}],
    "revoke_token":"rvk_secret_token","revoke_token_expires_at":"2026-10-14T12:10:00Z"}"#;
const TOKENS: &str = r#"{"access_token":"acc_1","refresh_token":"ref_1"}"#;
const ME: &str = r#"{"account_id":"acc","email":"a@b.test"}"#;

/// 记下的一条请求：`(方法, 路径, 请求体)`。
type Seen = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

/// 按脚本作答的假服务端：同一路径的多条答复按出现顺序依次用掉；脚本里没有（或已用完）的回 404。
fn serve_script(script: Vec<(&'static str, u16, &'static str)>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut remaining = script;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let (method, path, body) = read_request(&mut stream);
            log.lock().unwrap().push((method, path.clone(), body));
            let reply = remaining
                .iter()
                .position(|(route, _, _)| *route == path)
                .map(|index| remaining.remove(index));
            let response = match reply {
                Some((_, status, body)) => format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
            };
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{address}"), seen)
}

/// 读完整的一条请求（头 + `Content-Length` 指定长度的体）。
fn read_request(stream: &mut std::net::TcpStream) -> (String, String, serde_json::Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
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
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
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
    (method, path, body)
}

fn client_for(api_base: &str) -> CloudClient {
    let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false))
        .with_fingerprint(FingerprintProvider::with_source(Arc::new(|| {
            Some(HARDWARE_ID.to_owned())
        })));
    client.account().set_server(crate::account::AccountServer {
        origin: api_base.to_owned(),
        display_name: None,
    });
    client.reconfigure();
    client
}

fn expected_fingerprint() -> String {
    derive(HARDWARE_ID).as_str().to_owned()
}

fn bodies_to(seen: &Seen, path: &str) -> Vec<serde_json::Value> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(_, at, _)| at == path)
        .map(|(_, _, body)| body.clone())
        .collect()
}

const SESSION: &str = "/v1/auth/opaque/session";
const REGISTER_FINISH: &str = "/v1/auth/opaque/register/finish";

/// 应用内登录最后一步的那条请求（领取会话 / 注册口令）：握手本身由 `client_opaque_tests` 覆盖，这里只看
/// 撞上设备上限之后的去留。
async fn grant(client: &CloudClient, path: &'static str) -> Option<serde_json::Value> {
    let bearer = (path == REGISTER_FINISH).then(|| zeroize::Zeroizing::new("rg_grant".to_owned()));
    client
        .send_grant(
            path,
            serde_json::json!({
                "login_id": "l1",
                "claim": "claim-secret",
                "device_fingerprint": client.fingerprint().get().as_str(),
            }),
            bearer,
        )
        .await
        .unwrap()
}

async fn claim(client: &CloudClient) {
    assert!(grant(client, SESSION).await.is_none(), "应撞上设备上限");
}

/// 两条建设备会话的应用内请求都带同一枚指纹；撞上限 → 选一台移除 → 原样重发（含同一个 Bearer）→ 登录成功。
#[tokio::test]
async fn every_in_app_grant_resolves_a_device_limit_by_resending_it() {
    for path in [SESSION, REGISTER_FINISH] {
        let (base, seen) = serve_script(vec![
            (path, 403, LIMIT_BODY),
            ("/v1/devices/revoke-for-sign-in", 204, ""),
            (path, 200, TOKENS),
            ("/v1/me", 200, ME),
        ]);
        let client = client_for(&base);

        assert!(grant(&client, path).await.is_none(), "{path}");
        assert!(!client.logged_in(), "{path}：撞上限不是登录成功");
        let AccountPhaseDto::DeviceLimit { limit } = client.account_state().phase else {
            panic!("{path}：账户状态应转到「选一台移除」");
        };
        assert_eq!(limit.max, 2);
        assert_eq!(limit.devices.len(), 2);
        assert_eq!(limit.devices[0].name, "Mac mini");
        assert_eq!(limit.expires_at, "2026-10-14T12:10:00Z");

        let resolved = client.sign_in_resolve_device_limit("d2").await.unwrap();
        assert_eq!(resolved, DeviceLimitResolutionDto::SignedIn, "{path}");
        assert!(client.logged_in(), "{path}");
        assert!(client.device_limit().is_none(), "选完即清");
        assert!(matches!(
            client.account_state().phase,
            AccountPhaseDto::SignedIn { .. }
        ));

        // 线上：两次请求逐字相同，都带指纹；撤销请求只带令牌与那台设备。
        let sent = bodies_to(&seen, path);
        assert_eq!(sent.len(), 2, "{path}");
        assert_eq!(sent[0], sent[1], "{path}：重发必须是同一条请求");
        assert_eq!(
            sent[0]["device_fingerprint"].as_str(),
            Some(expected_fingerprint().as_str()),
            "{path}"
        );
        assert!(
            !sent[0].to_string().contains(HARDWARE_ID),
            "原始硬件标识不出本机"
        );
        let revoke = bodies_to(&seen, "/v1/devices/revoke-for-sign-in");
        assert_eq!(
            revoke,
            vec![serde_json::json!({ "revoke_token": "rvk_secret_token", "device_id": "d2" })]
        );
    }
}

/// 设备码：发起帧与轮询帧都带指纹；撞上限时配对留着、状态是「选一台」而不是「等浏览器」；
/// 移除之后接着轮询同一个配对。
#[tokio::test]
async fn device_code_keeps_the_pairing_and_polls_it_again_after_removal() {
    let code = r#"{"device_code":"ldc_x","user_code":"ABCD-EFGH","verification_uri":"http://v","interval":0,"expires_in":600}"#;
    let (base, seen) = serve_script(vec![
        ("/v1/device/code", 200, code),
        ("/v1/device/token", 403, LIMIT_BODY),
        ("/v1/devices/revoke-for-sign-in", 204, ""),
        ("/v1/device/token", 200, TOKENS),
        ("/v1/me", 200, ME),
    ]);
    let client = client_for(&base);
    client
        .login_start("Mac".into(), "macos".into())
        .await
        .unwrap();
    let poll = client.login_poll().await.unwrap();
    assert_eq!(poll.status, LoginPollStatus::DeviceLimitReached);
    assert!(
        client.pending_login().is_some(),
        "配对留着，移除之后接着轮询"
    );
    assert!(matches!(
        client.account_state().phase,
        AccountPhaseDto::DeviceLimit { .. }
    ));

    let resolved = client.sign_in_resolve_device_limit("d1").await.unwrap();
    assert_eq!(resolved, DeviceLimitResolutionDto::SignedIn);
    assert!(client.logged_in());
    assert!(client.pending_login().is_none());

    let fingerprint = expected_fingerprint();
    let starts = bodies_to(&seen, "/v1/device/code");
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0]["device_fingerprint"].as_str(),
        Some(fingerprint.as_str()),
        "指纹随发起帧登记"
    );
    for body in bodies_to(&seen, "/v1/device/token") {
        assert!(
            body.get("device_fingerprint").is_none(),
            "轮询帧不带指纹：{body}"
        );
    }
    let polls = bodies_to(&seen, "/v1/device/token");
    assert_eq!(polls.len(), 2);
    assert_eq!(
        polls[0]["device_code"], polls[1]["device_code"],
        "同一个配对"
    );
}

/// 移除之后浏览器那边还没批准完：回到「等浏览器」，轮询由命令层接着跑。
#[tokio::test]
async fn device_code_still_pending_after_removal_goes_back_to_waiting() {
    let code = r#"{"device_code":"ldc_x","user_code":"ABCD-EFGH","verification_uri":"http://v","interval":0,"expires_in":600}"#;
    let (base, _seen) = serve_script(vec![
        ("/v1/device/code", 200, code),
        ("/v1/device/token", 403, LIMIT_BODY),
        ("/v1/devices/revoke-for-sign-in", 204, ""),
        (
            "/v1/device/token",
            400,
            r#"{"error":"authorization_pending"}"#,
        ),
    ]);
    let client = client_for(&base);
    client
        .login_start("Mac".into(), "macos".into())
        .await
        .unwrap();
    client.login_poll().await.unwrap();
    let resolved = client.sign_in_resolve_device_limit("d1").await.unwrap();
    assert_eq!(resolved, DeviceLimitResolutionDto::AwaitingBrowser);
    assert!(matches!(
        client.account_state().phase,
        AccountPhaseDto::AwaitingBrowser { .. }
    ));
    assert!(client.pending_poll_ms().is_some());
}

/// 令牌过期 / 已用（撤销被 4xx 拒）：挂着的选择清掉、报 `DeviceLimitExpired`；设备码那一路连配对一起
/// 丢掉，界面回到登录而不是停在一个永远「设备已满」的配对上。
#[tokio::test]
async fn an_expired_revoke_token_clears_the_choice_and_restarts_sign_in() {
    for status in [400_u16, 401, 403, 404, 410] {
        // 应用内登录那一路。
        let (base, seen) = serve_script(vec![
            (SESSION, 403, LIMIT_BODY),
            (
                "/v1/devices/revoke-for-sign-in",
                status,
                r#"{"error":"revoke_token_invalid"}"#,
            ),
        ]);
        let client = client_for(&base);
        claim(&client).await;
        let err = client.sign_in_resolve_device_limit("d1").await.unwrap_err();
        assert!(
            matches!(err, CloudError::DeviceLimitExpired),
            "{status}: {err:?}"
        );
        assert!(client.device_limit().is_none(), "{status}");
        assert!(matches!(
            client.account_state().phase,
            AccountPhaseDto::SignedOut
        ));
        assert_eq!(
            bodies_to(&seen, SESSION).len(),
            1,
            "{status}：令牌失效不该重发登录"
        );
    }

    // 设备码那一路。
    let code = r#"{"device_code":"ldc_x","user_code":"ABCD-EFGH","verification_uri":"http://v","interval":0,"expires_in":600}"#;
    let (base, _seen) = serve_script(vec![
        ("/v1/device/code", 200, code),
        ("/v1/device/token", 403, LIMIT_BODY),
        (
            "/v1/devices/revoke-for-sign-in",
            410,
            r#"{"error":"revoke_token_expired"}"#,
        ),
    ]);
    let client = client_for(&base);
    client
        .login_start("Mac".into(), "macos".into())
        .await
        .unwrap();
    client.login_poll().await.unwrap();
    let err = client.sign_in_resolve_device_limit("d1").await.unwrap_err();
    assert!(matches!(err, CloudError::DeviceLimitExpired));
    assert!(client.pending_login().is_none(), "配对一并作废");
    assert!(matches!(
        client.account_state().phase,
        AccountPhaseDto::SignedOut
    ));
}

/// 连不上 / 服务端出错 / 限频：令牌没被用掉，选择留着，用户可以再点一次。
#[tokio::test]
async fn a_server_error_on_removal_keeps_the_choice() {
    for (status, body) in [
        (503_u16, r#"{"error":"internal"}"#),
        (429, r#"{"error":"rate_limited"}"#),
    ] {
        let (base, _seen) = serve_script(vec![
            (SESSION, 403, LIMIT_BODY),
            ("/v1/devices/revoke-for-sign-in", status, body),
        ]);
        let client = client_for(&base);
        claim(&client).await;
        let err = client.sign_in_resolve_device_limit("d1").await.unwrap_err();
        assert!(
            matches!(
                err,
                CloudError::Server { status: 503, .. } | CloudError::RateLimited { .. }
            ),
            "{status}: {err:?}"
        );
        assert!(client.device_limit().is_some(), "{status}");
    }
}

/// 界面交来服务端没列出的设备、或根本没有挂着的选择：不发任何请求。
#[tokio::test]
async fn removal_only_targets_a_listed_device_of_a_live_choice() {
    let (base, seen) = serve_script(vec![(SESSION, 403, LIMIT_BODY)]);
    let client = client_for(&base);
    assert!(matches!(
        client.sign_in_resolve_device_limit("d1").await,
        Err(CloudError::DeviceLimitExpired)
    ));
    claim(&client).await;
    assert!(matches!(
        client.sign_in_resolve_device_limit("not-listed").await,
        Err(CloudError::Internal { .. })
    ));
    assert!(bodies_to(&seen, "/v1/devices/revoke-for-sign-in").is_empty());
    assert!(client.device_limit().is_some(), "交错了 id 不该丢掉选择");

    // 取消即清（令牌与要重发的领取一起丢掉）。
    client.sign_in_cancel();
    assert!(client.device_limit().is_none());
}

/// 另起一次登录 = 上一次撞上限的那一步作废（它的令牌属于上一次请求）。
#[tokio::test]
async fn a_new_sign_in_attempt_drops_the_previous_choice() {
    let (base, _seen) = serve_script(vec![(SESSION, 403, LIMIT_BODY)]);
    let client = client_for(&base);
    claim(&client).await;
    assert!(client.device_limit().is_some());
    let err = client
        .sign_in_password("pw", "Mac", "macos")
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::NoPendingLogin));
    assert!(client.device_limit().is_none());
}

/// 一次性邮箱域名：注册被拒说「换常用邮箱」，不落成通用服务端错误。
#[tokio::test]
async fn a_disallowed_email_domain_has_its_own_code() {
    let (base, _seen) = serve_script(vec![(
        REGISTER_FINISH,
        400,
        r#"{"error":"email_domain_not_allowed"}"#,
    )]);
    let client = client_for(&base);
    let err = client
        .send_grant(REGISTER_FINISH, serde_json::json!({}), None)
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::EmailDomainNotAllowed), "{err:?}");

    // 控制面通用映射同样认它（状态与码成对才算）。
    let (base, _seen) = serve_script(vec![(
        "/v1/usage",
        400,
        r#"{"error":"email_domain_not_allowed"}"#,
    )]);
    let client = client_for(&base);
    client.cred_set(KEY_ACCESS, "access").unwrap();
    client.cred_set(KEY_REFRESH, "refresh").unwrap();
    assert!(matches!(
        client.usage().await,
        Err(CloudError::EmailDomainNotAllowed)
    ));
}

/// 402 `free_quota_withheld` 在控制面也有自己的码；402 别的体照旧通用服务端错误。
#[tokio::test]
async fn a_withheld_free_quota_maps_to_its_own_cloud_error() {
    for (body, withheld) in [
        (r#"{"error":"free_quota_withheld"}"#, true),
        (r#"{"error":"quota_exhausted"}"#, false),
    ] {
        let (base, _seen) = serve_script(vec![("/v1/usage", 402, body)]);
        let client = client_for(&base);
        client.cred_set(KEY_ACCESS, "access").unwrap();
        client.cred_set(KEY_REFRESH, "refresh").unwrap();
        let err = client.usage().await.unwrap_err();
        assert_eq!(
            matches!(err, CloudError::FreeQuotaWithheld),
            withheld,
            "{body}: {err:?}"
        );
    }
}

/// 付款方式验证：成功交回地址（只在 Rust 里流转）；409 = 已不需要验证；503 = 本部署不卖；未登录零网络。
#[tokio::test]
async fn verify_card_returns_the_hosted_url_or_says_why_not() {
    let (base, seen) = serve_script(vec![
        (
            "/v1/billing/verify-card",
            200,
            r#"{"url":"https://checkout.stripe.test/c/pay/cs_x"}"#,
        ),
        (
            "/v1/billing/verify-card",
            409,
            r#"{"error":"verification_not_required"}"#,
        ),
        (
            "/v1/billing/verify-card",
            503,
            r#"{"error":"billing_disabled"}"#,
        ),
    ]);
    let client = client_for(&base);
    assert!(matches!(
        client.billing_verify_card_url().await,
        Err(CloudError::NotLoggedIn)
    ));
    assert!(seen.lock().unwrap().is_empty(), "未登录一个字节都不发");

    client.cred_set(KEY_ACCESS, "access").unwrap();
    client.cred_set(KEY_REFRESH, "refresh").unwrap();
    assert_eq!(
        client.billing_verify_card_url().await.unwrap(),
        "https://checkout.stripe.test/c/pay/cs_x"
    );
    assert!(matches!(
        client.billing_verify_card_url().await,
        Err(CloudError::VerificationNotRequired)
    ));
    assert!(matches!(
        client.billing_verify_card_url().await,
        Err(CloudError::BillingDisabled)
    ));
}

/// 指纹与撤销令牌不进日志：本 crate 每一处 `eprintln!` 的参数里都不出现它们（读源码判定——日志
/// 走宿主，单元测试抓不到 stderr，而一行带值的日志只需要有人在格式串里多写一个变量）。
#[test]
fn no_log_line_carries_the_fingerprint_or_the_revoke_token() {
    let sources = [
        ("account.rs", include_str!("account.rs")),
        ("billing.rs", include_str!("billing.rs")),
        ("client.rs", include_str!("client.rs")),
        ("commands.rs", include_str!("commands.rs")),
        ("device_limit.rs", include_str!("device_limit.rs")),
        ("fingerprint.rs", include_str!("fingerprint.rs")),
        ("lib.rs", include_str!("lib.rs")),
    ];
    for (file, source) in sources {
        let mut rest = source;
        while let Some(at) = rest.find("eprintln!(") {
            let call = &rest[at..];
            let end = call.find(");").map_or(call.len(), |end| end + 2);
            let invocation = &call[..end];
            for secret in [
                "fingerprint",
                "hardware_id",
                "revoke_token",
                "DeviceLimitPending",
                "fallback_path",
            ] {
                assert!(
                    !invocation.contains(secret),
                    "{file}: 日志里出现了 `{secret}`：{invocation}"
                );
            }
            rest = &call[end..];
        }
    }
}

/// 错误与界面 DTO 的序列化里没有令牌与指纹（错误串会进日志，DTO 会过 IPC）。
#[tokio::test]
async fn neither_errors_nor_the_ipc_state_carry_secrets() {
    let (base, _seen) = serve_script(vec![(SESSION, 403, LIMIT_BODY)]);
    let client = client_for(&base);
    claim(&client).await;
    let state = serde_json::to_string(&client.account_state()).unwrap();
    let fingerprint = expected_fingerprint();
    for secret in [
        "rvk_secret_token",
        "claim-secret",
        fingerprint.as_str(),
        HARDWARE_ID,
    ] {
        assert!(!state.contains(secret), "账户状态里出现了秘密");
    }
    let pending = format!(
        "{:?}",
        client.device_limit.lock().unwrap().as_ref().unwrap()
    );
    for secret in ["rvk_secret_token", "claim-secret"] {
        assert!(!pending.contains(secret), "Debug 打出了秘密");
    }
}
