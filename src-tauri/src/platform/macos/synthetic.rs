//! macOS 合成键盘事件（**打自发事件指纹**，TDD-05 §1.4 / §2.1-B 自污染防护）。
//!
//! 选区主动捕获（合成 `Cmd+C`，§1.4-B，Phase 5）与文本注入兜底（合成 `Cmd+V`，§2.1-B）须发出
//! 模拟复制/粘贴键。**所有 Lunarca 合成的键必须被本进程的 `CGEventTap` 识别后忽略**，否则合成的
//! `Cmd+C` 会被自己的双击检测误判为"用户双击"→ 误弹窗甚至递归。
//!
//! 手段：用专属 `CGEventSource` 并 `CGEventSourceSetUserData` 打上 [`LUNA_SYNTHETIC_EVENT_MAGIC`]；
//! 该 userData 经 `kCGEventSourceUserData`（[`keytap`] 读 `EVENT_SOURCE_USER_DATA`）回传给 tap 回调，
//! 回调首行据此过滤本进程自发事件。**写端（本模块）与读端（[`keytap`]）共享同一魔数常量**（单一来源）。
//!
//! [`keytap`]: super::keytap
//! [`LUNA_SYNTHETIC_EVENT_MAGIC`]: crate::platform::LUNA_SYNTHETIC_EVENT_MAGIC
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译。"合成 `Cmd+V` 能粘贴 + 不被自己的 tap 误判双击"
//! 是自污染闭环的**首次真机行使**（TDD-05 §1.4），须 GUI 实测。

use crate::platform::LUNA_SYNTHETIC_EVENT_MAGIC;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use foreign_types::ForeignType as _;
use std::ffi::c_void;

// CGEventSourceSetUserData 未被 core-graphics 0.24 封装；走 raw FFI（CoreGraphics 已由 core-graphics
// 链接）。把本进程合成事件的源 userData 设为魔数，供 tap 读端过滤（自污染防护，§1.4）。
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSetUserData(source: *mut c_void, user_data: i64);
}

/// `kVK_ANSI_C`（复制键虚拟键码）。
const KEYCODE_C: CGKeyCode = 8;
/// `kVK_ANSI_V`（粘贴键虚拟键码）。
const KEYCODE_V: CGKeyCode = 9;

/// 合成键错误。
#[derive(Debug, thiserror::Error)]
pub enum SyntheticError {
    /// 创建事件源失败。
    #[error("failed to create a CGEventSource")]
    SourceCreationFailed,
    /// 创建键盘事件失败。
    #[error("failed to create a synthetic keyboard event")]
    EventCreationFailed,
}

/// 合成一次 **`Cmd+C`**（选区主动捕获，§1.4-B；Phase 5 消费）。带魔数 → 本进程 tap 忽略。
///
/// # Errors
/// 事件源 / 事件创建失败时返回 [`SyntheticError`]。
pub fn post_synthetic_copy() -> Result<(), SyntheticError> {
    post_cmd_key(KEYCODE_C)
}

/// 合成一次 **`Cmd+V`**（文本注入兜底，§2.1-B）。带魔数 → 本进程 tap 忽略。
///
/// # Errors
/// 事件源 / 事件创建失败时返回 [`SyntheticError`]。
pub fn post_synthetic_paste() -> Result<(), SyntheticError> {
    post_cmd_key(KEYCODE_V)
}

/// 用打了魔数的专属事件源合成 `Cmd+<keycode>`（keydown + keyup）并投递到系统（HID）。
fn post_cmd_key(keycode: CGKeyCode) -> Result<(), SyntheticError> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| SyntheticError::SourceCreationFailed)?;

    // 打自发事件指纹（自污染防护，§1.4）：源 userData = 魔数，tap 读端据此过滤。
    // SAFETY: `source` 为刚创建的有效 CGEventSource；`as_ptr` 为其底层指针。
    unsafe {
        CGEventSourceSetUserData(source.as_ptr().cast::<c_void>(), LUNA_SYNTHETIC_EVENT_MAGIC)
    };

    for keydown in [true, false] {
        // new_keyboard_event 消费 source；克隆复用（CFRetain，保住同一源 userData）。
        let event = CGEvent::new_keyboard_event(source.clone(), keycode, keydown)
            .map_err(|()| SyntheticError::EventCreationFailed)?;
        event.set_flags(CGEventFlags::CGEventFlagCommand);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}
