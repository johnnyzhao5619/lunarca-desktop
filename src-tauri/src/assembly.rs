//! 官方构建 Rust 装配接缝（TDD-14 §3.2 ①）。
//!
//! [`crate::run`] = **空默认装配**的薄壳（OSS 构建，行为零变化）；官方构建（商业仓
//! app-official）构造非空 [`AppAssembly`] 后调 [`crate::run_with`] 注入额外插件与
//! setup 钩子。装配是**构建期组合**，不加载任何运行时远程代码。
//!
//! 本批承载 TDD-14 §3.2 ① 中的插件装配与 (c) setup 钩子两个扩展点，以及 §3.2 ②
//! 的 [`ArtifactFetcher`] 注入点（真实消费者 = 商业仓 M3 制品面客户端）；
//! (a) Provider 注册与 (b) IPC 命令组注册按规格随首个商业纵切（云 Provider 到场）
//! 同批引入——公开仓不先行留占位（TDD-14 §3.2 / PRD-13-010）。

use std::sync::Arc;

use crate::artifact_fetcher::ArtifactFetcher;

/// Tauri 插件装配步骤：在内建插件之后逐个应用到 `tauri::Builder`。
type PluginStep = Box<dyn FnOnce(tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> + Send>;

/// 额外 setup 钩子：在既有 setup 闭包**末尾**逐个执行；错误沿既有 setup 错误路径传播
/// （返回 `Err` 即装配失败）。
type SetupHook =
    Box<dyn FnOnce(&tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> + Send>;

/// 应用装配（官方构建装配接缝，TDD-14 §3.2 ①）。
///
/// `Default` = OSS 默认装配：各扩展点均为空/未注入，[`crate::run_with`] 的行为与历史
/// 单体 `run()` 完全一致（硬验收：OSS 构建行为零变化）。
#[derive(Default)]
pub struct AppAssembly {
    /// 额外 Tauri 插件（内建插件之后逐个应用；OSS 默认为空）。
    pub(crate) plugins: Vec<PluginStep>,
    /// 额外 setup 钩子（既有 setup 末尾逐个执行；OSS 默认为空）。
    pub(crate) extra_setup: Vec<SetupHook>,
    /// 工件下载实现注入点（TDD-14 §3.2 ②）：`None`（OSS 默认）= 维持
    /// [`crate::artifact_fetcher::DirectArtifactFetcher`]（行为零变化）；官方构建在此
    /// 注入制品面客户端实现（digest → 镜像集 → 断点续传 → 本地 sha256 终验）。
    pub(crate) artifact_fetcher: Option<Arc<dyn ArtifactFetcher>>,
}

impl AppAssembly {
    /// 追加一个插件装配步骤（链式；应用顺序 = 追加顺序）。
    #[must_use]
    pub fn with_plugin(
        mut self,
        plugin: impl FnOnce(tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> + Send + 'static,
    ) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// 追加一个 setup 钩子（链式；执行顺序 = 追加顺序）。
    #[must_use]
    pub fn with_setup(
        mut self,
        hook: impl FnOnce(&tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> + Send + 'static,
    ) -> Self {
        self.extra_setup.push(Box::new(hook));
        self
    }

    /// 注入工件下载实现（链式；TDD-14 §3.2 ②）。不调用（OSS 默认 `None`）时装配期
    /// 落到 [`crate::artifact_fetcher::DirectArtifactFetcher`]——行为零变化；后调覆盖先调。
    #[must_use]
    pub fn with_artifact_fetcher(mut self, fetcher: Arc<dyn ArtifactFetcher>) -> Self {
        self.artifact_fetcher = Some(fetcher);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OSS 默认装配恒空——`run_with(ctx, AppAssembly::default())` 不引入任何额外装配步骤
    /// （TDD-14 §3.2 硬验收：OSS 构建行为零变化）。
    #[test]
    fn default_assembly_is_empty() {
        let assembly = AppAssembly::default();
        assert!(assembly.plugins.is_empty());
        assert!(assembly.extra_setup.is_empty());
        assert!(
            assembly.artifact_fetcher.is_none(),
            "OSS 默认必须不带注入 fetcher（None = DirectArtifactFetcher，行为零变化）"
        );
    }

    /// 链式方法按追加顺序积累装配步骤（官方构建装配的最小契约）。
    #[test]
    fn chained_registration_accumulates_steps() {
        let assembly = AppAssembly::default()
            .with_plugin(|builder| builder)
            .with_setup(|_app| Ok(()))
            .with_setup(|_app| Ok(()));
        assert_eq!(assembly.plugins.len(), 1);
        assert_eq!(assembly.extra_setup.len(), 2);
    }

    /// 注入自定义 fetcher 后，装配结构确实携带**同一个**实例（TDD-14 §3.2 ②；
    /// 只测结构，不起 tauri App——贯通到 AppState 的落点由 providers 装配点保证）。
    #[test]
    fn with_artifact_fetcher_carries_injected_instance() {
        struct StubFetcher;
        #[async_trait::async_trait]
        impl ArtifactFetcher for StubFetcher {
            async fn fetch(
                &self,
                _request: crate::artifact_fetcher::ArtifactRequest<'_>,
                _on_progress: &crate::artifact_fetcher::ArtifactProgress<'_>,
            ) -> Result<(), String> {
                Ok(())
            }
        }

        let fetcher: Arc<dyn ArtifactFetcher> = Arc::new(StubFetcher);
        let assembly = AppAssembly::default().with_artifact_fetcher(Arc::clone(&fetcher));
        let carried = assembly
            .artifact_fetcher
            .expect("注入后装配结构必须携带 fetcher");
        assert!(
            Arc::ptr_eq(&carried, &fetcher),
            "装配携带的必须是注入的同一个实例，而非默认实现"
        );
    }
}
