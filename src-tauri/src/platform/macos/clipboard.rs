//! macOS 剪贴板读写与**自适应保护**（TDD-05 §1.4-B / §2.1-B）。
//!
//! 用非弃用的 `objc2-app-kit::NSPasteboard`（不再用 cocoa）。本模块是**叶子函数**：调用方须在
//! 持 [`crate::platform::CLIPBOARD_MUTEX`] 时调用"备份→写→还原"序列，且**每流程只取一次锁**
//! （`tokio::sync::Mutex` 不可重入，TDD-05 §2.1）。
//!
//! 双击翻译只读不写（[`read_text`] + [`change_count`]，无需锁/备份）；合成粘贴注入（§2.1-B）/
//! 主动合成捕获（§1.4-B，Phase 5）才需"备份→写 transient→合成键→还原"全序列。
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译；多格式备份还原保真、changeCount 时序、transient
//! 标记是否令剪贴板历史工具忽略，均需 macOS 桌面实测。

use crate::platform::ClipboardBackup;
use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardType, NSPasteboardTypeString};
use objc2_foundation::{NSArray, NSData, NSString};

/// nspasteboard.org 约定的 transient 标记类型：剪贴板历史工具（Raycast/Paste 等）见之即忽略
/// 本次写入（PRD-01-027 免污染）。
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// 通用剪贴板的 `changeCount`——"复制是否真的落盘"的可靠信号（轮询其自增，TDD-05 §1.2）。
#[must_use]
pub fn change_count() -> i64 {
    NSPasteboard::generalPasteboard().changeCount() as i64
}

/// 读纯文本（双击路径读用户那次 `Cmd+C` 的结果；无文本返回 `None`）。
#[must_use]
pub fn read_text() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    // SAFETY: 读取系统常量 `NSPasteboardTypeString`（extern static）。
    let ty = unsafe { NSPasteboardTypeString };
    pb.stringForType(ty).map(|s| s.to_string())
}

/// 只读 HTML 表示；与纯文本来自同一系统剪贴板，不下载任何资源。
#[must_use]
pub fn read_html() -> Option<String> {
    NSPasteboard::generalPasteboard()
        .stringForType(&NSString::from_str("public.html"))
        .map(|value| value.to_string())
}

/// 备份当前剪贴板的**全部类型**数据（多格式无损，注入前调用，§2.1-B）。
#[must_use]
pub fn read_all_types() -> ClipboardBackup {
    let pb = NSPasteboard::generalPasteboard();
    let mut backup = Vec::new();
    if let Some(types) = pb.types() {
        for ty in types.to_vec() {
            if let Some(data) = pb.dataForType(&ty) {
                backup.push((ty.to_string(), data.to_vec()));
            }
        }
    }
    backup
}

/// 写纯文本并打 **transient 标记**（PRD-01-027：通知剪贴板历史工具忽略本次记录）。
///
/// 仅用于合成粘贴注入路径（§2.1-B）：写完即合成 `Cmd+V`，随后 [`restore_all_types`] 还原。
pub fn write_text_transient(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: 读取系统常量 `NSPasteboardTypeString`。
    let string_ty: &NSPasteboardType = unsafe { NSPasteboardTypeString };
    let transient_ty = NSString::from_str(TRANSIENT_TYPE);
    let types = NSArray::from_slice(&[string_ty, &*transient_ty]);
    // SAFETY: `declareTypes_owner` 仅声明类型 + 设 owner=nil，参数均有效。
    unsafe { pb.declareTypes_owner(&types, None) };
    let ns = NSString::from_str(text);
    pb.setString_forType(&ns, string_ty);
    // transient 标记：空数据即可（约定型）。
    let empty = NSData::with_bytes(&[]);
    pb.setData_forType(Some(&empty), &transient_ty);
}

/// 写**持久**纯文本到剪贴板（"复制译文"用，PRD-01-021）：用户随后 `Cmd+V` 即粘贴该文本。
///
/// 与 [`write_text_transient`] 的区别：不打 transient 标记——这是用户**主动复制**，应正常进入
/// 剪贴板（及其历史），而非"注入兜底"的临时写入。
pub fn write_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: 读取系统常量 `NSPasteboardTypeString`。
    let string_ty: &NSPasteboardType = unsafe { NSPasteboardTypeString };
    let types = NSArray::from_slice(&[string_ty]);
    // SAFETY: 仅声明字符串类型 + owner=nil。
    unsafe { pb.declareTypes_owner(&types, None) };
    let ns = NSString::from_str(text);
    pb.setString_forType(&ns, string_ty);
}

/// 写**持久**的「带格式 + 纯文本」两种表示（"复制表格为带格式"用）：粘进文档 / 表格软件
/// 取 HTML，粘进纯文本处取纯文本。与 [`write_text`] 一样是用户主动复制，不打 transient 标记。
pub fn write_html(plain: &str, html: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: 读取系统常量 `NSPasteboardTypeString`。
    let string_ty: &NSPasteboardType = unsafe { NSPasteboardTypeString };
    let html_ty = NSString::from_str("public.html");
    let types = NSArray::from_slice(&[&*html_ty, string_ty]);
    // SAFETY: 仅声明两种类型 + owner=nil。
    unsafe { pb.declareTypes_owner(&types, None) };
    pb.setString_forType(&NSString::from_str(html), &html_ty);
    pb.setString_forType(&NSString::from_str(plain), string_ty);
}

/// 绝对无损还原备份的**全部类型**（注入后调用，§2.1-B）。
pub fn restore_all_types(backup: &ClipboardBackup) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let ns_types: Vec<Retained<NSString>> =
        backup.iter().map(|(t, _)| NSString::from_str(t)).collect();
    let arr = NSArray::from_retained_slice(&ns_types);
    // SAFETY: 声明备份过的全部类型，owner=nil。
    unsafe { pb.declareTypes_owner(&arr, None) };
    for ((_, data), ty) in backup.iter().zip(&ns_types) {
        let nsdata = NSData::with_bytes(data);
        pb.setData_forType(Some(&nsdata), ty);
    }
}
