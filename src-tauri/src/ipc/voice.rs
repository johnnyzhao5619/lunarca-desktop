//! 语音输入命令域（PRD-02，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：语音输入会话的起停与落地（HUD 显隐与钉住、麦克风权限、术语表加载、
//! 速记笔记的文件名与正文成型）。采集与引擎在 [`crate::voice_input`]，本模块只做 IPC 面。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;
use luna_sync_core::naming::sanitized_file_stem;

pub(crate) fn voice_start_options_from_settings(settings: &Settings) -> VoiceInputStartOptions {
    let lang_hint = settings
        .get_str("voice_input.lang_hint")
        .filter(|lang| !lang.trim().is_empty() && lang != "auto")
        .map(|lang| Lang::from(lang.as_str()));
    VoiceInputStartOptions {
        landing_mode: LandingMode::from_setting(settings.get_str("voice_input.landing_mode")),
        input_mode: VoiceInputMode::from_setting(settings.get_str("voice_input.input_mode")),
        auto_refine: settings.get_bool("voice_input.auto_refine").unwrap_or(true),
        translate_enabled: settings
            .get_bool("voice_input.translate_enabled")
            .unwrap_or(false),
        translate_target_lang: {
            let lang = settings
                .get_str("voice_input.translate_target_lang")
                .filter(|lang| !lang.trim().is_empty())
                .unwrap_or_else(|| "en".to_string());
            Lang::from(lang.as_str())
        },
        refine_mode: VoiceRefineMode::General,
        glossary: Vec::new(),
        lang_hint,
        audio_capture_dir: None,
        vad: None,
        sink: VoiceInputSink::CursorOrClipboard,
        mic_holder: MicHolder::VoiceInput,
        popup_session: None,
        save_quick_note: None,
        client_session_id: None,
        // 采集调参与录音/实时字幕**同一条投影**（波形帧率、VAD 灵敏度…，见 capture_tuning）。
        tuning: capture_tuning(settings),
        // 流式实时识别 Provider 句柄由命令层在 ensure_asr_provider_ready 后注入。
        asr_provider: None,
    }
}

/// 「应用内输入框」一类落点的启动选项（`is_field_sink`）：原话、不过自动优化与翻译。
fn field_voice_start_options_from_settings(
    settings: &Settings,
    sink: VoiceInputSink,
    mic_holder: MicHolder,
) -> VoiceInputStartOptions {
    let mut options = voice_start_options_from_settings(settings);
    options.sink = sink;
    options.mic_holder = mic_holder;
    options.auto_refine = false;
    options.translate_enabled = false;
    options.refine_mode = VoiceRefineMode::Raw;
    options
}

fn popup_voice_start_options_from_settings(
    settings: &Settings,
    popup_session: Arc<Mutex<PopupSession>>,
) -> VoiceInputStartOptions {
    let mut options = field_voice_start_options_from_settings(
        settings,
        VoiceInputSink::PopupTranslateInput,
        MicHolder::PopupVoiceButton,
    );
    options.popup_session = Some(popup_session);
    options
}

pub(crate) fn load_glossary_entries(state: &AppState) -> Vec<GlossaryEntry> {
    let root = {
        let Ok(guard) = state.vault.lock() else {
            return Vec::new();
        };
        let Some(vault) = guard.as_ref() else {
            return Vec::new();
        };
        vault.root().to_path_buf()
    };
    let Ok(values) = read_meta::<serde_json::Value>(&root, "glossary") else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value::<GlossaryEntry>(value).ok())
        .filter(|entry| !entry.term.trim().is_empty() && !entry.canonical.trim().is_empty())
        .collect()
}

fn voice_quick_note_saver(
    app: tauri::AppHandle,
    state: &AppState,
) -> crate::voice_input::SaveQuickNoteFn {
    let vault_arc = state.vault_arc();
    // `'static` 闭包在落盘那一刻读切库过渡态真值（边界⑤：措辞按此刻真实原因）。
    let switching = state.vault_switching_flag();
    // 文案来源随闭包一起走（`Arc`）：这句话是给人读的，不能只发一个码。
    let copy_for_save = state.backend_copy_arc();
    let settings = state.settings_snapshot();
    let i18n = backend_copy_arc(&app);
    let lang = i18n
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string());
    Arc::new(move |draft: VoiceQuickNoteDraft| {
        let body = voice_quick_note_body(&draft, &lang);
        if body.trim().is_empty() {
            return Err(coded_error(&app, "EMPTY_RESULT", "voice-empty-result"));
        }
        let dir = note_type_dir(&settings, NoteType::QuickNote);
        // 默认名带日期时间 + 内容摘要（PRD-02-020：便于用户之后按时间归档检索）。
        let rel_path = format!(
            "{dir}/{}",
            voice_quick_note_stem(&draft, &lang, chrono::Local::now())
        );
        // 库锁只罩住落盘与录音归档这一段，**出了这个块才做派生更新**：派生更新的第一步
        // （AI 标签刷新）会再锁同一把库锁，std Mutex 不可重入——在锁内调用就是同线程死锁，
        // 而死锁的那条线程攥着库锁，之后任何要读库的命令都排不进去，整个应用卡死
        // （真机 2026-10-02：语音「存入速记笔记」后应用无响应）。
        let doc = {
            let guard = vault_arc.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| {
                vault_absent_message(
                    copy_for_save.as_deref(),
                    switching.load(std::sync::atomic::Ordering::Acquire),
                )
            })?;
            let doc = create_unique_note(
                copy_for_save.as_deref(),
                vault,
                &rel_path,
                NoteType::QuickNote,
                body,
            )?;
            // 原始录音随笔记归档（PRD-02-030）：搬进该笔记的附件目录并登记为媒体——从此这篇
            // 速记与录音笔记同构：能回放、能二次转写（PRD-02-032）。登记失败不让落盘失败：
            // 文字已经在了，音频是增强。
            if let Some(audio) = draft.audio.as_ref() {
                if let Err(e) = register_quick_note_audio(vault, &settings, &doc.path, audio) {
                    eprintln!("[voice] 速记录音归档失败（笔记已保存）：{e}");
                }
            }
            doc
        };
        emit_vault_changed(&app, &doc.path, false);
        // 速记直接作证据（ADR-136 ①）：落盘即走同一条派生更新，不等下一次编辑器保存。
        super::notes::refresh_derived_after_write(&app, &doc.path);
        // AI 提炼（PRD-06-058 C′）：默认关；开了就后台跑一次并写回这一篇，落盘本身不等它。
        // 读**落盘这一刻**的设置而不是闭包建好时的快照：这枚开关就在 HUD 的设置面板里，
        // 会话中间被拨动是常态。
        if app
            .state::<AppState>()
            .settings_snapshot()
            .get_bool("voice_input.quick_note_refine")
            .unwrap_or(false)
        {
            let app = app.clone();
            let note_rel = doc.path.clone();
            tauri::async_runtime::spawn(async move {
                super::quick_note_refine::refine_quick_note(app, note_rel).await;
            });
        }
        Ok(doc.path)
    })
}

/// 把会话录音搬进速记的附件目录并登记为媒体（零段：只登记媒体，转写块留空待二次转写）。
fn register_quick_note_audio(
    vault: &luna_vault::Vault,
    settings: &Settings,
    note_rel: &str,
    audio: &crate::voice_input::VoiceAudioFile,
) -> Result<(), String> {
    let stem = Path::new(note_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("voice");
    let asset_rel = format!("{}/{stem}.wav", media_asset_dir(settings, note_rel));
    let dest = vault.root().join(&asset_rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("VOICE_AUDIO_DIR: {e}"))?;
    }
    // 跨卷（缓存目录与库不在一块盘）rename 会失败：退回拷贝 + 删源。
    if std::fs::rename(&audio.path, &dest).is_err() {
        std::fs::copy(&audio.path, &dest).map_err(|e| format!("VOICE_AUDIO_MOVE: {e}"))?;
        let _ = std::fs::remove_file(&audio.path);
    }
    vault
        .finalize_recording(
            note_rel,
            &[luna_vault::RecordingPart {
                path: asset_rel,
                start_ms: 0,
                end_ms: audio.duration_ms,
                duration: Some(audio.duration_ms as f64 / 1000.0),
                model: None,
                lang: None,
                tracks: None,
            }],
            &[],
            &[],
            luna_vault::TranscriptAuthority::Preview,
        )
        .map_err(|e| e.to_string())
}

/// 语音速记默认文件名（PRD-02-020）：`<本地化前缀> YYYY-MM-DD HHMM[ <内容摘要>]`。
///
/// 时间在名内且靠前——速记多靠"什么时候说的"归档检索，按名排序即按时间排序；
/// 摘要取最终输出文本（非正文标题行）的前 [`NOTE_STEM_EXCERPT_CHARS`] 字符，
/// 经 [`sanitized_file_stem`] 清洗文件系统保留字符；无可用摘要时仅前缀 + 时间。
/// 时间刻意不含 `:`（文件名非法）与 `.`（`create_unique_note` 经 `file_stem` 取名，
/// 点会被当扩展名截断）。重名由 `create_unique_note` 追加序号兜底。
pub(crate) fn voice_quick_note_stem(
    draft: &VoiceQuickNoteDraft,
    lang: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    // 摘要源 = 用户实际会读到的最终输出（编辑稿/优化/译文），空则退回原始转录。
    let source = if draft.final_text.trim().is_empty() {
        draft.raw.trim()
    } else {
        draft.final_text.trim()
    };
    quick_note_stem(voice_capture_prefix(lang), source, now)
}

/// 语音捕获的文件名前缀（本地化）。
pub(crate) fn voice_capture_prefix(lang: &str) -> &'static str {
    if lang.starts_with("zh") {
        "语音速记"
    } else {
        "Voice note"
    }
}

/// 文字捕获的文件名前缀（PRD-06-058 B 的捕获框）。
///
/// 与语音那条**分前缀而不是分格式**：命名惯例（`<前缀> 日期 时间 摘要`）只有一套，
/// 前缀老实说出这条是敲进来的还是说出来的。共用一个前缀会让收件箱里每一行都自称语音，
/// 而那是文件名在说谎——文件名是用户三个月后唯一还看得见的线索。
pub(crate) fn text_capture_prefix(lang: &str) -> &'static str {
    if lang.starts_with("zh") {
        "速记"
    } else {
        "Quick note"
    }
}

/// 速记文件名的**唯一**命名惯例（PRD-02-020）：`<前缀> YYYY-MM-DD HHMM[ <内容摘要>]`。
///
/// 语音捕获与文字捕获共用这一份——第二套命名的代价不是不好看：同一个收件箱里两种文件名
/// 格式，按名排序就不再等于按时间排序，而那正是这套命名存在的全部理由。
pub(crate) fn quick_note_stem(
    prefix: &str,
    excerpt_source: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let time_str = now.format("%Y-%m-%d %H%M").to_string();
    let excerpt = luna_sync_core::naming::title_line(excerpt_source)
        .map(|line| {
            line.chars()
                .take(NOTE_STEM_EXCERPT_CHARS)
                .collect::<String>()
        })
        .and_then(|line| sanitized_file_stem(&line));
    match excerpt {
        Some(excerpt) => format!("{prefix} {time_str} {excerpt}"),
        None => format!("{prefix} {time_str}"),
    }
}

/// 语音速记正文里「原始转录」一节的小标题（中 / 英）。蒸馏时按它把这一节剔掉
/// （`distill::draft_distill_input`）：它与「优化结果」是同一段话的两个版本。
/// 与 `luna_vault::quick_note_products` 读回用的是同一份标题（PRD-02-033）。
pub(crate) const VOICE_RAW_HEADINGS: &[&str] = luna_vault::QUICK_NOTE_RAW_HEADINGS;

pub(crate) fn voice_quick_note_body(draft: &VoiceQuickNoteDraft, lang: &str) -> String {
    let final_text = draft.final_text.trim();
    let raw = draft.raw.trim();
    if final_text.is_empty() {
        return raw.to_string();
    }
    if raw.is_empty() || raw == final_text || matches!(draft.output_kind, VoiceInputOutputKind::Raw)
    {
        return final_text.to_string();
    }

    let is_zh = lang.starts_with("zh");
    let final_heading = match draft.output_kind {
        VoiceInputOutputKind::Translated => {
            if is_zh {
                "译文"
            } else {
                "Translation"
            }
        }
        VoiceInputOutputKind::Refined | VoiceInputOutputKind::Raw => {
            if is_zh {
                "优化结果"
            } else {
                "Polished text"
            }
        }
    };
    let raw_heading = if is_zh {
        VOICE_RAW_HEADINGS[0]
    } else {
        VOICE_RAW_HEADINGS[1]
    };
    format!("## {final_heading}\n\n{final_text}\n\n## {raw_heading}\n\n{raw}")
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_state(state: tauri::State<'_, AppState>) -> VoiceInputState {
    state.voice_input.state()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_set_mode(
    state: tauri::State<'_, AppState>,
    landing_mode: LandingMode,
    input_mode: VoiceInputMode,
    auto_refine: bool,
    translate_enabled: bool,
    translate_target_lang: String,
) -> Result<VoiceInputState, String> {
    state.voice_input.set_mode(
        landing_mode,
        input_mode,
        auto_refine,
        translate_enabled,
        translate_target_lang,
    );
    Ok(state.voice_input.state())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_subscribe_levels(
    state: tauri::State<'_, AppState>,
    channel: Channel<RawBytes>,
) -> Result<(), String> {
    state.voice_input.subscribe_levels(channel)
}

/// 采集前确保麦克风已授权：未授权则发起系统授权请求并返回中文引导（DRY：语音输入与弹窗
/// 语音按钮共用，避免两处复制权限检查）。非 macOS 平台暂为 no-op。
pub(crate) fn ensure_microphone_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::permissions::{
            PermissionKind, PermissionManager as _, PermissionStatus,
        };
        let manager = crate::platform::macos::MacPermissionManager;
        let status = manager.check(PermissionKind::Microphone);
        if status != PermissionStatus::Granted {
            manager.request(PermissionKind::Microphone);
            let report = crate::platform::permissions::report(PermissionKind::Microphone, status);
            return Err(report.guidance_zh);
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    client_session_id: Option<String>,
    refine_mode: VoiceRefineMode,
    on_event: Channel<VoiceInputEvent>,
) -> Result<VoiceInputState, String> {
    ensure_microphone_permission()?;
    state.ensure_asr_provider_ready(&app)?;

    let settings = state.settings_snapshot();
    let options = voice_start_options_from_settings(&settings);
    let current = state.voice_input.state();
    let options = VoiceInputStartOptions {
        landing_mode: current.landing_mode,
        input_mode: current.input_mode,
        auto_refine: current.auto_refine,
        translate_enabled: current.translate_enabled,
        translate_target_lang: Lang::from(current.translate_target_lang.as_str()),
        refine_mode,
        glossary: load_glossary_entries(&state),
        save_quick_note: Some(voice_quick_note_saver(app.clone(), &state)),
        // 原始录音（PRD-02-030）：设置开着就落到应用缓存目录，落成速记时再搬进笔记附件目录。
        audio_capture_dir: settings
            .get_bool("voice_input.keep_audio")
            .unwrap_or(true)
            .then(|| {
                app.path()
                    .app_cache_dir()
                    .ok()
                    .map(|dir| dir.join("voice-audio"))
            })
            .flatten(),
        vad: state.vad_factory(&app),
        client_session_id,
        // 流式实时识别（PRD-02-004）：与录音同构，具体句柄供 register_stream。
        asr_provider: state.asr_provider_handle(),
        ..options
    };
    let started =
        state
            .voice_input
            .start(state.queue_handle(), state.mic_handle(), options, on_event);
    // 会话开起来了：语音速记与「开始录音」两个入口此刻都不可用了
    // （TDD-07 §2.2 的「既有会话 / 音频占用」那一档）。
    super::create_actions::publish_create_action_states(&app);
    started
}

#[tauri::command]
#[specta::specta]
pub(crate) fn popup_voice_input_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    on_event: Channel<VoiceInputEvent>,
) -> Result<VoiceInputState, String> {
    ensure_microphone_permission()?;
    state.ensure_asr_provider_ready(&app)?;

    let settings = state.settings_snapshot();
    let mut options = popup_voice_start_options_from_settings(&settings, Arc::clone(&state.popup));
    options.glossary = load_glossary_entries(&state);
    options.asr_provider = state.asr_provider_handle();
    let started =
        state
            .voice_input
            .start(state.queue_handle(), state.mic_handle(), options, on_event);
    // 弹窗翻译里的语音同样占同一支麦克风（`MicHolder` 不同、仲裁器只有一个）。
    super::create_actions::publish_create_action_states(&app);
    started
}

/// 脚本大纲「口述一段」（ADR-162 ⑮，PRD-19-013）：与普通语音输入同一采集与识别主干、同一麦克风
/// 持有者，落点是 [`VoiceInputSink::ScriptDraft`]——原话经 `FieldInput` 交回发起的界面，由它插进
/// 脚本光标处。停止 / 取消沿用 `voice_input_stop` / `voice_input_cancel`。
///
/// # Errors
/// 麦克风权限缺失、转写不可用或已有语音会话时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn script_voice_input_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    on_event: Channel<VoiceInputEvent>,
) -> Result<VoiceInputState, String> {
    ensure_microphone_permission()?;
    state.ensure_asr_provider_ready(&app)?;

    let settings = state.settings_snapshot();
    let mut options = field_voice_start_options_from_settings(
        &settings,
        VoiceInputSink::ScriptDraft,
        MicHolder::VoiceInput,
    );
    options.glossary = load_glossary_entries(&state);
    options.asr_provider = state.asr_provider_handle();
    options.vad = state.vad_factory(&app);
    let started =
        state
            .voice_input
            .start(state.queue_handle(), state.mic_handle(), options, on_event);
    super::create_actions::publish_create_action_states(&app);
    started
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_stop(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    client_session_id: Option<String>,
) -> Result<(), String> {
    let stopped = state.voice_input.stop(client_session_id);
    // 停止采集时会话**还没结束**（要出终稿、等用户落地），但麦克风已经放开，而
    // 「开始录音」只看麦克风占用——不发的话，用户明明可以接着录音了，那个入口还灰着。
    super::create_actions::publish_create_action_states(&app);
    stopped
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_cancel(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    client_session_id: Option<String>,
) -> Result<(), String> {
    // 取消 = 会话结束：清掉本次预设（TDD-07 §2.3）。下一次唤起重新读持久默认。
    state.voice_input.clear_session_preset();
    let cancelled = state.voice_input.cancel(client_session_id);
    super::create_actions::publish_create_action_states(&app);
    cancelled
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_input_land(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    landing_mode: LandingMode,
    edited_text: Option<String>,
) -> Result<VoiceInputState, String> {
    let landed = state
        .voice_input
        .land_pending(
            landing_mode,
            Some(voice_quick_note_saver(app.clone(), &state)),
            edited_text,
        )
        .await;
    // 落地 = 会话真的走完了（成功或失败都不再占用）：创建入口的可用性随之回到常态。
    super::create_actions::publish_create_action_states(&app);
    // **只有成功落地才清预设**：失败要保留，用户点重试时落点不该从速记变回注入光标
    // （TDD-07 §2.3）。ASR 出终稿、停止采集、重说同样不清——那时会话还没结束。
    if landed.is_ok() {
        state.voice_input.clear_session_preset();
        // 首个 AI 任务的「成功时刻」（PRD-14-050）：一次真实落地即算。
        super::onboarding::mark_first_task_done(&app, &state);
    }
    landed
}

/// 唤起 HUD 时这一次的落点预设（TDD-07 §2.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceInputHudPresetDto {
    /// 普通入口：用用户此刻的持久化默认落点。
    Current,
    /// 「语音速记」入口：本次默认存为速记。
    QuickNote,
}

/// HUD 已被唤起（`voice-input://hud-opened`）。
///
/// **预热窗口需要一个"该重读状态了"的通知**：它常驻但隐藏，挂载发生在很久以前，而
/// 预设是在 `show` **之前**提交的——没有这条低频事件，HUD 显示出来时渲染的还是上一次的
/// 落点。录音进度仍走原来的 Channel，不在这里另建高频事件流。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "voice-input://hud-opened")]
pub struct VoiceInputHudOpened;

/// 显示语音输入 HUD 非激活面板（不触发热键桥、不开始采音）。
///
/// 应用内入口（「添加内容 / 快速开始」的语音速记、仪表盘快速操作）与全局热键共用同一
/// 预热窗口；用户在 HUD 内自行选择按住/切换模式开始说话。
///
/// **`preset` 必填**：不给缺省值，也不留旧签名——普通入口显式传 `current`，语音速记
/// 传 `quick_note`。可选参数在这里的代价是"忘了传"与"就是要 current"从调用点看长得一样。
///
/// 已有会话在进行时**照样把 HUD 显示出来**（用户要的是回到那个会话），但不改它的任何
/// 设定，并如实返回 `VOICE_SESSION_BUSY`。
#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_show_hud(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    preset: VoiceInputHudPresetDto,
) -> Result<crate::voice_input::VoiceInputState, String> {
    show_voice_hud_with_preset(&app, &state, preset)
}

/// 唤起 HUD 的**唯一实现**：先提交预设，再 show，最后通知 HUD 重读状态。
///
/// 顺序不可换：`show` 之后再改预设，HUD 已经按旧值画完了第一帧。
pub(crate) fn show_voice_hud_with_preset(
    app: &tauri::AppHandle,
    state: &AppState,
    preset: VoiceInputHudPresetDto,
) -> Result<crate::voice_input::VoiceInputState, String> {
    use crate::voice_input::HudPreset;
    let persisted = crate::voice_input::LandingMode::from_setting(
        state
            .settings_snapshot()
            .get("voice_input.landing_mode")
            .and_then(|v| v.as_str().map(str::to_owned)),
    );
    let prepared = state.voice_input.prepare_session(
        match preset {
            VoiceInputHudPresetDto::Current => HudPreset::Current,
            VoiceInputHudPresetDto::QuickNote => HudPreset::QuickNote,
        },
        persisted,
    );
    let shown = crate::show_voice_input_hud_panel(app).is_some();
    if shown {
        // 低频通知：常驻隐藏的预热窗口据此重读 `voice_input_state`。
        let _ = <VoiceInputHudOpened as tauri_specta::Event>::emit(&VoiceInputHudOpened, app);
    }
    match prepared {
        // 会话忙：HUD 已经显示出来了（上一行），但如实报忙——调用方据此不再假装开了新会话。
        Err(busy) => Err(busy),
        Ok(state) if shown => Ok(state),
        Ok(_) => Err("voice-input-hud 窗口未预热".to_string()),
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_close(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // 明确丢弃并关闭 = 会话结束（TDD-07 §2.3）：窗口关闭走的就是这条唯一的 cancel 路径，
    // 不存在"只把 WebView 藏起来、业务会话还留着"的第二条路。
    state.voice_input.clear_session_preset();
    let cancelled = state.voice_input.cancel_holder(MicHolder::VoiceInput);
    // 还没落地的结果随窗口一起丢弃——HUD 关闭前已就此确认过（`result_pending`）。
    state.voice_input.discard_pending_output();
    // 会话结束：先发快照再处理窗口——两件事互不依赖，而提早返回的 `?` 会跳过它。
    super::create_actions::publish_create_action_states(&app);
    cancelled?;
    if let Some(win) = app.get_webview_window("voice-input-hud") {
        // 固定模式下收起前刷新记忆坐标：用户显示期间拖到的新位置即新的固定点。
        persist_voice_hud_position_if_pinned(&app, &state, &win);
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 固定/取消固定语音输入 HUD 位置（PRD-02-019）。
///
/// 固定：记忆当前窗口左上角（物理像素），此后显示用记忆坐标、不再跟随光标；
/// 取消：恢复光标跟随（记忆坐标保留，便于再次固定时回到原位附近仍以当下位置覆盖）。
#[tauri::command]
#[specta::specta]
pub(crate) fn voice_input_set_hud_pinned(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    pinned: bool,
) -> Result<(), String> {
    if pinned {
        let win = app
            .get_webview_window("voice-input-hud")
            .ok_or_else(|| "voice-input-hud 窗口未预热".to_string())?;
        let pos = win.outer_position().map_err(|e| e.to_string())?;
        write_voice_hud_pin_position(&state, pos.x, pos.y)?;
    }
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "voice_input.hud_pinned",
        SettingValueDto::Bool(pinned),
    )?;
    let _ = SettingsChanged {
        key: "voice_input.hud_pinned".into(),
    }
    .emit(&app);
    Ok(())
}

/// 固定模式下把窗口当前位置写回记忆坐标（收起前调用；未固定为 no-op）。
fn persist_voice_hud_position_if_pinned(
    app: &tauri::AppHandle,
    state: &AppState,
    win: &tauri::WebviewWindow,
) {
    let pinned = state
        .settings_snapshot()
        .get_bool("voice_input.hud_pinned")
        .unwrap_or(false);
    if !pinned {
        return;
    }
    if let Ok(pos) = win.outer_position() {
        if write_voice_hud_pin_position(state, pos.x, pos.y).is_ok() {
            let _ = SettingsChanged {
                key: "voice_input.hud_pinned".into(),
            }
            .emit(app);
        }
    }
}

fn write_voice_hud_pin_position(state: &AppState, x: i32, y: i32) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "voice_input.hud_pin_x",
        SettingValueDto::Int(x),
    )?;
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "voice_input.hud_pin_y",
        SettingValueDto::Int(y),
    )
}

#[cfg(test)]
mod quick_note_saver_lock_tests {
    /// 派生更新必须在库锁之外（真机 2026-10-02 应用卡死的回归锁）。
    ///
    /// `refresh_derived_after_write` 的第一步会再锁一次库锁；std Mutex 不可重入，锁内调用
    /// 就是同线程死锁，而攥着库锁的线程永远不放，之后所有读库命令都排在后面。这里读源码：
    /// 库锁只能出现在 `let doc = { … };` 那个块里，派生更新必须出现在块结束之后。
    #[test]
    fn derived_refresh_runs_after_the_vault_lock_is_released() {
        let code = crate::ipc::source_without_tests(include_str!("voice.rs"));
        let saver = &code[code
            .find("fn voice_quick_note_saver(")
            .expect("找不到 voice_quick_note_saver")..];
        let saver = &saver[..saver.find("\nfn ").unwrap_or(saver.len())];
        let lock = saver.find("vault_arc.lock()").expect("saver 里应有库锁");
        let block_start = saver
            .find("let doc = {")
            .expect("库锁必须罩在 `let doc = {` 块里");
        let block_end = saver[block_start..]
            .find("\n        };")
            .map(|i| block_start + i)
            .expect("找不到库锁块的结尾");
        let refresh = saver
            .find("refresh_derived_after_write(")
            .expect("saver 里应有派生更新");
        assert!(block_start < lock && lock < block_end, "库锁必须在块内");
        assert!(refresh > block_end, "派生更新必须在库锁块结束之后");
    }
}

#[cfg(test)]
mod hud_entry_tests {
    /// **所有唤起路径都必须经会话准备函数**（TDD-07 §2.3）。
    ///
    /// 读源码：这几条路径分别在托盘回调、热键回调与双击手势里，各自要真 `AppHandle` 与
    /// 真窗口。而这里要拦的恰好是源码层面的绕过——谁直接调 `show_voice_input_hud_panel`，
    /// "这一次该落到哪儿"就从来没被回答过，HUD 显示的是上一次会话遗留的运行态。
    #[test]
    fn no_entry_bypasses_the_session_preparation() {
        let lib = crate::ipc::source_without_tests(include_str!("../lib.rs"));
        let code: String = lib
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        // `lib.rs` 里这个名字**只该出现一次——它自己的定义**：唯一的调用点在
        // `show_voice_hud_with_preset`（本文件）。多出来的那一次就是一条绕过会话准备的路径。
        assert_eq!(
            code.matches("show_voice_input_hud_panel(").count(),
            1,
            "lib.rs 里出现了绕过 `show_voice_hud_from_rust` 的面板唤起"
        );
        // 三条 Rust 侧路径都走统一入口。
        assert!(
            code.contains("show_voice_hud_from_rust(app, ipc::VoiceInputHudPresetDto::Current)")
        );
        assert!(
            code.matches("show_voice_hud_from_rust(").count() >= 3,
            "托盘 / 热键 / 双击三条路径应各有一次"
        );
    }

    /// 预设必须在 `show` **之前**提交，否则 HUD 已经按旧值画完了第一帧。
    #[test]
    fn the_preset_is_committed_before_the_window_is_shown() {
        let body_src = crate::ipc::source_without_tests(include_str!("voice.rs"));
        let body = body_src
            .split("pub(crate) fn show_voice_hud_with_preset(")
            .nth(1)
            .expect("找不到统一入口")
            .split("\n/// ")
            .next()
            .expect("入口边界变了");
        let prepare = body.find("prepare_session(").expect("没有准备会话");
        let show = body
            .find("show_voice_input_hud_panel(")
            .expect("没有显示窗口");
        assert!(prepare < show, "预设必须先于 show 提交");
        // 显示之后要通知常驻预热窗口重读状态——没有它，HUD 渲染的还是上一次的落点。
        let opened = body.find("VoiceInputHudOpened").expect("没有发出唤起通知");
        assert!(show < opened, "通知应在 show 之后");
    }
}
