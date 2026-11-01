//! 实时字幕命令域（PRD-04-040/045，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：**无录音形态的实时字幕**——复用录音同一条采集→ASR→转译引擎，但
//! `SessionPurpose::Caption`（不落盘、不归档）；面板显隐、译文镜像窗、字幕存为笔记。
//! 停止/暂停/转写转译开关等控制命令与录音共用（形态无关，见 [`super::recording`]）。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 启动**实时字幕**会话（PRD-04-040）：复用录音同一条采集→ASR→转译引擎，但**不落盘、不归档**
/// （`SessionPurpose::Caption`，`audio_dir=None`，finalize 空操作）。停止/暂停/书签/转写转译开关/
/// 目标语现场切换共用 `recording_*` 控制命令（形态无关，按 session_id 作用于活动会话）。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    on_event: tauri::ipc::Channel<RecordingEvent>,
) -> Result<RecordingState, String> {
    let settings = state.settings_snapshot();
    // 实时字幕与录音是两个任务：字幕默认同时听麦克风 + 系统声音，且持有独立设置，
    // 避免用户为了录视频/会议改过录音来源后，下一次字幕悄然漏掉另一侧声音。
    let capture_source = crate::recording::RecordingCaptureSource::from_setting(
        settings.get_str("caption.capture_source").as_deref(),
        crate::recording::RecordingCaptureSource::Both,
    );
    if capture_source.uses_microphone() {
        ensure_microphone_permission()?;
    }
    state.ensure_asr_provider_ready(&app)?;
    let lang_hint = settings
        .get_str("realtime.transcribe_lang")
        .filter(|s| !s.trim().is_empty() && s != "auto")
        .map(|s| luna_core::Lang::from(s.as_str()));
    let input_device = settings
        .get_str("recording.input_device")
        .filter(|s| !s.trim().is_empty());
    // 系统声音用于视频/会议听译；Both 失败时既有编排会诚实降级为麦克风继续。
    // 字幕是会话不变量，始终转写；翻译偏好由 caption 设置单点拥有，HUD 只回显、不复制状态。
    // 不读取录音的 realtime 开关，两个任务的偏好互不污染。
    let transcribe_enabled = true;
    let translate_enabled = settings
        .get_bool("caption.translation_enabled")
        .unwrap_or(true);
    // 转译不再传定向 LLM（ADR-083 ⑥）：档位由 `Job::service_class()` 推导，
    // 路由层按「泳道 → 模型槽位」总映射填 `preferred_provider`，调用点一律不声明。
    let glossary = load_glossary_entries(&state);
    // 术语表同时喂给 ASR（`initial_prompt` + canonical 兜底）与转译（命中项作强制译法），
    // 一份词典四路注入（PRD-01-017，合成逻辑单一来源 `luna_core::glossary`）。
    let translate_config = realtime_translate_config(
        &settings,
        Arc::clone(&state.detector),
        Arc::from(glossary.clone()),
    );

    let options = RecordingStartOptions {
        session_id,
        purpose: crate::recording::SessionPurpose::Caption,
        note_rel: String::new(),                  // 字幕不归档，无笔记
        audio_dir: None,                          // 不落盘
        vault_root: None,                         // 不落盘，无需路径换算锚点
        format: luna_audio::RecordingFormat::Wav, // 字幕不落盘，仅为类型完备
        tuning: capture_tuning(&settings),
        vad: state.vad_factory(&app),
        lang_hint,
        input_device,
        capture_source,
        transcribe_enabled,
        translate_enabled,
        translate_config,
        glossary,
        asr_provider: state.asr_provider_handle(),
        // 字幕不归档，模型名无消费者——给一个空格子而不是省略字段。
        asr_model_name: Arc::new(std::sync::Mutex::new(None)),
        // 空操作 finalize：字幕形态收尾走 Caption 分支（不调用此闭包），此处仅为类型完备。
        finalize_fn: Box::new(|_, _, _| Ok(String::new())),
        // 字幕没有旁听者。
        partial_observer: None,
    };

    let started = state.recording.start(
        state.queue_handle(),
        state.mic_handle(),
        options,
        app.clone(),
        on_event,
    );
    // 字幕会话同样占麦克风（`capture_source` 含麦时）：创建入口的可用性随之变化
    // （TDD-07 §2.2 的「音频」那一档）。字幕的停止走 `recording_stop`，那边已发。
    super::create_actions::publish_create_action_states(&app);
    started
}
/// 显示实时字幕非激活面板（不开始采音；用户在面板内自行开始，PRD-04-040）。
/// 应用内入口（工作区语音与 AI 组 / 仪表盘）与全局热键共用同一常驻面板。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_show_hud(app: tauri::AppHandle) -> Result<(), String> {
    if crate::show_live_caption(&app) {
        Ok(())
    } else {
        Err("live-caption 窗口未预热".to_string())
    }
}

/// 关闭实时字幕面板（仅非活动态有效）：复位 idle + Rust 侧隐藏面板（同录音 HUD dismiss 语义）。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_dismiss(app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    state.recording.dismiss_idle();
    crate::hide_live_caption(&app);
    // 原文窗关闭时一并收起译文窗（避免残留孤窗）。
    crate::hide_live_caption_translation(&app);
}

/// 显示译文独立窗（PRD-04-044）：由原文窗在「译文独立窗」开启时唤起，纯窗口动作。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_translation_show(app: tauri::AppHandle) -> Result<(), String> {
    if crate::show_live_caption_translation(&app) {
        Ok(())
    } else {
        Err("live-caption-translation 窗口未预热".to_string())
    }
}

/// 隐藏译文独立窗（关闭独立窗模式 / 关闭字幕时）。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_translation_dismiss(app: tauri::AppHandle) {
    crate::hide_live_caption_translation(&app);
}

/// 某扇字幕窗被收起（PRD-04-040 / 044）。字幕窗常驻，收起只是 hide，网页里的状态原样
/// 留着——收起时开着的「外观与显示」面板，下一次打开还开着，看上去像「打开字幕默认就弹出
/// 设置」（真机 2026-10-02，只在上次关时面板开着才出现，所以「不是每次」）。译文窗还会被
/// 原文窗收起（选回同窗双语、会话结束），它自己无从得知，只能由 Rust 在收起处说一声。
/// 全局广播、载荷带 label，两扇窗各认自己的。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "caption://window-hidden")]
pub struct CaptionWindowHidden {
    /// 被收起的窗口 label（`live-caption` / `live-caption-translation`）。
    pub label: String,
}

/// 字幕窗记下的几何（物理像素，左上角 + 内容区尺寸；PRD-04-040「记住上一次」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptionWindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 每扇记位置的常驻面板的四个内部键（x, y, 宽, 高）：两扇字幕窗（前缀 `caption.window_`）与
/// 提词器（`teleprompter.window_`，ADR-162 ⑤），都归 `is_internal`。
fn caption_window_keys(label: &str) -> Option<[&'static str; 4]> {
    match label {
        "live-caption" => Some([
            "caption.window_source_x",
            "caption.window_source_y",
            "caption.window_source_width",
            "caption.window_source_height",
        ]),
        "live-caption-translation" => Some([
            "caption.window_translation_x",
            "caption.window_translation_y",
            "caption.window_translation_width",
            "caption.window_translation_height",
        ]),
        TELEPROMPTER_LABEL => Some([
            "teleprompter.window_x",
            "teleprompter.window_y",
            "teleprompter.window_width",
            "teleprompter.window_height",
        ]),
        _ => None,
    }
}

/// 读这扇窗记下的几何；宽或高为 0（默认值）= 还没记过。
pub(crate) fn remembered_caption_window_geometry(
    settings: &Settings,
    label: &str,
) -> Option<CaptionWindowGeometry> {
    let [x, y, width, height] = caption_window_keys(label)?;
    let width = u32::try_from(settings.get_int(width)?).ok()?;
    let height = u32::try_from(settings.get_int(height)?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(CaptionWindowGeometry {
        x: i32::try_from(settings.get_int(x)?).ok()?,
        y: i32::try_from(settings.get_int(y)?).ok()?,
        width,
        height,
    })
}

/// 记下这扇窗此刻的几何（收起前 / 应用退出时调用）。
pub(crate) fn remember_caption_window_geometry(
    state: &AppState,
    label: &str,
    geometry: CaptionWindowGeometry,
) -> Result<(), String> {
    let Some(keys) = caption_window_keys(label) else {
        return Ok(());
    };
    let values = [
        geometry.x,
        geometry.y,
        i32::try_from(geometry.width).unwrap_or(i32::MAX),
        i32::try_from(geometry.height).unwrap_or(i32::MAX),
    ];
    for (key, value) in keys.into_iter().zip(values) {
        set_setting_value(
            state.copy(),
            &state.settings,
            &state.settings_persistence,
            key,
            SettingValueDto::Int(value),
        )?;
    }
    Ok(())
}

/// 一条待保存的字幕（原文 + 可选译文），由前端按句配对后传入。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct CaptionLineDto {
    /// 原文（可能为空——仅译文行）。
    pub source: String,
    /// 译文（无则 `None`）。
    pub translation: Option<String>,
}

/// **把实时字幕内容保存为 Vault 笔记**（PRD-04-045）。
///
/// 实时字幕本不落盘（`SessionPurpose::Caption`，PRD-04-040）；本命令是用户显式动作——把已积累的
/// 字幕（原文 + 双语译文）组装为 Markdown 正文写入 Vault 普通笔记（`NoteType::Note`，落 `vault.notes_dir`），
/// 标题带时间戳、正文按句「原文 + `> 译文`」。前端只转发内容，落盘/命名/frontmatter 全在 Rust（ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) fn caption_save_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    lines: Vec<CaptionLineDto>,
) -> Result<String, String> {
    let has_content = lines.iter().any(|l| {
        !l.source.trim().is_empty() || l.translation.as_ref().is_some_and(|t| !t.trim().is_empty())
    });
    if !has_content {
        return Err(coded_error(&app, "CAPTION_EMPTY", "caption-empty"));
    }

    let lang = backend_copy_arc(&app)
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string());
    let is_zh = lang.starts_with("zh");
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let title = if is_zh {
        format!("实时字幕 {stamp}")
    } else {
        format!("Live captions {stamp}")
    };

    let mut body = format!("# {title}\n\n");
    for line in &lines {
        let src = line.source.trim();
        let tr = line
            .translation
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if src.is_empty() && tr.is_none() {
            continue;
        }
        if !src.is_empty() {
            body.push_str(src);
            body.push('\n');
        }
        if let Some(tr) = tr {
            // 译文用引用块，便于笔记里区分双语。
            body.push_str("> ");
            body.push_str(tr);
            body.push('\n');
        }
        body.push('\n');
    }

    let settings = state.settings_snapshot();
    let dir = note_type_dir(&settings, NoteType::Note);
    let rel_path = format!(
        "{dir}/{}",
        draft_note_rel_path(&body, NoteType::Note, &lang)?
    );
    let doc = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        create_unique_note(state.copy(), vault, &rel_path, NoteType::Note, body)?
    };
    emit_vault_changed(&app, &doc.path, false);
    Ok(doc.path)
}

#[cfg(test)]
mod window_geometry_tests {
    use super::caption_window_keys;

    /// 键名拼错时写入会被注册表拒掉，而调用点只打一行日志——用户看到的只是「怎么又没记住」。
    /// 两扇窗的八个键必须都在注册表里、都是内部键（不进设置页）。
    #[test]
    fn every_caption_window_key_is_a_registered_internal_setting() {
        for label in ["live-caption", "live-caption-translation"] {
            let keys = caption_window_keys(label).expect("两扇字幕窗都有一组键");
            for key in keys {
                assert!(luna_config::registry::spec(key).is_some(), "{key} 未注册");
                assert!(
                    luna_config::registry::is_internal(key),
                    "{key} 不该出现在设置页"
                );
            }
        }
        assert!(caption_window_keys("voice-input-hud").is_none());
    }
}
