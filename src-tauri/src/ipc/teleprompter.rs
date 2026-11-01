//! 提词器（ADR-162 ⑤ C4a，PRD-19-016）：常驻浮动面板显示脚本里要念的文字。
//!
//! 念什么由 Rust 从脚本抽出（`luna_vault::prompter_blocks`：按段分开、去掉代码块与行首形式
//! 标记）；面板只渲染、只发命令（ADR-003）。字号、行宽、镜像、滚动速度是内部设置
//! （`teleprompter.*`），面板按钮经通用设置命令改写。

use super::*;
use luna_core::prompter_align::{PrompterAligner, ScriptPosition};

/// 提词器面板的窗口标签（只有一扇，常驻）。
pub(crate) const TELEPROMPTER_LABEL: &str = "teleprompter";

/// 提词器上的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PrompterLineDto {
    pub text: String,
    /// 行首形式标记（`口播` / `出镜` / `VO` / `CAM`），显示成小标签，不念。
    pub marker: Option<String>,
}

/// 提词器上的一段（一个二级标题下的各行；开场段没有标题）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PrompterBlockDto {
    pub heading: Option<String>,
    pub lines: Vec<PrompterLineDto>,
}

/// 提词器此刻要显示的脚本。`path` 为 `None` = 还没打开过任何脚本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TeleprompterDto {
    pub path: Option<String>,
    pub title: String,
    pub blocks: Vec<PrompterBlockDto>,
}

fn prompter_dto(path: &str, content: &str) -> TeleprompterDto {
    TeleprompterDto {
        path: Some(path.to_owned()),
        title: note_stem_of(path),
        blocks: luna_vault::prompter_blocks(content)
            .into_iter()
            .map(|block| PrompterBlockDto {
                heading: block.heading,
                lines: block
                    .lines
                    .into_iter()
                    .map(|line| PrompterLineDto {
                        text: line.text,
                        marker: line.marker,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// 用提词器打开一篇脚本：记下是哪一篇，显示面板（回到记下的位置），面板随即重读。`follow`：
/// 打开后直接开始跟读（录制页签的「开始跟读」，ADR-165 ⑧）——跟读的会话通道在面板里，
/// 由面板自己开起来。
///
/// # Errors
/// 「创作」没启用（`APP_DISABLED`，窗口归它，ADR-169 ⑧）、库未打开或这一篇读不出时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_open(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    follow: bool,
) -> Result<(), String> {
    require_window(&app, &state, TELEPROMPTER_LABEL)?;
    state.with_vault(window.label(), |vault| {
        vault.read(&path).map(|_| ()).map_err(vault_err)
    })?;
    let previous = state
        .teleprompter_script
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?
        .replace(path.clone());
    // 换了一篇脚本：上一篇攒下的录制标记先写回去（C4c）。
    if previous.as_deref() != Some(path.as_str()) {
        flush_retake_marks(&app);
    }
    crate::show_teleprompter(&app);
    if follow {
        use tauri::Manager as _;
        if let Some(panel) = app.get_webview_window(TELEPROMPTER_LABEL) {
            let _ = panel.eval("window.__lunaTeleprompterFollow?.()");
        }
    }
    Ok(())
}

/// 提词器面板取它要显示的内容（打开时、换脚本时、脚本被保存时各取一次）。
///
/// # Errors
/// 库未打开或脚本读不出时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn teleprompter_content(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<TeleprompterDto, String> {
    let path = state
        .teleprompter_script
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?
        .clone();
    let Some(path) = path else {
        return Ok(TeleprompterDto {
            path: None,
            title: String::new(),
            blocks: Vec::new(),
        });
    };
    state.with_vault(window.label(), |vault| {
        let doc = vault.read(&path).map_err(vault_err)?;
        Ok(prompter_dto(&path, &doc.content))
    })
}

/// 收起提词器（记下位置与尺寸；面板只隐藏不关闭），攒下的录制标记随之写回脚本。
#[tauri::command]
#[specta::specta]
pub(crate) fn teleprompter_hide(app: tauri::AppHandle) {
    crate::hide_teleprompter(&app);
    flush_retake_marks(&app);
}

// ── 跟读（C4b，PRD-19-015）──────────────────────────────────────────────────

/// 跟读位置的二进制载荷（ADR-011）：段、行、行内第几个字，各 4 字节 LE。
fn position_payload(position: ScriptPosition) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(&position.block.to_le_bytes());
    bytes.extend_from_slice(&position.line.to_le_bytes());
    bytes.extend_from_slice(&position.char_offset.to_le_bytes());
    bytes
}

/// 由脚本全文建对齐器：段号、行号与 `teleprompter_content` 给面板的是同一套。脚本先统一成
/// 简体——识别文字也一样（见旁听者），两边才是同一种字形。
fn aligner_for(content: &str) -> PrompterAligner {
    let blocks = luna_vault::prompter_blocks(content);
    let lines: Vec<(u32, u32, String)> = blocks
        .into_iter()
        .enumerate()
        .flat_map(|(block, b)| {
            b.lines.into_iter().enumerate().map(move |(line, l)| {
                (
                    u32::try_from(block).unwrap_or(u32::MAX),
                    u32::try_from(line).unwrap_or(u32::MAX),
                    luna_export::to_simplified(&l.text),
                )
            })
        })
        .collect();
    PrompterAligner::new(
        lines
            .iter()
            .map(|(block, line, text)| (*block, *line, text.as_str())),
    )
}

/// 开始跟读（ADR-162 ⑤ C4b）：起一个只听麦克风、不落盘、不转译的录音会话
/// （`SessionPurpose::Teleprompter`），主轨转写喂给对齐器，念到的位置一变就经二进制
/// Channel 推给面板。停止 / 暂停沿用 `recording_*` 控制命令（按 session_id 作用于活动会话）。
///
/// # Errors
/// 没有打开脚本、麦克风权限缺失、转写不可用或麦克风正被别的会话占用时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_follow_start(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    session_id: String,
    on_event: tauri::ipc::Channel<RecordingEvent>,
    on_position: tauri::ipc::Channel<RawBytes>,
) -> Result<RecordingState, String> {
    let path = state
        .teleprompter_script
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?
        .clone()
        .ok_or_else(|| coded_error(&app, "TELEPROMPTER_NO_SCRIPT", "teleprompter-no-script"))?;
    let content = state.with_vault(window.label(), |vault| {
        vault.read(&path).map(|doc| doc.content).map_err(vault_err)
    })?;
    ensure_microphone_permission()?;
    state.ensure_asr_provider_ready(&app)?;
    let settings = state.settings_snapshot();
    let lang_hint = settings
        .get_str("realtime.transcribe_lang")
        .filter(|s| !s.trim().is_empty() && s != "auto")
        .map(|s| luna_core::Lang::from(s.as_str()));
    let input_device = settings
        .get_str("recording.input_device")
        .filter(|s| !s.trim().is_empty());
    let glossary = load_glossary_entries(&state);
    let translate_config = realtime_translate_config(
        &settings,
        Arc::clone(&state.detector),
        Arc::from(glossary.clone()),
    );

    let aligner = Arc::new(Mutex::new(aligner_for(&content)));
    let follow = Arc::clone(&aligner);
    let observer: crate::recording::PartialObserver = Arc::new(move |committed, hypothesis| {
        // Whisper 识别普通话常出繁体：统一成简体再对（脚本那边建对齐器时同样转过）。
        let committed = luna_export::to_simplified(committed);
        let hypothesis = luna_export::to_simplified(hypothesis);
        let moved = aligner
            .lock()
            .ok()
            .and_then(|mut aligner| aligner.feed(&committed, &hypothesis));
        if let Some(position) = moved {
            // 只记段号行号（日志不含转写文字）：真机核对标记落点时要知道高亮当时在哪。
            eprintln!(
                "[C4c] 跟读位置 → 段 {} 行 {} 字 {}",
                position.block, position.line, position.char_offset
            );
            let _ = on_position.send(crate::voice_input::voice_level_payload(position_payload(
                position,
            )));
        }
    });

    let options = RecordingStartOptions {
        session_id,
        purpose: crate::recording::SessionPurpose::Teleprompter,
        note_rel: String::new(),
        audio_dir: None,
        vault_root: None,
        format: luna_audio::RecordingFormat::Wav,
        tuning: capture_tuning(&settings),
        vad: state.vad_factory(&app),
        lang_hint,
        input_device,
        // 只听麦克风：念稿的人在麦克风这一侧，系统声音里没有要跟的东西。
        capture_source: crate::recording::RecordingCaptureSource::Mic,
        transcribe_enabled: true,
        translate_enabled: false,
        translate_config,
        glossary,
        asr_provider: state.asr_provider_handle(),
        asr_model_name: Arc::new(std::sync::Mutex::new(None)),
        // 跟读不归档：收尾走不归档分支，不调用此闭包。
        finalize_fn: Box::new(|_, _, _| Ok(String::new())),
        partial_observer: Some(observer),
    };
    let started = state.recording.start(
        state.queue_handle(),
        state.mic_handle(),
        options,
        app.clone(),
        on_event,
    );
    // 会话真正开始了才挂上跟读状态（麦克风被占时什么都不留）；会话收场时由
    // [`end_follow`] 清掉。
    if started.as_ref().is_ok_and(|state| {
        state.purpose == crate::recording::SessionPurpose::Teleprompter
            && state.status == crate::recording::RecordingStatus::Recording
    }) {
        *state
            .teleprompter_follow
            .lock()
            .map_err(|_| "提词器状态锁中毒".to_owned())? = Some(follow);
    }
    // 跟读同样占麦克风：创建入口的可用性随之变化（与实时字幕同理）。
    super::create_actions::publish_create_action_states(&app);
    started
}

/// 跟读时「回到开头」：对齐器也从头对起（不在跟读时什么都不做）。
#[tauri::command]
#[specta::specta]
pub(crate) fn teleprompter_follow_reset(state: tauri::State<'_, AppState>) {
    if let Ok(follow) = state.teleprompter_follow.lock() {
        if let Some(aligner) = follow.as_ref() {
            if let Ok(mut aligner) = aligner.lock() {
                aligner.reset();
            }
        }
    }
}

/// 录音会话收场（任何终态，含出错）时调用：跟读状态清空，跟读期间攒下的录制标记写回脚本。
/// 不在跟读时什么都不做。
pub(crate) fn end_follow(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let was_following = state
        .teleprompter_follow
        .lock()
        .ok()
        .and_then(|mut follow| follow.take())
        .is_some();
    if was_following {
        flush_retake_marks(app);
    }
}

// ── 录制标记（C4c，PRD-19-017）─────────────────────────────────────────────

/// 还没写回脚本的一条录制标记：同一篇同一行的多次合成一条，记下每次的时刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingRetake {
    path: String,
    block: u32,
    line: u32,
    /// 标记时那一行的文字（写回时据此核对，脚本改过也找得到）。
    text: String,
    /// 每次标记的本地时刻（`HH:MM`）。
    times: Vec<String>,
}

/// 面板报来的「此刻那一行」（跟读时是高亮行，没跟读时是阅读线上的那一行）。
#[derive(Debug, Clone, Copy, Deserialize, Type)]
pub struct PrompterAtDto {
    pub block: u32,
    pub line: u32,
}

/// 在攒着的标记里记一笔，返回此刻一共攒了几次。
fn add_mark(
    marks: &mut Vec<PendingRetake>,
    path: &str,
    block: u32,
    line: u32,
    text: &str,
    time: String,
) -> u32 {
    match marks
        .iter_mut()
        .find(|mark| mark.path == path && mark.block == block && mark.line == line)
    {
        Some(mark) => mark.times.push(time),
        None => marks.push(PendingRetake {
            path: path.to_owned(),
            block,
            line,
            text: text.to_owned(),
            times: vec![time],
        }),
    }
    pending_count(marks)
}

fn pending_count(marks: &[PendingRetake]) -> u32 {
    u32::try_from(marks.iter().map(|mark| mark.times.len()).sum::<usize>()).unwrap_or(u32::MAX)
}

/// 标记「这一句重来」（ADR-162 ⑤ C4c）：`at` 是面板报来的那一行；不给时取跟读念到的位置
/// （快捷键路径）。标记先攒在本机，停止跟读 / 收起提词器 / 换脚本时一次写回。返回攒了几次。
///
/// # Errors
/// 没有打开脚本、既没给行也没在跟读、或那一行已不在脚本里时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_mark(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    at: Option<PrompterAtDto>,
) -> Result<u32, String> {
    record_mark(&app, &state, at)
}

/// [`teleprompter_mark`] 的本体，快捷键路径也走这里。
pub(crate) fn record_mark(
    app: &tauri::AppHandle,
    state: &AppState,
    at: Option<PrompterAtDto>,
) -> Result<u32, String> {
    let no_line = || coded_error(app, "TELEPROMPTER_NO_LINE", "teleprompter-no-line");
    let path = state
        .teleprompter_script
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?
        .clone()
        .ok_or_else(|| coded_error(app, "TELEPROMPTER_NO_SCRIPT", "teleprompter-no-script"))?;
    let following = state
        .teleprompter_follow
        .lock()
        .ok()
        .and_then(|follow| follow.clone())
        .and_then(|aligner| aligner.lock().ok().map(|aligner| aligner.position()));
    eprintln!(
        "[C4c] 标记：面板报 {:?}，跟读位置 {:?}",
        at.map(|at| (at.block, at.line)),
        following.map(|p| (p.block, p.line))
    );
    let (block, line) = match at {
        Some(at) => (at.block, at.line),
        None => {
            let follow = state
                .teleprompter_follow
                .lock()
                .map_err(|_| "提词器状态锁中毒".to_owned())?
                .clone()
                .ok_or_else(no_line)?;
            let position = follow
                .lock()
                .map_err(|_| "提词器状态锁中毒".to_owned())?
                .position();
            (position.block, position.line)
        }
    };
    let content = state.with_vault(TELEPROMPTER_LABEL, |vault| {
        vault.read(&path).map(|doc| doc.content).map_err(vault_err)
    })?;
    let text = luna_vault::prompter_blocks(&content)
        .get(usize::try_from(block).unwrap_or(usize::MAX))
        .and_then(|b| b.lines.get(usize::try_from(line).unwrap_or(usize::MAX)))
        .map(|l| l.text.clone())
        .ok_or_else(no_line)?;
    let time = chrono::Local::now().format("%H:%M").to_string();
    let mut marks = state
        .teleprompter_marks
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?;
    let count = add_mark(&mut marks, &path, block, line, &text, time);
    Ok(count)
}

/// 「录音打点」快捷键在跟读会话里按下（`lib.rs` 的快捷键分发调用）：标在念到的那一行上，
/// 并告诉面板攒了几次。标不上（还没打开脚本等）只记日志——快捷键没有地方弹错误。
pub(crate) fn mark_from_hotkey(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    match record_mark(app, &state, None) {
        Ok(_) => notify_panel_marks(app, &state, 0),
        Err(error) => eprintln!("[C4c] 快捷键打标记未成：{error}"),
    }
}

/// 标注块标题：「重来 · 20:41」或「重来 ×2 · 20:41 20:43」（按界面语言）。
fn retake_title(i18n: BackendCopy<'_>, times: &[String]) -> String {
    let joined = times.join(" ");
    let (key, fallback) = if times.len() > 1 {
        (
            "teleprompter-retake-many",
            format!("×{} · {joined}", times.len()),
        )
    } else {
        ("teleprompter-retake-once", joined.clone())
    };
    i18n.and_then(|i18n| {
        i18n.translate_args(
            key,
            &[
                ("count", times.len().to_string()),
                ("times", joined.clone()),
            ],
        )
    })
    .unwrap_or(fallback)
}

/// 把攒下的录制标记写回各自的脚本（紧跟被标的那一行插入 `> [!retake]` 标注块），写成的经
/// `note://written` 告诉打开着它的编辑器；写不成的放回去等下一次。返回写进去的条数，并把
/// 结果告诉提词器面板。
pub(crate) fn flush_retake_marks(app: &tauri::AppHandle) -> usize {
    let Some(state) = app.try_state::<AppState>() else {
        return 0;
    };
    let pending = match state.teleprompter_marks.lock() {
        Ok(mut marks) => std::mem::take(&mut *marks),
        Err(_) => return 0,
    };
    if pending.is_empty() {
        return 0;
    }
    let i18n = backend_copy_arc(app);
    let mut paths: Vec<&str> = Vec::new();
    for mark in &pending {
        if !paths.contains(&mark.path.as_str()) {
            paths.push(&mark.path);
        }
    }
    let mut written = 0;
    let mut failed: Vec<PendingRetake> = Vec::new();
    for path in paths {
        let group: Vec<&PendingRetake> = pending.iter().filter(|m| m.path == path).collect();
        let marks: Vec<luna_vault::RetakeMark> = group
            .iter()
            .map(|mark| luna_vault::RetakeMark {
                block: mark.block,
                line: mark.line,
                text: mark.text.clone(),
                title: retake_title(i18n.as_deref(), &mark.times),
            })
            .collect();
        match write_marks(&state, path, &marks) {
            Ok(Some((content, count))) => {
                written += count;
                let _ = (NoteWritten {
                    path: path.to_owned(),
                    content,
                    origin: TELEPROMPTER_LABEL.to_owned(),
                })
                .emit(app);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("[C4c] 录制标记写回失败，留待下次：{error}");
                failed.extend(group.into_iter().cloned());
            }
        }
    }
    if !failed.is_empty() {
        if let Ok(mut marks) = state.teleprompter_marks.lock() {
            failed.append(&mut marks);
            *marks = failed;
        }
    }
    eprintln!("[C4c] 写回 {written} 处录制标记");
    notify_panel_marks(app, &state, written);
    written
}

/// 读—插—按读到的基线保存；保存时文件刚好又变了就重来一次。没有可写的返回 `None`。
fn write_marks(
    state: &AppState,
    path: &str,
    marks: &[luna_vault::RetakeMark],
) -> Result<Option<(String, usize)>, String> {
    for _ in 0..2 {
        let outcome = state.with_vault(TELEPROMPTER_LABEL, |vault| {
            let base = vault.read(path).map_err(vault_err)?.content;
            let (next, count) = luna_vault::insert_retake_marks(&base, marks);
            if count == 0 {
                return Ok(Some(None));
            }
            match vault.save_note(path, &base, &next).map_err(vault_err)? {
                luna_vault::NoteSave::Saved { .. } => Ok(Some(Some((next, count)))),
                luna_vault::NoteSave::Conflict { .. } => Ok(None),
            }
        })?;
        if let Some(result) = outcome {
            return Ok(result);
        }
    }
    Err("脚本在写回期间一直在变".to_owned())
}

/// 告诉提词器面板此刻攒了几次、刚写回了几处（面板据此更新提示）。
pub(crate) fn notify_panel_marks(app: &tauri::AppHandle, state: &AppState, written: usize) {
    let pending = state
        .teleprompter_marks
        .lock()
        .map(|marks| pending_count(&marks))
        .unwrap_or(0);
    if let Some(win) = app.get_webview_window(TELEPROMPTER_LABEL) {
        let _ = win.eval(format!(
            "window.__lunaTeleprompterMarks?.({pending}, {written})"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_carries_sections_lines_and_markers_and_titles_by_file_name() {
        let dto = prompter_dto(
            "Creator/第一期/第一期-脚本.md",
            "## 开头\n[口播] 你好。\n\n```broll\n空镜\n```\n## 结尾\n再见。\n",
        );
        assert_eq!(dto.title, "第一期-脚本");
        assert_eq!(dto.blocks.len(), 2);
        assert_eq!(dto.blocks[0].heading.as_deref(), Some("开头"));
        assert_eq!(
            dto.blocks[0].lines,
            [PrompterLineDto {
                text: "你好。".into(),
                marker: Some("口播".into())
            }]
        );
        assert_eq!(dto.blocks[1].lines[0].text, "再见。");
    }

    #[test]
    fn positions_are_twelve_little_endian_bytes_and_line_numbers_match_the_panel() {
        let bytes = position_payload(ScriptPosition {
            block: 1,
            line: 2,
            char_offset: 300,
        });
        assert_eq!(bytes, [1, 0, 0, 0, 2, 0, 0, 0, 44, 1, 0, 0]);
        // 与面板同一套段号行号：开场段是第 0 段，broll 不占行。
        let script =
            "先打个招呼大家好\n\n## 开头\n```broll\n空镜\n```\n[口播] 今天我们聊一聊怎么写脚本\n";
        let mut aligner = aligner_for(script);
        let position = aligner.feed("今天我们聊一聊怎么写", "").unwrap();
        assert_eq!((position.block, position.line), (1, 0));
        assert_eq!(
            prompter_dto("P.md", script).blocks[1].lines[0].text,
            "今天我们聊一聊怎么写脚本"
        );
    }

    #[test]
    fn traditional_recognition_still_follows_a_simplified_script() {
        // 真机：随包小模型把普通话转成繁体；不统一字形时这一句一个位置都对不上。
        let script = "## 开头\n大家好，今天我们聊一聊怎么把视频脚本写得更顺口。\n很多人写完稿子一念，才发现句子太长、喘不过气。\n";
        let mut aligner = aligner_for(script);
        let heard = luna_export::to_simplified("很多人寫完稿子一念才發現句子太長");
        let position = aligner.feed(&heard, "").unwrap();
        assert_eq!((position.block, position.line), (0, 1));
    }

    #[test]
    fn marks_on_the_same_line_merge_and_the_count_adds_up() {
        let mut marks = Vec::new();
        assert_eq!(
            add_mark(&mut marks, "A.md", 0, 1, "大家好。", "20:41".into()),
            1
        );
        assert_eq!(
            add_mark(&mut marks, "A.md", 0, 1, "大家好。", "20:43".into()),
            2
        );
        assert_eq!(
            add_mark(&mut marks, "A.md", 1, 0, "下期见。", "20:44".into()),
            3
        );
        assert_eq!(
            add_mark(&mut marks, "B.md", 0, 1, "大家好。", "20:45".into()),
            4
        );
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[0].times, ["20:41", "20:43"]);
    }

    #[test]
    fn the_callout_title_says_how_many_times_and_when() {
        let zh = crate::i18n::BackendI18n::new("zh-CN");
        assert_eq!(retake_title(Some(&zh), &["20:41".into()]), "重来 · 20:41");
        assert_eq!(
            retake_title(Some(&zh), &["20:41".into(), "20:43".into()]),
            "重来 ×2 · 20:41 20:43"
        );
        let en = crate::i18n::BackendI18n::new("en-US");
        assert_eq!(retake_title(Some(&en), &["20:41".into()]), "Retake · 20:41");
    }
}
