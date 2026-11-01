//! 排除路径（PRD-06-062，笔记库对标轮 V4a）：读取与增删。
//!
//! 判定与存储全在 `luna_vault::exclusions`；这里只做 IPC 形状与广播。改动广播
//! `vault://changed`（规则文件路径），各处（文件树淡化、设置页清单、搜索与图谱）据此重取。

use super::*;

/// 规则文件的库内路径（与前端认出这次变更的常量同一契约）。
fn rules_rel() -> String {
    format!(".luna/{}.json", luna_vault::EXCLUDED_PATHS_META)
}

/// 当前的排除规则（库相对路径，排序去重）。
#[tauri::command]
#[specta::specta]
pub(crate) fn excluded_paths(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault.exclusions().rules().to_vec())
}

/// 增删一条排除规则（文件树右键、设置页清单），返回更新后的完整清单。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_excluded_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    excluded: bool,
) -> Result<Vec<String>, String> {
    let rules = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.set_excluded(&path, excluded).map_err(vault_err)?
    };
    emit_vault_changed(&app, &rules_rel(), false);
    Ok(rules)
}
