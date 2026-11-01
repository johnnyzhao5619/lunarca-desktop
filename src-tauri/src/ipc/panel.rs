//! 非激活面板的通用窗口态命令（ADR-015，IPC 面）。
//!
//! 与 `popup.rs` / `recording.rs` 等按业务纵切的域不同，这里只放**所有面板共用**的
//! 窗口层行为；目前一条：文本编辑期的层级让位。

/// 面板内文本框获得/失去焦点时调用（TDD-05 §5.1.1）：编辑中把面板降到
/// `NSFloatingWindowLevel`，让输入法候选窗浮在面板之上；失焦回 `NSStatusWindowLevel`。
///
/// Windows（ADR-163 ①）：键盘只给前台窗口，非激活面板里的输入框收不到键入——编辑中把面板
/// 临时改成可激活并激活它，离开输入框再改回不可激活。源窗口因此让出前台，注入前由注入器
/// 把前台还回去再核对。
///
/// 以**调用方窗口**为目标；非面板窗口（主窗口、编辑器窗）在平台层是 no-op，
/// 所以共用的输入组件（快捷对话 composer 同时挂在主窗口与 quick-chat 面板）可以无差别调用。
/// 其它平台为 no-op。
#[tauri::command]
#[specta::specta]
pub(crate) fn panel_text_editing(
    window: tauri::WebviewWindow,
    editing: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        window
            .run_on_main_thread(move || {
                // SAFETY: run_on_main_thread 保证主线程；ns_window 取自存活的 WebviewWindow。
                unsafe {
                    crate::platform::macos::set_text_editing_level(
                        ns_window as *mut std::ffi::c_void,
                        editing,
                    );
                }
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        if !crate::platform::windows::panel::is_panel(hwnd.0 as isize) {
            return Ok(());
        }
        window.set_focusable(editing).map_err(|e| e.to_string())?;
        if editing {
            window.set_focus().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (&window, editing);
        Ok(())
    }
}

/// 收起文字速记面板（PRD-06-058 B″ 的 Esc / 关闭键）。
///
/// 与 `quick_chat_close` / `voice_input_close` 并列而不是抽一个通用的 `panel_hide`：那两条
/// 命令的名字带着会话语义（关掉这一场对话 / 结束这次语音输入），今天恰好只做了 hide；
/// 合并成「隐藏调用方窗口」等于把三种收尾语义压成同一个名字，而它们迟早会分叉。这一条
/// 只隐藏窗口——这扇窗没有会话，未存的文本留在前端 state 里，下次唤起时还在。
#[tauri::command]
#[specta::specta]
pub(crate) fn text_quick_note_close(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window(crate::TEXT_QUICK_NOTE_WINDOW) {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 跨档跃迁（ADR-112 ③）：把调用方窗口的高度设为该档首帧高（宽沿用当前），再按所在屏 clamp
/// 左上角，保证整窗仍完整可见。几何表在 [`crate::hud_tier_height`]；档位由前端按会话状态
/// 与用户动作（展开 / 收起）决定，但**尺寸、最小值与多屏 clamp 只在这里**。
///
/// 真机 spike（2026-09-08，ADR-112 ⑥ ⑴）：非激活 NSPanel `set_size` 原点不漂（左上角锚定）、
/// 固定态同样；尺寸在下一轮才生效——故 clamp 用**请求的**尺寸算，不读 `outer_size`。
#[tauri::command]
#[specta::specta]
pub(crate) fn hud_set_tier(window: tauri::WebviewWindow, tier: String) -> Result<(), String> {
    let label = window.label().to_owned();
    let Some(height) = crate::hud_tier_height(&label, &tier) else {
        return Err(format!("{label} 没有登记档位 {tier}"));
    };
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let width = f64::from(inner.width) / scale;
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
        .map_err(|e| e.to_string())?;
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let current = crate::popup::ScreenPoint::new(f64::from(pos.x), f64::from(pos.y));
    let clamped = crate::popup::clamp_saved_position(
        current,
        crate::popup::ScreenSize::new(width * scale, height * scale),
        &crate::collect_monitors(&window),
    );
    if clamped != current {
        window
            .set_position(tauri::PhysicalPosition::new(
                clamped.x as i32,
                clamped.y as i32,
            ))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
