//! 前台窗口事实（ADR-163）：读出 [`ForegroundFacts`] 交给 `routing::decide_inject_route` 判定，
//! 以及判定之后需要的两个动作（把前台还给目标窗口、读输入法开关）。

use super::routing::ForegroundFacts;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW,
    SetForegroundWindow, SMTO_ABORTIFHUNG, WM_IME_CONTROL,
};

/// `WM_IME_CONTROL` 的子命令（`imm.h`）。
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_GETOPENSTATUS: usize = 0x0005;
/// 转换模式里的「本地语言」位：中文输入法在中文状态时置位，切到英文状态时清零。
const IME_CMODE_NATIVE: usize = 0x0001;
/// 跨进程问输入法的超时：目标卡死时不能把注入一起拖住。
const IME_QUERY_TIMEOUT: Duration = Duration::from_millis(100);

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

/// 此刻的前台窗口。
#[must_use]
pub fn foreground_window() -> Option<isize> {
    // SAFETY: 无参数的只读查询。
    let fg = unsafe { GetForegroundWindow() };
    (!fg.is_invalid()).then_some(fg.0 as isize)
}

fn window_pid(raw: isize) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: 句柄可能已失效，系统此时返回 0 而不是崩溃。
    unsafe { GetWindowThreadProcessId(hwnd(raw), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// 窗口属于 Lunarca 自己的进程。
#[must_use]
pub fn is_own_window(raw: isize) -> bool {
    // SAFETY: 无参数的只读查询。
    window_pid(raw) == Some(unsafe { GetCurrentProcessId() })
}

/// 窗口类名（判断终端等，见 `routing::is_terminal_window_class`）。
#[must_use]
pub fn window_class(raw: isize) -> Option<String> {
    let mut buf = [0u16; 256];
    // SAFETY: 缓冲区有效；句柄失效时返回 0。
    let len = unsafe { GetClassNameW(hwnd(raw), &mut buf) };
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// 进程令牌的完整性级别（Low / Medium / High / System 的 RID）。
fn token_integrity(process: HANDLE) -> Option<u32> {
    // SAFETY: 标准的令牌查询：先问长度再取，缓冲区按返回长度分配，令牌句柄用完即关。
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
        let label = &*(buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

/// 目标窗口所属进程的权限比我们高（管理员窗口）。读不到对方令牌也按「高」算：
/// 普通进程读不了管理员进程的令牌，而合成按键送不进去的正是这类窗口。
#[must_use]
pub fn is_elevated_above_us(raw: isize) -> bool {
    let Some(pid) = window_pid(raw) else {
        return false;
    };
    // SAFETY: 只取查询权限的句柄，用完即关。
    let target = unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return true;
        };
        let level = token_integrity(process);
        let _ = CloseHandle(process);
        level
    };
    // SAFETY: 伪句柄，无需关闭。
    let own = token_integrity(unsafe { GetCurrentProcess() });
    match (target, own) {
        (Some(target), Some(own)) => target > own,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// 读出注入前核对要的全部事实。
#[must_use]
pub fn facts_for(target: Option<isize>) -> ForegroundFacts {
    let foreground = foreground_window();
    ForegroundFacts {
        foreground,
        foreground_is_own: foreground.is_some_and(is_own_window),
        // SAFETY: 只读查询句柄是否仍有效。
        target_alive: target.is_some_and(|t| unsafe { IsWindow(Some(hwnd(t))) }.as_bool()),
        target_elevated: target.is_some_and(is_elevated_above_us),
    }
}

/// 把前台还给目标窗口（只在前台是我们自己的窗口时调用：前台进程才有权转交前台）。
pub fn bring_to_front(raw: isize) -> bool {
    // SAFETY: 句柄失效时系统返回 FALSE。
    unsafe { SetForegroundWindow(hwnd(raw)) }.as_bool()
}

fn ime_query(ime_window: HWND, command: usize) -> Option<usize> {
    let mut result = 0usize;
    // SAFETY: 向输入法默认窗口发标准的 WM_IME_CONTROL 查询，带超时。
    let ok = unsafe {
        SendMessageTimeoutW(
            ime_window,
            WM_IME_CONTROL,
            WPARAM(command),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            IME_QUERY_TIMEOUT.as_millis() as u32,
            Some(&raw mut result),
        )
    };
    (ok.0 != 0).then_some(result)
}

/// 目标窗口的输入法是否处在「本地语言」输入状态（中文输入法的中文状态）。
///
/// 跨进程读不到「正在组字」，只读得到开关与转换模式，故按开着算（`routing::choose_inject_method`）。
/// 没有输入法窗口 = 没有输入法；问不出来（目标卡住）按开着算——粘贴在两种情况下都安全。
#[must_use]
pub fn ime_is_composing_language(raw: isize) -> bool {
    // SAFETY: 只读查询目标线程的默认输入法窗口。
    let ime_window = unsafe { ImmGetDefaultIMEWnd(hwnd(raw)) };
    if ime_window.is_invalid() {
        return false;
    }
    match ime_query(ime_window, IMC_GETOPENSTATUS) {
        Some(0) => false,
        Some(_) => ime_query(ime_window, IMC_GETCONVERSIONMODE)
            .is_none_or(|mode| mode & IME_CMODE_NATIVE != 0),
        None => true,
    }
}
