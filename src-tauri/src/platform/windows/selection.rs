//! 取选中文本（ADR-163 ②）：UI Automation `TextPattern.GetSelection` 优先；读不到再回落
//! 「备份剪贴板 → 合成 Ctrl+C → 读取 → 还原剪贴板」。回落顺序是纯函数 `routing::selection_route`。
//!
//! - UI Automation 调用跨进程、可能卡在目标应用里：每次在一条专用线程上做（COM 多线程套间），
//!   调用方只等 [`UIA_TIMEOUT`]，到点就当读不到。
//! - 剪贴板回落全程持 [`CLIPBOARD_MUTEX`] 一次（与粘贴注入互斥，TDD-05 §2.1-B），轮询
//!   `GetClipboardSequenceNumber` 判断复制是否落盘，有超时；还原失败如实报给调用方。
//! - 日志只记长度与走了哪条路，绝不记选中文本（TDD-12）。

use super::clipboard::{self, ClipboardRestore};
use super::routing::{selection_route, SelectionRoute, UiaSelection};
use super::{foreground, input, keys};
use crate::platform::CLIPBOARD_MUTEX;
use std::time::Duration;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};

/// 等 UI Automation 的上限。
pub const UIA_TIMEOUT: Duration = Duration::from_millis(300);
/// 等合成的 Ctrl+C 落进剪贴板的上限（TDD-05 §1.2 的 ~300ms）。
pub const COPY_TIMEOUT: Duration = Duration::from_millis(300);
/// 合成按键前等用户松开热键修饰键的上限。
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 一次主动取选区的结果。
#[derive(Debug, Default)]
pub struct CapturedSelection {
    /// 选中的文字（`None` = 没有选区）。
    pub text: Option<String>,
    /// 触发时的前台窗口（「替换原文」的注入目标）。
    pub target: Option<isize>,
    /// 走了剪贴板回落时，用户原来的剪贴板怎么样了；没碰剪贴板为 `None`。
    pub clipboard: Option<ClipboardRestore>,
}

fn read_uia_selection_blocking() -> UiaSelection {
    // SAFETY: 本线程专用于这一次查询：进入多线程套间，结束时对称地反初始化。
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return UiaSelection::Unavailable;
        }
        let result = (|| -> windows::core::Result<UiaSelection> {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let element = automation.GetFocusedElement()?;
            let Ok(pattern) =
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            else {
                return Ok(UiaSelection::Unavailable);
            };
            let ranges = pattern.GetSelection()?;
            let mut parts = Vec::new();
            for index in 0..ranges.Length()? {
                let text = ranges.GetElement(index)?.GetText(-1)?.to_string();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            Ok(if parts.is_empty() {
                UiaSelection::Empty
            } else {
                UiaSelection::Text(parts.join("\n"))
            })
        })();
        CoUninitialize();
        result.unwrap_or(UiaSelection::Unavailable)
    }
}

/// 读焦点控件的选区（UI Automation），最多等 [`UIA_TIMEOUT`]。
#[must_use]
pub fn read_uia_selection() -> UiaSelection {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("luna-uia-selection".into())
        .spawn(move || {
            let _ = tx.send(read_uia_selection_blocking());
        });
    if spawned.is_err() {
        return UiaSelection::Unavailable;
    }
    // 超时的那条线程会在目标应用回应后自行结束；结果没人收，不影响这次判定。
    rx.recv_timeout(UIA_TIMEOUT)
        .unwrap_or(UiaSelection::Unavailable)
}

/// 等剪贴板序号离开 `baseline`（复制真的落盘了），最多等 `timeout`。
pub async fn wait_sequence_changed(baseline: u32, timeout: Duration) -> bool {
    let polls = (timeout.as_millis() / POLL_INTERVAL.as_millis()).max(1);
    for _ in 0..polls {
        if clipboard::sequence_number() != baseline {
            return true;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    clipboard::sequence_number() != baseline
}

/// 受保护的剪贴板取选区：备份 → 合成 Ctrl+C → 等落盘 → 读 → 还原。全程持剪贴板锁一次。
async fn capture_via_clipboard() -> (Option<String>, ClipboardRestore) {
    let _lock = CLIPBOARD_MUTEX.lock().await;
    let prepared = tokio::task::spawn_blocking(|| {
        let backup = clipboard::backup()?;
        let baseline = clipboard::sequence_number();
        let held = input::wait_for_modifiers_released(MODIFIER_RELEASE_TIMEOUT);
        let sent = input::send(&keys::chord_strokes(&held, keys::VK_C)).is_ok();
        Some((backup, baseline, sent))
    })
    .await
    .ok()
    .flatten();
    // 剪贴板都打不开：什么也没动，原样就是完好的。
    let Some((backup, baseline, sent)) = prepared else {
        return (None, ClipboardRestore::Restored);
    };
    let copied = sent && wait_sequence_changed(baseline, COPY_TIMEOUT).await;
    if !copied {
        // 序号没动 = 什么都没写进去，剪贴板仍是原样，不必（也不该）重写一遍。
        return (None, ClipboardRestore::Restored);
    }
    let text = tokio::task::spawn_blocking(clipboard::read_text)
        .await
        .ok()
        .flatten();
    let complete = backup.is_complete();
    let restored = tokio::task::spawn_blocking(move || clipboard::restore(&backup))
        .await
        .unwrap_or(false);
    (text, clipboard::restore_report(complete, restored))
}

/// 主动取选区（热键路径）：UI Automation 优先，读不到且前台不是终端时回落受保护的剪贴板复制。
pub async fn capture_selection() -> CapturedSelection {
    let target = foreground::foreground_window();
    // 前台是 Lunarca 自己（比如弹窗正开着输入框）：没有别的应用的选区可取。
    if target.is_some_and(foreground::is_own_window) {
        return CapturedSelection {
            text: None,
            target,
            clipboard: None,
        };
    }
    let class = target.and_then(foreground::window_class);
    let uia = tokio::task::spawn_blocking(read_uia_selection)
        .await
        .unwrap_or(UiaSelection::Unavailable);
    let route = selection_route(uia, class.as_deref());
    let captured = match route {
        SelectionRoute::Uia(text) => CapturedSelection {
            text: Some(text),
            target,
            clipboard: None,
        },
        SelectionRoute::NoSelection => CapturedSelection {
            text: None,
            target,
            clipboard: None,
        },
        SelectionRoute::ClipboardCopy => {
            let (text, restore) = capture_via_clipboard().await;
            CapturedSelection {
                text: text.filter(|t| !t.trim().is_empty()),
                target,
                clipboard: Some(restore),
            }
        }
    };
    eprintln!(
        "[selection] 取选区 → via={} len={} clipboard_intact={}",
        match (&captured.clipboard, &captured.text) {
            (Some(_), _) => "clipboard",
            (None, Some(_)) => "uia",
            (None, None) => "none",
        },
        captured.text.as_ref().map_or(0, String::len),
        captured.clipboard.is_none_or(ClipboardRestore::is_intact),
    );
    captured
}
