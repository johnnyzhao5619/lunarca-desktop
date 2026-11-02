//! 非激活 `NSPanel` 转换（ADR-015 / TDD-05 §5，objc2 直做）。`cfg(target_os = "macos")`。
//!
//! **物理硬约束（ADR-015）**：Tauri 默认窗口是会 **activate App** 的 `NSWindow`——弹出瞬间夺走
//! 前台第三方 App 焦点 → 选区丢失、注入目标错乱。弹窗翻译/AI 悬浮菜单 UI 宿主必须是
//! **非激活面板**（`nonactivatingPanel` 掩码仅对 `NSPanel` 生效，故须把窗口类换成 `NSPanel`）。
//!
//! **手段 = objc2 直做**（非 tauri-nspanel，TDD-05 §5 修订）：spike 实测 tauri-nspanel v2 仍依赖
//! 已弃用 cocoa，违反"不用 cocoa"硬约束；objc2 已随 Tauri 2.11 进树、零新增。
//!
//! **时序铁律（TDD-05 §5.2）**：转换必须在 **Rust 侧、首次 `show()` 之前**完成（预热即转换）；
//! 否则预热出的仍是会夺焦的普通窗，首弹即丢选区。
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译。"弹出不夺第三方焦点 / 不触发 Spaces 滑屏 /
//! 输入模式文本框可键入而不 activate App / webview 仍正常渲染"等需 GUI 实测。
//! 见 ADR-015 非激活面板验收清单（内部记录）。
//!
//! ⚠ **类交换风险**：`object_setClass` 把 Tauri 的 `NSWindow` 实例改类为 `NSPanel`。`NSPanel`
//! 是 `NSWindow` 子类、ivar 布局兼容，但若 Tauri/wry 给窗口挂了自定义子类方法，平替为裸
//! `NSPanel` 可能丢失之；真机若现 webview 异常即据此回退为"`NSPanel` 子类保留原类链"。

use objc2::ffi::{object_getClass, object_setClass};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use objc2_app_kit::{NSPanel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::ffi::CStr;
use std::sync::OnceLock;

/// 浮动顶层级（`NSStatusWindowLevel`，TDD-05 §5.1）。
const NS_STATUS_WINDOW_LEVEL: isize = 25;
/// 文本编辑期的让位层级（`NSFloatingWindowLevel`，TDD-05 §5.1.1）：仍浮在普通窗口之上，
/// 但低于输入法候选窗——真机 2026-09-06 录音 HUD 书签备注框输入拼音时，候选窗被
/// `NSStatusWindowLevel` 的面板整个盖住，选不了词。
const NS_FLOATING_WINDOW_LEVEL: isize = 3;
const LUNA_PANEL_CLASS_NAME: &[u8] = b"LunaNonActivatingPanel\0";

fn luna_panel_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let name = CStr::from_bytes_with_nul(LUNA_PANEL_CLASS_NAME)
            .expect("Lunarca NSPanel class name must be nul-terminated");
        if let Some(existing) = AnyClass::get(name) {
            return existing;
        }

        extern "C" fn can_become_key_window(_this: &AnyObject, _sel: Sel) -> Bool {
            Bool::YES
        }

        extern "C" fn can_become_main_window(_this: &AnyObject, _sel: Sel) -> Bool {
            Bool::NO
        }

        let mut builder = ClassBuilder::new(name, class!(NSPanel))
            .expect("Lunarca NSPanel subclass should register once");
        // SAFETY: Signatures match AppKit's `-canBecomeKeyWindow` / `-canBecomeMainWindow`
        // Objective-C methods (`BOOL self:_cmd`). Key=true lets borderless input controls receive
        // text; main=false keeps the host app from activating as the document context.
        unsafe {
            builder.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key_window as extern "C" fn(_, _) -> _,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                can_become_main_window as extern "C" fn(_, _) -> _,
            );
        }
        builder.register()
    })
}

/// 把一个 Tauri `NSWindow` 指针转为**非激活 `NSPanel`**（ADR-015）。
///
/// `ns_window` = `tauri::WebviewWindow::ns_window()` 返回的 `*mut c_void`（实为 `NSWindow*`）。
///
/// # Safety
/// 调用方须保证 `ns_window` 是有效的、存活的 `NSWindow` 指针，且**在主线程**调用（AppKit 要求）。
/// 须在窗口首次 `show()` 之前调用（预热即转换，TDD-05 §5.2）。
pub unsafe fn convert_to_nonactivating_panel(ns_window: *mut std::ffi::c_void) {
    let obj: *mut AnyObject = ns_window.cast();

    // 1. 类交换为专用 NSPanel 子类：nonactivatingPanel 掩码仅对 NSPanel 生效；子类额外允许
    // borderless 面板按需成为 key window，让输入框能收键入，同时保持 canBecomeMainWindow=false。
    object_setClass(obj, luna_panel_class());

    let window: &NSWindow = &*(obj.cast());

    // 2. 注入 NonactivatingPanel 掩码（防夺焦），并让 WindowServer 也按非激活处理（见 prevent_activation）。
    let mask = window.styleMask() | NSWindowStyleMask::NonactivatingPanel;
    window.setStyleMask(mask);
    prevent_activation(window);

    // 3. collectionBehavior：跨全部 Space + 全屏辅助（Spaces 漂移抑制，PRD-01-025）。
    let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    window.setCollectionBehavior(behavior);

    // 4. 浮动顶层级。
    window.setLevel(NS_STATUS_WINDOW_LEVEL);

    // 5. Borderless NSPanel 没有标题栏；声明窗口本体可移动，并允许 AppKit 对背景区域进入拖拽。
    // WebView 内的控件级过滤仍由前端控制，避免按钮/输入框被当作拖拽区。
    window.setMovable(true);
    window.setMovableByWindowBackground(true);
}

/// WindowServer 窗口标签里的「禁止激活」位（`kCGSPreventsActivationTagBit`）。
const CGS_PREVENTS_ACTIVATION_TAG: u64 = 1 << 16;

/// 让 WindowServer 也认这扇面板「点了不激活 App」（ADR-015，TDD-05 §5.1.1）。
///
/// `NonactivatingPanel` 掩码只有在 `NSPanel` **初始化时**就带上，才会同步成 WindowServer 的窗口
/// 标签；这里是窗口建好之后才换类、补掩码（Tauri 不给面板构造入口），AppKit 一侧看着都对，
/// WindowServer 仍按普通窗口处理：鼠标一按就把整个 App 激活，源应用退到后台，Esc 收起面板后
/// 系统再把主窗口提到前面。真机 2026-10-30（macOS 27）：初始化即带掩码的面板点击不激活，建后补
/// 掩码的会激活，补上本标签后不再激活，输入框照常收键入。
///
/// 两处都是私有接口：`-_setPreventsActivation:` 让 AppKit 自己的记录与之一致，`CGSSetWindowTags`
/// 让 WindowServer 当场生效。哪一个不在（系统改了）就跳过并打一行日志，退回点击即激活的旧行为。
///
/// # Safety
/// 同 [`convert_to_nonactivating_panel`]：主线程、存活的窗口。
unsafe fn prevent_activation(window: &NSWindow) {
    let responds: bool = msg_send![window, respondsToSelector: sel!(_setPreventsActivation:)];
    if responds {
        let _: () = msg_send![window, _setPreventsActivation: true];
    } else {
        eprintln!("[ADR-015] 没有 -_setPreventsActivation:，只设 WindowServer 标签");
    }

    type MainConnectionId = unsafe extern "C" fn() -> i32;
    type SetWindowTags = unsafe extern "C" fn(i32, i32, *const u64, i32) -> i32;
    let connection = libc::dlsym(libc::RTLD_DEFAULT, c"CGSMainConnectionID".as_ptr());
    let set_tags = libc::dlsym(libc::RTLD_DEFAULT, c"CGSSetWindowTags".as_ptr());
    if connection.is_null() || set_tags.is_null() {
        eprintln!("[ADR-015] 找不到 CGSSetWindowTags，面板被点击时仍会激活 App");
        return;
    }
    // SAFETY: 两个符号都来自 CoreGraphics，签名是它们一直以来的 C 声明
    // （`CGSConnectionID CGSMainConnectionID(void)`；`CGError CGSSetWindowTags(cid, wid, const
    // CGSWindowTagBit *tags, int maxTagSize)`，tags 指向 64 位、maxTagSize 以位计）。
    let connection = std::mem::transmute::<*mut libc::c_void, MainConnectionId>(connection);
    let set_tags = std::mem::transmute::<*mut libc::c_void, SetWindowTags>(set_tags);
    // tao 建窗用 `defer: false`，转换时窗口号已经存在；万一没有就没法打标签。
    let number = window.windowNumber();
    if number <= 0 {
        eprintln!("[ADR-015] 面板还没有 WindowServer 窗口号，跳过禁止激活标签");
        return;
    }
    let tags = CGS_PREVENTS_ACTIVATION_TAG;
    let error = set_tags(connection(), number as i32, &tags, 64);
    if error != 0 {
        eprintln!("[ADR-015] CGSSetWindowTags 失败（{error}），面板被点击时仍会激活 App");
    }
}

/// 让面板上的文本框**按需成为 key responder** 接收键入，但**不 activate 整个 App**（输入模式，
/// TDD-05 §5.1.1，服务 PRD-01-005/006）。须在 [`convert_to_nonactivating_panel`] 之后调用。
///
/// # Safety
/// 同 [`convert_to_nonactivating_panel`]：有效 `NSPanel` 指针、主线程调用。
pub unsafe fn enable_input_mode_focus(ns_window: *mut std::ffi::c_void) {
    let panel: &NSPanel = &*(ns_window.cast::<AnyObject>().cast());
    let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
}

/// 面板内文本框进入/退出编辑时切换层级：编辑中降到 `NSFloatingWindowLevel` 给输入法候选窗
/// 让路，退出即回 `NSStatusWindowLevel`。**只对经 [`convert_to_nonactivating_panel`] 转过的
/// 面板生效**——主窗口等普通 `NSWindow` 调到这里是 no-op，前端可以不分窗口一律调用。
///
/// # Safety
/// 同 [`convert_to_nonactivating_panel`]：调用方须传入主线程上存活的 `NSWindow` 指针。
pub unsafe fn set_text_editing_level(ns_window: *mut std::ffi::c_void, editing: bool) {
    let obj: *mut AnyObject = ns_window.cast();
    if !std::ptr::eq(object_getClass(obj), luna_panel_class()) {
        return;
    }
    let window: &NSWindow = &*(obj.cast());
    window.setLevel(if editing {
        NS_FLOATING_WINDOW_LEVEL
    } else {
        NS_STATUS_WINDOW_LEVEL
    });
}

/// 切换面板的鼠标穿透。译境在静默状态必须穿透，避免一个透明窗口截获用户对
/// 任意第三方应用的点击；进入显式调整态后才临时接收鼠标事件（PRD-01-036）。
///
/// # Safety
/// 同 [`convert_to_nonactivating_panel`]：调用方须传入主线程上存活的 `NSPanel` 指针。
pub unsafe fn set_ignores_mouse_events(ns_window: *mut std::ffi::c_void, ignores: bool) {
    let window: &NSWindow = &*(ns_window.cast::<AnyObject>().cast());
    window.setIgnoresMouseEvents(ignores);
}
