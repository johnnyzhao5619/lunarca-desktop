//! macOS「服务」菜单的提供者（笔记库对标轮 W5b）：别的应用里选中文字 → 右键 / 应用菜单「服务」
//! →「用 Lunarca 存为速记」。
//!
//! 系统按 `Info.plist` 的 `NSServices` 登记菜单项（`NSMessage = saveAsQuickNote`），用户点它时向
//! `NSApp.servicesProvider` 发 `saveAsQuickNote:userData:error:`，选中的文字在那块粘贴板上。
//! 这里只做 FFI：取出文字交给注入的 Rust 闭包，存不存、存到哪由调用方决定（`crate::services`）。
//! 粘贴板内容不写日志。
//!
//! **线程契约**：服务消息在主线程投递；回调闭包须立刻返回（落盘挪到别的线程），不能卡住主线程
//! ——发起服务的那个应用此刻在等我们。

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{class, define_class, msg_send, AnyThread, DefinedClass};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

/// 用户点的是哪一项服务（选择子与 `Info.plist` 的 `NSMessage` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// 「用 Lunarca 存为速记」（`saveAsQuickNote`）。
    QuickNote,
    /// 「在 Lunarca 提词器中显示」（`showInTeleprompter`，ADR-176 ⑨）。
    Teleprompter,
}

/// 收到一段文字（粘贴板上没有文字时为 `None`）。
pub type ServiceCallback = Box<dyn Fn(ServiceKind, Option<String>) + Send + 'static>;

struct ProviderIvars {
    callback: ServiceCallback,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ProviderIvars]
    struct LunaServicesProvider;

    unsafe impl NSObjectProtocol for LunaServicesProvider {}

    impl LunaServicesProvider {
        // 选择子与 `Info.plist` 的 `NSMessage` 必须一致（`saveAsQuickNote` + 固定的两个参数）。
        #[unsafe(method(saveAsQuickNote:userData:error:))]
        fn save_as_quick_note(
            &self,
            pboard: &NSPasteboard,
            _user_data: *mut NSString,
            _error: *mut *mut NSString,
        ) {
            // SAFETY: NSPasteboardTypeString 是 AppKit 导出的常量，进程生命周期内有效。
            let text = unsafe { pboard.stringForType(NSPasteboardTypeString) };
            (self.ivars().callback)(ServiceKind::QuickNote, text.map(|text| text.to_string()));
        }

        #[unsafe(method(showInTeleprompter:userData:error:))]
        fn show_in_teleprompter(
            &self,
            pboard: &NSPasteboard,
            _user_data: *mut NSString,
            _error: *mut *mut NSString,
        ) {
            // SAFETY: 同上。
            let text = unsafe { pboard.stringForType(NSPasteboardTypeString) };
            (self.ivars().callback)(ServiceKind::Teleprompter, text.map(|text| text.to_string()));
        }
    }
);

impl LunaServicesProvider {
    fn new(callback: ServiceCallback) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ProviderIvars { callback });
        // SAFETY: NSObject 超类无额外初始化前置条件。
        unsafe { msg_send![super(this), init] }
    }
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    /// 让系统重读本应用 `Info.plist` 里的服务登记（新装或升级后不用重新登录就出现在菜单里）。
    fn NSUpdateDynamicServices();
}

/// 把提供者挂到 `NSApp` 上。**必须在主线程调用**（应用 setup 里）；提供者随进程常驻。
pub fn register(callback: ServiceCallback) {
    let provider = LunaServicesProvider::new(callback);
    // SAFETY: 主线程调用；`sharedApplication` 恒返回进程唯一的 NSApplication。
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setServicesProvider: &*provider];
        NSUpdateDynamicServices();
    }
    // `servicesProvider` 是弱引用（assign），提供者要活到进程结束。
    std::mem::forget(provider);
}
