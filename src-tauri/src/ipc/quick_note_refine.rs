//! 语音速记落盘后的 **AI 提炼**（PRD-06-058 C′）：标题、待办复选框、可能的日期——写回该速记
//! 正文本身（复选框列表 + frontmatter），不新建对象、不新增存储。
//!
//! - **走单一 AI 主干**（ADR-001）：`JobKind::Summarize` + 只回 JSON 的指令，不新增 JobKind；
//!   结果经本模块的 [`Refinement`]（ResultSchema）校验后才落盘。
//! - **默认关、失败静默**：开关 `voice_input.quick_note_refine` 默认关闭（默认离线、显式可见）；
//!   模型不可用 / 回得不像 JSON / 用户已改过正文，一律保留原稿，不打扰、不重试。
//! - **隐私铁律**：本模块不写任何日志——速记正文不能进日志。

use super::*;
use luna_core::{JobEvent, JobKind, JobOrigin, Priority, ResultPayload, SummaryStyle, TextRef};
use luna_vault::{body_of, read_luna, update_luna, LunaMeta};

/// 提炼结果（ResultSchema）：模型只允许回这一种形状。
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct Refinement {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub todos: Vec<String>,
    #[serde(default)]
    pub dates: Vec<String>,
}

/// 标题上限（字符）：速记标题是一行摘要，不是段落。
const TITLE_MAX_CHARS: usize = 60;
/// 待办条数上限：一条速记不该炸出一整张任务表。
const TODOS_MAX: usize = 10;
/// 提炼 Job 的输出预算（速记很短；预算给宽是为了 JSON 不被截断）。
const REFINE_OUTPUT_TOKENS: u32 = 512;

/// 提炼指令归注册表 `quick_note.refine`（ADR-050 修订 2026-09-05）；`{today}` 是唯一变量。
/// JSON 形状那段花括号在模板里原样保留——单趟渲染只认已登记的占位符名。
fn instruction(today: &str) -> luna_core::Instruction {
    luna_core::prompt::registry().instruction(
        luna_core::PromptScope::QuickNote,
        "refine",
        &[("today", today)],
    )
}

/// 送给模型的正文（纯函数）。
///
/// 语音速记落盘的形态是「## 优化结果 / ## 原始转录」两节（`voice_quick_note_body`）：两节
/// 是同一段话的两个版本，一起送只会让模型把小标题「优化结果」当成标题回来（真机 2026-09-03）。
/// 有分节就只取**第一节**的正文；没有分节就整篇去掉标题行。
pub(crate) fn refine_source_text(body: &str) -> String {
    let mut lines = body.lines().peekable();
    let sectioned = lines
        .peek()
        .is_some_and(|first| first.trim_start().starts_with("## "));
    let mut out = String::new();
    let mut in_first_section = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            if sectioned {
                if in_first_section {
                    break;
                }
                in_first_section = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

/// 正文里已有的标题 / 小标题文字（去掉井号），标题候选与之相同即拒收。
fn heading_texts(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 从模型原文里抠出 JSON 对象并按 [`Refinement`] 校验；不像 JSON 就是 `None`。
pub(crate) fn parse_refinement(raw: &str) -> Option<Refinement> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let parsed: Refinement = serde_json::from_str(&raw[start..=end]).ok()?;
    Some(sanitize(parsed))
}

fn sanitize(r: Refinement) -> Refinement {
    let title = r
        .title
        .map(|t| t.lines().next().unwrap_or("").trim().to_owned())
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(TITLE_MAX_CHARS).collect::<String>());
    let todos = r
        .todos
        .into_iter()
        .map(|t| {
            t.trim()
                .trim_start_matches(['-', '*', '•'])
                .trim()
                .to_owned()
        })
        .filter(|t| !t.is_empty())
        .take(TODOS_MAX)
        .collect();
    let dates = r
        .dates
        .into_iter()
        .map(|d| d.trim().to_owned())
        .filter(|d| is_iso_date(d))
        .collect();
    Refinement {
        title,
        todos,
        dates,
    }
}

/// `YYYY-MM-DD` 的形状校验（不解析历法：模型给 `2026-02-30` 也只是一个待核对的线索）。
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit())
}

/// 把提炼结果写进这一篇（纯函数）：无标题时补 `# 标题`、待办作复选框列表追加到正文末尾、
/// `luna.dates` 与 `luna.refined_at` 进 frontmatter。已提炼过的笔记恒返回 `None`（幂等）。
pub(crate) fn apply_refinement(
    content: &str,
    refinement: &Refinement,
    todo_heading: &str,
    now_rfc3339: &str,
) -> Option<String> {
    let mut meta: LunaMeta = read_luna(content);
    if meta.refined_at.is_some() {
        return None;
    }
    let body = body_of(content).trim_end();
    let mut next = String::new();
    // 只认**一级标题**：语音速记的正文天然以「## 优化结果」开头，那是分节不是标题，
    // 否则每一篇语音速记都永远补不上标题。
    let has_heading = body
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.trim_start().starts_with("# "));
    // 模型把正文里的小标题（「优化结果」）当标题回来是常见失误：与任一标题行同文即拒收。
    let headings = heading_texts(body);
    let title = refinement
        .title
        .as_deref()
        .filter(|t| !headings.iter().any(|h| h.eq_ignore_ascii_case(t.trim())));
    if let (Some(title), false) = (title, has_heading) {
        next.push_str("# ");
        next.push_str(title);
        next.push_str("\n\n");
    }
    next.push_str(body);
    if !refinement.todos.is_empty() {
        next.push_str("\n\n## ");
        next.push_str(todo_heading);
        next.push('\n');
        for todo in &refinement.todos {
            next.push_str("- [ ] ");
            next.push_str(todo);
            next.push('\n');
        }
    }
    if !next.ends_with('\n') {
        next.push('\n');
    }
    meta.refined_at = Some(now_rfc3339.to_owned());
    meta.dates = refinement.dates.clone();
    update_luna(&next, &meta).ok()
}

/// 后台跑一次提炼并写回（由语音速记落盘处 spawn；开关已由调用方判过）。
///
/// 三处「不写」：模型没回 / 回得不像 JSON；速记在提炼期间被用户改过（正文快照不再相等）；
/// 已经提炼过。任何一处都静默保留原稿。
pub(crate) async fn refine_quick_note(app: tauri::AppHandle, note_rel: String) {
    let state = app.state::<AppState>();
    let snapshot = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        let Ok(doc) = vault.read(&note_rel) else {
            return;
        };
        doc.content
    };
    if read_luna(&snapshot).refined_at.is_some() {
        return;
    }
    let text = refine_source_text(body_of(&snapshot));
    if text.is_empty() {
        return;
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    // `Faithful` 才把指令**原样**当系统提示（`SummaryStyle::uses_raw_instruction`）；
    // 其它风格会把指令嵌进「总结成要点」模板，模型于是回一串要点而不是 JSON——真机上
    // 表现为开关开着、速记却一动不动。
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(text),
            style: SummaryStyle::Faithful,
            instruction: Some(instruction(&today)),
        },
        // `Batch` 而不是 `VoiceInput`：后者按 Realtime 服务档路由（结果要立刻注入光标处），
        // 而提炼发生在落盘之后、没有任何人在等——占实时泳道既抢别人的额度，托管端也可能
        // 因实时会话额度不足而拒绝它。
        JobOrigin::Batch,
    );
    job.priority = Priority::Background;
    job.routing.local_only = state.strict_local();
    job.options.output_budget = Some(REFINE_OUTPUT_TOKENS);
    let mut handle = state.queue.submit(job);
    let mut output: Option<String> = None;
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                if let ResultPayload::Text(text) = result.payload {
                    output = Some(super::distill::normalize_model_output(&text));
                }
                break;
            }
            JobEvent::Failed(err) => {
                // 只记结果码——速记正文与模型输出都不进日志（隐私铁律）。
                eprintln!("[quick-note-refine] job failed code={}", err.code());
                break;
            }
            JobEvent::Cancelled => break,
            _ => {}
        }
    }
    let Some(refinement) = output.as_deref().and_then(parse_refinement) else {
        if output.is_some() {
            eprintln!("[quick-note-refine] skipped: model output was not the expected JSON");
        }
        return;
    };
    let heading = backend_copy_arc(&app)
        .and_then(|i18n| i18n.translate("quick-note-refine-todos"))
        .unwrap_or_else(|| "To-do".to_owned());
    let now = chrono::Local::now().to_rfc3339();
    let written = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        let Ok(doc) = vault.read(&note_rel) else {
            return;
        };
        if doc.content != snapshot {
            eprintln!("[quick-note-refine] skipped: note changed while refining");
            return;
        }
        let Some(next) = apply_refinement(&doc.content, &refinement, &heading, &now) else {
            return;
        };
        vault.write(&note_rel, &next).is_ok()
    };
    if written {
        emit_vault_changed(&app, &note_rel, false);
        super::notes::refresh_derived_after_write(&app, &note_rel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refine_job_puts_the_json_instruction_verbatim_as_the_system_prompt() {
        // Faithful 风格才把指令原样当系统提示；其它风格会把它嵌进「总结成要点」模板，
        // 模型回的就是要点不是 JSON（真机 2026-09-03：开关开着、速记一动不动）。本机与
        // 托管两条路径共用这一个组装器，这里测的就是两边都会发出去的那份消息。
        let job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline("周三开会，记得通知小李".into()),
                style: SummaryStyle::Faithful,
                instruction: Some(instruction("2026-09-03")),
            },
            JobOrigin::Batch,
        );
        let assembled = luna_provider::build_chat_messages(luna_core::prompt::registry(), &job);
        assert_eq!(assembled.messages.len(), 2);
        assert_eq!(assembled.messages[0].role, "system");
        assert!(assembled.messages[0]
            .content
            .contains("reply with ONLY one JSON object"));
        assert!(assembled.messages[0].content.contains("today = 2026-09-03"));
        assert_eq!(assembled.messages[1].role, "user");
        assert_eq!(assembled.messages[1].content, "周三开会，记得通知小李");
        assert_eq!(job.service_class(), luna_core::ServiceClass::Batch);
    }

    #[test]
    fn parses_json_even_when_the_model_wraps_it_in_prose_or_fences() {
        let raw = "Sure!\n```json\n{\"title\":\"周三改会\",\"todos\":[\"- 通知小李\",\" \"],\"dates\":[\"2026-09-10\",\"下周三\"]}\n```";
        let r = parse_refinement(raw).unwrap();
        assert_eq!(r.title.as_deref(), Some("周三改会"));
        assert_eq!(r.todos, vec!["通知小李"]);
        assert_eq!(r.dates, vec!["2026-09-10"]);
        assert!(parse_refinement("no json here").is_none());
        assert!(parse_refinement("{\"title\": 3}").is_none());
    }

    #[test]
    fn caps_title_and_todo_counts_and_keeps_only_iso_dates() {
        let r = sanitize(Refinement {
            title: Some("第一行\n第二行".into()),
            todos: (0..20).map(|i| format!("t{i}")).collect(),
            dates: vec!["2026-9-1".into(), "2026-09-01".into()],
        });
        assert_eq!(r.title.as_deref(), Some("第一行"));
        assert_eq!(r.todos.len(), TODOS_MAX);
        assert_eq!(r.dates, vec!["2026-09-01"]);
    }

    #[test]
    fn writes_title_todos_and_frontmatter_back_into_the_same_note_and_is_idempotent() {
        let content = "---\nluna:\n  schema: 1\n  type: quick-note\n---\n开会改到下周三下午三点，记得通知小李。\n";
        let r = Refinement {
            title: Some("开会改期".into()),
            todos: vec!["通知小李".into()],
            dates: vec!["2026-09-09".into()],
        };
        let next = apply_refinement(content, &r, "待办", "2026-09-03T10:00:00+08:00").unwrap();
        let body = body_of(&next);
        assert!(body.starts_with("# 开会改期\n\n开会改到下周三"), "{body}");
        assert!(body.contains("\n## 待办\n- [ ] 通知小李\n"), "{body}");
        let meta = read_luna(&next);
        assert_eq!(
            meta.refined_at.as_deref(),
            Some("2026-09-03T10:00:00+08:00")
        );
        assert_eq!(meta.dates, vec!["2026-09-09"]);
        assert_eq!(meta.note_type, luna_vault::NoteType::QuickNote);
        // 已提炼过：第二次恒不写（幂等）。
        assert!(apply_refinement(&next, &r, "待办", "2026-09-03T11:00:00+08:00").is_none());
    }

    #[test]
    fn a_voice_quick_note_that_opens_with_a_section_heading_still_gets_a_title() {
        let content = "---\nluna:\n  type: quick-note\n---\n## 优化结果\n\n今天下午两点去超市买水果。\n\n## 原始转录\n\n今天下午两点去超市买水果\n";
        let r = Refinement {
            title: Some("超市采购".into()),
            todos: vec!["去超市买水果".into()],
            dates: vec!["2026-09-03".into()],
        };
        let next = apply_refinement(content, &r, "待办", "2026-09-03T12:00:00+08:00").unwrap();
        let body = body_of(&next);
        assert!(body.starts_with("# 超市采购\n\n## 优化结果"), "{body}");
        assert!(
            body.ends_with(
                "## 原始转录\n\n今天下午两点去超市买水果\n\n## 待办\n- [ ] 去超市买水果\n"
            ),
            "{body}"
        );
    }

    #[test]
    fn the_model_only_sees_the_polished_section_and_a_heading_echo_is_not_a_title() {
        let body = "## 优化结果\n\n周三开会，通知小李。\n\n## 原始转录\n\n周三 开会 通知小李\n";
        assert_eq!(refine_source_text(body), "周三开会，通知小李。");
        assert_eq!(
            refine_source_text("# 标题\n第一行\n第二行\n"),
            "第一行\n第二行"
        );
        assert_eq!(refine_source_text("随手记\n"), "随手记");

        let content = format!("---\nluna:\n  type: quick-note\n---\n{body}");
        let r = Refinement {
            title: Some("优化结果".into()),
            todos: vec![],
            dates: vec![],
        };
        let next = apply_refinement(&content, &r, "待办", "2026-09-03T12:00:00+08:00").unwrap();
        assert!(
            body_of(&next).starts_with("## 优化结果"),
            "小标题回声不能变成一级标题"
        );
        assert!(!next.contains("# 优化结果\n\n## 优化结果"));
    }

    #[test]
    fn keeps_an_existing_heading_and_only_stamps_when_there_is_nothing_to_add() {
        let content = "---\nluna:\n  type: quick-note\n---\n# 我的标题\n正文\n";
        let r = Refinement {
            title: Some("别的标题".into()),
            todos: vec![],
            dates: vec![],
        };
        let next = apply_refinement(content, &r, "待办", "2026-09-03T10:00:00+08:00").unwrap();
        assert!(body_of(&next).starts_with("# 我的标题\n正文"));
        assert!(!next.contains("别的标题"));
        assert!(!next.contains("## 待办"));
        assert!(read_luna(&next).refined_at.is_some());
    }
}
