//! 既有 Obsidian 库的一次性设置建议（PRD-06-064，笔记库对标轮 V4c）。
//!
//! 判定在 `luna_vault::obsidian`（只读 `.obsidian/app.json`，从不写）；这里取当前附件设置、
//! 落用户的选择。沿用 = 写两项附件设置（走 `set_setting` 同一条校验与广播）+ 加排除规则；
//! 不用 = 只记下「问过了」。两种都只问一次，记在库里。

use super::*;

/// 一次建议（只列出与当前不同的部分）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ObsidianSuggestionDto {
    /// 附件位置要改成哪一档（`vault.attachment_location` 的取值）；与当前一致时为 `null`。
    pub attachment_location: Option<String>,
    /// 附件子目录 / 指定文件夹；不涉及时为 `null`。
    pub attachment_folder: Option<String>,
    /// 尚未排除、建议加入的路径。
    pub exclusions: Vec<String>,
    /// 正则写法的排除条目数（Lunarca 不支持，无法沿用）。
    pub skipped_patterns: u32,
}

fn current_attachment(state: &AppState) -> luna_vault::AttachmentSetting {
    let settings = state.settings_snapshot();
    luna_vault::AttachmentSetting {
        location: settings
            .get_str("vault.attachment_location")
            .unwrap_or_else(|| "note_subfolder".to_owned()),
        folder: settings.get_str("vault.attachment_folder"),
    }
}

fn suggestion_of(state: &AppState) -> Result<Option<luna_vault::ObsidianSuggestion>, String> {
    let current = current_attachment(state);
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let Some(vault) = guard.as_ref() else {
        return Ok(None);
    };
    vault.obsidian_suggestion(&current).map_err(vault_err)
}

/// 这个库要不要给「沿用 Obsidian 设置」的建议；不是 Obsidian 库、问过了或没有不同时为 `null`。
#[tauri::command]
#[specta::specta]
pub(crate) fn obsidian_suggestion(
    state: tauri::State<'_, AppState>,
) -> Result<Option<ObsidianSuggestionDto>, String> {
    Ok(suggestion_of(&state)?.map(|suggestion| {
        let (location, folder) = suggestion
            .attachment
            .map_or((None, None), |a| (Some(a.location), a.folder));
        ObsidianSuggestionDto {
            attachment_location: location,
            attachment_folder: folder,
            exclusions: suggestion.exclusions,
            skipped_patterns: suggestion.skipped_patterns,
        }
    }))
}

/// 沿用：按此刻在 Rust 重算的建议落盘（不信前端传来的内容），并记下问过了。
#[tauri::command]
#[specta::specta]
pub(crate) fn accept_obsidian_suggestion(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let Some(suggestion) = suggestion_of(&state)? else {
        return Ok(());
    };
    if let Some(attachment) = suggestion.attachment {
        if let Some(folder) = attachment.folder {
            set_setting(
                app.clone(),
                state.clone(),
                "vault.attachment_folder".to_owned(),
                SettingValueDto::Str(folder),
            )?;
        }
        set_setting(
            app.clone(),
            state.clone(),
            "vault.attachment_location".to_owned(),
            SettingValueDto::Str(attachment.location),
        )?;
    }
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        for path in &suggestion.exclusions {
            vault.set_excluded(path, true).map_err(vault_err)?;
        }
        vault.record_obsidian_decision().map_err(vault_err)?;
    }
    if !suggestion.exclusions.is_empty() {
        emit_vault_changed(
            &app,
            &format!(".luna/{}.json", luna_vault::EXCLUDED_PATHS_META),
            false,
        );
    }
    Ok(())
}

/// 不用：只记下问过了。
#[tauri::command]
#[specta::specta]
pub(crate) fn dismiss_obsidian_suggestion(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.record_obsidian_decision().map_err(vault_err)
}
