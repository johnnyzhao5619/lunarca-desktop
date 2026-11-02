//! 工作流配方的执行（纵切 7b B5，ADR-161 ② ③，PRD-18-003 / 010）。
//!
//! **只有从在线目录装的包能运行配方**（ADR-161 ② 用户裁决：服务端审核过、目录签了名）；本地导入的包里的配方
//! 始终只展示。被封禁的包不在可用清单里（`usable_template_packs`），它的配方随之消失。
//!
//! **只执行白名单步骤**，每一步都落在已有的主干上，不另起调用栈（ADR-001）：
//! - `transcribe`：取这条录音已有的转写稿（边车段落，带说话人名）。录音落盘时已由转写队列转好，配方不重新转写；
//!   还没有转写稿就停下、说清楚。
//!   > 假设：「转写」步骤复用录音已有的转写稿而不是再跑一遍（理由：Lunarca 的录音停止后即由处理队列转写，
//!   > 再转一遍只会花时间、得到同一份字；没有转写稿时如实停下，比悄悄排一个长任务更可预期）。
//! - `translate`：`JobKind::Translate`，目标语言是步骤声明的那一个。
//! - `llm`：包里那份提示词与用户自定义 AI 预设走同一条路（`editor.ask.selection` + `Refine`，ADR-161 ④），
//!   结果只收文字（ResultSchema = `ResultPayload::Text`）。
//! - `vault.create_note`：在清单声明的库内相对目录下新建一篇笔记，名字被占就加序号，**从不覆盖**。
//! - `export`：经导出注册表（`luna-export`）渲染，保存位置由用户在对话框里选。
//!
//! 输入超过这一步能处理的长度时**停下并说明**，不静默截断（PRD-06-025 同一纪律）。运行前前端要拿 Rust 给的那份
//! 能力做一次确认，运行时 Rust 再把确认过的能力与此刻推导出的比一遍——包在确认之后被更新、能力变了，就要重新确认。

use luna_content_schema::{Recipe, RecipeInput, RecipeStep};

use super::template_library::{usable_template_packs, InstalledPack, PackCapabilitiesDto};
use super::*;

/// 配方接在哪一类东西上（由第一步推导，见 `luna_content_schema::RecipeInput`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecipeInputDto {
    /// 录音 / 转写笔记的转写稿。
    Recording,
    /// 任意一篇笔记的正文。
    Note,
}

impl From<RecipeInput> for RecipeInputDto {
    fn from(input: RecipeInput) -> Self {
        match input {
            RecipeInput::Recording => Self::Recording,
            RecipeInput::Note => Self::Note,
        }
    }
}

/// 一步（确认框逐条写「它会做的事」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecipeStepDto {
    Transcribe,
    Translate {
        to: String,
    },
    /// `prompt`：包里那份提示词的文件名（不含目录与扩展名）。
    Llm {
        prompt: String,
    },
    VaultCreateNote {
        dir: String,
    },
    Export {
        format: String,
    },
}

impl From<&RecipeStep> for RecipeStepDto {
    fn from(step: &RecipeStep) -> Self {
        match step {
            RecipeStep::Transcribe => Self::Transcribe,
            RecipeStep::Translate { to } => Self::Translate { to: to.clone() },
            RecipeStep::Llm { prompt } => {
                let name = prompt
                    .as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or(prompt.as_str());
                Self::Llm {
                    prompt: name
                        .rsplit_once('.')
                        .map_or(name, |(stem, _)| stem)
                        .to_owned(),
                }
            }
            RecipeStep::VaultCreateNote { dir } => Self::VaultCreateNote {
                dir: dir.as_str().to_owned(),
            },
            RecipeStep::Export { format } => Self::Export {
                format: format.clone(),
            },
        }
    }
}

/// 一份能运行的配方（笔记 / 录音右键菜单的「运行配方」）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RecipeDto {
    pub pack: String,
    pub pack_name: String,
    pub pack_version: String,
    pub id: String,
    pub title: String,
    pub input: RecipeInputDto,
    pub steps: Vec<RecipeStepDto>,
    /// 只这份配方会做的事（运行前确认，运行时原样交回比对）。
    pub capabilities: PackCapabilitiesDto,
}

/// 能运行配方的包（**纯函数**）：从在线目录装的才算（ADR-161 ②）。
fn runnable(packs: &[InstalledPack]) -> Vec<RecipeDto> {
    packs
        .iter()
        .filter(|pack| pack.installed_from_catalog())
        .flat_map(|pack| {
            let manifest = &pack.contents.manifest;
            pack.contents.recipes.iter().map(move |recipe| RecipeDto {
                pack: manifest.id.as_str().to_owned(),
                pack_name: manifest.name.clone(),
                pack_version: manifest.version.to_string(),
                id: recipe.id.as_str().to_owned(),
                title: recipe.title.clone(),
                input: recipe.input().into(),
                steps: recipe.steps.iter().map(Into::into).collect(),
                capabilities: recipe.capabilities().into(),
            })
        })
        .collect()
}

/// 此刻能运行的配方（零网络）。本地导入的包与被封禁的包不在其中。
#[tauri::command]
#[specta::specta]
pub(crate) fn runnable_recipes(app: tauri::AppHandle) -> Vec<RecipeDto> {
    runnable(&usable_template_packs(&app))
}

/// 一次运行的结果：新建的笔记（库相对路径）与导出的文件（绝对路径）；没有这一步或用户在保存对话框里取消为 `None`。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub struct RecipeRunDto {
    pub created: Option<String>,
    pub exported: Option<String>,
}

/// 运行中手里的东西：还没动过的转写稿保留时间轴（字幕类导出要它），一旦经过翻译或 AI 就只剩文字。
#[derive(Debug, Clone)]
enum Value {
    Transcript(Vec<luna_core::Segment>),
    Text(String),
}

impl Value {
    fn text(&self) -> String {
        match self {
            Self::Transcript(segments) => segments
                .iter()
                .map(|segment| match &segment.speaker {
                    Some(speaker) => format!("{speaker}: {}", segment.text.trim()),
                    None => segment.text.trim().to_owned(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Text(text) => text.clone(),
        }
    }
}

/// 一种导出：注册表里的格式，或原样写出 Markdown（**纯函数**）。名字不认识时 `None`。
fn export_target(format: &str) -> Option<Result<luna_export::ExportFormat, &'static str>> {
    use luna_export::ExportFormat as F;
    Some(Ok(match format {
        "txt" => F::Txt,
        "srt" => F::Srt,
        "vtt" => F::Vtt,
        "ass" => F::Ass,
        "lrc" => F::Lrc,
        "bcc" => F::Bcc,
        "md" => return Some(Err("md")),
        _ => return None,
    }))
}

/// 按一步导出的内容与扩展名（**纯函数**）。字幕类要时间轴：手里已经只剩文字时说清楚，不编时间。
fn render_export(format: &str, value: &Value) -> Result<(String, &'static str), String> {
    let target = export_target(format).ok_or_else(|| format!("RECIPE_UNKNOWN_FORMAT: {format}"))?;
    let format = match target {
        Err(extension) => return Ok((value.text(), extension)),
        Ok(format) => format,
    };
    let exportable = match (value, format) {
        (Value::Transcript(segments), _) => luna_export::Exportable::Segments(segments.clone()),
        (Value::Text(text), luna_export::ExportFormat::Txt) => {
            luna_export::Exportable::Text(text.clone())
        }
        (Value::Text(_), _) => return Err(format!("RECIPE_EXPORT_NEEDS_TIMING: {format:?}")),
    };
    luna_export::ExportRegistry::with_builtin()
        .export(format, &exportable, &luna_export::ExportOptions::default())
        .map(|text| (text, format.extension()))
        .map_err(|error| format!("EXPORT_FAILED: {error}"))
}

/// 跑一个只回文字的 Job（交互泳道；严格本地时只走本机）。
async fn run_text_job(app: &tauri::AppHandle, kind: JobKind) -> Result<String, String> {
    let state = app.state::<AppState>();
    let mut job = Job::new(kind, JobOrigin::FloatingMenu);
    job.priority = Priority::Interactive;
    job.routing.local_only = state.strict_local();
    let mut handle = state.queue.submit(job);
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Text(text) => Ok(super::distill::normalize_model_output(&text)),
                    _ => Err("RECIPE_UNEXPECTED_RESULT".to_owned()),
                };
            }
            JobEvent::Failed(error) => {
                eprintln!("[recipe] step failed code={}", error.code());
                return Err(format!("RECIPE_STEP_FAILED: {}", error.code()));
            }
            JobEvent::Cancelled => return Err("CANCELLED".to_owned()),
            _ => {}
        }
    }
    Err("RECIPE_UNEXPECTED_RESULT".to_owned())
}

/// 文字超过这一步能处理的长度就停下（**纯函数**；不静默截断，PRD-06-025）。
fn within_limit(text: &str, limit: usize) -> Result<(), String> {
    let chars = text.chars().count();
    if chars > limit {
        return Err(format!("RECIPE_INPUT_TOO_LONG: {chars} > {limit}"));
    }
    if text.trim().is_empty() {
        return Err("RECIPE_EMPTY_INPUT".to_owned());
    }
    Ok(())
}

/// 找到要运行的那一份：包可用、从在线目录装的、配方在、确认过的能力与此刻推导出的一致（**纯函数**）。
fn find_recipe<'a>(
    packs: &'a [InstalledPack],
    pack: &str,
    recipe: &str,
    confirmed: &PackCapabilitiesDto,
) -> Result<(&'a InstalledPack, &'a Recipe), String> {
    let installed = packs
        .iter()
        .find(|installed| installed.contents.manifest.id.as_str() == pack)
        .ok_or_else(|| format!("RECIPE_NOT_FOUND: {pack}"))?;
    if !installed.installed_from_catalog() {
        return Err(format!("RECIPE_NOT_RUNNABLE: {pack}"));
    }
    let found = installed
        .contents
        .recipes
        .iter()
        .find(|item| item.id.as_str() == recipe)
        .ok_or_else(|| format!("RECIPE_NOT_FOUND: {recipe}"))?;
    if PackCapabilitiesDto::from(found.capabilities()) != *confirmed {
        return Err(format!("RECIPE_CONFIRM_AGAIN: {recipe}"));
    }
    Ok((installed, found))
}

/// 在一篇笔记（或录音）上运行一份配方（PRD-18-003 / 010）。
///
/// `confirmed`：用户在确认框里看过的能力（`RecipeDto.capabilities` 原样交回）。
///
/// # Errors
/// `RECIPE_NOT_FOUND` / `RECIPE_NOT_RUNNABLE`（本地导入或已被封禁）/ `RECIPE_CONFIRM_AGAIN`（包变了）/
/// `RECIPE_WRONG_INPUT`（录音配方用在了普通笔记上）/ `RECIPE_NO_TRANSCRIPT` / `RECIPE_EMPTY_INPUT` /
/// `RECIPE_INPUT_TOO_LONG` / `RECIPE_STEP_FAILED` / `RECIPE_UNKNOWN_FORMAT` / `RECIPE_EXPORT_NEEDS_TIMING`，
/// 库未打开与写盘失败。前面的步骤已经写下的笔记不会被撤回（它是一篇普通笔记，可以直接删）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn run_recipe(
    app: tauri::AppHandle,
    window: tauri::Window,
    note_rel: String,
    pack: String,
    recipe: String,
    confirmed: PackCapabilitiesDto,
) -> Result<RecipeRunDto, String> {
    let packs = usable_template_packs(&app);
    let (installed, recipe) = find_recipe(&packs, &pack, &recipe, &confirmed)?;
    let state = app.state::<AppState>();
    let limit = super::floating_input_limit(state.chat_ctx_window());
    // 副库窗口里点的就在副库里读、在副库里建（ADR 同 `with_vault`：笔记路径按窗口路由）。
    let (mut value, stem) = state.with_vault(window.label(), |vault| {
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        let stem = super::note_stem_of(&note_rel);
        let value = match recipe.input() {
            RecipeInput::Recording => {
                let mut segments = super::playback::collect_note_segments(vault.root(), &doc.meta);
                if segments.is_empty() {
                    return Err(if doc.meta.media.is_empty() {
                        "RECIPE_WRONG_INPUT".to_owned()
                    } else {
                        "RECIPE_NO_TRANSCRIPT".to_owned()
                    });
                }
                let i18n = backend_copy_arc(&app);
                super::playback::resolve_segment_speakers(
                    &mut segments,
                    &doc.meta.speakers,
                    i18n.as_deref(),
                );
                Value::Transcript(segments)
            }
            RecipeInput::Note => Value::Text(luna_vault::body_of(&doc.content).trim().to_owned()),
        };
        Ok((value, stem))
    })?;
    let mut run = RecipeRunDto::default();
    for step in &recipe.steps {
        match step {
            // 转写稿已在手里（见模块说明）。
            RecipeStep::Transcribe => {}
            RecipeStep::Translate { to } => {
                let text = value.text();
                within_limit(&text, limit)?;
                value = Value::Text(
                    run_text_job(
                        &app,
                        JobKind::Translate {
                            text: TextRef::Inline(text),
                            source: None,
                            target: Lang::from(to.as_str()),
                            instruction: None,
                            template: None,
                        },
                    )
                    .await?,
                );
            }
            RecipeStep::Llm { prompt } => {
                let text = value.text();
                within_limit(&text, limit)?;
                let instruction_text = installed
                    .read_asset(prompt)
                    .ok_or_else(|| format!("RECIPE_NOT_FOUND: {}", prompt.as_str()))?;
                let instruction = luna_core::prompt::registry().instruction(
                    luna_core::PromptScope::Editor,
                    "ask.selection",
                    &[("instruction", instruction_text.trim()), ("context", "")],
                );
                value = Value::Text(
                    run_text_job(
                        &app,
                        JobKind::Refine {
                            text: TextRef::Inline(text),
                            instruction: Some(instruction),
                        },
                    )
                    .await?,
                );
            }
            RecipeStep::VaultCreateNote { dir } => {
                let body = value.text();
                let rel = format!("{}/{stem} · {}.md", dir.as_str(), recipe.title);
                let doc = state.with_vault(window.label(), |vault| {
                    super::create_unique_note_with(
                        state.copy(),
                        vault,
                        &rel,
                        NoteType::Note,
                        |_| body.clone(),
                    )
                })?;
                emit_vault_changed_in_scope(
                    &app,
                    &doc.path,
                    false,
                    VaultChangeScope::for_window(window.label()),
                );
                run.created = Some(doc.path);
            }
            RecipeStep::Export { format } => {
                let (content, extension) = render_export(format, &value)?;
                let file_name = format!("{stem} · {}.{extension}", recipe.title);
                let Some(dest) =
                    super::pick_export_destination(&app, &file_name, extension).await?
                else {
                    // 用户在保存对话框里取消：这一步跳过，前面写下的笔记照常留着。
                    continue;
                };
                std::fs::write(&dest, content)
                    .map_err(|error| format!("EXPORT_WRITE_FAILED: {error}"))?;
                run.exported = Some(dest.to_string_lossy().into_owned());
            }
        }
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(text: &str, speaker: Option<&str>, start_ms: u64) -> luna_core::Segment {
        luna_core::Segment {
            id: luna_core::SegmentId::new(),
            start: std::time::Duration::from_millis(start_ms),
            end: std::time::Duration::from_millis(start_ms + 900),
            text: text.to_owned(),
            translation: None,
            speaker: speaker.map(str::to_owned),
            confidence: None,
            words: None,
        }
    }

    /// 转写稿交给模型时带上说话人；字幕类导出只在时间轴还在时可做，Markdown 与纯文本随时可做。
    #[test]
    fn transcripts_keep_speakers_and_subtitles_need_the_timeline() {
        let transcript = Value::Transcript(vec![
            segment("开场", Some("主持人"), 0),
            segment(" 好的 ", None, 1_000),
        ]);
        assert_eq!(transcript.text(), "主持人: 开场\n好的");
        let (srt, extension) = render_export("srt", &transcript).unwrap();
        assert_eq!(extension, "srt");
        assert!(srt.contains("-->"));

        let text = Value::Text("# 标题\n正文".to_owned());
        assert_eq!(
            render_export("md", &text).unwrap(),
            ("# 标题\n正文".to_owned(), "md")
        );
        assert_eq!(render_export("txt", &text).unwrap().1, "txt");
        assert!(render_export("srt", &text)
            .unwrap_err()
            .starts_with("RECIPE_EXPORT_NEEDS_TIMING"));
        assert!(render_export("docx", &text)
            .unwrap_err()
            .starts_with("RECIPE_UNKNOWN_FORMAT"));
    }

    /// 只有从在线目录装的包能运行配方（ADR-161 ②）；确认过的能力与此刻推导出的不一致就要重新确认。
    #[test]
    fn only_catalog_packs_run_and_a_changed_recipe_needs_confirming_again() {
        let (_root, pack) = crate::ipc::template_library::tests::installed_full_pack();
        let packs = vec![pack.clone()];
        assert!(runnable(&packs).is_empty(), "本地导入的包只展示");
        let caps = PackCapabilitiesDto {
            uses_ai: true,
            transcribes: false,
            translates: false,
            creates_notes_in: vec!["Projects".to_owned()],
            exports: false,
        };
        assert!(find_recipe(&packs, "com.example.kit", "publish", &caps)
            .unwrap_err()
            .starts_with("RECIPE_NOT_RUNNABLE"));

        crate::ipc::template_library::mark_from_catalog(pack.dir(), "sha").unwrap();
        let listed = runnable(&packs);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].input, RecipeInputDto::Note);
        assert_eq!(listed[0].capabilities, caps);
        assert_eq!(
            listed[0].steps,
            vec![
                RecipeStepDto::Llm {
                    prompt: "script".into()
                },
                RecipeStepDto::VaultCreateNote {
                    dir: "Projects".into()
                },
            ]
        );
        assert!(find_recipe(&packs, "com.example.kit", "publish", &caps).is_ok());
        let quieter = PackCapabilitiesDto {
            creates_notes_in: Vec::new(),
            ..caps
        };
        assert!(find_recipe(&packs, "com.example.kit", "publish", &quieter)
            .unwrap_err()
            .starts_with("RECIPE_CONFIRM_AGAIN"));
    }

    #[test]
    fn long_or_empty_input_stops_instead_of_being_truncated() {
        assert!(within_limit("一二三", 3).is_ok());
        assert!(within_limit("一二三四", 3)
            .unwrap_err()
            .starts_with("RECIPE_INPUT_TOO_LONG"));
        assert_eq!(within_limit("  \n", 10).unwrap_err(), "RECIPE_EMPTY_INPUT");
    }
}
