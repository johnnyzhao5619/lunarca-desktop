//! 创建副本（PRD-06-006，笔记库对标轮 V5a）。命名与复制规则在 `luna_vault::duplicate`；这里只
//! 取界面语言的后缀、按窗口路由到对应的库、广播变更。

use super::*;

/// 在原文件旁边复制一份，返回副本的库相对路径（前端据此打开它）。
#[tauri::command]
#[specta::specta]
pub(crate) fn duplicate_vault_entry(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    // 后缀跟界面语言走（访达同样按系统语言命名副本）；文案来源未装配时退回英文。
    let suffix = state
        .copy()
        .and_then(|copy| copy.translate("duplicate-suffix"))
        .unwrap_or_else(|| "copy".to_owned());
    let copy = state.with_vault(window.label(), |vault| {
        vault.duplicate(&path, &suffix).map_err(vault_err)
    })?;
    emit_vault_changed(&app, &copy, false);
    Ok(copy)
}
