//! `luna-model-fetch` — 模型工件的下载与识别（ADR-144），客户端与服务端运维工具共用。
//!
//! - [`download`]：按顺序尝试多个来源，单源内断点续传（`Range` / 206）、**边写边算 sha256**
//!   （不再把整个文件读回内存——GGUF 动辄数 GB）、大小与摘要终验；429 / 503 按
//!   `Retry-After` 或 HF 的 `RateLimit` 头退避后重试。
//! - [`catalog_client`]：目录来源专用的 HTTP 客户端，**跨主机跳转只认白名单**
//!   （HF → `*.hf.co` 的 CDN、GitHub → `*.githubusercontent.com`、ModelScope 自家域）。
//!   组织服务端的镜像不走它（镜像地址由运营方配置，见 `luna-cloud`）。
//! - [`pinned_tls`]：按服务端钉住的 TLS 根证书（ADR-153）——发往服务端的客户端都从这里取。
//! - [`hf`]：Hugging Face 仓库 id → 当前提交上的 GGUF 清单（高级模式「按 HF 仓库 id 添加」）。
//!
//! 工件身份是 `sha256 + 字节数`（ADR-035）：换来源续传是安全的，各源字节必然一致；
//! 不一致就是摘要失败，删掉重来。

#![forbid(unsafe_code)]

pub mod hf;
pub mod pinned_tls;

use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

/// 下载失败的分类（调用方各自映射成稳定错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 网络 / HTTP 失败（含跳转被白名单拦下、限流重试用尽）。
    Download {
        /// 明细（不含用户内容，只有地址与状态）。
        detail: String,
    },
    /// 本地写盘失败。
    Install {
        /// 明细。
        detail: String,
    },
    /// 字节数不符（`.part` 已删）。
    SizeMismatch {
        /// 期望。
        expected: u64,
        /// 实得。
        actual: u64,
    },
    /// sha256 不符（`.part` 已删）。
    ChecksumMismatch,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download { detail } => write!(f, "download failed: {detail}"),
            Self::Install { detail } => write!(f, "write failed: {detail}"),
            Self::SizeMismatch { expected, actual } => {
                write!(f, "size mismatch: expected {expected}, got {actual}")
            }
            Self::ChecksumMismatch => f.write_str("sha256 mismatch"),
        }
    }
}

/// 进度回调：参数 = 累计字节（续传时含起点）。节奏：每次尝试开始一次 + 每个 chunk 一次。
pub type Progress<'a> = dyn Fn(u64) + Send + Sync + 'a;

/// 跳转可以落到的主机（后缀匹配，必须 https）。只放目录来源实测会跳到的地方
/// （2026-10-07：HF `resolve` → `us.aws.cdn.hf.co`；GitHub release → `release-assets.githubusercontent.com`；
/// ModelScope 直接 200）。新来源要先实测它的跳转链再加。
pub const REDIRECT_ALLOWLIST: &[&str] = &[
    "huggingface.co",
    "hf.co",
    "modelscope.cn",
    "github.com",
    "githubusercontent.com",
];

/// `host` 是否落在白名单内（等于某后缀，或是它的子域）。
#[must_use]
pub fn host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    REDIRECT_ALLOWLIST
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// 目录来源专用客户端：跨主机跳转只认 [`REDIRECT_ALLOWLIST`]，最多 10 跳。
///
/// # Errors
/// TLS 后端初始化失败时。
pub fn catalog_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            let url = attempt.url();
            if url.scheme() == "https" && url.host_str().is_some_and(host_allowed) {
                attempt.follow()
            } else {
                let host = url.host_str().unwrap_or("").to_owned();
                attempt.error(format!("redirect to non-allowlisted host {host}"))
            }
        }))
        .build()
}

/// 单源内遇 429 / 503 的最多重试次数；之后换下一个来源。
const MAX_THROTTLE_RETRIES: u32 = 3;
/// 单次退避上限：服务端要我们等更久时不在这里干等，换源或报错让用户稍后再试。
const MAX_THROTTLE_WAIT: Duration = Duration::from_secs(60);

/// 按顺序从 `urls` 取工件到 `dest`（`.part` 暂存文件）。全部失败时返回最后一个来源的
/// 错误分类，明细里聚合每个来源的失败原因。
///
/// 失败语义（与此前直连实现一致）：大小 / 摘要不符删 `.part`；网络中断保留已下字节，
/// 下一个来源或下一次安装从断点续。
///
/// # Errors
/// 见 [`FetchError`]。
pub async fn download(
    client: &reqwest::Client,
    urls: &[String],
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    on_progress: &Progress<'_>,
) -> Result<(), FetchError> {
    if urls.is_empty() {
        return Err(FetchError::Download {
            detail: "no sources".to_owned(),
        });
    }
    let mut failures = Vec::new();
    let mut last = None;
    for (index, url) in urls.iter().enumerate() {
        match download_one(
            client,
            url,
            dest,
            expected_size,
            expected_sha256,
            on_progress,
        )
        .await
        {
            Ok(()) => return Ok(()),
            // 字节对不上不是来源的问题，是工件身份的问题：换源也只会再下一份同样的错。
            Err(
                err @ (FetchError::ChecksumMismatch
                | FetchError::SizeMismatch { .. }
                | FetchError::Install { .. }),
            ) => {
                return Err(err);
            }
            Err(err) => {
                failures.push(format!(
                    "source {}/{} ({}): {err}",
                    index + 1,
                    urls.len(),
                    redact(url)
                ));
                last = Some(err);
            }
        }
    }
    match last {
        Some(FetchError::Download { .. }) | None => Err(FetchError::Download {
            detail: failures.join("; "),
        }),
        Some(other) => Err(other),
    }
}

/// 只留主机与路径：预签名地址的查询串里是临时凭据，不进错误明细。
#[must_use]
pub fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// 单源：断点续传 + 流式摘要 + 终验；429 / 503 退避重试。
///
/// # Errors
/// 见 [`FetchError`]。
pub async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    on_progress: &Progress<'_>,
) -> Result<(), FetchError> {
    let mut throttled = 0;
    let response = loop {
        let existing = tokio::fs::metadata(dest)
            .await
            .map(|meta| meta.len().min(expected_size))
            .unwrap_or(0);
        let mut request = client.get(url);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|e| FetchError::Download { detail: chain(&e) })?;
        let status = response.status();
        if (status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
            && throttled < MAX_THROTTLE_RETRIES
        {
            let wait = throttle_wait(response.headers()).min(MAX_THROTTLE_WAIT);
            throttled += 1;
            tokio::time::sleep(wait).await;
            continue;
        }
        if !status.is_success() {
            return Err(FetchError::Download {
                detail: format!("HTTP {status}"),
            });
        }
        break (response, existing);
    };
    let (mut response, existing) = response;

    let resumed = existing > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let install = |e: std::io::Error| FetchError::Install {
        detail: e.to_string(),
    };
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    if resumed {
        // 续传：先把已有前缀喂给摘要，之后边写边算——全程不把文件整个读进内存。
        let mut prefix = tokio::fs::File::open(dest)
            .await
            .map_err(install)?
            .take(existing);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = prefix.read(&mut buf).await.map_err(install)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            downloaded += n as u64;
        }
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    // 续传不用 `append`：Windows 上追加模式的句柄没有 FILE_WRITE_DATA，下面的 `set_len`
    // 会被拒（「拒绝访问」，续传整个不可用，Windows 真机 2026-10-14）。改为截断 + 定位到末尾，
    // 两个平台语义相同。
    options.truncate(!resumed);
    let mut file = options.open(dest).await.map_err(install)?;
    if resumed {
        // 文件可能比 expected 长（上次写过头）：截到续传起点，与上面喂给摘要的前缀一致。
        file.set_len(existing).await.map_err(install)?;
        file.seek(std::io::SeekFrom::Start(existing))
            .await
            .map_err(install)?;
    }
    on_progress(downloaded);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| FetchError::Download { detail: chain(&e) })?
    {
        downloaded += chunk.len() as u64;
        if downloaded > expected_size {
            drop(file);
            let _ = tokio::fs::remove_file(dest).await;
            return Err(FetchError::SizeMismatch {
                expected: expected_size,
                actual: downloaded,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(install)?;
        on_progress(downloaded);
    }
    file.flush().await.map_err(install)?;
    drop(file);
    if downloaded != expected_size {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(FetchError::SizeMismatch {
            expected: expected_size,
            actual: downloaded,
        });
    }
    if format!("{:x}", hasher.finalize()) != expected_sha256 {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(FetchError::ChecksumMismatch);
    }
    Ok(())
}

/// reqwest 的顶层文本常常只是「跟随跳转出错」，真正的原因（我们自己的拦截理由、TLS、DNS）
/// 在 `source()` 链里——展开整条链，错误明细才说得出发生了什么。
fn chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

/// 限流后该等多久：`Retry-After`（秒）优先，其次 HF 的 `RateLimit: "…";r=0;t=<秒>`，都没有等 5 s。
fn throttle_wait(headers: &reqwest::header::HeaderMap) -> Duration {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(secs) = header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()) {
        return Duration::from_secs(secs);
    }
    if let Some(secs) = header("ratelimit").and_then(|v| {
        v.split(';')
            .find_map(|part| part.trim().strip_prefix("t="))
            .and_then(|t| t.trim().parse::<u64>().ok())
    }) {
        return Duration::from_secs(secs);
    }
    Duration::from_secs(5)
}

#[cfg(test)]
mod tests;
