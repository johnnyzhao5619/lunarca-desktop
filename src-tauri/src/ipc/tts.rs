//! 文本朗读（TTS）命令域（IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 事实源与编排：朗读控制器 [`crate::tts`]（播放型系统引擎，非 JobQueue；ADR-022 / TDD-05 §9）。
//! 本模块只做 Tauri 命令边界（收发/校验/设置读取/语言解析）+ 转调控制器；业务逻辑不在此（ADR-003）。
//!
//! 三表面共用同一控制器（单一发声者）：弹窗翻译 HUD 原文/译文发音、笔记编辑器「朗读选区」、
//! 「全文朗读」。前端据 `tts_speak` 返回的 `session_id` 认领当前发声会话。

use super::*;
use crate::tts::{bcp47_for_voice, detect_sample, plan_speech, TtsStateChanged};

/// 本平台是否支持朗读（仅 macOS 有原生实现）。前端据此隐藏/禁用朗读入口。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_available() -> bool {
    crate::tts::TtsController::available()
}

/// 开始朗读 `text`。
///
/// - `markdown`：`true` 时按 Markdown 剥离（跳代码块/去标题井号与行内标记）后朗读，`false` 为纯文本。
/// - `lang`：显式语言（BCP-47 或主子标签如 `"zh"`/`"en"`）；`None`/空串则用本地检测器判定源语言
///   （复用 `AppState.detector`，与弹窗翻译分支判定同源）。最终规整为区域化 BCP-47 供选嗓音。
/// - `scope`：发起表面标识（如 `hud:source`/`hud:translation`/`editor:<path>`），随状态事件回传供前端认领。
///
/// 语速取设置 `speech.rate`（百分比，默认 100）。返回新会话状态快照（含 `session_id`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_speak(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
    markdown: bool,
    lang: Option<String>,
    scope: String,
) -> TtsStateChanged {
    let settings = state.settings_snapshot();
    let rate_pct = settings.get_int("speech.rate").unwrap_or(100);

    // 断句规划在命令层完成（纯逻辑）：控制器只管播放状态机，且检测可据此只取正文样本。
    let segments = plan_speech(&text, markdown);

    // 语言解析：显式优先，否则本地检测；均得不到则 None（系统默认嗓音）。
    // 检测**只送开头若干句的样本**——lingua 是 CPU 密集且本命令同步执行，整篇长文会拖慢
    // 「点击即读」的响应；语言不会在文档中途改变（`detect_sample` 单测锁定上限）。
    let primary = lang.filter(|value| !value.trim().is_empty()).or_else(|| {
        state
            .detector
            .detect(&detect_sample(&segments))
            .reliable_lang()
            .map(|detected| detected.as_str().to_string())
    });
    let voice_lang = primary.map(|code| bcp47_for_voice(&code));

    state.tts.speak(&app, segments, voice_lang, scope, rate_pct)
}

/// 暂停当前朗读。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_pause(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> TtsStateChanged {
    state.tts.pause(&app)
}

/// 继续朗读。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_resume(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> TtsStateChanged {
    state.tts.resume(&app)
}

/// 停止朗读并复位空闲。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_stop(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> TtsStateChanged {
    state.tts.stop(&app)
}

/// 当前朗读状态快照。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_state(state: tauri::State<'_, AppState>) -> TtsStateChanged {
    state.tts.state()
}

/// 调整当前朗读语速（百分比 50–200；对后续句即时生效）。持久化偏好由前端另经 `set_setting` 写回。
#[tauri::command]
#[specta::specta]
pub(crate) fn tts_set_rate(state: tauri::State<'_, AppState>, rate_pct: i32) {
    state.tts.set_rate(i64::from(rate_pct));
}
