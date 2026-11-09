//! 项目的来源（ADR-167 ③，PRD-19-044）：「添加来源」里文件与网页之外的两种——粘贴文字、从库里
//! 选已有的笔记——以及「移出项目」。导入文件与网页剪藏仍走 `creator_quote` 的那两条。
//!
//! 来源是否属于项目、归哪一类由 `Vault::project_overview` 判；研究笔记的「素材」一节是来源的
//! 索引：这里加的、拿掉的都是那一节里的一行，来源本身的文件不动（移出项目不删文件）。来源被
//! 删除时，`vault_delete` 经 [`research_links_to`] / [`unlink_material`] 把索引里那一行一并去掉。

use super::creator_quote::{link_material, material_target, research_note_rel, unlink_material};
use super::*;
use luna_sync_core::naming::sanitized_file_stem;

/// 粘贴的文字最长多少字（再长就该作为文件导入）。
const PASTED_TEXT_MAX_CHARS: usize = 200_000;

/// 「添加来源 ▸ 粘贴文字」：在项目的「素材」文件夹里存成一篇笔记（标题即文件名，空了用
/// 「粘贴的文字」），链进研究笔记的「素材」一节。返回新笔记的库相对路径。
///
/// # Errors
/// 创作未启用、`home` 不是视频项目主页、文字为空或过长（`CREATOR_SOURCE_TEXT`）、写盘失败时
/// 返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_add_text_source(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    title: String,
    text: String,
) -> Result<String, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let text = text.replace("\r\n", "\n");
    let text = text.trim();
    if text.is_empty() || text.chars().count() > PASTED_TEXT_MAX_CHARS {
        return Err(coded_error(
            &app,
            "CREATOR_SOURCE_TEXT",
            "creator-source-text",
        ));
    }
    let (folder, research) = material_target(&app, &window, &state, &home)?;
    let i18n = backend_copy_arc(&app);
    let fallback = backend_text_with(i18n.as_deref(), "creator-pasted-title", "粘贴的文字");
    let stem = sanitized_file_stem(&title).unwrap_or(fallback);
    let path = state.with_vault(window.label(), |vault| {
        let path = super::distill::unique_note_path_in(vault, &folder, &stem);
        vault
            .create(
                &path,
                luna_vault::NoteType::Note,
                &luna_vault::TemplateVars {
                    body: format!("{text}\n"),
                    ..luna_vault::TemplateVars::default()
                },
            )
            .map_err(vault_err)?;
        Ok(path)
    })?;
    emit_vault_changed(&app, &path, false);
    link_material(&app, &research, &path);
    Ok(path)
}

/// 「添加来源 ▸ 从库里选」：把库里已有的笔记链进研究笔记的「素材」一节（不复制、不移动）。
/// 项目主页、研究笔记自己、读不出的路径跳过。返回链上了几篇。
///
/// # Errors
/// 创作未启用、`home` 不是视频项目主页时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_link_sources(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    paths: Vec<String>,
) -> Result<u32, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let (_, research) = material_target(&app, &window, &state, &home)?;
    let notes: Vec<String> = state.with_vault(window.label(), |vault| {
        Ok(paths
            .into_iter()
            .filter(|path| path != &home && path != &research && vault.note_exists(path))
            .collect())
    })?;
    for path in &notes {
        link_material(&app, &research, path);
    }
    Ok(u32::try_from(notes.len()).unwrap_or(u32::MAX))
}

/// 「移出项目」：把一篇来源从研究笔记的「素材」一节里拿掉（文件不动）。返回是否拿掉了。
///
/// # Errors
/// 创作未启用、`home` 不是视频项目主页时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_unlink_source(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    path: String,
) -> Result<bool, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let (_, research) = material_target(&app, &window, &state, &home)?;
    Ok(unlink_material(&app, &research, &path))
}

/// 删除之前：哪些项目研究笔记的索引链着要删的笔记（`(研究笔记, 被删的笔记)`）。研究笔记 =
/// 某个视频项目主页旁的 `<项目>-研究.md`；整个文件夹、非笔记跳过。删完由调用方逐对
/// [`unlink_material`]。
pub(crate) fn research_links_to(
    vault: &Vault,
    copy: BackendCopy<'_>,
    targets: &[String],
) -> Vec<(String, String)> {
    let notes: Vec<&String> = targets
        .iter()
        .filter(|path| path.ends_with(".md"))
        .collect();
    if notes.is_empty() {
        return Vec::new();
    }
    let research_notes: Vec<String> = vault
        .video_project_homes()
        .unwrap_or_default()
        .iter()
        .map(|home| research_note_rel(home, copy))
        .collect();
    let mut pairs = Vec::new();
    for note in notes {
        for source in vault.backlinks(note).unwrap_or_default() {
            if research_notes.contains(&source) && !targets.contains(&source) {
                pairs.push((source, note.clone()));
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_a_linked_source_finds_its_project_research_note_only() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        vault
            .write(
                "Creator/P/P-研究.md",
                "## 素材\n\n- [访谈](素材/访谈.md)\n- [外部](../../Notes/外部.md)\n",
            )
            .unwrap();
        vault.write("Creator/P/素材/访谈.md", "访谈\n").unwrap();
        vault.write("Notes/外部.md", "外部\n").unwrap();
        // 一篇普通笔记也链着外部那篇：它不是研究笔记，不动。
        vault.write("Notes/随手.md", "[外部](外部.md)\n").unwrap();
        vault.rebuild_index().unwrap();
        let targets = vec!["Notes/外部.md".to_owned(), "Creator/P/素材".to_owned()];
        assert_eq!(
            research_links_to(&vault, None, &targets),
            [("Creator/P/P-研究.md".to_owned(), "Notes/外部.md".to_owned())]
        );
        // 研究笔记自己也在删除之列：不必再改它。
        let with_research = vec!["Notes/外部.md".to_owned(), "Creator/P/P-研究.md".to_owned()];
        assert!(research_links_to(&vault, None, &with_research).is_empty());
    }
}
