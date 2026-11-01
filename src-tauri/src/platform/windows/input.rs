//! `SendInput` 薄层（ADR-163）：把 [`KeyStroke`] 序列一次性送进系统输入流。
//!
//! 每个事件的 `dwExtraInfo` 打 [`LUNA_SYNTHETIC_EVENT_MAGIC`]，本进程的低层键盘钩子据此忽略
//! （自污染防护，TDD-05 §1.4）。整串一次 `SendInput`：系统保证中间不插入别的输入。

use super::keys::{self, KeyStroke, MODIFIER_VKS};
use crate::platform::LUNA_SYNTHETIC_EVENT_MAGIC;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC,
    VIRTUAL_KEY,
};

/// 合成按键错误。
#[derive(Debug, thiserror::Error)]
pub enum SendInputError {
    /// 系统收下的事件少于送出的（输入被别的程序阻断，或目标在安全桌面上）。
    #[error("SendInput accepted {accepted} of {sent} events")]
    Partial { accepted: u32, sent: u32 },
}

/// 此刻物理键盘上按着的左右修饰键（`GetAsyncKeyState` 最高位）。
#[must_use]
pub fn held_modifiers() -> Vec<u16> {
    MODIFIER_VKS
        .iter()
        .copied()
        // SAFETY: 只读查询键状态。
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(*vk)) } < 0)
        .collect()
}

/// 等用户松开修饰键（热键刚触发时手还按着），最多等 `timeout`；返回到时仍按着的那些。
pub fn wait_for_modifiers_released(timeout: Duration) -> Vec<u16> {
    let started = Instant::now();
    loop {
        let held = held_modifiers();
        if held.is_empty() || started.elapsed() >= timeout {
            return held;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn to_input(stroke: KeyStroke) -> INPUT {
    let (vk, scan, flags) = match stroke {
        KeyStroke::Vk { vk, down } => {
            // SAFETY: 纯查表。
            let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if keys::is_extended_vk(vk) {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            (vk, scan, flags)
        }
        KeyStroke::Unicode { unit, down } => {
            let flags = if down {
                KEYEVENTF_UNICODE
            } else {
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
            };
            (0, unit, flags)
        }
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: LUNA_SYNTHETIC_EVENT_MAGIC as usize,
            },
        },
    }
}

/// 把整串按键一次送出。
///
/// # Errors
/// 系统没有收下全部事件时返回 [`SendInputError::Partial`]。注意：目标是更高权限的窗口时系统会
/// **静默**丢弃而不报错，所以调用方要在送之前核对（`routing::decide_inject_route`）。
pub fn send(strokes: &[KeyStroke]) -> Result<(), SendInputError> {
    if strokes.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = strokes.iter().copied().map(to_input).collect();
    // SAFETY: inputs 是有效的 INPUT 数组，cbSize 为单个 INPUT 的大小。
    let accepted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    let sent = inputs.len() as u32;
    if accepted == sent {
        Ok(())
    } else {
        Err(SendInputError::Partial { accepted, sent })
    }
}
