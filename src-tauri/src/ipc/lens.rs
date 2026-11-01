//! 桌面取词与截屏取词命令域（PRD-01，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：桌面取词浮窗的显隐、编辑态、几何持久化与取词捕获，以及截屏选区的
//! 确认/取消回调。窗口本身由 [`crate::screenshot`] 与平台层承载，本模块只做 IPC 面。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 返回译境当前是否运行。运行态不等同于镜框可见：单次拍摄时镜框会暂时隐藏。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_lens_state(state: tauri::State<'_, AppState>) -> bool {
    state.desktop_lens_active()
}

/// 切换固定的译境取景器。静默时镜框鼠标穿透，用户须点击贴边触发器才进入调整态。
/// 关闭会同步取消本会话正在执行的 OCR，不能被完成回调重新唤醒。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_lens_toggle(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    if !state.desktop_lens_enabled() {
        return Err("请先在「设置 → 实验室」启用译境。".to_string());
    }
    if state.desktop_lens_active() {
        crate::hide_desktop_lens(&app);
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    {
        if !crate::platform::macos::permissions::has_screen_capture_access() {
            let _ = crate::platform::macos::permissions::request_screen_capture_access();
            return Err("需要系统的屏幕录制权限才能启用译境".to_string());
        }
        crate::show_desktop_lens(&app);
        Ok(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        Err("当前平台尚未实现译境".to_string())
    }
}

/// 触发器悬停/编辑完成共用的状态切换；鼠标命中策略只在 Rust/NSPanel 层变更。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_lens_set_editing(app: tauri::AppHandle, editing: bool) -> Result<(), String> {
    crate::set_desktop_lens_editing(&app, editing);
    Ok(())
}

/// 持久化用户拖动/缩放后的固定取景器几何（**逻辑点**，单位铁律见 `show_desktop_lens`）。
/// 只保存坐标和尺寸，不保存任何屏幕内容。尺寸在此权威夹紧到最小值——前端拖拽同口径夹紧
/// 只为交互手感，落盘以本处为准。
///
/// **退出调整态与写盘成败解耦**：几何存不下（配置目录只读 / 损坏恢复态）不该把用户困在调整
/// 面板里，故无论成败都退出调整态；失败改由通知如实告知"本次位置不会被记住"，绝不静默——
/// 前端此后不再重复调用一次退出，退出时机由本命令单点拥有（ADR-003 业务在 Rust）。
#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_lens_save_geometry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    // 四个字段作为一个布局快照原子落盘：不能逐项写四次，否则进程中断会留下混合的新旧几何。
    let persisted = {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "Settings 锁中毒".to_string())?;
        let mut next = settings.clone();
        next.set("desktop_lens.placed", true);
        next.set("desktop_lens.x", i64::from(x));
        next.set("desktop_lens.y", i64::from(y));
        next.set(
            "desktop_lens.width",
            i64::from(width).max(crate::DESKTOP_LENS_MIN_WIDTH_POINTS),
        );
        next.set(
            "desktop_lens.height",
            i64::from(height).max(crate::DESKTOP_LENS_MIN_HEIGHT_POINTS),
        );
        // 只有写盘成功才把新几何采纳进内存态，避免内存与磁盘分叉。
        persist_device_settings(&next, &state.settings_persistence).map(|()| {
            *settings = next;
        })
    };
    crate::set_desktop_lens_editing(&app, false);
    if let Err(error) = persisted {
        if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
            center.publish(
                crate::notifications::NotificationEvent::desktop_lens_geometry_not_saved(),
                &app,
            );
        }
        return Err(error);
    }
    Ok(())
}

/// 按当前取景范围拍一张：隐藏两块 Lunarca 面板 → 一帧截图 → 进入既有 OCR → 弹窗翻译主干。
///
/// 控制条相机键与全局捕捉快捷键（`shortcuts.desktop_lens_capture`）**共用本函数**，两条
/// 入口不得各自编排一遍采集（ADR-001 单一主干）。
pub(crate) async fn run_desktop_lens_capture(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager as _;
    let lens = app
        .get_webview_window(crate::DESKTOP_LENS_WINDOW)
        .ok_or_else(|| "译境尚未创建".to_string())?;
    // 取景框几何已是逻辑点（见 `show_desktop_lens` 的单位铁律），而 `screencapture -R` 要的
    // 正是全局逻辑点：此处不做任何换算，混合 DPI 下也就无从错位。
    let (position, size) =
        crate::window_logical_geometry(&lens).ok_or_else(|| "译境几何不可读".to_string())?;
    let _ = lens.hide();
    if let Some(trigger) = app.get_webview_window(crate::DESKTOP_LENS_TRIGGER) {
        let _ = trigger.hide();
    }
    crate::screenshot::capture_fixed_region(
        app,
        crate::screenshot::SelectionRect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        },
    )
    .await;
    Ok(())
}

/// 取景器的显式拍摄按钮。
#[tauri::command]
#[specta::specta]
pub(crate) async fn desktop_lens_capture(app: tauri::AppHandle) -> Result<(), String> {
    run_desktop_lens_capture(app).await
}

#[tauri::command]
#[specta::specta]
pub(crate) fn desktop_lens_hide(app: tauri::AppHandle) -> Result<(), String> {
    crate::hide_desktop_lens(&app);
    Ok(())
}

/// 框选层确认（松开鼠标即确认，Snipaste 式，TDD-05 §1.5）：坐标为**发起面板**窗口内
/// CSS 逻辑像素（多屏每屏一面板），全局换算/截屏/OCR/进弹窗全在 Rust（ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn screenshot_overlay_confirm(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    crate::screenshot::confirm_region(
        app,
        window,
        crate::screenshot::SelectionRect {
            x,
            y,
            width,
            height,
        },
    )
    .await;
    Ok(())
}

/// 框选层取消（右键；Esc 走 CGEventTap 同一收口）。
#[tauri::command]
#[specta::specta]
pub(crate) fn screenshot_overlay_cancel(app: tauri::AppHandle) -> Result<(), String> {
    crate::screenshot::cancel_overlay(&app);
    Ok(())
}
