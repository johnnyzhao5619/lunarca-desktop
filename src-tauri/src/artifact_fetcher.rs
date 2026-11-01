//! 工件下载接缝（TDD-14 §3.2 ②）：模型安装的「取字节到本地文件」层。
//!
//! OSS 默认实现 [`DirectArtifactFetcher`] 即现行直连路径（直连 URL + 断点续传 +
//! sha256 终验），**原样搬移**自 `install_luna_managed_model` 的下载/校验段——真实
//! 消费者，非占位。官方构建（商业仓 `luna-cloud`）以同一契约实现制品面路径
//! （digest → 有时效镜像集 → 断点续传 → 本地 sha256 终验，TDD-14 §3.1）；目录客户端
//! 状态机的类型与实现归 `luna-cloud`，公开仓不留空壳。
//!
//! 接缝刻意只覆盖「字节从哪来」：临时目录布局、归档提取、manifest 落盘与最终改名等
//! 安装编排仍在调用侧（`ipc::install_luna_managed_model`）——本层回调只交付「累计已下载
//! 字节」这一个事实，把它翻译成阶段并广播到前端是编排侧
//! [`crate::model_store::ModelInstallProgressSink`] 的职责。

use std::path::Path;

/// 一次工件下载请求。
pub struct ArtifactRequest<'a> {
    /// 按顺序尝试的下载地址（目录条目的来源，ADR-144 ①）；至少一个。
    pub urls: &'a [String],
    /// 本地落点（`.part` 暂存文件）：已存在的部分字节按实现的断点语义续传；
    /// 成功返回时该文件字节完整且已通过 sha256 终验。
    pub dest: &'a Path,
    /// 期望字节数（直连实现据此判定续传起点与完整性）。
    pub expected_size_bytes: u64,
    /// 期望 sha256（小写十六进制）。本地终验以此为准——镜像 URL 可换、工件身份不变
    /// （TDD-14 §3.1 制品面不变量）。
    pub expected_sha256: &'a str,
}

/// 下载进度回调：参数 = 累计已下载字节（续传时含既有起点）。调用节奏与既有直连路径
/// 一致：开始时一次 + 每个 chunk 一次；事件构造/节流归调用侧。
pub type ArtifactProgress<'a> = dyn Fn(u64) + Send + Sync + 'a;

/// 「取字节到本地文件」抽象（TDD-14 §3.2 ②）。
#[async_trait::async_trait]
pub trait ArtifactFetcher: Send + Sync {
    /// 把 `request.urls` 指向的字节取到 `request.dest`，完成大小与 sha256 终验。
    ///
    /// # Errors
    /// 网络失败 / IO 失败 / 大小不符 / 摘要不符时返回稳定错误码字符串
    /// （`MODEL_DOWNLOAD_FAILED` / `MODEL_INSTALL_FAILED` / `MODEL_CHECKSUM_FAILED`），
    /// 与既有直连路径完全一致。
    async fn fetch(
        &self,
        request: ArtifactRequest<'_>,
        on_progress: &ArtifactProgress<'_>,
    ) -> Result<(), String>;
}

/// 直连实现（OSS 默认）：按目录来源顺序取（`luna-model-fetch`：断点续传、流式 sha256、
/// 跳转白名单、限流退避），错误码与此前一致。
pub struct DirectArtifactFetcher;

/// `luna-model-fetch` 的失败分类 → 模型安装的稳定错误码（前端据此出文案）。
pub(crate) fn fetch_error_code(error: luna_model_fetch::FetchError) -> String {
    use luna_model_fetch::FetchError;
    match error {
        FetchError::Download { detail } => format!("MODEL_DOWNLOAD_FAILED: {detail}"),
        FetchError::Install { detail } => format!("MODEL_INSTALL_FAILED: {detail}"),
        FetchError::SizeMismatch { expected, actual } => {
            format!("MODEL_CHECKSUM_FAILED: 下载大小不匹配，期望 {expected}，实际 {actual}")
        }
        FetchError::ChecksumMismatch => "MODEL_CHECKSUM_FAILED: sha256 校验失败".to_owned(),
    }
}

#[async_trait::async_trait]
impl ArtifactFetcher for DirectArtifactFetcher {
    async fn fetch(
        &self,
        request: ArtifactRequest<'_>,
        on_progress: &ArtifactProgress<'_>,
    ) -> Result<(), String> {
        let client = luna_model_fetch::catalog_client()
            .map_err(|e| format!("MODEL_DOWNLOAD_FAILED: {e}"))?;
        luna_model_fetch::download(
            &client,
            request.urls,
            request.dest,
            request.expected_size_bytes,
            request.expected_sha256,
            on_progress,
        )
        .await
        .map_err(fetch_error_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::Duration;

    /// 假服务端的读超时/收摊死线：绝不让一条挂住的连接把测试变成「跑一小时、零输出」。
    const DEADLINE: Duration = Duration::from_secs(5);

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    /// 起一个只服务一次的本地假服务端。`respond` 收到完整请求头文本（原样），返回响应字节。
    ///
    /// 返回 (base_url, 线程句柄 → 它读到的请求头文本)。用真 TCP 而不是打桩 reqwest：
    /// 断点续传的全部实质就是「发没发 `Range`、回的是 206 还是 200」，那是 HTTP 层的事实，
    /// 桩掉它等于把要验的东西验掉。
    fn serve_once(
        respond: impl FnOnce(&str) -> Vec<u8> + Send + 'static,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(DEADLINE)).unwrap();
            // 读到请求头结束为止：GET 无 body，但 TCP 可能把头拆成多段送达。
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
            stream.write_all(&respond(&request)).unwrap();
            stream.flush().unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    /// 组装一条 HTTP 响应（状态行由调用方给，`Connection: close` 让 reqwest 立刻收到 EOF）。
    fn response(status: &str, extra_headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// 记录进度回调实参（累计已下载字节）。
    #[derive(Default)]
    struct Progress(Mutex<Vec<u64>>);

    impl Progress {
        fn seen(&self) -> Vec<u64> {
            self.0.lock().unwrap().clone()
        }
    }

    /// 全新下载：不发 `Range`，进度从 0 起算，字节与摘要都对得上。
    #[tokio::test]
    async fn a_fresh_download_sends_no_range_header_and_starts_from_zero() {
        let payload = b"lunarca-artifact-payload".to_vec();
        let body = payload.clone();
        let (base, server) = serve_once(move |_| response("200 OK", "", &body));
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");
        let progress = Progress::default();

        DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: payload.len() as u64,
                    expected_sha256: &sha256_hex(&payload),
                },
                &|downloaded| progress.0.lock().unwrap().push(downloaded),
            )
            .await
            .expect("全新下载必须成功");

        let request = server.join().unwrap().to_lowercase();
        assert!(
            !request.contains("range:"),
            "本地没有部分字节却发了 Range——服务端会回 206 而我们会把它当全量写下去"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert_eq!(progress.seen().first().copied(), Some(0));
        assert_eq!(progress.seen().last().copied(), Some(payload.len() as u64));
    }

    /// **206 = 续传：新字节必须 append 在既有字节之后。**
    ///
    /// 写错成截断，最终文件就只有后半段——大小恰好也可能对不上，但真正致命的是
    /// 「大小对得上、内容是错的」那一档，只有 sha256 拦得住，而那时用户已经等完了整次下载。
    #[tokio::test]
    async fn a_206_response_appends_to_the_existing_partial_file() {
        let payload = b"lunarca-artifact-payload".to_vec();
        let head = &payload[..10];
        let tail = payload[10..].to_vec();
        let total = payload.len();
        let (base, server) = serve_once(move |request| {
            assert!(
                request.to_lowercase().contains("range: bytes=10-"),
                "本地已有 10 字节却没有请求续传：{request}"
            );
            response(
                "206 Partial Content",
                &format!("Content-Range: bytes 10-{}/{total}\r\n", total - 1),
                &tail,
            )
        });
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");
        std::fs::write(&dest, head).unwrap();
        let progress = Progress::default();

        DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: payload.len() as u64,
                    expected_sha256: &sha256_hex(&payload),
                },
                &|downloaded| progress.0.lock().unwrap().push(downloaded),
            )
            .await
            .expect("续传必须成功");

        server.join().unwrap();
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            payload,
            "续传把已有字节覆盖掉了"
        );
        assert_eq!(
            progress.seen().first().copied(),
            Some(10),
            "续传的进度必须从断点起算，而不是从 0 —— 否则进度条会在重试时倒退"
        );
    }

    /// **200 = 服务端不认续传：本地部分字节必须被截断丢弃。**
    ///
    /// 附加在旧字节后面会得到一个「前半段是陈旧/错误字节」的文件；镜像换机、URL 换内容、
    /// 代理不支持 Range 时这条随时会走到。
    #[tokio::test]
    async fn a_200_response_truncates_the_stale_partial_file() {
        let payload = b"lunarca-artifact-payload".to_vec();
        let body = payload.clone();
        let (base, server) = serve_once(move |_| response("200 OK", "", &body));
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");
        std::fs::write(&dest, b"stale-bytes-from-another-mirror").unwrap();
        let progress = Progress::default();

        DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: payload.len() as u64,
                    expected_sha256: &sha256_hex(&payload),
                },
                &|downloaded| progress.0.lock().unwrap().push(downloaded),
            )
            .await
            .expect("服务端回全量时必须重下并成功");

        server.join().unwrap();
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            payload,
            "陈旧字节没有被截断，最终文件被污染"
        );
        assert_eq!(
            progress.seen().first().copied(),
            Some(0),
            "回落全量时进度必须从 0 起算"
        );
    }

    /// 大小不符 → `MODEL_CHECKSUM_FAILED`，且**部分文件被删掉**。
    ///
    /// 留着它下次会被当成断点续传的起点，而它的长度已经等于/超过期望——续传请求会从
    /// 文件末尾要字节，服务端回 416，于是这个模型再也装不上。
    #[tokio::test]
    async fn a_size_mismatch_reports_checksum_failed_and_deletes_the_partial() {
        let short = b"too-short".to_vec();
        let (base, server) = serve_once(move |_| response("200 OK", "", &short));
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");

        let err = DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: 4096,
                    expected_sha256: &sha256_hex(b"whatever"),
                },
                &|_| {},
            )
            .await
            .expect_err("大小不符必须失败");

        server.join().unwrap();
        assert!(err.starts_with("MODEL_CHECKSUM_FAILED"), "错误码：{err}");
        assert!(err.contains("下载大小不匹配"), "错误文案：{err}");
        assert!(
            !dest.exists(),
            "部分文件没被删——下次续传会从它续起并永远失败"
        );
    }

    /// sha256 不符 → `MODEL_CHECKSUM_FAILED`，且**部分文件被删掉**。
    ///
    /// 这一档最危险：字节数完全正确，只有内容是错的（镜像被投毒、CDN 回了错误对象）。
    /// 留下这个文件等于把一份坏权重缓存下来，而它每次都能通过「大小检查」。
    #[tokio::test]
    async fn a_digest_mismatch_reports_checksum_failed_and_deletes_the_partial() {
        let payload = b"lunarca-artifact-payload".to_vec();
        let size = payload.len() as u64;
        let body = payload.clone();
        let (base, server) = serve_once(move |_| response("200 OK", "", &body));
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");

        let err = DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: size,
                    // 大小对得上，摘要对不上——只有终验拦得住。
                    expected_sha256: &sha256_hex(b"a completely different artifact"),
                },
                &|_| {},
            )
            .await
            .expect_err("摘要不符必须失败");

        server.join().unwrap();
        assert!(err.starts_with("MODEL_CHECKSUM_FAILED"), "错误码：{err}");
        assert!(err.contains("sha256"), "错误文案：{err}");
        assert!(!dest.exists(), "坏权重被缓存下来了");
    }

    /// 非 2xx → `MODEL_DOWNLOAD_FAILED`（与安装编排的清理规则配套：这一档留下的
    /// 断点字节是可续传的，编排刻意不删）。
    #[tokio::test]
    async fn a_non_success_status_reports_download_failed() {
        let (base, server) = serve_once(move |_| response("503 Service Unavailable", "", b""));
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("weights.bin.part");

        let err = DirectArtifactFetcher
            .fetch(
                ArtifactRequest {
                    urls: &[format!("{base}/artifact.bin")],
                    dest: &dest,
                    expected_size_bytes: 4096,
                    expected_sha256: &sha256_hex(b"whatever"),
                },
                &|_| {},
            )
            .await
            .expect_err("503 必须失败");

        server.join().unwrap();
        assert!(err.starts_with("MODEL_DOWNLOAD_FAILED"), "错误码：{err}");
    }
}
