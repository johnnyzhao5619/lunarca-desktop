//! 提词器（ADR-162 ⑤ C4a，PRD-19-016）：常驻浮动面板显示脚本里要念的文字。
//!
//! 念什么由 Rust 从脚本抽出（`luna_vault::prompter_blocks`：按段分开、去掉代码块与行首形式
//! 标记）；面板只渲染、只发命令（ADR-003）。字号、行宽、镜像、滚动速度是内部设置
//! （`teleprompter.*`），面板按钮经通用设置命令改写。

use super::*;
use luna_core::prompter_align::{PrompterAligner, ScriptPosition};

/// 提词器面板的窗口标签（只有一扇，常驻）。
pub(crate) const TELEPROMPTER_LABEL: &str = "teleprompter";

/// 提词器一行里的一段（口播记号，ADR-176 ⑦）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrompterSpanDto {
    Text {
        text: String,
    },
    /// 强调：要念，加粗加色。
    Emphasis {
        text: String,
    },
    /// 停一拍：不念。
    Pause,
    /// 不念的提示：灰色小字。
    Note {
        text: String,
    },
}

impl From<luna_vault::PrompterSpan> for PrompterSpanDto {
    fn from(span: luna_vault::PrompterSpan) -> Self {
        match span {
            luna_vault::PrompterSpan::Text(text) => Self::Text { text },
            luna_vault::PrompterSpan::Emphasis(text) => Self::Emphasis { text },
            luna_vault::PrompterSpan::Pause => Self::Pause,
            luna_vault::PrompterSpan::Note(text) => Self::Note { text },
        }
    }
}

/// 提词器上的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PrompterLineDto {
    /// 要念的文字（口播记号已去掉；跟读对齐、录制标记都按它）。
    pub text: String,
    /// 行首形式标记（`口播` / `出镜` / `VO` / `CAM`），显示成小标签，不念。
    pub marker: Option<String>,
    /// 这一行约念多少毫秒（本人语速 + 停顿，与脚本时长估算同一口径）：面板据此给「预计剩余」。
    pub millis: u32,
    /// 显示用的各段（ADR-176 ⑦）。
    pub spans: Vec<PrompterSpanDto>,
    /// 动作提示行（`> [!cue] …`）：整行不念，显示成彩色条。
    pub cue: bool,
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
    /// 「提词器」应用生效没有：倒计时、计时、贴近镜头、防录屏等扩展只在它生效时出现（ADR-176 ③）。
    pub extended: bool,
    /// 改稿重读时找回的那一行（PRD-20-015）：面板请求里带了 `anchor` 才有。
    pub anchor: Option<PrompterAtDto>,
}

/// 面板重读时带上的「刚才那一行」（改稿前的段号行号与要念的文字，PRD-20-015）。`path` 是面板
/// 刚才显示的那一篇：此刻换成了别的稿子就不找回、不动跟读。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct PrompterAnchorDto {
    pub path: String,
    pub block: u32,
    pub line: u32,
    pub text: String,
}

fn prompter_dto(
    path: &str,
    content: &str,
    rate: luna_vault::SpeakingRate,
    extended: bool,
) -> TeleprompterDto {
    TeleprompterDto {
        path: Some(path.to_owned()),
        title: note_stem_of(path),
        extended,
        anchor: None,
        blocks: luna_vault::prompter_blocks(content)
            .into_iter()
            .map(|block| PrompterBlockDto {
                heading: block.heading,
                lines: block
                    .lines
                    .into_iter()
                    .map(|line| PrompterLineDto {
                        millis: luna_vault::spoken_millis(&line.text, rate)
                            + line.pauses * luna_vault::PAUSE_MILLIS,
                        text: line.text,
                        marker: line.marker,
                        spans: line.spans.into_iter().map(Into::into).collect(),
                        cue: line.cue,
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
    open_script(&app, &state, window.label(), &path, follow)
}

/// [`teleprompter_open`] 的本体：送稿路径（快捷键、服务菜单、拖放）也走这里。
pub(crate) fn open_script(
    app: &tauri::AppHandle,
    state: &AppState,
    label: &str,
    path: &str,
    follow: bool,
) -> Result<(), String> {
    let (app, path) = (app.clone(), path.to_owned());
    require_window(&app, state, TELEPROMPTER_LABEL)?;
    state.with_vault(label, |vault| {
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
    remember_recent(&app, state, label, &path);
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
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_content(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    anchor: Option<PrompterAnchorDto>,
) -> Result<TeleprompterDto, String> {
    let extended = crate::teleprompter_extended(&app);
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
            extended,
            anchor: None,
        });
    };
    let (mut dto, content) = state.with_vault(window.label(), |vault| {
        let doc = vault.read(&path).map_err(vault_err)?;
        let dto = prompter_dto(&path, &doc.content, vault.speaking_rate(), extended);
        Ok((dto, doc.content))
    })?;
    // 改稿重读（PRD-20-015）：在新稿里找回刚才那一行；跟读开着就用新稿重建对齐器、从那一行接着跟。
    if let Some(anchor) = anchor.filter(|anchor| anchor.path == path) {
        let blocks = luna_vault::prompter_blocks(&content);
        let found = luna_vault::relocate_line(&blocks, &anchor.text, anchor.block, anchor.line);
        if let Some((block, line)) = found {
            dto.anchor = Some(PrompterAtDto { block, line });
            rebase_follow(&state, &content, block, line);
        }
    }
    Ok(dto)
}

/// 稿子改了而跟读开着：对齐器换成新稿的，从 `(block, line)` 接着跟（不在跟读时什么都不做）。
fn rebase_follow(state: &AppState, content: &str, block: u32, line: u32) {
    let Ok(follow) = state.teleprompter_follow.lock() else {
        return;
    };
    if let Some(aligner) = follow.as_ref() {
        if let Ok(mut aligner) = aligner.lock() {
            *aligner = aligner_for(content);
            aligner.seek(block, line);
        }
    }
}

/// 贴近镜头（ADR-176 ⑤，PRD-20-008）：面板放到所在显示器工作区的上沿居中、收成窄条，并记住。
///
/// # Errors
/// 「提词器」应用没生效时返回 `APP_DISABLED`（贴近镜头是它的扩展能力）。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_snap_to_camera(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    super::apps::require_app(&app, &state, luna_config::apps::TELEPROMPTER_APP.id)?;
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window(TELEPROMPTER_LABEL) {
        crate::snap_teleprompter_to_camera(&win);
        crate::remember_caption_window(&app, &win);
    }
    Ok(())
}

/// 收起提词器（记下位置与尺寸；面板只隐藏不关闭），攒下的录制标记随之写回脚本。
#[tauri::command]
#[specta::specta]
pub(crate) fn teleprompter_hide(app: tauri::AppHandle) {
    crate::hide_teleprompter(&app);
    flush_retake_marks(&app);
}

/// 跟读的增长快照间隔（毫秒，ADR-176 ④）：念到一行到它被带到阅读线的延迟 ≈ 间隔 + 单窗解码耗时。
/// 取 1 秒而不是更短：快照解码按次计成本（随包 tiny 档单次约 0.3 秒，真机 2026-11-04），0.6 秒一次
/// 时实时 RTF 压在 0.6 的降档线上下，一降到 Yellow 就跳掉全部快照窗（`recording::should_decode_window`）。
const FOLLOW_SNAPSHOT_INTERVAL_MS: u32 = 1000;

/// 跟读总开关（ADR-176 ④，PRD-20-005）：用户开、默认关。
pub(crate) const FOLLOW_ENABLED_KEY: &str = "teleprompter.follow_enabled";

fn follow_enabled(settings: &luna_config::Settings) -> bool {
    settings.get_bool(FOLLOW_ENABLED_KEY).unwrap_or(false)
}

// ── 提词器首页（ADR-176 ②，PRD-20-004）────────────────────────────────────────

/// 最近列表最多记几篇。
const RECENT_LIMIT: usize = 20;

/// 最近列表落在应用数据目录（只记在本机、不写库）：库根 → 库相对路径，新的在前。
fn recent_file(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("teleprompter").join("recent.json"))
}

fn read_recent(app: &tauri::AppHandle) -> BTreeMap<String, Vec<String>> {
    recent_file(app)
        .and_then(|file| std::fs::read(file).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 把 `path` 提到最前（去重、截到上限）。纯函数，便于测试。
fn bump_recent(list: &mut Vec<String>, path: &str) {
    list.retain(|item| item != path);
    list.insert(0, path.to_owned());
    list.truncate(RECENT_LIMIT);
}

fn vault_key(state: &AppState, label: &str) -> Option<String> {
    state
        .with_vault(label, |vault| {
            Ok(vault.root().to_string_lossy().into_owned())
        })
        .ok()
}

/// 记下这次提词的是哪一篇。写不成只记日志（最近列表是便利，不是事实）。
fn remember_recent(app: &tauri::AppHandle, state: &AppState, label: &str, path: &str) {
    let (Some(file), Some(root)) = (recent_file(app), vault_key(state, label)) else {
        return;
    };
    let mut all = read_recent(app);
    bump_recent(all.entry(root).or_default(), path);
    let written = serde_json::to_vec(&all)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&file, &bytes).map_err(|e| e.to_string())
        });
    if let Err(error) = written {
        eprintln!("[teleprompter] 最近列表写入失败：{error}");
    }
}

/// 最近提词过的一篇。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TeleprompterRecentDto {
    pub path: String,
    pub title: String,
}

/// 提词器首页要显示的东西。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TeleprompterHomeDto {
    /// 面板此刻打开的是哪一篇（没打开过为 `None`）。
    pub current: Option<String>,
    /// 最近提词过的稿子（新的在前；已不在库里的不列）。
    pub recent: Vec<TeleprompterRecentDto>,
    /// 新建的提词稿落在哪个目录（库相对）。
    pub draft_dir: String,
}

/// 提词器首页（ADR-176 ②）。
///
/// # Errors
/// 「提词器」没启用（`APP_DISABLED`）或库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_home(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<TeleprompterHomeDto, String> {
    super::apps::require_app(&app, &state, luna_config::apps::TELEPROMPTER_APP.id)?;
    let current = state
        .teleprompter_script
        .lock()
        .map_err(|_| "提词器状态锁中毒".to_owned())?
        .clone();
    let root = vault_key(&state, window.label()).unwrap_or_default();
    let listed = read_recent(&app).remove(&root).unwrap_or_default();
    let recent = state.with_vault(window.label(), |vault| {
        Ok(listed
            .into_iter()
            .filter(|path| vault.read(path).is_ok())
            .map(|path| TeleprompterRecentDto {
                title: note_stem_of(&path),
                path,
            })
            .collect::<Vec<_>>())
    })?;
    Ok(TeleprompterHomeDto {
        current,
        recent,
        draft_dir: draft_dir(&state.settings_snapshot()),
    })
}

fn draft_dir(settings: &luna_config::Settings) -> String {
    safe_vault_relative_dir(settings, "vault.teleprompter_dir", "Teleprompter")
}

/// 一段稿子按本人语速约念多少秒（含停顿，与脚本时长估算同一口径）：AI「按时长压缩」的结果旁边
/// 写明「约多少秒、比目标多 / 少多少」（ADR-176 ⑧，PRD-20-016：说清多出几秒，不替你删）。
///
/// # Errors
/// 库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_estimate_seconds(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<u32, String> {
    state.with_vault(window.label(), |vault| {
        Ok(luna_vault::script_outline(&text, vault.speaking_rate()).total_seconds)
    })
}

// ── 送稿（ADR-176 ⑨，PRD-20-020~023）──────────────────────────────────────────

/// 新建提词稿的正文从哪来。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeleprompterDraftSourceDto {
    /// 空稿（「新建提词稿」：建好后在编辑器里写）。
    Empty,
    /// 剪贴板里的文字（「从剪贴板新建」）。
    Clipboard,
    /// 界面交来的文字（首页 ⌘V：粘贴事件里的那段）。
    Text { text: String },
}

/// 新建一篇提词稿并用提词器打开，返回新稿路径。剪贴板或粘贴里没有文字时报错、不建空稿。
///
/// # Errors
/// 「提词器」没启用、没有文字（`TELEPROMPTER_CLIPBOARD_EMPTY`）、库未打开或写入失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn teleprompter_new_draft(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    source: TeleprompterDraftSourceDto,
) -> Result<String, String> {
    let empty = || {
        coded_error(
            &app,
            "TELEPROMPTER_CLIPBOARD_EMPTY",
            "teleprompter-clipboard-empty",
        )
    };
    let body = match source {
        TeleprompterDraftSourceDto::Empty => String::new(),
        TeleprompterDraftSourceDto::Clipboard => super::popup::read_clipboard_text()?
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(empty)?,
        TeleprompterDraftSourceDto::Text { text } => {
            if text.trim().is_empty() {
                return Err(empty());
            }
            text
        }
    };
    let path = create_draft_only(&app, &state, window.label(), &body)?;
    open_script(&app, &state, window.label(), &path, false)?;
    Ok(path)
}

/// 建一篇提词稿（不打开）：清洗正文，以第一行起名（没有可用的第一行时用时间），落在提词稿
/// 目录。返回新稿路径。送稿的各条入口都经这里（ADR-176 ⑨：外来文字一律先成为库里的笔记）。
fn create_draft_only(
    app: &tauri::AppHandle,
    state: &AppState,
    label: &str,
    raw: &str,
) -> Result<String, String> {
    super::apps::require_app(app, state, luna_config::apps::TELEPROMPTER_APP.id)?;
    let body = luna_vault::normalize_pasted_script(raw);
    let i18n = backend_copy_arc(app);
    let stem = luna_vault::script_title_of(&body)
        .map(|title| super::processing::safe_media_stem(&title))
        .unwrap_or_else(|| {
            let stamp = chrono::Local::now().format("%Y-%m-%d %H%M").to_string();
            i18n.as_deref()
                .and_then(|i18n| {
                    i18n.translate_args("teleprompter-draft-title", &[("time", stamp.clone())])
                })
                .unwrap_or_else(|| format!("Script {stamp}"))
        });
    let rel = format!("{}/{}.md", draft_dir(&state.settings_snapshot()), stem);
    let doc = state.with_vault(label, |vault| {
        super::notes::create_unique_note(i18n.as_deref(), vault, &rel, NoteType::Note, body.clone())
    })?;
    Ok(doc.path)
}

/// 提词器认得的文件（PRD-20-022）：纯文本与 Markdown、网页、Word、PDF，以及 macOS 上由系统转换
/// 的 .doc / .rtf / .odt。表格类不认——一格一格念不成稿。
fn prompter_format(path: &Path) -> Option<luna_docimport::DocFormat> {
    use luna_docimport::DocFormat as F;
    F::from_path(path).filter(|format| {
        matches!(
            format,
            F::Text | F::Markdown | F::Html | F::Docx | F::Pdf | F::Doc | F::Rtf | F::Odt
        ) && format.available_here()
    })
}

/// 转换时不要图片与页码：提词器只念文字。
struct PrompterConvertHost;

impl luna_docimport::ConvertHost for PrompterConvertHost {
    fn image_link(&mut self, _index: usize, _ext: &str) -> String {
        String::new()
    }
    fn page_marker(&mut self, _page: usize) -> String {
        String::new()
    }
}

/// 把一个文件转成要念的 Markdown。
fn convert_for_prompter(app: &tauri::AppHandle, path: &Path) -> Result<String, String> {
    use luna_docimport::DocImportError as E;
    let format = prompter_format(path).ok_or_else(|| {
        coded_error(
            app,
            "TELEPROMPTER_FILE_UNSUPPORTED",
            "teleprompter-file-unsupported",
        )
    })?;
    let (format, input) = if format.needs_system_html() {
        let html = super::doc_import::system_html(path, &luna_core::CancelToken::new())
            .map_err(|code| coded_error(app, code, "doc-import-corrupt"))?;
        (luna_docimport::DocFormat::Html, html)
    } else {
        let bytes = std::fs::read(path).map_err(|e| format!("FILE_READ_FAILED: {e}"))?;
        (format, bytes)
    };
    let converted =
        luna_docimport::convert(format, &input, &mut PrompterConvertHost).map_err(|e| {
            let (code, key) = match e {
                E::Empty => ("DOC_EMPTY", "doc-import-empty"),
                E::Encrypted => ("DOC_ENCRYPTED", "doc-import-encrypted"),
                E::NoTextLayer => ("DOC_NO_TEXT_LAYER", "doc-import-no-text-layer"),
                E::Corrupt(_) | E::NeedsSystemHtml => ("DOC_CORRUPT", "doc-import-corrupt"),
            };
            coded_error(app, code, key)
        })?;
    // 文件名比正文第一行更像这篇稿子的名字：正文没有一级标题时用文件名作标题。
    let markdown = converted.markdown;
    Ok(match path.file_stem().and_then(|s| s.to_str()) {
        Some(stem) if !markdown.trim_start().starts_with("# ") => format!("# {stem}\n\n{markdown}"),
        _ => markdown,
    })
}

/// 拖到首页或浮窗上的文件（PRD-20-022）：每个文件转成一篇提词稿，打开第一篇，返回各稿路径。
///
/// # Errors
/// 「提词器」没启用，或没有一个文件能转成提词稿时返回第一个文件的错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn teleprompter_import_files(
    app: tauri::AppHandle,
    window: tauri::Window,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager as _;
        let state = app.state::<AppState>();
        super::apps::require_app(&app, &state, luna_config::apps::TELEPROMPTER_APP.id)?;
        let mut created = Vec::new();
        let mut first_error = None;
        for path in paths {
            let made = convert_for_prompter(&app, Path::new(&path))
                .and_then(|markdown| create_draft_only(&app, &state, &label, &markdown));
            match made {
                Ok(rel) => created.push(rel),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match (created.first(), first_error) {
            (Some(first), _) => {
                open_script(&app, &state, &label, first, false)?;
                Ok(created)
            }
            (None, Some(error)) => Err(error),
            (None, None) => Ok(created),
        }
    })
    .await
    .map_err(|e| format!("TELEPROMPTER_TASK_PANIC: {e}"))?
}

/// 「送进提词器」快捷键与「服务」菜单（PRD-20-021 / 023）拿到文字后：建稿并打开；失败给通知
/// （参数不含正文）。`text = None` = 既没有选中的文字、剪贴板里也没有。须在主线程调用。
pub(crate) fn send_text_to_teleprompter(app: &tauri::AppHandle, text: Option<String>) {
    use crate::notifications::{NotificationCenter, NotificationEvent, NotificationTarget};
    use tauri::Manager as _;
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let enabled = luna_config::apps::is_enabled(
        &state.settings_snapshot(),
        luna_config::apps::TELEPROMPTER_APP.id,
    );
    let event = if enabled {
        match text.filter(|text| !text.trim().is_empty()) {
            None => Some(NotificationEvent::failure(
                "teleprompter_send_empty",
                NotificationTarget::Workspace,
            )),
            Some(text) => create_draft_only(app, &state, "main", &text)
                .and_then(|path| open_script(app, &state, "main", &path, false))
                .err()
                .map(|_| {
                    NotificationEvent::failure(
                        "teleprompter_send_failed",
                        NotificationTarget::Workspace,
                    )
                }),
        }
    } else {
        Some(NotificationEvent::failure(
            "teleprompter_app_missing",
            NotificationTarget::Route("apps://teleprompter".to_owned()),
        ))
    };
    if let (Some(event), Some(center)) = (event, app.try_state::<NotificationCenter>()) {
        center.publish(event, app);
    }
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
    // 跟读由用户开、默认关（ADR-176 ④）：关着时连麦克风权限都不去问。
    if !follow_enabled(&state.settings_snapshot()) {
        return Err(coded_error(
            &app,
            "TELEPROMPTER_FOLLOW_OFF",
            "teleprompter-follow-off",
        ));
    }
    let content = state.with_vault(window.label(), |vault| {
        vault.read(&path).map(|doc| doc.content).map_err(vault_err)
    })?;
    ensure_microphone_permission()?;
    // 跟读只用本机识别（ADR-176 ④）：即使实时转写设成了服务端，跟读的声音也不出本机。
    let asr = state.follow_stream_asr(&app)?;
    let settings = state.settings_snapshot();
    // 识别语言设成「自动」时按稿子的主语言提示：短窗里的自动语种检测常在中英之间乱跳。
    let lang_hint = settings
        .get_str("realtime.transcribe_lang")
        .filter(|s| !s.trim().is_empty() && s != "auto")
        .or_else(|| luna_vault::script_language(&content).map(str::to_owned))
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
            // 只记段号行号与时刻（日志不含转写文字）：真机核对标记落点、量跟读延迟时要知道
            // 高亮在什么时候到了哪里。
            let at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            eprintln!(
                "[teleprompter] 跟读位置 → 段 {} 行 {} 字 {} @{at_ms}",
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
        // 跟读只要最近十几个字、要的是快：增长快照从约 1.5 秒缩到 1 秒（ADR-176 ④）。
        tuning: luna_audio::CaptureTuning {
            stream_snapshot_interval_ms: FOLLOW_SNAPSHOT_INTERVAL_MS,
            ..capture_tuning(&settings)
        },
        vad: state.vad_factory(&app),
        lang_hint,
        input_device,
        // 只听麦克风：念稿的人在麦克风这一侧，系统声音里没有要跟的东西。
        capture_source: crate::recording::RecordingCaptureSource::Mic,
        transcribe_enabled: true,
        translate_enabled: false,
        translate_config,
        glossary,
        asr_provider: Some(asr),
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

/// 跟读时从某一行接着跟（ADR-176 ⑥，PRD-20-013）：点了浮窗上的一行，或按键换行换段。不在
/// 跟读时什么都不做（没跟读时位置只是浮窗的滚动，归浮窗）。
#[tauri::command]
#[specta::specta]
pub(crate) fn teleprompter_follow_seek(state: tauri::State<'_, AppState>, at: PrompterAtDto) {
    if let Ok(follow) = state.teleprompter_follow.lock() {
        if let Some(aligner) = follow.as_ref() {
            if let Ok(mut aligner) = aligner.lock() {
                aligner.seek(at.block, at.line);
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
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
            luna_vault::SpeakingRate::DEFAULT,
            true,
        );
        assert!(dto.extended);
        assert_eq!(dto.title, "第一期-脚本");
        assert_eq!(dto.blocks.len(), 2);
        assert_eq!(dto.blocks[0].heading.as_deref(), Some("开头"));
        assert_eq!(
            dto.blocks[0].lines,
            [PrompterLineDto {
                text: "你好。".into(),
                marker: Some("口播".into()),
                // 「你好」两个字，默认 240 字 / 分 → 500 毫秒（与脚本时长同一口径）。
                millis: 500,
                spans: vec![PrompterSpanDto::Text {
                    text: "你好。".into()
                }],
                cue: false,
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
            prompter_dto("P.md", script, luna_vault::SpeakingRate::DEFAULT, false).blocks[1].lines
                [0]
            .text,
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

#[cfg(test)]
mod recent_tests {
    use super::*;

    #[test]
    fn bumping_moves_to_front_dedups_and_caps() {
        let mut list: Vec<String> = (0..RECENT_LIMIT).map(|i| format!("N/{i}.md")).collect();
        bump_recent(&mut list, "N/5.md");
        assert_eq!(list[0], "N/5.md");
        assert_eq!(list.len(), RECENT_LIMIT);
        assert_eq!(list.iter().filter(|p| *p == "N/5.md").count(), 1);
        bump_recent(&mut list, "N/new.md");
        assert_eq!(list[0], "N/new.md");
        assert_eq!(list.len(), RECENT_LIMIT, "超出上限的最老一篇被挤掉");
    }

    #[test]
    fn follow_is_off_by_default() {
        assert!(!follow_enabled(&luna_config::Settings::default()));
    }
}

#[cfg(test)]
mod cue_tests {
    use super::*;

    #[test]
    fn pauses_add_to_a_line_and_cue_lines_are_not_read() {
        let dto = prompter_dto(
            "P.md",
            "## 开头\n你好 // 你好\n> [!cue] 看镜头\n",
            luna_vault::SpeakingRate::DEFAULT,
            true,
        );
        let lines = &dto.blocks[0].lines;
        // 「你好你好」四个字 1000 毫秒 + 一个停顿 600 毫秒。
        assert_eq!(lines[0].millis, 1600);
        assert_eq!(lines[0].text, "你好 你好");
        assert!(lines[1].cue);
        assert_eq!(lines[1].millis, 0);
        assert_eq!(
            lines[1].spans,
            [PrompterSpanDto::Text {
                text: "看镜头".into()
            }]
        );
    }
}
