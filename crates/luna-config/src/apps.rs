//! 一方应用清单（ADR-165 ②）——**一方应用的唯一事实源**。
//!
//! 一方应用 = 随客户端发布、可在「应用」里启用与停用的一组能力（首个是「创作」，PRD-19）。
//! 它拥有哪些空间、哪些 `luna.type`、哪些设置键、声明哪些权限、贡献哪几类入口，全部写在
//! 这一张表里；活动栏、设置页、模板选择器、编辑器动作都从这里问「这个东西归哪个应用、那个
//! 应用开着没有」，不各自维护一份名单。
//!
//! **启用状态与活动栏显隐是两件事**（ADR-165 ③）：`apps.installed` / `apps.enabled` 回答「这个
//! 应用此刻生效没有」（生效 = 已安装 ∧ 已启用，ADR-168 ②），`workspace.rail_layout` 回答「它的
//! 空间放不放在活动栏上、放在第几格」。卸载或暂时停用都收回这个应用的全部入口，库里的文件一字
//! 不动（ADR-165 ④）。
//!
//! 用户看到的状态只有一套（ADR-168 ①）：[`lifecycle`] 由这两位与组件事实算出，[`primary_action`]
//! 给出与之对应的主按钮，[`install_plan`] 给出安装要下载哪几件——三者都是纯函数，IO（找组件在
//! 不在）由 IPC 层做完再交进来。
//!
//! 零 IO：只有常量与纯函数，设置注册表与 IPC 层都能直接依赖。`luna.type` 以稳定字符串给出
//! （本 crate 不能依赖 `luna-vault`），与 `NoteType` 的一致性由 `luna-app` 的测试钉住。

use crate::registry::{resolve_ordered_toggle_list, toggle_token, ToggleOption};
use crate::Settings;

/// 启用状态的设置键（有序开关列表，设备级，内部键——只由「应用」页改）。值是全部一方应用的
/// token 列表：`<id>` = 已启用，`off:<id>` = 未启用；缺项按 [`APP_OPTIONS`] 的默认补回。
///
/// 这一位只在已安装时有意义（[`is_enabled`]）。「应用变了」的广播一律以这个键发出：三条写入
/// 命令每次都会写它，订阅方只认它一个键就够。
pub const APPS_ENABLED_KEY: &str = "apps.enabled";

/// 安装状态的设置键（ADR-168 ②）：与 [`APPS_ENABLED_KEY`] 同形、同一份选项，默认全部未安装。
pub const APPS_INSTALLED_KEY: &str = "apps.installed";

/// 一方应用的开发者（详情页信息栏）。
pub const FIRST_PARTY_DEVELOPER: &str = "Lunarca";

/// 一方应用的许可证（SPDX）：一方应用的代码就是客户端的代码，许可证即工作区的许可证。
pub const FIRST_PARTY_LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// 未启用项的 token 前缀。
pub const APPS_OFF_PREFIX: &str = "off:";

/// 一方应用声明的权限（详情页「权限与数据」逐条写明，ADR-165 ⑤）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppPermission {
    /// 在库里自己的目录下新建与改写笔记。
    VaultFolder,
    /// 使用麦克风（口述、跟读）。
    Microphone,
    /// 联网（可单独关闭的显式动作）。
    Network,
    /// 调用 AI（按用户的算力设置路由）。
    Ai,
    /// 下载并运行第三方程序（应用的组件，ADR-167 ⑤：每次下载都由你确认）。
    ExternalPrograms,
}

impl AppPermission {
    /// 稳定名（IPC 投影与文案键的后缀）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VaultFolder => "vault_folder",
            Self::Microphone => "microphone",
            Self::Network => "network",
            Self::Ai => "ai",
            Self::ExternalPrograms => "external_programs",
        }
    }
}

/// 一方应用贡献的入口种类（详情页「启用后会多出」由它生成；停用时这些一并收回）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppContribution {
    /// 活动栏上的一个空间（及命令面板里的「前往」）。
    Space,
    /// 编辑器里的动作（选区气泡、AI 菜单的一组、文档类型专属的工具）。
    EditorActions,
    /// 模板选择器里的笔记类型。
    Templates,
    /// 独立窗口（如提词器）。
    Windows,
    /// 设置项。
    Settings,
    /// 随应用发布的 AI 提示词（ADR-166 ⑤：`luna_core::prompt::APP_TEMPLATES` 里以应用 id 开头的
    /// 那些，离线恒在，服务端目录可选覆盖）。
    Prompts,
    /// 「从链接导入」入口（转写页、创作「添加来源」的视频链接，ADR-167 ④⑤）。
    LinkImport,
}

impl AppContribution {
    /// 稳定名（IPC 投影与文案键的后缀）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::EditorActions => "editor_actions",
            Self::Templates => "templates",
            Self::Windows => "windows",
            Self::Settings => "settings",
            Self::Prompts => "prompts",
            Self::LinkImport => "link_import",
        }
    }
}

/// 一个组件对应用有多要紧（ADR-167 ⑤）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentNeed {
    /// 没有它应用就不能用。
    Required,
    /// 没有它也能用，但常见场景会受限（如 YouTube 需要 JS 运行时）。
    Recommended,
    /// 锦上添花（如高清合并）。
    Optional,
}

impl ComponentNeed {
    /// 稳定名（IPC 投影与文案键的后缀）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Recommended => "recommended",
            Self::Optional => "optional",
        }
    }
}

/// 应用的一个组件：一件第三方程序（ADR-167 ⑤）。装不装、从哪装、怎么检测由组件自己的实现
/// 决定（`luna_fetch::ExternalTool`，本 crate 只记稳定 id）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppComponent {
    /// 组件 id（与 `luna_fetch::ExternalTool::id` 同一个词表，由 `luna-app` 的测试钉住）。
    pub id: &'static str,
    pub need: ComponentNeed,
}

/// 一份一方应用清单。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppManifest {
    /// 稳定 id（`apps.enabled` 的取值、`apps://<id>` 深链、文案键的一段）。
    pub id: &'static str,
    /// 它拥有的活动栏空间 id（在 [`crate::workspace_space::RAIL_SPACE_IDS`] 内）。
    pub space: Option<&'static str>,
    /// 它拥有的 `luna.type`（稳定字符串）。
    pub note_types: &'static [&'static str],
    /// 它拥有的设置键：精确键。
    pub setting_keys: &'static [&'static str],
    /// 它拥有的设置键：前缀（`creator.` 一枝全归它）。
    pub setting_prefixes: &'static [&'static str],
    /// 声明的权限（按详情页的展示顺序）。
    pub permissions: &'static [AppPermission],
    /// 贡献的入口种类（按详情页的展示顺序）。
    pub contributions: &'static [AppContribution],
    /// 需要下载的第三方组件（按详情页的展示顺序；没有就是空）。
    pub components: &'static [AppComponent],
    /// 详情页的亮点（两到四条，按「能做成什么」写；文案键 `apps.<id>.highlight.<亮点>.title|body`，
    /// ADR-168 ⑥）。
    pub highlights: &'static [&'static str],
    /// 它拥有的窗口（Tauri 窗口标签，ADR-169 ⑧）：暂时停用与卸载时收起，未启用时打不开。
    /// 有窗口的应用在 `contributions` 里写 [`AppContribution::Windows`]，由测试钉成一致。
    pub windows: &'static [&'static str],
}

impl AppManifest {
    /// 这个设置键归不归它。
    #[must_use]
    pub fn owns_setting(&self, key: &str) -> bool {
        self.setting_keys.contains(&key)
            || self
                .setting_prefixes
                .iter()
                .any(|prefix| key.starts_with(prefix))
    }
}

/// 「从链接导入」第一次使用前确认过权利声明没有（设备级内部键，ADR-167 ④）：只导入你拥有或
/// 已获授权使用的内容、遵守来源平台条款、不绕过登录 / 付费墙 / DRM。
pub const LINK_IMPORT_RIGHTS_KEY: &str = "link_import.rights_confirmed";

/// 一方应用的登记处（ADR-169 ⑧）：每个应用只在这里写一次——id 与清单正文。清单常量、
/// [`FIRST_PARTY_APPS`]（「应用」页的列表顺序）、`apps.enabled` / `apps.installed` 的选项
/// [`APP_OPTIONS`] 与默认值 [`APPS_ENABLED_DEFAULT`] 都由它展开，不再各写一份名单。
macro_rules! first_party_apps {
    ($($(#[$meta:meta])* $name:ident = $id:literal { $($field:tt)* })+) => {
        $(
            $(#[$meta])*
            pub const $name: AppManifest = AppManifest { id: $id, $($field)* };
        )+

        /// 全部一方应用（「应用」页的列表顺序）。
        pub const FIRST_PARTY_APPS: &[AppManifest] = &[$($name),+];

        /// `apps.enabled` 的选项全集（与 [`FIRST_PARTY_APPS`] 同序）：每个应用一位开关，一律默认关
        /// （ADR-165 ③），没有锁定项。
        pub const APP_OPTIONS: &[ToggleOption] = &[$(
            ToggleOption {
                id: $id,
                label_key: concat!("workspace:apps.", $id, ".name"),
                locked: false,
                default_visible: false,
            }
        ),+];

        /// `apps.enabled` 的默认值：全部未启用（与 [`APP_OPTIONS`] 的 `default_visible` 是同一个事实）。
        pub const APPS_ENABLED_DEFAULT: &[&str] = &[$(concat!("off:", $id)),+];
    };
}

first_party_apps! {
    /// 「创作」（PRD-19 / ADR-162 / ADR-165）。
    CREATOR_APP = "creator" {
        space: Some("creator"),
        note_types: &["video-project", "script", "swipe"],
        // 创作目录与网页剪藏开关只服务于创作（ADR-164 ⑦ 的剪藏入口在项目概览里）。
        setting_keys: &["vault.creator_dir", "privacy.web_clip"],
        // 看板偏好与提词器的显示偏好（后者本就是内部键）。
        setting_prefixes: &["creator.", "teleprompter."],
        permissions: &[
            AppPermission::VaultFolder,
            AppPermission::Microphone,
            AppPermission::Network,
            AppPermission::Ai,
        ],
        contributions: &[
            AppContribution::Space,
            AppContribution::EditorActions,
            AppContribution::Templates,
            AppContribution::Windows,
            AppContribution::Settings,
            AppContribution::Prompts,
        ],
        components: &[],
        highlights: &["pipeline", "teleprompter", "publish"],
        // 提词器（PRD-19 §3.4）：与「提词器」应用共用这一扇（ADR-176 ③）。
        windows: &["teleprompter"],
    }

    /// 「提词器」（PRD-20 / ADR-176）：可单独安装的提词器，有自己的空间；与「创作」共用同一扇
    /// 面板与 `teleprompter.*` 内部键（任一拥有者生效即可用），扩展能力只归它。
    TELEPROMPTER_APP = "teleprompter" {
        space: Some("teleprompter"),
        // 不拥有任何类型：库里任意一篇笔记都能提词。
        note_types: &[],
        // 提词稿目录只服务于提词器（「新建提词稿」「从剪贴板新建」）。
        setting_keys: &["vault.teleprompter_dir"],
        // 提词器的全局键（立即隐藏、送稿、浮窗按键）一枝全归它（ADR-176 ⑤ ⑥ ⑨）。
        setting_prefixes: &["teleprompter.", "shortcuts.teleprompter_"],
        permissions: &[
            AppPermission::Microphone,
            AppPermission::VaultFolder,
            AppPermission::Ai,
        ],
        contributions: &[
            AppContribution::Space,
            AppContribution::EditorActions,
            AppContribution::Windows,
            AppContribution::Settings,
            AppContribution::Prompts,
        ],
        components: &[],
        highlights: &["any_note", "follow", "camera"],
        windows: &["teleprompter"],
    }

    /// 「从链接导入」（ADR-167 ⑤，PRD-19-045 / PRD-05-006）：从视频网站的链接下载音视频再转写。
    /// 组件是 yt-dlp（必需）、JS 运行时（推荐，YouTube 需要）与 ffmpeg（可选，高清合并）。
    LINK_IMPORT_APP = "link_import" {
        space: None,
        note_types: &[],
        // 下载偏好、代理与 yt-dlp 路径覆盖只服务于从链接导入。
        setting_keys: &[
            "transcribe.fetch_media",
            "transcribe.fetch_proxy",
            "transcribe.fetch_tool_path",
        ],
        setting_prefixes: &["link_import."],
        permissions: &[
            AppPermission::Network,
            AppPermission::ExternalPrograms,
            AppPermission::VaultFolder,
        ],
        contributions: &[AppContribution::LinkImport, AppContribution::Settings],
        components: &[
            AppComponent {
                id: "yt_dlp",
                need: ComponentNeed::Required,
            },
            AppComponent {
                id: "js_runtime",
                need: ComponentNeed::Recommended,
            },
            AppComponent {
                id: "ffmpeg",
                need: ComponentNeed::Optional,
            },
        ],
        highlights: &["paste", "local", "sources"],
        windows: &[],
    }
}

/// `apps.installed` 的默认值：全部未安装（与 [`APPS_ENABLED_DEFAULT`] 同一份 token）。
pub const APPS_INSTALLED_DEFAULT: &[&str] = APPS_ENABLED_DEFAULT;

/// 声明了这个窗口（Tauri 标签）的应用（清单顺序）。同一扇窗可以有多个主人（提词器面板归创作与
/// 提词器，ADR-176 ③）；不归任何应用的窗口没有主人。
pub fn window_owners(label: &str) -> impl Iterator<Item = &'static AppManifest> + '_ {
    FIRST_PARTY_APPS
        .iter()
        .filter(move |app| app.windows.contains(&label))
}

/// 这扇窗此刻可用吗：不归任何应用，或者它的主人里**至少一个**已生效（ADR-176 ③）。
#[must_use]
pub fn window_available(settings: &Settings, label: &str) -> bool {
    let mut owners = window_owners(label).peekable();
    owners.peek().is_none() || owners.any(|app| is_enabled(settings, app.id))
}

/// 按 id 找清单。
#[must_use]
pub fn manifest(id: &str) -> Option<&'static AppManifest> {
    FIRST_PARTY_APPS.iter().find(|app| app.id == id)
}

/// 某个开关键归一化后的取值（与注册表的有序开关列表同一个实现：未知 id 丢弃、重复只留首次、
/// 缺项按默认补回）。
fn resolved(settings: &Settings, key: &str) -> Vec<(&'static str, bool)> {
    let tokens = settings
        .get(key)
        .and_then(|value| value.as_string_list().map(<[String]>::to_vec))
        .unwrap_or_default();
    resolve_ordered_toggle_list(APP_OPTIONS, APPS_OFF_PREFIX, 0, &tokens)
}

/// 某个开关键为开的应用 id（按清单顺序）。
fn on_ids(settings: &Settings, key: &str) -> Vec<&'static str> {
    let on: Vec<&'static str> = resolved(settings, key)
        .into_iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(id, _)| id)
        .collect();
    FIRST_PARTY_APPS
        .iter()
        .map(|app| app.id)
        .filter(|id| on.contains(id))
        .collect()
}

/// 已安装的应用 id（按清单顺序）。
#[must_use]
pub fn installed_ids(settings: &Settings) -> Vec<&'static str> {
    on_ids(settings, APPS_INSTALLED_KEY)
}

/// 这个应用装了没有（未知 id 一律为否）。
#[must_use]
pub fn is_installed(settings: &Settings, id: &str) -> bool {
    installed_ids(settings).contains(&id)
}

/// 生效的应用 id（按清单顺序）：已安装 ∧ 已启用（ADR-168 ②）。
#[must_use]
pub fn enabled_ids(settings: &Settings) -> Vec<&'static str> {
    let installed = installed_ids(settings);
    on_ids(settings, APPS_ENABLED_KEY)
        .into_iter()
        .filter(|id| installed.contains(id))
        .collect()
}

/// 这个应用此刻生效没有（未知 id 一律为否）。所有入口都只问这一个函数。
#[must_use]
pub fn is_enabled(settings: &Settings, id: &str) -> bool {
    enabled_ids(settings).contains(&id)
}

/// 只改一位之后某个开关键的取值（完整全集；未知 id 什么都不改）。
fn with_bit(settings: &Settings, key: &str, id: &str, on: bool) -> Vec<String> {
    resolved(settings, key)
        .into_iter()
        .map(|(app, now)| toggle_token(APPS_OFF_PREFIX, app, if app == id { on } else { now }))
        .collect()
}

/// 启用 / 暂时停用之后的 `apps.enabled` 取值。
#[must_use]
pub fn with_enabled(settings: &Settings, id: &str, enabled: bool) -> Vec<String> {
    with_bit(settings, APPS_ENABLED_KEY, id, enabled)
}

/// 安装 / 卸载之后的 `apps.installed` 取值。
#[must_use]
pub fn with_installed(settings: &Settings, id: &str, installed: bool) -> Vec<String> {
    with_bit(settings, APPS_INSTALLED_KEY, id, installed)
}

/// 一件组件在这台机器上的情况（IPC 层查完文件系统后交进来）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentPresence {
    /// 没找到。
    Missing,
    /// 找到的是 Lunarca 下载的那一份（卸载时会删、检查更新时会查）。
    Managed,
    /// 找到的是用户自己装的（Homebrew 等）：直接用，Lunarca 不动它。
    External,
}

/// 一件组件的事实。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentFact {
    pub id: &'static str,
    pub need: ComponentNeed,
    pub presence: ComponentPresence,
    /// Lunarca 能不能在这个平台上代装它（ffmpeg 一律不能，ADR-023）。
    pub installable: bool,
    /// 上一次「检查更新」说它有新版（只对 Lunarca 下载的那份有意义）。
    pub update_available: bool,
}

/// 用户看到的应用状态（ADR-168 ①）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLifecycle {
    NotInstalled,
    Installed,
    /// 已安装、暂时停用（入口收起，组件保留）。
    Paused,
    /// 已安装、已启用，但必需组件不在了（被用户删了、或换机后没装）。
    NeedsRepair,
    /// 已安装、可用，某件 Lunarca 下载的组件有新版。
    UpdateAvailable,
}

/// 与状态一一对应的主按钮（ADR-168 ①）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    Install,
    /// 有空间的应用进空间；没有空间的去它的入口（如「从链接导入」对话框）。
    Open,
    Enable,
    Repair,
    Update,
    /// 这一步要联网，而严格本地模式开着：主按钮带用户去关它。
    LeaveStrictLocal,
}

/// 由安装 / 启用两位与组件事实算出状态（**纯函数**）。暂时停用优先于需要修复：停用的应用不
/// 被使用，缺什么等启用之后再说。
#[must_use]
pub fn lifecycle(installed: bool, enabled: bool, components: &[ComponentFact]) -> AppLifecycle {
    if !installed {
        return AppLifecycle::NotInstalled;
    }
    if !enabled {
        return AppLifecycle::Paused;
    }
    if components
        .iter()
        .any(|c| c.need == ComponentNeed::Required && c.presence == ComponentPresence::Missing)
    {
        return AppLifecycle::NeedsRepair;
    }
    if components
        .iter()
        .any(|c| c.presence == ComponentPresence::Managed && c.update_available)
    {
        return AppLifecycle::UpdateAvailable;
    }
    AppLifecycle::Installed
}

/// 安装（或修复）要下载的组件 id，按清单顺序（**纯函数**）：必需与推荐里本机没有、Lunarca 又
/// 能代装的那几件。用户自己装的直接用；可选组件不进安装（ADR-168 ③）。
#[must_use]
pub fn install_plan(components: &[ComponentFact]) -> Vec<&'static str> {
    components
        .iter()
        .filter(|c| c.need != ComponentNeed::Optional)
        .filter(|c| c.presence == ComponentPresence::Missing && c.installable)
        .map(|c| c.id)
        .collect()
}

/// 已经能用、但因推荐组件不在而受限：那几件的 id（详情页如实说哪一项受限）。
#[must_use]
pub fn limited_by(components: &[ComponentFact]) -> Vec<&'static str> {
    components
        .iter()
        .filter(|c| {
            c.need == ComponentNeed::Recommended && c.presence == ComponentPresence::Missing
        })
        .map(|c| c.id)
        .collect()
}

/// 状态对应的主按钮（**纯函数**）。要联网的那几步（要下载的安装、修复、更新）在严格本地模式下
/// 一律换成「去关闭严格本地」——按钮点了也只会被硬拦截，不如直说。
#[must_use]
pub fn primary_action(
    state: AppLifecycle,
    components: &[ComponentFact],
    strict_local: bool,
) -> AppAction {
    let (action, needs_network) = match state {
        AppLifecycle::NotInstalled => (AppAction::Install, !install_plan(components).is_empty()),
        AppLifecycle::Installed => (AppAction::Open, false),
        AppLifecycle::Paused => (AppAction::Enable, false),
        AppLifecycle::NeedsRepair => (AppAction::Repair, true),
        AppLifecycle::UpdateAvailable => (AppAction::Update, true),
    };
    if needs_network && strict_local {
        AppAction::LeaveStrictLocal
    } else {
        action
    }
}

/// 声明了这个设置键的应用（清单顺序；不归任何应用 = 空）。`teleprompter.*` 同归创作与提词器
/// （ADR-176 ③）。
pub fn setting_owners(key: &str) -> impl Iterator<Item = &'static AppManifest> + '_ {
    FIRST_PARTY_APPS
        .iter()
        .filter(move |app| app.owns_setting(key))
}

/// 这个设置键此刻可用吗：不归任何应用，或者它的主人里至少一个已生效（与 [`window_available`]
/// 同一条）。
#[must_use]
pub fn setting_available(settings: &Settings, key: &str) -> bool {
    let mut owners = setting_owners(key).peekable();
    owners.peek().is_none() || owners.any(|app| is_enabled(settings, app.id))
}

/// 这个活动栏空间归哪个应用。
#[must_use]
pub fn space_owner(space: &str) -> Option<&'static AppManifest> {
    FIRST_PARTY_APPS.iter().find(|app| app.space == Some(space))
}

/// 这个 `luna.type` 归哪个应用。
#[must_use]
pub fn note_type_owner(note_type: &str) -> Option<&'static AppManifest> {
    FIRST_PARTY_APPS
        .iter()
        .find(|app| app.note_types.contains(&note_type))
}

/// 这个空间此刻可用吗：不归任何应用，或者它的应用已启用。
#[must_use]
pub fn space_available(settings: &Settings, space: &str) -> bool {
    space_owner(space).is_none_or(|app| is_enabled(settings, app.id))
}

/// 此刻不在活动栏上的空间：所属一方应用未启用（ADR-165 ③）。活动栏、设置页的「活动栏布局」
/// 与拖拽排序都按它把这些空间整格拿掉，用户对它们的排序与显隐偏好原样保留。
#[must_use]
pub fn unavailable_spaces(settings: &Settings) -> Vec<&'static str> {
    crate::workspace_space::RAIL_SPACE_IDS
        .iter()
        .copied()
        .filter(|id| !space_available(settings, id))
        .collect()
}

/// 这个 `luna.type` 的专属入口此刻可用吗：不归任何应用，或者它的应用已启用。
#[must_use]
pub fn note_type_available(settings: &Settings, note_type: &str) -> bool {
    note_type_owner(note_type).is_none_or(|app| is_enabled(settings, app.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SettingValue, Settings};

    fn ids_of(tokens: &[String]) -> Vec<&str> {
        tokens.iter().map(String::as_str).collect()
    }

    fn with(tokens: &[&str]) -> Settings {
        let mut settings = Settings::default();
        settings.set(
            APPS_ENABLED_KEY,
            SettingValue::StringList(tokens.iter().map(|s| (*s).to_owned()).collect()),
        );
        settings
    }

    #[test]
    fn options_and_manifests_are_the_same_list_in_the_same_order() {
        let from_manifests: Vec<&str> = FIRST_PARTY_APPS.iter().map(|app| app.id).collect();
        let from_options: Vec<&str> = APP_OPTIONS.iter().map(|o| o.id).collect();
        assert_eq!(from_manifests, from_options);
        assert!(
            APP_OPTIONS.iter().all(|o| !o.default_visible && !o.locked),
            "一律默认关、不锁定"
        );
        let defaults: Vec<String> = APP_OPTIONS
            .iter()
            .map(|o| toggle_token(APPS_OFF_PREFIX, o.id, o.default_visible))
            .collect();
        assert_eq!(ids_of(&defaults), APPS_ENABLED_DEFAULT);
    }

    #[test]
    fn apps_with_windows_say_so_and_each_window_has_one_owner() {
        for app in FIRST_PARTY_APPS {
            assert_eq!(
                app.contributions.contains(&AppContribution::Windows),
                !app.windows.is_empty(),
                "{} 的「贡献窗口」与它声明的窗口对不上",
                app.id
            );
            for label in app.windows {
                assert!(window_owners(label).any(|owner| owner.id == app.id));
            }
        }
        assert!(window_owners("main").next().is_none());
    }

    #[test]
    fn the_teleprompter_panel_is_available_while_either_owner_is_on() {
        let owners: Vec<&str> = window_owners("teleprompter").map(|app| app.id).collect();
        assert_eq!(owners, ["creator", "teleprompter"]);
        assert!(!window_available(&Settings::default(), "teleprompter"));
        let creator = installed_and_enabled(&["creator"], &["creator"]);
        assert!(window_available(&creator, "teleprompter"));
        assert!(setting_available(&creator, "teleprompter.font_size"));
        let own = installed_and_enabled(&["teleprompter"], &["teleprompter"]);
        assert!(window_available(&own, "teleprompter"));
        assert!(setting_available(&own, "teleprompter.follow_enabled"));
        assert!(!setting_available(&own, "creator.board_view"));
        let paused = installed_and_enabled(
            &["creator", "teleprompter"],
            &["off:creator", "off:teleprompter"],
        );
        assert!(
            !window_available(&paused, "teleprompter"),
            "两个都停用才收起"
        );
        assert!(
            window_available(&Settings::default(), "main"),
            "不归任何应用的窗口恒可用"
        );
    }

    #[test]
    fn every_owned_space_is_a_rail_space_and_owned_once() {
        for app in FIRST_PARTY_APPS {
            if let Some(space) = app.space {
                assert!(
                    crate::workspace_space::RAIL_SPACE_IDS.contains(&space),
                    "{} 的空间 {space} 不在活动栏词表里",
                    app.id
                );
                assert_eq!(space_owner(space).map(|owner| owner.id), Some(app.id));
            }
        }
    }

    #[test]
    fn nothing_is_enabled_by_default() {
        let settings = Settings::default();
        assert!(enabled_ids(&settings).is_empty());
        assert!(!is_enabled(&settings, "creator"));
        assert!(!space_available(&settings, "creator"));
        assert!(
            space_available(&settings, "notes"),
            "不归任何应用的空间恒可用"
        );
        assert!(!note_type_available(&settings, "script"));
        assert!(note_type_available(&settings, "note"));
    }

    #[test]
    fn unknown_and_duplicate_ids_do_not_count() {
        let settings = installed_and_enabled(&["creator"], &["bogus", "creator", "off:creator"]);
        assert_eq!(enabled_ids(&settings), ["creator"], "重复只认第一次");
        assert!(space_available(&settings, "creator"));
    }

    fn installed_and_enabled(installed: &[&str], enabled: &[&str]) -> Settings {
        let mut settings = with(enabled);
        settings.set(
            APPS_INSTALLED_KEY,
            SettingValue::StringList(installed.iter().map(|s| (*s).to_owned()).collect()),
        );
        settings
    }

    #[test]
    fn an_app_takes_effect_only_when_installed_and_enabled() {
        let both = installed_and_enabled(&["creator"], &["creator"]);
        assert!(is_enabled(&both, "creator"));
        assert!(space_available(&both, "creator"));

        let enabled_only = with(&["creator"]);
        assert!(!is_installed(&enabled_only, "creator"));
        assert!(!is_enabled(&enabled_only, "creator"), "没装就不生效");
        assert!(!space_available(&enabled_only, "creator"));

        let paused = installed_and_enabled(&["creator"], &["off:creator"]);
        assert!(is_installed(&paused, "creator"));
        assert!(!is_enabled(&paused, "creator"));
        assert!(!note_type_available(&paused, "script"));
    }

    #[test]
    fn installing_writes_the_full_set_in_manifest_order() {
        let off = Settings::default();
        assert_eq!(
            ids_of(&with_installed(&off, "link_import", true)),
            ["off:creator", "off:teleprompter", "link_import"]
        );
    }

    fn fact(id: &'static str, need: ComponentNeed, presence: ComponentPresence) -> ComponentFact {
        ComponentFact {
            id,
            need,
            presence,
            installable: id != "ffmpeg",
            update_available: false,
        }
    }

    fn link_import(yt: ComponentPresence, js: ComponentPresence) -> Vec<ComponentFact> {
        vec![
            fact("yt_dlp", ComponentNeed::Required, yt),
            fact("js_runtime", ComponentNeed::Recommended, js),
            fact(
                "ffmpeg",
                ComponentNeed::Optional,
                ComponentPresence::Missing,
            ),
        ]
    }

    #[test]
    fn the_lifecycle_follows_the_two_bits_then_the_components() {
        use ComponentPresence::{External, Managed, Missing};
        let ready = link_import(Managed, External);
        assert_eq!(lifecycle(false, true, &ready), AppLifecycle::NotInstalled);
        assert_eq!(lifecycle(true, false, &ready), AppLifecycle::Paused);
        assert_eq!(lifecycle(true, true, &ready), AppLifecycle::Installed);
        assert_eq!(
            lifecycle(true, false, &link_import(Missing, Missing)),
            AppLifecycle::Paused,
            "停用的应用不被使用，缺什么等启用再说"
        );
        assert_eq!(
            lifecycle(true, true, &link_import(Missing, Managed)),
            AppLifecycle::NeedsRepair
        );
        assert_eq!(
            lifecycle(true, true, &link_import(Managed, Missing)),
            AppLifecycle::Installed,
            "推荐组件不在只是受限，不算坏"
        );
        let mut stale = link_import(Managed, Managed);
        stale[0].update_available = true;
        assert_eq!(lifecycle(true, true, &stale), AppLifecycle::UpdateAvailable);
        let mut external_stale = link_import(External, External);
        external_stale[0].update_available = true;
        assert_eq!(
            lifecycle(true, true, &external_stale),
            AppLifecycle::Installed,
            "用户自己装的那份不归 Lunarca 更新"
        );
        assert_eq!(lifecycle(true, true, &[]), AppLifecycle::Installed);
    }

    #[test]
    fn the_install_plan_skips_what_is_already_here_and_optional_components() {
        use ComponentPresence::{External, Managed, Missing};
        assert_eq!(
            install_plan(&link_import(Missing, Missing)),
            ["yt_dlp", "js_runtime"],
            "ffmpeg 是可选组件，不进安装"
        );
        assert_eq!(
            install_plan(&link_import(External, Missing)),
            ["js_runtime"]
        );
        assert!(install_plan(&link_import(Managed, External)).is_empty());
        let mut no_js_asset = link_import(Missing, Missing);
        no_js_asset[1].installable = false;
        assert_eq!(install_plan(&no_js_asset), ["yt_dlp"], "装不了的不进计划");
        assert!(install_plan(&[]).is_empty(), "没有组件的应用什么都不下");
        assert_eq!(limited_by(&link_import(Managed, Missing)), ["js_runtime"]);
        assert!(limited_by(&link_import(Managed, External)).is_empty());
    }

    #[test]
    fn the_primary_action_matches_the_state_and_respects_strict_local() {
        use ComponentPresence::{Managed, Missing};
        let missing = link_import(Missing, Missing);
        assert_eq!(
            primary_action(AppLifecycle::NotInstalled, &missing, false),
            AppAction::Install
        );
        assert_eq!(
            primary_action(AppLifecycle::NotInstalled, &missing, true),
            AppAction::LeaveStrictLocal
        );
        assert_eq!(
            primary_action(AppLifecycle::NotInstalled, &[], true),
            AppAction::Install,
            "不用下载的安装不受严格本地影响"
        );
        let ready = link_import(Managed, Managed);
        assert_eq!(
            primary_action(AppLifecycle::Installed, &ready, true),
            AppAction::Open
        );
        assert_eq!(
            primary_action(AppLifecycle::Paused, &ready, true),
            AppAction::Enable
        );
        assert_eq!(
            primary_action(AppLifecycle::NeedsRepair, &missing, false),
            AppAction::Repair
        );
        assert_eq!(
            primary_action(AppLifecycle::UpdateAvailable, &ready, true),
            AppAction::LeaveStrictLocal
        );
    }

    #[test]
    fn every_app_has_two_to_four_distinct_highlights() {
        for app in FIRST_PARTY_APPS {
            assert!(
                (2..=4).contains(&app.highlights.len()),
                "{} 的亮点要两到四条（ADR-168 ⑥）",
                app.id
            );
            for (index, highlight) in app.highlights.iter().enumerate() {
                assert!(
                    !app.highlights[..index].contains(highlight),
                    "{} 的亮点 {highlight} 重复",
                    app.id
                );
            }
        }
    }

    #[test]
    fn first_party_license_is_the_workspace_license() {
        assert_eq!(FIRST_PARTY_LICENSE, "AGPL-3.0-only");
    }

    #[test]
    fn enabling_and_disabling_round_trip_in_manifest_order() {
        let off = Settings::default();
        assert_eq!(
            ids_of(&with_enabled(&off, "creator", true)),
            ["creator", "off:teleprompter", "off:link_import"]
        );
        let on = with(&["creator"]);
        assert_eq!(
            ids_of(&with_enabled(&on, "creator", false)),
            ["off:creator", "off:teleprompter", "off:link_import"]
        );
        assert_eq!(
            ids_of(&with_enabled(&on, "bogus", true)),
            ["creator", "off:teleprompter", "off:link_import"],
            "未知 id 写不进去"
        );
    }

    #[test]
    fn creator_owns_its_settings_and_nothing_else() {
        for key in [
            "creator.board_stages",
            "creator.board_view",
            "vault.creator_dir",
            "privacy.web_clip",
        ] {
            let owners: Vec<&str> = setting_owners(key).map(|app| app.id).collect();
            assert_eq!(owners, ["creator"], "{key}");
        }
        let shared: Vec<&str> = setting_owners("teleprompter.font_size")
            .map(|app| app.id)
            .collect();
        assert_eq!(
            shared,
            ["creator", "teleprompter"],
            "提词器偏好两者共有（ADR-176 ③）"
        );
        for key in [
            "workspace.rail_layout",
            "vault.documents_dir",
            "privacy.strict_local",
            APPS_ENABLED_KEY,
            APPS_INSTALLED_KEY,
        ] {
            assert!(setting_owners(key).next().is_none(), "{key} 不归任何应用");
        }
    }

    #[test]
    fn link_import_owns_the_fetch_settings_and_declares_its_components() {
        for key in [
            "transcribe.fetch_media",
            "transcribe.fetch_proxy",
            "transcribe.fetch_tool_path",
        ] {
            let owners: Vec<&str> = setting_owners(key).map(|app| app.id).collect();
            assert_eq!(owners, ["link_import"], "{key}");
        }
        let needs: Vec<(&str, ComponentNeed)> = LINK_IMPORT_APP
            .components
            .iter()
            .map(|component| (component.id, component.need))
            .collect();
        assert_eq!(
            needs,
            [
                ("yt_dlp", ComponentNeed::Required),
                ("js_runtime", ComponentNeed::Recommended),
                ("ffmpeg", ComponentNeed::Optional),
            ]
        );
        assert!(LINK_IMPORT_APP
            .permissions
            .contains(&AppPermission::ExternalPrograms));
    }
}
