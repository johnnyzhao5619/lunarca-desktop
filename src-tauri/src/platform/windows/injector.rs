//! Windows `TextInjector`（ADR-163 ③、TDD-05 §2）：注入前核对目标窗口仍在前台 →
//! `SendInput` + `KEYEVENTF_UNICODE` 逐字输入；长文本、含换行或输入法开着时改走
//! 「备份剪贴板 → 写入 → 合成 Ctrl+V → 还原」。核对与方式选择是 `routing` 里的纯函数。
//!
//! 弹窗「替换原文」另多一步：目标控件暴露选区时，先核对选中的仍是触发时那段文字
//! （`routing::check_selection_snapshot`），变了就不打——打进去是插入而不是替换。

use super::clipboard::{self, ClipboardRestore};
use super::routing::{
    check_selection_snapshot, choose_inject_method, decide_inject_route, InjectFallback,
    InjectMethod, InjectRoute, SnapshotCheck,
};
use super::{foreground, input, keys, selection};
use crate::platform::{TextInjectionError, TextInjector, CLIPBOARD_MUTEX};
use std::time::Duration;

/// 把前台还给目标窗口后，等它真正到前台再核对一次。
const REACTIVATE_SETTLE: Duration = Duration::from_millis(80);
/// 合成按键前等用户松开修饰键的上限（按住说话松手后、热键刚触发时）。
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
/// 合成 Ctrl+V 之后、还原剪贴板之前的等待：Windows 没有「对方已读走」的信号，慢的应用
/// （Electron 系）在按键后一两百毫秒才读剪贴板，过早还原会把用户原来的内容粘进去。
const PASTE_SETTLE: Duration = Duration::from_millis(300);

/// 注入成功时，用户的剪贴板怎么样了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectReport {
    /// 走了粘贴时的剪贴板还原结论；逐字输入没碰剪贴板为 `None`。
    pub clipboard: Option<ClipboardRestore>,
}

impl InjectReport {
    /// 用户原来的剪贴板完好（没碰过，或原样还原）。
    #[must_use]
    pub fn clipboard_intact(self) -> bool {
        self.clipboard.is_none_or(ClipboardRestore::is_intact)
    }
}

/// 注入没做成的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectFailure {
    /// 前台核对没过（目标没了、切走了、是管理员窗口）。
    Fallback(InjectFallback),
    /// 「替换原文」时源选区已经变了。
    SelectionChanged,
    /// 系统没收下合成的按键。
    SendFailed,
}

impl From<InjectFailure> for TextInjectionError {
    fn from(failure: InjectFailure) -> Self {
        match failure {
            InjectFailure::Fallback(_) => Self::TargetNotInFront,
            InjectFailure::SelectionChanged => Self::SelectionSnapshotInvalid,
            InjectFailure::SendFailed => {
                Self::SyntheticPasteFailed("SendInput rejected the events".into())
            }
        }
    }
}

/// Windows 文本注入器。
#[derive(Debug, Default, Clone, Copy)]
pub struct WinTextInjector;

/// 核对前台；前台在我们自己窗口上时还回去再核一次。
async fn ensure_target_in_front(target: Option<isize>) -> Result<(), InjectFailure> {
    match decide_inject_route(target, foreground::facts_for(target), true) {
        InjectRoute::Inject => Ok(()),
        InjectRoute::Fallback(reason) => Err(InjectFailure::Fallback(reason)),
        InjectRoute::Reactivate => {
            if let Some(target) = target {
                foreground::bring_to_front(target);
            }
            tokio::time::sleep(REACTIVATE_SETTLE).await;
            match decide_inject_route(target, foreground::facts_for(target), false) {
                InjectRoute::Inject => Ok(()),
                InjectRoute::Fallback(reason) => Err(InjectFailure::Fallback(reason)),
                // `may_reactivate = false` 不会再要求还前台。
                InjectRoute::Reactivate => {
                    Err(InjectFailure::Fallback(InjectFallback::TargetChanged))
                }
            }
        }
    }
}

async fn type_unicode(text: String) -> Result<InjectReport, InjectFailure> {
    tokio::task::spawn_blocking(move || {
        let held = input::wait_for_modifiers_released(MODIFIER_RELEASE_TIMEOUT);
        input::send(&keys::unicode_strokes(&held, &text))
    })
    .await
    .map_err(|_| InjectFailure::SendFailed)?
    .map_err(|_| InjectFailure::SendFailed)?;
    Ok(InjectReport { clipboard: None })
}

async fn paste_via_clipboard(text: String) -> Result<InjectReport, InjectFailure> {
    let _lock = CLIPBOARD_MUTEX.lock().await;
    let backup = tokio::task::spawn_blocking(move || {
        let backup = clipboard::backup();
        if !clipboard::write_text_transient(&text) {
            return (backup, false);
        }
        let held = input::wait_for_modifiers_released(MODIFIER_RELEASE_TIMEOUT);
        let sent = input::send(&keys::chord_strokes(&held, keys::VK_V)).is_ok();
        (backup, sent)
    })
    .await
    .map_err(|_| InjectFailure::SendFailed)?;
    let (backup, sent) = backup;
    if sent {
        tokio::time::sleep(PASTE_SETTLE).await;
    }
    let restore = match backup {
        Some(backup) => {
            let complete = backup.is_complete();
            let ok = tokio::task::spawn_blocking(move || clipboard::restore(&backup))
                .await
                .unwrap_or(false);
            clipboard::restore_report(complete, ok)
        }
        // 备份时剪贴板就打不开：原来的内容无从还原。
        None => ClipboardRestore::Failed,
    };
    if !sent {
        return Err(InjectFailure::SendFailed);
    }
    Ok(InjectReport {
        clipboard: Some(restore),
    })
}

impl WinTextInjector {
    /// 把文本打进目标窗口的光标处（语音速记「注入光标」）。
    ///
    /// # Errors
    /// 前台核对没过、或系统没收下按键时返回 [`InjectFailure`]；调用方据此改落剪贴板。
    pub async fn inject_into(
        &self,
        target: Option<isize>,
        text: String,
    ) -> Result<InjectReport, InjectFailure> {
        ensure_target_in_front(target).await?;
        let ime = target.is_some_and(foreground::ime_is_composing_language);
        let method = choose_inject_method(&text, ime);
        eprintln!("[TextInjector] 注入 → method={method:?} len={}", text.len());
        match method {
            InjectMethod::UnicodeTyping => type_unicode(text).await,
            InjectMethod::ClipboardPaste => paste_via_clipboard(text).await,
        }
    }

    /// 「替换原文」：核对前台与源选区后，用译文替换目标控件里仍选中的那段文字。
    ///
    /// # Errors
    /// 前台核对没过、源选区已变、或系统没收下按键时返回 [`InjectFailure`]。
    pub async fn replace_selection(
        &self,
        target: Option<isize>,
        source_text: &str,
        text: String,
    ) -> Result<InjectReport, InjectFailure> {
        ensure_target_in_front(target).await?;
        let current = tokio::task::spawn_blocking(selection::read_uia_selection)
            .await
            .unwrap_or(super::routing::UiaSelection::Unavailable);
        if check_selection_snapshot(&current, source_text) == SnapshotCheck::Changed {
            return Err(InjectFailure::SelectionChanged);
        }
        self.inject_into(target, text).await
    }
}

impl TextInjector for WinTextInjector {
    async fn inject_at_cursor(&self, text: String) -> Result<(), TextInjectionError> {
        self.inject_into(foreground::foreground_window(), text)
            .await
            .map(|_| ())
            .map_err(TextInjectionError::from)
    }

    fn place_in_clipboard(&self, text: &str) -> Result<(), TextInjectionError> {
        if clipboard::write_text(text) {
            Ok(())
        } else {
            Err(TextInjectionError::ClipboardUnavailable)
        }
    }
}
