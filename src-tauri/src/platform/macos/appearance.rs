//! 原生外观同步（`NSApplication.appearance`），Task 4 / TDD-11 §主题。`cfg(target_os = "macos")`。
//!
//! 前端 theme.ts 负责 webview 内的明暗（data-theme + 语义 Token）；窗口边框/标题栏/原生控件的
//! 明暗由 AppKit 的 `NSAppearance` 决定。设 App 级 appearance 即可统一所有 Tauri 窗口（含 NSPanel）。
//!
//! ⚠ 运行时只能真机验：headless 仅保证编译 + 纯映射逻辑。"明暗切换后 NSWindow 边框/原生控件
//! 同步"需 GUI 实测（见 F14 首启引导真机验收清单「主题同步」行，内部记录）。

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;

/// 解析 theme 字符串到 `NSAppearance` 名称；`None` 表示清空 appearance（跟随系统）。
///
/// 纯函数，便于 headless 单测（AppKit 调用本身只能真机验）。
#[must_use]
pub fn appearance_name_for(theme: &str) -> Option<&'static str> {
    match theme {
        "dark" => Some("NSAppearanceNameDarkAqua"),
        "light" => Some("NSAppearanceNameAqua"),
        // system / 未知：跟随系统。
        _ => None,
    }
}

/// 在主线程设置 App 级原生外观。
///
/// `theme`：`"dark"` → DarkAqua，`"light"` → Aqua，其它（含 `"system"`）→ 清空（跟随系统）。
///
/// # Safety
/// 必须在主线程调用（AppKit 要求）。调用方经 `AppHandle::run_on_main_thread` 保证。
pub unsafe fn apply_app_appearance(theme: &str) {
    let appearance: *mut AnyObject = match appearance_name_for(theme) {
        Some(name) => {
            let ns_name = NSString::from_str(name);
            msg_send![class!(NSAppearance), appearanceNamed: &*ns_name]
        }
        None => std::ptr::null_mut(),
    };
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: () = msg_send![app, setAppearance: appearance];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_name_mapping_resolves_aqua_variants() {
        assert_eq!(
            appearance_name_for("dark"),
            Some("NSAppearanceNameDarkAqua")
        );
        assert_eq!(appearance_name_for("light"), Some("NSAppearanceNameAqua"));
        assert_eq!(appearance_name_for("system"), None);
        assert_eq!(appearance_name_for("unknown"), None);
    }
}
