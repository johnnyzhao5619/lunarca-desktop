//! 库（Vault）管理命令域（PRD-06/PRD-07，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：库的选择、创建、注册表增删改、切库前置检查与**切库本身**——
//! 包括交还旧库资源、关闭笔记窗、绑定新库与后台重扫。单库读写在 [`super::notes`]。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 默认建议的 Vault 位置（PRD-14-040：给出默认建议而非空输入框）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DefaultVaultPathDto {
    pub path: String,
    /// 「文档」在 OneDrive 里，所以建议放到了用户目录下（首启与新建库据此说明原因）。
    pub avoided_onedrive: bool,
}

/// 默认建议的 Vault 位置：文档目录下 `LunaVault`；文档目录在 OneDrive 里时改到用户目录下
/// `Lunarca`；文档目录不可得时退家目录（判定见 [`crate::vault_location`]）。仅是建议值，不创建目录。
#[tauri::command]
#[specta::specta]
pub(crate) fn default_vault_path(app: tauri::AppHandle) -> Result<DefaultVaultPathDto, String> {
    let documents = app.path().document_dir().ok();
    let home = app.path().home_dir();
    let location = crate::vault_location::default_vault_location(
        documents.as_deref(),
        home.as_deref().ok(),
        &crate::vault_location::onedrive_roots(),
    );
    // 两个目录都取不到时才没有默认位置，此时如实报用户目录的错。
    let location = location.ok_or_else(|| {
        home.as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
    })?;
    Ok(DefaultVaultPathDto {
        path: location.path.to_string_lossy().into_owned(),
        avoided_onedrive: location.avoided_onedrive,
    })
}

/// 这个位置在 OneDrive 的同步文件夹里：用户自己选的库位置据此提示「可能与 OneDrive 双重同步
/// 冲突」，不拦。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_path_in_onedrive(path: String) -> bool {
    let path = path.trim();
    !path.is_empty()
        && crate::vault_location::inside_onedrive(
            Path::new(path),
            &crate::vault_location::onedrive_roots(),
        )
}

/// 当前 Vault 状态（前端据此决定显示工作区还是“去选择 Vault”入口）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct VaultStatusDto {
    /// 是否已打开 Vault。
    pub open: bool,
    /// 已打开 Vault 的根路径（未打开为 `None`）。
    pub root: Option<String>,
    /// 未打开时：启动配置指着、却找不到的库根（PRD-14-062）。`None` = 从未选过库。
    pub missing_root: Option<String>,
    /// `missing_root` 不是找不到了，是库的 `.luna/` 还在 iCloud 上没取回：空状态说「还在下载」、给「重试」。
    pub missing_in_cloud: bool,
}

/// 后台扫描进度（`scan://progress`）。高频但低体量；done=true 为终态一帧。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "scan://progress")]
pub struct ScanProgress {
    /// 这次已处理的篇数（要重读的与要删掉的；没变的不算）。
    pub indexed: u32,
    /// 这次要处理的总篇数（增量扫描只数变了的，没变的一篇不算）。
    pub total: u32,
    /// 当前处理的相对路径（done 帧为 None）。
    pub current_path: Option<String>,
    /// 是否扫描完成。
    pub done: bool,
    /// 终态是被取消（或被切库打断）而不是做完。
    pub cancelled: bool,
}

/// 索引现状（PRD-06-004 ④）：搜索、快速切换器等据此在扫描未完成时说「索引中，结果可能
/// 不全」，不拿空结果冒充「没有」。启动时前端可能错过前几帧事件，故另有查询命令。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub(crate) struct IndexStatusDto {
    pub scanning: bool,
    pub indexed: u32,
    pub total: u32,
}

/// 后台扫描的控制面：取消标志、本次序号（新一轮扫描会让旧的那轮自行停下）与现状。
#[derive(Default)]
pub(crate) struct ScanControl {
    cancel: std::sync::atomic::AtomicBool,
    seq: std::sync::atomic::AtomicU64,
    status: Mutex<IndexStatusDto>,
}

impl ScanControl {
    fn set(&self, status: IndexStatusDto) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
    }

    pub(crate) fn snapshot(&self) -> IndexStatusDto {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// LunaManaged 模型下载进度。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "model://download-progress")]
pub struct ModelDownloadProgress {
    /// 模型 id。
    pub model_id: String,
    /// 已下载字节数。
    pub downloaded_bytes: f64,
    /// 总字节数。
    pub total_bytes: f64,
    /// 是否已完成。
    pub done: bool,
    /// 这一次取字节被用户暂停（PRD-14-020）：已下载的字节留在断点上，再点「下载」即续。
    /// `true` 时 `done` 恒为 `false`，前端据此收起进度条而不视为安装完成。
    pub paused: bool,
}

/// 读取当前 Vault 状态。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_status(state: tauri::State<'_, AppState>) -> VaultStatusDto {
    vault_status_of(&state)
}

/// 命令之外的取用口（启动快照 `app_bootstrap` 与命令同一份判定）。
pub(crate) fn vault_status_of(state: &AppState) -> VaultStatusDto {
    match state.vault.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(vault) => VaultStatusDto {
                open: true,
                root: Some(vault.root().to_string_lossy().into_owned()),
                missing_root: None,
                missing_in_cloud: false,
            },
            None => VaultStatusDto {
                open: false,
                root: None,
                // 换账户交出了库（启动指针已清空）：那是「这个账户没有打开的库」，不是「上次的库
                // 找不到了」。
                missing_root: state
                    .startup_vault_missing()
                    .filter(|_| {
                        state
                            .settings_snapshot()
                            .get_str("storage.vault_root")
                            .is_some_and(|root| !root.trim().is_empty())
                    })
                    .map(|p| p.to_string_lossy().into_owned()),
                missing_in_cloud: state.startup_vault_in_cloud(),
            },
        },
        Err(_) => VaultStatusDto {
            open: false,
            root: None,
            missing_root: None,
            missing_in_cloud: false,
        },
    }
}

/// 选择一个已有文件夹作为 Vault 并打开（不创建目录）。
///
/// 已开着一个库时这就是一次**切库**（管理库弹窗的「打开文件夹为库」走的正是这里），
/// 必须过同一张交接表——绕过 `release_current_vault` 直接换根，等于无护栏切库：
/// 代际不增（在途蒸馏写进新库）、监听不停、笔记窗不关、令牌不换。
#[tauri::command]
#[specta::specta]
pub(crate) fn select_vault(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let root = resolve_vault_root(state.copy(), PathBuf::from(path), false)?;
    reject_nested_vault(&state, &root)?;
    handover_and_bind(&app, &state, root, false)
}

/// 无 Vault 可写时的错误措辞（**唯一措辞点**，PRD-06-049 边界⑤）：切库过渡期说
/// 「正在切换」（稍候即好），其余才是「尚未选择」（要去引导/设置）——两种状态给用户的
/// 行动指引完全不同，混为一句就是误导。
pub(crate) fn vault_absent_message(i18n: BackendCopy<'_>, switching: bool) -> String {
    if switching {
        coded_error_with(i18n, "VAULT_SWITCHING", "vault-switching")
    } else {
        coded_error_with(i18n, "VAULT_NOT_OPEN", "vault-not-open")
    }
}

/// 切库过渡态的 RAII 标记（PRD-06-049 边界⑤）：构造置位、析构复位——settle 超时、
/// 交接中途出错、bind 失败回滚，任何一条退出路径都不许把「切换中」漏在真值上，
/// 否则此后每个 `vault_not_open` 都在撒谎（"稍候再试"却永远好不了）。
pub(crate) struct VaultSwitchingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl<'a> VaultSwitchingGuard<'a> {
    pub(crate) fn arm(flag: &'a std::sync::atomic::AtomicBool) -> Self {
        flag.store(true, std::sync::atomic::Ordering::Release);
        Self(flag)
    }
}

impl Drop for VaultSwitchingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// 交接 + 装载 + 失败回滚（`select_vault` / `create_vault_root` / `switch_vault` 三处共用）：
/// 当前没开库（首启引导）= 无事可交接直接装载；已开库且目标就是当前库 = 幂等重装；
/// 否则录音护栏 + 完整交接表 + 过渡态标记。
///
/// `bind_vault` 失败发生在旧库**已释放之后**（打开新库权限不足等）——不回滚就停在
/// "两头都没有库"的状态；尽力装回旧库，把原始错误如实上抛（顺序契约：任何一步失败
/// 都停在旧库完好态，TDD-06 §12）。回滚自身失败（旧库同时被卸载等极端情形）时不掩盖
/// 首因，仍报原始错误——此时注册表完好，用户可从库选择重新落脚。
fn handover_and_bind(
    app: &tauri::AppHandle,
    state: &AppState,
    root: PathBuf,
    create: bool,
) -> Result<(), String> {
    let root = if create {
        root
    } else {
        std::fs::canonicalize(&root)
            .map_err(|error| vault_missing_open_failed(state.copy(), &error.to_string()))?
    };
    // 先于交出当前库判：目标库的 `.luna/` 还在 iCloud 上没取回时，下面在主线程上读设置、开索引会
    // 一直卡到取回为止。等几秒（后台照常取回），还没回来就让用户稍后重试，当前库不动。
    if !create
        && !luna_vault::icloud::wait_until_local(
            &root.join(super::LUNA_VAULT_PRIVATE_DIR),
            super::VAULT_CLOUD_FETCH_WAIT,
        )
    {
        return Err(coded_error_args_with(
            state.copy(),
            "VAULT_IN_CLOUD",
            "vault-in-cloud",
            &[("path", root.to_string_lossy().into_owned())],
        ));
    }
    let previous = current_vault_root(state)?.filter(|current| current.as_path() != root);
    if previous.is_some() {
        // 录音进行中一律阻止（数据事故，不是可确认的代价）。
        if recording_blocks_switch(state) {
            return Err(coded_error(
                app,
                "VAULT_SWITCH_RECORDING",
                "vault-switch-recording",
            ));
        }
    }
    // 目标库若开着副库窗口，先关它（PRD-06-050）：同一库同时活在主窗与副库窗
    // = 两套写守卫与索引句柄互踩；主窗口接管后那扇窗的存在意义也消失了。
    if let Some(label) = state.secondary_window_for_root(&root) {
        super::workspace::close_vault_window(app, state, &label);
    }
    // 过渡态标记只在真正的切库上臂（首启引导的首次装载不是"切换"）；RAII 保证
    // 下面任何一条 `?`/回滚路径都会复位。
    let _switching = previous
        .is_some()
        .then(|| VaultSwitchingGuard::arm(&state.vault_switching));
    if previous.is_some() {
        hand_over_current(app, state)?;
    }
    match bind_vault(app, state, root, create) {
        Ok(()) => {
            schedule_local_knowledge_metrics_reconcile(app);
            // 有没有一个能写的库，是三个写作入口能不能用的前提（TDD-07 §2.2）——装载
            // 成功即刷新快照，否则「添加内容」会一直灰着直到用户手动重开一次。
            super::create_actions::publish_create_action_states(app);
            if previous.is_some() {
                // 真切库才广播：独立 webview（快捷对话窗等）据此向 Rust 真值重新对齐。
                let _ = (VaultSwitched {}).emit(app);
            }
            Ok(())
        }
        Err(err) => {
            if let Some(prev) = previous {
                let _ = bind_vault(app, state, prev, false);
            }
            Err(err)
        }
    }
}

/// 交出当前库（切库的前半段；切库与「换了账户、回到空状态」共用）：取消待落地的语音会话并清
/// 预设（TDD-07 §2.3）——那份结果的落点是"存为速记"，而速记要落进**哪个库**已经变了，留着它就是
/// 让一次采集的产物落进用户没打算放它的库里；再按交接表收尾。录音护栏由调用方先判。
fn hand_over_current(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    state.voice_input.clear_session_preset();
    let _ = state
        .voice_input
        .cancel_holder(crate::arbiter::MicHolder::VoiceInput);
    release_current_vault(app, state)
}

/// 换了账户：切到新账户上次打开的库（[`crate::vault_lists`]）。走与切换器同一条交接 + 装载，
/// 护栏与回滚照旧；成功后让主窗口工作区整体重挂载。
pub(crate) fn open_for_account(
    app: &tauri::AppHandle,
    state: &AppState,
    root: PathBuf,
) -> Result<(), String> {
    handover_and_bind(app, state, root, false)?;
    let _ = crate::vault_lists::VaultFollowedAccount {}.emit(app);
    Ok(())
}

/// 换了账户、此刻打开的库新账户看不见、它也没有可去的库：交出当前库，回到空状态（新建 / 打开
/// 文件夹 / 从云端库下载）。录音中一律不交（与切库同一护栏）。启动指针清空：下次启动同样停在
/// 空状态，而不是去开一个这个账户看不见的库。
pub(crate) fn close_for_account(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    if current_vault_root(state)?.is_none() {
        return Ok(());
    }
    if recording_blocks_switch(state) {
        return Err(coded_error(
            app,
            "VAULT_SWITCH_RECORDING",
            "vault-switch-recording",
        ));
    }
    {
        let _switching = VaultSwitchingGuard::arm(&state.vault_switching);
        hand_over_current(app, state)?;
    }
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "storage.vault_root",
        SettingValueDto::Str(String::new()),
    )?;
    super::create_actions::publish_create_action_states(app);
    let _ = (VaultSwitched {}).emit(app);
    let _ = crate::vault_lists::VaultFollowedAccount {}.emit(app);
    Ok(())
}

/// 拒绝与已知库**重叠**的注册（PRD-06-049 边界②）：两库的文件监听与派生索引会重叠——
/// 子库的笔记被父库当普通笔记索引，父库一次「重建索引」还会连子库一起扫。
/// 判据单一（`vault_registry::paths_overlap`，按路径段比较），前端不自行判断。
fn reject_nested_vault(state: &AppState, root: &Path) -> Result<(), String> {
    let known = state.vault_registry.list();
    if let Some(existing) = known
        .iter()
        .find(|e| e.path != root && luna_vault::registry::paths_overlap(&e.path, root))
    {
        return Err(coded_error_args_with(
            state.copy(),
            "VAULT_NESTED",
            "vault-nested",
            &[("name", existing.name.clone())],
        ));
    }
    Ok(())
}

/// `VAULT_PATH_INVALID` 的唯一措辞点（本文件里它出现五次，各拼一遍就会分叉）。
fn vault_path_invalid(i18n: BackendCopy<'_>, detail: &str) -> String {
    coded_error_args_with(
        i18n,
        "VAULT_PATH_INVALID",
        "vault-path-invalid",
        &[("detail", detail.to_owned())],
    )
}

/// 同上，`VAULT_MISSING` 的「打不开」那一支。
fn vault_missing_open_failed(i18n: BackendCopy<'_>, detail: &str) -> String {
    coded_error_args_with(
        i18n,
        "VAULT_MISSING",
        "vault-missing-open-failed",
        &[("error", detail.to_owned())],
    )
}

/// 把用户选择的路径解析为一个稳定的 Vault 身份。
///
/// 已有目录直接 canonicalize；新建目录则 canonicalize 最近的已存在祖先，再接回尚未创建的
/// 后缀。这样重叠校验、库注册表、启动指针与 [`Vault`] 从入口起就使用同一个真实根，软链
/// 别名不能绕过“同库/父子库”判断。这里只解析，不创建目录。
fn resolve_vault_root(
    i18n: BackendCopy<'_>,
    root: PathBuf,
    create: bool,
) -> Result<PathBuf, String> {
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|error| vault_path_invalid(i18n, &error.to_string()))?
            .join(root)
    };
    if !create || root.exists() {
        return std::fs::canonicalize(&root)
            .map_err(|error| vault_missing_open_failed(i18n, &error.to_string()));
    }
    if root
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        // 对尚不存在的目标，`missing/../target` 无法通过 canonicalize 得到真实语义；若把
        // `..` 当普通后缀接回，重叠校验看到的路径会与 create_dir_all 实际创建的路径分叉。
        return Err(coded_error_with(
            i18n,
            "VAULT_PATH_INVALID",
            "vault-path-invalid-parent-component",
        ));
    }

    let mut ancestor = root.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| vault_path_invalid(i18n, &root.display().to_string()))?;
        suffix.push(name.to_owned());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| vault_path_invalid(i18n, &root.display().to_string()))?;
    }
    let mut resolved = std::fs::canonicalize(ancestor)
        .map_err(|error| vault_path_invalid(i18n, &error.to_string()))?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// 在给定路径创建并打开一个新的 Vault 根。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_vault_root(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let root = resolve_vault_root(state.copy(), PathBuf::from(path), true)?;
    reject_nested_vault(&state, &root)?;
    // 已开库时「新建库」同样是切库（管理库弹窗入口），过同一张交接表（见 `select_vault`）。
    handover_and_bind(&app, &state, root, true)
}

/// 打开原生文件夹选择器，返回用户选择的本地目录；取消时返回 `None`。
///
/// 首启引导选 Vault、设置页的目录类项（Vault 位置 / 模型存储目录）共用——目录选择本身
/// 与 Vault 无关，故不叫 `pick_vault_folder`（那会诱使为「模型目录」再造一个同样的命令）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_folder(
    app: tauri::AppHandle,
    title: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    // 选的是什么、选了之后会发生什么，写在对话框标题上（例如下载云端库时「会在其中新建同名文件夹」）。
    if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
        dialog = dialog.set_title(title);
    }
    dialog.pick_folder(move |path| {
        let result = path
            .map(|path| {
                path.into_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))?
}

/// 打开原生**文件**选择器，返回用户选择的本地文件；取消时返回 `None`。
///
/// 服务设置注册表里 `PathKind::AbsoluteFile` 的项（如外部抓取工具可执行文件）——目录选择器
/// 里选不中一个可执行文件，故与 [`pick_folder`] 分立而非加参数：两者的原生对话框不同类。
/// 不加扩展名过滤：可执行文件在 macOS/Linux 上通常无扩展名。
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |path| {
        let result = path
            .map(|path| {
                path.into_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))?
}

// ── 多库：注册表 / 切换 / 管理（PRD-06-049 / PRD-07-064 / ADR-025）─────────────

/// 一条已知库（前端清单与切换器投影）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultEntryDto {
    pub path: String,
    /// 显示名（清单别名，改名不动目录名）。
    pub name: String,
    /// 最后打开时间（Unix 毫秒；`0` = 从未打开）。TS `number`。
    pub last_opened_ms: f64,
    /// 是否为当前活跃库。
    pub active: bool,
    /// 路径当前是否仍存在——外部删除/改名/卸载磁盘后如实标记，界面不假装它还在。
    pub missing: bool,
    /// 切换菜单里归哪一节。
    pub section: VaultSectionDto,
}

/// 库在切换菜单里归哪一节（ADR-151 修订 2026-10-15「续二」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VaultSectionDto {
    /// 本机库：不属于任何账户，谁都看得见。
    Device,
    /// 列表账户的库（同步着的、连在另一个账户下又被重新加回来的）。
    Account,
}

/// 库清单（切换菜单与「管理库」共用一份订阅）：列表账户看得见的库，按节归好。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultListDto {
    /// 列表账户的打码名（「<账户> 的库」一节的标题）：已登录 = 登录的账户；退出后 = 退出前那一个；
    /// 从没登录过 = `None`（只有本机库）。
    pub account: Option<String>,
    /// 列出「云端库（不在这台设备上）」一节：已登录、不在严格本地、同步服务在。
    pub cloud: bool,
    pub vaults: Vec<VaultEntryDto>,
}

/// 切库前的预检：**代价与阻断都在点之前给**（与批量整理确认卡同一心智）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultSwitchPreflightDto {
    /// 录音进行中——**阻止切换**（正往旧库写盘，静默换根是数据事故）。
    pub recording_active: bool,
    /// 处理队列中的活动项数（切换后暂停、回到本库自动续跑）。
    pub pending_tasks: u32,
    /// 目标库路径已不存在。
    pub target_missing: bool,
}

fn vault_entry_dto(
    entry: &luna_vault::registry::VaultEntry,
    active_root: Option<&Path>,
) -> VaultEntryDto {
    VaultEntryDto {
        active: active_root == Some(entry.path.as_path()),
        missing: !entry.path.is_dir(),
        path: entry.path.to_string_lossy().into_owned(),
        name: entry.name.clone(),
        last_opened_ms: entry.last_opened_ms as f64,
        section: if entry.is_device() {
            VaultSectionDto::Device
        } else {
            VaultSectionDto::Account
        },
    }
}

/// 列表账户看得见的库；此刻打开的库即使看不见也列上（换账户时录音中没能切走——它确实开着，
/// 切换菜单不能连当前库都不认）。
fn list_of(
    entries: &[luna_vault::registry::VaultEntry],
    account: Option<&str>,
    active: Option<&Path>,
) -> Vec<VaultEntryDto> {
    entries
        .iter()
        .filter(|e| e.visible_to(account) || active == Some(e.path.as_path()))
        .map(|e| vault_entry_dto(e, active))
        .collect()
}

/// 库清单（切换器与管理库共用一份订阅；含当前库标记、失效标记与分节）。注册表每改写一次发一次
/// `vaults://changed`，界面据此重取。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_vaults(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<VaultListDto, String> {
    let active = current_vault_root(&state)?;
    let list_account = state.vault_registry.list_account();
    let cloud = crate::vault_lists::signed_in_key(&app).is_some()
        && !state.strict_local()
        && app
            .try_state::<Arc<crate::sync_service::SyncService>>()
            .is_some();
    Ok(VaultListDto {
        vaults: list_of(
            &state.vault_registry.list(),
            list_account.as_ref().map(|a| a.key.as_str()),
            active.as_deref(),
        ),
        account: list_account.map(|a| account_title(&app, a)),
        cloud,
    })
}

/// 列表那一节的账户名：列表正是此刻登录的账户 → 完整邮箱（现取档案，不读存下来的标签）；
/// 否则（退出后仍显示上一个账户的列表）→ 存下来的打码标签（用户裁决 2026-10-15）。
fn account_title(app: &tauri::AppHandle, list: luna_vault::registry::ListAccount) -> String {
    title_for(list, crate::account_scope::current_email(app))
}

/// [`account_title`] 的判定本体（纯函数，可单测）：`current` = 此刻登录账户的（账户键，完整邮箱）。
fn title_for(list: luna_vault::registry::ListAccount, current: Option<(String, String)>) -> String {
    match current {
        Some((key, email)) if key == list.key => email,
        _ => list.label,
    }
}

/// 当前活跃库根（未绑定库时 `None`）。
pub(crate) fn current_vault_root(state: &AppState) -> Result<Option<PathBuf>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    Ok(guard.as_ref().map(|v| v.root().to_path_buf()))
}

/// 移除之前：移除会不会顺带断开它在这台设备上的同步绑定（确认框据此说清；零网络）。
#[tauri::command]
#[specta::specta]
pub(crate) fn forget_vault_preflight(
    app: tauri::AppHandle,
    path: String,
) -> Result<crate::sync_service::VaultSyncLinkDto, String> {
    let state = app.state::<AppState>();
    let entry = state
        .known_vaults()
        .into_iter()
        .find(|e| e.path == Path::new(&path));
    let (Some(entry), Ok(config_dir)) = (entry, app.path().app_config_dir()) else {
        return Ok(crate::sync_service::VaultSyncLinkDto::None);
    };
    if !forget_releases(&state, &config_dir, &entry) {
        return Ok(crate::sync_service::VaultSyncLinkDto::None);
    }
    Ok(crate::sync_service::sync_link_of(
        &config_dir,
        &entry,
        crate::account_scope::current_in(&app).as_ref(),
    ))
}

/// 移除这一条会不会断开它的同步绑定（预检与移除同一判据，[`crate::vault_lists::forget_releases_binding`]）。
fn forget_releases(
    state: &AppState,
    config_dir: &Path,
    entry: &luna_vault::registry::VaultEntry,
) -> bool {
    let list_account = state.vault_registry.list_account().map(|a| a.key);
    let owner = crate::vault_lists::bound_owner(config_dir, entry)
        .map(|owner| crate::account_scope::account_key(&owner));
    crate::vault_lists::forget_releases_binding(entry, list_account.as_deref(), owner.as_deref())
}

/// 从清单移除一条记录。**只动记录，绝不删磁盘上的库**（PRD-06-049）。
///
/// 本机库整条删掉；账户的库只从当前列表账户的列表里拿掉，别的账户的列表不动（ADR-151 修订
/// 2026-10-15「续二」）。绑定归列表账户自己（或这一条会被整条删掉）时先在这台设备上断开
/// （[`release_local`]：会话、主密钥缓存、基线钉住、同步目录；云端副本与本地文件都不动），再动
/// 记录——同步列表只列库清单里的库，不再出现「库已移除、同步还在」。断开失败（钥匙串）就不动
/// 记录，如实报码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn forget_vault(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let entry = state
        .known_vaults()
        .into_iter()
        .find(|e| e.path == Path::new(&path));
    let Some(entry) = entry else {
        return Ok(());
    };
    if let Ok(config_dir) = app.path().app_config_dir() {
        if forget_releases(&state, &config_dir, &entry) {
            if let Some(id) = crate::sync_service::identity_of(&entry) {
                release_local(&app, &id, &entry.path).await?;
            }
        }
    }
    let list_account = state.vault_registry.list_account().map(|a| a.key);
    state
        .vault_registry
        .forget(&entry.path, list_account.as_deref());
    // 当场按新的库清单重发同步总览（不等下一轮）。
    if let Some(service) = app.try_state::<Arc<crate::sync_service::SyncService>>() {
        service.refresh(&app).await;
    }
    Ok(())
}

/// 改库的显示名（清单别名，不动目录名）。
#[tauri::command]
#[specta::specta]
pub(crate) fn rename_vault(
    state: tauri::State<'_, AppState>,
    path: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(coded_error_with(
            state.copy(),
            "VAULT_NAME_EMPTY",
            "vault-name-empty",
        ));
    }
    state
        .vault_registry
        .rename(Path::new(&path), name.to_owned());
    Ok(())
}

/// 切库预检（PRD-06-049）：数字与阻断理由在**点开始之前**给。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_switch_preflight(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<VaultSwitchPreflightDto, String> {
    Ok(VaultSwitchPreflightDto {
        recording_active: recording_blocks_switch(&state),
        pending_tasks: state.processing.active_count(),
        target_missing: !Path::new(&path).is_dir(),
    })
}

/// 录音是否阻断切库：只有**空闲**才放行——录音中/暂停/停止收尾中都还持有旧库的写盘路径。
pub(crate) fn recording_blocks_switch(state: &AppState) -> bool {
    !matches!(
        state.recording.state().status,
        crate::recording::RecordingStatus::Idle
    )
}

/// **切换到另一个库**（PRD-06-049 / ADR-025 / TDD-06 §12 资源交接表）。
///
/// 顺序契约：先完成旧库全部收尾，**再**装载新库——启动指针是最后翻转的（在 `bind_vault`
/// 内），任何一步失败都停在旧库完好态。
#[tauri::command]
#[specta::specta]
pub(crate) fn switch_vault(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let root = resolve_vault_root(state.copy(), PathBuf::from(&path), false).map_err(|_| {
        coded_error_args_with(
            state.copy(),
            "VAULT_MISSING",
            "vault-missing-path-gone",
            &[("path", path.clone())],
        )
    })?;
    if current_vault_root(&state)?.as_deref() == Some(root.as_path()) {
        return Ok(()); // 已在该库：幂等，不做无谓的卸载重装。
    }
    handover_and_bind(&app, &state, root, false)
}

/// 关闭全部笔记独立窗口（切库收尾）：它们持 label→**旧库**路径映射，留着就是一扇指向
/// 不存在上下文的窗口——比少几扇窗更糟（TDD-06 §12）。
fn close_all_note_windows(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let labels: Vec<String> = app
        .try_state::<AppState>()
        .and_then(|state| {
            state
                .note_windows
                .lock()
                .ok()
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default();
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.close(); // 关闭钩子会自行从 `note_windows` 摘除。
        }
    }
}

/// 释放当前库：按 TDD-06 §12 的交接表逐项收尾。**只关不开**——开新库归 `bind_vault`。
fn release_current_vault(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    // ⓪ 队列先真正静下来（唯一可能失败的一步，放最前——顺序契约：任何一步失败都停在
    //    旧库完好态，此刻还没有任何东西被拆）。取消是协作式的，只发取消不等退出的话，
    //    运行中任务的迟到写盘会落进换根后的新库；而且旧 worker 在当前件落定后还会立刻
    //    领取下一件旧库任务——故先暂停领取，再取消运行项，再定界等待它落定。
    state.processing.begin_handover();
    state.processing.cancel_running();
    if !state
        .processing
        .wait_running_settled(std::time::Duration::from_secs(10))
    {
        state.processing.end_handover();
        return Err(coded_error(app, "VAULT_SWITCH_BUSY", "vault-switch-busy"));
    }
    // 此刻旧库还未拆资源。与最终发布共用 Vault 锁：旧队列确实落盘并摘下后立刻
    // 自增代际，中间不能插进一次已通过代际复验的旧库发布。
    let guard = match state.vault.lock() {
        Ok(guard) => guard,
        Err(_) => {
            state.processing.end_handover();
            return Err("Vault 锁中毒".to_string());
        }
    };
    if state
        .processing
        .rebind(None, state.vault_generation())
        .is_err()
    {
        drop(guard);
        state.processing.end_handover();
        return Err(coded_error(
            app,
            "VAULT_QUEUE_SAVE_FAILED",
            "vault-queue-save-failed",
        ));
    }
    state.bump_vault_generation_locked(&guard);
    drop(guard);

    // ① 旧库已经失效；以下清理不会发生“旧队列未存就换根”。

    // ② 单例长任务止步（重扫）：取消标志即止，当前篇跑完自然收束。
    //    批量整理不在此列——它已并入统一处理队列（PRD-06-051），而队列的交接
    //    （暂停领取 → 取消运行项 → 定界等待落定）在本表更早一步已经做过了。
    if let Ok(guard) = state.graph_rescan.lock() {
        if let Some(cancel) = guard.cancel.as_ref() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    //    语义索引全量建库同口径（ADR-027）：它是另一个单例长任务（S16：万片约 40 分钟）。
    //    代际校验在 `embed_note` 的提交点已能拦住写盘，但那要等当前这一批嵌入跑完——
    //    置位取消令牌让它在**下一篇之前**就停，不白烧几分钟本地算力。
    if let Ok(guard) = state.embed_rebuild.lock() {
        if let Some(cancel) = guard.cancel.as_ref() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    //    深度体检同口径（PRD-16-031）：它是第三个单例长任务（每条声明一次本地 LLM）。
    //    判定结果的落盘点已有代际校验，但那要等当前这一条判完——置位取消令牌让它在
    //    **下一条之前**就停；`checkup_last_run` 也因此不会被一次半途而废的运行记上去。
    if let Ok(guard) = state.checkup_run.lock() {
        if let Some(cancel) = guard.cancel.as_ref() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    // ③ 蒸馏预览暂存槽：产物与库绑定，直接弃（TDD-06 §12）。
    if let Ok(mut slot) = state.distill_preview.lock() {
        *slot = None;
    }
    // ③b 语义检索的内存矩阵（ADR-027 / PRD-16 §5.6）：它是**按库**建的一块常驻内存，
    //    留着就会让新库的第一次问答按**上一个库的内容**排序——而且不会报错，只是安静
    //    地给错答案。整体作废，下次查询时按新库懒加载重建。
    embed::invalidate_matrix(state);

    // ④ 对话会话：持久会话已逐轮落盘、临时按定义即弃；顶替语义 = 直接清空内存槽。
    if let Ok(mut store) = state.chat.lock() {
        *store = ChatSessionStore::default();
    }

    // ⑤ 文件监听停机（新库的监听由 `install_vault_watcher` 重装）。
    state.stop_vault_watcher();

    // ⑥ 笔记独立窗口关闭：它们持 label→旧库路径映射，留着就是指向不存在上下文的悬空窗口。
    close_all_note_windows(app);

    // ⑦ 媒体服务：清空虚拟拼接注册表并轮换 token（TDD-12 §4——旧库媒体句柄不得跨库存活）。
    // `set_root` 已清空拼接注册表；token 轮换让旧库遗留的 URL 立即失效（TDD-12 §4）。
    state.media_server.rotate_token();

    // ⑧ 释放旧库索引句柄；队列已在 ⓪ 确认落盘并摘下，不能在资源拆除后才发现保存失败。
    {
        let mut guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        *guard = None;
    }
    Ok(())
}

/// 打开/创建 Vault 并绑定到 AppState：持久化启动指针、设媒体服务根、装监听、后台扫描上报进度。
pub(crate) fn bind_vault(
    app: &tauri::AppHandle,
    state: &AppState,
    root: PathBuf,
    create: bool,
) -> Result<(), String> {
    if create {
        std::fs::create_dir_all(&root).map_err(|e| format!("创建 Vault 目录失败：{e}"))?;
    } else if !root.is_dir() {
        return Err(format!("路径不是目录：{}", root.display()));
    }

    // 创建完成后再锚定一次，覆盖并发地把目标替换为链接的情况；Vault 级设置也必须从这条
    // 真实根读取，不能在 `Vault::open` 锚定之前先沿别名读一次。
    let root = std::fs::canonicalize(&root)
        .map_err(|error| vault_path_invalid(state.copy(), &error.to_string()))?;

    let changed_settings = merge_vault_settings_from_root(state, &root)?;

    let snapshot_max = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| "Settings 锁中毒".to_string())?;
        snapshot_max_from_settings(&settings)
    };
    let mut vault = Vault::open(&root)
        .map_err(vault_err)?
        .with_snapshot_max_versions(snapshot_max);
    // 登记进设备级库清单、认领身份（首启引导 / 切换器 / 管理库三条路共用这一处）；此刻登录着
    // 就记成这个账户上次打开的库，账户的库（含另一个账户的、被重新加回来的）列进它的列表。
    register_opened_vault(
        &state.vault_registry,
        &mut vault,
        app.path().app_config_dir().ok().as_deref(),
        crate::vault_lists::signed_in_key(app).as_deref(),
    );
    state.external_renames.reset();
    let _ = ExternalRenamesChanged.emit(app);
    // `Vault::open` 已把根锚定到真实目录；此后设置、媒体服务、监听与队列必须沿用同一根，
    // 不能一半保存用户选中的软链、一半使用真实路径，形成两个“当前库”身份。
    let root = vault.root().to_path_buf();

    {
        let mut guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        *guard = Some(vault);
    }

    // 持久化 Device 级启动指针（下次启动直接打开）。
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "storage.vault_root",
        SettingValueDto::Str(root.to_string_lossy().into_owned()),
    )?;

    // 媒体服务指向新根；放开 asset 协议作用域；（重新）安装文件监听。
    state.media_server.set_root(root.clone());
    let _ = app.asset_protocol_scope().allow_directory(&root, true);
    state.install_vault_watcher(app.clone());

    // 续跑档接上新库（ADR-025）：从**本库** `.luna/processing-queue.json` 恢复未完成任务，
    // 有待办即拉起 worker——「回到本库自动续跑」不能等下一次启动或下一次手动入队。
    state
        .processing
        .rebind(Some(&root), state.vault_generation())
        .map_err(|_| coded_error(app, "VAULT_QUEUE_SAVE_FAILED", "vault-queue-save-failed"))?;
    // 上次会话的失败 / 取消项随本库回填（PRD-04-062）：历史随库，切库即换。
    super::processing::restore_queue_history(app);
    if state.processing.has_pending() {
        transcribe::start_processing_worker_if_idle(app);
    }

    // Vault 级设置换源后逐键广播（TDD-06 §12）：漏掉这步，切库后前端各表面还拿着上一个
    // 库的取值——文件树按旧库的归档目录分区、蒸馏三档是旧库的设置，且没有任何报错。
    for key in changed_settings {
        let _ = SettingsChanged { key }.emit(app);
    }

    // 后台增量扫描 + 上报进度（不阻塞 UI 线程）。启动装载那条路在 `lib.rs` setup 里挂同一个
    // 入口（`start_index_scan`），守护见 `both_paths_that_open_a_vault_start_the_index_scan`。
    spawn_background_scan(app.clone(), false);
    Ok(())
}

/// 开库后登记进库清单并认领身份（PRD-06-049、ADR-140 ②）。
///
/// 启动装载与 `bind_vault` 两条开库路径共用（守护见
/// `both_paths_that_open_a_vault_register_it`）。身份已被另一份仍在的库持有（用户复制了整个
/// 库）时，后打开的这一份让出：两份共用一套设备状态会互相剪枝标签页。让出失败（库只读）
/// 就保持原身份——最坏是两份共用布局，不值得因此打不开库。
///
/// 账户（ADR-151 修订 2026-10-15「续二」）：先登记、再把它的同步绑定归属账户列进列表
/// （[`crate::vault_lists::claim_bound`]，`config_dir` 为空 = 没有配置目录，不看绑定），最后标记
/// 打开——`account` 是此刻登录的账户（启动装载时还不知道 = `None`，由启动对齐补记）。顺序不能换：
/// 先标记的话，一个清单外、却连着 A 的同步的文件夹会先被当成本机库，B 打开它也列不进 B 的列表。
pub(crate) fn register_opened_vault(
    registry: &luna_vault::registry::VaultRegistry,
    vault: &mut Vault,
    config_dir: Option<&Path>,
    account: Option<&str>,
) {
    let root = vault.root().to_path_buf();
    if registry.id_held_elsewhere(&root, vault.id()) {
        if let Err(error) = vault.reassign_id() {
            eprintln!("[vault] 库身份与另一份库重复，换新身份失败：{error}");
        }
    }
    if registry.register(&root, None).is_ok() {
        if let Some(config_dir) = config_dir {
            crate::vault_lists::claim_bound(registry, config_dir, Some(&root));
        }
    }
    registry.touch(&root, vault.id(), now_ms_for_registry(), account);
}

/// 库清单的"最后打开"时间戳（Unix 毫秒；取不到系统时间则记 0——排序退化为按名，不崩）。
pub(crate) fn now_ms_for_registry() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 一键重建索引（PRD-07-023 / TDD-06 §3）：从文件重新算一遍投影——走与开库同一条
/// 后台扫描（`scan://progress` 报进度、终态触发工作区刷新）。只读，绝不改写笔记；
/// 归入计划 / 嵌入向量 / 体检判定不在「从文件算得出」之列，`Index::clear` 刻意不动它们。
#[tauri::command]
#[specta::specta]
pub(crate) fn rebuild_vault_index(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        if guard.is_none() {
            return Err(state.vault_not_open());
        }
    }
    spawn_background_scan(app, true);
    Ok(())
}

/// 索引现状（PRD-06-004 ④）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_index_status(state: tauri::State<'_, AppState>) -> IndexStatusDto {
    state.scan.snapshot()
}

/// 取消正在进行的索引扫描（PRD-06-004 ②）：已写进去的批次有效，下次打开接着比对。
#[tauri::command]
#[specta::specta]
pub(crate) fn cancel_vault_scan(state: tauri::State<'_, AppState>) {
    state
        .scan
        .cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 启动装载（不经 `bind_vault`）的那一次扫描：库开着才起。
pub(crate) fn start_index_scan(app: &tauri::AppHandle) {
    let open = app
        .state::<AppState>()
        .vault
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false);
    if open {
        spawn_background_scan(app.clone(), false);
    }
}

/// 后台增量扫描（TDD-06 §4.1）：遍历与读文件都不持 Vault 锁，只在每批写索引时短暂持有；
/// 批与批之间查取消、查是否切了库（代际号变了）、查有没有新一轮扫描接手。`force` = 「重建
/// 索引」：全部重读，但同样不先清空。终态帧之后才排依赖完整索引的维护（知识地图比对）。
fn spawn_background_scan(app: tauri::AppHandle, force: bool) {
    use std::sync::atomic::Ordering;
    let state = app.state::<AppState>();
    let control = state.scan.clone();
    let vault = state.vault_arc();
    let generation = state.vault_generation.clone();
    let seq = control.seq.fetch_add(1, Ordering::SeqCst) + 1;
    control.cancel.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let gen0 = generation.load(Ordering::SeqCst);
        let stopped = || {
            control.cancel.load(Ordering::SeqCst)
                || control.seq.load(Ordering::SeqCst) != seq
                || generation.load(Ordering::SeqCst) != gen0
        };
        let finish = |indexed: u32, total: u32, cancelled: bool| {
            // 被新一轮接替的旧轮静默退出：新一轮已经在报自己的进度，旧轮的终态帧会把
            // 「扫描中」覆写成「已结束」，搜索与状态栏就会在扫描进行中撤掉「结果可能不全」。
            if control.seq.load(Ordering::SeqCst) != seq {
                return;
            }
            control.set(IndexStatusDto {
                scanning: false,
                indexed,
                total,
            });
            let _ = ScanProgress {
                indexed,
                total,
                current_path: None,
                done: true,
                cancelled,
            }
            .emit(&app);
        };
        // ① 根与指纹：短暂持锁。
        let prepared = vault.lock().ok().and_then(|guard| {
            let vault = guard.as_ref()?;
            Some((vault.root().to_path_buf(), vault.index_fingerprints().ok()?))
        });
        let Some((root, indexed)) = prepared else {
            finish(0, 0, true);
            return;
        };
        // ② 遍历与比对：不持锁。
        let Ok(walk) = luna_vault::scan::walk(&root) else {
            finish(0, 0, true);
            return;
        };
        // iCloud 未下载占位（TDD-06 §4.1 ②）：不进索引，但链接解析要把它们当作在。读不了的
        // 子目录（V1b）：其下笔记不进索引，交给体检列出来。
        if let Ok(guard) = vault.lock() {
            if let Some(vault) = guard.as_ref().filter(|vault| vault.root() == root) {
                vault.set_cloud_placeholders(walk.placeholders.iter().cloned());
                vault.set_unreadable_folders(walk.skipped_dirs.iter().cloned());
            }
        }
        let plan = luna_vault::scan::plan(&walk, &indexed, force);
        // 库外改名配对（PRD-06-030 ⑤）：从索引里消失的篇带着原来的指纹，新出现的篇读到后算指纹。
        // 重建索引（force）不配：那一轮里每篇都是「重读」，不是新出现。
        let vanished: Vec<(String, String)> = if force {
            Vec::new()
        } else {
            plan.to_remove
                .iter()
                .filter_map(|path| {
                    indexed
                        .get(path)
                        .map(|(_, hash)| (path.clone(), hash.clone()))
                })
                .collect()
        };
        let mut appeared: Vec<(String, String)> = Vec::new();
        let total = u32::try_from(plan.to_read.len() + plan.to_remove.len()).unwrap_or(u32::MAX);
        let known = if force {
            std::collections::HashMap::new()
        } else {
            indexed
        };
        control.set(IndexStatusDto {
            scanning: true,
            indexed: 0,
            total,
        });
        let mut done: u32 = 0;
        let emit = |done: u32, current: Option<&str>| {
            if control.seq.load(Ordering::SeqCst) != seq {
                return;
            }
            control.set(IndexStatusDto {
                scanning: true,
                indexed: done,
                total,
            });
            let _ = ScanProgress {
                indexed: done,
                total,
                current_path: current.map(str::to_owned),
                done: false,
                cancelled: false,
            }
            .emit(&app);
        };
        emit(0, None);
        // ③ 分批：读文件不持锁，写索引一批一个事务、短暂持锁。
        for batch in plan.to_read.chunks(luna_vault::scan::BATCH) {
            if stopped() {
                finish(done, total, true);
                return;
            }
            let items = luna_vault::scan::read_batch(&root, batch, &known);
            if !force {
                for item in &items {
                    if let luna_vault::scan::Item::Upsert { path, content, .. } = item {
                        if !known.contains_key(path) {
                            appeared.push((
                                path.clone(),
                                luna_vault::ContentHash::of(content.as_bytes())
                                    .as_str()
                                    .to_owned(),
                            ));
                        }
                    }
                }
            }
            let applied = vault.lock().ok().and_then(|guard| {
                let vault = guard.as_ref()?;
                (vault.root() == root).then(|| vault.apply_scan_batch(&items).is_ok())
            });
            if applied != Some(true) {
                finish(done, total, true);
                return;
            }
            done += u32::try_from(batch.len()).unwrap_or(u32::MAX);
            emit(done, batch.last().map(|note| note.path.as_str()));
        }
        // ④ 删掉磁盘上已经没有的。
        if !plan.to_remove.is_empty() && !stopped() {
            let removals: Vec<luna_vault::scan::Item> = plan
                .to_remove
                .iter()
                .map(|path| luna_vault::scan::Item::Remove { path: path.clone() })
                .collect();
            let applied = vault.lock().ok().and_then(|guard| {
                let vault = guard.as_ref()?;
                (vault.root() == root).then(|| vault.apply_scan_batch(&removals).is_ok())
            });
            if applied != Some(true) {
                finish(done, total, true);
                return;
            }
            done = total;
        }
        finish(done, total, false);
        report_scan_renames(&app, &vanished, &appeared, done as usize);
        // 全量索引终态重新比对既有 AI 地图的期望正文：覆盖外部编辑、崩溃发生在去抖入队
        // 之前，以及启动直开（不经 bind_vault）三条路径。缺地图仍绝不创建。
        knowledge::schedule_knowledge_map_refresh(&app);
        // 知识体检的周期提醒（PRD-16-032）：**默认关**，开着时也只在打开库这个自然时机看一眼
        // 距上次体检多久，且**只发通知、绝不代跑**。挂在扫描做完之后（V1b）：此前在扫描开始前
        // 就看，数的是上一次（或半截）的索引。启动与切库两条开库路径都经过这里。
        checkup::maybe_remind(&app);
    });
}

/// 等这一轮索引扫描结束（V1b）。持续整理等要读索引判候选的后台任务在动手前调用：启动或切库
/// 后的那一轮扫描期间，索引还停在上一次（或只更新了一半），拿它判候选会漏判、误判。换了库就
/// 不再等（调用方随后按代际号作废）；用户取消扫描也算结束——那是他的决定。
pub(crate) async fn wait_for_index_ready(state: &AppState, vault_generation: u64) {
    while state.scan.snapshot().scanning && state.still_current_vault(vault_generation) {
        tokio::time::sleep(INDEX_READY_POLL).await;
    }
}

/// [`wait_for_index_ready`] 的查询间隔：扫描以秒计，四分之一秒的迟到用户感觉不到。
const INDEX_READY_POLL: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(test)]
mod tests {

    /// 当前登录的账户显示完整邮箱；列表是别的账户的（退出后仍显示上一个账户的列表）就显示
    /// 存下来的打码标签（用户裁决 2026-10-15）。
    #[test]
    fn the_signed_in_account_shows_its_full_email_others_stay_masked() {
        let list = || luna_vault::registry::ListAccount {
            key: "acct-a".into(),
            label: "jo…88@gmail.com".into(),
        };
        let a = Some(("acct-a".to_owned(), "johnnymai5788@gmail.com".to_owned()));
        let b = Some(("acct-b".to_owned(), "johnnyzhao5619@gmail.com".to_owned()));
        assert_eq!(super::title_for(list(), a), "johnnymai5788@gmail.com");
        assert_eq!(super::title_for(list(), b), "jo…88@gmail.com");
        assert_eq!(super::title_for(list(), None), "jo…88@gmail.com");
    }

    /// 两条开库路径都起索引扫描，且启动装载不再同步扫描（PRD-06-004、TDD-06 §4.1）。
    ///
    /// 启动装载不经 `bind_vault`：只在切库时扫，绝大多数只开同一个库的用户就永远不扫描；
    /// 而同步扫描会让 1 万篇的库每次启动卡 50 秒（笔记库对标轮 V1 基准）。任一处被删即红。
    #[test]
    fn both_paths_that_open_a_vault_start_the_index_scan() {
        let startup = crate::ipc::source_without_tests(include_str!("../lib.rs"));
        assert!(
            startup.contains("ipc::start_index_scan(app.handle())"),
            "启动装载没有起索引扫描"
        );
        let switch = crate::ipc::source_without_tests(include_str!("vaults.rs"));
        let bind = switch
            .split("pub(crate) async fn bind_vault")
            .nth(1)
            .or_else(|| switch.split("fn bind_vault").nth(1))
            .expect("找得到 bind_vault");
        let bind_body = bind.split("\n}\n").next().unwrap_or(bind);
        assert!(
            bind_body.contains("spawn_background_scan(app.clone(), false)"),
            "切库没有起索引扫描"
        );
        let opening = crate::ipc::source_without_tests(include_str!("providers.rs"));
        assert!(
            !opening.contains("opened.scan()"),
            "启动装载又同步扫描了——大库每次启动都会卡住"
        );
    }

    /// 读索引的后续动作等这一轮扫描做完（V1b）：体检提醒挂在扫描终态之后（不在开库那一刻、
    /// 不在扫描开始之前），持续整理的两个执行体动手前先等就绪。任一处被挪回去即红。
    #[test]
    fn index_readers_wait_for_the_scan_to_finish() {
        let source = crate::ipc::source_without_tests(include_str!("vaults.rs"));
        let scan = source
            .split("fn spawn_background_scan")
            .nth(1)
            .expect("找得到 spawn_background_scan");
        let terminal = scan.find("finish(done, total, false);").expect("终态帧");
        let remind = scan
            .find("checkup::maybe_remind(&app)")
            .expect("体检提醒挂在扫描里");
        assert!(terminal < remind, "体检提醒跑到扫描做完之前去了");
        let startup = crate::ipc::source_without_tests(include_str!("../lib.rs"));
        assert!(
            !startup.contains("maybe_remind"),
            "启动又在扫描前提醒体检了"
        );
        let bind = source.split("fn bind_vault").nth(1).expect("bind_vault");
        let bind_body = bind.split("\n}\n").next().unwrap_or(bind);
        assert!(
            !bind_body.contains("maybe_remind"),
            "切库又在扫描前提醒体检了"
        );
        let ingest = crate::ipc::source_without_tests(include_str!("ingest.rs"));
        for executor in [
            "pub(crate) async fn run_background_topic_prepare(",
            "pub(crate) async fn run_background_topic_draft(",
        ] {
            let body = ingest.split(executor).nth(1).expect(executor);
            let body = body.split("\n}\n").next().unwrap_or(body);
            assert!(
                body.contains("wait_for_index_ready(&state, vault_generation).await"),
                "{executor} 没等索引就绪"
            );
        }
    }

    /// 两条开库路径都登记进库清单并认领身份（ADR-140 ②），并且都顾到账户（ADR-151 修订
    /// 2026-10-15「续二」）：切库那条带此刻登录的账户；启动装载那条那时还不知道谁登录着，由启动
    /// 对齐补记、并按列表账户判可见性。只挂在一条上，另一条打开的复制品就会和原库共用一套标签页、
    /// 或打开一个这个账户看不见的库；任一处被删即红。比对前去掉空白：守护不该跟着排版变红。
    #[test]
    fn both_paths_that_open_a_vault_register_it() {
        let squash = |text: &str| text.split_whitespace().collect::<String>();
        let startup = squash(&crate::ipc::source_without_tests(include_str!(
            "providers.rs"
        )));
        assert!(
            startup.contains(
                "register_opened_vault(&vault_registry,&mutopened,config_dir.as_deref(),"
            ),
            "启动装载没有登记库与认领身份"
        );
        assert!(
            startup.contains("vault_lists::startup_root(&vault_registry,explicit_root)"),
            "启动装载没有按列表账户判可见性"
        );
        let lib = squash(&crate::ipc::source_without_tests(include_str!("../lib.rs")));
        assert!(
            lib.contains("vault_lists::startup(app.handle())"),
            "启动装载的账户那一笔没人补"
        );
        let switch = crate::ipc::source_without_tests(include_str!("vaults.rs"));
        let bind = switch
            .split("fn bind_vault")
            .nth(1)
            .expect("找得到 bind_vault");
        let bind_body = squash(bind.split("\n}\n").next().unwrap_or(bind));
        assert!(
            bind_body.contains("register_opened_vault(&state.vault_registry,&mutvault,"),
            "切库没有登记库与认领身份"
        );
        assert!(
            bind_body.contains("crate::vault_lists::signed_in_key(app)"),
            "切库没有带上此刻登录的账户"
        );
        for source in [&startup, &squash(&switch)] {
            assert!(
                !source.contains(concat!("vault_registry", ".touch(")),
                "绕过 register_opened_vault 直接 touch：身份没认领"
            );
        }
    }

    /// 切换菜单与「管理库」的清单：只列列表账户看得见的库，按本机 / 账户分节；此刻开着的库即使
    /// 看不见（换账户时录音中没能切走）也列上。
    #[test]
    fn the_list_shows_visible_vaults_in_sections_and_never_hides_the_open_one() {
        use super::{list_of, VaultSectionDto};
        let entry = |path: &str, accounts: &[&str]| luna_vault::registry::VaultEntry {
            path: path.into(),
            name: path.into(),
            last_opened_ms: 0,
            id: None,
            accounts: accounts.iter().map(|a| (*a).to_owned()).collect(),
        };
        let entries = vec![
            entry("/v/device", &[]),
            entry("/v/a", &["acct-a"]),
            entry("/v/b", &["acct-b"]),
        ];
        let rows = list_of(
            &entries,
            Some("acct-b"),
            Some(std::path::Path::new("/v/device")),
        );
        let shown: Vec<_> = rows.iter().map(|r| (r.path.as_str(), r.section)).collect();
        assert_eq!(
            shown,
            vec![
                ("/v/device", VaultSectionDto::Device),
                ("/v/b", VaultSectionDto::Account)
            ]
        );
        assert!(rows[0].active);
        let stuck = list_of(&entries, Some("acct-b"), Some(std::path::Path::new("/v/a")));
        assert!(stuck.iter().any(|r| r.path == "/v/a" && r.active));
        let never = list_of(&entries, None, None);
        assert_eq!(never.len(), 1, "从没登录过：只有本机库");
    }

    #[cfg(unix)]
    #[test]
    fn vault_path_resolution_uses_one_real_identity_for_existing_and_new_roots() {
        use super::resolve_vault_root;

        let parent = tempfile::tempdir().expect("parent");
        let real = parent.path().join("real");
        std::fs::create_dir_all(&real).expect("real root");
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).expect("alias");

        assert_eq!(
            resolve_vault_root(None, alias.clone(), false).expect("existing root"),
            std::fs::canonicalize(&real).unwrap()
        );
        assert_eq!(
            resolve_vault_root(None, alias.join("new").join("nested"), true).expect("new root"),
            std::fs::canonicalize(&real)
                .unwrap()
                .join("new")
                .join("nested")
        );
        assert!(
            resolve_vault_root(None, alias.join("missing").join("..").join("escape"), true)
                .is_err(),
            "重叠校验与实际创建不得对父级分量得出两条路径"
        );
    }
}
