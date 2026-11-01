//! 项目研究（ADR-167 ⑥，PRD-19-046）：来源指南与研究简报。
//!
//! 两件事都走同一条 AI 主干（`run_summarize_job`：`AIProvider + JobQueue + ResultSchema`，ADR-001），
//! 提示词归创作应用（`creator.research.guide` / `creator.research.brief`，ADR-166 ⑤）。解析、渲染与
//! 缓存在 `luna_vault::research`；这里只读来源、按上下文窗口分配字数、调模型、落盘。
//!
//! - 来源指南：一条来源的摘要与要点，按「路径 + 正文指纹」缓存在库里，正文没变就不再调模型。
//! - 研究简报：选中的来源编号喂给模型，渲染成项目文件夹里的一篇新笔记（`<项目>-研究简报`，重名
//!   追加序号，绝不覆盖已有的那篇）；每条发现后面链回它出自的来源。
//! - 从简报出发：读最新一份研究简报，提三个一句话选题（选一个写进主页「选题」一节），或起草一版
//!   脚本——项目脚本还是空骨架就写进它，否则另起一篇「脚本初稿」，绝不覆盖用户写过的脚本。

use super::*;
use luna_vault::research::{
    clean_script_draft, parse_research_brief, parse_source_guide, parse_topics,
    render_research_brief, script_is_blank, source_text, BriefCopy,
};

/// 来源指南的输出预算（摘要 + 至多八条要点）。
const GUIDE_OUTPUT_TOKENS: u32 = 700;
/// 研究简报的输出预算。
const BRIEF_OUTPUT_TOKENS: u32 = 1_800;
/// 一份简报最多读几条来源（再多，每条分到的字数就太少，模型只能看到开头）。
const BRIEF_MAX_SOURCES: usize = 12;
/// 提选题的输出预算。
const TOPICS_OUTPUT_TOKENS: u32 = 600;
/// 起草脚本的输出预算。
const SCRIPT_OUTPUT_TOKENS: u32 = 2_500;
/// 写进「选题」一节的文字最多多少字。
const TOPIC_MAX_CHARS: usize = 500;

/// 一条来源的指南。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SourceGuideDto {
    pub summary: String,
    pub points: Vec<String>,
    /// 来自缓存（正文没变，没有再调模型）。
    pub cached: bool,
}

/// 此刻的输入预算（字数）：按探测到的上下文窗口。
async fn input_limit(app: &tauri::AppHandle) -> usize {
    super::chat::ensure_chat_context_window(app).await;
    floating_input_limit(app.state::<AppState>().chat_ctx_window())
}

/// 「来源指南」：一条来源的摘要与要点（ADR-167 ⑥）。`refresh` = 不用缓存、重新生成。
///
/// # Errors
/// 创作未启用、读不出这一篇或它没有正文（`CREATOR_RESEARCH_EMPTY`）、模型失败或回复解析不了时
/// 返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_source_guide(
    app: tauri::AppHandle,
    window: tauri::Window,
    path: String,
    refresh: bool,
) -> Result<SourceGuideDto, String> {
    let state = app.state::<AppState>();
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let content = state.with_vault(window.label(), |vault| {
        vault.read(&path).map(|doc| doc.content).map_err(vault_err)
    })?;
    if !refresh {
        let cached = state.with_vault(window.label(), |vault| {
            Ok(vault.cached_source_guide(&path, &content))
        })?;
        if let Some(guide) = cached {
            return Ok(SourceGuideDto {
                summary: guide.summary,
                points: guide.points,
                cached: true,
            });
        }
    }
    let text = source_text(&content, input_limit(&app).await);
    if text.is_empty() {
        return Err(coded_error(
            &app,
            "CREATOR_RESEARCH_EMPTY",
            "creator-research-empty",
        ));
    }
    let output = super::creator_publish::run_summarize_job(
        &app,
        "creator.research.guide",
        text,
        GUIDE_OUTPUT_TOKENS,
    )
    .await?;
    let guide = parse_source_guide(&output)
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;
    // 缓存写不成不影响这一次的结果（下次再生成一遍而已）。
    let _ = state.with_vault(window.label(), |vault| {
        vault
            .remember_source_guide(&path, &content, &guide)
            .map_err(vault_err)
    });
    Ok(SourceGuideDto {
        summary: guide.summary,
        points: guide.points,
        cached: false,
    })
}

/// 喂给模型的来源清单：`[编号] 标题` + 正文（每条分到同样多的字数）。读不出或没有正文的跳过。
fn brief_input(
    vault: &Vault,
    paths: &[String],
    limit_chars: usize,
) -> (Vec<(String, String)>, String) {
    let per_source = (limit_chars / paths.len().max(1)).max(400);
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut text = String::new();
    for path in paths {
        let Ok(doc) = vault.read(path) else {
            continue;
        };
        let body = source_text(&doc.content, per_source);
        if body.is_empty() {
            continue;
        }
        let title = note_stem_of(path);
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("[{}] {title}\n{body}", sources.len() + 1));
        sources.push((path.clone(), title));
    }
    (sources, text)
}

/// 「写研究简报」：把选中的来源合成一篇简报，建在项目文件夹里，返回新笔记的库相对路径。
///
/// # Errors
/// 创作未启用、`home` 不是视频项目主页、没有选来源或选了太多（`CREATOR_RESEARCH_PICK`）、选中的
/// 都没有正文（`CREATOR_RESEARCH_EMPTY`）、模型失败、回复解析不了或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_research_brief(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
    paths: Vec<String>,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    // 确认它是项目主页（顺带取出素材文件夹的位置，不用它）。
    super::creator_quote::material_target(&app, &window, &state, &home)?;
    if paths.is_empty() || paths.len() > BRIEF_MAX_SOURCES {
        return Err(coded_error_args(
            &app,
            "CREATOR_RESEARCH_PICK",
            "creator-research-pick",
            &[("max", BRIEF_MAX_SOURCES.to_string())],
        ));
    }
    let limit = input_limit(&app).await;
    let (sources, text) = state.with_vault(window.label(), |vault| {
        Ok(brief_input(vault, &paths, limit))
    })?;
    if sources.is_empty() {
        return Err(coded_error(
            &app,
            "CREATOR_RESEARCH_EMPTY",
            "creator-research-empty",
        ));
    }
    let output = super::creator_publish::run_summarize_job(
        &app,
        "creator.research.brief",
        text,
        BRIEF_OUTPUT_TOKENS,
    )
    .await?;
    let brief = parse_research_brief(&output, sources.len())
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;

    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let project = note_stem_of(&home);
    let word = backend_text_with(copy, "creator-brief-file", "研究简报");
    let title = format!("{project} {word}");
    let based_on = backend_text_with(copy, "creator-brief-based-on", "依据：");
    let separator = backend_text_with(copy, "creator-brief-separator", "、");
    let overview = backend_text_with(copy, "creator-brief-overview", "概述");
    let findings = backend_text_with(copy, "creator-brief-findings", "要点");
    let questions = backend_text_with(copy, "creator-brief-questions", "待查的问题");
    let folder = super::creator_publish::project_folder(&home).to_owned();
    let note_rel = state.with_vault(window.label(), |vault| {
        let note_rel =
            super::distill::unique_note_path_in(vault, &folder, &format!("{project}-{word}"));
        let markdown = render_research_brief(
            &brief,
            &note_rel,
            &sources,
            BriefCopy {
                title: &title,
                based_on: &based_on,
                separator: &separator,
                overview: &overview,
                findings: &findings,
                questions: &questions,
            },
        );
        vault.write(&note_rel, &markdown).map_err(vault_err)?;
        Ok(note_rel)
    })?;
    emit_vault_changed(&app, &note_rel, false);
    Ok(note_rel)
}

/// 项目最新的一份研究简报：`(路径, 全文)`；还没有就报 `CREATOR_RESEARCH_NO_BRIEF`。
fn latest_brief(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    state: &AppState,
    home: &str,
) -> Result<(String, String), String> {
    super::creator_quote::material_target(app, window, state, home)?;
    let i18n = backend_copy_arc(app);
    let word = backend_text_with(i18n.as_deref(), "creator-brief-file", "研究简报");
    let stem = format!("{}-{word}", note_stem_of(home));
    let folder = super::creator_publish::project_folder(home).to_owned();
    let found = state.with_vault(window.label(), |vault| {
        let Some(path) = vault
            .latest_research_brief(&folder, &stem)
            .map_err(vault_err)?
        else {
            return Ok(None);
        };
        let content = vault.read(&path).map_err(vault_err)?.content;
        Ok(Some((path, content)))
    })?;
    found.ok_or_else(|| {
        coded_error(
            app,
            "CREATOR_RESEARCH_NO_BRIEF",
            "creator-research-no-brief",
        )
    })
}

/// 主页里「选题」一节现在写的是什么；还是骨架里那句提示就当没写。
fn current_topic(content: &str, copy: BackendCopy<'_>) -> Option<String> {
    let heading = backend_text_with(copy, "creator-section-idea", "选题");
    let hint = backend_text_with(
        copy,
        "creator-section-idea-hint",
        "一句话说清这期视频讲什么、讲给谁。",
    );
    let body = luna_vault::section_body(content, &heading)?;
    let body = body.trim();
    (!body.is_empty() && body != hint).then(|| body.to_owned())
}

/// 模型提的一个选题。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TopicIdeaDto {
    pub title: String,
    pub angle: String,
}

/// 「用简报提选题」：读最新一份研究简报，提三个一句话选题（ADR-167 ⑥）。
///
/// # Errors
/// 创作未启用、不是项目主页、还没有研究简报（`CREATOR_RESEARCH_NO_BRIEF`）、模型失败或回复解析
/// 不了时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_brief_topics(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
) -> Result<Vec<TopicIdeaDto>, String> {
    let state = app.state::<AppState>();
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let (_, brief) = latest_brief(&app, &window, &state, &home)?;
    let text = source_text(&brief, input_limit(&app).await);
    let output = super::creator_publish::run_summarize_job(
        &app,
        "creator.research.topics",
        text,
        TOPICS_OUTPUT_TOKENS,
    )
    .await?;
    let topics = parse_topics(&output)
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;
    Ok(topics
        .into_iter()
        .map(|topic| TopicIdeaDto {
            title: topic.title,
            angle: topic.angle,
        })
        .collect())
}

/// 把选定的选题写进项目主页的「选题」一节（整节换成这一句；其余各节不动）。
///
/// # Errors
/// 创作未启用、不是项目主页、文字为空或太长（`CREATOR_TOPIC_TEXT`）、主页写入期间一直在变或写盘
/// 失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_set_topic(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    text: String,
) -> Result<(), String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    super::creator_quote::material_target(&app, &window, &state, &home)?;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.chars().count() > TOPIC_MAX_CHARS {
        return Err(coded_error(
            &app,
            "CREATOR_TOPIC_TEXT",
            "creator-topic-text",
        ));
    }
    let i18n = backend_copy_arc(&app);
    let heading = backend_text_with(i18n.as_deref(), "creator-section-idea", "选题");
    let content = state.with_vault(window.label(), |vault| {
        for _ in 0..2 {
            let base = vault.read(&home).map_err(vault_err)?.content;
            let next = luna_vault::upsert_section(&base, &heading, &text);
            if matches!(
                vault.save_note(&home, &base, &next).map_err(vault_err)?,
                luna_vault::NoteSave::Saved { .. }
            ) {
                return Ok(next);
            }
        }
        Err("项目主页在写入期间一直在变".to_owned())
    })?;
    let _ = (NoteWritten {
        path: home,
        content,
        origin: "main".to_owned(),
    })
    .emit(&app);
    Ok(())
}

/// 起草好的脚本落在哪。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ScriptDraftDto {
    pub path: String,
    /// 写进了项目脚本（它原本还是空骨架）；否则是另起的一篇「脚本初稿」。
    pub into_script: bool,
}

/// 「用简报起草脚本」：以主页的选题与最新一份研究简报为依据起草一版口播脚本（ADR-167 ⑥）。
///
/// # Errors
/// 创作未启用、不是项目主页、还没有研究简报、模型失败、回复里没有分段或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_draft_script(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
) -> Result<ScriptDraftDto, String> {
    let state = app.state::<AppState>();
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let (_, brief) = latest_brief(&app, &window, &state, &home)?;
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let home_content = state.with_vault(window.label(), |vault| {
        vault.read(&home).map(|doc| doc.content).map_err(vault_err)
    })?;
    let topic = current_topic(&home_content, copy).unwrap_or_else(|| note_stem_of(&home));
    let limit = input_limit(&app).await;
    let text = format!(
        "Topic: {topic}\n\nResearch brief:\n{}",
        source_text(&brief, limit)
    );
    let hook = backend_text_with(copy, "creator-script-hook", "开头");
    let beat = copy
        .and_then(|i18n| i18n.translate_args("creator-script-beat", &[("n", "1".to_owned())]))
        .unwrap_or_else(|| "节拍 1".to_owned());
    let outro = backend_text_with(copy, "creator-script-outro", "结尾");
    let output = super::creator_publish::run_summarize_job_with(
        &app,
        "creator.research.script",
        &[
            ("hook", hook.as_str()),
            ("beat", beat.as_str()),
            ("outro", outro.as_str()),
        ],
        &super::creator_publish::CreatorNotes::default(),
        text,
        SCRIPT_OUTPUT_TOKENS,
    )
    .await?;
    let draft = clean_script_draft(&output)
        .ok_or_else(|| coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse"))?;
    let preferred = super::creator::project_child_rel(&home, copy, "creator-note-script", "脚本");
    let folder = super::creator_publish::project_folder(&home).to_owned();
    let draft_word = backend_text_with(copy, "creator-script-draft", "脚本初稿");
    let project = note_stem_of(&home);
    let (path, written) = state.with_vault(window.label(), |vault| {
        // 项目脚本还是空骨架：写进它（保留 frontmatter）；写过字了就另起一篇，绝不覆盖。
        if let Some(script) = vault.project_script(&home, &preferred).map_err(vault_err)? {
            let base = vault.read(&script).map_err(vault_err)?.content;
            if script_is_blank(&base) {
                let body = luna_vault::frontmatter::body_of(&base);
                let next = format!("{}{draft}", &base[..base.len() - body.len()]);
                if matches!(
                    vault.save_note(&script, &base, &next).map_err(vault_err)?,
                    luna_vault::NoteSave::Saved { .. }
                ) {
                    return Ok((script, Some(next)));
                }
            }
        }
        let path =
            super::distill::unique_note_path_in(vault, &folder, &format!("{project}-{draft_word}"));
        vault
            .create(
                &path,
                luna_vault::NoteType::Script,
                &luna_vault::TemplateVars {
                    body: draft.clone(),
                    ..luna_vault::TemplateVars::default()
                },
            )
            .map_err(vault_err)?;
        Ok((path, None))
    })?;
    let into_script = written.is_some();
    if let Some(content) = written {
        // 脚本可能正开在编辑器里：照主页写选题一样广播新正文。
        let _ = (NoteWritten {
            path: path.clone(),
            content,
            origin: "main".to_owned(),
        })
        .emit(&app);
    } else {
        emit_vault_changed(&app, &path, false);
    }
    Ok(ScriptDraftDto { path, into_script })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skeleton_hint_does_not_count_as_a_topic() {
        let skeleton = "## 选题\n\n一句话说清这期视频讲什么、讲给谁。\n\n## 资料\n";
        assert_eq!(current_topic(skeleton, None), None);
        let written = "## 选题\n\n讲给新手的一周发布法。\n\n## 资料\n";
        assert_eq!(
            current_topic(written, None).as_deref(),
            Some("讲给新手的一周发布法。")
        );
    }

    #[test]
    fn brief_input_numbers_readable_sources_and_skips_empty_ones() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("Notes/甲.md", "第一条来源的内容\n").unwrap();
        vault
            .write("Notes/空.md", "---\nluna:\n  type: note\n---\n")
            .unwrap();
        vault.write("Notes/乙.md", "第二条\n").unwrap();
        let paths = vec![
            "Notes/甲.md".to_owned(),
            "Notes/空.md".to_owned(),
            "Notes/不存在.md".to_owned(),
            "Notes/乙.md".to_owned(),
        ];
        let (sources, text) = brief_input(&vault, &paths, 10_000);
        assert_eq!(
            sources,
            [
                ("Notes/甲.md".to_owned(), "甲".to_owned()),
                ("Notes/乙.md".to_owned(), "乙".to_owned()),
            ]
        );
        assert_eq!(text, "[1] 甲\n第一条来源的内容\n\n[2] 乙\n第二条");
    }
}
