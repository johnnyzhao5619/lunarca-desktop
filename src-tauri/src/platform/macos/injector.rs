//! macOS `TextInjector`（TDD-05 §2）：AX 直插 + 合成粘贴兜底。
//!
//! - 替换选区：当前 AX 选区可确认时先 `AXSelectedText` 直插，失败再兜底合成粘贴。
//! - 光标注入 / IME 风险：无可确认选区时合成粘贴优先。`Cmd+V` 会让多数应用先提交 IME 预编辑串，
//!   比向 `AXSelectedText` 写入更安全（TDD-05 §2.2）。
//! - 合成粘贴全程只取一次 [`crate::platform::CLIPBOARD_MUTEX`]：备份全格式 → transient 写入 →
//!   合成 Cmd+V（带魔数）→ changeCount 轮询 → 还原全格式。

use crate::platform::{TextInjectionError, TextInjector, CLIPBOARD_MUTEX};
use crate::popup::SelectionSnapshot;
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::time::Duration;

/// macOS 文本注入器。
#[derive(Debug, Default, Clone, Copy)]
pub struct MacTextInjector;

pub enum SnapshotReplaceOutcome {
    Injected,
    CopiedFallback,
}

impl MacTextInjector {
    pub async fn replace_selection_snapshot(
        &self,
        snapshot: &SelectionSnapshot,
        text: String,
    ) -> Result<SnapshotReplaceOutcome, TextInjectionError> {
        if let Some(target) = snapshot.macos_target() {
            if let Some(current) = target.selected_text() {
                if current != snapshot.source_text() {
                    return Err(TextInjectionError::SelectionSnapshotInvalid);
                }
            }

            if target.replace_selected_text(&text).is_ok() {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if replacement_took_effect(
                    target.selected_text().as_deref(),
                    snapshot.source_text(),
                ) {
                    eprintln!("[TextInjector] 源选区快照 AX 直插成功。");
                    return Ok(SnapshotReplaceOutcome::Injected);
                }
                let _ = target.restore_selected_range();
            }

            let _ = target.restore_selected_range();
        }

        eprintln!("[TextInjector] 源选区快照不可 AX 直插，降级复制给用户手动粘贴。");
        self.place_in_clipboard(&text)?;
        Ok(SnapshotReplaceOutcome::CopiedFallback)
    }
}

fn replacement_took_effect(current_selected_text: Option<&str>, source_text: &str) -> bool {
    !matches!(current_selected_text, Some(current) if current == source_text)
}

impl TextInjector for MacTextInjector {
    async fn inject_at_cursor(&self, text: String) -> Result<(), TextInjectionError> {
        let has_confirmed_selection = super::selection::read_selected_text().is_some();
        if has_confirmed_selection && inject_text_via_accessibility(&text).is_ok() {
            eprintln!("[TextInjector] AX 直插成功。");
            return Ok(());
        }
        eprintln!("[TextInjector] 使用合成粘贴兜底。");
        safe_synthetic_paste(&text).await
    }

    fn place_in_clipboard(&self, text: &str) -> Result<(), TextInjectionError> {
        super::clipboard::write_text(text);
        Ok(())
    }
}

type AXUIElementRef = *mut c_void;
type AXError = i32;
const AX_ERROR_SUCCESS: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
}

fn inject_text_via_accessibility(text: &str) -> Result<(), TextInjectionError> {
    // SAFETY: AX Copy/Create results are wrapped with create-rule ownership; focused element is checked
    // for null and only used for the immediate setAttribute call.
    unsafe {
        let system_ref = AXUIElementCreateSystemWide();
        if system_ref.is_null() {
            return Err(TextInjectionError::AccessibilityUnavailable);
        }
        let _system_guard = core_foundation::base::CFType::wrap_under_create_rule(system_ref);

        let focused_attr = CFString::new("AXFocusedUIElement");
        let mut focused_ref: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            system_ref,
            focused_attr.as_concrete_TypeRef(),
            &mut focused_ref,
        );
        if err != AX_ERROR_SUCCESS || focused_ref.is_null() {
            return Err(TextInjectionError::AccessibilityUnavailable);
        }
        let _focused_guard = core_foundation::base::CFType::wrap_under_create_rule(focused_ref);

        let selected_text_attr = CFString::new("AXSelectedText");
        let replacement = CFString::new(text);
        let err = AXUIElementSetAttributeValue(
            focused_ref as AXUIElementRef,
            selected_text_attr.as_concrete_TypeRef(),
            replacement.as_CFTypeRef(),
        );
        if err == AX_ERROR_SUCCESS {
            Ok(())
        } else {
            Err(TextInjectionError::AccessibilityUnavailable)
        }
    }
}

async fn safe_synthetic_paste(text: &str) -> Result<(), TextInjectionError> {
    const POLL_INTERVAL_MS: u64 = 10;
    const POLL_TIMEOUT_MS: u64 = 150;
    const SLOW_APP_BUFFER_MS: u64 = 80;

    let _lock = CLIPBOARD_MUTEX.lock().await;
    let backup = super::clipboard::read_all_types();
    let start_change_count = super::clipboard::change_count();

    super::clipboard::write_text_transient(text);
    let paste_result = super::synthetic::post_synthetic_paste()
        .map_err(|e| TextInjectionError::SyntheticPasteFailed(e.to_string()));

    for _ in 0..(POLL_TIMEOUT_MS / POLL_INTERVAL_MS) {
        if super::clipboard::change_count() != start_change_count {
            break;
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }

    tokio::time::sleep(Duration::from_millis(SLOW_APP_BUFFER_MS)).await;
    super::clipboard::restore_all_types(&backup);
    paste_result
}

#[cfg(test)]
mod tests {
    use super::replacement_took_effect;

    #[test]
    fn replacement_confirmation_rejects_unchanged_source_selection() {
        assert!(!replacement_took_effect(Some("hello"), "hello"));
    }

    #[test]
    fn replacement_confirmation_accepts_changed_or_cleared_selection() {
        assert!(replacement_took_effect(Some("你好"), "hello"));
        assert!(replacement_took_effect(None, "hello"));
    }
}
