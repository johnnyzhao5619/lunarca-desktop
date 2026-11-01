//! 非激活浮层（ADR-163 ①，ADR-015 在 Windows 上的对应物）：HUD 与弹窗翻译窗不夺焦。
//!
//! - `WS_EX_NOACTIVATE` + `SW_SHOWNOACTIVATE`：由 tao 的 `focusable(false)` + `focused(false)` 给出——
//!   tao 每次改窗口标志都会按它自己的标志表重写扩展样式，所以这两位必须走 tao 的标志，不能自己写。
//! - `WS_EX_TOPMOST`：`always_on_top(true)`。
//! - `WS_EX_TOOLWINDOW`（不进 Alt+Tab）：tao 没有对应标志，自己写上去会在下一次 show / hide 时被
//!   tao 重写掉。故给窗口挂一个子类过程，在 `WM_STYLECHANGING` 里把它补回去——不管谁改样式都保住。
//! - 子类过程同时在 `WM_MOUSEACTIVATE` 上答 `MA_NOACTIVATE`（窗口不可激活时）：点按钮不激活面板。
//!
//! 面板里的输入框要打字就得激活（Windows 的键盘只给前台窗口）：前端经 `panel_text_editing`
//! 上报编辑态，Rust 把面板临时改成可激活并激活它，离开输入框再改回去；源窗口因此让出前台，
//! 注入前由 `injector` 把前台还回去再核对（`routing::decide_inject_route`）。
//!
//! 判定是纯函数，在任何平台上单测；系统调用只在 `cfg(target_os = "windows")` 的薄层里。

pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
/// `WM_MOUSEACTIVATE` 的回答：不激活，点击照常送达。
pub const MA_NOACTIVATE: isize = 3;

/// 浮层的扩展样式：在任何人请求的样式上补 `WS_EX_TOOLWINDOW`、去掉 `WS_EX_APPWINDOW`
/// （后者会强行把窗口放进任务栏与 Alt+Tab）。
#[must_use]
pub fn panel_ex_style(requested: u32) -> u32 {
    (requested | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
}

/// 鼠标点在浮层上时怎么答 `WM_MOUSEACTIVATE`：窗口此刻不可激活就答 `MA_NOACTIVATE`，
/// 可激活（正在编辑输入框）就交给默认处理。
#[must_use]
pub fn mouse_activate_reply(ex_style: u32) -> Option<isize> {
    (ex_style & WS_EX_NOACTIVATE != 0).then_some(MA_NOACTIVATE)
}

#[cfg(target_os = "windows")]
pub use imp::{install, is_panel};

#[cfg(target_os = "windows")]
mod imp {
    use super::{mouse_activate_reply, panel_ex_style};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, STYLESTRUCT, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_MOUSEACTIVATE, WM_NCDESTROY,
        WM_STYLECHANGING,
    };

    /// 子类过程的 id（同一窗口上与 tao / wry 自己的子类区分开）。
    const SUBCLASS_ID: usize = 0x4C55_4E41; // "LUNA"

    /// 已转成浮层的窗口（`panel_text_editing` 据此只对浮层生效，主窗口 / 编辑器窗不受影响）。
    static PANELS: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

    /// 窗口是否已由 [`install`] 转成浮层。
    #[must_use]
    pub fn is_panel(hwnd: isize) -> bool {
        PANELS
            .lock()
            .ok()
            .is_some_and(|set| set.as_ref().is_some_and(|set| set.contains(&hwnd)))
    }

    /// 把窗口转成非激活浮层的样式守卫（须在主线程、窗口所属线程上调用）。
    pub fn install(hwnd: isize) -> bool {
        let window = HWND(hwnd as *mut std::ffi::c_void);
        // SAFETY: 窗口由本线程创建且存活；子类过程是本模块的 extern "system" 函数。
        let installed = unsafe {
            if !SetWindowSubclass(window, Some(subclass_proc), SUBCLASS_ID, 0).as_bool() {
                return false;
            }
            // 立即生效一次（之后的每次改样式由 WM_STYLECHANGING 守住）。
            let current = GetWindowLongW(window, GWL_EXSTYLE) as u32;
            SetWindowLongW(window, GWL_EXSTYLE, panel_ex_style(current) as i32);
            let _ = SetWindowPos(
                window,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
            true
        };
        if let Ok(mut set) = PANELS.lock() {
            set.get_or_insert_with(HashSet::new).insert(hwnd);
        }
        installed
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_STYLECHANGING if wparam.0 as i32 == GWL_EXSTYLE.0 => {
                // SAFETY: WM_STYLECHANGING 的 lparam 指向系统提供、可改写的 STYLESTRUCT。
                let style = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
                style.styleNew = panel_ex_style(style.styleNew);
            }
            WM_MOUSEACTIVATE => {
                // SAFETY: 读本窗口自己的扩展样式。
                let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
                if let Some(reply) = mouse_activate_reply(ex) {
                    return LRESULT(reply);
                }
            }
            WM_NCDESTROY => {
                if let Ok(mut set) = PANELS.lock() {
                    if let Some(set) = set.as_mut() {
                        set.remove(&(hwnd.0 as isize));
                    }
                }
                // SAFETY: 窗口销毁前摘掉本子类。
                unsafe {
                    let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                }
            }
            _ => {}
        }
        // SAFETY: 原样交给下一层窗口过程。
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_style_keeps_requested_bits_adds_tool_window_and_drops_app_window() {
        let requested = WS_EX_NOACTIVATE | WS_EX_APPWINDOW | 0x0000_0100;
        let styled = panel_ex_style(requested);
        assert_ne!(styled & WS_EX_TOOLWINDOW, 0, "不进 Alt+Tab");
        assert_eq!(styled & WS_EX_APPWINDOW, 0);
        assert_ne!(styled & WS_EX_NOACTIVATE, 0, "tao 给的不可激活位原样保留");
        assert_ne!(styled & 0x0000_0100, 0);
        assert_eq!(panel_ex_style(styled), styled, "幂等");
    }

    #[test]
    fn clicks_do_not_activate_a_non_activating_panel() {
        assert_eq!(
            mouse_activate_reply(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW),
            Some(MA_NOACTIVATE)
        );
        assert_eq!(
            mouse_activate_reply(WS_EX_TOOLWINDOW),
            None,
            "编辑输入框期间面板可激活：交给默认处理"
        );
    }
}
