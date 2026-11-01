//! 附件生命周期（PRD-06-012 ②，笔记库对标轮 V2）：孤儿附件的检测与移走。
//!
//! 判定全在 `luna_vault::orphans`（与改名重构同一个链接解析器）；这里只做 IPC 形状、删除
//! 策略取值与广播。只列不删：移走由用户勾选触发，走当前删除策略（可还原）。

use super::*;

/// 一个孤儿文件。
#[derive(Debug, Clone, Serialize, Type)]
pub struct OrphanAssetDto {
    pub path: String,
    pub bytes: f64,
}

/// 孤儿附件清单（按体积倒序）与合计。
#[derive(Debug, Clone, Serialize, Type)]
pub struct OrphanReportDto {
    pub items: Vec<OrphanAssetDto>,
    pub total_bytes: f64,
}

/// 一次移走的结果。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub struct OrphanRemovalDto {
    /// 已移走（或本来就不在了）。
    pub removed: Vec<String>,
    /// 列出来之后又被引用上了：没动。
    pub kept: Vec<String>,
    pub failed: Vec<VaultActionFailureDto>,
}

/// 列出库里没有任何笔记引用的文件（含回收站里的笔记与版本快照的引用，PRD-06-012 ②）。
#[tauri::command]
#[specta::specta]
pub(crate) fn orphan_assets(state: tauri::State<'_, AppState>) -> Result<OrphanReportDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let items: Vec<OrphanAssetDto> = vault
        .orphan_assets()
        .map_err(vault_err)?
        .into_iter()
        .map(|asset| OrphanAssetDto {
            path: asset.path,
            bytes: asset.bytes as f64,
        })
        .collect();
    let total_bytes = items.iter().map(|item| item.bytes).sum();
    Ok(OrphanReportDto { items, total_bytes })
}

/// 移走勾选的孤儿：按当前删除策略（与删除笔记同一条路，可还原）；Rust 在这一刻重判，
/// 列出之后被引用上的原样留下。整批只广播一次。
#[tauri::command]
#[specta::specta]
pub(crate) fn remove_orphan_assets(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<OrphanRemovalDto, String> {
    let mode = vault_delete_mode_from_settings(&state.settings_snapshot());
    let removal = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .remove_orphan_assets(&paths, mode)
            .map_err(vault_err)?
    };
    emit_vault_changes(
        &app,
        removal.removed.iter().map(|path| (path.clone(), true)),
    );
    Ok(OrphanRemovalDto {
        removed: removal.removed,
        kept: removal.kept,
        failed: removal
            .failed
            .into_iter()
            .map(|(path, error)| VaultActionFailureDto {
                path,
                error: vault_err(error),
            })
            .collect(),
    })
}
