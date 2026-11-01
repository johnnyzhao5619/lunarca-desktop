//! 应用组件（ADR-167 ⑤ / ADR-169 ⑧）：一件第三方程序在不在、装在哪、怎么装 / 更新 / 卸载，全在
//! 它自己的那一份实现里（[`Component`]）。「应用」页的组件状态、安装、更新与卸载只经
//! [`component`] 拿到实现再调它，不按组件 id 分支——**新组件 = 写一份实现 + 登记进 [`COMPONENTS`]**。
//!
//! 实际的下载、校验与落位仍在 `fetch_tool`（与 `luna_fetch` 的发布物契约），这里只把每一件的
//! 那几步收拢成同一个接口。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use luna_fetch::install::PlannedComponent;
use luna_fetch::tool::ExternalTool;

/// 装 / 更新一件组件的异步结果（成功时给出装上的版本）。
pub(crate) type InstallFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// 本机找到的一份组件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    pub path: PathBuf,
    /// 上游项目主页（JS 运行时按在用的那一种写）。
    pub source: &'static str,
}

/// 一件组件。默认实现是「只认本机已装的、不能代装」那一类（如 ffmpeg）。
pub(crate) trait Component: Sync {
    /// 组件 id（清单里写的那个，也是 [`ExternalTool`] 的 id）。
    fn tool(&self) -> ExternalTool;

    /// 在本机找它：用户设置里指定的路径、Lunarca 下载的那份（`managed`）、系统里装的，按各自的
    /// 优先级。只看文件，不跑程序、不联网。
    fn locate(&self, settings: &luna_config::Settings, managed: Option<&Path>) -> Option<Located>;

    /// Lunarca 下载的那份在磁盘上的落点；不能代装的组件没有。
    fn managed_file(&self, _models_root: &Path) -> Option<PathBuf> {
        None
    }

    /// 这个平台上能不能代装（上游有没有对应的发布物）。
    fn installable(&self) -> bool {
        false
    }

    /// Lunarca 下载的那份的版本（展示用）。
    fn installed_version(&self, _models_root: &Path) -> Option<String> {
        None
    }

    /// 下载进度事件里的 id。
    fn progress_id(&self) -> Option<&'static str> {
        None
    }

    /// 安装后大约占多少磁盘（安装计划的约数）。
    fn typical_disk_bytes(&self) -> u64 {
        0
    }

    /// 下载大约多大（安装计划的约数）。
    fn typical_download_bytes(&self) -> u64 {
        0
    }

    /// 代装时从哪下载（上游仓库地址，不带协议；安装确认框写它）。
    fn download_source(&self) -> &'static str {
        self.tool().source()
    }

    /// 上一次「检查更新」说它有新版没有（只对 Lunarca 下载的那份有意义）。
    fn update_known(&self) -> bool {
        false
    }

    /// 下载、校验并落位（更新也走这条：装成最新版）。
    fn install<'a>(
        &'a self,
        _app: &'a tauri::AppHandle,
        _models_root: &'a Path,
    ) -> InstallFuture<'a> {
        let id = self.tool().as_str();
        Box::pin(async move { Err(format!("COMPONENT_NOT_INSTALLABLE: {id}")) })
    }

    /// 删掉 Lunarca 下载的那份（用户自己装的一律不动；本来就没装也算成功）。
    ///
    /// # Errors
    /// 删除失败（权限等）时返回稳定错误码。
    fn uninstall(&self, _models_root: &Path) -> Result<(), String> {
        Ok(())
    }
}

/// yt-dlp（「从链接导入」的必需组件）。
struct YtDlp;

impl Component for YtDlp {
    fn tool(&self) -> ExternalTool {
        ExternalTool::YtDlp
    }

    fn locate(&self, settings: &luna_config::Settings, managed: Option<&Path>) -> Option<Located> {
        let explicit = settings
            .get_str("transcribe.fetch_tool_path")
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        luna_fetch::tool::locate_yt_dlp(explicit.as_deref(), managed).map(|path| Located {
            path,
            source: ExternalTool::YtDlp.source(),
        })
    }

    fn managed_file(&self, models_root: &Path) -> Option<PathBuf> {
        Some(crate::fetch_tool::managed_path(models_root))
    }

    fn installable(&self) -> bool {
        !luna_fetch::install::platform_asset().is_empty()
    }

    fn installed_version(&self, models_root: &Path) -> Option<String> {
        crate::fetch_tool::installed_version(models_root)
    }

    fn progress_id(&self) -> Option<&'static str> {
        Some(crate::fetch_tool::TOOL_ID)
    }

    fn typical_disk_bytes(&self) -> u64 {
        PlannedComponent::YtDlp.typical_disk_bytes()
    }

    fn typical_download_bytes(&self) -> u64 {
        PlannedComponent::YtDlp.typical_download_bytes()
    }

    fn download_source(&self) -> &'static str {
        PlannedComponent::YtDlp.source()
    }

    fn update_known(&self) -> bool {
        crate::fetch_tool::known_update(PlannedComponent::YtDlp)
    }

    fn install<'a>(
        &'a self,
        app: &'a tauri::AppHandle,
        models_root: &'a Path,
    ) -> InstallFuture<'a> {
        Box::pin(crate::fetch_tool::install_managed(app, models_root))
    }

    fn uninstall(&self, models_root: &Path) -> Result<(), String> {
        crate::fetch_tool::uninstall_managed(models_root)
    }
}

/// JS 运行时（「从链接导入」的推荐组件：YouTube 需要；Lunarca 代装的是 Deno，本机的 Node / Bun 也认）。
struct JsRuntime;

impl Component for JsRuntime {
    fn tool(&self) -> ExternalTool {
        ExternalTool::JsRuntime
    }

    fn locate(&self, _settings: &luna_config::Settings, managed: Option<&Path>) -> Option<Located> {
        luna_fetch::tool::locate_js_runtime(managed).map(|runtime| Located {
            source: luna_fetch::tool::js_runtime_source(&runtime.name),
            path: runtime.path,
        })
    }

    fn managed_file(&self, models_root: &Path) -> Option<PathBuf> {
        Some(crate::fetch_tool::managed_js_runtime_path(models_root))
    }

    fn installable(&self) -> bool {
        !luna_fetch::install::js_runtime_platform_asset().is_empty()
    }

    fn installed_version(&self, models_root: &Path) -> Option<String> {
        crate::fetch_tool::installed_js_runtime_version(models_root)
    }

    fn progress_id(&self) -> Option<&'static str> {
        Some(crate::fetch_tool::JS_RUNTIME_TOOL_ID)
    }

    fn typical_disk_bytes(&self) -> u64 {
        PlannedComponent::JsRuntime.typical_disk_bytes()
    }

    fn typical_download_bytes(&self) -> u64 {
        PlannedComponent::JsRuntime.typical_download_bytes()
    }

    fn download_source(&self) -> &'static str {
        PlannedComponent::JsRuntime.source()
    }

    fn update_known(&self) -> bool {
        crate::fetch_tool::known_update(PlannedComponent::JsRuntime)
    }

    fn install<'a>(
        &'a self,
        app: &'a tauri::AppHandle,
        models_root: &'a Path,
    ) -> InstallFuture<'a> {
        Box::pin(crate::fetch_tool::install_managed_js_runtime(
            app,
            models_root,
        ))
    }

    fn uninstall(&self, models_root: &Path) -> Result<(), String> {
        crate::fetch_tool::uninstall_managed_js_runtime(models_root)
    }
}

/// ffmpeg（「从链接导入」的可选组件：高清合并）。只认本机已装的——不代装（体积与许可都不合适）。
struct Ffmpeg;

impl Component for Ffmpeg {
    fn tool(&self) -> ExternalTool {
        ExternalTool::Ffmpeg
    }

    fn locate(
        &self,
        _settings: &luna_config::Settings,
        _managed: Option<&Path>,
    ) -> Option<Located> {
        luna_fetch::tool::locate_ffmpeg().map(|path| Located {
            path,
            source: ExternalTool::Ffmpeg.source(),
        })
    }
}

/// 全部组件的实现（与 [`ExternalTool::ALL`] 一一对应，由测试钉住）。
pub(crate) static COMPONENTS: &[&dyn Component] = &[&YtDlp, &Ffmpeg, &JsRuntime];

/// 按 id 找一件组件的实现。
#[must_use]
pub(crate) fn component(id: &str) -> Option<&'static dyn Component> {
    COMPONENTS
        .iter()
        .copied()
        .find(|component| component.tool().as_str() == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_external_tool_has_exactly_one_implementation() {
        let ids: Vec<&str> = COMPONENTS.iter().map(|c| c.tool().as_str()).collect();
        let tools: Vec<&str> = ExternalTool::ALL.iter().map(|tool| tool.as_str()).collect();
        assert_eq!(ids, tools);
        assert!(component("nope").is_none());
    }

    #[test]
    fn installable_components_know_where_their_managed_copy_lives() {
        let root = Path::new("/models");
        for component in COMPONENTS {
            if component.installable() {
                assert!(
                    component.managed_file(root).is_some(),
                    "{}",
                    component.tool().as_str()
                );
                assert!(
                    component.progress_id().is_some(),
                    "{}",
                    component.tool().as_str()
                );
                assert!(
                    component.typical_disk_bytes() > 0,
                    "{}",
                    component.tool().as_str()
                );
            }
        }
        assert!(
            component("ffmpeg").is_some_and(|c| !c.installable() && c.managed_file(root).is_none())
        );
    }
}
