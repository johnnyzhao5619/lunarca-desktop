//! macOS 服务「用 Lunarca 存为速记」的应用层（笔记库对标轮 W5b）。FFI 在
//! `platform::macos::services`；这里决定存到哪、怎么告诉用户。
//!
//! - 落点与文字速记窗口**同一条**：[`capture_text_note`] + `Drafts` 目的地，落主库。
//! - 发起服务的那个应用此刻在等我们：回调里只起一个线程就返回，落盘在那个线程里做。
//! - 结果走通知中心：存下了给「已存为速记」（落点直达那一篇），没存下（库没打开、文字是空的）
//!   给失败通知。Lunarca 在后台时由系统横幅送达。参数不含正文。

#[cfg(target_os = "macos")]
use super::*;

/// 在应用 setup 里调用一次（主线程）。非 macOS 平台没有「服务」菜单，什么都不做。
pub(crate) fn install_quick_note_service(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        crate::platform::macos::services::register(Box::new(move |kind, text| {
            let app = handle.clone();
            match kind {
                crate::platform::macos::services::ServiceKind::QuickNote => {
                    std::thread::spawn(move || save_service_text(&app, text.unwrap_or_default()));
                }
                // 提词器（ADR-176 ⑨）：服务消息本就在主线程投递；建稿很快，开窗要在主线程。
                crate::platform::macos::services::ServiceKind::Teleprompter => {
                    super::send_text_to_teleprompter(&app, text);
                }
            }
        }));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
fn save_service_text(app: &tauri::AppHandle, text: String) {
    use crate::notifications::{NotificationCenter, NotificationEvent, NotificationTarget};
    use tauri::Manager as _;
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let event = match capture_text_note(app, &state, "main", &text, CaptureDestinationDto::Drafts) {
        Ok(doc) => NotificationEvent::completion(
            "service_quick_note_saved",
            NotificationTarget::Route(format!("quick-notes://{}", doc.path)),
            crate::notifications::PrivatePayload::new(String::new()),
        ),
        Err(_) => {
            NotificationEvent::failure("service_quick_note_failed", NotificationTarget::Workspace)
        }
    };
    if let Some(center) = app.try_state::<NotificationCenter>() {
        center.publish(event, app);
    }
}

#[cfg(test)]
mod tests {
    /// 服务登记散在四处，任一处对不上，菜单项要么不出现、要么点了没反应，而且都不报错：
    /// `Info.plist` 的 `NSMessage` ↔ 提供者的选择子；菜单名的键 ↔ 每份 `ServicesMenu.strings`；
    /// 每份 `.strings` ↔ `tauri.conf.json` 里把它打进包的那一行。
    #[test]
    fn the_service_registration_matches_the_provider_and_its_menu_names() {
        let root = env!("CARGO_MANIFEST_DIR");
        let read = |rel: &str| std::fs::read_to_string(format!("{root}/{rel}")).unwrap();
        let plist = read("Info.plist");
        let provider = read("src/platform/macos/services.rs");
        let conf = read("tauri.conf.json");
        for (message, default) in [
            ("saveAsQuickNote", "Save to Lunarca as Quick Note"),
            ("showInTeleprompter", "Show in Lunarca Teleprompter"),
        ] {
            assert!(
                plist.contains(&format!("<string>{message}</string>")),
                "{message}"
            );
            assert!(
                provider.contains(&format!("method({message}:userData:error:)")),
                "{message}"
            );
            assert!(
                plist.contains(&format!("<string>{default}</string>")),
                "{default}"
            );
            for lang in ["zh-Hans", "en"] {
                let strings = read(&format!("macos/{lang}.lproj/ServicesMenu.strings"));
                assert!(
                    strings.contains(&format!("\"{default}\" = ")),
                    "{lang} {default}"
                );
            }
        }
        for lang in ["zh-Hans", "en"] {
            assert!(
                conf.contains(&format!(
                    "\"Resources/{lang}.lproj/ServicesMenu.strings\": \"macos/{lang}.lproj/ServicesMenu.strings\""
                )),
                "{lang} 没打进包"
            );
        }
    }
}
