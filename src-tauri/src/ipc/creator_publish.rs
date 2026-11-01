//! 发布包（ADR-162 ⑪，PRD-19-018~020）：成片候选、字幕一次出齐（原文 / 译文 / 双语，可选 BCC）。
//!
//! 写哪几份、叫什么名字是这里的纯函数（[`subtitle_plan`]，可单测）；重切分、简繁转换与格式
//! 渲染全在 `luna-export`（同一条导出管线，PRD-11）。前端只发命令、渲染结果（ADR-003）。

use super::*;
use luna_core::chapters::{
    chapter_source, check_chapters, fix_chapters, format_chapters, parse_chapters, parse_timestamp,
    snap_to_nearest, Chapter, ChapterIssue,
};
use luna_core::{JobEvent, JobKind, JobOrigin, ResultPayload, SummaryStyle, TextRef};
use luna_export::{Bilingual, ExportFormat, ExportOptions, ExportRegistry, Exportable};
use luna_vault::channel::{Channel, ChannelFormat, ChannelLimits};

/// 发布包的一篇成片候选（项目文件夹里已转写的录音 / 转写笔记）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PublishSourceDto {
    /// 笔记的库相对路径。
    pub path: String,
    /// 标题（文件名）。
    pub title: String,
    /// 修改时间（Unix 毫秒；TS 侧是 number）。
    pub modified_ms: f64,
    /// 媒体总时长（秒）；元数据里没有为 `None`。
    pub duration_secs: Option<f64>,
    /// 是否已有译文（没有时只能导出原文，界面给「先翻译」）。
    pub has_translation: bool,
}

impl From<luna_vault::PublishSource> for PublishSourceDto {
    fn from(source: luna_vault::PublishSource) -> Self {
        Self {
            path: source.path,
            title: source.title,
            #[allow(clippy::cast_precision_loss)]
            modified_ms: source.modified_ms as f64,
            duration_secs: source.duration_secs,
            has_translation: source.has_translation,
        }
    }
}

/// 发布包字幕的主格式（BCC 是额外勾选的一份，不在这里）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PublishSubtitleFormat {
    /// SubRip（YouTube 与多数剪辑软件都收）。
    Srt,
    /// WebVTT。
    Vtt,
}

impl From<PublishSubtitleFormat> for ExportFormat {
    fn from(format: PublishSubtitleFormat) -> Self {
        match format {
            PublishSubtitleFormat::Srt => Self::Srt,
            PublishSubtitleFormat::Vtt => Self::Vtt,
        }
    }
}

/// 一次导出的结果：写进了哪个文件夹、哪几份文件（都是库相对路径）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PublishSubtitlesDto {
    pub folder: String,
    pub files: Vec<String>,
}

/// 计划写出的一份字幕。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedSubtitle {
    /// 文件名（不含扩展名）。
    stem: String,
    format: ExportFormat,
    content: Bilingual,
}

/// 文件名里的语言码：中文按选定的字形给出 `zh-Hans` / `zh-Hant-TW` / `zh-Hant-HK`，
/// 别的语言原样；不知道是什么语言时为 `None`。
fn lang_tag(lang: Option<&str>, han: luna_export::HanVariant) -> Option<String> {
    let lang = lang.map(str::trim).filter(|l| !l.is_empty())?;
    let is_chinese = lang == "zh" || lang.starts_with("zh-") || lang.starts_with("zh_");
    if !is_chinese {
        return Some(lang.to_owned());
    }
    Some(
        match han {
            luna_export::HanVariant::Keep => lang,
            luna_export::HanVariant::Simplified => "zh-Hans",
            luna_export::HanVariant::TraditionalTaiwan => "zh-Hant-TW",
            luna_export::HanVariant::TraditionalHongKong => "zh-Hant-HK",
        }
        .to_owned(),
    )
}

/// 写哪几份、叫什么名字（ADR-162 ⑪）：原文一份；有译文再加译文与双语；勾了 BCC 每份再出一个
/// `.bcc`。文件名 `<项目>.<语言码>`，双语 `<项目>.<源>-<译>`；语言不明时退回内容名
/// （`source` / `translation` / `bilingual`，与单篇导出的命名同一套）。
fn subtitle_plan(
    project: &str,
    source_lang: Option<&str>,
    target_lang: Option<&str>,
    has_translation: bool,
    han: luna_export::HanVariant,
    format: ExportFormat,
    bcc: bool,
) -> Vec<PlannedSubtitle> {
    let source = lang_tag(source_lang, han);
    let target = lang_tag(target_lang, han);
    let mut contents = vec![(
        Bilingual::SourceOnly,
        source.clone().unwrap_or_else(|| "source".to_owned()),
    )];
    if has_translation {
        contents.push((
            Bilingual::TargetOnly,
            target.clone().unwrap_or_else(|| "translation".to_owned()),
        ));
        contents.push((
            Bilingual::Both,
            match (&source, &target) {
                (Some(source), Some(target)) => format!("{source}-{target}"),
                _ => "bilingual".to_owned(),
            },
        ));
    }
    let mut formats = vec![format];
    if bcc {
        formats.push(ExportFormat::Bcc);
    }
    formats
        .into_iter()
        .flat_map(|format| {
            contents
                .iter()
                .map(move |(content, label)| PlannedSubtitle {
                    stem: format!("{project}.{label}"),
                    format,
                    content: *content,
                })
        })
        .collect()
}

/// 用一段样本文字猜语言（只认可靠的结果）。
fn detect_lang(
    detector: &dyn LanguageDetector,
    texts: impl Iterator<Item = String>,
) -> Option<String> {
    let sample: String = texts.take(40).collect::<Vec<_>>().join("\n");
    detector
        .detect(&sample)
        .reliable_lang()
        .map(|lang| lang.as_str().to_owned())
}

/// 项目主页所在的文件夹（库相对路径；主页在库根时为空串）。
pub(super) fn project_folder(home: &str) -> &str {
    home.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// 一个项目的成片候选（PRD-19-018）：项目文件夹里已转写的录音 / 转写笔记，新的在前。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_publish_sources(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<Vec<PublishSourceDto>, String> {
    state.with_vault(window.label(), |vault| {
        Ok(vault
            .project_publish_sources(&home)
            .map_err(vault_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    })
}

/// 在发布包页导入成片（ADR-162 ⑪）：选好的音视频文件走与「转写」页同一条导入与转写管线，
/// 但转写笔记直接建进这个项目的文件夹——文件夹由 Rust 从项目主页推出，不收前端传来的路径。
/// 返回入队件数（没有认识的媒体为 0）。
///
/// # Errors
/// `home` 不是视频项目主页、批量转写不可用时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_import_final_cut(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    paths: Vec<String>,
) -> Result<u32, String> {
    let folder = project_folder(&home).to_owned();
    let is_project = state.with_vault(window.label(), |vault| {
        Ok(vault
            .read(&home)
            .is_ok_and(|doc| doc.meta.note_type == NoteType::VideoProject))
    })?;
    if folder.is_empty() || !is_project {
        return Err(coded_error(
            &app,
            "CREATOR_NOT_A_PROJECT",
            "creator-not-a-project",
        ));
    }
    super::transcribe::enqueue_media_imports(&app, &state, paths, Some(folder), None)
}

/// 发布包字幕一次出齐（PRD-19-018~020）：原文、译文、双语三份（没有译文就只有原文），可选每份
/// 再出一个 BCC；写进项目文件夹里的「发布」子文件夹，同名文件追加序号、不覆盖。
///
/// # Errors
/// 库未打开、成片不在这个项目里、没有转写段或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn creator_export_subtitles(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    source: String,
    format: PublishSubtitleFormat,
    bcc: bool,
    han_variant: HanVariantDto,
) -> Result<PublishSubtitlesDto, String> {
    let i18n = backend_copy_arc(&app);
    let folder_name = backend_text_with(i18n.as_deref(), "creator-publish-folder", "发布");
    let utf8_bom = state
        .settings_snapshot()
        .get_bool("export.utf8_bom")
        .unwrap_or(false);
    let han: luna_export::HanVariant = han_variant.into();
    let detector = Arc::clone(&state.detector);
    let folder = project_folder(&home).to_owned();
    let project = note_stem_of(&home);
    let dir_rel = if folder.is_empty() {
        folder_name
    } else {
        format!("{folder}/{folder_name}")
    };
    let files = state.with_vault(window.label(), |vault| {
        // 成片必须在这个项目文件夹里（界面过期时不往别的项目里写）。
        if !folder.is_empty() && !source.starts_with(&format!("{folder}/")) {
            return Err(coded_error(
                &app,
                "CREATOR_PUBLISH_SOURCE_OUTSIDE",
                "creator-publish-source-outside",
            ));
        }
        let doc = vault.read(&source).map_err(vault_err)?;
        let segments = luna_vault::note_segments(vault.root(), &doc.meta);
        if segments.is_empty() {
            return Err(coded_error(
                &app,
                "EXPORT_NO_TRANSCRIPT",
                "export-no-transcript-single",
            ));
        }
        let has_translation = segments.iter().any(|s| {
            s.translation
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty())
        });
        let source_lang = doc
            .meta
            .media
            .iter()
            .find_map(|m| m.lang.clone())
            .or_else(|| detect_lang(detector.as_ref(), segments.iter().map(|s| s.text.clone())));
        let target_lang = if has_translation {
            detect_lang(
                detector.as_ref(),
                segments.iter().filter_map(|s| s.translation.clone()),
            )
        } else {
            None
        };
        let plan = subtitle_plan(
            &project,
            source_lang.as_deref(),
            target_lang.as_deref(),
            has_translation,
            han,
            format.into(),
            bcc,
        );
        let dir = vault.root().join(&dir_rel);
        std::fs::create_dir_all(&dir).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(segments);
        let mut files = Vec::with_capacity(plan.len());
        for file in plan {
            let options = ExportOptions {
                bilingual: file.content,
                utf8_bom,
                han_variant: han,
                ..ExportOptions::default()
            };
            let text = registry
                .export(file.format, &content, &options)
                .map_err(|e| format!("EXPORT_FAILED: {e}"))?;
            let dest = unique_export_path(&dir, &file.stem, file.format.extension());
            std::fs::write(&dest, text).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
            let name = dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push(format!("{dir_rel}/{name}"));
        }
        Ok(files)
    })?;
    emit_vault_changes_in_scope(
        &app,
        files.iter().map(|path| (path.clone(), false)),
        VaultChangeScope::for_window(window.label()),
    );
    Ok(PublishSubtitlesDto {
        folder: dir_rel,
        files,
    })
}

// ── 章节（C3b，PRD-19-021）────────────────────────────────────────────────────

/// 章节生成 Job 的输出预算（十来行 JSON；给宽是为了不被截断）。
const CHAPTER_OUTPUT_TOKENS: u32 = 1024;
/// 章节标题上限（字符）：一行短标题，不是段落。
const CHAPTER_TITLE_MAX_CHARS: usize = 80;

/// 一处不合规（章号、行号都从 1 起，界面直接显示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChapterIssueDto {
    Unparsed { line: u32 },
    FirstNotAtZero,
    TooFew { count: u32 },
    TooShort { chapter: u32, seconds: u32 },
    NotIncreasing { chapter: u32 },
    EmptyTitle { chapter: u32 },
    PastEnd { chapter: u32 },
}

fn ordinal(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

impl From<ChapterIssue> for ChapterIssueDto {
    fn from(issue: ChapterIssue) -> Self {
        match issue {
            ChapterIssue::Unparsed { line } => Self::Unparsed {
                line: u32::try_from(line).unwrap_or(u32::MAX),
            },
            ChapterIssue::FirstNotAtZero => Self::FirstNotAtZero,
            ChapterIssue::TooFew { count } => Self::TooFew {
                count: u32::try_from(count).unwrap_or(u32::MAX),
            },
            ChapterIssue::TooShort { index, seconds } => Self::TooShort {
                chapter: ordinal(index),
                seconds: u32::try_from(seconds).unwrap_or(u32::MAX),
            },
            ChapterIssue::NotIncreasing { index } => Self::NotIncreasing {
                chapter: ordinal(index),
            },
            ChapterIssue::EmptyTitle { index } => Self::EmptyTitle {
                chapter: ordinal(index),
            },
            ChapterIssue::PastEnd { index } => Self::PastEnd {
                chapter: ordinal(index),
            },
        }
    }
}

/// 一段章节文字的检查结果：哪里不合规；能自动修时给出修好的文字。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct ChaptersCheckDto {
    pub issues: Vec<ChapterIssueDto>,
    /// 按规则修正后的文字；已经合规、或修了也不会变时为 `None`。
    pub fixed: Option<String>,
}

/// 检查一段章节文字（纯函数，界面边改边调）。
fn check_text(text: &str, duration_ms: Option<u64>) -> ChaptersCheckDto {
    let (chapters, unparsed) = parse_chapters(text);
    let mut issues: Vec<ChapterIssueDto> = unparsed
        .into_iter()
        .map(|line| ChapterIssue::Unparsed { line }.into())
        .collect();
    issues.extend(
        check_chapters(&chapters, duration_ms)
            .into_iter()
            .map(Into::into),
    );
    let fixed = if issues.is_empty() || chapters.is_empty() {
        None
    } else {
        let fixed = format_chapters(&fix_chapters(chapters, duration_ms));
        (fixed.trim() != text.trim()).then_some(fixed)
    };
    ChaptersCheckDto { issues, fixed }
}

/// 模型回的章节（ResultSchema）：只收这一种形状。
#[derive(Debug, serde::Deserialize)]
struct ModelChapters {
    #[serde(default)]
    chapters: Vec<ModelChapter>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelChapter {
    start: serde_json::Value,
    #[serde(default)]
    title: String,
}

/// 读模型输出：截出 JSON、读时间（`"1:05"` 或秒数）、标题只取一行并截短、时间吸附到最近的
/// 句子起点（`starts` 升序）。读不出或一章都没有为 `None`。
fn parse_model_chapters(raw: &str, starts: &[u64]) -> Option<Vec<Chapter>> {
    let begin = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= begin {
        return None;
    }
    let parsed: ModelChapters = serde_json::from_str(&raw[begin..=end]).ok()?;
    let chapters: Vec<Chapter> = parsed
        .chapters
        .into_iter()
        .filter_map(|chapter| {
            let start_ms = match &chapter.start {
                serde_json::Value::String(text) => parse_timestamp(text)?,
                serde_json::Value::Number(n) => {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let ms = (n.as_f64()?.max(0.0) * 1000.0) as u64;
                    ms
                }
                _ => return None,
            };
            let title: String = chapter
                .title
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .chars()
                .take(CHAPTER_TITLE_MAX_CHARS)
                .collect();
            Some(Chapter {
                start_ms: snap_to_nearest(starts, start_ms),
                title,
            })
        })
        .collect();
    (!chapters.is_empty()).then_some(chapters)
}

/// AI 给出的章节草稿：可直接改的文字 + 它的检查结果 + 成片时长（之后边改边检查要用）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ChaptersDraftDto {
    pub text: String,
    pub check: ChaptersCheckDto,
    /// 成片时长（毫秒）；不知道为 `None`。
    pub duration_ms: Option<f64>,
}

/// 检查一段章节文字（PRD-19-021）：按 YouTube 规则逐条说清哪里不合规，能修的给修好的文字。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_check_chapters(text: String, duration_ms: Option<f64>) -> ChaptersCheckDto {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let duration = duration_ms.filter(|d| *d > 0.0).map(|d| d as u64);
    check_text(&text, duration)
}

/// 成片的转写：按时间排的（起点毫秒，文字）行与成片时长（生成章节、简介共用）。
pub(super) struct SourceTranscript {
    pub(super) lines: Vec<(u64, String)>,
    pub(super) duration_ms: Option<u64>,
}

/// 读项目里一篇成片的转写段（成片必须在这个项目文件夹里）。没有转写段时返回错误。
pub(super) fn load_source_transcript(
    app: &tauri::AppHandle,
    window_label: &str,
    home: &str,
    source: &str,
) -> Result<SourceTranscript, String> {
    let state = app.state::<AppState>();
    let folder = project_folder(home).to_owned();
    let (segments, media_duration_ms) = state.with_vault(window_label, |vault| {
        if !folder.is_empty() && !source.starts_with(&format!("{folder}/")) {
            return Err(coded_error(
                app,
                "CREATOR_PUBLISH_SOURCE_OUTSIDE",
                "creator-publish-source-outside",
            ));
        }
        let doc = vault.read(source).map_err(vault_err)?;
        let durations: Vec<f64> = doc.meta.media.iter().filter_map(|m| m.duration).collect();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let duration =
            (!durations.is_empty()).then(|| (durations.iter().sum::<f64>() * 1000.0) as u64);
        Ok((luna_vault::note_segments(vault.root(), &doc.meta), duration))
    })?;
    if segments.is_empty() {
        return Err(coded_error(
            app,
            "EXPORT_NO_TRANSCRIPT",
            "export-no-transcript-single",
        ));
    }
    let to_ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    Ok(SourceTranscript {
        duration_ms: media_duration_ms.or_else(|| segments.iter().map(|s| to_ms(s.end)).max()),
        lines: segments
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| (to_ms(s.start), s.text.clone()))
            .collect(),
    })
}

/// 用成片转写跑一次创作空间的 AI 动作（章节、渠道包装、分发）：转写按上下文窗截取；提示词带
/// 变量（渠道名、上限、风格），指令之后带上创作者写的话（[`CreatorNotes`]）。
pub(super) async fn run_publish_job_with(
    app: &tauri::AppHandle,
    key: &str,
    vars: &[(&str, &str)],
    notes: &CreatorNotes,
    lines: &[(u64, String)],
    output_tokens: u32,
) -> Result<String, String> {
    super::chat::ensure_chat_context_window(app).await;
    let limit = floating_input_limit(app.state::<AppState>().chat_ctx_window());
    run_summarize_job_with(
        app,
        key,
        vars,
        notes,
        chapter_source(lines, limit),
        output_tokens,
    )
    .await
}

/// 设置「我的频道风格」的键（ADR-166 ⑤，PRD-19-042；随库）。
pub(crate) const CHANNEL_VOICE_SETTING: &str = "creator.channel_voice";
/// 频道风格最多带多少字（一段话；再长就是在写第二份提示词了）。
const VOICE_MAX_CHARS: usize = 1000;
/// 「这次的额外要求」最多带多少字。
const EXTRA_MAX_CHARS: usize = 500;

/// 创作者写给模型的话（ADR-166 ⑤，PRD-19-042）：设置里的「我的频道风格」与这一次生成的「这次的
/// 额外要求」。作为**内容**拼在指令之后（`creator.notes.voice` / `creator.notes.extra` 片段，
/// 单趟渲染），不改模板结构——输出形状与评测都不受影响；各自截到上限，空的不带。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CreatorNotes {
    voice: String,
    extra: String,
}

impl CreatorNotes {
    pub(crate) fn new(settings: &luna_config::Settings, extra: Option<&str>) -> Self {
        let clip = |text: &str, max: usize| {
            text.trim()
                .chars()
                .take(max)
                .collect::<String>()
                .trim()
                .to_owned()
        };
        let voice = settings
            .get(CHANNEL_VOICE_SETTING)
            .and_then(|value| value.as_str().map(|text| clip(text, VOICE_MAX_CHARS)))
            .unwrap_or_default();
        Self {
            voice,
            extra: clip(extra.unwrap_or_default(), EXTRA_MAX_CHARS),
        }
    }

    /// 读当前设置（频道风格）+ 这一次的额外要求。
    pub(crate) fn of(app: &tauri::AppHandle, extra: Option<&str>) -> Self {
        Self::new(&app.state::<AppState>().settings_snapshot(), extra)
    }

    pub(crate) fn apply(&self, instruction: &mut luna_core::prompt::Instruction) {
        let registry = luna_core::prompt::registry();
        for (key, text) in [
            ("creator.notes.voice", &self.voice),
            ("creator.notes.extra", &self.extra),
        ] {
            if !text.is_empty() {
                registry.append(
                    instruction,
                    luna_core::PromptScope::Summarize,
                    key,
                    &[("text", text)],
                );
            }
        }
    }
}

/// 跑一次创作空间的 AI 动作（单一 AI 主干：`Summarize` + 注册表提示词，ADR-001）：交互泳道、
/// 严格本地时只走本机；回的是文字，形状由调用方按各自的 ResultSchema 解析。
pub(super) async fn run_summarize_job(
    app: &tauri::AppHandle,
    key: &str,
    text: String,
    output_tokens: u32,
) -> Result<String, String> {
    run_summarize_job_with(app, key, &[], &CreatorNotes::default(), text, output_tokens).await
}

/// 同 [`run_summarize_job`]，提示词带变量。
pub(super) async fn run_summarize_job_with(
    app: &tauri::AppHandle,
    key: &str,
    vars: &[(&str, &str)],
    notes: &CreatorNotes,
    text: String,
    output_tokens: u32,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let mut instruction =
        luna_core::prompt::registry().instruction(luna_core::PromptScope::Summarize, key, vars);
    notes.apply(&mut instruction);
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(text),
            style: SummaryStyle::Faithful,
            instruction: Some(instruction),
        },
        // 用户点了按钮在等结果：走交互泳道。
        JobOrigin::FloatingMenu,
    );
    job.routing.local_only = state.strict_local();
    job.options.output_budget = Some(output_tokens);
    let mut handle = state.queue.submit(job);
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Text(text) => Ok(super::distill::normalize_model_output(&text)),
                    _ => Err(coded_error(
                        app,
                        "CREATOR_PUBLISH_PARSE",
                        "creator-publish-parse",
                    )),
                };
            }
            JobEvent::Failed(err) => {
                eprintln!("[creator-publish] {key} failed code={}", err.code());
                return Err(err.code().to_owned());
            }
            JobEvent::Cancelled => return Err("CANCELLED".to_owned()),
            _ => {}
        }
    }
    Err(coded_error(
        app,
        "CREATOR_PUBLISH_PARSE",
        "creator-publish-parse",
    ))
}

/// 由 AI 从成片转写里分章节（PRD-19-021，ADR-162 ⑪）：走单一 AI 主干（`Summarize` + 只回
/// JSON 的注册表指令），时间吸附到句子起点，再按规则修正一遍。
///
/// # Errors
/// 库未打开、成片不在项目里、没有转写段、模型不可用或回得不像章节时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_generate_chapters(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
    source: String,
    extra: Option<String>,
) -> Result<ChaptersDraftDto, String> {
    let transcript = load_source_transcript(&app, window.label(), &home, &source)?;
    let starts: Vec<u64> = transcript.lines.iter().map(|(start, _)| *start).collect();
    let duration_ms = transcript.duration_ms;
    let output = run_publish_job_with(
        &app,
        "creator.publish.chapters",
        &[],
        &CreatorNotes::of(&app, extra.as_deref()),
        &transcript.lines,
        CHAPTER_OUTPUT_TOKENS,
    )
    .await?;
    let chapters = parse_model_chapters(&output, &starts)
        .ok_or_else(|| coded_error(&app, "CREATOR_CHAPTERS_PARSE", "creator-chapters-parse"))?;
    let text = format_chapters(&fix_chapters(chapters, duration_ms));
    #[allow(clippy::cast_precision_loss)]
    Ok(ChaptersDraftDto {
        check: check_text(&text, duration_ms),
        text,
        duration_ms: duration_ms.map(|d| d as f64),
    })
}

/// 把章节写进项目的发布笔记（`<项目>-发布.md` 的「章节」一节；已有就替换，没有这篇就新建），
/// 返回这篇笔记的库相对路径。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_save_chapters(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    text: String,
) -> Result<String, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let heading = backend_text_with(copy, "creator-publish-chapters", "章节");
    let note_rel = publish_note_rel(&home, copy);
    let (content, created) = state.with_vault(window.label(), |vault| {
        let (existing, created) = match vault.read(&note_rel) {
            Ok(doc) => (doc.content, false),
            Err(_) => (String::new(), true),
        };
        let content = luna_vault::upsert_section(&existing, &heading, &text);
        vault.write(&note_rel, &content).map_err(vault_err)?;
        Ok((content, created))
    })?;
    broadcast_note_write(&app, window.label(), &note_rel, content);
    if created {
        emit_vault_changed_in_scope(
            &app,
            &note_rel,
            false,
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(note_rel)
}

/// 项目的发布笔记：`<项目文件夹>/<项目>-<发布>.md`（「发布」随界面语言，与字幕文件夹同一个词）。
pub(crate) fn publish_note_rel(home: &str, copy: BackendCopy<'_>) -> String {
    let project = note_stem_of(home);
    let suffix = backend_text_with(copy, "creator-publish-folder", "发布");
    match project_folder(home) {
        "" => format!("{project}-{suffix}.md"),
        folder => format!("{folder}/{project}-{suffix}.md"),
    }
}

// ── 渠道包装（ADR-166 ④，PRD-19-022 / 023 / 041）───────────────────────────────

/// 渠道包装 Job 的输出预算（三个标题 + 简介 + 封面字 + 分类 + 标签的 JSON）。
const PACKAGE_OUTPUT_TOKENS: u32 = 2048;
/// 标题备选个数（YouTube「测试与比较」一次最多三个）。
const TITLE_COUNT: usize = 3;
/// 封面字的可读性建议（不是平台上限）：最多两行、每行约 12 个汉字宽（西文按半个字算）。
const COVER_LINE_COUNT_MAX: usize = 2;
const COVER_LINE_WIDTH_MAX: usize = 24;

fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 标题一行一个（空行不算；去掉列表符号）。
fn title_lines(titles: &str) -> Vec<&str> {
    titles
        .lines()
        .map(|line| line.trim().trim_start_matches(['-', '*', '•']).trim())
        .filter(|line| !line.is_empty())
        .collect()
}

/// 标签一行一个；去掉行首的 `#`（平台自己会加）。
fn tag_lines(tags: &str) -> Vec<&str> {
    title_lines(tags)
        .into_iter()
        .map(|tag| tag.trim_start_matches('#').trim())
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// 一行封面字的显示宽度（半宽单位）：汉字等全宽字 2，其余 1。
fn cover_width(line: &str) -> usize {
    line.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 一个渠道的包装：五块都是可直接改的文字（标题、封面字、标签一行一项）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PackageDto {
    pub titles: String,
    pub description: String,
    pub cover: String,
    pub category: String,
    pub tags: String,
}

impl PackageDto {
    fn is_empty(&self) -> bool {
        [
            &self.titles,
            &self.description,
            &self.cover,
            &self.category,
            &self.tags,
        ]
        .iter()
        .all(|text| text.trim().is_empty())
    }
}

/// 渠道包装的一处超限或缺项（序号从 1 起；`max` 是这个渠道的上限）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageIssueDto {
    /// 标题不是三个。
    TitleCount {
        count: u32,
    },
    TitleTooLong {
        title: u32,
        over: u32,
        max: u32,
    },
    DescriptionTooLong {
        over: u32,
        max: u32,
    },
    TooManyTags {
        count: u32,
        max: u32,
    },
    TagTooLong {
        tag: u32,
        over: u32,
        max: u32,
    },
    /// 封面字超过两行（可读性建议）。
    TooManyCoverLines {
        count: u32,
    },
    /// 某行封面字太长（可读性建议，按汉字宽度算）。
    CoverLineTooLong {
        line: u32,
    },
}

/// 检查一个渠道的包装（纯函数；不替用户截断，只说清是哪一条、多出多少）。只按这个渠道
/// **有的**上限查——没核对过的上限不查（ADR-166 ①）。分类只是建议文字，不查。
fn check_package(package: &PackageDto, limits: &ChannelLimits) -> Vec<PackageIssueDto> {
    let mut issues = Vec::new();
    let titles = title_lines(&package.titles);
    if !titles.is_empty() && titles.len() != TITLE_COUNT {
        issues.push(PackageIssueDto::TitleCount {
            count: count_u32(titles.len()),
        });
    }
    if let Some(max) = limits.title_max {
        for (index, title) in titles.iter().enumerate() {
            let len = count_u32(title.chars().count());
            if len > max {
                issues.push(PackageIssueDto::TitleTooLong {
                    title: ordinal(index),
                    over: len - max,
                    max,
                });
            }
        }
    }
    if let Some(max) = limits.body_max {
        let len = count_u32(package.description.chars().count());
        if len > max {
            issues.push(PackageIssueDto::DescriptionTooLong {
                over: len - max,
                max,
            });
        }
    }
    let cover = title_lines(&package.cover);
    if cover.len() > COVER_LINE_COUNT_MAX {
        issues.push(PackageIssueDto::TooManyCoverLines {
            count: count_u32(cover.len()),
        });
    }
    for (index, line) in cover.iter().enumerate() {
        if cover_width(line) > COVER_LINE_WIDTH_MAX {
            issues.push(PackageIssueDto::CoverLineTooLong {
                line: ordinal(index),
            });
        }
    }
    let tags = tag_lines(&package.tags);
    if let Some(max) = limits.tags_max {
        if count_u32(tags.len()) > max {
            issues.push(PackageIssueDto::TooManyTags {
                count: count_u32(tags.len()),
                max,
            });
        }
    }
    if let Some(max) = limits.tag_max {
        for (index, tag) in tags.iter().enumerate() {
            let len = count_u32(tag.chars().count());
            if len > max {
                issues.push(PackageIssueDto::TagTooLong {
                    tag: ordinal(index),
                    over: len - max,
                    max,
                });
            }
        }
    }
    issues
}

/// 研究笔记「来源」一节 → 简介里的来源行：只取列表项，Markdown 链接摊成「文字 地址」，
/// `[[双链]]` 只留文字（简介是纯文本，平台不渲染 Markdown）。
fn source_lines(section: &str) -> Vec<String> {
    section
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .map(str::trim)
        })
        .map(plain_link_text)
        .filter(|line| !line.is_empty())
        .collect()
}

/// `[文字](地址)` → `文字 地址`；`[[笔记|别名]]` → `别名`（无别名留笔记名）。
fn plain_link_text(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        if let Some(inner) = after.strip_prefix("[[") {
            if let Some(close) = inner.find("]]") {
                let target = &inner[..close];
                out.push_str(target.rsplit('|').next().unwrap_or(target));
                rest = &inner[close + 2..];
                continue;
            }
        } else if let Some(close) = after.find("](") {
            if let Some(end) = after[close + 2..].find(')') {
                let text = &after[1..close];
                let url = &after[close + 2..close + 2 + end];
                out.push_str(text);
                out.push(' ');
                out.push_str(url);
                rest = &after[close + 3 + end..];
                continue;
            }
        }
        out.push('[');
        rest = &after[1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// 简介里的固定几块（标题文字随界面语言）。
struct CopyBlocks<'a> {
    chapters: Option<&'a str>,
    sources: &'a [String],
    links: Option<&'a str>,
    chapters_heading: &'a str,
    sources_heading: &'a str,
    links_heading: &'a str,
}

/// 拼一版简介：正文 → 章节 → 参考来源 → 链接；没有的块省掉（ADR-162 ⑪）。
fn assemble_description(summary: &str, blocks: &CopyBlocks<'_>) -> String {
    let mut parts = vec![summary.trim().to_owned()];
    if let Some(chapters) = blocks.chapters.map(str::trim).filter(|c| !c.is_empty()) {
        parts.push(format!("{}\n{chapters}", blocks.chapters_heading));
    }
    if !blocks.sources.is_empty() {
        parts.push(format!(
            "{}\n{}",
            blocks.sources_heading,
            blocks.sources.join("\n")
        ));
    }
    if let Some(links) = blocks.links.map(str::trim).filter(|l| !l.is_empty()) {
        parts.push(format!("{}\n{links}", blocks.links_heading));
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n\n")
}

/// 发布笔记里各节、各小节的标题（随界面语言）。
struct PublishHeadings {
    chapters: String,
    links: String,
    sources: String,
    titles: String,
    description: String,
    cover: String,
    category: String,
    tags: String,
}

fn publish_headings(copy: BackendCopy<'_>) -> PublishHeadings {
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    PublishHeadings {
        chapters: text("creator-publish-chapters", "章节"),
        links: text("creator-publish-links", "链接"),
        sources: text("creator-publish-sources", "参考来源"),
        titles: text("creator-publish-titles", "标题备选"),
        description: text("creator-publish-description", "简介"),
        cover: text("creator-publish-cover", "封面文字"),
        category: text("creator-publish-category", "分区"),
        tags: text("creator-publish-tags", "标签"),
    }
}

/// 一个渠道给人看的名字：内置按界面语言（`creator-channel-<id>`），自定义用用户起的名字。
pub(super) fn channel_label(channel: &Channel, copy: BackendCopy<'_>) -> String {
    match &channel.name {
        Some(name) => name.clone(),
        None => backend_text_with(
            copy,
            &format!("creator-channel-{}", channel.id),
            &channel.id,
        ),
    }
}

/// 发布笔记里这个渠道那一节的标题（`〈渠道〉包装`）。
fn package_heading(channel: &Channel, copy: BackendCopy<'_>) -> String {
    heading_for(channel_label(channel, copy), copy)
}

/// 内置渠道那一节的标题（示例项目按它写好一节，与发布页签读的是同一份词）。
pub(crate) fn builtin_package_heading(id: &str, copy: BackendCopy<'_>) -> String {
    heading_for(
        backend_text_with(copy, &format!("creator-channel-{id}"), id),
        copy,
    )
}

fn heading_for(name: String, copy: BackendCopy<'_>) -> String {
    copy.and_then(|i18n| {
        i18n.translate_args("creator-publish-package", &[("channel", name.clone())])
    })
    .unwrap_or_else(|| format!("{name} · 包装"))
}

/// 这个渠道要不要在发布清单里有一步「包装」：这期视频本身发出去的渠道（长视频、音频）。
/// 短视频、文章、帖子、简报是二次分发（分发页签，ADR-166 ④）。
fn packages_in_publish(channel: &Channel) -> bool {
    matches!(
        channel.format,
        ChannelFormat::LongVideo | ChannelFormat::Audio
    )
}

/// 发布笔记里那一节 → 包装（每块按小标题取；列表符号去掉）。
fn package_from_section(section: &str, headings: &PublishHeadings) -> PackageDto {
    let list = |label: &str| title_lines(subsection(section, label)).join("\n");
    PackageDto {
        titles: list(&headings.titles),
        description: subsection(section, &headings.description).trim().to_owned(),
        cover: list(&headings.cover),
        category: list(&headings.category),
        tags: tag_lines(subsection(section, &headings.tags)).join("\n"),
    }
}

/// 包装 → 发布笔记里那一节的正文：五个小标题，标题 / 封面字 / 分类 / 标签各列一项一行，
/// 简介原样；空的那块省掉。
fn package_section(package: &PackageDto, headings: &PublishHeadings) -> String {
    let list = |items: Vec<&str>| {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    [
        (&headings.titles, list(title_lines(&package.titles))),
        (&headings.description, package.description.trim().to_owned()),
        (&headings.cover, list(title_lines(&package.cover))),
        (&headings.category, list(title_lines(&package.category))),
        (&headings.tags, list(tag_lines(&package.tags))),
    ]
    .into_iter()
    .filter(|(_, body)| !body.is_empty())
    .map(|(label, body)| format!("### {label}\n\n{body}"))
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// 模型回的渠道包装（ResultSchema）：只收这一种形状。
#[derive(Debug, serde::Deserialize)]
struct ModelPackage {
    #[serde(default)]
    titles: Vec<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    cover: Vec<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// 每项只取一行、去空、去重；`limit` 截到前几个（只用于标题——模型多给的备选不是用户写的）。
fn single_lines(items: Vec<String>, limit: Option<usize>, strip_hash: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let mut line = item.lines().next().unwrap_or("").trim();
        if strip_hash {
            line = line.trim_start_matches('#').trim();
        }
        if !line.is_empty() && !out.iter().any(|seen| seen == line) {
            out.push(line.to_owned());
        }
        if limit.is_some_and(|limit| out.len() >= limit) {
            break;
        }
    }
    out
}

/// 读模型输出：截出 JSON 再按上面的规矩收拾（简介还没拼章节与来源）；标题与简介都空为 `None`。
fn parse_model_package(raw: &str) -> Option<PackageDto> {
    let begin = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= begin {
        return None;
    }
    let parsed: ModelPackage = serde_json::from_str(&raw[begin..=end]).ok()?;
    let package = PackageDto {
        titles: single_lines(parsed.titles, Some(TITLE_COUNT), false).join("\n"),
        description: parsed.description.trim().to_owned(),
        cover: single_lines(parsed.cover, None, false).join("\n"),
        category: parsed
            .category
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned(),
        tags: single_lines(parsed.tags, None, true).join("\n"),
    };
    (!package.titles.is_empty() || !package.description.is_empty()).then_some(package)
}

/// 喂给 `publish.package` 的渠道说明：上限一句一句（只写有的）、风格（内置渠道取注册表里
/// 那个渠道的风格句，自定义渠道取用户写的风格说明）。句子都来自注册表（ADR-050）。
fn package_prompt_vars(channel: &Channel, copy: BackendCopy<'_>) -> (String, String, String) {
    let registry = luna_core::prompt::registry();
    let scope = luna_core::PromptScope::Summarize;
    let limits = channel.limits;
    let mut sentences: Vec<String> = Vec::new();
    for (value, key) in [
        (limits.title_max, "creator.publish.package.limit.title"),
        (limits.body_max, "creator.publish.package.limit.description"),
        (limits.tags_max, "creator.publish.package.limit.tags"),
        (limits.tag_max, "creator.publish.package.limit.tag"),
    ] {
        if let Some(n) = value {
            sentences.push(registry.render(scope, key, &[("n", &n.to_string())]));
        }
    }
    if sentences.is_empty() {
        sentences.push(registry.render(scope, "creator.publish.package.limit.none", &[]));
    }
    (
        channel_label(channel, copy),
        sentences.join(" "),
        channel_style(channel, "creator.publish.package.style"),
    )
}

/// 一个渠道的风格句：自定义渠道用它自己写的一句；内置渠道取 `<prefix>.<id>` 片段；都没有时
/// 取 `<prefix>.none`。
pub(super) fn channel_style(channel: &Channel, prefix: &str) -> String {
    let registry = luna_core::prompt::registry();
    let scope = luna_core::PromptScope::Summarize;
    let none = || registry.render(scope, &format!("{prefix}.none"), &[]);
    match &channel.style {
        Some(style) if !style.trim().is_empty() => style.trim().to_owned(),
        Some(_) => none(),
        None => {
            let builtin = registry.render(scope, &format!("{prefix}.{}", channel.id), &[]);
            if builtin.is_empty() {
                none()
            } else {
                builtin
            }
        }
    }
}

/// 按 id 找项目可用的渠道（目录里没有为错误：界面过期，渠道刚被删）。
pub(super) fn channel_of(
    vault: &Vault,
    id: &str,
    app: &tauri::AppHandle,
) -> Result<Channel, String> {
    vault
        .channel_catalog()
        .into_iter()
        .find(|channel| channel.id == id)
        .ok_or_else(|| coded_error(app, "CREATOR_CHANNEL_UNKNOWN", "creator-channel-unknown"))
}

/// 一个渠道已写进发布笔记的包装（表单打开时填回）；还没写过为 `None`。
///
/// # Errors
/// 库未打开、渠道不在目录里时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_package(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    channel: String,
) -> Result<Option<PackageDto>, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let headings = publish_headings(copy);
    let note_rel = publish_note_rel(&home, copy);
    state.with_vault(window.label(), |vault| {
        let channel = channel_of(vault, &channel, &app)?;
        let note = vault
            .read(&note_rel)
            .map(|doc| doc.content)
            .unwrap_or_default();
        Ok(
            luna_vault::section_body(&note, &package_heading(&channel, copy))
                .map(|section| package_from_section(&section, &headings))
                .filter(|package| !package.is_empty()),
        )
    })
}

/// 检查一个渠道的包装（界面边改边调）：按这个渠道的上限。
///
/// # Errors
/// 库未打开、渠道不在目录里时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_check_package(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    channel: String,
    draft: PackageDto,
) -> Result<Vec<PackageIssueDto>, String> {
    state.with_vault(window.label(), |vault| {
        Ok(check_package(
            &draft,
            &channel_of(vault, &channel, &app)?.limits,
        ))
    })
}

/// 一个渠道包装的草稿（五块都可直接改）+ 检查结果。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PackageDraftDto {
    pub package: PackageDto,
    pub issues: Vec<PackageIssueDto>,
}

/// 由 AI 为一个渠道写包装（ADR-166 ④）：三个标题、简介正文、封面字、分类与标签，按这个渠道的
/// 上限与风格；简介正文之后由这里按固定顺序拼进章节、参考来源与链接（ADR-162 ⑪）。只出文字，
/// 不接平台任何接口（PRD-19 §7）。
///
/// # Errors
/// 库未打开、渠道不在目录里、成片不在项目里、没有转写段、模型不可用或回得不像包装时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_generate_package(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
    source: String,
    channel: String,
    extra: Option<String>,
) -> Result<PackageDraftDto, String> {
    let transcript = load_source_transcript(&app, window.label(), &home, &source)?;
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let headings = publish_headings(copy);
    let research_rel =
        super::creator::project_child_rel(&home, copy, "creator-note-research", "研究");
    let sources_heading_in_note = backend_text_with(copy, "creator-research-sources", "来源");
    let publish_rel = publish_note_rel(&home, copy);
    let state = app.state::<AppState>();
    let (channel, chapters, links, sources) = state.with_vault(window.label(), |vault| {
        let channel = channel_of(vault, &channel, &app)?;
        let publish = vault
            .read(&publish_rel)
            .map(|doc| doc.content)
            .unwrap_or_default();
        let research = vault
            .read(&research_rel)
            .map(|doc| doc.content)
            .unwrap_or_default();
        Ok((
            channel,
            luna_vault::section_body(&publish, &headings.chapters),
            luna_vault::section_body(&publish, &headings.links),
            luna_vault::section_body(&research, &sources_heading_in_note)
                .map(|section| source_lines(&section))
                .unwrap_or_default(),
        ))
    })?;
    let (name, limits, style) = package_prompt_vars(&channel, copy);
    let output = run_publish_job_with(
        &app,
        "creator.publish.package",
        &[("channel", &name), ("limits", &limits), ("style", &style)],
        &CreatorNotes::of(&app, extra.as_deref()),
        &transcript.lines,
        PACKAGE_OUTPUT_TOKENS,
    )
    .await?;
    let mut package = parse_model_package(&output)
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;
    package.description = assemble_description(
        &package.description,
        &CopyBlocks {
            chapters: chapters.as_deref(),
            sources: &sources,
            links: links.as_deref(),
            chapters_heading: &headings.chapters,
            sources_heading: &headings.sources,
            links_heading: &headings.links,
        },
    );
    Ok(PackageDraftDto {
        issues: check_package(&package, &channel.limits),
        package,
    })
}

/// 把一个渠道的包装写进发布笔记的「〈渠道〉包装」一节（已有就替换），返回这篇笔记的库相对路径。
///
/// # Errors
/// 库未打开、渠道不在目录里或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_save_package(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    channel: String,
    draft: PackageDto,
) -> Result<String, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let headings = publish_headings(copy);
    let note_rel = publish_note_rel(&home, copy);
    let (content, created) = state.with_vault(window.label(), |vault| {
        let channel = channel_of(vault, &channel, &app)?;
        let (existing, created) = match vault.read(&note_rel) {
            Ok(doc) => (doc.content, false),
            Err(_) => (String::new(), true),
        };
        let content = luna_vault::upsert_section(
            &existing,
            &package_heading(&channel, copy),
            &package_section(&draft, &headings),
        );
        vault.write(&note_rel, &content).map_err(vault_err)?;
        Ok((content, created))
    })?;
    broadcast_note_write(&app, window.label(), &note_rel, content);
    if created {
        emit_vault_changed_in_scope(
            &app,
            &note_rel,
            false,
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(note_rel)
}

// ── 项目概览（C5a，PRD-19-004）────────────────────────────────────────────────

/// 项目概览里的一类（顺序即页面上的顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OverviewGroupDto {
    Scripts,
    /// 素材与来源（ADR-167 ③）。
    Sources,
    Footage,
    Research,
    Swipes,
    Publish,
}

impl From<luna_vault::OverviewGroup> for OverviewGroupDto {
    fn from(value: luna_vault::OverviewGroup) -> Self {
        match value {
            luna_vault::OverviewGroup::Scripts => Self::Scripts,
            luna_vault::OverviewGroup::Sources => Self::Sources,
            luna_vault::OverviewGroup::Footage => Self::Footage,
            luna_vault::OverviewGroup::Research => Self::Research,
            luna_vault::OverviewGroup::Swipes => Self::Swipes,
            luna_vault::OverviewGroup::Publish => Self::Publish,
        }
    }
}

/// 一条来源是什么（ADR-167 ③）：界面据此给类型标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceKindDto {
    Media,
    Document,
    Web,
    Text,
    Note,
}

impl From<luna_vault::SourceKind> for SourceKindDto {
    fn from(value: luna_vault::SourceKind) -> Self {
        match value {
            luna_vault::SourceKind::Media => Self::Media,
            luna_vault::SourceKind::Document => Self::Document,
            luna_vault::SourceKind::Web => Self::Web,
            luna_vault::SourceKind::Text => Self::Text,
            luna_vault::SourceKind::Note => Self::Note,
        }
    }
}

/// 项目概览里的一篇笔记。
#[derive(Debug, Clone, Serialize, Type)]
pub struct OverviewItemDto {
    pub path: String,
    pub title: String,
    /// 修改时间（Unix 毫秒）。
    pub modified_ms: f64,
    pub group: OverviewGroupDto,
    /// 来源的类型；不在「素材与来源」里为 `null`。
    pub kind: Option<SourceKindDto>,
    /// 研究笔记链到它（可「移出项目」）。
    pub linked_from_research: bool,
    /// 项目主页链到它。
    pub linked_from_home: bool,
    /// 它链到项目主页。
    pub links_to_home: bool,
    /// 它在项目文件夹里。
    pub in_folder: bool,
}

/// 一个项目的概览（ADR-162 ⑫）：类与类内顺序都已由 Rust 排好。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectOverviewDto {
    pub stage: ProjectStageDto,
    pub items: Vec<OverviewItemDto>,
    /// 「发布」文件夹里已导出的文件（库相对路径）。
    pub exports: Vec<String>,
}

/// 一个项目的概览（PRD-19-004）：主页链出、链入主页、位于项目文件夹的笔记，按类列出；只列出、
/// 不复制内容。
///
/// # Errors
/// 库未打开、`home` 不是视频项目主页时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_project_overview(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<ProjectOverviewDto, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let publish_note = publish_note_rel(&home, copy);
    let folder_name = backend_text_with(copy, "creator-publish-folder", "发布");
    let export_dir = match project_folder(&home) {
        "" => folder_name,
        folder => format!("{folder}/{folder_name}"),
    };
    let research_note = super::creator_quote::research_note_rel(&home, copy);
    let material_name = backend_text_with(copy, "creator-material-folder", "素材");
    let material_dir = match project_folder(&home) {
        "" => material_name,
        folder => format!("{folder}/{material_name}"),
    };
    let layout = luna_vault::ProjectLayout {
        publish_note: &publish_note,
        research_note: &research_note,
        material_dir: &material_dir,
        export_dir: &export_dir,
    };
    let overview = state.with_vault(window.label(), |vault| {
        vault.project_overview(&home, &layout).map_err(vault_err)
    })?;
    Ok(ProjectOverviewDto {
        stage: overview.stage.into(),
        items: overview
            .items
            .into_iter()
            .map(|item| OverviewItemDto {
                #[allow(clippy::cast_precision_loss)]
                modified_ms: item.modified_ms as f64,
                path: item.path,
                title: item.title,
                group: item.group.into(),
                kind: item.kind.map(Into::into),
                linked_from_research: item.linked_from_research,
                linked_from_home: item.linked_from_home,
                links_to_home: item.links_to_home,
                in_folder: item.in_folder,
            })
            .collect(),
        exports: overview.exports,
    })
}

// ── 发布清单（ADR-165 ⑧ / ADR-166 ④，PRD-19-039）──────────────────────────────

/// 发布清单的一步（顺序即页面上的顺序）：成片 · 字幕 · 章节 · 每个长视频 / 音频渠道一步包装 · 检查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublishStepDto {
    FinalCut,
    Subtitles,
    Chapters,
    /// 一个渠道的包装（标题、简介、封面字、分类、标签）。
    Package {
        channel: String,
    },
    Check,
}

/// 一步的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PublishStepStateDto {
    pub step: PublishStepDto,
    /// 这一步的产物已经在了（发布笔记里有那一节 / 「发布」文件夹里有字幕）。
    pub done: bool,
    /// 前置不满足：要先做完哪一步（界面写明缺什么、给去那一步的按钮）。
    pub blocked_by: Option<PublishStepDto>,
    /// 这一步的产物里有几处不合规（「检查」一步汇总）。
    pub issues: u32,
}

/// 「检查」里的一条（每条都能回到它所属的那一步）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublishCheckDto {
    /// 这一步还没做。
    Missing {
        step: PublishStepDto,
    },
    Chapters {
        issue: ChapterIssueDto,
    },
    Package {
        channel: String,
        issue: PackageIssueDto,
    },
}

/// 一个项目的发布清单：步骤按渠道生成，完成度与不合规处都由这里判定（ADR-003）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PublishChecklistDto {
    /// 项目的渠道 id（有序）。
    pub channels: Vec<String>,
    pub steps: Vec<PublishStepStateDto>,
    /// 已完成几步 / 共几步（含「检查」）。
    pub done: u32,
    pub total: u32,
    pub checks: Vec<PublishCheckDto>,
}

/// 一个要在发布清单里包装的渠道：它的上限，与发布笔记里已写下的那一份。
struct ChannelPackage {
    channel: String,
    limits: ChannelLimits,
    package: Option<PackageDto>,
}

/// 判定清单要读的东西（都已从盘上取好；纯函数只看这些）。
struct ChecklistInput<'a> {
    channels: &'a [String],
    /// 项目里要包装的渠道（长视频、音频），按项目里的顺序。
    packages: &'a [ChannelPackage],
    has_final_cut: bool,
    /// 最新一篇成片的时长（章节「超出片长」按它查）。
    duration_ms: Option<u64>,
    exports: &'a [String],
    chapters: Option<&'a str>,
}

/// 字幕文件：发布包导出的三种格式。
fn is_subtitle_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".srt", ".vtt", ".bcc"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// 一节里 `### 小标题` 下的那块正文（到下一个 `###` 为止）。
fn subsection<'a>(section: &'a str, label: &str) -> &'a str {
    let mut start: Option<usize> = None;
    let mut at = 0;
    for raw in section.split_inclusive('\n') {
        let heading = raw.trim().strip_prefix("### ").map(str::trim);
        if let Some(heading) = heading {
            if let Some(start) = start {
                return &section[start..at];
            }
            if heading == label {
                start = Some(at + raw.len());
            }
        }
        at += raw.len();
    }
    start.map_or("", |start| &section[start..])
}

fn non_empty(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

/// 判定发布清单（纯函数）：成片 · 字幕 · 章节 · 〈渠道〉包装… · 检查。包装「做完」= 标题与简介
/// 都有；超限不算没做，只记作待改并汇总进「检查」。
fn publish_checklist(input: &ChecklistInput<'_>) -> PublishChecklistDto {
    let needs_cut = (!input.has_final_cut).then_some(PublishStepDto::FinalCut);
    let chapter_issues: Vec<ChapterIssueDto> = input
        .chapters
        .filter(|text| !text.trim().is_empty())
        .map(|text| check_text(text, input.duration_ms).issues)
        .unwrap_or_default();

    let mut steps = vec![
        PublishStepStateDto {
            step: PublishStepDto::FinalCut,
            done: input.has_final_cut,
            blocked_by: None,
            issues: 0,
        },
        PublishStepStateDto {
            step: PublishStepDto::Subtitles,
            done: input.exports.iter().any(|path| is_subtitle_file(path)),
            blocked_by: needs_cut.clone(),
            issues: 0,
        },
        PublishStepStateDto {
            step: PublishStepDto::Chapters,
            done: non_empty(input.chapters),
            blocked_by: needs_cut.clone(),
            issues: count_u32(chapter_issues.len()),
        },
    ];
    let mut package_issues: Vec<PublishCheckDto> = Vec::new();
    for entry in input.packages {
        let issues = entry
            .package
            .as_ref()
            .map(|package| check_package(package, &entry.limits))
            .unwrap_or_default();
        let done = entry.package.as_ref().is_some_and(|package| {
            !package.titles.trim().is_empty() && !package.description.trim().is_empty()
        });
        steps.push(PublishStepStateDto {
            step: PublishStepDto::Package {
                channel: entry.channel.clone(),
            },
            done,
            blocked_by: needs_cut.clone(),
            issues: count_u32(issues.len()),
        });
        package_issues.extend(issues.into_iter().map(|issue| PublishCheckDto::Package {
            channel: entry.channel.clone(),
            issue,
        }));
    }
    // 做过的步骤不再标「被挡」：成片后来被移走，已写好的章节仍是写好的。
    for step in &mut steps {
        if step.done {
            step.blocked_by = None;
        }
    }

    let mut checks: Vec<PublishCheckDto> = steps
        .iter()
        .filter(|step| !step.done)
        .map(|step| PublishCheckDto::Missing {
            step: step.step.clone(),
        })
        .collect();
    checks.extend(
        chapter_issues
            .into_iter()
            .map(|issue| PublishCheckDto::Chapters { issue }),
    );
    checks.extend(package_issues);
    steps.push(PublishStepStateDto {
        step: PublishStepDto::Check,
        done: checks.is_empty(),
        blocked_by: None,
        issues: count_u32(checks.len()),
    });

    PublishChecklistDto {
        channels: input.channels.to_vec(),
        done: count_u32(steps.iter().filter(|step| step.done).count()),
        total: count_u32(steps.len()),
        steps,
        checks,
    }
}

/// 一个项目的发布清单（PRD-19-039）：从项目的渠道、成片候选、发布笔记各节与「发布」文件夹
/// 判定每一步（只读）。
///
/// # Errors
/// 库未打开、`home` 不是视频项目主页时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_publish_checklist(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<PublishChecklistDto, String> {
    let i18n = backend_copy_arc(&app);
    state.with_vault(window.label(), |vault| {
        publish_checklist_of(vault, &home, i18n.as_deref())
    })
}

/// 读盘、判定一个项目的发布清单——发布页签与看板卡片的进度（PRD-19-040）共用这一处。
pub(crate) fn publish_checklist_of(
    vault: &Vault,
    home: &str,
    copy: BackendCopy<'_>,
) -> Result<PublishChecklistDto, String> {
    let headings = publish_headings(copy);
    let publish_rel = publish_note_rel(home, copy);
    let export_dir = match project_folder(home) {
        "" => backend_text_with(copy, "creator-publish-folder", "发布"),
        folder => format!(
            "{folder}/{}",
            backend_text_with(copy, "creator-publish-folder", "发布")
        ),
    };
    let channels = vault.project_channels(home).map_err(vault_err)?;
    let catalog = vault.channel_catalog();
    let sources = vault.project_publish_sources(home).map_err(vault_err)?;
    let exports = vault.publish_exports(&export_dir);
    let note = vault
        .read(&publish_rel)
        .map(|doc| doc.content)
        .unwrap_or_default();
    let packages: Vec<ChannelPackage> = channels
        .iter()
        .filter_map(|id| catalog.iter().find(|channel| &channel.id == id))
        .filter(|channel| packages_in_publish(channel))
        .map(|channel| ChannelPackage {
            channel: channel.id.clone(),
            limits: channel.limits,
            package: luna_vault::section_body(&note, &package_heading(channel, copy))
                .map(|section| package_from_section(&section, &headings))
                .filter(|package| !package.is_empty()),
        })
        .collect();
    let chapters = luna_vault::section_body(&note, &headings.chapters);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let duration_ms = sources
        .first()
        .and_then(|source| source.duration_secs)
        .map(|secs| (secs * 1000.0) as u64);
    Ok(publish_checklist(&ChecklistInput {
        channels: &channels,
        packages: &packages,
        has_final_cut: !sources.is_empty(),
        duration_ms,
        exports: &exports,
        chapters: chapters.as_deref(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_export::HanVariant;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn package_for(
        channel: &str,
        limits: ChannelLimits,
        package: Option<PackageDto>,
    ) -> ChannelPackage {
        ChannelPackage {
            channel: channel.to_owned(),
            limits,
            package,
        }
    }

    fn youtube_limits() -> ChannelLimits {
        luna_vault::channel::builtin_channel("youtube")
            .unwrap()
            .limits
    }

    fn bilibili_limits() -> ChannelLimits {
        luna_vault::channel::builtin_channel("bilibili")
            .unwrap()
            .limits
    }

    fn checklist_input<'a>(
        channels: &'a [String],
        packages: &'a [ChannelPackage],
    ) -> ChecklistInput<'a> {
        ChecklistInput {
            channels,
            packages,
            has_final_cut: false,
            duration_ms: None,
            exports: &[],
            chapters: None,
        }
    }

    #[test]
    fn checklist_has_one_package_step_per_publishing_channel_and_blocks_on_final_cut() {
        let channels = ids(&["youtube", "bilibili", "douyin"]);
        let packages = [
            package_for("youtube", youtube_limits(), None),
            package_for("bilibili", bilibili_limits(), None),
        ];
        let list = publish_checklist(&checklist_input(&channels, &packages));
        let steps: Vec<PublishStepDto> = list.steps.iter().map(|s| s.step.clone()).collect();
        assert_eq!(
            steps,
            [
                PublishStepDto::FinalCut,
                PublishStepDto::Subtitles,
                PublishStepDto::Chapters,
                PublishStepDto::Package {
                    channel: "youtube".into()
                },
                PublishStepDto::Package {
                    channel: "bilibili".into()
                },
                PublishStepDto::Check,
            ]
        );
        assert_eq!((list.done, list.total), (0, 6));
        assert!(list.steps[1..5]
            .iter()
            .all(|s| s.blocked_by == Some(PublishStepDto::FinalCut)));
        assert_eq!(
            list.checks[0],
            PublishCheckDto::Missing {
                step: PublishStepDto::FinalCut
            }
        );
    }

    #[test]
    fn package_steps_are_done_with_titles_and_description_and_issues_feed_the_check() {
        let channels = ids(&["youtube", "bilibili"]);
        let exports = ["Creator/P/发布/P.zh.srt".to_owned()];
        let long_tag = "很".repeat(21);
        let packages = [
            package_for(
                "youtube",
                youtube_limits(),
                Some(PackageDto {
                    titles: "一\n二\n三".into(),
                    description: "简介".into(),
                    ..PackageDto::default()
                }),
            ),
            package_for(
                "bilibili",
                bilibili_limits(),
                Some(PackageDto {
                    titles: "一\n二\n三".into(),
                    tags: long_tag,
                    ..PackageDto::default()
                }),
            ),
        ];
        let list = publish_checklist(&ChecklistInput {
            has_final_cut: true,
            duration_ms: Some(600_000),
            exports: &exports,
            chapters: Some("0:00 开场\n0:30 正文\n1:00 结尾"),
            ..checklist_input(&channels, &packages)
        });
        let state =
            |step: PublishStepDto| list.steps.iter().find(|s| s.step == step).unwrap().clone();
        assert!(
            state(PublishStepDto::Package {
                channel: "youtube".into()
            })
            .done
        );
        // B 站没写简介：这一步没做完，它的标签超长照样记下。
        let bilibili = state(PublishStepDto::Package {
            channel: "bilibili".into(),
        });
        assert!(!bilibili.done);
        assert_eq!(bilibili.issues, 1);
        assert!(list.checks.contains(&PublishCheckDto::Package {
            channel: "bilibili".into(),
            issue: PackageIssueDto::TagTooLong {
                tag: 1,
                over: 1,
                max: 20
            },
        }));
        assert_eq!(list.done, 4);
    }

    #[test]
    fn package_checks_only_use_the_limits_the_channel_has() {
        let package = PackageDto {
            titles: format!("{}\n短\n短", "长".repeat(90)),
            description: "x".repeat(2001),
            cover: "第一行\n第二行特别特别特别特别长啊啊\n第三行".into(),
            tags: (1..=11)
                .map(|n| format!("标签{n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ..PackageDto::default()
        };
        assert_eq!(
            check_package(&package, &bilibili_limits()),
            [
                PackageIssueDto::TitleTooLong {
                    title: 1,
                    over: 10,
                    max: 80
                },
                PackageIssueDto::DescriptionTooLong { over: 1, max: 2000 },
                PackageIssueDto::TooManyCoverLines { count: 3 },
                PackageIssueDto::CoverLineTooLong { line: 2 },
                PackageIssueDto::TooManyTags { count: 11, max: 10 },
            ]
        );
        // 没核对过的渠道：只剩与渠道无关的那几条（标题个数、封面可读性）。
        assert_eq!(
            check_package(
                &PackageDto {
                    titles: "一".into(),
                    cover: "Short english cover".into(),
                    ..PackageDto::default()
                },
                &ChannelLimits::default()
            ),
            [PackageIssueDto::TitleCount { count: 1 }]
        );
    }

    #[test]
    fn a_package_round_trips_through_its_section_in_the_publish_note() {
        let headings = publish_headings(None);
        let package = PackageDto {
            titles: "标题一\n- 标题二\n标题三".into(),
            description: "这期讲统一内存。\n\n章节\n0:00 开场".into(),
            cover: String::new(),
            category: "知识 › 科学科普".into(),
            tags: "#科普\n脚本".into(),
        };
        let section = package_section(&package, &headings);
        assert!(!section.contains("### 封面文字"), "空的那块省掉");
        assert_eq!(
            package_from_section(&section, &headings),
            PackageDto {
                titles: "标题一\n标题二\n标题三".into(),
                tags: "科普\n脚本".into(),
                ..package
            }
        );
    }

    #[test]
    fn the_model_package_is_tidied_into_one_item_per_line() {
        let raw = "好的：{\"titles\":[\"标题一\",\"标题一\",\"标题二\\n多余\",\"标题三\",\"标题四\"],\"description\":\" 简介 \",\"cover\":[\"三分钟\",\"\",\"看懂\"],\"category\":\"知识 › 科学科普\\n多余\",\"tags\":[\"#科普\",\"科普\",\"脚本\"]}";
        assert_eq!(
            parse_model_package(raw),
            Some(PackageDto {
                titles: "标题一\n标题二\n标题三".into(),
                description: "简介".into(),
                cover: "三分钟\n看懂".into(),
                category: "知识 › 科学科普".into(),
                tags: "科普\n脚本".into(),
            })
        );
        assert_eq!(parse_model_package("{\"titles\":[]}"), None);
        assert_eq!(parse_model_package("没有 JSON"), None);
    }

    #[test]
    fn package_prompt_vars_list_only_known_limits_and_the_channel_style() {
        let youtube = luna_vault::channel::Channel {
            id: "youtube".into(),
            name: None,
            format: ChannelFormat::LongVideo,
            limits: youtube_limits(),
            style: None,
            builtin: true,
            ai_notice: false,
        };
        let (name, limits, style) = package_prompt_vars(&youtube, None);
        assert_eq!(name, "youtube", "没有界面语言时退回 id");
        assert!(limits.contains("100") && limits.contains("5000"));
        assert!(!style.is_empty());
        let custom = luna_vault::channel::Channel {
            id: "custom-1a2b3c4d".into(),
            name: Some("我的播客".into()),
            format: ChannelFormat::Audio,
            limits: ChannelLimits::default(),
            style: Some("口语化，开头一句话抓人".into()),
            builtin: false,
            ai_notice: false,
        };
        let (name, limits, style) = package_prompt_vars(&custom, None);
        assert_eq!(name, "我的播客");
        assert!(!limits.is_empty());
        assert_eq!(style, "口语化，开头一句话抓人");
    }

    #[test]
    fn creator_notes_ride_after_the_instruction_as_content_and_are_clipped() {
        let mut settings = luna_config::Settings::default();
        let plain = luna_core::prompt::registry().instruction(
            luna_core::PromptScope::Summarize,
            "creator.publish.chapters",
            &[],
        );
        let mut untouched = plain.clone();
        CreatorNotes::new(&settings, Some("   ")).apply(&mut untouched);
        assert_eq!(untouched, plain, "没写风格、额外要求为空：指令一字不改");

        settings.set(CHANNEL_VOICE_SETTING, "说话直接，{爱用}比喻。");
        let long_extra = "多".repeat(EXTRA_MAX_CHARS + 50);
        let mut noted = plain.clone();
        CreatorNotes::new(&settings, Some(&long_extra)).apply(&mut noted);
        assert!(noted.starts_with(plain.as_str()), "模板原样在前，不改结构");
        assert!(
            noted.contains("说话直接，{爱用}比喻。"),
            "风格原话带进去，花括号不被展开"
        );
        assert!(noted.contains(&"多".repeat(EXTRA_MAX_CHARS)));
        assert!(
            !noted.contains(&"多".repeat(EXTRA_MAX_CHARS + 1)),
            "额外要求截到上限"
        );
    }

    #[test]
    fn subsection_reads_until_the_next_small_heading() {
        let package = "### 标题备选\n\n- 一\n\n### 标签\n\n- 甲\n- 乙\n";
        assert_eq!(subsection(package, "标题备选").trim(), "- 一");
        assert_eq!(subsection(package, "标签").trim(), "- 甲\n- 乙");
        assert_eq!(subsection(package, "封面文字"), "");
    }

    fn names(plan: &[PlannedSubtitle]) -> Vec<String> {
        plan.iter()
            .map(|p| format!("{}.{}", p.stem, p.format.extension()))
            .collect()
    }

    #[test]
    fn with_a_translation_three_subtitles_come_out_and_bcc_doubles_them() {
        let plan = subtitle_plan(
            "第一期",
            Some("zh"),
            Some("en"),
            true,
            HanVariant::TraditionalTaiwan,
            ExportFormat::Srt,
            true,
        );
        assert_eq!(
            names(&plan),
            [
                "第一期.zh-Hant-TW.srt",
                "第一期.en.srt",
                "第一期.zh-Hant-TW-en.srt",
                "第一期.zh-Hant-TW.bcc",
                "第一期.en.bcc",
                "第一期.zh-Hant-TW-en.bcc",
            ]
        );
        assert_eq!(plan[2].content, Bilingual::Both);
    }

    #[test]
    fn without_a_translation_only_the_original_and_unknown_languages_fall_back_to_content_names() {
        let plan = subtitle_plan(
            "P",
            None,
            None,
            false,
            HanVariant::Keep,
            ExportFormat::Vtt,
            false,
        );
        assert_eq!(names(&plan), ["P.source.vtt"]);
        let plan = subtitle_plan(
            "P",
            Some("en"),
            None,
            true,
            HanVariant::Keep,
            ExportFormat::Srt,
            false,
        );
        assert_eq!(
            names(&plan),
            ["P.en.srt", "P.translation.srt", "P.bilingual.srt"]
        );
    }

    #[test]
    fn only_chinese_takes_the_script_variant_in_its_tag() {
        assert_eq!(
            lang_tag(Some("zh"), HanVariant::Keep).as_deref(),
            Some("zh")
        );
        assert_eq!(
            lang_tag(Some("zh"), HanVariant::Simplified).as_deref(),
            Some("zh-Hans")
        );
        assert_eq!(
            lang_tag(Some("zh-CN"), HanVariant::TraditionalHongKong).as_deref(),
            Some("zh-Hant-HK")
        );
        assert_eq!(
            lang_tag(Some("ja"), HanVariant::TraditionalTaiwan).as_deref(),
            Some("ja")
        );
        assert_eq!(lang_tag(Some(" "), HanVariant::Keep), None);
    }

    #[test]
    fn the_project_folder_is_the_home_pages_directory() {
        assert_eq!(project_folder("Creator/第一期/第一期.md"), "Creator/第一期");
        assert_eq!(project_folder("第一期.md"), "");
    }

    #[test]
    fn model_chapters_snap_to_sentence_starts_and_accept_seconds() {
        let starts = [0, 6_000, 12_000, 30_000];
        let raw = "好的：{\"chapters\": [{\"start\": \"0:01\", \"title\": \"开场\\n多余\"}, {\"start\": 13, \"title\": \"正文\"}, {\"start\": \"x\"}]}";
        let chapters = parse_model_chapters(raw, &starts).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(
            chapters[0],
            Chapter {
                start_ms: 0,
                title: "开场".into()
            }
        );
        assert_eq!(chapters[1].start_ms, 12_000);
        assert!(parse_model_chapters("不是 JSON", &starts).is_none());
        assert!(parse_model_chapters("{\"chapters\": []}", &starts).is_none());
    }

    #[test]
    fn checking_reports_issues_and_offers_a_fix_only_when_it_changes_something() {
        let ok = check_text("0:00 开场\n0:20 正文\n0:40 结尾", Some(60_000));
        assert!(ok.issues.is_empty());
        assert_eq!(ok.fixed, None);
        let bad = check_text(
            "0:05 开场\n0:20 正文\n0:25 太短\n0:40 结尾\n乱写",
            Some(60_000),
        );
        assert!(bad.issues.contains(&ChapterIssueDto::Unparsed { line: 5 }));
        assert!(bad.issues.contains(&ChapterIssueDto::FirstNotAtZero));
        assert!(bad.issues.contains(&ChapterIssueDto::TooShort {
            chapter: 2,
            seconds: 5
        }));
        assert_eq!(
            bad.fixed.as_deref(),
            Some("0:00 开场\n0:20 正文\n0:40 结尾")
        );
        // 章数不够：修不出来，不给「修正」。
        let few = check_text("0:00 开场\n0:30 结尾", None);
        assert_eq!(few.issues, [ChapterIssueDto::TooFew { count: 2 }]);
        assert_eq!(few.fixed, None);
    }

    #[test]
    fn the_publish_note_sits_next_to_the_home_page() {
        assert_eq!(
            publish_note_rel("Creator/第一期/第一期.md", None),
            "Creator/第一期/第一期-发布.md"
        );
        assert_eq!(publish_note_rel("第一期.md", None), "第一期-发布.md");
    }

    #[test]
    fn sources_become_plain_text_lines() {
        let section = "- [官网](https://lunarca.app)\n* 参考 [[会议纪要|那次会]]\n普通段落不算\n- [不是链接\n";
        assert_eq!(
            source_lines(section),
            ["官网 https://lunarca.app", "参考 那次会", "[不是链接"]
        );
    }

    #[test]
    fn a_description_is_summary_then_chapters_then_sources_then_links() {
        let sources = vec!["官网 https://lunarca.app".to_owned()];
        let blocks = CopyBlocks {
            chapters: Some("0:00 开场\n1:00 正文"),
            sources: &sources,
            links: Some("微博 @lunarca"),
            chapters_heading: "章节",
            sources_heading: "参考来源",
            links_heading: "链接",
        };
        assert_eq!(
            assemble_description("这期讲怎么写脚本。", &blocks),
            "这期讲怎么写脚本。\n\n章节\n0:00 开场\n1:00 正文\n\n参考来源\n官网 https://lunarca.app\n\n链接\n微博 @lunarca"
        );
        let bare = CopyBlocks {
            chapters: None,
            sources: &[],
            links: Some("  "),
            ..blocks
        };
        assert_eq!(assemble_description("只有正文", &bare), "只有正文");
    }
}
