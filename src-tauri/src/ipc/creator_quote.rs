//! 项目研究笔记的两类自动写入：引文卡片（ADR-162 ⑭，PRD-19-007）——「存为引文」把选区记成
//! 「引文」一节里的一行，脚本里「插入引文」列出本项目的卡片、插入 `![[研究#^q-…]]` 嵌入；导入素材
//! （ADR-162 ⑯，PRD-19-006）——素材转写完成后在「素材」一节链上它。
//!
//! 引文怎么清理、时刻怎么取、属于哪个项目、卡片长什么样全在 Rust（`luna_vault::quote`）；
//! 前端只把选区与它在编辑器里的起始行交过来（ADR-003）。

use super::*;
use luna_vault::quote::{
    clock, locate_quote, new_quote_id, quote_cards, quote_line, quote_text, time_at_line,
};

/// 项目研究笔记的库相对路径：`<项目文件夹>/<项目>-<研究>.md`（与项目骨架同一命名）。
pub(crate) fn research_note_rel(home: &str, copy: BackendCopy<'_>) -> String {
    super::creator::project_child_rel(home, copy, "creator-note-research", "研究")
}

/// 一个项目研究笔记里有几条引文（看板卡片的信号，PRD-19-040）；没有研究笔记为 0。
pub(crate) fn project_quote_count(vault: &Vault, home: &str, copy: BackendCopy<'_>) -> usize {
    vault
        .read(&research_note_rel(home, copy))
        .map_or(0, |doc| quote_cards(&doc.content).len())
}

/// 存好的引文：写进了哪篇研究笔记、属于哪个项目。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SavedQuoteDto {
    pub research_path: String,
    pub project: String,
}

/// 「存为引文」（PRD-19-007）：选区 → 所属项目研究笔记「引文」一节末尾的一行卡片。`start_line`
/// 是选区起点在编辑器里的行（1 起，含 frontmatter）。
///
/// # Errors
/// 选区去掉时间码后为空、这篇不属于任何视频项目、写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_save_quote(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    source: String,
    start_line: u32,
    text: String,
) -> Result<SavedQuoteDto, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let quote = quote_text(&text);
    if quote.is_empty() {
        return Err(coded_error(
            &app,
            "CREATOR_QUOTE_EMPTY",
            "creator-quote-empty",
        ));
    }
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let quotes_heading = backend_text_with(copy, "creator-research-quotes", "引文");
    let sources_heading = backend_text_with(copy, "creator-research-sources", "来源");
    let dash = backend_text_with(copy, "creator-quote-dash", "——");
    let (research_rel, project, content, created) = state.with_vault(window.label(), |vault| {
        let project = vault
            .projects_of(&source)
            .map_err(vault_err)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                coded_error(&app, "CREATOR_QUOTE_NO_PROJECT", "creator-quote-no-project")
            })?;
        let research_rel = research_note_rel(&project.path, copy);
        let source_doc = vault.read(&source).map_err(vault_err)?;
        let line = locate_quote(&source_doc.content, start_line, &quote);
        let time = line.and_then(|line| time_at_line(&source_doc.content, line));
        let title = note_stem_of(&source);
        // PDF 资料：出处带页码（ADR-164 ⑧），跳转仍是转换稿里的那一行。
        let page = line
            .filter(|_| {
                source_doc
                    .meta
                    .original
                    .as_ref()
                    .is_some_and(|original| original.format == "pdf")
            })
            .and_then(|line| luna_vault::quote::page_at_line(&source_doc.content, line));
        let (label, fragment) = match (time, line, page) {
            (Some(ms), _, _) => (format!("{title} · {}", clock(ms)), format!("#t={ms}")),
            (None, Some(line), Some(page)) => {
                let page = copy
                    .and_then(|i18n| {
                        i18n.translate_args("creator-quote-page", &[("page", page.to_string())])
                    })
                    .unwrap_or_else(|| format!("第 {page} 页"));
                (format!("{title} · {page}"), format!("#L{line}"))
            }
            (None, Some(line), None) => (title.clone(), format!("#L{line}")),
            (None, None, _) => (title.clone(), String::new()),
        };
        let href = format!(
            "{}{fragment}",
            luna_vault::encode_link_target(&luna_vault::relative_link_path(&research_rel, &source))
        );
        for _ in 0..2 {
            let (base, created) = match vault.read(&research_rel) {
                Ok(doc) => (Some(doc.content), false),
                Err(_) => (None, true),
            };
            let existing = base
                .clone()
                .unwrap_or_else(|| format!("## {sources_heading}\n\n## {quotes_heading}\n"));
            let seed = uuid::Uuid::new_v4().as_u128();
            let card = quote_line(&quote, &label, &href, &dash, &new_quote_id(&existing, seed));
            let section = luna_vault::section_body(&existing, &quotes_heading).unwrap_or_default();
            let section = section.trim_end();
            let body = if section.is_empty() {
                card
            } else {
                format!("{section}\n\n{card}")
            };
            let next = luna_vault::upsert_section(&existing, &quotes_heading, &body);
            let saved = match base {
                Some(base) => matches!(
                    vault
                        .save_note(&research_rel, &base, &next)
                        .map_err(vault_err)?,
                    luna_vault::NoteSave::Saved { .. }
                ),
                None => {
                    vault.write(&research_rel, &next).map_err(vault_err)?;
                    true
                }
            };
            if saved {
                return Ok((research_rel, project.title, next, created));
            }
        }
        Err("研究笔记在写入期间一直在变".to_owned())
    })?;
    broadcast_note_write(&app, window.label(), &research_rel, content);
    if created {
        emit_vault_changed_in_scope(
            &app,
            &research_rel,
            false,
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(SavedQuoteDto {
        research_path: research_rel,
        project,
    })
}

/// 一张可插入的引文卡片。
#[derive(Debug, Clone, Serialize, Type)]
pub struct QuoteCardDto {
    /// 卡片所在的研究笔记。
    pub research_path: String,
    /// 卡片那一行（引文 + 出处，不含 `> ` 与块标记）。
    pub text: String,
    /// 插进脚本的嵌入（`![[研究笔记名#^q-…]]`）。
    pub embed: String,
}

/// 这篇笔记（通常是脚本）所属项目的全部引文卡片，项目按最近改动排、卡片按出现顺序。
///
/// # Errors
/// 库未打开或索引查询失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_project_quotes(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<QuoteCardDto>, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    state.with_vault(window.label(), |vault| {
        let mut cards = Vec::new();
        for project in vault.projects_of(&path).map_err(vault_err)? {
            let research_rel = research_note_rel(&project.path, copy);
            let Ok(doc) = vault.read(&research_rel) else {
                continue;
            };
            let stem = note_stem_of(&research_rel);
            for card in quote_cards(&doc.content) {
                cards.push(QuoteCardDto {
                    research_path: research_rel.clone(),
                    embed: format!("![[{stem}#^{}]]", card.id),
                    text: card.text,
                });
            }
        }
        Ok(cards)
    })
}

// ── 导入素材（C8，PRD-19-006）────────────────────────────────────────────────

/// 项目概览「导入素材…」（ADR-162 ⑯ / ADR-164 ③）：音视频走与「转写」页同一条导入与转写管线，
/// 文档走文档导入；产物都建在 `<项目文件夹>/<素材>`，成功后链进项目研究笔记的「素材」一节。
/// 文件夹与研究笔记都由 Rust 从项目主页推出。返回入队件数（文档 + 媒体）。
///
/// # Errors
/// `home` 不是视频项目主页、选了音视频而批量转写不可用时返回错误（文档已照常入队）。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_import_material(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    paths: Vec<String>,
) -> Result<u32, String> {
    let (folder, research) = material_target(&app, &window, &state, &home)?;
    let (documents, media) = super::doc_import::split_documents(paths);
    let documents = super::doc_import::enqueue_document_imports(
        &app,
        &state,
        documents,
        Some(folder.clone()),
        Some(research.clone()),
    );
    let media = super::transcribe::enqueue_media_imports(
        &app,
        &state,
        media,
        Some(folder),
        Some(research),
    )?;
    Ok(documents + media)
}

/// 项目的「素材」文件夹与研究笔记（库相对路径），都从项目主页推出；`home` 不是视频项目主页时
/// 报错。导入素材与网页剪藏共用（ADR-162 ⑯ / ADR-164 ⑦）。
pub(crate) fn material_target(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    state: &AppState,
    home: &str,
) -> Result<(String, String), String> {
    let is_project = state.with_vault(window.label(), |vault| {
        Ok(vault
            .read(home)
            .is_ok_and(|doc| doc.meta.note_type == NoteType::VideoProject))
    })?;
    let Some((project_folder, _)) = home.rsplit_once('/').filter(|_| is_project) else {
        return Err(coded_error(
            app,
            "CREATOR_NOT_A_PROJECT",
            "creator-not-a-project",
        ));
    };
    let i18n = backend_copy_arc(app);
    let copy = i18n.as_deref();
    let materials = backend_text_with(copy, "creator-material-folder", "素材");
    Ok((
        format!("{project_folder}/{materials}"),
        research_note_rel(home, copy),
    ))
}

/// 研究笔记里要加链接的那一节。
#[derive(Clone, Copy)]
enum ResearchSection {
    /// 「来源」：公开链接，会进简介的参考来源（网页剪藏，ADR-164 ⑦）。
    Sources,
    /// 「素材」：库里的转写 / 文档笔记（ADR-162 ⑯）。
    Materials,
}

/// 在研究笔记某一节末尾加一行 `line`（这一节里已有指向 `target` 的链接就不重复）；笔记或这一节
/// 不在就按项目骨架新建。返回新全文；没有要写的为 `None`。
fn with_section_line(
    existing: Option<&str>,
    headings: (&str, &str, &str),
    which: ResearchSection,
    target: &str,
    line: &str,
) -> Option<String> {
    let (sources, quotes, materials) = headings;
    let base = existing.map_or_else(|| format!("## {sources}\n\n## {quotes}\n"), str::to_owned);
    let heading = match which {
        ResearchSection::Sources => sources,
        ResearchSection::Materials => materials,
    };
    let section = luna_vault::section_body(&base, heading).unwrap_or_default();
    if section.contains(&format!("]({target})")) {
        return None;
    }
    let section = section.trim_end();
    let body = if section.is_empty() {
        line.to_owned()
    } else {
        format!("{section}\n{line}")
    };
    Some(luna_vault::upsert_section(&base, heading, &body))
}

/// 「素材」一节的那一行：`- [素材标题](相对路径)`。
fn material_line(research_rel: &str, note_rel: &str) -> (String, String) {
    let target =
        luna_vault::encode_link_target(&luna_vault::relative_link_path(research_rel, note_rel));
    let line = format!(
        "- {}",
        luna_vault::markdown_link(research_rel, note_rel, &note_stem_of(note_rel))
    );
    (target, line)
}

/// 素材转写 / 转换完成后链进研究笔记「素材」一节（处理队列在导入成功后调用）。写不成只记日志，
/// 不影响导入本身。
pub(crate) fn link_material(app: &tauri::AppHandle, research_rel: &str, note_rel: &str) {
    let (target, line) = material_line(research_rel, note_rel);
    link_into_research(
        app,
        research_rel,
        ResearchSection::Materials,
        &target,
        &line,
    );
}

/// 网页剪藏完成后在研究笔记「来源」一节加一行 `- [标题](规范链接)`（ADR-164 ⑦）。
pub(crate) fn link_source(app: &tauri::AppHandle, research_rel: &str, url: &str, title: &str) {
    let title = title.replace('[', "(").replace(']', ")");
    let title = if title.trim().is_empty() {
        url
    } else {
        title.trim()
    };
    let line = format!("- [{title}]({url})");
    link_into_research(app, research_rel, ResearchSection::Sources, url, &line);
}

fn link_into_research(
    app: &tauri::AppHandle,
    research_rel: &str,
    which: ResearchSection,
    target: &str,
    line: &str,
) {
    rewrite_research(app, research_rel, |existing, headings| {
        with_section_line(existing, headings, which, target, line)
    });
}

/// 把一篇笔记从研究笔记「素材」一节里拿掉（ADR-167 ③：来源被删除或移出项目时，索引里那一行
/// 一并去掉）。返回是否真的改了。
pub(crate) fn unlink_material(app: &tauri::AppHandle, research_rel: &str, note_rel: &str) -> bool {
    let (target, _) = material_line(research_rel, note_rel);
    rewrite_research(app, research_rel, |existing, (_, _, materials)| {
        without_section_link(existing?, materials, &target)
    })
}

/// 「素材」一节里去掉指向 `target` 的行；这一节里没有就是 `None`（不写）。
fn without_section_link(existing: &str, heading: &str, target: &str) -> Option<String> {
    let section = luna_vault::section_body(existing, heading)?;
    let needle = format!("]({target})");
    let kept: Vec<&str> = section
        .lines()
        .filter(|line| !line.contains(&needle))
        .collect();
    if kept.len() == section.lines().count() {
        return None;
    }
    Some(luna_vault::upsert_section(
        existing,
        heading,
        kept.join("\n").trim_end(),
    ))
}

/// 读—改—写研究笔记（以读到的全文为基线，被并发改了就再读一次），写成后广播。`edit` 拿到
/// 现有全文（没有这篇为 `None`）与三节标题，返回新全文；`None` = 不用写。返回是否写了。
fn rewrite_research(
    app: &tauri::AppHandle,
    research_rel: &str,
    edit: impl Fn(Option<&str>, (&str, &str, &str)) -> Option<String>,
) -> bool {
    let i18n = backend_copy_arc(app);
    let copy = i18n.as_deref();
    let sources = backend_text_with(copy, "creator-research-sources", "来源");
    let quotes = backend_text_with(copy, "creator-research-quotes", "引文");
    let materials = backend_text_with(copy, "creator-research-materials", "素材");
    let state = app.state::<AppState>();
    let outcome = (|| -> Result<Option<(String, bool)>, String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        for _ in 0..2 {
            let existing = vault.read(research_rel).ok().map(|doc| doc.content);
            let Some(next) = edit(existing.as_deref(), (&sources, &quotes, &materials)) else {
                return Ok(None);
            };
            let saved = match &existing {
                Some(base) => matches!(
                    vault
                        .save_note(research_rel, base, &next)
                        .map_err(vault_err)?,
                    luna_vault::NoteSave::Saved { .. }
                ),
                None => {
                    vault.write(research_rel, &next).map_err(vault_err)?;
                    true
                }
            };
            if saved {
                return Ok(Some((next, existing.is_none())));
            }
        }
        Err("研究笔记在写入期间一直在变".to_owned())
    })();
    match outcome {
        Ok(Some((content, created))) => {
            let _ = (NoteWritten {
                path: research_rel.to_owned(),
                content,
                origin: "main".to_owned(),
            })
            .emit(app);
            if created {
                emit_vault_changed(app, research_rel, false);
            }
            true
        }
        Ok(None) => false,
        Err(error) => {
            eprintln!("[research-link] 没能改写研究笔记：{error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADINGS: (&str, &str, &str) = ("来源", "引文", "素材");

    #[test]
    fn unlinking_a_material_drops_only_its_line_and_leaves_other_sections() {
        let research = "## 来源\n\n- [文章](https://a.example/x)\n\n## 素材\n\n- [访谈](素材/访谈.md)\n- [外部](../../Notes/外部.md)\n";
        let (target, _) = material_line("Creator/P/P-研究.md", "Creator/P/素材/访谈.md");
        let next = without_section_link(research, "素材", &target).unwrap();
        assert!(!next.contains("访谈"));
        assert!(next.contains("- [外部](../../Notes/外部.md)"));
        assert!(next.contains("- [文章](https://a.example/x)"));
        // 已经不在：不写。
        assert_eq!(without_section_link(&next, "素材", &target), None);
        assert_eq!(
            without_section_link("没有素材一节\n", "素材", &target),
            None
        );
    }

    fn with_material_link(
        existing: Option<&str>,
        research_rel: &str,
        note_rel: &str,
        headings: (&str, &str, &str),
    ) -> Option<String> {
        let (target, line) = material_line(research_rel, note_rel);
        with_section_line(
            existing,
            headings,
            ResearchSection::Materials,
            &target,
            &line,
        )
    }

    /// 剪藏的网页链进「来源」一节（公开链接），同一个网址只链一次，素材一节不动。
    #[test]
    fn a_clipped_page_goes_into_sources_once() {
        let base = "## 来源\n\n## 引文\n\n## 素材\n\n- [访谈](素材/访谈.md)\n";
        let url = "https://example.com/post/1";
        let line = format!("- [三个办法]({url})");
        let next =
            with_section_line(Some(base), HEADINGS, ResearchSection::Sources, url, &line).unwrap();
        assert_eq!(
            luna_vault::section_body(&next, "来源").unwrap().trim(),
            "- [三个办法](https://example.com/post/1)"
        );
        assert!(next.contains("- [访谈](素材/访谈.md)"), "素材一节不动");
        assert_eq!(
            with_section_line(Some(&next), HEADINGS, ResearchSection::Sources, url, &line),
            None
        );
    }

    #[test]
    fn a_material_link_goes_into_its_own_section_once() {
        let research = "Creator/P/P-研究.md";
        let first = with_material_link(
            Some("## 来源\n\n- https://example.com\n\n## 引文\n"),
            research,
            "Creator/P/素材/访谈 一.md",
            HEADINGS,
        )
        .unwrap();
        assert!(first.contains("## 素材\n\n- [访谈 一](素材/访谈%20一.md)"));
        assert!(first.contains("- https://example.com"), "来源一节不动");
        // 同一篇再链一次：不重复。
        assert_eq!(
            with_material_link(
                Some(&first),
                research,
                "Creator/P/素材/访谈 一.md",
                HEADINGS
            ),
            None
        );
        // 第二篇接在后面。
        let second =
            with_material_link(Some(&first), research, "Creator/P/素材/播客.md", HEADINGS).unwrap();
        let section = luna_vault::section_body(&second, "素材").unwrap();
        assert_eq!(section.trim().lines().count(), 2);
    }

    #[test]
    fn a_missing_research_note_starts_from_the_project_skeleton() {
        let created = with_material_link(
            None,
            "Creator/P/P-研究.md",
            "Creator/P/素材/访谈.md",
            HEADINGS,
        )
        .unwrap();
        assert!(created.starts_with("## 来源\n\n## 引文\n"));
        assert!(created.contains("## 素材\n\n- [访谈](素材/访谈.md)"));
    }
}
