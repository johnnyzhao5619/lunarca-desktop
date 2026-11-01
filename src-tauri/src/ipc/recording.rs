//! 录音会话命令域（IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：**录音会话本身**——启停/暂停/书签/静音、会话内转写转译开关与语种热切、
//! 输入设备枚举、录音 HUD 面板显隐，以及停录后的权威重转写入队。
//! 事实源：录音状态机 [`crate::recording`]；采集管线 [`luna_audio`]（TDD-02）。
//!
//! 同域姊妹模块：实时字幕 [`super::caption`]、同步回放/导出 [`super::playback`]、
//! 导入与队列命令 [`super::transcribe`]、任务编排 [`super::processing`]。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`（供 `mod.rs` 的 `collect_commands!` 引用）；
//! 复用的共享 helper（AppState、`create_unique_note` 等）经 `use super::*` 从 `mod.rs` 取。

use super::*;

// ─── 录音状态跨窗广播（B2 / H2）────────────────────────────────────────────────

/// 录音控制器状态**转移**事件（`recording://state-changed`，全窗广播，载荷是权威快照）。
///
/// 会话的 `on_event` Channel 只通向发起会话的那一扇窗；这条事件让**任何**表面（主窗状态栏、
/// 录音 HUD、字幕面板）都能以控制器为准——它只在 `status` / `session_id` 转移时发，计时
/// 推进仍走会话 Channel 的节流 `State`（见 [`crate::recording::RecordingController`] 字段文档）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "recording://state-changed")]
pub struct RecordingStateChanged(pub RecordingState);

/// 把控制器此刻的权威快照广播一次（面板显示时调用：H2 的「挂载时拉一次真值」——录音 HUD
/// 是常驻 webview，React 只挂载一次，「每次显示」才是它的挂载时刻）。
pub(crate) fn broadcast_recording_state(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let _ = RecordingStateChanged(state.recording.state()).emit(app);
}

/// App 装配时把控制器的状态转移接到全窗事件上（`app.manage(state)` 之后调用一次）。
pub(crate) fn install_recording_state_broadcast(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let handle = app.clone();
    state
        .recording
        .set_state_observer(Box::new(move |snapshot| {
            let _ = RecordingStateChanged(snapshot.clone()).emit(&handle);
            // 会话收场 → 补发录音期间扣下的通知（PRD-15-032）。另起线程：补发会读录音状态
            // 与发 IPC，不让它挂在录音任务的状态转移上。
            if !matches!(
                snapshot.status,
                crate::recording::RecordingStatus::Recording
                    | crate::recording::RecordingStatus::Paused
                    | crate::recording::RecordingStatus::Stopping
            ) {
                let app = handle.clone();
                std::thread::spawn(move || {
                    // 跟读会话收场：清掉跟读状态，攒下的录制标记写回脚本（ADR-162 ⑤ C4c）。
                    super::teleprompter::end_follow(&app);
                    // 会话真正收场、麦克风此刻才放开：`recording_stop` 返回时采集可能还在收尾，
                    // 那一刻发的快照仍是「被占用」（录制页签的开录前检查实测停在这一档）。
                    super::create_actions::publish_create_action_states(&app);
                    if let Some(center) = app.try_state::<NotificationCenter>() {
                        center.flush_held(&app);
                    }
                    // 录音中推迟的换账户切库：录音停了，看看能不能接着做（还要等处理收尾）。
                    crate::vault_lists::retry_pending(&app);
                });
            }
        }));
}

// ─── 录音命令 ──────────────────────────────────────────────────────────────────

/// 返回录音控制器当前状态（脱敏）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_state(state: tauri::State<'_, AppState>) -> RecordingState {
    state.recording.state()
}

/// 订阅录音波形电平二进制通道（ADR-011，每帧 [peak, rms] 字节）。
///
/// 按调用方**窗口 label** 登记订阅槽（多常驻录音表面并存互不覆盖；同窗重挂载替换）。
#[tauri::command]
#[specta::specta]
pub(crate) fn subscribe_recording_levels(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    channel: tauri::ipc::Channel<RawBytes>,
) -> Result<(), String> {
    state
        .recording
        .subscribe_levels(window.label().to_string(), channel)
}

/// 启动录音会话：占用麦克风、派发异步录音任务、on_event 通道接收 RecordingEvent。
///
/// `note_rel` 为预先创建好的归档笔记相对路径（由 `create_note` 生成后传入）。
/// 停止后自动调 `luna_vault::finalize_recording` 归档（TDD-06 §2.1/§2.2）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    note_rel: String,
    on_event: tauri::ipc::Channel<RecordingEvent>,
) -> Result<RecordingState, String> {
    // 录音格式 + 转写语种 + 设备 + 转写开关（设置驱动，去硬编码/去死键）：
    // 当前仅 WAV 实现；语种 "auto" = 自动检测；设备空串 = 系统默认。
    let settings = state.settings_snapshot();
    let capture_source = crate::recording::RecordingCaptureSource::from_setting(
        settings.get_str("recording.capture_source").as_deref(),
        crate::recording::RecordingCaptureSource::Mic,
    );
    if capture_source.uses_microphone() {
        ensure_microphone_permission()?;
    }
    state.ensure_asr_provider_ready(&app)?;

    // 音频落盘目录 = vault_root/<录音笔记附件目录>（录音音频作为笔记附件，遵循附件位置策略，
    // 与粘贴/图片附件同源；不再散落在录音归档根目录下，PRD-04-030 / PRD-06-012 收口）。
    let vault_root = state.vault_root();
    let asset_dir = media_asset_dir(&settings, &note_rel);
    let audio_dir = vault_root.as_ref().map(|root| root.join(&asset_dir));
    // 落盘容器恒 WAV：FLAC 增量编码器待 spike（TDD-02 §3.5），此前那个「录音格式」设置只有
    // 一个合法取值、读取函数还直接忽略取值 → 已从注册表下线，不再假装可选。
    let format = luna_audio::RecordingFormat::Wav;
    // 采集调参（PRD-04 §5 / PRD-09-040）：波形帧率、磁盘预警阈值、滚动切分上限。
    // 三项在注册表登记已久却从未接线（writer/pipeline 只用编译期默认），此处收口。
    let tuning = capture_tuning(&settings);
    let lang_str = settings
        .get_str("realtime.transcribe_lang")
        .filter(|s| !s.trim().is_empty() && s != "auto");
    let lang_hint = lang_str.as_deref().map(luna_core::Lang::from);
    let input_device = settings
        .get_str("recording.input_device")
        .filter(|s| !s.trim().is_empty());
    // 采集源（PRD-04-052）：麦克风 / 系统音频（会议对端声音）。
    let transcribe_enabled = settings
        .get_bool("realtime.transcribe_enabled")
        .unwrap_or(true);
    // 实时转译（PRD-04-011，TDD-08）：总开关 + 智能双向配置（默认关；语言决策与弹窗翻译同源，
    // 按句检测源语言解析有效目标，从根上规避「中文语音开转译却仍输出中文」的同语改写）。
    let translate_enabled = settings
        .get_bool("realtime.translate_enabled")
        .unwrap_or(false);
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
    // 媒体元数据（MediaRef.model/lang，PRD-04-021/030：对照、二次转写与导出提示）。
    // 起始值 = 本机权重名（服务端档下为 `None`——那时我们还不知道对方会用哪个模型）；
    // 会话中收到 `executed_by` 后由录音循环覆写成如实的那个。
    let asr_model_name = Arc::new(std::sync::Mutex::new(state.asr_model_display_name()));
    let asr_model_for_finalize = Arc::clone(&asr_model_name);

    let vault_arc = state.vault_arc();
    // `'static` 闭包在归档那一刻读切库过渡态真值（边界⑤：措辞按此刻真实原因）。
    let switching = state.vault_switching_flag();
    // 文案来源随闭包一起走（`Arc`，见 `AppState::backend_copy_arc`）：归档那一刻要说的
    // 「正在切换 / 尚未选择」是给人读的，不能只发一个码。
    let copy_for_finalize = state.backend_copy_arc();
    let note_rel_clone = note_rel.clone();
    // 停录后台权威重转写（PRD-04-030）：即时归档用实时预览段，随后对已落盘音频以批量档模型
    // 重转写并覆盖为权威结果（+双语），故 finalize 闭包捕获 app 句柄以派发后台任务。
    let app_for_retr = app.clone();
    let finalize_fn: crate::recording::FinalizeFn =
        Box::new(move |mut parts, segments, bookmarks| {
            // 会话级模型/语种元数据回填到各分片（单点：finalize 闭包持有设置快照）。
            // 读**此刻**的答案：会话中途降级换过腿时，这里是最后一个如实的模型名。
            let model = asr_model_for_finalize
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            for part in &mut parts {
                part.model = model.clone();
                part.lang = lang_str.clone();
            }
            {
                let guard = vault_arc.lock().map_err(|_| "Vault 锁中毒".to_string())?;
                let vault = guard.as_ref().ok_or_else(|| {
                    crate::ipc::vault_absent_message(
                        copy_for_finalize.as_deref(),
                        switching.load(std::sync::atomic::Ordering::Acquire),
                    )
                })?;
                // 控制器收集的是 `RecordingBookmark`（会话内形态），落盘用 Vault 的 `Bookmark`。
                let bookmarks: Vec<luna_vault::Bookmark> = bookmarks
                    .into_iter()
                    .map(|b| luna_vault::Bookmark {
                        ms: b.abs_ms,
                        note: b.note,
                    })
                    .collect();
                vault
                    .finalize_recording(
                        &note_rel_clone,
                        &parts,
                        &segments,
                        &bookmarks,
                        // 录音时的实时预览不是权威转写；停录后重转写写回才是（PRD-16-047）。
                        luna_vault::TranscriptAuthority::Preview,
                    )
                    .map_err(|e| e.to_string())?;
            }
            // 归档完成 → 触发后台权威重转写（不阻塞归档返回；无批量档模型/无音频则静默跳过）。
            spawn_post_stop_retranscribe(app_for_retr.clone(), note_rel_clone.clone());
            Ok(note_rel_clone.clone())
        });

    let options = RecordingStartOptions {
        session_id,
        purpose: crate::recording::SessionPurpose::Record,
        note_rel,
        audio_dir,
        // 绝对路径→Vault 相对换算的锚点（音频目录不再是 vault_root 的直接子目录，
        // 不能再由 audio_dir.parent() 反推，须显式传入真实 vault root）。
        vault_root,
        format,
        tuning,
        vad: state.vad_factory(&app),
        lang_hint,
        input_device,
        capture_source,
        transcribe_enabled,
        translate_enabled,
        translate_config,
        glossary,
        // 流式实时转录所需的 ASR Provider 具体句柄；None = 降级纯录音（无实时转写）。
        asr_provider: state.asr_provider_handle(),
        asr_model_name,
        finalize_fn,
        // 录音不需要旁听者。
        partial_observer: None,
    };

    let started = state.recording.start(
        state.queue_handle(),
        state.mic_handle(),
        options,
        app.clone(),
        on_event,
    );
    // 麦克风此刻已被这场会话占住：语音速记与「开始录音」两个创建入口的可用性都变了
    // （TDD-07 §2.2 的「音频」那一档）。不发的话，已经打开的「添加内容」还画着可用。
    super::create_actions::publish_create_action_states(&app);
    started
}
/// 停止录音会话（收尾转写 + 归档）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_stop(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<RecordingState, String> {
    let stopped = state.recording.stop(session_id);
    // 麦克风放开了：不发的话，界面会一直停在 `audio_busy` 上。
    super::create_actions::publish_create_action_states(&app);
    stopped
}

/// 暂停录音（释放麦克风，等待继续或停止）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_pause(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<RecordingState, String> {
    state.recording.pause(session_id)
}

/// 继续录音（重新开始一个新 part 的采集）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_resume(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<RecordingState, String> {
    state.recording.resume(session_id)
}

/// 在当前位置插入书签（abs_ms 由前端据 recording_state.elapsed_ms 传入），可带短备注。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_bookmark(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    abs_ms: f64, // TS number → Rust u64；f64 可无损表示 2^53 ms（约 28.5 万年）。
    note: Option<String>,
) -> Result<(), String> {
    state.recording.bookmark(session_id, abs_ms as u64, note)
}

/// 给刚打的书签补 / 改备注（PRD-04-005：打点即时，备注随后输入，两步都在录音中完成）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_bookmark_note(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    abs_ms: f64,
    note: String,
) -> Result<(), String> {
    state
        .recording
        .bookmark_note(session_id, abs_ms as u64, note)
}

/// 切换麦克风静音（PRD-04-003）：只作用于麦克风轨；系统音频持续采集。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_set_microphone_muted(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    muted: bool,
) -> Result<(), String> {
    state.recording.set_microphone_muted(session_id, muted)
}

/// 切换实时转写喂窗闸门（PRD-04-012）：关 = 停喂 ASR 窗，录音/落盘不受影响。
///
/// 仅作用于当前会话；持久默认值由前端另行写 `realtime.transcribe_enabled` 设置。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_set_transcribe(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    enabled: bool,
) -> Result<(), String> {
    state.recording.set_transcribe(session_id, enabled)
}

/// 切换实时转译喂句闸门（PRD-04-011/012，TDD-08）：关 = 停喂翻译管线，转写不受影响。
///
/// 仅作用于当前会话；持久默认值由前端另行写 `realtime.translate_enabled` 设置。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_set_translate(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    enabled: bool,
) -> Result<(), String> {
    state.recording.set_translate(session_id, enabled)
}

/// 现场切换实时转译目标外语（录音/字幕面板下拉，PRD-04-041）。
///
/// 写 `realtime.translate_target_lang` 持久默认（下次会话）+ 当前会话热更新翻译配置（下一句界即生效）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_set_translate_target(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    lang: String,
) -> Result<(), String> {
    state.recording.set_translate_target(session_id, lang)
}

/// 现场切换转写源语言（录音/字幕面板下拉，PRD-04-046，Chrome Live Caption 对标）。
///
/// `lang = "auto"` 或空 = 回自动检测（清锁重新检测）；否则显式指定（whisper 恒用该语言）。
/// 持久默认由前端写 `realtime.transcribe_lang` 设置（与目标外语下拉同一模式）；本命令
/// 只做当前会话热更新（当前解码窗之后即生效，无需重启会话）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_set_transcribe_lang(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
    lang: String,
) -> Result<(), String> {
    let configured = Some(lang.trim())
        .filter(|s| !s.is_empty() && *s != "auto")
        .map(str::to_owned);
    state.recording.set_transcribe_lang(session_id, configured)
}
/// 枚举可用音频输入设备（HUD 设备选择，PRD-04-003）。
///
/// cpal 枚举为阻塞调用 → `spawn_blocking`（ADR-010 同纪律：阻塞活不上 async executor）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_audio_input_devices() -> Vec<AudioInputDeviceDto> {
    tauri::async_runtime::spawn_blocking(|| {
        luna_audio::list_input_devices()
            .into_iter()
            .map(|d| AudioInputDeviceDto {
                name: d.name,
                is_default: d.is_default,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 音频输入设备 DTO（HUD 设备选择）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AudioInputDeviceDto {
    /// 设备名（即 `recording.input_device` 设置值）。
    pub name: String,
    /// 是否系统默认输入。
    pub is_default: bool,
}
/// 关闭录音 HUD 面板：隐藏窗口；非会话态顺带复位 idle（清错误残留）。
///
/// **关窗 ≠ 中断**（ADR-111 已知代价 ③）：录音进行中调用只藏窗——`dismiss_idle`
/// 对活动会话是 no-op，会话与状态原样保留，重开（`recording_show_hud`）即回到现场；
/// 录音中的可见性由状态栏持续态指示兜底（PRD-15-060）。
/// **隐藏走 Rust** 而非 JS：录音 HUD 是非激活 NSPanel，前端 `window.hide()` 受
/// Tauri v2 capability 约束（`core:window:default` 不含 `allow-hide`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_hud_dismiss(app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    state.recording.dismiss_idle();
    crate::hide_recording_hud(&app);
    // 录音开着「译文独立窗」时镜像窗随 HUD 收起（同 caption_dismiss 语义，避免孤窗）。
    crate::hide_recording_translation_mirror(&app);
}

/// 显示录音 HUD 非激活面板（不切换录音状态、不开始采音）。
///
/// 应用内入口（工作区录音页 / 仪表盘「开始录音」）与全局热键共用同一常驻面板；
/// 用户在 HUD 内自行点击开始（与 `voice_input_show_hud` 同语义，PRD-04-001）。
#[tauri::command]
#[specta::specta]
pub(crate) fn recording_show_hud(app: tauri::AppHandle) -> Result<(), String> {
    if crate::show_recording_hud(&app) {
        Ok(())
    } else {
        Err("recording-hud 窗口未预热".to_string())
    }
}
/// 停录后台权威重转写：入**统一处理队列**（`Retranscribe` 任务，单 worker 顺序处理并广播
/// 进度 + 完成通知，与手动二次转写完全同源 DRY，PRD-04-030/055）。无批量档模型 / 无音频 →
/// 静默跳过（音频已归档，可手动二次转写）。模型按需加载可能耗时，置于后台任务不阻塞停录收尾。
/// 停录后台重转写的翻译目标（PRD-04-030）：录音会话开着实时转译才补译，目标与实时同源。
/// `None` = 只转写不译（翻译在别处是显式动作，见 [`super::transcribe`] 的二次翻译入口）。
fn post_stop_translate_target(state: &AppState) -> Option<String> {
    let settings = state.settings_snapshot();
    if !settings
        .get_bool("realtime.translate_enabled")
        .unwrap_or(false)
    {
        return None;
    }
    Some(
        settings
            .get_str("realtime.translate_target_lang")
            .unwrap_or_else(|| "smart".into()),
    )
}

/// 已归档、还没决定要不要入队权威重转写的停录收尾数。它在录音变回空闲**之前**登记（归档回调
/// 里），入队与否落定才注销：推迟的换账户切库（[`crate::vault_lists`]）据此不在这道缝里切——
/// 录音一空闲就切，这一件会在换根之后才去找它的笔记。
static POST_STOP_PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 还有停录收尾没落定（见 [`POST_STOP_PENDING`]）。
pub(crate) fn post_stop_pending() -> bool {
    POST_STOP_PENDING.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// 一件停录收尾的登记：丢掉即注销，并让推迟的换账户切库再看一眼。
struct PostStopTicket(tauri::AppHandle);

impl PostStopTicket {
    fn take(app: &tauri::AppHandle) -> Self {
        POST_STOP_PENDING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(app.clone())
    }
}

impl Drop for PostStopTicket {
    fn drop(&mut self) {
        POST_STOP_PENDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        crate::vault_lists::retry_pending(&self.0);
    }
}

pub(crate) fn spawn_post_stop_retranscribe(app: tauri::AppHandle, note_rel: String) {
    let ticket = PostStopTicket::take(&app);
    tauri::async_runtime::spawn(async move {
        let _ticket = ticket;
        // State 借用限于块内，出块即释放，随后 emit/启动 worker 可再借 app。
        {
            let state = app.state::<AppState>();
            if state.ensure_batch_asr_provider(&app).is_err() {
                return; // 无批量档模型：静默跳过。
            }
            if state.processing.has_active(&note_rel, "retranscribe") {
                return; // 已在队列（防重）。
            }
            let has_audio = match state.vault.lock() {
                Ok(guard) => match guard.as_ref() {
                    Some(vault) => build_retranscribe_parts(vault, &note_rel)
                        .map(|parts| !parts.is_empty())
                        .unwrap_or(false),
                    None => false,
                },
                Err(_) => false,
            };
            if !has_audio {
                return; // 无音频媒体：跳过。
            }
            state.processing.enqueue(
                crate::processing_queue::ProcTaskKind::Retranscribe {
                    note_rel: note_rel.clone(),
                    // 录音时开着实时转译 = 用户要双语，停录后的权威重转写据此补译；
                    // 目标语言取录音会话同一口径（PRD-04-030/041）。
                    translate_to: post_stop_translate_target(&state),
                },
                note_task_label(&note_rel),
            );
        }
        emit_processing_queue(&app);
        start_processing_worker_if_idle(&app);
    });
}
