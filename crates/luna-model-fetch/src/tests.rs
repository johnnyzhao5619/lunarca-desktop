//! 用真 TCP 假服务端测：续传、限流、换源、跳转白名单都是 HTTP 层的事实，桩掉 reqwest
//! 等于把要验的东西验掉（同 `src-tauri` 直连下载器测试的手法）。

use super::*;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Mutex;

const DEADLINE: Duration = Duration::from_secs(5);

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 一个连接的应答：收到请求头文本，返回响应字节。
type Responder = Box<dyn FnOnce(&str) -> Vec<u8> + Send>;

/// 依次服务 `responders.len()` 个连接；返回 (base_url, 线程句柄 → 各请求头文本)。
fn serve(responders: Vec<Responder>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for respond in responders {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(DEADLINE)).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let size = stream.read(&mut chunk).unwrap();
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..size]);
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            let _ = stream.write_all(&respond(&request));
            let _ = stream.flush();
            seen.push(request);
        }
        seen
    });
    (format!("http://{address}"), handle)
}

fn response(status: &str, extra_headers: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[derive(Default)]
struct Seen(Mutex<Vec<u64>>);

const PAYLOAD: &[u8] = b"lunarca-model-artifact-payload-0123456789";

async fn run(urls: &[String], dest: &Path, seen: &Seen) -> Result<(), FetchError> {
    download(
        &reqwest::Client::new(),
        urls,
        dest,
        PAYLOAD.len() as u64,
        &sha256_hex(PAYLOAD),
        &|n| seen.0.lock().unwrap().push(n),
    )
    .await
}

#[tokio::test]
async fn resume_hashes_the_existing_prefix_and_appends() {
    let (base, server) = serve(vec![Box::new(|req: &str| {
        assert!(req.to_lowercase().contains("range: bytes=10-"), "{req}");
        response("206 Partial Content", "", &PAYLOAD[10..])
    })]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    std::fs::write(&dest, &PAYLOAD[..10]).unwrap();
    let seen = Seen::default();
    run(&[format!("{base}/w")], &dest, &seen).await.unwrap();
    server.join().unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), PAYLOAD);
    assert_eq!(
        seen.0.lock().unwrap().first(),
        Some(&10),
        "progress starts at the resume point"
    );
}

#[tokio::test]
async fn a_server_ignoring_range_is_rewritten_from_zero() {
    let (base, server) = serve(vec![Box::new(|_: &str| response("200 OK", "", PAYLOAD))]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    std::fs::write(&dest, b"garbage!!!").unwrap();
    run(&[format!("{base}/w")], &dest, &Seen::default())
        .await
        .unwrap();
    server.join().unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), PAYLOAD);
}

#[tokio::test]
async fn falls_through_to_the_next_source_and_reports_every_failure_when_all_fail() {
    let (bad, bad_server) = serve(vec![Box::new(|_: &str| response("404 Not Found", "", b""))]);
    let (good, good_server) = serve(vec![Box::new(|_: &str| response("200 OK", "", PAYLOAD))]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    run(
        &[format!("{bad}/w"), format!("{good}/w")],
        &dest,
        &Seen::default(),
    )
    .await
    .unwrap();
    bad_server.join().unwrap();
    good_server.join().unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), PAYLOAD);

    let (a, a_server) = serve(vec![Box::new(|_: &str| response("404 Not Found", "", b""))]);
    let (b, b_server) = serve(vec![Box::new(|_: &str| {
        response("500 Internal Server Error", "", b"")
    })]);
    let err = run(
        &[format!("{a}/w?sig=SECRET"), format!("{b}/w")],
        &temp.path().join("x.part"),
        &Seen::default(),
    )
    .await
    .unwrap_err();
    a_server.join().unwrap();
    b_server.join().unwrap();
    let FetchError::Download { detail } = err else {
        panic!("expected Download, got {err:?}");
    };
    assert!(detail.contains("404") && detail.contains("500"), "{detail}");
    assert!(
        !detail.contains("SECRET"),
        "presigned query strings must not leak into errors: {detail}"
    );
}

#[tokio::test]
async fn a_checksum_failure_does_not_try_other_sources_and_deletes_the_part() {
    let (a, a_server) = serve(vec![Box::new(|_: &str| {
        let mut wrong = PAYLOAD.to_vec();
        wrong[0] ^= 1;
        response("200 OK", "", &wrong)
    })]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    // 第二个来源根本不应该被请求：它的监听没人连，测试也就不 join 它。
    let unused = "http://127.0.0.1:9/w".to_owned();
    let err = run(&[format!("{a}/w"), unused], &dest, &Seen::default())
        .await
        .unwrap_err();
    a_server.join().unwrap();
    assert_eq!(err, FetchError::ChecksumMismatch);
    assert!(!dest.exists());
}

#[tokio::test]
async fn oversized_bodies_are_cut_off_instead_of_filling_the_disk() {
    let (a, a_server) = serve(vec![Box::new(|_: &str| {
        let mut big = PAYLOAD.to_vec();
        big.extend_from_slice(&[0; 4096]);
        response("200 OK", "", &big)
    })]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    let err = run(&[format!("{a}/w")], &dest, &Seen::default())
        .await
        .unwrap_err();
    a_server.join().unwrap();
    assert!(matches!(err, FetchError::SizeMismatch { .. }), "{err:?}");
    assert!(!dest.exists());
}

#[tokio::test]
async fn throttling_waits_as_told_then_retries_the_same_source() {
    let (a, a_server) = serve(vec![
        Box::new(|_: &str| response("429 Too Many Requests", "Retry-After: 0\r\n", b"")),
        Box::new(|_: &str| {
            response(
                "503 Service Unavailable",
                "RateLimit: \"resolvers\";r=0;t=0\r\n",
                b"",
            )
        }),
        Box::new(|_: &str| response("200 OK", "", PAYLOAD)),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("w.part");
    run(&[format!("{a}/w")], &dest, &Seen::default())
        .await
        .unwrap();
    assert_eq!(a_server.join().unwrap().len(), 3);
}

#[test]
fn throttle_wait_reads_retry_after_then_hf_ratelimit() {
    let mut h = reqwest::header::HeaderMap::new();
    assert_eq!(throttle_wait(&h), Duration::from_secs(5));
    h.insert("ratelimit", "\"resolvers\";r=0;t=237".parse().unwrap());
    assert_eq!(throttle_wait(&h), Duration::from_secs(237));
    h.insert("retry-after", "12".parse().unwrap());
    assert_eq!(throttle_wait(&h), Duration::from_secs(12));
}

#[tokio::test]
async fn catalog_client_refuses_redirects_off_the_allowlist() {
    let (a, a_server) = serve(vec![Box::new(|_: &str| {
        response(
            "302 Found",
            "Location: http://127.0.0.1:9/elsewhere\r\n",
            b"",
        )
    })]);
    let temp = tempfile::tempdir().unwrap();
    let err = download(
        &catalog_client().unwrap(),
        &[format!("{a}/w")],
        &temp.path().join("w.part"),
        PAYLOAD.len() as u64,
        &sha256_hex(PAYLOAD),
        &|_| {},
    )
    .await
    .unwrap_err();
    a_server.join().unwrap();
    let FetchError::Download { detail } = err else {
        panic!("expected Download, got {err:?}");
    };
    assert!(detail.contains("non-allowlisted"), "{detail}");
}

#[test]
fn allowlist_matches_suffixes_not_substrings() {
    for ok in [
        "huggingface.co",
        "us.aws.cdn.hf.co",
        "cas-bridge.xethub.hf.co",
        "www.modelscope.cn",
        "release-assets.githubusercontent.com",
        "github.com",
    ] {
        assert!(host_allowed(ok), "{ok}");
    }
    for bad in [
        "evilhuggingface.co",
        "hf.co.evil.com",
        "example.com",
        "githubusercontent.com.attacker.net",
        "",
    ] {
        assert!(!host_allowed(bad), "{bad}");
    }
}

/// 实网：目录里两条真实来源的跳转链都在白名单内，且字节与目录声明一致。
/// `cargo test -p luna-model-fetch live_catalog_sources -- --ignored`
#[tokio::test]
#[ignore = "联网下载约 33 MB"]
async fn live_catalog_sources_follow_allowlisted_redirects() {
    let client = catalog_client().unwrap();
    let temp = tempfile::tempdir().unwrap();
    for id in ["silero-vad", "whisper-tiny-bundled"] {
        let entry = luna_model_catalog::find(id).unwrap();
        let dest = temp.path().join(format!("{id}.part"));
        download(
            &client,
            &entry.weights.urls(),
            &dest,
            entry.weights.size_bytes,
            &entry.weights.sha256,
            &|_| {},
        )
        .await
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}
