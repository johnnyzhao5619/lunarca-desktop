//! 纵切 3 C6 的客户端行为：注销前预览、注销回执与 503 `billing_unavailable`、账户数据导出的流式写盘
//! （含中途断开时半份文件的清理）、保存对话框取消时一个字节都不发；协议 v17 的「能不能删」与
//! 409 `last_org_owner`。
//!
//! 假服务端按连接逐条作答并计数——「没发请求」看的是连接数，而不是某个返回值。

use super::*;
use crate::account_data::{
    export_with_picker, AccountDeletionDto, AccountExportDto, DeletionBlockedReasonDto,
    DeletionSubscriptionDto, ExportDestinationPicker,
};
use crate::store::MemoryStore;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 每条连接：读完请求头，写出 `reply` 的各段（段与段之间刷一次，模拟分块到达），然后关闭。
/// 返回基址与连接计数。
fn serve(reply: Vec<Vec<u8>>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            counter.fetch_add(1, Ordering::SeqCst);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&buffer[..read]);
            }
            for part in &reply {
                if stream.write_all(part).is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    });
    (format!("http://{address}"), hits)
}

fn json_reply(status: u16, body: &str) -> Vec<Vec<u8>> {
    vec![format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()]
}

fn client_for(base: &str) -> CloudClient {
    let client = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| false));
    client.account().set_server(crate::account::AccountServer {
        origin: base.to_owned(),
        display_name: None,
    });
    client.reconfigure();
    client
}

fn signed_in(base: &str) -> CloudClient {
    let client = client_for(base);
    client.cred_set(KEY_ACCESS, "access").unwrap();
    client.cred_set(KEY_REFRESH, "refresh").unwrap();
    client
}

/// 记下被要求的默认文件名，并交回给定的路径（`None` = 用户取消）。
fn picker(answer: Option<PathBuf>) -> (ExportDestinationPicker, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&asked);
    let picker: ExportDestinationPicker = Arc::new(move |name| {
        log.lock().unwrap().push(name);
        let answer = answer.clone();
        Box::pin(async move { answer })
    });
    (picker, asked)
}

/// 目录里除了 `keep` 之外还剩什么（半份临时文件不得留下）。
fn leftovers(dir: &std::path::Path, keep: &[&str]) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !keep.contains(&name.as_str()))
        .collect()
}

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
}

#[tokio::test]
async fn deletion_preview_is_converted_to_credits() {
    let (base, _) = serve(json_reply(
        200,
        r#"{"deletion":{"state":"allowed"},"subscription":{"state":"active","tier":"max"},
            "prepaid_micro_credits":600000000,"billing_retention_years":7}"#,
    ));
    let preview = signed_in(&base).deletion_preview().await.unwrap();
    assert_eq!(preview.deletion, AccountDeletionDto::Allowed);
    assert_eq!(
        preview.subscription,
        DeletionSubscriptionDto::Active {
            tier: "max".to_owned()
        }
    );
    assert_eq!(preview.prepaid_credits, 600.0);
    assert_eq!(preview.billing_retention_years, 7);
}

/// 组织唯一所有者（协议 v17）：预览说「不能删」并带成因，界面据此收起确认。
#[tokio::test]
async fn deletion_preview_says_when_deletion_is_blocked() {
    let (base, _) = serve(json_reply(
        200,
        r#"{"deletion":{"state":"blocked","reason":"last_org_owner"},"subscription":{"state":"none"},
            "prepaid_micro_credits":0,"billing_retention_years":7}"#,
    ));
    let preview = signed_in(&base).deletion_preview().await.unwrap();
    assert_eq!(
        preview.deletion,
        AccountDeletionDto::Blocked {
            reason: DeletionBlockedReasonDto::LastOrgOwner
        }
    );
}

/// 409 `last_org_owner`：预览之后处境变了，服务端照样拦下、什么都没删——单独的错误码，**会话原样
/// 还在**。别的 409 照旧是通用服务端错误。
#[tokio::test]
async fn last_org_owner_deletes_nothing_and_keeps_the_session() {
    let (base, _) = serve(json_reply(409, r#"{"error":"last_org_owner"}"#));
    let client = signed_in(&base);
    let err = client.delete_account().await.unwrap_err();
    assert!(matches!(err, CloudError::LastOrgOwner), "{err:?}");
    assert!(client.logged_in(), "什么都没删，会话不该被清掉");

    let (base, _) = serve(json_reply(409, r#"{"error":"conflict"}"#));
    let err = signed_in(&base).delete_account().await.unwrap_err();
    assert!(
        matches!(err, CloudError::Server { status: 409, .. }),
        "{err:?}"
    );
}

/// 注销成功：回执只带保留年限，本地会话清掉。
#[tokio::test]
async fn deleting_returns_the_retention_and_clears_the_session() {
    let (base, _) = serve(json_reply(
        200,
        r#"{"deleted":true,"retained":{"billing_retention_years":7}}"#,
    ));
    let client = signed_in(&base);
    let receipt = client.delete_account().await.unwrap();
    assert_eq!(receipt.billing_retention_years, 7);
    assert!(!client.logged_in(), "注销成功后本地会话必须清掉");
}

/// 503 `billing_unavailable`：订阅退不掉、什么都没删——单独的错误码，**会话原样还在**。
/// 别的 503 照旧是通用服务端错误。
#[tokio::test]
async fn billing_unavailable_deletes_nothing_and_keeps_the_session() {
    let (base, _) = serve(json_reply(503, r#"{"error":"billing_unavailable"}"#));
    let client = signed_in(&base);
    let err = client.delete_account().await.unwrap_err();
    assert!(matches!(err, CloudError::BillingUnavailable), "{err:?}");
    assert!(client.logged_in(), "什么都没删，会话不该被清掉");

    let (base, _) = serve(json_reply(503, r#"{"error":"maintenance"}"#));
    let err = signed_in(&base).delete_account().await.unwrap_err();
    assert!(
        matches!(err, CloudError::Server { status: 503, .. }),
        "{err:?}"
    );
}

/// 服务端没说「已删除」：如实报读不懂，不清会话。
#[tokio::test]
async fn a_receipt_without_confirmation_keeps_the_session() {
    let (base, _) = serve(json_reply(
        200,
        r#"{"deleted":false,"retained":{"billing_retention_years":7}}"#,
    ));
    let client = signed_in(&base);
    let err = client.delete_account().await.unwrap_err();
    assert!(
        matches!(err, CloudError::UnreadableResponse { .. }),
        "{err:?}"
    );
    assert!(client.logged_in());
}

/// 导出分多块到达：逐块写进同目录的临时文件，收完改名成目标；目录里不留临时文件。
#[tokio::test]
async fn export_streams_the_body_into_the_chosen_file() {
    let body: String = (0..20_000)
        .map(|i| format!("{{\"n\":{i}}},"))
        .collect::<String>();
    let full = format!("[{}null]", body);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Content-Disposition: attachment\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        full.len()
    );
    let bytes = full.as_bytes();
    let middle = bytes.len() / 2;
    let (base, _) = serve(vec![
        head.into_bytes(),
        bytes[..middle].to_vec(),
        bytes[middle..].to_vec(),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export.json");
    signed_in(&base).export_account_to(&target).await.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), full);
    assert!(leftovers(dir.path(), &["export.json"]).is_empty());
}

/// 中途断开（宣称的长度没收齐连接就关了）：报错，半份临时文件删掉，原先在那里的同名文件原样不动。
#[tokio::test]
async fn an_interrupted_export_leaves_no_partial_file_and_keeps_the_old_one() {
    let (base, _) = serve(vec![
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\
          Connection: close\r\n\r\n{\"account\":"
            .to_vec(),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export.json");
    std::fs::write(&target, "previous export").unwrap();
    let err = signed_in(&base)
        .export_account_to(&target)
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::Network { .. }), "{err:?}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "previous export");
    assert!(
        leftovers(dir.path(), &["export.json"]).is_empty(),
        "半份临时文件没有删掉"
    );
}

/// 被限流：照既有的限流处置报，不建任何文件。
#[tokio::test]
async fn a_rate_limited_export_writes_nothing() {
    let (base, _) = serve(vec![
        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\n\
          Content-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("export.json");
    let err = signed_in(&base)
        .export_account_to(&target)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            CloudError::RateLimited {
                retry_after_secs: Some(30)
            }
        ),
        "{err:?}"
    );
    assert!(leftovers(dir.path(), &[]).is_empty());
}

/// 写不进选中的位置（目录不存在）：写盘失败，不是网络失败。
#[tokio::test]
async fn an_unwritable_destination_is_a_write_failure() {
    let (base, _) = serve(json_reply(200, "{}"));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("missing").join("export.json");
    let err = signed_in(&base)
        .export_account_to(&target)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CloudError::ExportWriteFailed { .. }),
        "{err:?}"
    );
}

/// 用户取消保存对话框：一个字节都不发；对话框收到的是带日期的默认名。
#[tokio::test]
async fn cancelling_the_dialog_sends_nothing() {
    let (base, hits) = serve(json_reply(200, "{}"));
    let (pick, asked) = picker(None);
    let outcome = export_with_picker(&signed_in(&base), &pick, today())
        .await
        .unwrap();
    assert_eq!(outcome, AccountExportDto::Cancelled);
    assert_eq!(
        *asked.lock().unwrap(),
        ["lunarca-account-export-2026-10-14.json"]
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hits.load(Ordering::SeqCst), 0, "取消之后发出了请求");
}

/// 未登录 / 严格本地：连对话框都不弹（不让用户先选位置再被告知不行），也不发请求。
#[tokio::test]
async fn without_a_session_the_dialog_never_opens() {
    let (base, hits) = serve(json_reply(200, "{}"));
    let (pick, asked) = picker(Some(PathBuf::from("/nowhere/export.json")));
    let err = export_with_picker(&client_for(&base), &pick, today())
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::NotLoggedIn), "{err:?}");

    let strict = CloudClient::new(Box::new(MemoryStore::new()), Arc::new(|| true));
    let err = export_with_picker(&strict, &pick, today())
        .await
        .unwrap_err();
    assert!(matches!(err, CloudError::StrictLocalBlocked), "{err:?}");

    assert!(asked.lock().unwrap().is_empty(), "没有会话却弹了对话框");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// 选好位置：写进去并交回那个路径。
#[tokio::test]
async fn a_chosen_destination_is_written_and_reported() {
    let (base, hits) = serve(json_reply(200, r#"{"account":{"tier":"pro"}}"#));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("mine.json");
    let (pick, _) = picker(Some(target.clone()));
    let outcome = export_with_picker(&signed_in(&base), &pick, today())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        AccountExportDto::Saved {
            path: target.to_string_lossy().into_owned()
        }
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        r#"{"account":{"tier":"pro"}}"#
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
