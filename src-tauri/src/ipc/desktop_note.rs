//! 桌面便签的 IPC 面（ADR-119 / PRD-06-058 F）。
//!
//! 便签是速记的第二个呈现面，不是第二个对象：这里的每一条命令要么改 `luna.desktop_note`
//! 这一个 frontmatter 键，要么开关一扇窗，绝不产生第二份存储。
//!
//! 正文与 frontmatter 的拆合全在这一层：便签窗只收发一段正文，前端不认识 frontmatter
//! （ADR-003）。

use super::{coded_error, coded_error_args, AppState};

/// 便签窗口 label 前缀（与 `App.tsx` / capabilities 通配 `desktop-note-*` 同源）。
pub(crate) const DESKTOP_NOTE_WINDOW_PREFIX: &str = "desktop-note-";

/// 一张便签此刻的内容与外观（便签窗挂载时取一次）。
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct DesktopNoteDto {
    /// 这一篇的相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 正文（已剥 frontmatter）。
    pub body: String,
    /// 配色键；`null` = 默认色。
    pub color: Option<String>,
}

/// 便签窗问「我是哪一篇」，并把内容一次取回。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_content(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<DesktopNoteDto, String> {
    let label = window.label().to_owned();
    let path = state
        .desktop_note_path_of(&label)
        .ok_or_else(|| coded_error(&app, "NOTE_NOT_FOUND", "note-not-found"))?;
    state.with_vault("main", |vault| {
        let doc = vault.read(&path).map_err(super::vault_err)?;
        Ok(DesktopNoteDto {
            title: super::workspace::note_title_of(&path),
            // 只给人写的那一段（转写块等机器段不进便签，写回时由 `replace_body` 原样接回）。
            body: luna_vault::sticky_text(luna_vault::body_of(&doc.content))
                .0
                .trim_start()
                .to_owned(),
            color: doc.meta.desktop_note.and_then(|d| d.color),
            path: path.clone(),
        })
    })
}

/// 便签里改了字：只覆盖正文，frontmatter 逐字节保留。
///
/// `base` = 便签以为磁盘上此刻是什么。对不上一个字节都不写，返回 `false` 让调用方
/// 重读——真机 2026-09-09：没有这道基线时，前端只要有一帧拿着陈旧的空串，用户整篇正文
/// 就没了。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_write_body(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    base: String,
    body: String,
) -> Result<bool, String> {
    let label = window.label().to_owned();
    let path = state
        .desktop_note_path_of(&label)
        .ok_or_else(|| coded_error(&app, "NOTE_NOT_FOUND", "note-not-found"))?;
    let written = state.with_vault("main", |vault| {
        vault
            .replace_body(&path, &base, &body)
            .map_err(super::vault_err)
    })?;
    if written {
        super::emit_vault_changed(&app, &path, false);
    }
    Ok(written)
}

/// 记下便签被拖到哪、拉成多大（ADR-119 ②）。值没变时 Vault 侧不写盘。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_save_frame(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let Some(path) = state.desktop_note_path_of(&label) else {
        // 窗已被摘除（正在关）——这一次坐标没有归属，静默丢弃即可。
        return Ok(());
    };
    state.with_vault("main", |vault| {
        let current = vault
            .read(&path)
            .map_err(super::vault_err)?
            .meta
            .desktop_note
            .unwrap_or_default();
        vault
            .set_desktop_note(
                &path,
                Some(luna_vault::DesktopNote {
                    x: Some(x),
                    y: Some(y),
                    w: Some(w),
                    h: Some(h),
                    color: current.color,
                    // 记坐标的前提就是它此刻开着，故恒为钉住。
                    pinned: true,
                }),
            )
            .map_err(super::vault_err)
    })?;
    let _ = &app;
    Ok(())
}

/// 换一张便签的配色。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_set_color(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    color: Option<String>,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let path = state
        .desktop_note_path_of(&label)
        .ok_or_else(|| coded_error(&app, "NOTE_NOT_FOUND", "note-not-found"))?;
    state.with_vault("main", |vault| {
        let mut desktop = vault
            .read(&path)
            .map_err(super::vault_err)?
            .meta
            .desktop_note
            .unwrap_or_default();
        desktop.color = color.clone();
        vault
            .set_desktop_note(&path, Some(desktop))
            .map_err(super::vault_err)
    })?;
    super::emit_vault_changed(&app, &path, false);
    Ok(())
}

/// 把一篇速记钉到桌面（已钉住 = 聚焦那一扇，不开第二张）。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_pin(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    use tauri::Manager as _;
    if !path.ends_with(".md") {
        return Err(coded_error(&app, "NOT_A_NOTE", "not-a-note"));
    }
    // 已经钉住了就把那一扇拿到眼前——再开一张同一篇的便签，两扇窗会互相回灌。
    if let Some(label) = state.desktop_note_window_of(&path) {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.show();
            return Ok(());
        }
        state.forget_desktop_note_window(&label);
    }
    // 先落键再建窗：建窗失败时用户至少能在下次启动时看到它被恢复，反过来则是一扇
    // 关掉就再也回不来的窗。
    let existing = state.with_vault("main", |vault| {
        // 上一次的位置、尺寸与颜色**原样留用**：取消钉住只是把 `pinned` 放倒，那些属性
        // 一直在（真机 2026-09-09：此前取消钉住会连颜色一起抹掉，再钉回来是一张全新的
        // 默认色便签）。
        let mut desktop = vault
            .read(&path)
            .map_err(super::vault_err)?
            .meta
            .desktop_note
            .unwrap_or_default();
        desktop.pinned = true;
        vault
            .set_desktop_note(&path, Some(desktop.clone()))
            .map_err(super::vault_err)?;
        Ok(desktop)
    })?;
    let opened = open_desktop_note_window(&app, &state, &path, &existing)
        .map_err(|e| coded_error_args(&app, "WINDOW_FAILED", "window-failed", &[("error", e)]));
    // 「速记」行上的钉住态跟着变（真机 2026-09-09：关掉便签之后行右键仍写着「取消钉住」）。
    super::emit_vault_changed(&app, &path, false);
    opened
}

/// 取消钉住：删掉那个键并收起窗，笔记本身一个字不动。
///
/// 绝不 `close()`（真机 2026-09-09 复验第 5 条）：销毁一扇持有输入法会话的 NSPanel 会抛
/// ObjC 异常穿过 Rust（`IMKCFRunLoopWakeUpReliable` → `fatal runtime error: Rust cannot
/// catch foreign exceptions`），整个应用当场退出。仓库里其它面板一律只 `hide()`，本条此前
/// 是唯一走了 `close()` 的，也是唯一崩的。隐藏之后进空闲池，等下一次「钉到桌面」复用。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_note_unpin(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    use tauri::Manager as _;
    if let Some(label) = state.desktop_note_window_of(&path) {
        state.forget_desktop_note_window(&label);
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.hide();
            state.park_desktop_note_window(&label);
        }
    }
    state.with_vault("main", |vault| {
        // **不删这个键**：它同时记着位置、尺寸与颜色，而那些是笔记的属性，与"此刻在
        // 不在桌面上"是两件事。删掉的表现是用户摆好位置、挑好颜色，取消钉住再钉回来
        // 全部回到默认（真机 2026-09-09）。
        let mut desktop = vault
            .read(&path)
            .map_err(super::vault_err)?
            .meta
            .desktop_note
            .unwrap_or_default();
        desktop.pinned = false;
        vault
            .set_desktop_note(&path, Some(desktop))
            .map_err(super::vault_err)
    })?;
    super::emit_vault_changed(&app, &path, false);
    Ok(())
}

/// 让一扇便签窗显示某一篇（复用空闲窗，池子空了才新建）。
///
/// 坐标进屏由 `clamp_saved_position` 兜住（ADR-119 ③）。复用时要把窗口的内容也换过来
/// ——它的 React 只挂载一次，隐藏期间不会自己重读。
pub(crate) fn open_desktop_note_window(
    app: &tauri::AppHandle,
    state: &AppState,
    path: &str,
    desktop: &luna_vault::DesktopNote,
) -> Result<(), String> {
    use tauri::Manager as _;
    if let Some(label) = state.take_idle_desktop_note_window() {
        // 池子里那个 label 的窗可能已经不在了（应用退出中等）——那就照常新建。
        if let Some(win) = app.get_webview_window(&label) {
            state.remember_desktop_note_window(&label, path);
            crate::place_desktop_note_window(&win, desktop);
            let _ = win.show();
            // 换了一篇就让它重读：窗口是复用的，组件早就挂载完了。
            let _ = win.eval("window.__lunaReloadStickyNote && window.__lunaReloadStickyNote()");
            return Ok(());
        }
    }
    let seq = state
        .desktop_note_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{DESKTOP_NOTE_WINDOW_PREFIX}{seq}");
    state.remember_desktop_note_window(&label, path);
    crate::open_desktop_note_panel(app, &label, desktop).map_err(|e| {
        state.forget_desktop_note_window(&label);
        e.to_string()
    })
}
