//! 主题注册表（ADR-133 / ADR-169 ③ ④）：内置主题与内容层里导入的主题包，可以同时装多套。
//!
//! 内置主题也是主题包（`luna_content_schema::builtin_themes`），与导入的走同一个生成器：随包的
//! 那一份编进前端 CSS，导入的在运行时注入 `css_for` 的输出。注册表按「内置 < 内容层」分层
//! （`luna_content::layered`）列出全部主题；`appearance.theme_family` 的取值是主题 id，它找不到
//! 对应的主题时一律回落内置的 `luna`——判定只在这里（[`resolve_theme`]），首帧外观、稳态外观、
//! 窗口底色与两个主题面板都问它。
//!
//! 导入走内容基座的安装管线（`luna_content::install`，本地导入 = 市场安装去掉下载）：读文件 →
//! 按 `ThemePackage` 解析与对比度校验 → 内容层原子落位 → 选中它。

use super::settings::{set_setting_value, SettingValueDto};
use super::*;
use luna_content::{
    layered, ContentDomain, ContentError, ContentStore, InstallError, Layer, Layered, Prepared,
    Source,
};
use luna_content_schema::{
    builtin_themes, is_builtin_theme, ContentId, PackVersion, RelPath, ThemePackage,
    ThemePackageError, FALLBACK_THEME_ID,
};

/// 设置键。
pub(crate) const THEME_FAMILY_KEY: &str = "appearance.theme_family";

/// 内容层里一份主题的文件名。
const THEME_FILE: &str = "theme.toml";

/// 主题包大小上限：两套颜色加元数据远小于这个数，超了就不是主题包。
const MAX_THEME_BYTES: u64 = 64 * 1024;

/// 内容层的根（应用数据目录下的 `content/`）。
pub(crate) fn content_store(app: &tauri::AppHandle) -> Result<ContentStore, String> {
    app.path()
        .app_data_dir()
        .map(|dir| ContentStore::new(dir.join("content")))
        .map_err(|error| format!("THEME_STORAGE_UNAVAILABLE: {error}"))
}

/// 内容层里全部能读、能通过校验的主题包（读不了、不合格或占了内置 id 的跳过——列出来也用不了）。
fn installed_theme_packages(app: &tauri::AppHandle) -> Vec<ThemePackage> {
    let Ok(store) = content_store(app) else {
        return Vec::new();
    };
    store
        .list(ContentDomain::Themes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let source = std::fs::read_to_string(item.dir.join(THEME_FILE)).ok()?;
            ThemePackage::parse(&source)
                .ok()
                .filter(|package| package.id == item.id && !is_builtin_theme(package.id.as_str()))
        })
        .collect()
}

/// 主题注册表：内置在前、导入的在后（**纯函数**；导入的包由调用方读好交进来）。
pub(crate) fn theme_registry(installed: Vec<ThemePackage>) -> Vec<Layered<ThemePackage>> {
    layered(
        vec![
            (Layer::Builtin, builtin_themes().to_vec()),
            (Layer::Content, installed),
        ],
        |package| package.id.clone(),
    )
}

/// 这台服务端封禁的主题 id 与原因（ADR-159 ⑦；零网络、**不随目录过期失效**，见
/// `CatalogClient::revoked`）。id 写法不合规的那条跳过——它对不上任何已装的主题。
fn theme_bans(app: &tauri::AppHandle) -> Vec<(ContentId, String)> {
    app.try_state::<luna_cloud::commands::Catalogs>()
        .map(|catalogs| catalogs.themes.revoked())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|ban| Some((ContentId::parse(&ban.id).ok()?, ban.reason)))
        .collect()
}

/// 已装的主题按封禁分成「可用」与「已停用（连同原因）」两份（**纯函数**）。停用只是不进注册表：文件不动，
/// 设置里选的 id 也不动——`resolve_theme` 找不到它就回落 `luna`，解除封禁后它自动回来（PRD-18-012）。
fn split_banned(
    installed: Vec<ThemePackage>,
    bans: &[(ContentId, String)],
) -> (Vec<ThemePackage>, Vec<(ThemePackage, String)>) {
    let mut usable = Vec::new();
    let mut disabled = Vec::new();
    for package in installed {
        match bans.iter().find(|(id, _)| *id == package.id) {
            Some((_, reason)) => disabled.push((package, reason.clone())),
            None => usable.push(package),
        }
    }
    (usable, disabled)
}

/// 读内容层、去掉被封禁的、组好注册表。首帧外观、稳态外观、两个主题面板与设置校验都问它，封禁因此只在
/// 这一处生效。
pub(crate) fn load_theme_registry(app: &tauri::AppHandle) -> Vec<Layered<ThemePackage>> {
    theme_registry(split_banned(installed_theme_packages(app), &theme_bans(app)).0)
}

/// 此刻实际用哪套主题：设置里的 id 在注册表里就用它，否则回落 `luna`（**纯函数**）。
pub(crate) fn resolve_theme<'a>(
    selected: &str,
    registry: &'a [Layered<ThemePackage>],
) -> &'a Layered<ThemePackage> {
    let find = |id: &str| registry.iter().find(|entry| entry.item.id.as_str() == id);
    find(selected)
        .or_else(|| find(FALLBACK_THEME_ID))
        .expect("注册表总含内置的回落主题")
}

/// 导入的主题要在运行时另装的样式；内置主题的样式随包，为 `None`。
pub(crate) fn runtime_css(entry: &Layered<ThemePackage>) -> Option<String> {
    (entry.layer != Layer::Builtin).then(|| entry.item.css_for(&entry.item.id))
}

/// 这套主题从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSource {
    /// 随 Lunarca 发布（不能卸载）。
    Builtin,
    /// 从文件导入或市场安装、落在内容层的。
    Content,
}

/// 注册表里的一套主题（两个主题面板的卡片、预览与色块都用它）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ThemeEntryDto {
    pub id: String,
    pub version: String,
    pub name: String,
    pub author: String,
    pub license: String,
    pub source: ThemeSource,
    /// 预览要另装的样式（Rust 从校验过的配色生成）；内置主题的样式随包，为 `None`。
    pub css: Option<String>,
    /// 卡片色块：挂在 `[data-theme-swatch="<id>"]` 上的四个变量。
    pub swatch_css: String,
    /// 已验签的主题目录里有更高的版本、这台客户端也装得上时，那一版的版本号（PRD-18-011：「已安装」据此标出
    /// 「可更新」；用户点了才装）。只有 [`theme_library`] 填，其余出口为 `None`。
    pub update_to: Option<String>,
}

impl ThemeEntryDto {
    /// 按在线目录的状态填 [`Self::update_to`]（**纯函数**）：判定只认 [`market_themes`] 算出的 `UpdateAvailable`，
    /// 不在这里再比一次版本。
    fn with_update(mut self, market: &[MarketThemeDto]) -> Self {
        self.update_to = market
            .iter()
            .find(|theme| theme.id == self.id && theme.state == MarketThemeState::UpdateAvailable)
            .map(|theme| theme.version.clone());
        self
    }
}

impl From<&Layered<ThemePackage>> for ThemeEntryDto {
    fn from(entry: &Layered<ThemePackage>) -> Self {
        let package = &entry.item;
        Self {
            id: package.id.as_str().to_owned(),
            version: package.version.to_string(),
            name: package.name.clone(),
            author: package.author.clone(),
            license: package.license.clone(),
            source: if entry.layer == Layer::Builtin {
                ThemeSource::Builtin
            } else {
                ThemeSource::Content
            },
            css: runtime_css(entry),
            swatch_css: package.swatch_css(),
            update_to: None,
        }
    }
}

/// 因安全原因被服务端封禁、在本机停用的一套主题（PRD-18-012）：只能卸载；解除封禁后自动回到 `themes`。
#[derive(Debug, Clone, Serialize, Type)]
pub struct DisabledThemeDto {
    pub id: String,
    pub version: String,
    pub name: String,
    pub author: String,
    pub license: String,
    /// 服务端写的原因，原样给用户看。
    pub reason: String,
}

/// 可用的主题集合变了，而设置没变（主题目录交付了新的封禁 / 解除了封禁，纵切 7a T3）：外观、两个主题面板与
/// 「获取更多主题」据此重读。设置变了的那些情形（选用、导入、卸载）照旧走 `settings://changed`。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "themes://changed")]
pub struct ThemeLibraryChanged;

/// 主题库：注册表的投影与此刻在用的那一套。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ThemeLibraryDto {
    /// 此刻在用的主题 id（设置里的 id 找不到时已回落 `luna`）。
    pub in_use: String,
    pub themes: Vec<ThemeEntryDto>,
    /// 已停用的（被封禁的已装主题）。
    pub disabled: Vec<DisabledThemeDto>,
    /// 此刻能向在线主题目录「检查更新」（已登录、未开严格本地、云插件已装上）。与「获取更多主题」那一块出不出现
    /// 分开判：目录里的条目全被封禁时那一块消失，而查一遍正是得知解除封禁的途径（2026-10-23 Windows 真机）。
    pub can_check_updates: bool,
}

/// 此刻生效的主题（稳态外观用：前端不自己判断 id 对不对得上）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ThemeAppearanceDto {
    /// 挂到 `data-theme-family` 上的 id（找不到时已回落 `luna`）。
    pub family: String,
    /// 导入的主题要另装的样式；内置主题为 `None`。
    pub css: Option<String>,
}

fn selected_theme(state: &AppState) -> String {
    state
        .settings_snapshot()
        .get_str(THEME_FAMILY_KEY)
        .unwrap_or_else(|| FALLBACK_THEME_ID.to_owned())
}

/// 主题库（设置 › 外观、「应用 › 主题」与「应用 › 已安装」共用）。**零网络**：「可更新」读已验签主题目录的
/// 本地副本，入口不该出现时（严格本地、未登录、没有生效目录）一条也不标。
#[tauri::command]
#[specta::specta]
pub(crate) fn theme_library(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> ThemeLibraryDto {
    let installed = installed_theme_packages(&app);
    let market = available_catalog_themes(&app)
        .map(|entries| market_themes(&entries, &installed, client_version()))
        .unwrap_or_default();
    let (usable, disabled) = split_banned(installed, &theme_bans(&app));
    let registry = theme_registry(usable);
    ThemeLibraryDto {
        in_use: resolve_theme(&selected_theme(&state), &registry)
            .item
            .id
            .as_str()
            .to_owned(),
        themes: registry
            .iter()
            .map(|entry| ThemeEntryDto::from(entry).with_update(&market))
            .collect(),
        disabled: disabled
            .into_iter()
            .map(|(package, reason)| DisabledThemeDto {
                id: package.id.as_str().to_owned(),
                version: package.version.to_string(),
                name: package.name,
                author: package.author,
                license: package.license,
                reason,
            })
            .collect(),
        can_check_updates: super::content_market::online_catalog_usable(&app),
    }
}

/// 此刻生效的主题。
#[tauri::command]
#[specta::specta]
pub(crate) fn appearance_theme(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> ThemeAppearanceDto {
    let registry = load_theme_registry(&app);
    let entry = resolve_theme(&selected_theme(&state), &registry);
    ThemeAppearanceDto {
        family: entry.item.id.as_str().to_owned(),
        css: runtime_css(entry),
    }
}

/// 写主题族之前问注册表：选一套不存在的主题就拒（与选项型设置同一个错误码）。
pub(crate) fn check_theme_family(
    app: &tauri::AppHandle,
    value: &SettingValueDto,
) -> Result<(), String> {
    theme_family_in_registry(&load_theme_registry(app), value)
}

/// [`check_theme_family`] 的判定本身（**纯函数**）。
fn theme_family_in_registry(
    registry: &[Layered<ThemePackage>],
    value: &SettingValueDto,
) -> Result<(), String> {
    let SettingValueDto::Str(id) = value else {
        return Ok(());
    };
    if registry
        .iter()
        .any(|entry| entry.item.id.as_str() == id.trim())
    {
        Ok(())
    } else {
        Err(format!(
            "SETTING_OPTION: {THEME_FAMILY_KEY} {id} is not an installed theme"
        ))
    }
}

/// 导入时的拒绝原因（域内的，管线外层再包一层）。
#[derive(Debug, thiserror::Error)]
enum ThemeImportError {
    #[error(transparent)]
    Invalid(#[from] ThemePackageError),
    #[error("the id {0} belongs to a built-in theme")]
    Reserved(String),
}

/// 按 `ThemePackage` 解析一份导入的文件（**纯函数**：管线把字节交进来）。
fn prepare_theme(bytes: &[u8]) -> Result<Prepared<ThemePackage>, ThemeImportError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| ThemePackageError::Parse("theme package must be UTF-8".to_owned()))?;
    let package = ThemePackage::parse(source)?;
    if is_builtin_theme(package.id.as_str()) {
        return Err(ThemeImportError::Reserved(package.id.as_str().to_owned()));
    }
    Ok(Prepared {
        id: package.id.clone(),
        version: package.version,
        files: vec![(
            RelPath::parse(THEME_FILE).expect("固定的合法路径"),
            bytes.to_vec(),
        )],
        payload: package,
    })
}

/// 管线错误 → 稳定错误码（前端按码取文案）。
fn import_error_code(error: &InstallError<ThemeImportError>) -> String {
    match error {
        InstallError::Invalid(_) | InstallError::TooLarge { .. } | InstallError::Checksum => {
            format!("THEME_IMPORT_REJECTED: {error}")
        }
        InstallError::Store(ContentError::AlreadyInstalled { .. }) => {
            format!("THEME_ALREADY_INSTALLED: {error}")
        }
        InstallError::Read(_) => format!("THEME_IMPORT_READ_FAILED: {error}"),
        InstallError::Store(ContentError::Io(_)) => format!("THEME_IMPORT_WRITE_FAILED: {error}"),
    }
}

/// 写主题族设置并广播（与设置页同一条通路）。
fn select_theme(app: &tauri::AppHandle, state: &AppState, family: &str) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        THEME_FAMILY_KEY,
        SettingValueDto::Str(family.to_owned()),
    )?;
    let _ = SettingsChanged {
        key: THEME_FAMILY_KEY.to_owned(),
    }
    .emit(app);
    Ok(())
}

/// 从文件导入一份主题包并立即选中（ADR-169 ② ④）：与市场安装同一条管线；不合格、比已装的
/// 旧或同版本、写盘失败时当前主题与内容层都不变。用户在选文件时取消返回 `None`。
///
/// # Errors
/// `THEME_IMPORT_REJECTED`（格式、对比度、占用内置 id、过大）、`THEME_ALREADY_INSTALLED`（已装
/// 同一或更新版本）、`THEME_IMPORT_READ_FAILED`、`THEME_IMPORT_WRITE_FAILED`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn install_theme_package(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ThemeEntryDto>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Lunarca Theme", &["toml"])
        .pick_file(move |path| {
            let result = path
                .map(|path| path.into_path().map_err(|error| error.to_string()))
                .transpose();
            let _ = tx.send(result);
        });
    let Some(path) = rx
        .await
        .map_err(|_| "THEME_DIALOG_CANCELLED".to_owned())??
    else {
        return Ok(None);
    };
    let store = content_store(&app)?;
    let (_, package) = luna_content::install(
        &store,
        ContentDomain::Themes,
        Source::LocalFile(&path),
        MAX_THEME_BYTES,
        prepare_theme,
    )
    .map_err(|error| import_error_code(&error))?;
    select_theme(&app, &state, package.id.as_str())?;
    let entry = Layered {
        item: package,
        layer: Layer::Content,
        shadows: Vec::new(),
    };
    Ok(Some((&entry).into()))
}

/// 卸载一份导入的主题（「应用 › 主题」）。正在用它时先切回内置 `luna`、再删——根节点不会有一刻
/// 挂着一个已经没有样式的族。内置主题不能卸载；本来就没装也算成功。
///
/// # Errors
/// id 不合规或是内置主题、切回内置失败（此时文件不动）、删除失败时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) fn uninstall_theme_package(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let id = ContentId::parse(&id).map_err(|error| format!("THEME_UNINSTALL_FAILED: {error}"))?;
    if is_builtin_theme(id.as_str()) {
        return Err("THEME_UNINSTALL_FAILED: built-in themes cannot be uninstalled".to_owned());
    }
    let in_use = state
        .settings_snapshot()
        .get_str(THEME_FAMILY_KEY)
        .is_some_and(|family| family == id.as_str());
    if in_use {
        select_theme(&app, &state, FALLBACK_THEME_ID)?;
    }
    content_store(&app)?
        .remove(ContentDomain::Themes, &id)
        .map_err(|error| format!("THEME_UNINSTALL_FAILED: {error}"))?;
    let _ = SettingsChanged {
        key: THEME_FAMILY_KEY.to_owned(),
    }
    .emit(&app);
    Ok(())
}

// ── 在线主题目录（纵切 7a，ADR-159 ③ ⑤，PRD-18-006 / 007 / 010）────────────────────────────────
//
// 「应用 › 主题」里的「获取更多主题」：条目来自已验签的主题目录（`Catalogs.themes.current()`，零网络读本桶
// 缓存），安装走与本地导入同一条管线。**入口在不可执行时不出现**：严格本地、未登录，或这台服务端没发布
// 主题目录（含缓存已过期）时 [`theme_market`] 返回 `None`；安装命令按同一判据再拦一次，不信前端。

/// 这台客户端的版本：条目的最低客户端版本与它比。
fn client_version() -> PackVersion {
    PackVersion::parse(env!("CARGO_PKG_VERSION")).expect("应用版本是 x.y.z")
}

/// 在线目录里一套主题此刻对这台客户端是什么状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MarketThemeState {
    /// 没装，可以获取。
    Available,
    /// 已装同一或更新的版本。
    Installed,
    /// 已装旧版本，可以更新（用户点了才装，PRD-18-011）。
    UpdateAvailable,
    /// 这台客户端低于条目的最低版本：只说「需要更新 Lunarca」，不给安装（PRD-18-007）。
    NeedsNewerApp,
}

/// 在线目录里的一套主题（卡片、预览与色块）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct MarketThemeDto {
    pub id: String,
    pub version: String,
    pub name: String,
    pub author: String,
    pub license: String,
    pub state: MarketThemeState,
    /// 已装的版本（`UpdateAvailable` 时给出「从哪一版更新」）。
    pub installed_version: Option<String>,
    /// 预览要另装的样式（Rust 从校验过的配色生成）。
    pub css: String,
    /// 卡片色块：挂在 `[data-theme-swatch="<id>"]` 上的四个变量。
    pub swatch_css: String,
}

/// 「获取更多主题」区块的内容。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ThemeMarketDto {
    pub themes: Vec<MarketThemeDto>,
}

/// 一份签名目录里**认得出、校验得过**的条目（**纯函数**）：本版不认识的形状或不合格的包只跳过那一条，
/// 不连累整份目录——与本地模型目录同一口径。
fn catalog_themes(values: Vec<serde_json::Value>) -> Vec<luna_content_schema::ThemeCatalogEntry> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .filter(|entry| luna_content_schema::validate_theme_entry(entry).is_ok())
        .collect()
}

/// 目录条目对照已装的主题，算出每一条的状态（**纯函数**）。
fn market_themes(
    entries: &[luna_content_schema::ThemeCatalogEntry],
    installed: &[ThemePackage],
    client: PackVersion,
) -> Vec<MarketThemeDto> {
    entries
        .iter()
        .map(|entry| {
            let package = &entry.package;
            let local = installed.iter().find(|item| item.id == package.id);
            let state = match local {
                _ if !entry.supports(client) => MarketThemeState::NeedsNewerApp,
                Some(local) if local.version >= package.version => MarketThemeState::Installed,
                Some(_) => MarketThemeState::UpdateAvailable,
                None => MarketThemeState::Available,
            };
            MarketThemeDto {
                id: package.id.as_str().to_owned(),
                version: package.version.to_string(),
                name: package.name.clone(),
                author: package.author.clone(),
                license: package.license.clone(),
                state,
                installed_version: local.map(|local| local.version.to_string()),
                css: package.css_for(&package.id),
                swatch_css: package.swatch_css(),
            }
        })
        .collect()
}

/// 此刻可用的在线主题条目；入口不该出现时为 `None`（严格本地、未登录、没有生效的主题目录）。
fn available_catalog_themes(
    app: &tauri::AppHandle,
) -> Option<Vec<luna_content_schema::ThemeCatalogEntry>> {
    if !super::content_market::online_catalog_usable(app) {
        return None;
    }
    let catalogs = app.try_state::<luna_cloud::commands::Catalogs>()?;
    let bans = theme_bans(app);
    let entries: Vec<_> = catalog_themes(catalogs.themes.current()?.entries)
        .into_iter()
        .filter(|entry| bans.iter().all(|(id, _)| *id != entry.package.id))
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// 「获取更多主题」（**零网络**：读已验签目录的本地副本）。`None` = 这一块不出现。
#[tauri::command]
#[specta::specta]
pub(crate) fn theme_market(app: tauri::AppHandle) -> Option<ThemeMarketDto> {
    let entries = available_catalog_themes(&app)?;
    Some(ThemeMarketDto {
        themes: market_themes(&entries, &installed_theme_packages(&app), client_version()),
    })
}

/// 获取或更新在线目录里的一套主题（PRD-18-008 / 010 / 011）：Rust 按 id 从已验签目录里重新取那一条（不信
/// 前端交来的任何内容），走与本地导入同一条安装管线落进内容层。**第一次获取即选中它**（与从文件导入一致）；
/// 更新不改用户此刻选的是哪一套。
///
/// # Errors
/// `THEME_MARKET_UNAVAILABLE`（严格本地 / 未登录 / 没有生效的主题目录）、`THEME_MARKET_NOT_FOUND`、
/// `THEME_MARKET_NEEDS_UPDATE`（客户端低于最低版本），以及安装管线的 `THEME_IMPORT_REJECTED` /
/// `THEME_ALREADY_INSTALLED` / `THEME_IMPORT_WRITE_FAILED`。
#[tauri::command]
#[specta::specta]
pub(crate) fn install_market_theme(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ThemeEntryDto, String> {
    let entries =
        available_catalog_themes(&app).ok_or_else(|| "THEME_MARKET_UNAVAILABLE".to_owned())?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.package.id.as_str() == id)
        .ok_or_else(|| format!("THEME_MARKET_NOT_FOUND: {id}"))?;
    if !entry.supports(client_version()) {
        return Err(format!(
            "THEME_MARKET_NEEDS_UPDATE: requires {}",
            entry.min_client_version
        ));
    }
    let first_install = !installed_theme_packages(&app)
        .iter()
        .any(|package| package.id == entry.package.id);
    let store = content_store(&app)?;
    let (_, package) = luna_content::install(
        &store,
        ContentDomain::Themes,
        Source::Signed(entry.package.to_toml().into_bytes()),
        MAX_THEME_BYTES,
        prepare_theme,
    )
    .map_err(|error| import_error_code(&error))?;
    if first_install {
        select_theme(&app, &state, package.id.as_str())?;
    } else {
        // 更新不改选中的那一套，但两个主题面板与在用主题的样式都要重读（版本与配色变了）。
        let _ = SettingsChanged {
            key: THEME_FAMILY_KEY.to_owned(),
        }
        .emit(&app);
    }
    let entry = Layered {
        item: package,
        layer: Layer::Content,
        shadows: Vec::new(),
    };
    Ok((&entry).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_source(id: &str, version: &str) -> String {
        format!(
            r##"
schema_version = 3
id = "{id}"
version = "{version}"
name = "海湾"
author = "Ada"
license = "CC0-1.0"

[light]
canvas = "#ffffff"
surface = "#ffffff"
panel = "#ffffff"
text_primary = "#111111"
text_secondary = "#333333"
text_muted = "#555555"
border_subtle = "#777777"
border_default = "#666666"
border_strong = "#555555"
accent = "#0055aa"
accent_strong = "#003d7a"
accent_text = "#ffffff"
ai = "#6b258e"
attention = "#765b00"
success = "#08752f"
warning = "#984900"
error = "#b00020"

[dark]
canvas = "#111111"
surface = "#181818"
panel = "#202020"
text_primary = "#ffffff"
text_secondary = "#dddddd"
text_muted = "#bbbbbb"
border_subtle = "#888888"
border_default = "#999999"
border_strong = "#aaaaaa"
accent = "#72b8ff"
accent_strong = "#a4d1ff"
accent_text = "#111111"
ai = "#d7a5ef"
attention = "#f0d56d"
success = "#70dc91"
warning = "#ffad70"
error = "#ff8795"
"##
        )
    }

    #[test]
    fn the_registry_lists_builtins_first_and_resolves_unknown_ids_to_luna() {
        let bay = ThemePackage::parse(&package_source("bay", "1.0.0")).expect("valid");
        let registry = theme_registry(vec![bay]);
        let ids: Vec<_> = registry
            .iter()
            .map(|entry| entry.item.id.as_str())
            .collect();
        assert_eq!(ids, ["luna", "nebula", "bay"]);
        assert_eq!(
            resolve_theme("nebula", &registry).item.id.as_str(),
            "nebula"
        );
        let bay = resolve_theme("bay", &registry);
        assert_eq!(bay.layer, Layer::Content);
        assert!(runtime_css(bay).is_some_and(|css| css.contains("[data-theme-family=\"bay\"]")));
        assert!(
            runtime_css(resolve_theme("nebula", &registry)).is_none(),
            "内置主题的样式随包"
        );
        assert_eq!(resolve_theme("gone", &registry).item.id.as_str(), "luna");
        assert_eq!(
            resolve_theme("installed", &registry).item.id.as_str(),
            "luna",
            "旧的单槽取值已退役"
        );
        assert!(theme_family_in_registry(&registry, &SettingValueDto::Str("bay".into())).is_ok());
        assert!(
            theme_family_in_registry(&registry, &SettingValueDto::Str("gone".into()))
                .is_err_and(|error| error.starts_with("SETTING_OPTION"))
        );
        let dto = ThemeEntryDto::from(&registry[1]);
        assert_eq!(dto.source, ThemeSource::Builtin);
        assert!(dto.swatch_css.contains("[data-theme-swatch=\"nebula\"]"));
    }

    /// 随包样式表是生成物：主题包改了而没重新生成就红（`cargo run -p luna-app --bin export-bindings`）。
    #[test]
    fn the_bundled_theme_stylesheet_matches_the_builtin_packages() {
        let bundled = std::fs::read_to_string(crate::THEMES_CSS_PATH).expect("随包样式表");
        assert!(
            bundled == luna_content_schema::builtin_stylesheet(),
            "themes.generated.css 与内置主题包不一致：运行 cargo run -p luna-app --bin export-bindings"
        );
    }

    #[test]
    fn importing_refuses_builtin_ids_and_maps_store_outcomes_to_codes() {
        assert!(matches!(
            prepare_theme(package_source("luna", "1.0.0").as_bytes()),
            Err(ThemeImportError::Reserved(_))
        ));
        assert!(prepare_theme(b"\xff\xfe").is_err(), "不是 UTF-8 也只是拒绝");

        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        let file = root.path().join("bay.toml");
        std::fs::write(&file, package_source("bay", "1.0.0")).unwrap();
        let install = |path: &std::path::Path| {
            luna_content::install(
                &store,
                ContentDomain::Themes,
                Source::LocalFile(path),
                MAX_THEME_BYTES,
                prepare_theme,
            )
        };
        install(&file).expect("first import");
        let again = install(&file).expect_err("same version");
        assert!(import_error_code(&again).starts_with("THEME_ALREADY_INSTALLED"));
        std::fs::write(&file, package_source("bay", "1.1.0")).unwrap();
        install(&file).expect("newer version updates");
        std::fs::write(
            &file,
            package_source("bay", "1.2.0").replace("#111111", "#eeeeee"),
        )
        .unwrap();
        let rejected = install(&file).expect_err("low contrast");
        assert!(import_error_code(&rejected).starts_with("THEME_IMPORT_REJECTED"));
        let listed = store.list(ContentDomain::Themes).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].version.to_string(), "1.1.0", "被拒的那一版不落盘");
    }

    fn catalog_entry(id: &str, version: &str, min_client: &str) -> serde_json::Value {
        serde_json::json!({
            "package": ThemePackage::parse(&package_source(id, version)).expect("valid"),
            "min_client_version": min_client,
        })
    }

    /// 目录里认不出的形状、不合格的包、占了内置 id 的条目都**只跳过那一条**——与本地模型目录同一口径：
    /// 一条坏的不该让整个「获取更多主题」消失。
    #[test]
    fn catalog_themes_skip_only_the_entries_this_client_cannot_use() {
        let mut broken = catalog_entry("com.example.broken", "1.0.0", "1.0.0");
        broken["package"]["light"]["canvas"] = serde_json::json!("red");
        let values = vec![
            catalog_entry("com.example.dusk", "1.0.0", "1.0.0"),
            serde_json::json!({ "package": {}, "min_client_version": "1.0.0", "future": true }),
            broken,
            catalog_entry("nebula", "1.0.0", "1.0.0"),
        ];
        let entries = catalog_themes(values);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].package.id.as_str(), "com.example.dusk");
    }

    #[test]
    fn market_states_follow_the_installed_version_and_the_client_version() {
        let entries = catalog_themes(vec![
            catalog_entry("com.example.fresh", "1.0.0", "1.0.0"),
            catalog_entry("com.example.same", "1.0.0", "1.0.0"),
            catalog_entry("com.example.older", "1.2.0", "1.0.0"),
            catalog_entry("com.example.future", "1.0.0", "9.0.0"),
        ]);
        let installed = vec![
            ThemePackage::parse(&package_source("com.example.same", "1.0.0")).unwrap(),
            ThemePackage::parse(&package_source("com.example.older", "1.1.0")).unwrap(),
        ];
        let themes = market_themes(&entries, &installed, PackVersion::parse("1.1.0").unwrap());
        let states: Vec<_> = themes
            .iter()
            .map(|theme| (theme.id.as_str(), theme.state))
            .collect();
        assert_eq!(
            states,
            [
                ("com.example.fresh", MarketThemeState::Available),
                ("com.example.same", MarketThemeState::Installed),
                ("com.example.older", MarketThemeState::UpdateAvailable),
                ("com.example.future", MarketThemeState::NeedsNewerApp),
            ]
        );
        assert_eq!(themes[2].installed_version.as_deref(), Some("1.1.0"));
        assert!(
            themes[0].css.contains("com.example.fresh"),
            "预览样式按主题 id 限定作用域"
        );

        // 主题库只给「可更新」的那条标出目标版本：已是最新、装不上的、没装的都不标。
        let library: Vec<_> = installed
            .iter()
            .chain(std::iter::once(
                &ThemePackage::parse(&package_source("com.example.future", "0.9.0")).unwrap(),
            ))
            .map(|package| {
                let entry = Layered {
                    item: package.clone(),
                    layer: Layer::Content,
                    shadows: Vec::new(),
                };
                let dto = ThemeEntryDto::from(&entry).with_update(&themes);
                (dto.id, dto.update_to)
            })
            .collect();
        assert_eq!(
            library,
            [
                ("com.example.same".to_owned(), None),
                ("com.example.older".to_owned(), Some("1.2.0".to_owned())),
                ("com.example.future".to_owned(), None),
            ]
        );
    }

    /// 内联在目录里的主题包经 `to_toml` 进安装管线，落盘的就是本地导入同一种文件：读回来与目录里签过的
    /// 那一套逐字段相同，同 id 再装同版本照样拒（更新必须版本更高）。
    #[test]
    fn a_signed_catalog_theme_installs_through_the_same_pipeline_as_a_file() {
        let dir = std::env::temp_dir().join(format!("luna-market-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ContentStore::new(&dir);
        let entry = &catalog_themes(vec![catalog_entry("com.example.dusk", "1.0.0", "1.0.0")])[0];
        let install = || {
            luna_content::install(
                &store,
                ContentDomain::Themes,
                Source::Signed(entry.package.to_toml().into_bytes()),
                MAX_THEME_BYTES,
                prepare_theme,
            )
        };
        let (item, package) = install().expect("installs");
        assert_eq!(package, entry.package);
        let on_disk = std::fs::read_to_string(item.dir.join(THEME_FILE)).unwrap();
        assert_eq!(ThemePackage::parse(&on_disk).unwrap(), entry.package);
        let again = install().unwrap_err();
        assert!(import_error_code(&again).starts_with("THEME_ALREADY_INSTALLED"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 封禁只是不进注册表：在用的那套被封，`resolve_theme` 回落 `luna`；解除封禁后它自动回来，设置一字不改。
    #[test]
    fn a_banned_theme_leaves_the_registry_falls_back_to_luna_and_returns_when_unbanned() {
        let bay = ThemePackage::parse(&package_source("bay", "1.0.0")).unwrap();
        let bans = vec![(
            ContentId::parse("bay").unwrap(),
            "misleading name".to_owned(),
        )];
        let (usable, disabled) = split_banned(vec![bay.clone()], &bans);
        assert!(usable.is_empty());
        assert_eq!(disabled, vec![(bay.clone(), "misleading name".to_owned())]);
        let registry = theme_registry(usable);
        assert_eq!(
            resolve_theme("bay", &registry).item.id.as_str(),
            FALLBACK_THEME_ID
        );

        let (usable, disabled) = split_banned(vec![bay], &[]);
        assert!(disabled.is_empty());
        assert_eq!(
            resolve_theme("bay", &theme_registry(usable))
                .item
                .id
                .as_str(),
            "bay"
        );
    }
}
