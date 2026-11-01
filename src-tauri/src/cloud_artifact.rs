//! 制品面 `ArtifactFetcher` 适配器（ADR-037 后为本仓普通模块）。
//!
//! 分层裁决保持不变：`ArtifactFetcher` trait 定义在本 crate；luna-cloud 只提供纯 HTTP
//! 制品客户端（`luna_cloud::artifact`）——本模块把两者缝在一起，避免 luna-cloud 背上
//! 本 crate 的 whisper FFI 构建面（该分层与仓库归属无关，合并后依然成立）。
//!
//! **本适配器只在下载这个显式动作里被调用**（[`crate::cloud::CloudAwareArtifactFetcher`]）：
//! 已连接时问那台服务端，未连接时问内置的官方后端（ADR-145 ⑥ 修订 2026-10-07：模型存在
//! 后端，防上游链接失效）；严格本地下不调用。
//!
//! 行为（PRD-13-032）：先经制品面 resolve 镜像逐个尝试（沿用 trait 的进度与
//! sha256 契约），全败再回落条目直连 URL；错误信息如实分层（各源失败明细聚合）。
//! 错误码映射与 upstream `DirectArtifactFetcher` 完全同口径——上层安装编排
//! （`install_luna_managed_model`）对 fetcher 替换零感知。
//!
//! 严格本地：upstream 安装命令在调 fetcher **之前**已硬拦截（`ipc/transcribe.rs`）；
//! 这里再设防御性二闸——装配注入的探针说「严格本地」时拒绝联网，杜绝未来新增
//! 调用点绕过上游闸门的可能。

use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::artifact_fetcher::{ArtifactFetcher, ArtifactProgress, ArtifactRequest};
use luna_cloud::artifact::ArtifactClient;
use luna_cloud::StrictLocalProbe;

/// 「当前生效的云地址」共享格子。
///
/// 从 `OnceLock` 改成 `RwLock` 是热切的**必要条件**，也是最容易漏的一处：`OnceLock`
/// 只能填一次，漏改它的话编译完全通过、运行期静默沿用旧地址——用户改完地址，账户、
/// 目录、对话、转写全切过去了，**只有模型下载还打向旧服务器**，而这件事只在下载失败
/// 那一刻才现形。
pub type CloudAddressCell = Arc<RwLock<String>>;

/// 托管态包装：让「当前生效云地址」这个格子随 app 走，而不是做成进程级全局。
///
/// 进程级 `static` 在同进程内装配两次（测试）时会指向**第一个**格子，于是热切写进
/// 了一个没人读的地方——而生产只装配一次，这类缺陷在测试里也照样绿。挂在 app 上就没有
/// 这个歧义：谁的 app，谁的格子。
pub struct CloudAddress(pub CloudAddressCell);

/// 新建一个空格子（空 = 尚未解析，[`ArtifactPlaneFetcher::client`] 退回内置默认）。
#[must_use]
pub fn new_address_cell() -> CloudAddressCell {
    Arc::new(RwLock::new(String::new()))
}

/// 写入当前生效地址（装配期解析与设置热切共用同一入口）。
pub fn set_address(cell: &CloudAddressCell, api_base: &str) {
    if let Ok(mut guard) = cell.write() {
        *guard = api_base.to_owned();
    }
}

/// 制品面 fetcher（由 [`crate::cloud::CloudAwareArtifactFetcher`] 在非严格本地时委派）。
pub struct ArtifactPlaneFetcher {
    api_base: CloudAddressCell,
    probe: StrictLocalProbe,
}

impl ArtifactPlaneFetcher {
    /// **地址延迟解析且可热切**：制品面客户端在 `default_assembly()` 里构造，那时 app
    /// 还不存在、设备设置也没加载，直接取值只能拿到内置默认——于是配了组织服务端地址的
    /// 用户，账户与目录连对了服务端、制品下载却仍打向公有云。共享同一个格子，由插件
    /// setup 填入、由设置变更热更新，两条路径不可能分叉。
    pub fn new(api_base: CloudAddressCell, probe: StrictLocalProbe) -> Self {
        Self { api_base, probe }
    }

    /// 解析后的客户端。未填充时退回内置默认（不 panic：制品下载不该因为装配顺序而崩）。
    fn client(&self) -> ArtifactClient {
        let base = self
            .api_base
            .read()
            .ok()
            .map(|value| value.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| luna_cloud::config::DEFAULT_API_BASE.to_owned());
        ArtifactClient::new(base)
    }
}

#[async_trait::async_trait]
impl ArtifactFetcher for ArtifactPlaneFetcher {
    async fn fetch(
        &self,
        request: ArtifactRequest<'_>,
        on_progress: &ArtifactProgress<'_>,
    ) -> Result<(), String> {
        if (self.probe)() {
            // 防御性二闸（上游命令层已拦截；此处兜住未来新增调用点）。
            return Err("MODEL_DOWNLOAD_FAILED: 严格本地模式已拦截联网下载".to_owned());
        }
        let dest: &Path = request.dest;
        self.client()
            .fetch(
                request.urls,
                dest,
                request.expected_size_bytes,
                request.expected_sha256,
                &|downloaded| on_progress(downloaded),
            )
            .await
            .map_err(crate::artifact_fetcher::fetch_error_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 严格本地探针为真时：零网络、直接拒绝，且错误码沿 upstream 稳定前缀。
    #[tokio::test]
    async fn strict_local_probe_blocks_fetch() {
        // 不可达端口：若真发请求会得到网络错误而非拦截文案。
        let base = new_address_cell();
        set_address(&base, "http://127.0.0.1:9");
        let fetcher = ArtifactPlaneFetcher::new(base, Arc::new(|| true));
        let dest = std::env::temp_dir().join("luna-official-fetcher-strict-test.part");
        let request = ArtifactRequest {
            urls: &["http://127.0.0.1:9/artifact.bin".to_owned()],
            dest: &dest,
            expected_size_bytes: 4,
            expected_sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };
        let err = fetcher
            .fetch(request, &|_| {})
            .await
            .expect_err("strict local must block");
        assert!(
            err.starts_with("MODEL_DOWNLOAD_FAILED"),
            "stable error code prefix, got: {err}"
        );
        assert!(
            err.contains("严格本地"),
            "must state the strict-local cause"
        );
    }
}
