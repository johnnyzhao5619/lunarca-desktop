//! 书签（PRD-06-006，笔记库对标轮 V5b）：读取与增删。规则与存放在 `luna_vault::bookmarks`；
//! 这里只做 IPC 形状、按窗口路由与广播（`vault://changed` 带书签文件路径，各处据此重取）。

use super::*;

/// 书签文件的库内路径（与前端认出这次变更的常量同一契约）。
const BOOKMARKS_REL: &str = ".luna/bookmarks.json";

/// 一条书签。
#[derive(Debug, Clone, Serialize, Type)]
pub struct VaultBookmarkDto {
    pub path: String,
    pub is_dir: bool,
}

fn to_dto(list: Vec<luna_vault::VaultBookmark>) -> Vec<VaultBookmarkDto> {
    list.into_iter()
        .map(|b| VaultBookmarkDto {
            path: b.path,
            is_dir: b.is_dir,
        })
        .collect()
}

/// 此刻还在的书签（按加入的先后）。
#[tauri::command]
#[specta::specta]
pub(crate) fn bookmarks(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<VaultBookmarkDto>, String> {
    state.with_vault(window.label(), |vault| {
        vault.bookmarks().map(to_dto).map_err(vault_err)
    })
}

/// 加入 / 移除一条书签，返回更新后的书签。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_bookmark(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    bookmarked: bool,
) -> Result<Vec<VaultBookmarkDto>, String> {
    let list = state.with_vault(window.label(), |vault| {
        vault
            .set_bookmarked(&path, bookmarked)
            .map(to_dto)
            .map_err(vault_err)
    })?;
    emit_vault_changed(&app, BOOKMARKS_REL, false);
    Ok(list)
}
