//! 语音速记的产物结构（PRD-02-033）：三种产物对照 + 采用标记。
//!
//! 「采用」= 用户对「这三版哪一版算数」的表态，落 frontmatter `luna.adopted`；它只是标记：
//! 正文三节一字不改、导出与列表照旧——把"采用"扩成会改正文或改导出的语义是另一条需求，
//! 此处刻意不做（裁决 2026-09-06）。产物文本由 `luna_vault::quick_note_products` 从正文读回。

use super::*;

/// 一种产物：全文 + 字数（前端展示片段与计数，不再自己数）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct QuickNoteProductDto {
    pub text: String,
    pub chars: u32,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct QuickNoteProductsDto {
    pub raw: Option<QuickNoteProductDto>,
    pub refined: Option<QuickNoteProductDto>,
    pub second_pass: Option<QuickNoteProductDto>,
    /// `raw` / `refined` / `second_pass`；`None` = 没表态。
    pub adopted: Option<String>,
    /// 留了原始录音时的媒体 Vault 相对路径（回放条用）。
    pub media_rel: Option<String>,
}

fn product(text: Option<String>) -> Option<QuickNoteProductDto> {
    text.map(|text| QuickNoteProductDto {
        chars: u32::try_from(text.chars().count()).unwrap_or(u32::MAX),
        text,
    })
}

/// 速记的产物结构；不是速记（`type` ≠ quick-note）返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) fn quick_note_products(
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<Option<QuickNoteProductsDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let doc = vault.read(&note_rel).map_err(vault_err)?;
    if doc.meta.note_type != NoteType::QuickNote {
        return Ok(None);
    }
    let products = luna_vault::quick_note_products(&doc.content);
    Ok(Some(QuickNoteProductsDto {
        raw: product(products.raw),
        refined: product(products.refined),
        second_pass: product(products.second_pass),
        adopted: doc.meta.adopted.clone(),
        media_rel: doc.meta.media.first().map(|m| m.path.clone()),
    }))
}

/// 写 / 清采用标记（`None` = 清）。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_quick_note_adopted(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    adopted: Option<String>,
) -> Result<(), String> {
    let version = match adopted.as_deref() {
        None => None,
        Some(value) => Some(
            luna_vault::AdoptedVersion::parse(value)
                .ok_or_else(|| format!("QUICK_NOTE_ADOPTED_INVALID: {value}"))?,
        ),
    };
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .set_quick_note_adopted(&note_rel, version)
            .map_err(vault_err)?;
    }
    emit_vault_changed(&app, &note_rel, false);
    Ok(())
}
