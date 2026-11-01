//! macOS 原生系统集成实现（TDD-05）。`cfg(target_os = "macos")`。
//!
//! S1 spike 范围：[`keytap`]（`CGEventTap` 双击 `Cmd+C` 被动观测）。AX 选区捕获 /
//! `TextInjector` / NSPanel 为后续子步骤。

pub mod appearance;
pub mod clipboard;
pub mod gpu;
pub mod injector;
pub mod keytap;
pub mod nspanel;
pub mod ocr;
pub mod permissions;
pub mod power_network;
pub mod screen_capture_geometry;
#[cfg(debug_assertions)]
pub mod screen_capture_probe;
pub mod selection;
pub mod services;
pub mod spelling;
pub mod synthetic;
pub mod system_audio_tap;
pub mod tts;
pub mod voice_process_input;

pub use appearance::apply_app_appearance;
pub use injector::MacTextInjector;
pub use keytap::CGEventTapKeyTap;
pub use nspanel::{
    convert_to_nonactivating_panel, enable_input_mode_focus, set_ignores_mouse_events,
    set_text_editing_level,
};
// 辅助功能 trust + 权限管理器单一来源（DRY，原 is_process_trusted/prompt 在 keytap，已收口此处）。
pub use permissions::{is_process_trusted, prompt_accessibility_trust, MacPermissionManager};
pub use synthetic::{post_synthetic_copy, post_synthetic_paste, SyntheticError};
