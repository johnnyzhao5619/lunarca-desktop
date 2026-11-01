//! 工作区与窗口命令域（PRD-07，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：路由白名单与导航、笔记/库窗口的开启与身份、粘贴与远程附件落盘、
//! 以及工作区布局的读写与裁剪。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;
use crate::device_state::DeviceStateKind;

/// 启动就绪事件（低频 typed event）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct AppReady {
    /// 版本。
    pub version: String,
}

/// 跨窗口工作区导航请求到了（辅助窗口/后端 → 主窗口）。**载荷为空**：route 只存在
/// [`PendingNavigation`] 里、只经 [`take_pending_navigation`] 取——事件只是"来取"的信号。
///
/// 为什么不把 route 放进载荷：事件是易失的。冷启动由连接链接拉起时，Rust 在 setup 里就发了
/// 这条事件，那时主窗的工作区还没挂上监听器，事件落地即丢（2026-10-15 Windows 真机：应用
/// 起来了、却停在笔记而不是账户页）。请求因此先**存**，界面挂载时取一次、收到事件再取一次；
/// 取即清，所以无论两者谁先到，一次请求恰好落地一次。载荷里再带一份 route 就有了第二个读法，
/// 照它执行的那一端会让请求在下次挂载时被再执行一遍。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct WorkspaceNavigate {}

/// 待主窗工作区落地的那一次导航（单槽，后到覆盖先到：用户最后点的那一下才是他要去的地方）。
///
/// route 语义与主窗口 `luna:navigate` CustomEvent 完全一致（`settings://<分组>` 等），工作区
/// 取到后原样转发给既有处理器（单一导航语义，不另起一套路由）。
#[derive(Debug, Default)]
pub(crate) struct PendingNavigation(std::sync::Mutex<Option<String>>);

impl PendingNavigation {
    /// 存下一次导航（覆盖还没被取走的那一次）。
    pub(crate) fn set(&self, route: String) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(route);
        }
    }

    /// 取走并清空：同一次请求只有一个取的人拿得到。
    pub(crate) fn take(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// 请求主窗工作区去某个 route——后端发起导航的**唯一**入口（[`open_workspace_route`] 与
/// 连接链接送达都经这里）：先存、再通知。存是为了冷启动（工作区还没挂上，事件会丢），
/// 通知是为了运行中（工作区早已挂着，不必等下次挂载）。调用方负责白名单与亮主窗。
pub(crate) fn request_workspace_navigation(app: &tauri::AppHandle, route: String) {
    use tauri::Manager as _;
    if let Some(state) = app.try_state::<AppState>() {
        state.pending_navigation.set(route);
    }
    let _ = WorkspaceNavigate {}.emit(app);
}

/// 取走待落地的导航（工作区挂载时取一次、收到 [`WorkspaceNavigate`] 再取一次）。
#[tauri::command]
#[specta::specta]
pub(crate) fn take_pending_navigation(state: tauri::State<'_, AppState>) -> Option<String> {
    state.pending_navigation.take()
}

/// 允许跨窗口导航的 route 前缀白名单（与主窗口 `luna:navigate` 处理器支持的
/// scheme 同源；命令可被任意窗口调用，白名单挡住任意字符串注入）。
/// `workspace://` 在处理器中无分支是刻意的：该目标的全部语义就是"唤起主窗口"，
/// 命令本体已完成 show/focus，route 事件到达后无事可做。
///
/// **`activity://` 已随「动态」聚合页一并退役**（ADR-087 ②）：它此前把用户送进一个
/// 把「通知」与「用量」塞在一起的容器，而那两件事已各自回到自己的宿主（状态栏铃铛的
/// 通知抽屉 / 设置里的用量与执行记录）。**不留静默重定向**（ADR-046 无兼容层）：留一条
/// "自动改送到用量页"的分支，等于让一个已经不存在的心智继续在代码里活着，而调用方永远
/// 不会知道自己用的是旧路由。现在它按未知前缀被 `ROUTE_REJECTED` 明确拒绝。
const WORKSPACE_ROUTE_PREFIXES: &[&str] = &[
    "settings://",
    "queue://",
    "workspace://",
    "notifications://",
    "recording://",
    "transcribe://",
    "quick-notes://",
    "chats://",
    "notes://",
    // 创作空间（ADR-162）：`creator://` 落看板；`creator://<项目主页路径>` 打开那篇主页。
    "creator://",
    // 「应用」目的地（ADR-165 ⑤）：`apps://` 落列表，`apps://<id>` 落那个应用的详情页。
    "apps://",
    // **知识库分区的两条**（ADR-088）：`topics://` 是主题页目录那一格，
    // `tags://` 的语义**逐字不变**——它此前落 `knowledge/tags`，此后仍落 `knowledge/tags`，
    // 变的只是那一格从合并分区的一半变成一个独立分区。拆分前那个合并分区从来没有过
    // 自己的 scheme，故这里**没有一条要退役的前缀**，也不需要任何拒绝分支。
    "topics://",
    "tags://",
    "graph://",
];

/// 跨窗口导航目标是否在白名单内（`open_workspace_route` 的唯一判定点，拆出纯
/// 函数供守护测试锁住"处理器支持的 scheme 必须可跨窗口到达"）。
pub(crate) fn workspace_route_allowed(route: &str) -> bool {
    WORKSPACE_ROUTE_PREFIXES
        .iter()
        .any(|prefix| route.starts_with(prefix))
}

/// 从辅助窗口（弹窗翻译等）唤起主窗口并导航到指定 route——服务失败引导
/// 「打开模型设置」等动作（PRD-01 §异常）。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_workspace_route(app: tauri::AppHandle, route: String) -> Result<(), String> {
    use tauri::Manager as _;
    if !workspace_route_allowed(&route) {
        return Err(coded_error_args(
            &app,
            "ROUTE_REJECTED",
            "route-rejected",
            &[("route", route.to_string())],
        ));
    }
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_owned())?;
    let _ = main.unminimize();
    let _ = main.show();
    let _ = main.set_focus();
    request_workspace_navigation(&app, route);
    Ok(())
}

/// 弹出编辑器独立窗口标签前缀（与 `App.tsx` / capabilities 通配 `note-editor-*` 同源）。
const NOTE_EDITOR_WINDOW_PREFIX: &str = "note-editor-";
/// 副库窗口 label 前缀（PRD-06-050）：命令按窗口身份解析落点库的唯一判据。
pub(crate) const VAULT_WINDOW_PREFIX: &str = "vault-window-";

/// 笔记标题（= 文件名去 `.md`，PRD-06-008）供窗口标题栏。
///
/// **不要与 [`super::playback::note_stem_of`] 混为一谈**：那一个去的是**任意**扩展名、
/// 取不到时兜底 `"transcript"`，服务的是导出文件名；这一个只认 `.md`、取不到就原样返回，
/// 服务的是窗口标题栏。两者此前同名，靠 `mod.rs` 里的本地定义**遮蔽**掉兄弟模块那份而
/// 相安无事——纵切把遮蔽拿掉后当场编译不过（E0659）。同名不同义比重复更贵：它不会被
/// 任何测试发现，只会让下一个人随手调到错的那个。
pub(crate) fn note_title_of(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    name.strip_suffix(".md").unwrap_or(name).to_owned()
}

/// 在独立窗口打开一篇笔记（PRD-07-036 弹出窗口；Obsidian「移动到新窗口」对标）。
/// 常规可激活窗口（编辑器需夺焦 + 键盘输入，非 NSPanel）；关闭窗口时前端经
/// `open_workspace_route("notes://…")` 回主窗打开（单一导航语义）。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_note_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    // 仅接受 Vault 内既有的 .md（挡路径穿越与非笔记目标）。
    if !path.ends_with(".md") {
        return Err(coded_error(&app, "NOT_A_NOTE", "not-a-note"));
    }
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        crate::media_server::resolve_vault_path(vault.root(), &path).map_err(|_| {
            coded_error_args(
                &app,
                "NOTE_NOT_FOUND",
                "note-not-found",
                &[("path", path.to_string())],
            )
        })?;
    }

    let seq = state
        .note_window_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{NOTE_EDITOR_WINDOW_PREFIX}{seq}");
    state
        .note_windows
        .lock()
        .map_err(|_| "窗口表锁中毒".to_string())?
        .insert(label.clone(), path.clone());

    crate::open_note_editor_window(&app, &label, &note_title_of(&path)).map_err(|e| {
        // 建窗失败时回滚映射，避免悬挂条目。
        if let Ok(mut map) = state.note_windows.lock() {
            map.remove(&label);
        }
        e.to_string()
    })
}

/// 打开一扇**副库独立窗口**（PRD-06-050）：受限浏览 + 正文编辑，不承载采集/队列/AI。
///
/// 护栏：目标必须在库清单里（入口是管理库弹窗）；当前主库被拒（它就在主窗口）；
/// 同库已开即前置聚焦（同一库两扇副库窗 = 两套写守卫互踩）。v1 不装监听、不跑全量
/// 扫描——树与读写都走文件系统，索引由本窗口自己的写盘增量维护。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_vault_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    use tauri::Manager as _;
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(coded_error_args(
            &app,
            "VAULT_MISSING",
            "vault-missing-path-gone",
            &[("path", path.to_string())],
        ));
    }
    if current_vault_root(&state)?.as_deref() == Some(root.as_path()) {
        return Err(coded_error(
            &app,
            "VAULT_WINDOW_IS_ACTIVE",
            "vault-window-is-active",
        ));
    }
    // 只开列表账户看得见的库（另一个账户的库这里看不见，ADR-151 修订 2026-10-15「续二」）。
    if !state.known_vaults().iter().any(|e| e.path == root) {
        return Err(coded_error(
            &app,
            "VAULT_NOT_REGISTERED",
            "vault-not-registered",
        ));
    }
    // 同库去重：已开即聚焦（用户要的是"看到那个库"，不是第二扇窗）。
    if let Some(label) = state.secondary_window_for_root(&root) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.set_focus();
            return Ok(());
        }
        state.forget_vault_window(&label); // 窗口已没了却残留映射：自愈。
    }

    let vault = Vault::open(&root).map_err(vault_err)?;
    // 同步扫描建索引（与主库启动同口径"MVP-0 小库足够快"）：图谱与标签读的是索引，
    // 不扫描就是一张空图。树与读写走文件系统，不依赖此步。
    let _ = vault.scan();
    // 图片经 asset 协议渲染，放行副库根（只读服务面；媒体 A/V 回放仍是主库专属，TDD-12 §4）。
    let _ = app.asset_protocol_scope().allow_directory(&root, true);

    let seq = state
        .vault_window_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{VAULT_WINDOW_PREFIX}{seq}");
    let title = state
        .vault_registry
        .list()
        .iter()
        .find(|e| e.path == root)
        .map(|e| e.name.clone())
        .unwrap_or_else(|| luna_vault::registry::default_name(&root));
    state
        .secondary_vaults
        .lock()
        .map_err(|_| "副库表锁中毒".to_string())?
        .insert(label.clone(), Arc::new(Mutex::new(vault)));

    crate::open_secondary_vault_window(&app, &label, &title).map_err(|e| {
        state.forget_vault_window(&label); // 建窗失败回滚映射。
        e.to_string()
    })
}

/// 关一扇副库窗口（主库切到它的根、换了账户后它看不见了）：走窗口自己的关闭路径——编辑器有未
/// 保存的改动先落盘，落盘失败由窗口否决关闭（`VaultEditor` 的 `onCloseRequested`），绝不直接销毁。
/// 窗口已经没了却残留映射：摘掉即可。
pub(crate) fn close_vault_window(app: &tauri::AppHandle, state: &AppState, label: &str) {
    use tauri::Manager as _;
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.close(); // 销毁钩子自会摘除映射。
    } else {
        state.forget_vault_window(label);
    }
}

/// 副库窗口挂载时查询自己的库名（页头徽标；label 即身份，免传参）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_window_info(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    let root = {
        let map = state
            .secondary_vaults
            .lock()
            .map_err(|_| "副库表锁中毒".to_string())?;
        let Some(handle) = map.get(window.label()) else {
            return Ok(None);
        };
        let vault = handle.lock().map_err(|_| "副库锁中毒".to_string())?;
        vault.root().to_path_buf()
    };
    Ok(Some(
        state
            .vault_registry
            .list()
            .iter()
            .find(|e| e.path == root)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| luna_vault::registry::default_name(&root)),
    ))
}

/// 弹出窗口挂载时按自身 label 取要打开的笔记路径（免 URL 编码传参）。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_window_path(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state
        .note_windows
        .lock()
        .map_err(|_| "窗口表锁中毒".to_string())?
        .get(window.label())
        .cloned())
}

/// 附件落盘目标（`attachment_target` 纯逻辑推导，PRD-06-012 四模式）。
pub(crate) struct AttachmentTarget {
    /// 库内落盘目录（相对 vault root；`""` = 仓库根）。
    pub(crate) dir_rel: String,
    /// 回填到正文的**笔记相对**链接（如 `assets/x.png` / `../Attachments/x.png` / `x.png`）。
    pub(crate) link: String,
}

/// 附件位置策略（PRD-06-012 四模式）→ 落盘目录 + 笔记相对链接（纯逻辑，单一事实源）。
/// `mode` ∈ vault_root / note_folder / note_subfolder / specified_folder（未知回退子目录）。
/// 路径推导复用 `luna_vault::pathutil`（与附件跟随移动同源，DRY）。
pub(crate) fn attachment_target(
    mode: &str,
    folder: &str,
    note_rel: &str,
    filename: &str,
) -> AttachmentTarget {
    let note_dir = luna_vault::parent_dir(note_rel);
    let folder = luna_vault::sanitize_vault_rel_dir(folder);
    let dir_rel = match mode {
        "vault_root" => String::new(),
        "note_folder" => note_dir.to_owned(),
        "specified_folder" => {
            if folder.is_empty() {
                "Attachments".to_owned()
            } else {
                folder
            }
        }
        // note_subfolder（默认）：笔记同级子目录。
        _ => {
            luna_vault::join_vault_rel(note_dir, if folder.is_empty() { "assets" } else { &folder })
        }
    };
    let file_vault_rel = luna_vault::join_vault_rel(&dir_rel, filename);
    AttachmentTarget {
        link: luna_vault::relative_vault_path(note_dir, &file_vault_rel),
        dir_rel,
    }
}

/// 采集调参：把设置里的四个旋钮投影为 [`luna_audio::CaptureTuning`]。
///
/// **三条采集入口共用同一条投影**：录音（[`super::recording`]）、实时字幕
/// （[`super::caption`]）、语音输入（[`super::voice`]）。语音输入此前直接用
/// `CaptureConfig::default()` 起会话，绕过了这里——于是连 `recording.waveform_fps`
/// 都对语音输入 HUD 无效；接进来之后，"设置里能调的"与"真机上生效的"才是同一份。
///
/// `recording.waveform_fps` / `recording.disk_warn_gib` / `recording.rolling_split_gib` /
/// `recording.vad_sensitivity` 此前**登记了却从未被读取**——writer 与 pipeline 一直只用
/// 编译期默认，用户怎么调都没反应（PRD-04 §5 磁盘预警与自动切分、PRD-09-040、PRD-04 §3.2）。
/// 容量项以 GiB 存储，此处是唯一换算点；灵敏度的字面量 ↔ 枚举对应关系在
/// [`luna_audio::VadSensitivity::from_setting`]（**本层不做任何字符串比较**，否则设置值与
/// 枚举的对应关系就有了第二处实现，加档位时必然漂移）。
///
/// ⏱ **生效时机：下一次会话**。本函数在会话装配时读一次快照，会话进行中改设置不回灌
/// （VAD 的噪声底跟踪是有状态的，中途换余量会让门限跳变）——与 `caption.capture_source`
/// 同语义。
pub(crate) fn capture_tuning(settings: &Settings) -> luna_audio::CaptureTuning {
    const GIB: u64 = 1024 * 1024 * 1024;
    let defaults = luna_audio::CaptureTuning::default();
    let gib = |key: &str, fallback: u64| -> u64 {
        settings
            .get_int(key)
            .and_then(|v| u64::try_from(v).ok())
            .filter(|v| *v > 0)
            .map_or(fallback, |v| v.saturating_mul(GIB))
    };
    luna_audio::CaptureTuning {
        waveform_fps: settings
            .get_int("recording.waveform_fps")
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .unwrap_or(defaults.waveform_fps),
        disk_warn_bytes: gib("recording.disk_warn_gib", defaults.disk_warn_bytes),
        rolling_split_max_bytes: gib(
            "recording.rolling_split_gib",
            defaults.rolling_split_max_bytes,
        ),
        vad_sensitivity: luna_audio::VadSensitivity::from_setting(
            settings
                .get_str("recording.vad_sensitivity")
                .unwrap_or_default()
                .as_str(),
        ),
    }
}

/// 录音/导入媒体与边车的落盘目录（Vault 相对；`""` = 仓库根）——媒体作为宿主笔记的**附件**，
/// 与粘贴/远程下载/图片附件遵循**同一** `vault.attachment_location` / `vault.attachment_folder`
/// 策略（PRD-06-012 四模式，DRY），不再散落在录音归档目录根下（PRD-04-030 收口）。
/// `note_rel` = 宿主笔记 Vault 相对路径。移动笔记时媒体随附件跟随迁移（`Vault::move_`）。
pub(crate) fn media_asset_dir(settings: &Settings, note_rel: &str) -> String {
    let mode = settings
        .get_str("vault.attachment_location")
        .unwrap_or_default();
    let folder = settings
        .get_str("vault.attachment_folder")
        .unwrap_or_default();
    attachment_target(&mode, &folder, note_rel, "x").dir_rel
}

/// 把附件字节落到笔记的附件目录并回填笔记相对链接（粘贴/远程下载共用，DRY）。
/// 命名 `<prefix>-<时间戳>.<ext>`（无空格、同名追加序号，PRD-06-030 可移植）；广播
/// `vault://changed` 使新文件即现于文件树。`root` 由调用方在**未跨 await 持锁**下取得。
/// 附件落点：哪个库、按哪个库的附件位置设置、变更广播给谁（PRD-06-050）。
///
/// 副库窗口里粘贴 / 拖入的附件必须落副库——此前四条附件命令都取主库根，副库笔记里的
/// 图片链接指向一个不存在的文件，主库多出一份无主附件。附件位置是 Vault 级设置，故副库
/// 按它自己的 `.luna/settings.toml` 推导目录，不借主库的。
struct AssetDestination {
    root: PathBuf,
    settings: Settings,
    scope: VaultChangeScope,
}

fn asset_destination(state: &AppState, window_label: &str) -> Result<AssetDestination, String> {
    let root = state.with_vault(window_label, |vault| Ok(vault.root().to_path_buf()))?;
    let scope = VaultChangeScope::for_window(window_label);
    let settings = match scope {
        VaultChangeScope::ActiveVault => state.settings_snapshot(),
        VaultChangeScope::SecondaryVault => {
            settings_with_vault_scope_from_root(state.settings_snapshot(), &root)
        }
    };
    Ok(AssetDestination {
        root,
        settings,
        scope,
    })
}

fn write_asset_to_note_folder(
    app: &tauri::AppHandle,
    dest: &AssetDestination,
    note_path: &str,
    prefix: &str,
    ext: &str,
    bytes: &[u8],
) -> Result<String, String> {
    if note_path.contains("..") || note_path.starts_with('/') {
        return Err(coded_error(app, "INVALID_NOTE_PATH", "invalid-note-path"));
    }
    // 缺省回落注册表默认（note_subfolder / assets）。
    let mode = dest
        .settings
        .get_str("vault.attachment_location")
        .unwrap_or_default();
    let folder = dest
        .settings
        .get_str("vault.attachment_folder")
        .unwrap_or_default();
    let target_dir = dest
        .root
        .join(attachment_target(&mode, &folder, note_path, "x").dir_rel);
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("MKDIR_FAILED: {e}"))?;

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut name = format!("{prefix}-{stamp}.{ext}");
    let mut counter = 1u32;
    while target_dir.join(&name).exists() {
        name = format!("{prefix}-{stamp}-{counter}.{ext}");
        counter += 1;
    }
    let target = attachment_target(&mode, &folder, note_path, &name);
    std::fs::write(target_dir.join(&name), bytes).map_err(|e| format!("WRITE_FAILED: {e}"))?;

    // 新附件即出现在文件树（新建的 assets 目录/文件；watcher 可能有延迟，显式广播确保即刷）。
    emit_vault_changed_in_scope(
        app,
        &luna_vault::join_vault_rel(&target.dir_rel, &name),
        false,
        dest.scope,
    );
    Ok(target.link)
}

/// 粘贴的图片/附件落盘（PRD-06-012，Obsidian 附件粘贴对标）。
/// 入参 `data_url` = `data:<mime>;base64,<payload>`（前端 `FileReader.readAsDataURL`
/// 原生编码，免手写分块 base64）；落盘目录由**附件位置策略**（`vault.attachment_location`
/// / `vault.attachment_folder`）决定，返回可移植的**笔记相对**链接供插入 `![](…)`（PRD-06-030）。
#[tauri::command]
#[specta::specta]
pub(crate) fn save_pasted_asset(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    data_url: String,
) -> Result<String, String> {
    use base64::Engine as _;

    let (mime, payload) = data_url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
        .ok_or_else(|| coded_error(&app, "INVALID_DATA_URL", "invalid-data-url"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("DECODE_FAILED: {e}"))?;
    if bytes.is_empty() {
        return Err(coded_error(&app, "EMPTY_ASSET", "empty-asset-paste"));
    }

    let dest = asset_destination(&state, window.label())?;
    let prefix = if mime.starts_with("image/") {
        "Pasted-image"
    } else {
        "Pasted-file"
    };
    write_asset_to_note_folder(
        &app,
        &dest,
        &note_path,
        prefix,
        &asset_ext_from_mime(mime),
        &bytes,
    )
}

/// 粘贴 / 拖入的附件**已落盘、插入却被取消**（结果回来前撤销了、或这一格换成了别的笔记）时回收它们
/// （PRD-06-012 ②）：否则每取消一次就在库里留一份没人引用的文件（E5 真机 2026-10-06：粘贴大图后立刻
/// ⌘Z，留下一张 20 MB 的图）。`links` 是落盘命令回给前端的**笔记相对链接**，换算到库内路径归 Rust；移走走孤儿移除同一条
/// 路——此刻重判引用、按删除策略（可还原），被别处引用上的原样留下。作用在发起窗口的那个库上。
#[tauri::command]
#[specta::specta]
pub(crate) fn discard_pasted_assets(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    links: Vec<String>,
) -> Result<(), String> {
    let paths = pasted_asset_vault_paths(&note_path, &links);
    if paths.is_empty() {
        return Ok(());
    }
    let dest = asset_destination(&state, window.label())?;
    let mode = vault_delete_mode_from_settings(&dest.settings);
    let removal = state.with_vault(window.label(), |vault| {
        vault.remove_orphan_assets(&paths, mode).map_err(vault_err)
    })?;
    for path in &removal.removed {
        emit_vault_changed_in_scope(&app, path, true, dest.scope);
    }
    Ok(())
}

/// 笔记相对链接（可能转义过空格、可能带 `../`）→ 库内路径；越出库根的丢掉。
fn pasted_asset_vault_paths(note_path: &str, links: &[String]) -> Vec<String> {
    let note_dir = luna_vault::parent_dir(note_path);
    links
        .iter()
        .filter_map(|link| {
            luna_vault::normalize_vault_rel(note_dir, &luna_vault::percent_decode(link))
        })
        .collect()
}

#[cfg(test)]
mod discard_pasted_assets_tests {
    use super::pasted_asset_vault_paths;

    #[test]
    fn note_relative_links_resolve_to_vault_paths() {
        let links = vec![
            "assets/Pasted-image-1.png".to_string(),
            "../Attachments/a%20b.png".to_string(),
            "../../escape.png".to_string(),
        ];
        assert_eq!(
            pasted_asset_vault_paths("项目/会议纪要.md", &links),
            vec![
                "项目/assets/Pasted-image-1.png".to_string(),
                "Attachments/a b.png".to_string()
            ],
        );
        assert_eq!(
            pasted_asset_vault_paths("根.md", &["p.png".to_string()]),
            vec!["p.png".to_string()],
        );
    }
}

/// 下载远程图片链接为**本地附件**并回填笔记相对链接（PRD-06-012，用户粘贴显式触发）。
/// 严格本地模式硬拦截（联网纪律）；仅接受 `image/*` 响应；命名 `Downloaded-image-<时间戳>`。
/// 之后即为本地图片，就地渲染（Live Preview / 阅读模式）不再依赖网络。
#[tauri::command]
#[specta::specta]
pub(crate) async fn download_remote_asset(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    url: String,
) -> Result<String, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(coded_error(&app, "URL_REJECTED", "url-rejected"));
    }
    if state.strict_local() {
        return Err(coded_error(
            &app,
            "STRICT_LOCAL_BLOCKED",
            "strict-local-blocked-image",
        ));
    }
    // 未跨 await 取落点（std::Mutex 不可跨 await 持有）。
    let dest = asset_destination(&state, window.label())?;

    let response = reqwest::Client::new()
        .get(trimmed)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("DOWNLOAD_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("DOWNLOAD_FAILED: HTTP {}", response.status()));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !mime.starts_with("image/") {
        return Err(coded_error_args(
            &app,
            "NOT_AN_IMAGE",
            "not-an-image",
            &[("mime", mime.to_string())],
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("DOWNLOAD_FAILED: {e}"))?;
    if bytes.is_empty() {
        return Err(coded_error(&app, "EMPTY_ASSET", "empty-asset-download"));
    }

    write_asset_to_note_folder(
        &app,
        &dest,
        &note_path,
        "Downloaded-image",
        &asset_ext_from_mime(&mime),
        &bytes,
    )
}

/// 用户从磁盘挑选的文件复制进笔记附件目录（PRD-06-012「插入图片 / 附件」，Obsidian 对标）。
///
/// 与粘贴落盘（`write_asset_to_note_folder`）走**同一条**目录推导（`attachment_target`），
/// 命名却不同：粘贴物没有名字，故按 `Pasted-image-<时间戳>` 造一个；用户挑的文件**有名字**，
/// 保留它（空白换连字符便于移植，PRD-06-030），只在同名时追加序号——`diagram.png` 进库后
/// 还叫 `diagram.png`，正文链接里读得出它是什么。
fn copy_asset_to_note_folder(
    app: &tauri::AppHandle,
    dest: &AssetDestination,
    note_path: &str,
    source: &Path,
) -> Result<String, String> {
    if note_path.contains("..") || note_path.starts_with('/') {
        return Err(coded_error(app, "INVALID_NOTE_PATH", "invalid-note-path"));
    }
    let mode = dest
        .settings
        .get_str("vault.attachment_location")
        .unwrap_or_default();
    let folder = dest
        .settings
        .get_str("vault.attachment_folder")
        .unwrap_or_default();
    let target_dir = dest
        .root
        .join(attachment_target(&mode, &folder, note_path, "x").dir_rel);
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("MKDIR_FAILED: {e}"))?;

    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .map(portable_asset_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Attachment".to_owned());
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()));
    let with_ext = |base: &str| match &ext {
        Some(ext) => format!("{base}.{ext}"),
        None => base.to_owned(),
    };
    let mut name = with_ext(&stem);
    let mut counter = 1u32;
    while target_dir.join(&name).exists() {
        name = with_ext(&format!("{stem}-{counter}"));
        counter += 1;
    }
    let target = attachment_target(&mode, &folder, note_path, &name);
    std::fs::copy(source, target_dir.join(&name)).map_err(|e| format!("COPY_FAILED: {e}"))?;
    emit_vault_changed_in_scope(
        app,
        &luna_vault::join_vault_rel(&target.dir_rel, &name),
        false,
        dest.scope,
    );
    Ok(target.link)
}

/// 可移植的附件文件名主干：空白 → 连字符，路径分隔符与控制字符剔除（PRD-06-030）。
fn portable_asset_name(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| !matches!(c, '/' | '\\' | ':') && !c.is_control())
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// 「插入图片 / 附件」（PRD-06-012）：原生**多选**文件对话框（Rust 侧，前端零业务）→ 逐个复制
/// 进笔记附件目录 → 返回**笔记相对**链接列表（顺序 = 用户选择顺序），供前端回填
/// `![](…)` / `[名](…)`。用户取消返回空列表；不加扩展名过滤——附件本就不限类型。
/// 一个文件复制失败即整批报错，已复制好的留在目录里（`vault://changed` 已广播，用户看得见）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_asset_files(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_files(move |paths| {
        let result: Result<Vec<std::path::PathBuf>, String> = paths
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.into_path().map_err(|e| e.to_string()))
            .collect();
        let _ = tx.send(result);
    });
    let sources = rx
        .await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))??;
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let dest = asset_destination(&state, window.label())?;
    sources
        .iter()
        .map(|source| copy_asset_to_note_folder(&app, &dest, &note_path, source))
        .collect()
}

/// 从系统拖进编辑器的文件（对标方案 B4）：与「插入图片 / 附件」同一条复制规则（附件目录推导、
/// 保留原文件名、同名才加序号），只是来源是拖放给的绝对路径而不是对话框。目录与不存在的
/// 路径跳过——拖进来一个文件夹不该把整个文件夹抄进库里。返回笔记相对链接，顺序同拖入。
#[tauri::command]
#[specta::specta]
pub(crate) fn import_asset_paths(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let dest = asset_destination(&state, window.label())?;
    paths
        .iter()
        .map(std::path::PathBuf::from)
        .filter(|source| source.is_file())
        .map(|source| copy_asset_to_note_folder(&app, &dest, &note_path, &source))
        .collect()
}

/// MIME → 文件扩展名（附件命名用）；未知 image/* 取子类型，非图片回退 `bin`。
pub(crate) fn asset_ext_from_mime(mime: &str) -> String {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        other => other
            .strip_prefix("image/")
            .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()) && !s.is_empty())
            .unwrap_or("bin"),
    }
    .to_owned()
}

/// 编辑器标签视图模式（PRD-06-010/011/017：Live Preview / 源码 / 阅读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EditorViewModeDto {
    Live,
    Source,
    Reading,
}

/// 工作区主区布局快照——单个标签（PRD-07-035）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspaceTabDto {
    /// 笔记 Vault 相对路径。
    pub path: String,
    pub mode: EditorViewModeDto,
    /// 固定标签（PRD-07-035 W2b）：「关闭其他 / 关闭右侧」不关它，打开别的笔记不替换它。
    ///
    /// `#[serde(default)]` 不是兼容层，与 `history` 同理：「没固定」是标签的常态，写出来就是
    /// `false`；导航历史条目复用本结构，恒为 `false`。
    #[serde(default)]
    pub pinned: bool,
}

/// 工作区主区布局快照——单个分屏 pane（PRD-07-036：各 pane 独立标签栈）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspacePaneDto {
    pub tabs: Vec<WorkspaceTabDto>,
    /// 活跃标签下标（越界时前端/加载端夹回）。
    pub active_tab: u32,
    /// 本 pane 的**导航历史**（PRD-07-037 / TDD-07 §4.1）：这一格先后显示过哪些笔记。
    ///
    /// **只存引用，不镜像正文**（同 `tabs`）：条目是「路径 + 视图模式」，正文仍由
    /// Rust 现读现给。历史镜像正文的代价是快照随笔记增长、且与磁盘上的那一份必然分叉。
    ///
    /// `#[serde(default)]` 不是兼容层：它表达的是「这一格是快照的可选部分」——
    /// 没有历史的 pane（刚开、只打开过一篇）写出来就是空列表。
    #[serde(default)]
    pub history: Vec<WorkspaceTabDto>,
    /// 历史游标：`history` 中「此刻正显示的那一条」的下标。
    ///
    /// **空历史时为 0 而不是 -1**：它与 `active_tab` 同为无符号下标，负值哨兵会逼
    /// 每个消费者各写一遍「-1 是什么意思」。空列表本身已经说清了「没有可后退的」。
    #[serde(default)]
    pub history_cursor: u32,
}

/// 主侧栏列宽合法区间（px；加载端剪枝夹回的**权威口径**，前端拖拽 clamp 同值）。
pub const SIDEBAR_WIDTH_MIN: u32 = 180;
pub const SIDEBAR_WIDTH_MAX: u32 = 600;

/// 工作区主区布局快照（TDD-07 §4.1；按库身份存在应用配置目录供重启恢复，ADR-140）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspaceLayoutDto {
    pub panes: Vec<WorkspacePaneDto>,
    /// 活跃 pane 下标（越界时夹回）。
    pub active_pane: u32,
    /// 主侧栏列宽（px；0 = 未定制走 CSS 默认；PRD-07-057 活动栏+主侧栏可拖拽调宽）。
    #[serde(default)]
    pub sidebar_width: u32,
}

/// 布局剪枝（纯逻辑，可测）：丢弃已不存在的笔记标签与空 pane，夹回活跃下标，
/// 树列宽夹回合法区间（0 保留 = 未定制）。
/// 全部标签都失效时返回 `None`（前端回落初始单 pane 布局）。
pub(crate) fn prune_workspace_layout(
    mut layout: WorkspaceLayoutDto,
    exists: impl Fn(&str) -> bool,
) -> Option<WorkspaceLayoutDto> {
    if layout.sidebar_width != 0 {
        layout.sidebar_width = layout
            .sidebar_width
            .clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
    }
    for pane in &mut layout.panes {
        // 活跃标签按对象身份跟随剪枝（下标在剪枝后会漂移）。
        let active_path = pane
            .tabs
            .get(pane.active_tab as usize)
            .map(|tab| tab.path.clone());
        pane.tabs.retain(|tab| {
            !tab.path.contains("..") && !tab.path.starts_with('/') && exists(&tab.path)
        });
        pane.active_tab = active_path
            .and_then(|path| pane.tabs.iter().position(|tab| tab.path == path))
            .unwrap_or_else(|| pane.tabs.len().saturating_sub(1)) as u32;
        // 历史同受剪枝：一条指向已删除笔记的后退记录，按下去只会打开一个空白标签，
        // 而用户读到的是「后退坏了」。游标按**对象身份**跟随（下标在剪枝后会漂移），
        // 与上面活跃标签那一步同一手法。
        let cursor_entry = pane
            .history
            .get(pane.history_cursor as usize)
            .map(|entry| entry.path.clone());
        pane.history.retain(|entry| {
            !entry.path.contains("..") && !entry.path.starts_with('/') && exists(&entry.path)
        });
        pane.history_cursor = cursor_entry
            .and_then(|path| pane.history.iter().position(|entry| entry.path == path))
            .unwrap_or_else(|| pane.history.len().saturating_sub(1))
            as u32;
    }
    layout.panes.retain(|pane| !pane.tabs.is_empty());
    if layout.panes.is_empty() {
        return None;
    }
    layout.active_pane = layout
        .active_pane
        .min(layout.panes.len().saturating_sub(1) as u32);
    Some(layout)
}

/// 主库的根与身份（布局与树状态只属于主窗口：副库窗口用自己的浏览树、不存布局）。
fn main_vault_identity(state: &AppState) -> Result<(PathBuf, luna_vault::VaultId), String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok((vault.root().to_path_buf(), vault.id()))
}

/// 读取工作区主区布局快照（PRD-07-035 重启恢复）：加载时对已删除/失效的
/// 笔记路径做剪枝（业务在 Rust，前端只渲染可用布局）。无快照或全失效返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) fn workspace_layout(
    state: tauri::State<'_, AppState>,
) -> Result<Option<WorkspaceLayoutDto>, String> {
    let (root, id) = main_vault_identity(&state)?;
    let Some(layout) = state
        .device_state
        .read::<WorkspaceLayoutDto>(id, DeviceStateKind::Workspace)
    else {
        return Ok(None);
    };
    Ok(prune_workspace_layout(layout, |rel| {
        root.join(rel).is_file()
    }))
}

/// 持久化工作区主区布局快照：设备状态，存在应用配置目录、按库身份分存（ADR-140），
/// 不进库。布局属 UI 会话状态而非笔记内容：**不**触发 `vault://changed`（避免高频布局
/// 变更反复驱动全库刷新）。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_workspace_layout(
    state: tauri::State<'_, AppState>,
    layout: WorkspaceLayoutDto,
) -> Result<(), String> {
    let (_, id) = main_vault_identity(&state)?;
    state
        .device_state
        .write(id, DeviceStateKind::Workspace, &layout)
}

/// 文件树排序方式（PRD-06-006：名称 / 修改时间 / 创建时间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TreeSortDto {
    #[default]
    Name,
    Mtime,
    Created,
}

/// 文件树的设备状态：排序方式 + 展开着的文件夹（库相对路径）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TreeViewStateDto {
    pub sort: TreeSortDto,
    pub expanded: Vec<String>,
}

/// 树状态剪枝（纯逻辑，可测）：丢掉已不存在的文件夹与越界路径、去重。
pub(crate) fn prune_tree_view_state(
    mut tree: TreeViewStateDto,
    is_dir: impl Fn(&str) -> bool,
) -> TreeViewStateDto {
    let mut seen = std::collections::HashSet::new();
    tree.expanded.retain(|path| {
        !path.split('/').any(|part| part == "..")
            && !path.starts_with('/')
            && is_dir(path)
            && seen.insert(path.clone())
    });
    tree
}

/// 读文件树的排序与展开状态（PRD-06-006 排序「记住」；ADR-140 设备状态）。没有就给默认。
#[tauri::command]
#[specta::specta]
pub(crate) fn tree_view_state(
    state: tauri::State<'_, AppState>,
) -> Result<TreeViewStateDto, String> {
    let (root, id) = main_vault_identity(&state)?;
    let tree = state
        .device_state
        .read::<TreeViewStateDto>(id, DeviceStateKind::Tree)
        .unwrap_or_default();
    Ok(prune_tree_view_state(tree, |rel| root.join(rel).is_dir()))
}

/// 存文件树的排序与展开状态（设备状态，不进库，不触发 `vault://changed`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_tree_view_state(
    state: tauri::State<'_, AppState>,
    tree: TreeViewStateDto,
) -> Result<(), String> {
    let (_, id) = main_vault_identity(&state)?;
    state.device_state.write(id, DeviceStateKind::Tree, &tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// 副库窗口的附件落副库，并按副库自己的附件位置设置推导目录（PRD-06-050）。
    #[tokio::test]
    async fn asset_destination_follows_the_window_vault_and_its_own_settings() {
        let config_dir = tempfile::tempdir().unwrap();
        let main_dir = tempfile::tempdir().unwrap();
        let side_dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set(
            "storage.vault_root",
            main_dir.path().to_string_lossy().as_ref(),
        );
        let state = crate::ipc::providers::build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(config_dir.path().join("config.toml")),
            luna_vault::UsageStore::open(config_dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        let side = luna_vault::Vault::open(side_dir.path()).unwrap();
        let mut side_settings = Settings::with_defaults();
        side_settings.set("vault.attachment_location", "specified_folder");
        side_settings.set("vault.attachment_folder", "Media");
        side_settings
            .save_path_for_scope(&vault_settings_path(side.root()), SettingScope::Vault)
            .unwrap();
        state
            .secondary_vaults
            .lock()
            .unwrap()
            .insert("vault-window-0".into(), Arc::new(Mutex::new(side)));

        let main = asset_destination(&state, "main").unwrap();
        assert_eq!(main.root, std::fs::canonicalize(main_dir.path()).unwrap());
        assert_eq!(main.scope, VaultChangeScope::ActiveVault);
        assert_ne!(
            main.settings.get_str("vault.attachment_folder").as_deref(),
            Some("Media"),
            "主库不该读到副库的设置"
        );

        let side = asset_destination(&state, "vault-window-0").unwrap();
        assert_eq!(side.root, std::fs::canonicalize(side_dir.path()).unwrap());
        assert_eq!(side.scope, VaultChangeScope::SecondaryVault);
        assert_eq!(
            side.settings
                .get_str("vault.attachment_location")
                .as_deref(),
            Some("specified_folder")
        );
        assert_eq!(
            side.settings.get_str("vault.attachment_folder").as_deref(),
            Some("Media")
        );

        // 已关掉的副库窗口：报失效，绝不回落主库。
        assert!(asset_destination(&state, "vault-window-9").is_err());
    }

    #[test]
    fn tree_view_state_keeps_only_live_folders_inside_the_vault() {
        let tree = TreeViewStateDto {
            sort: TreeSortDto::Created,
            expanded: vec![
                "甲".into(),
                "甲".into(),
                "已删".into(),
                "../外面".into(),
                "/etc".into(),
                "甲/乙".into(),
            ],
        };
        let got = prune_tree_view_state(tree, |path| path != "已删");
        assert_eq!(got.sort, TreeSortDto::Created);
        assert_eq!(got.expanded, vec!["甲".to_owned(), "甲/乙".to_owned()]);
    }

    #[test]
    fn pending_navigation_is_taken_exactly_once() {
        let pending = PendingNavigation::default();
        assert_eq!(pending.take(), None);
        pending.set("settings://account".to_owned());
        // 挂载时那一次取与事件触发的那一次取，只有先到的拿得到。
        assert_eq!(pending.take().as_deref(), Some("settings://account"));
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn a_later_navigation_replaces_one_not_yet_taken() {
        let pending = PendingNavigation::default();
        pending.set("settings://shortcuts".to_owned());
        pending.set("settings://account".to_owned());
        assert_eq!(pending.take().as_deref(), Some("settings://account"));
        assert_eq!(pending.take(), None);
    }

    /// 取出 `marker` 起那个函数的函数体（按花括号配平）。
    fn fn_body<'a>(src: &'a str, marker: &str) -> &'a str {
        let start = src
            .find(marker)
            .unwrap_or_else(|| panic!("找不到 {marker}"));
        let open = start + src[start..].find('{').expect("函数体");
        let mut depth = 0usize;
        for (idx, ch) in src[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &src[open..=open + idx];
                    }
                }
                _ => {}
            }
        }
        panic!("{marker} 的花括号不配平");
    }

    /// 后端发起的导航只有一条路：先存再通知（2026-10-15 Windows 真机，冷启动由连接链接拉起时
    /// 只发事件 = 去账户页那一跳丢了）。任何一处绕开 `request_workspace_navigation` 直接发
    /// `WorkspaceNavigate`，冷启动就又会丢。
    #[test]
    fn backend_navigation_is_stored_before_it_is_announced() {
        let workspace = crate::ipc::source_without_tests(include_str!("workspace.rs"));
        let request = fn_body(&workspace, "pub(crate) fn request_workspace_navigation(");
        let stored = request
            .find("pending_navigation.set(")
            .expect("请求必须先存");
        let announced = request
            .find("WorkspaceNavigate {}.emit(")
            .expect("请求必须通知");
        assert!(stored < announced, "先存再通知：事件先到时界面要取得到");
        assert!(
            fn_body(&workspace, "pub(crate) fn open_workspace_route(")
                .contains("request_workspace_navigation(&app, route)"),
            "辅助窗口唤起主窗的导航必须走同一条存储路径"
        );
        let account = crate::ipc::source_without_tests(include_str!("account.rs"));
        assert!(
            fn_body(&account, "pub(crate) fn handle_incoming_deep_link(")
                .contains("request_workspace_navigation(app, \"settings://account\""),
            "连接链接送达（含冷启动 get_current）必须把去账户页存下来，而不是只发事件"
        );
        // 发事件的地方只有那一处（结构体定义之外）。
        let lib = crate::ipc::source_without_tests(include_str!("../lib.rs"));
        for (name, src) in [
            ("workspace.rs", &workspace),
            ("account.rs", &account),
            ("lib.rs", &lib),
        ] {
            let emits = src.matches("WorkspaceNavigate {").count()
                - src.matches("struct WorkspaceNavigate {").count();
            let expected = usize::from(name == "workspace.rs");
            assert_eq!(emits, expected, "{name} 里有绕开存储直接发导航事件的地方");
        }
    }
}
