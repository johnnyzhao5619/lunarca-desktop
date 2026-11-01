//! macOS 权限查询/请求（TDD-05 §4）。辅助功能用真实 AX，麦克风用真实 AVFoundation 查询，
//! 屏幕录制用真实 CoreGraphics TCC 查询（截屏翻译 PRD-01-007）；通知/输入监听尚未接入真实
//! 查询（随 Phase 6/8），现阶段诚实返回 `NotDetermined`（非伪 `Granted`），`request` 打开对应
//! 系统设置面板。
//!
//! **AX 信任 FFI 单一来源**：`is_process_trusted`/`prompt_accessibility_trust` 收口于此（原散在
//! keytap.rs），keytap 与权限管理共用，杜绝两份 extern（DRY）。
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译。授权状态、弹框、打开设置面板需 GUI 实测。

use crate::platform::permissions::{PermissionKind, PermissionManager, PermissionStatus};
use block2::global_block;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use objc2::runtime::Bool;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::process::Command;

// 辅助功能（Accessibility）信任检查：CGEventTap 的 HID tap / AX 注入必须进程被 trust 才生效。
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

// 屏幕录制（Screen Recording）TCC 查询/请求：截屏翻译（PRD-01-007，`screencapture` 交互框选）
// 与系统音频 loopback（P1）共用同一权限。macOS 10.15+ 可用。
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

// AVAuthorizationStatus: notDetermined=0, restricted=1, denied=2, authorized=3.
const AV_AUTH_NOT_DETERMINED: isize = 0;
const AV_AUTH_RESTRICTED: isize = 1;
const AV_AUTH_DENIED: isize = 2;
const AV_AUTH_AUTHORIZED: isize = 3;
const AV_MEDIA_TYPE_AUDIO: &str = "soun";

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

/// 本进程当前是否被授予辅助功能（不弹框）。
#[must_use]
pub fn is_process_trusted() -> bool {
    // SAFETY: 无参 C 调用，返回 bool。
    unsafe { AXIsProcessTrusted() }
}

/// 本进程当前是否被授予屏幕录制（不弹框；截屏翻译前置检查，PRD-01-007）。
#[must_use]
pub fn has_screen_capture_access() -> bool {
    // SAFETY: 无参 C 调用，返回 bool（Preflight 只查不弹框）。
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 请求屏幕录制授权：未授权时触发系统 TCC 弹框（每个 app 每版本仅弹一次；用户勾选后
/// 系统要求重启 app 生效）。返回调用时是否已授权。
pub fn request_screen_capture_access() -> bool {
    // SAFETY: 无参 C 调用，返回 bool。
    unsafe { CGRequestScreenCaptureAccess() }
}

/// 检查辅助功能授权，**未授权则弹出系统授权对话框**（把本 app 加入「辅助功能」列表）。
/// 返回当前是否已授权（弹框后通常仍为 false，需用户手动勾选并重启进程）。
pub fn prompt_accessibility_trust() -> bool {
    // SAFETY: 用系统常量 key + true 构造选项字典，调 AX 信任检查（带提示）。
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

#[must_use]
fn microphone_status_from_av_status(status: isize) -> PermissionStatus {
    match status {
        AV_AUTH_AUTHORIZED => PermissionStatus::Granted,
        AV_AUTH_NOT_DETERMINED => PermissionStatus::NotDetermined,
        AV_AUTH_RESTRICTED | AV_AUTH_DENIED => PermissionStatus::Denied,
        _ => PermissionStatus::Unsupported,
    }
}

#[must_use]
fn microphone_authorization_status() -> PermissionStatus {
    let media_type = NSString::from_str(AV_MEDIA_TYPE_AUDIO);
    // SAFETY: `AVCaptureDevice` is an AVFoundation class; `authorizationStatusForMediaType:`
    // is a pure class query returning AVAuthorizationStatus for the provided `AVMediaType`.
    let status: isize = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*media_type]
    };
    microphone_status_from_av_status(status)
}

fn request_microphone_access() {
    let media_type = NSString::from_str(AV_MEDIA_TYPE_AUDIO);
    global_block! {
        static REQUEST_MICROPHONE_ACCESS = |_granted: Bool| {};
    }
    // SAFETY: `requestAccessForMediaType:completionHandler:` starts the system TCC prompt if
    // status is not determined. The completion block is static and intentionally carries no
    // user data; callers re-check state via `check`.
    unsafe {
        let _: () = msg_send![
            class!(AVCaptureDevice),
            requestAccessForMediaType: &*media_type,
            completionHandler: &*REQUEST_MICROPHONE_ACCESS
        ];
    }
}

/// 打开「系统设置 › 隐私与安全性」对应面板（尽力而为，引导用户手动开启）。
fn open_settings_pane(kind: PermissionKind) {
    let url = match kind {
        PermissionKind::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionKind::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        PermissionKind::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PermissionKind::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PermissionKind::Notification => {
            "x-apple.systempreferences:com.apple.preference.notifications"
        }
    };
    // `open <url>` 唤起系统设置；失败仅记日志（不阻断）。
    if let Err(e) = Command::new("open").arg(url).spawn() {
        eprintln!("[permissions] 打开系统设置面板失败：{e}");
    }
}

/// macOS 权限管理器。
#[derive(Debug, Default, Clone, Copy)]
pub struct MacPermissionManager;

impl PermissionManager for MacPermissionManager {
    fn check(&self, kind: PermissionKind) -> PermissionStatus {
        match kind {
            PermissionKind::Accessibility => {
                if is_process_trusted() {
                    PermissionStatus::Granted
                } else {
                    // AX 不区分"拒绝"与"未询问"，统一按 Denied 处理（降级路由一致：备用热键）。
                    PermissionStatus::Denied
                }
            }
            PermissionKind::Microphone => microphone_authorization_status(),
            PermissionKind::ScreenRecording => {
                // CGPreflight 不区分「拒绝」与「未询问」，未授予统一按 Denied 处理
                // （降级路由一致：截屏翻译不可用 + 引导授权，与辅助功能同口径）。
                if has_screen_capture_access() {
                    PermissionStatus::Granted
                } else {
                    PermissionStatus::Denied
                }
            }
            // 输入监听尚未接入真实查询（随 Phase 8）——诚实返回 NotDetermined，绝不伪 Granted。
            PermissionKind::InputMonitoring => PermissionStatus::NotDetermined,
            // 通知：系统通知经 `tauri-plugin-notification`（NSUserNotificationCenter）发出，
            // 该 API 没有可查询的授权状态，用户在「系统设置 › 通知」里开关；如实报「不可查」，
            // 而不是 NotDetermined（那暗示"会问你"，而它不会问）。应用内 toast/banner 不受影响。
            PermissionKind::Notification => PermissionStatus::Unsupported,
        }
    }

    fn request(&self, kind: PermissionKind) {
        // 辅助功能：先弹系统授权框（把 app 加入列表），再打开面板便于勾选。
        if kind == PermissionKind::Accessibility {
            let _ = prompt_accessibility_trust();
        }
        if kind == PermissionKind::Microphone {
            request_microphone_access();
        }
        if kind == PermissionKind::ScreenRecording {
            let _ = request_screen_capture_access();
        }
        open_settings_pane(kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_avfoundation_microphone_statuses() {
        assert_eq!(
            microphone_status_from_av_status(AV_AUTH_AUTHORIZED),
            PermissionStatus::Granted
        );
        assert_eq!(
            microphone_status_from_av_status(AV_AUTH_NOT_DETERMINED),
            PermissionStatus::NotDetermined
        );
        assert_eq!(
            microphone_status_from_av_status(AV_AUTH_DENIED),
            PermissionStatus::Denied
        );
        assert_eq!(
            microphone_status_from_av_status(AV_AUTH_RESTRICTED),
            PermissionStatus::Denied
        );
        assert_eq!(
            microphone_status_from_av_status(99),
            PermissionStatus::Unsupported
        );
    }
}
