//! 制品面 HTTP 客户端（TDD-14 §3.1 制品面 / PRD-13-032）。
//!
//! **纯 HTTP 层**：本模块不实现 upstream 的 `ArtifactFetcher` trait——那会把
//! luna-cloud 拽进 luna-app 的依赖图（whisper FFI 构建）。分层裁决：trait 适配器
//! 在 app-official（见其 `cloud_fetcher` 模块），这里只提供可独立测试的两块能力：
//! 1. [`ArtifactClient::resolve_mirrors`]：digest → 有时效镜像 URL 列表；
//! 2. [`ArtifactClient::fetch`]：镜像逐个尝试 → 全败回落条目自带的目录来源 → 断点续传 +
//!    流式 sha256 终验（单源下载就是 `luna-model-fetch` 那一份，与直连实现同一段代码）。
//!
//! 错误分层如实：每个下载源的失败单独留痕，聚合进最终错误 `detail`
//! （URL 是公开模型地址，无 PII；绝不含本地路径之外的用户数据）。

use std::path::Path;

use serde::Deserialize;

/// 下载失败分类：与目录直连下载同一个类型（`luna-model-fetch`），适配器据此映射 upstream
/// 稳定错误码（`MODEL_DOWNLOAD_FAILED` / `MODEL_INSTALL_FAILED` / `MODEL_CHECKSUM_FAILED`）。
pub use luna_model_fetch::FetchError as ArtifactError;

#[derive(Deserialize)]
struct MirrorsResponse {
    mirrors: Vec<MirrorEntry>,
}

#[derive(Deserialize)]
struct MirrorEntry {
    url: String,
    // expires_at 契约字段（M3 dev 占位时效）：客户端在单次安装内即用即弃，
    // 无跨请求缓存，无需消费具体时刻。
}

/// 制品面客户端。
pub struct ArtifactClient {
    http: luna_model_fetch::pinned_tls::PinnedClient,
    api_base: String,
}

impl ArtifactClient {
    pub fn new(api_base: String) -> Self {
        Self {
            http: luna_model_fetch::pinned_tls::PinnedClient::default(),
            api_base,
        }
    }

    /// digest → 镜像 URL 列表（按服务端下发顺序）。
    ///
    /// # Errors
    /// 网络失败 / 非 2xx（含 404 unknown digest）返回原因描述——调用侧
    /// （fetch 编排）把 resolve 失败降级为「跳过制品面，直接回落直连」。
    pub async fn resolve_mirrors(&self, sha256: &str) -> Result<Vec<String>, String> {
        let response = self
            .http
            .for_url(&self.api_base)
            .get(format!("{}/v1/artifacts/{sha256}/mirrors", self.api_base))
            .send()
            .await
            .map_err(|e| format!("mirror resolve: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("mirror resolve: HTTP {}", response.status()));
        }
        let body: MirrorsResponse = response
            .json()
            .await
            .map_err(|e| format!("mirror resolve: {e}"))?;
        Ok(body.mirrors.into_iter().map(|m| m.url).collect())
    }

    /// 制品面编排：resolve 镜像 → 逐个尝试 → 全败回落 `direct_url` → 终验。
    ///
    /// 断点语义跨源成立：工件以 digest 为身份（各源字节必然一致），失败源留下的
    /// `.part` 字节可被下一个源续传；大小/校验和不符时删除 `.part`（与直连实现一致）。
    ///
    /// # Errors
    /// 全部源失败时返回**最后一次尝试**的错误分类，`detail` 聚合各源失败明细。
    pub async fn fetch(
        &self,
        direct_urls: &[String],
        dest: &Path,
        expected_size_bytes: u64,
        expected_sha256: &str,
        on_progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ArtifactError> {
        // resolve 失败不终结安装：如实降级为直连（错误明细并入最终报告）。
        let (mirrors, resolve_note) = match self.resolve_mirrors(expected_sha256).await {
            Ok(mirrors) => (mirrors, None),
            Err(reason) => (Vec::new(), Some(reason)),
        };

        let mut failures: Vec<String> = resolve_note
            .map(|reason| vec![format!("制品面 resolve 失败（降级直连）：{reason}")])
            .unwrap_or_default();
        let mut last_error: Option<ArtifactError> = None;

        let total_mirrors = mirrors.len();
        for (index, url) in mirrors.iter().enumerate() {
            match self
                .download_to_file(url, dest, expected_size_bytes, expected_sha256, on_progress)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    failures.push(format!(
                        "镜像 {}/{total_mirrors}（{}）：{}",
                        index + 1,
                        luna_model_fetch::redact(url),
                        describe(&err)
                    ));
                    last_error = Some(err);
                }
            }
        }

        // 镜像全败（或无镜像）→ 回落条目自带的目录来源；若某来源恰与唯一镜像同址且刚失败，
        // 仍如实再试一次——失败可能是瞬时的，且行为可预测比省一次请求更重要。
        // 目录来源走专用客户端（跨主机跳转白名单，ADR-144 ①）；镜像不走——镜像地址由运营方配置。
        let catalog = luna_model_fetch::catalog_client().map_err(|e| ArtifactError::Download {
            detail: e.to_string(),
        })?;
        match luna_model_fetch::download(
            &catalog,
            direct_urls,
            dest,
            expected_size_bytes,
            expected_sha256,
            on_progress,
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                failures.push(format!("目录来源：{}", describe(&err)));
                let _ = last_error; // 最终分类取目录来源（最后一次尝试）的错误。
                Err(attach_detail(err, &failures.join("；")))
            }
        }
    }

    /// 单源下载：断点续传（Range）+ 大小判定 + sha256 终验。
    /// 语义逐条对齐 upstream `DirectArtifactFetcher`：
    /// - 既有 `.part` 按 `min(len, expected)` 续传，服务端 206 才算续上（否则重下）；
    /// - 进度回调 = 开始一次 + 每 chunk 一次，值为累计字节（含续传起点）；
    /// - 大小不符 / sha256 不符 → 删 `.part` 后报错。
    ///
    /// # Errors
    /// 见 [`ArtifactError`] 分类。
    pub async fn download_to_file(
        &self,
        url: &str,
        dest: &Path,
        expected_size_bytes: u64,
        expected_sha256: &str,
        on_progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ArtifactError> {
        luna_model_fetch::download_one(
            &self.http.for_url(url),
            url,
            dest,
            expected_size_bytes,
            expected_sha256,
            on_progress,
        )
        .await
    }
}

/// 单源失败的简述（聚合进 detail）。
fn describe(err: &ArtifactError) -> String {
    match err {
        ArtifactError::Download { detail } => format!("下载失败 {detail}"),
        ArtifactError::Install { detail } => format!("写盘失败 {detail}"),
        ArtifactError::SizeMismatch { expected, actual } => {
            format!("大小不符 期望 {expected} 实际 {actual}")
        }
        ArtifactError::ChecksumMismatch => "sha256 不符".to_owned(),
    }
}

/// 把聚合明细写回最终错误的 detail（分类保持最后一次尝试的分类）。
fn attach_detail(err: ArtifactError, aggregated: &str) -> ArtifactError {
    match err {
        ArtifactError::Download { .. } => ArtifactError::Download {
            detail: aggregated.to_owned(),
        },
        ArtifactError::Install { .. } => ArtifactError::Install {
            detail: aggregated.to_owned(),
        },
        // 大小/校验和错误保留结构化字段（upstream 错误文案依赖它们）。
        other => other,
    }
}
