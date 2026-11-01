//! 系统入口命令域（外部打开 / 文件管理器揭示 / 权限，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：把路径或 URL 交给系统去打开，以及 TCC 权限的查询与申请。
//! 这一域的每个命令都跨出应用边界，故各自带白名单/库内校验，不接受任意路径。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 在系统默认浏览器打开外部链接（对话/笔记 Markdown 外链点击）。
///
/// **协议白名单**：仅 `http`/`https`——命令可被任意窗口调用，白名单挡住 `file:`/自定义
/// scheme 注入（TDD-12 §4 提示注入护栏同精神）。默认离线纪律不受影响：这是用户显式
/// 点击的**外开**动作，应用自身不发起网络请求。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        return Err(coded_error_args(
            &app,
            "URL_REJECTED",
            "url-rejected-external",
            &[("url", trimmed.to_string())],
        ));
    }
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("/usr/bin/open")
        .arg(trimmed)
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", trimmed])
        .status();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let status = std::process::Command::new("xdg-open").arg(trimmed).status();
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("打开浏览器失败：exit={code}")),
        Err(e) => Err(format!("打开浏览器失败：{e}")),
    }
}

/// 用系统默认程序打开一个 **Vault 内既有文件**（PRD-06-007：非可预览二进制的「外部打开」）。
/// 经 `resolve_vault_path` 校验路径落在 Vault 根内且文件存在（防越界/穿越）；仅打开本地文件，
/// 应用自身不发起网络请求（离线纪律不受影响）。
fn open_local_path(abs: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("/usr/bin/open")
        .arg(abs)
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(abs)
        .status();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let status = std::process::Command::new("xdg-open").arg(abs).status();
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("打开文件失败：exit={code}")),
        Err(e) => Err(format!("打开文件失败：{e}")),
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn open_vault_path(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let abs = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        crate::media_server::resolve_vault_path(vault.root(), &path).map_err(|_| {
            coded_error_args_with(
                state.copy(),
                "PATH_REJECTED",
                "path-rejected-file",
                &[("path", path.clone())],
            )
        })?
    };
    open_local_path(&abs)
}

/// 在系统文件管理器中**定位并选中** Vault 内的某个文件/目录（PRD-07-059 右键菜单）。
///
/// 与 `open_vault_path`（用默认程序打开内容）分工明确：这里要的是"我要拿这个文件去别处"，
/// 「笔记即文件」的产品承诺必须有一条直达磁盘的路（Obsidian「在系统文件管理器中显示」同）。
/// 路径校验同样经 `resolve_vault_path`（越界/不存在一律拒绝）。
fn reveal_local_path(abs: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let outcome = match std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(abs)
        .status()
    {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("定位文件失败：exit={code}")),
        Err(e) => Err(format!("定位文件失败：{e}")),
    };
    // `explorer /select,<path>` 成功时也返回非零退出码（Windows 既有行为），
    // 故只判进程能否启动，不判退出码——否则每次成功都会报成失败。
    #[cfg(target_os = "windows")]
    let outcome = match std::process::Command::new("explorer")
        .arg(format!("/select,{}", abs.display()))
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("定位文件失败：{e}")),
    };
    // 各桌面环境没有统一的"选中某文件"协议，退一步打开其所在目录（诚实降级）。
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let outcome = match std::process::Command::new("xdg-open")
        .arg(abs.parent().unwrap_or(abs))
        .status()
    {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => Err(format!("定位文件失败：exit={code}")),
        Err(e) => Err(format!("定位文件失败：{e}")),
    };
    outcome
}

/// 解析 Vault 内的**既有条目**（文件**或目录**）为绝对路径。
///
/// 与 [`crate::media_server::resolve_vault_path`] 只差一处：允许目录。「在系统文件管理器中
/// 显示」对文件夹节点与 Vault 根同样成立，而媒体服务那条路只服务文件（目录不是可播放的
/// 媒体，那里的 `is_file` 是刻意的）。越界判据不变：规范化后必须落在库根内。
pub(crate) fn resolve_vault_entry(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    for comp in rel_path.components() {
        match comp {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err("path escapes vault".into());
            }
            _ => {}
        }
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("vault root is not canonical: {e}"))?;
    let target = root
        .join(rel_path)
        .canonicalize()
        .map_err(|e| format!("path is not canonical: {e}"))?;
    if !target.starts_with(&root) {
        return Err("path escapes vault".into());
    }
    Ok(target)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn reveal_vault_path(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let abs = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        resolve_vault_entry(vault.root(), &path).map_err(|_| {
            coded_error_args_with(
                state.copy(),
                "PATH_REJECTED",
                "path-rejected-entry",
                &[("path", path.clone())],
            )
        })?
    };
    reveal_local_path(&abs)
}

/// 在系统文件管理器里打开**系统回收站**（PRD-06-009：`trash` 策略下不在应用内重复实现）。
///
/// **这是"回收站"这条安全网在默认策略下的唯一落点**：`vault.delete_mode` 默认是系统
/// 废纸篓，删掉的东西根本不进库内 `.trash/`，应用内再画一份列表既是重复实现、也会与
/// 访达的真实内容分叉（用户在访达里还原了，我们的列表并不知道）。故这一格只负责把人
/// 送到系统那一处。
///
/// 不接受任何入参——目标由平台固定（macOS `~/.Trash`、Windows 回收站、其余 `trash:///`），
/// 没有可被注入的路径面。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_system_trash(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let outcome = {
        let trash = app
            .path()
            .home_dir()
            .map_err(|e| format!("打开系统废纸篓失败：{e}"))?
            .join(".Trash");
        match std::process::Command::new("/usr/bin/open")
            .arg(&trash)
            .status()
        {
            Ok(code) if code.success() => Ok(()),
            Ok(code) => Err(format!("打开系统废纸篓失败：exit={code}")),
            Err(e) => Err(format!("打开系统废纸篓失败：{e}")),
        }
    };
    // `explorer` 打开 shell 文件夹时同样返回非零退出码（与 `reveal_local_path` 同因），
    // 故只判进程能否启动。
    #[cfg(target_os = "windows")]
    let outcome = {
        let _ = &app;
        match std::process::Command::new("explorer")
            .arg("shell:RecycleBinFolder")
            .spawn()
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("打开系统回收站失败：{e}")),
        }
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let outcome = {
        let _ = &app;
        match std::process::Command::new("xdg-open")
            .arg("trash:///")
            .status()
        {
            Ok(code) if code.success() => Ok(()),
            Ok(code) => Err(format!("打开系统回收站失败：exit={code}")),
            Err(e) => Err(format!("打开系统回收站失败：{e}")),
        }
    };
    outcome
}

/// 显式打开用户模板目录。只有这个用户操作会创建缺失目录，模板枚举保持严格只读。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_user_templates_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings_snapshot();
    let dir = user_templates_dir(&settings);
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| state.vault_not_open())?
            .root()
            .to_path_buf()
    };
    let absolute = root.join(dir);
    std::fs::create_dir_all(&absolute).map_err(|error| format!("创建模板目录失败：{error}"))?;
    open_local_path(&absolute)
}

/// 查询权限状态 + 降级动作 + 中文引导（TDD-05 §4）。前端据此展示状态与引导（Phase 6 设置 UI 消费）。
#[tauri::command]
#[specta::specta]
pub(crate) fn check_permission(
    kind: crate::platform::permissions::PermissionKind,
) -> crate::platform::permissions::PermissionReport {
    #[cfg(target_os = "macos")]
    let status = {
        use crate::platform::permissions::PermissionManager as _;
        crate::platform::macos::MacPermissionManager.check(kind)
    };
    #[cfg(not(target_os = "macos"))]
    let status = crate::platform::permissions::PermissionStatus::Unsupported;
    crate::platform::permissions::report(kind, status)
}

/// 请求授权（尽力而为：辅助功能弹系统框 + 打开「系统设置 › 隐私与安全性」对应面板引导手动开启）。
#[tauri::command]
#[specta::specta]
pub(crate) fn request_permission(kind: crate::platform::permissions::PermissionKind) {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::permissions::PermissionManager as _;
        crate::platform::macos::MacPermissionManager.request(kind);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = kind;
}
