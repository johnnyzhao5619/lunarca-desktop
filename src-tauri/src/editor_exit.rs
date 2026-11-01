//! 退出前收齐编辑窗口的保存回执；失败、缺失或陈旧回执均不能放行退出。
use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex};
use tauri::Manager;
use tauri_specta::Event;

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Pending {
        id: u32,
        windows: BTreeSet<String>,
    },
    Approved,
}
#[derive(Default)]
struct Coordinator {
    phase: Phase,
    sequence: u32,
}
impl Coordinator {
    fn start(&mut self, windows: BTreeSet<String>) -> Option<u32> {
        if !matches!(self.phase, Phase::Idle) || windows.is_empty() {
            return None;
        }
        self.sequence += 1;
        self.phase = Phase::Pending {
            id: self.sequence,
            windows,
        };
        Some(self.sequence)
    }
    fn reply(&mut self, id: u32, window: &str, saved: bool) -> bool {
        let Phase::Pending {
            id: current,
            windows,
        } = &mut self.phase
        else {
            return false;
        };
        if id != *current || !windows.remove(window) {
            return false;
        }
        if !saved {
            self.phase = Phase::Idle;
            return false;
        }
        if windows.is_empty() {
            self.phase = Phase::Approved;
            return true;
        }
        false
    }
    fn cancel(&mut self, id: u32) -> bool {
        if matches!(self.phase, Phase::Pending { id: current, .. } if current == id) {
            self.phase = Phase::Idle;
            return true;
        }
        false
    }
    fn take_approval(&mut self) -> bool {
        if matches!(self.phase, Phase::Approved) {
            self.phase = Phase::Idle;
            return true;
        }
        false
    }
}
static EXIT: LazyLock<Mutex<Coordinator>> = LazyLock::new(|| Mutex::new(Coordinator::default()));

#[cfg(target_os = "macos")]
pub(crate) fn install_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
    let handle = app.handle();
    let menu = Menu::default(handle)?;
    let quit = MenuItem::with_id(
        handle,
        "editor_quit",
        app.state::<std::sync::Arc<crate::i18n::BackendI18n>>()
            .translate("tray:quit")
            .unwrap_or_else(|| "Quit Lunarca".into()),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    let application = Submenu::with_items(
        handle,
        handle.package_info().name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(
                handle,
                None,
                Some(AboutMetadata {
                    name: Some(handle.package_info().name.clone()),
                    version: Some(handle.package_info().version.to_string()),
                    copyright: handle.config().bundle.copyright.clone(),
                    authors: handle.config().bundle.publisher.clone().map(|p| vec![p]),
                    ..Default::default()
                }),
            )?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &quit,
        ],
    )?;
    // Tauri 默认应用菜单位于首项；原生 quit 直接调用 NSApp terminate，绕过退出回执。
    menu.remove_at(0)?;
    menu.insert(&application, 0)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id.as_ref() == "editor_quit" {
            app.exit(0);
        }
    });
    Ok(())
}

pub(crate) fn request(app: &tauri::AppHandle, api: &tauri::ExitRequestApi) {
    let mut coordinator = EXIT.lock().expect("exit coordinator");
    if coordinator.take_approval() {
        return;
    }
    let windows: BTreeSet<_> = app
        .webview_windows()
        .into_keys()
        .filter(|label| {
            label == "main"
                || label.starts_with("note-editor-")
                || label.starts_with("vault-window-")
        })
        .collect();
    if windows.is_empty() {
        return;
    }
    api.prevent_exit();
    let Some(id) = coordinator.start(windows.clone()) else {
        return;
    };
    drop(coordinator);
    for window in windows {
        if (crate::ipc::EditorFlushRequested { request_id: id })
            .emit_to(app, window)
            .is_err()
        {
            EXIT.lock().expect("exit coordinator").cancel(id);
            let _ = crate::ipc::EditorFlushCancelled.emit(app);
            crate::show_main_window(app);
            return;
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        if EXIT.lock().expect("exit coordinator").cancel(id) {
            let _ = crate::ipc::EditorFlushCancelled.emit(&app);
            crate::show_main_window(&app);
        }
    });
}

pub(crate) fn reply(app: &tauri::AppHandle, window: &str, id: u32, saved: bool) {
    let mut coordinator = EXIT.lock().expect("exit coordinator");
    let valid = matches!(&coordinator.phase, Phase::Pending { id: current, windows } if *current == id && windows.contains(window));
    let approved = coordinator.reply(id, window, saved);
    drop(coordinator);
    if approved {
        app.exit(0);
    } else if !saved && valid {
        let _ = crate::ipc::EditorFlushCancelled.emit(app);
        if let Some(window) = app.get_webview_window(window) {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn windows() -> BTreeSet<String> {
        ["main".into(), "note-editor-1".into()].into()
    }
    #[test]
    fn all_windows_must_ack_before_exit() {
        let mut c = Coordinator::default();
        let id = c.start(windows()).unwrap();
        assert!(!c.reply(id, "main", true));
        assert!(!c.take_approval());
        assert!(!c.reply(id, "main", true));
        assert!(!c.reply(id, "wrong-window", true));
        assert!(c.reply(id, "note-editor-1", true));
        assert!(c.take_approval());
        assert!(!c.take_approval());
    }
    #[test]
    fn failure_timeout_and_stale_reply_never_approve() {
        let mut c = Coordinator::default();
        let id = c.start(windows()).unwrap();
        assert!(!c.reply(id, "main", false));
        assert!(!c.take_approval());
        let next = c.start(windows()).unwrap();
        assert!(next > id);
        assert!(!c.reply(id, "note-editor-1", true));
        assert!(c.cancel(next));
        assert!(!c.reply(next, "main", true));
        assert!(!c.take_approval());
    }
}
