//! 弹窗翻译命令域（PRD-01，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：非激活弹窗面板的会话状态、预设、生成与替换回填、结果自动复制。
//! 面板窗口本身由 [`crate::popup`] 与平台层承载（ADR-015），本模块只做 IPC 面。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 列出可用于弹窗翻译的内置模板（紧凑预设选择器，PRD-01-015）。
///
/// 仅返回带"目标语言模式"的模板（即真正的弹窗翻译模板）。用户/Vault 级模板为后续增量。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_popup_presets() -> Vec<PresetInfo> {
    builtin_presets(luna_core::prompt::registry())
        .into_iter()
        .filter(|p| p.target_lang_mode.is_some())
        .map(|p| PresetInfo {
            id: p.id.as_str().to_string(),
            group: match p.target_lang_mode {
                Some(luna_core::TargetLangMode::Fixed(_)) => "target_lang".to_string(),
                _ => "style".to_string(),
            },
            name: p.name,
        })
        .collect()
}

/// 返回弹窗会话当前状态（弹窗窗口显示时主动拉取，避免依赖可能丢失的 `PopupSessionReady` 推送）。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_session_state(
    state: tauri::State<'_, AppState>,
) -> Result<PopupSessionState, String> {
    let session = state.popup.lock().map_err(|_| "会话锁中毒".to_string())?;
    let mode = match session.mode() {
        crate::popup::PopupMode::Display => "display",
        crate::popup::PopupMode::Input => "input",
    };
    let source_text = session.source_text();
    Ok(PopupSessionState {
        mode: mode.to_string(),
        has_source: source_text.is_some(),
        source_text,
        save_to_translate_space: session.save_to_translate_space(),
        draft: session.draft(),
        clipboard_not_restored: session.clipboard_not_restored(),
    })
}

/// 写入输入模式的未生成草稿（PRD-01-005 修订）：打字期间防抖写回、失焦与关窗前冲刷；
/// 关窗保留、重开预填。空串 = 没有草稿。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_set_draft(
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    state
        .popup
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .set_draft(Some(text));
    Ok(())
}

/// 复制译文到剪贴板（结果操作条"复制"，PRD-01-021）：持久写入，用户随后 `Cmd+V` 即粘贴该文本。
///
/// 区别于双击路径的"只读不写"免污染（PRD-01-027）——这是用户**主动取用**结果，理应改写剪贴板。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_copy(app: tauri::AppHandle, text: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::clipboard::write_text(&text);
    // Windows 上剪贴板可能正被别的程序打开着：写不进去就如实报错，不报「已复制」。
    #[cfg(target_os = "windows")]
    if !crate::platform::windows::clipboard::write_text(&text) {
        return Err(coded_error(
            &app,
            "CLIPBOARD_UNAVAILABLE",
            "clipboard-unavailable",
        ));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = text; // 其它平台的剪贴板写入为后续增量
    #[cfg(not(target_os = "windows"))]
    let _ = app;
    Ok(())
}

/// 写「带格式 + 纯文本」两种表示到剪贴板（编辑器「复制表格为 ▸ 带格式」，PRD-06-011）。
///
/// 内容只经这一次写入，**不入日志/事件通道**（TDD-12 剪贴板脱敏纪律）。
#[tauri::command]
#[specta::specta]
pub(crate) fn clipboard_write_rich(plain: String, html: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::clipboard::write_html(&plain, &html);
    #[cfg(not(target_os = "macos"))]
    let _ = (plain, html); // 其它平台的剪贴板写入为后续增量（与 popup_copy 同边界）
    Ok(())
}

/// 读取剪贴板文本（编辑器右键菜单「粘贴」，PRD-06-014）。
///
/// **只读不写**（不改 changeCount 语义之外的任何剪贴板状态）；内容经返回值直达
/// 调用方插入文档，**绝不入日志/事件通道**（TDD-12 剪贴板脱敏纪律）。
#[tauri::command]
#[specta::specta]
pub(crate) fn read_clipboard_text() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::clipboard::read_text())
    }
    #[cfg(target_os = "windows")]
    {
        Ok(crate::platform::windows::clipboard::read_text())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(None) // 其它平台的剪贴板读取为后续增量（与 popup_copy 同边界）
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ClipboardContentDto {
    pub text: Option<String>,
    pub html: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn read_clipboard_content() -> ClipboardContentDto {
    #[cfg(target_os = "macos")]
    {
        ClipboardContentDto {
            text: crate::platform::macos::clipboard::read_text(),
            html: crate::platform::macos::clipboard::read_html(),
        }
    }
    // Windows 只读文本；带格式（HTML Format）的读写与「复制表格为 ▸ 带格式」一起留作后续。
    #[cfg(target_os = "windows")]
    {
        ClipboardContentDto {
            text: crate::platform::windows::clipboard::read_text(),
            html: None,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        ClipboardContentDto {
            text: None,
            html: None,
        }
    }
}

/// 替换原文：基于触发时捕获的 `selection_snapshot` 注入；没有快照才降级复制译文。
#[tauri::command]
#[specta::specta]
pub(crate) async fn popup_replace(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<PopupReplaceOutcome, String> {
    let snapshot = state
        .popup
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .selection_snapshot();
    replace_selection(&app, snapshot, text).await
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn replace_selection(
    _app: &tauri::AppHandle,
    _snapshot: Option<crate::popup::SelectionSnapshot>,
    _text: String,
) -> Result<PopupReplaceOutcome, String> {
    Err("当前平台尚未实现 TextInjector".to_string())
}

/// Windows「替换原文」（ADR-163 ③）：核对触发时的前台窗口仍在前台、源选区未变，再逐字输入 /
/// 粘贴替换；任何一步不成立都改为复制译文，绝不打进别的窗口。
#[cfg(target_os = "windows")]
async fn replace_selection(
    app: &tauri::AppHandle,
    snapshot: Option<crate::popup::SelectionSnapshot>,
    text: String,
) -> Result<PopupReplaceOutcome, String> {
    use crate::platform::windows::injector::{InjectFailure, WinTextInjector};
    use crate::platform::TextInjector as _;
    use crate::popup::{decide_replace, ReplaceDecision};

    let injector = WinTextInjector;
    let copied = |kind: &str, message: &str| -> Result<PopupReplaceOutcome, String> {
        injector
            .place_in_clipboard(&text)
            .map_err(|e| e.to_string())?;
        Ok(PopupReplaceOutcome {
            kind: kind.to_string(),
            message: message.to_string(),
        })
    };

    let Some(snapshot) =
        snapshot.filter(|s| decide_replace(Some(s)) == ReplaceDecision::AttemptInjection)
    else {
        return copied(
            "copied_fallback",
            "源选区快照已失效，已复制译文，请手动粘贴。",
        );
    };
    match injector
        .replace_selection(
            snapshot.windows_target(),
            snapshot.source_text(),
            text.clone(),
        )
        .await
    {
        Ok(report) if report.clipboard_intact() => {
            let outcome = PopupReplaceOutcome {
                kind: "injected".to_string(),
                message: "已替换原文。".to_string(),
            };
            if replace_panel_action_for_kind(&outcome.kind) == ReplacePanelAction::Close {
                crate::close_popup(app);
            }
            Ok(outcome)
        }
        // 替换做成了，但借用剪贴板之后没能把用户原来的内容放回去：留着弹窗如实告诉他。
        Ok(_) => Ok(PopupReplaceOutcome {
            kind: "injected_clipboard_not_restored".to_string(),
            message: "已替换原文，但剪贴板没能还原。".to_string(),
        }),
        Err(InjectFailure::Fallback(_)) => copied(
            "copied_target_changed",
            "原窗口已不在前台，已复制译文，请手动粘贴。",
        ),
        Err(_) => copied(
            "copied_fallback",
            "目标应用不支持自动替换，已复制译文，请手动粘贴。",
        ),
    }
}

#[cfg(target_os = "macos")]
async fn replace_selection(
    app: &tauri::AppHandle,
    snapshot: Option<crate::popup::SelectionSnapshot>,
    text: String,
) -> Result<PopupReplaceOutcome, String> {
    use crate::platform::TextInjector as _;
    use crate::popup::{decide_replace, ReplaceDecision};

    let injector = crate::platform::macos::MacTextInjector;

    if decide_replace(snapshot.as_ref()) == ReplaceDecision::CopyFallback {
        injector
            .place_in_clipboard(&text)
            .map_err(|e| e.to_string())?;
        return Ok(PopupReplaceOutcome {
            kind: "copied_fallback".to_string(),
            message: "源选区快照已失效，已复制译文，请手动粘贴。".to_string(),
        });
    }

    let snapshot = snapshot.ok_or_else(|| "源选区快照不存在".to_string())?;
    match injector
        .replace_selection_snapshot(&snapshot, text.clone())
        .await
    {
        Ok(crate::platform::macos::injector::SnapshotReplaceOutcome::Injected) => {
            let outcome = PopupReplaceOutcome {
                kind: "injected".to_string(),
                message: "已替换原文。".to_string(),
            };
            if replace_panel_action_for_kind(&outcome.kind) == ReplacePanelAction::Close {
                crate::close_popup(app);
            }
            Ok(outcome)
        }
        Ok(crate::platform::macos::injector::SnapshotReplaceOutcome::CopiedFallback) => {
            Ok(PopupReplaceOutcome {
                kind: "copied_fallback".to_string(),
                message: "目标应用不支持自动替换，已复制译文，请手动粘贴。".to_string(),
            })
        }
        Err(e) => {
            injector
                .place_in_clipboard(&text)
                .map_err(|copy_error| copy_error.to_string())?;
            Ok(PopupReplaceOutcome {
                kind: "copied_fallback".to_string(),
                message: format!("替换不可用，已复制译文，请手动粘贴：{e}"),
            })
        }
    }
}

/// 设置弹窗翻译**输入模式**的源文本（手动键入 / 语音原始转录 / 选区预填，PRD-01-005/006）。
///
/// 决策 A：源文本归 Rust 会话（ADR-003），前端只发文本、不再持有它。源文本改变即整体失效
/// 会话缓存（PRD-01-016）。双击/选区触发路径由原生层直接写会话（交付 #1），不经本命令。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_set_input(
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    // 源文本变了，在途的生成译的是旧文本——继续跑只会把旧译文写进新槽位。
    state.cancel_popup_runs(None);
    state
        .popup
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .set_input(text);
    Ok(())
}

/// 停止弹窗的生成（PRD-01-012 结果槽「停止」）：`slot` = 只停那一槽；`None` = 全部。
///
/// 保留已产 token，前端呈「已取消」态；取消经 `CancelToken` 一路传到 Provider（真 abort，
/// 不是丢弃 future 后让模型继续烧着 GPU 把结果算完）。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_stop(state: tauri::State<'_, AppState>, slot: Option<u32>) {
    state.cancel_popup_runs(slot);
}

/// 固定 / 取消固定弹窗（输入模式下失焦不收，PRD-01-016 修订 / ADR-137 H4）。只管这一次
/// 唤起：下一次唤起重新武装（[`crate::popup::PopupBlurState`]）。返回生效后的值。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_set_pinned(
    state: tauri::State<'_, AppState>,
    pinned: bool,
) -> Result<bool, String> {
    state
        .popup_blur()
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .set_pinned(pinned);
    Ok(pinned)
}

/// 关闭弹窗：清空会话（PRD-01-016 关窗即清、不落盘）+ 隐藏非激活面板。
///
/// 与失焦自动收起 / ESC（经 CGEventTap）共用单一关闭路径（`crate::close_popup`，TDD-05 §5.2）。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_close(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state
        .voice_input
        .cancel_holder(MicHolder::PopupVoiceButton)?;
    crate::close_popup(&app);
    Ok(())
}

/// 为某个槽位**生成结果**（业务全在 Rust，ADR-003）：取会话源文本 → 本地语言检测 → 按模板
/// 目标语言模式决出翻译/润色分支 → 合成最终指令 → 经 `JobQueue` 走主干流式（ADR-001，与
/// 悬浮菜单共用 `stream_translate_events` 投影）。流首发一条 `Meta`（结果性质标识，PRD-01-029）。
///
/// **会话缓存（PRD-01-016）**：`force=false` 时命中 `(源文本, preset)` 即秒显已生成结果、
/// **不重复跑 LLM**；`force=true`（每槽"重新生成"）显式跳过缓存。`Meta` 每次重算（确定、廉价、
/// 永远正确），仅 LLM 最终译文入缓存。
///
/// `slot` = 前端结果槽位 id（主槽 / 对比槽并行生成）：同槽位再次生成会**取消**上一次
/// （换预设、重新生成都是「这一槽要换一份结果」，旧流继续写进来只会把两份 token 交错）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn popup_generate(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    preset_id: String,
    force: bool,
    slot: u32,
    on_event: Channel<TranslateEvent>,
) -> Result<(), String> {
    let preset = builtin_preset(luna_core::prompt::registry(), &preset_id)
        .ok_or_else(|| format!("未知模板：{preset_id}"))?;

    // 取会话源文本（短暂持锁，绝不跨 await）。
    let source = state
        .popup
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .source_text()
        .ok_or_else(|| "弹窗会话尚无源文本（请先设置输入或触发选区）".to_string())?;

    // 语言判定一次 → 分支决策 + 合成（不调 LLM，TDD-03 §3.5）。
    let detection = state.detector.detect(&source);
    let ctx = state.lang_context();
    let (branch, kind) = preset
        .compose(
            luna_core::prompt::registry(),
            &detection,
            &ctx,
            source.clone(),
        )
        .ok_or_else(|| format!("模板 {preset_id} 不是弹窗翻译模板"))?;

    // 结果性质标识先行且每次重算（PRD-01-029）：让用户理解某份结果是翻译还是润色。
    let mode = match branch.kind {
        BranchKind::Translate => "translate",
        BranchKind::Refine => "refine",
    };
    on_event
        .send(TranslateEvent::Meta(TranslateMeta {
            mode: mode.to_string(),
            target: branch.target.to_string(),
            uncertain: branch.uncertain,
            preset_id: preset_id.clone(),
        }))
        .map_err(|e| e.to_string())?;

    // 自动复制开关（PRD-01-032）：终态成功后把译文写入剪贴板并追发 AutoCopied 反馈。
    let auto_copy = state
        .settings_snapshot()
        .get_bool("popup_translate.auto_copy")
        .unwrap_or(false);

    // 缓存命中即秒显、不跑 LLM（PRD-01-016）。
    if !force {
        let cached = state
            .popup
            .lock()
            .map_err(|_| "会话锁中毒".to_string())?
            .cached(&source, &preset_id);
        if let Some(text) = cached {
            on_event
                .send(TranslateEvent::CacheHit(text.clone()))
                .map_err(|e| e.to_string())?;
            if auto_copy {
                auto_copy_result(&text, &on_event)?;
            }
            return Ok(());
        }
    }

    // **术语表注入**（PRD-01-017，ADR-083 已知代价 ①）：弹窗进**实时档**（= 默认跑更小
    // 的模型），而它的产出会被「替换原文」直接写进用户的邮件与代码。注入函数与悬浮菜单
    // 共用（`with_job_glossary`），四条翻译 / 润色路径从此真的是同一处。
    let kind = with_job_glossary(kind, &source, &load_glossary_entries(&state));

    let mut job = Job::new(kind, JobOrigin::PopupTranslate);
    job.options = preset.options.clone();
    job.routing.local_only = state.settings_snapshot().strict_local(); // 严格本地护栏（TDD-12 §1）
                                                                       // 弹窗翻译为交互式（插队低延迟，TDD-01 §5.2）。
                                                                       //
                                                                       // **服务档不在这里声明**：它由 `Job::service_class()` 从 `JobOrigin::PopupTranslate`
                                                                       // 推导（ADR-083 ③④），本模块一个字节的档位判断都不写——那正是这次缺陷的成因，
                                                                       // 弹窗此前连 `preferred_provider` 都没设，于是本该最看重延迟的调用落在质量档上。
    job.priority = Priority::Interactive;

    let handle = state.queue.submit(job);
    // 登记本槽位的取消令牌（顶替并取消该槽位上一次），流结束后只清自己那一代。
    let generation = state.register_popup_run(slot, handle.cancel_token())?;
    let outcome = stream_translate_events(handle, &on_event, Some(&app)).await;
    state.finish_popup_run(slot, generation);
    // 成功完成则把最终译文回填会话缓存（短暂持锁，在 await 之后）。
    if let Some(text) = outcome? {
        state
            .popup
            .lock()
            .map_err(|_| "会话锁中毒".to_string())?
            .put_cache(source, preset_id, text.clone());
        // 首个 AI 任务的「成功时刻」（PRD-14-050）：一次真实译文即算。
        super::onboarding::mark_first_task_done(&app, &state);
        if auto_copy {
            auto_copy_result(&text, &on_event)?;
        }
    }
    Ok(())
}

/// 自动复制译文到剪贴板并回发 `AutoCopied` 反馈（`popup_generate` 完成/缓存命中共用，PRD-01-032）。
fn auto_copy_result(text: &str, on_event: &Channel<TranslateEvent>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    crate::platform::macos::clipboard::write_text(text);
    #[cfg(not(target_os = "macos"))]
    let _ = text; // 其它平台的剪贴板写入为后续增量（与 popup_copy 同口径）
    on_event
        .send(TranslateEvent::AutoCopied)
        .map_err(|e| e.to_string())
}

/// 带预填原文唤起弹窗（PRD-14-050 首启「第一个任务」）：把一句示例原文写进会话再亮窗，
/// 用户点一下生成即见译文——比一个空输入框更接近"成功时刻"。文案由前端按界面语言给。
#[tauri::command]
#[specta::specta]
pub(crate) fn popup_show_input(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    if app.get_webview_window("popup-translate").is_none() {
        return Err("popup-translate 窗口未预热".to_string());
    }
    state.cancel_popup_runs(None);
    state
        .popup
        .lock()
        .map_err(|_| "会话锁中毒".to_string())?
        .set_input(text);
    crate::show_popup_panel(&app);
    Ok(())
}

/// 截屏翻译（PRD-01-007）：弹窗内按钮入口。与全局快捷键/托盘共用
/// [`crate::screenshot::run_screenshot_translate`] 单一编排。
#[tauri::command]
#[specta::specta]
pub(crate) async fn popup_screenshot_translate(app: tauri::AppHandle) -> Result<(), String> {
    crate::screenshot::run_screenshot_translate(app).await;
    Ok(())
}

/// **ADR-015 spike 独立入口**：直接显示已预热的非激活弹窗，不依赖双击 Cmd+C（S1）。
///
/// 让 A4（类交换后 webview 是否正常渲染）等可单独验证。窗口在 macOS 启动时预热创建；
/// 未预热则返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn spike_show_popup(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager as _;
    if app.get_webview_window("popup-translate").is_none() {
        return Err("popup-translate 窗口未预热".to_string());
    }
    if let Some(state) = app.try_state::<AppState>() {
        state
            .popup
            .lock()
            .map_err(|_| "会话锁中毒".to_string())?
            .set_empty_input();
    }
    crate::show_popup_panel(&app); // show + 注册临时 Escape（可 ESC 关闭）
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::{GlossaryEntry, Lang, TextRef};

    fn glossary() -> Vec<GlossaryEntry> {
        vec![
            GlossaryEntry {
                term: "Lunarca".into(),
                canonical: "月弧".into(),
            },
            // 本次输入里没出现：绝不该被注入（全表注入会撑爆 prompt）。
            GlossaryEntry {
                term: "Vault".into(),
                canonical: "库".into(),
            },
        ]
    }

    fn translate_kind(instruction: Option<&str>) -> JobKind {
        JobKind::Translate {
            text: TextRef::Inline("Lunarca ships today.".into()),
            source: None,
            target: Lang::from("zh"),
            instruction: instruction.map(luna_core::Instruction::raw),
            template: None,
        }
    }

    /// PRD-01-017 的那条缺口：弹窗此前**从头到尾没接过词典**，而它随 ADR-083 进实时档
    /// （= 默认跑更小的模型）之后，产出会被「替换原文」直接写进用户的邮件与代码。
    #[test]
    fn popup_translate_injects_only_the_terms_that_actually_appear() {
        let JobKind::Translate { instruction, .. } =
            with_job_glossary(translate_kind(None), "Lunarca ships today.", &glossary())
        else {
            panic!("应仍是 Translate");
        };
        let instruction = instruction.expect("命中术语应产生约束段");
        assert!(instruction.contains("Lunarca => 月弧"), "命中项必须注入");
        assert!(
            !instruction.contains("Vault"),
            "没出现的术语对这段文本毫无约束力，注入它只会撑大 prompt"
        );
    }

    #[test]
    fn popup_glossary_appends_to_the_preset_style_instead_of_replacing_it() {
        // 模板风格（`{style}`）是用户选的那一个，术语约束是追加不是覆盖。
        let JobKind::Translate { instruction, .. } = with_job_glossary(
            translate_kind(Some("Use a polite tone.")),
            "Lunarca ships today.",
            &glossary(),
        ) else {
            panic!("应仍是 Translate");
        };
        let instruction = instruction.expect("有指令");
        assert!(instruction.starts_with("Use a polite tone."));
        assert!(instruction.contains("Lunarca => 月弧"));
    }

    #[test]
    fn popup_refine_also_gets_the_glossary() {
        // 润色不译词，但 LLM 会把专有名词「顺手改回」错误形式——那与译错是同一种损失。
        let JobKind::Refine { instruction, .. } = with_job_glossary(
            JobKind::Refine {
                text: TextRef::Inline("Lunarca ships today.".into()),
                instruction: None,
            },
            "Lunarca ships today.",
            &glossary(),
        ) else {
            panic!("应仍是 Refine");
        };
        assert!(instruction
            .expect("命中术语应产生约束段")
            .contains("Lunarca => 月弧"));
    }

    #[test]
    fn no_glossary_hit_leaves_the_instruction_exactly_as_it_was() {
        let JobKind::Translate { instruction, .. } =
            with_job_glossary(translate_kind(None), "nothing matches here", &glossary())
        else {
            panic!("应仍是 Translate");
        };
        assert_eq!(instruction, None, "无命中不得凭空造出一句指令");
    }

    /// **弹窗归实时档是 `Job::service_class()` 推导出来的**（ADR-083 ③）：本模块里
    /// 一个字节的档位判断都不写。此前它连 `preferred_provider` 都不设，于是本该最看重
    /// 延迟的那条调用落在质量档上——声明是会漏的，这就是漏了的样子。
    #[test]
    fn popup_translate_is_realtime_because_of_its_origin_not_a_local_branch() {
        let job = Job::new(translate_kind(None), JobOrigin::PopupTranslate);
        assert_eq!(job.service_class(), luna_core::ServiceClass::Realtime);
        assert_eq!(job.model_slot(), Some(luna_core::ModelSlot::RealtimeLlm));
    }
}
