//! 一方应用（ADR-165 / ADR-168）：「应用」目的地的列表投影，与安装、卸载、启用 / 暂时停用。
//!
//! 清单与判定在 `luna_config::apps`（唯一事实源）；这里做两件 IO——查组件在不在、下载与删除
//! Lunarca 下载的那几份组件——再把结果交给那边的纯函数算状态与主按钮，投影成 typed IPC。
//! 安装状态落 `apps.installed`、启用状态落 `apps.enabled`，都走与设置页同一条
//! `set_setting_value` 通路（校验 + 落盘 + 广播）；三条写入命令最后一律以 `apps.enabled` 广播
//! 一次「应用变了」，活动栏、设置页、编辑器只订阅这一个键。
//!
//! **卸载与停用都不碰库**（ADR-165 ④）：这里没有一行会读写库里的文件；卸载删的只是
//! Lunarca 自己下载到应用数据目录里的组件，用户自己装的程序一律不动（ADR-168 ④）。

use super::settings::{apply_setting_side_effects, set_setting_value, SettingValueDto};
use super::*;
use luna_config::apps::{
    install_plan, is_enabled, is_installed, lifecycle, limited_by, manifest, primary_action,
    with_enabled, with_installed, AppAction, AppContribution, AppLifecycle, AppManifest,
    AppPermission, ComponentFact, ComponentNeed, ComponentPresence, APPS_ENABLED_KEY,
    APPS_INSTALLED_KEY, FIRST_PARTY_APPS, FIRST_PARTY_DEVELOPER, FIRST_PARTY_LICENSE,
};
use luna_fetch::tool::ExternalTool;

/// 一方应用的权限（详情页「权限与数据」）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppPermissionDto {
    VaultFolder,
    Microphone,
    Network,
    Ai,
    ExternalPrograms,
}

impl From<AppPermission> for AppPermissionDto {
    fn from(value: AppPermission) -> Self {
        match value {
            AppPermission::VaultFolder => Self::VaultFolder,
            AppPermission::Microphone => Self::Microphone,
            AppPermission::Network => Self::Network,
            AppPermission::Ai => Self::Ai,
            AppPermission::ExternalPrograms => Self::ExternalPrograms,
        }
    }
}

/// 一方应用贡献的入口种类（详情页「启用后会多出」）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppContributionDto {
    Space,
    EditorActions,
    Templates,
    Windows,
    Settings,
    Prompts,
    LinkImport,
}

impl From<AppContribution> for AppContributionDto {
    fn from(value: AppContribution) -> Self {
        match value {
            AppContribution::Space => Self::Space,
            AppContribution::EditorActions => Self::EditorActions,
            AppContribution::Templates => Self::Templates,
            AppContribution::Windows => Self::Windows,
            AppContribution::Settings => Self::Settings,
            AppContribution::Prompts => Self::Prompts,
            AppContribution::LinkImport => Self::LinkImport,
        }
    }
}

/// 组件对应用有多要紧（详情页「组件」一节的标签，ADR-167 ⑤）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentNeedDto {
    Required,
    Recommended,
    Optional,
}

impl From<ComponentNeed> for ComponentNeedDto {
    fn from(value: ComponentNeed) -> Self {
        match value {
            ComponentNeed::Required => Self::Required,
            ComponentNeed::Recommended => Self::Recommended,
            ComponentNeed::Optional => Self::Optional,
        }
    }
}

/// 一件组件在这台机器上的情况（ADR-168 ①）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentPresenceDto {
    Missing,
    /// Lunarca 下载的那一份。
    Managed,
    /// 用户自己装的（Homebrew 等），Lunarca 不动它。
    External,
}

impl From<ComponentPresence> for ComponentPresenceDto {
    fn from(value: ComponentPresence) -> Self {
        match value {
            ComponentPresence::Missing => Self::Missing,
            ComponentPresence::Managed => Self::Managed,
            ComponentPresence::External => Self::External,
        }
    }
}

/// 应用的一个组件（一件第三方程序）。默认视图不列它们，只在「开源组件与许可证」二级页出现
/// （ADR-168 ⑤）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppComponentDto {
    /// 组件 id（`yt_dlp` / `js_runtime` / `ffmpeg`，与外部工具同一个词表）。
    pub id: String,
    pub need: ComponentNeedDto,
    pub presence: ComponentPresenceDto,
    /// 上游项目的许可证（SPDX）。
    pub license: String,
    /// 上游项目主页（不带协议）。
    pub source: String,
    /// 在用那一份的版本：只有 Lunarca 下载的那份知道（记在旁文件里）；用户自己装的不去跑它
    /// 探版本（yt-dlp 一次要 8 秒），为 `None`。
    pub version: Option<String>,
    /// 在用那一份的路径（没找到为 `None`）。
    pub path: Option<String>,
    /// 上一次「检查更新」说 Lunarca 下载的这一份有新版（ADR-168 ⑧）。
    pub update_available: bool,
    /// 下载这件时 `model://download-progress` 里的 id（能代装的才有；更新时按钮上的进度据此认领）。
    pub progress_id: Option<String>,
}

/// 用户看到的应用状态（ADR-168 ①，`luna_config::apps::lifecycle` 的投影）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppLifecycleDto {
    NotInstalled,
    Installed,
    Paused,
    NeedsRepair,
    UpdateAvailable,
}

impl From<AppLifecycle> for AppLifecycleDto {
    fn from(value: AppLifecycle) -> Self {
        match value {
            AppLifecycle::NotInstalled => Self::NotInstalled,
            AppLifecycle::Installed => Self::Installed,
            AppLifecycle::Paused => Self::Paused,
            AppLifecycle::NeedsRepair => Self::NeedsRepair,
            AppLifecycle::UpdateAvailable => Self::UpdateAvailable,
        }
    }
}

/// 与状态对应的主按钮（ADR-168 ①）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppActionDto {
    Install,
    Open,
    Enable,
    Repair,
    Update,
    LeaveStrictLocal,
}

impl From<AppAction> for AppActionDto {
    fn from(value: AppAction) -> Self {
        match value {
            AppAction::Install => Self::Install,
            AppAction::Open => Self::Open,
            AppAction::Enable => Self::Enable,
            AppAction::Repair => Self::Repair,
            AppAction::Update => Self::Update,
            AppAction::LeaveStrictLocal => Self::LeaveStrictLocal,
        }
    }
}

/// 「应用」页的一行 / 详情页的数据。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppDto {
    /// 稳定 id（`apps://<id>`、文案键 `apps:<id>.*`）。
    pub id: String,
    /// 此刻是否生效（已安装 ∧ 已启用）：入口露不露出来只看它。
    pub enabled: bool,
    /// 装了没有（含暂时停用）。
    pub installed: bool,
    pub state: AppLifecycleDto,
    /// 主按钮。
    pub action: AppActionDto,
    /// 已经能用、但因推荐组件不在而受限：那几件组件的 id。
    pub limited_by: Vec<String>,
    /// 版本（一方应用 = 客户端版本）。
    pub version: String,
    pub developer: String,
    /// 许可证（SPDX）。
    pub license: String,
    /// 占用空间（字节）：已安装时是 Lunarca 下载的组件实占，未安装时是安装将占用的约数；
    /// 没有要下载的东西时为 `None`（应用本身随客户端发布，不另占空间）。
    pub size_bytes: Option<f64>,
    /// 它拥有的活动栏空间（启用后出现在活动栏上）。
    pub space: Option<WorkspaceSpaceDto>,
    /// 它拥有的 `luna.type`：这些笔记的专属入口随启用状态出现与收回。
    pub note_types: Vec<String>,
    /// 声明的权限（展示顺序）。
    pub permissions: Vec<AppPermissionDto>,
    /// 贡献的入口种类（展示顺序）。
    pub contributions: Vec<AppContributionDto>,
    /// 需要下载的第三方组件（展示顺序；没有就是空）。
    pub components: Vec<AppComponentDto>,
    /// 详情页的亮点 id（文案键 `apps.<id>.highlight.<亮点>.title|body`）。
    pub highlights: Vec<String>,
}

/// 查过一遍的组件：判定用的事实，加上展示用的路径与版本。
struct ProbedComponent {
    fact: ComponentFact,
    tool: ExternalTool,
    path: Option<PathBuf>,
    version: Option<String>,
    /// 上游项目主页：JS 运行时按在用的那一种（Node / Bun / Deno）写。
    source: &'static str,
    /// 下载进度事件里的 id（不能代装的组件没有）。
    progress_id: Option<&'static str>,
}

/// 只要判定用的事实（安装、修复与卸载的路径）。
fn component_facts(
    manifest: &AppManifest,
    settings: &luna_config::Settings,
    models_root: &Path,
) -> Vec<ComponentFact> {
    probe_components(manifest, settings, models_root)
        .into_iter()
        .map(|probed| probed.fact)
        .collect()
}

/// 查这个应用的组件此刻在不在（只看本机文件，不跑程序、不联网；毫秒级）。每一件怎么找、装在哪都
/// 由它自己的实现回答（`crate::components`，ADR-169 ⑧）。
fn probe_components(
    manifest: &AppManifest,
    settings: &luna_config::Settings,
    models_root: &Path,
) -> Vec<ProbedComponent> {
    manifest
        .components
        .iter()
        .filter_map(|declared| {
            let component = crate::components::component(declared.id)?;
            let managed = component.managed_file(models_root);
            let found = component.locate(settings, managed.as_deref());
            let presence = match &found {
                None => ComponentPresence::Missing,
                Some(located) if Some(&located.path) == managed.as_ref() => {
                    ComponentPresence::Managed
                }
                Some(_) => ComponentPresence::External,
            };
            let version = (presence == ComponentPresence::Managed)
                .then(|| component.installed_version(models_root))
                .flatten();
            Some(ProbedComponent {
                fact: ComponentFact {
                    id: declared.id,
                    need: declared.need,
                    presence,
                    installable: component.installable(),
                    // 只有用户点过「检查更新」才知道（ADR-168 ⑧）；只对 Lunarca 下载的那份有意义。
                    update_available: presence == ComponentPresence::Managed
                        && component.update_known(),
                },
                tool: component.tool(),
                source: found
                    .as_ref()
                    .map_or(component.tool().source(), |located| located.source),
                path: found.map(|located| located.path),
                version,
                progress_id: component.progress_id(),
            })
        })
        .collect()
}

/// 占用空间：已安装时是 Lunarca 下载的组件实占，否则是安装计划的约数（见 `AppDto::size_bytes`）。
fn size_bytes(installed: bool, facts: &[ComponentFact], models_root: &Path) -> Option<f64> {
    let bytes: u64 = if installed {
        facts
            .iter()
            .filter(|fact| fact.presence == ComponentPresence::Managed)
            .filter_map(|fact| crate::components::component(fact.id)?.managed_file(models_root))
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum()
    } else {
        install_plan(facts)
            .into_iter()
            .filter_map(crate::components::component)
            .map(|component| component.typical_disk_bytes())
            .sum()
    };
    (bytes > 0).then_some(bytes as f64)
}

/// 一个应用的投影（IO 已经做完：组件查验结果与占用空间由调用方交进来）。
fn app_dto(
    manifest: &AppManifest,
    settings: &luna_config::Settings,
    probed: &[ProbedComponent],
    size_bytes: Option<f64>,
) -> AppDto {
    let facts: Vec<ComponentFact> = probed.iter().map(|p| p.fact).collect();
    let facts = facts.as_slice();
    let installed = is_installed(settings, manifest.id);
    // 已安装时「生效」就是启用那一位；未安装时生命周期先看安装，那一位不起作用。
    let state = lifecycle(installed, is_enabled(settings, manifest.id), facts);
    AppDto {
        id: manifest.id.to_owned(),
        enabled: is_enabled(settings, manifest.id),
        installed,
        state: state.into(),
        action: primary_action(state, facts, settings.strict_local()).into(),
        limited_by: limited_by(facts).into_iter().map(str::to_owned).collect(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        developer: FIRST_PARTY_DEVELOPER.to_owned(),
        license: FIRST_PARTY_LICENSE.to_owned(),
        size_bytes,
        space: manifest.space.and_then(WorkspaceSpaceDto::from_str),
        note_types: manifest
            .note_types
            .iter()
            .map(|t| (*t).to_owned())
            .collect(),
        permissions: manifest
            .permissions
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        contributions: manifest
            .contributions
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        components: probed
            .iter()
            .map(|probed| AppComponentDto {
                id: probed.fact.id.to_owned(),
                need: probed.fact.need.into(),
                presence: probed.fact.presence.into(),
                license: probed.tool.license().to_owned(),
                source: probed.source.to_owned(),
                version: probed.version.clone(),
                path: probed
                    .path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                update_available: probed.fact.update_available,
                progress_id: probed.progress_id.map(str::to_owned),
            })
            .collect(),
        highlights: manifest
            .highlights
            .iter()
            .map(|h| (*h).to_owned())
            .collect(),
    }
}

/// 全部一方应用及其状态（「应用」页、编辑器入口判定共用这一份）。
#[tauri::command]
#[specta::specta]
pub(crate) fn apps_list(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Vec<AppDto> {
    let settings = state.settings_snapshot();
    let models_root = state.models_root(&app);
    FIRST_PARTY_APPS
        .iter()
        .map(|manifest| {
            let probed = probe_components(manifest, &settings, &models_root);
            let facts: Vec<ComponentFact> = probed.iter().map(|p| p.fact).collect();
            let installed = is_installed(&settings, manifest.id);
            let size = size_bytes(installed, &facts, &models_root);
            app_dto(manifest, &settings, &probed, size)
        })
        .collect()
}

/// 安装计划里要下载的一件组件。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppDownloadDto {
    /// 组件 id（原样交回 `app_install` 核对）。
    pub id: String,
    /// 上游来源（仓库地址，不带协议）。
    pub source: String,
    /// 典型下载体积（字节，约数）。
    pub approx_download_bytes: f64,
    /// 典型磁盘占用（字节，约数）。
    pub approx_disk_bytes: f64,
    /// 这一件下载时 `model://download-progress` 事件里的 id（按钮上的进度据此认领）。
    pub progress_id: String,
}

/// 「安装」确认框要说的事（ADR-168 ③）：它可以做什么、要下载什么、本机已有什么。不联网。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppInstallPlanDto {
    pub id: String,
    pub permissions: Vec<AppPermissionDto>,
    /// 要下载的组件（按清单顺序；没有就是空，确认框只列权限）。
    pub downloads: Vec<AppDownloadDto>,
    /// 本机已有、将直接使用的组件 id（用户自己装的那几件）。
    pub present: Vec<String>,
    /// 要下载但严格本地模式开着：确认框不给「安装」，给「去关闭严格本地」。
    pub strict_local_blocked: bool,
}

fn manifest_or_err(id: &str) -> Result<&'static AppManifest, String> {
    manifest(id).ok_or_else(|| format!("APP_UNKNOWN: {id}"))
}

/// 「安装」之前的计划（只看本机文件，不联网）。修复走同一份计划。
///
/// # Errors
/// 未知的应用 id。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_install_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<AppInstallPlanDto, String> {
    let manifest = manifest_or_err(&id)?;
    let settings = state.settings_snapshot();
    let facts = component_facts(manifest, &settings, &state.models_root(&app));
    let downloads: Vec<AppDownloadDto> = install_plan(&facts)
        .into_iter()
        .filter_map(crate::components::component)
        .filter_map(|component| {
            Some(AppDownloadDto {
                id: component.tool().as_str().to_owned(),
                source: component.download_source().to_owned(),
                approx_download_bytes: component.typical_download_bytes() as f64,
                approx_disk_bytes: component.typical_disk_bytes() as f64,
                progress_id: component.progress_id()?.to_owned(),
            })
        })
        .collect();
    Ok(AppInstallPlanDto {
        id: manifest.id.to_owned(),
        permissions: manifest
            .permissions
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        strict_local_blocked: !downloads.is_empty() && settings.strict_local(),
        downloads,
        present: facts
            .iter()
            .filter(|fact| fact.need != ComponentNeed::Optional)
            .filter(|fact| fact.presence == ComponentPresence::External)
            .map(|fact| fact.id.to_owned())
            .collect(),
    })
}

/// 一次安装的结果。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppInstallOutcomeDto {
    /// 装上了、但因推荐组件没装成而受限：那几件组件的 id（界面如实说哪一项受限）。
    pub limited_by: Vec<String>,
}

/// 写一位开关（与设置页同一条通路）。
fn write_bit(state: &AppState, key: &str, value: Vec<String>) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        key,
        SettingValueDto::StringList(value),
    )
}

/// 三条写入命令的收尾：重算依赖设置的快照，并以 `apps.enabled` 广播一次「应用变了」。
fn announce(app: &tauri::AppHandle, state: &AppState) {
    // 应用的全局快捷键随生效状态注册 / 解绑；面板的扩展（如内容保护）随之重算（ADR-176 ③ ⑤）。
    crate::reregister_global_shortcuts(app);
    crate::apply_teleprompter_capture(app);
    apply_setting_side_effects(app, state, APPS_INSTALLED_KEY);
    apply_setting_side_effects(app, state, APPS_ENABLED_KEY);
    let _ = SettingsChanged {
        key: APPS_ENABLED_KEY.to_owned(),
    }
    .emit(app);
}

/// 收起这个应用的窗口（暂时停用与卸载共用；窗口由清单声明，ADR-169 ⑧）。有自己收尾的窗口走它
/// 自己的收起路径，其余直接隐藏。
///
/// 共用的窗口（提词器面板归创作与提词器，ADR-176 ③）只在**再没有一个主人生效**时才收起：
/// 调用方先写完开关位再调这里，所以按写后的快照判。
fn retract_windows(app: &tauri::AppHandle, state: &AppState, manifest: &AppManifest) {
    let settings = state.settings_snapshot();
    for label in manifest.windows {
        if luna_config::apps::window_available(&settings, label) {
            continue;
        }
        retract_window(app, label);
    }
}

/// 收起一扇应用窗口。
fn retract_window(app: &tauri::AppHandle, label: &str) {
    if label == super::teleprompter::TELEPROMPTER_LABEL {
        // 提词器：攒下的重来标记照常写回脚本，与用户自己收起同一路径。
        super::teleprompter::teleprompter_hide(app.clone());
    } else if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
}

/// 打开应用窗口的护栏：声明它的应用一个都没生效就拒绝（与 [`require_app`] 同一个错误码）。
/// 共用的窗口任一主人生效即可（ADR-176 ③）。
pub(crate) fn require_window(
    app: &tauri::AppHandle,
    state: &AppState,
    label: &str,
) -> Result<(), String> {
    if luna_config::apps::window_available(&state.settings_snapshot(), label) {
        Ok(())
    } else {
        Err(coded_error(app, "APP_DISABLED", "app-disabled"))
    }
}

/// **安装**（ADR-168 ③）：核对用户看过的计划 → 下载本机缺的必需与推荐组件 → 两位置开。修复
/// 是对已安装的应用再跑一遍（计划只剩缺的那几件）。
///
/// 必需组件下载失败 = 安装失败、两位不动；推荐组件失败 = 装上了、在结果里注明（yt-dlp 能用
/// 就说它能用，与 `install_toolchain` 同一条）。进度沿用 `model://download` 事件。
///
/// # Errors
/// 未知应用、计划已变（`FETCH_TOOL_PLAN_CHANGED`）、要下载而严格本地开着、必需组件下载或
/// 校验失败、设置写入失败时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn app_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    confirmed: Vec<String>,
) -> Result<AppInstallOutcomeDto, String> {
    let manifest = manifest_or_err(&id)?;
    let models_root = state.models_root(&app);
    let facts = component_facts(manifest, &state.settings_snapshot(), &models_root);
    let plan = install_plan(&facts);
    // 从看计划到点安装之间，用户可能刚自己装了 node、或刚删了一件——环境变了就让界面重新
    // 给出计划，而不是按旧计划装、或悄悄多装一件用户没看见的东西。
    if plan.len() != confirmed.len() || !plan.iter().all(|id| confirmed.iter().any(|c| c == id)) {
        return Err(crate::fetch_tool::PLAN_CHANGED.to_owned());
    }
    if !plan.is_empty() {
        refuse_if_strict_local(&app, &state)?;
    }
    for component_id in plan {
        let Some(component) = crate::components::component(component_id) else {
            continue;
        };
        let outcome = component.install(&app, &models_root).await;
        let required = facts
            .iter()
            .any(|fact| fact.id == component_id && fact.need == ComponentNeed::Required);
        if let Err(err) = outcome {
            if required {
                return Err(err);
            }
            eprintln!("[apps] {component_id} 没装成，应用照常装上并注明受限：{err}");
        }
    }
    let settings = state.settings_snapshot();
    write_bit(
        &state,
        APPS_INSTALLED_KEY,
        with_installed(&settings, manifest.id, true),
    )?;
    let settings = state.settings_snapshot();
    write_bit(
        &state,
        APPS_ENABLED_KEY,
        with_enabled(&settings, manifest.id, true),
    )?;
    announce(&app, &state);
    let after = component_facts(manifest, &state.settings_snapshot(), &models_root);
    Ok(AppInstallOutcomeDto {
        limited_by: limited_by(&after).into_iter().map(str::to_owned).collect(),
    })
}

/// 严格本地开着就拦下联网的那几步（与抓取工具同一个错误码，并发一条通知）。
pub(crate) fn refuse_if_strict_local(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(), String> {
    if !state.strict_local() {
        return Ok(());
    }
    if let Some(center) = app.try_state::<NotificationCenter>() {
        center.publish(NotificationEvent::strict_local_blocked(), app);
    }
    Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned())
}

/// **检查更新**（ADR-168 ⑧）：一次查完所有已安装应用里 Lunarca 下载的组件，结论记在进程内，
/// `apps_list` 据此给出「可更新」。只在用户点了才联网；不后台检查、不自动更新。返回有新版的应用 id。
///
/// # Errors
/// 严格本地拦截、联网失败、上游发布物不符预期时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn apps_check_updates(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    refuse_if_strict_local(&app, &state)?;
    let models_root = state.models_root(&app);
    crate::fetch_tool::check_updates(&models_root).await?;
    let settings = state.settings_snapshot();
    Ok(FIRST_PARTY_APPS
        .iter()
        .filter(|manifest| is_installed(&settings, manifest.id))
        .filter(|manifest| {
            component_facts(manifest, &settings, &models_root)
                .iter()
                .any(|fact| fact.update_available)
        })
        .map(|manifest| manifest.id.to_owned())
        .collect())
}

/// **更新**（ADR-168 ⑧）：把这个应用里有新版的、Lunarca 下载的组件逐件重装成最新版（与安装同一条
/// 下载 → 校验 → 落位的路，进度同样走 `model://download-progress`）。用户自己装的不动。
///
/// # Errors
/// 未知应用、严格本地拦截、下载或校验失败时返回稳定错误码；失败的那一件保持原样。
#[tauri::command]
#[specta::specta]
pub(crate) async fn app_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let manifest = manifest_or_err(&id)?;
    refuse_if_strict_local(&app, &state)?;
    let models_root = state.models_root(&app);
    let stale: Vec<&'static dyn crate::components::Component> =
        component_facts(manifest, &state.settings_snapshot(), &models_root)
            .iter()
            .filter(|fact| fact.update_available)
            .filter_map(|fact| crate::components::component(fact.id))
            .collect();
    for component in stale {
        component.install(&app, &models_root).await?;
    }
    Ok(())
}

/// 启用 / 暂时停用一个**已安装**的应用（ADR-168 ④）。暂时停用收起它的窗口、保留组件；库里的
/// 内容一字不动。
///
/// # Errors
/// 未知应用、未安装时启用（`APP_NOT_INSTALLED`）、设置写入失败时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_set_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let manifest = manifest_or_err(&id)?;
    let settings = state.settings_snapshot();
    if enabled && !is_installed(&settings, manifest.id) {
        return Err(coded_error(&app, "APP_NOT_INSTALLED", "app-not-installed"));
    }
    write_bit(
        &state,
        APPS_ENABLED_KEY,
        with_enabled(&settings, manifest.id, enabled),
    )?;
    if !enabled {
        retract_windows(&app, &state, manifest);
    }
    announce(&app, &state);
    Ok(())
}

/// **卸载**（ADR-168 ④）：两位都关、收起窗口，再删掉 Lunarca 下载的那几份组件。用户自己装的
/// 程序一律不动；库里的内容一字不动。先关再删：删的那一刻已经没有入口会用到它们。
///
/// # Errors
/// 未知应用、设置写入失败、组件删除失败（权限等）时返回稳定错误码；删除失败时应用已是未安装。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_uninstall(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let manifest = manifest_or_err(&id)?;
    let settings = state.settings_snapshot();
    write_bit(
        &state,
        APPS_ENABLED_KEY,
        with_enabled(&settings, manifest.id, false),
    )?;
    let settings = state.settings_snapshot();
    write_bit(
        &state,
        APPS_INSTALLED_KEY,
        with_installed(&settings, manifest.id, false),
    )?;
    retract_windows(&app, &state, manifest);
    announce(&app, &state);
    let models_root = state.models_root(&app);
    for declared in manifest.components {
        if let Some(component) = crate::components::component(declared.id) {
            component.uninstall(&models_root)?;
        }
    }
    Ok(())
}

/// 入口命令的护栏：这个应用没启用就拒绝（前端此时本不该露出入口；这是别处触达时的兜底）。
pub(crate) fn require_app(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<(), String> {
    if is_enabled(&state.settings_snapshot(), id) {
        Ok(())
    } else {
        Err(coded_error(app, "APP_DISABLED", "app-disabled"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清单里的组件 id 与抓取工具的词表是同一个（本 crate 两边都能看到，在这里钉住）。
    #[test]
    fn component_ids_are_external_tool_ids() {
        let tools: Vec<&str> = luna_fetch::tool::ExternalTool::ALL
            .iter()
            .map(|tool| tool.as_str())
            .collect();
        for app in FIRST_PARTY_APPS {
            for component in app.components {
                assert!(
                    tools.contains(&component.id),
                    "{} 不是外部工具 id",
                    component.id
                );
            }
        }
    }

    fn settings_with(installed: &[&str], enabled: &[&str]) -> luna_config::Settings {
        let mut settings = luna_config::Settings::default();
        let list = |ids: &[&str]| {
            luna_config::SettingValue::StringList(ids.iter().map(|s| (*s).to_owned()).collect())
        };
        settings.set(APPS_INSTALLED_KEY, list(installed));
        settings.set(APPS_ENABLED_KEY, list(enabled));
        settings
    }

    #[test]
    fn the_projection_carries_state_action_and_store_facts() {
        let creator = &luna_config::apps::CREATOR_APP;
        let off = app_dto(creator, &luna_config::Settings::default(), &[], None);
        assert_eq!(off.id, "creator");
        assert!(
            !off.enabled && !off.installed,
            "创作默认未安装（ADR-168 ②）"
        );
        assert_eq!(off.state, AppLifecycleDto::NotInstalled);
        assert_eq!(off.action, AppActionDto::Install);
        assert_eq!(off.space, Some(WorkspaceSpaceDto::Creator));
        assert!(off.contributions.contains(&AppContributionDto::Space));
        assert_eq!(off.license, "AGPL-3.0-only");
        assert_eq!(off.developer, "Lunarca");
        assert_eq!(off.version, env!("CARGO_PKG_VERSION"));

        let on = app_dto(
            creator,
            &settings_with(&["creator"], &["creator"]),
            &[],
            None,
        );
        assert!(on.enabled);
        assert_eq!(on.state, AppLifecycleDto::Installed);
        assert_eq!(on.action, AppActionDto::Open);

        let paused = app_dto(
            creator,
            &settings_with(&["creator"], &["off:creator"]),
            &[],
            None,
        );
        assert!(!paused.enabled && paused.installed);
        assert_eq!(paused.state, AppLifecycleDto::Paused);
        assert_eq!(paused.action, AppActionDto::Enable);

        let enabled_only = app_dto(creator, &settings_with(&[], &["creator"]), &[], None);
        assert!(!enabled_only.enabled, "没装就不生效");
        assert_eq!(enabled_only.state, AppLifecycleDto::NotInstalled);
    }

    #[test]
    fn link_import_needs_repair_when_its_required_component_is_gone() {
        let link = &luna_config::apps::LINK_IMPORT_APP;
        let probed = [ProbedComponent {
            fact: ComponentFact {
                id: "yt_dlp",
                need: ComponentNeed::Required,
                presence: ComponentPresence::Missing,
                installable: true,
                update_available: false,
            },
            tool: ExternalTool::YtDlp,
            path: None,
            version: None,
            source: ExternalTool::YtDlp.source(),
            progress_id: Some(crate::fetch_tool::TOOL_ID),
        }];
        let dto = app_dto(
            link,
            &settings_with(&["link_import"], &["link_import"]),
            &probed,
            None,
        );
        assert_eq!(dto.state, AppLifecycleDto::NeedsRepair);
        assert_eq!(dto.action, AppActionDto::Repair);
        let yt = &dto.components[0];
        assert_eq!(yt.presence, ComponentPresenceDto::Missing);
        assert_eq!(
            (yt.license.as_str(), yt.source.as_str()),
            ("Unlicense", "github.com/yt-dlp/yt-dlp")
        );
        assert_eq!(dto.highlights, ["paste", "local", "sources"]);
        assert_eq!(yt.progress_id.as_deref(), Some(crate::fetch_tool::TOOL_ID));
        assert!(!yt.update_available);
    }

    #[test]
    fn every_declared_component_has_an_implementation_and_windows_are_real() {
        for app in FIRST_PARTY_APPS {
            for declared in app.components {
                assert!(
                    crate::components::component(declared.id).is_some(),
                    "{} 没有组件实现",
                    declared.id
                );
            }
            // 清单里声明的窗口都是应用真的会建的那几扇（新窗口要在这里与 `retract_window` 里各认一次）。
            for label in app.windows {
                assert!(
                    [super::super::teleprompter::TELEPROMPTER_LABEL].contains(label),
                    "{} 声明的窗口 {label} 不是已知窗口",
                    app.id
                );
            }
        }
        assert!(
            crate::components::component("ffmpeg").is_some_and(|c| !c.installable()),
            "ffmpeg 不能代装（ADR-023）"
        );
    }

    #[test]
    fn owned_note_types_are_real_note_types() {
        // 清单里的 `luna.type` 是字符串（luna-config 不依赖 luna-vault）；这里钉住它们都是
        // 真实存在的 `NoteType`，改名时当场红。
        for app in FIRST_PARTY_APPS {
            for note_type in app.note_types {
                assert!(
                    luna_vault::NoteType::ALL
                        .iter()
                        .any(|known| known.key() == *note_type),
                    "{} 的类型 {note_type} 不是 NoteType",
                    app.id
                );
            }
        }
    }
}
