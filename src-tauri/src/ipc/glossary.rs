//! 术语表命令域（PRD-02/PRD-10，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 词典批量导入结果（PRD-02 F-GAP-01）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct GlossaryImportResultDto {
    /// 新增条数。
    pub imported: u32,
    /// 因词条重复/无效被跳过的条数。
    pub skipped: u32,
    /// 用户取消了文件选择（前端据此不弹结果提示）。
    pub cancelled: bool,
}

/// 批量导入个人词典（PRD-02 F-GAP-01，对话框在 Rust，前端零业务）。
///
/// 选择 txt/csv/tsv 文件，每行 `词条<TAB或,>规范形`（单列 = 词条即规范形），
/// 按词条与现有条目去重合并（已有词条不覆盖），原子写回 `.luna/glossary.json`
/// 并广播 `vaultChanged`（词典面板据此刷新）。
///
/// # Errors
/// Vault 未打开、文件不可读或写回失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_glossary_entries(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<GlossaryImportResultDto, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| state.vault_not_open())?
            .root()
            .to_path_buf()
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Glossary", &["txt", "csv", "tsv"])
        .pick_file(move |path| {
            let result: Result<Option<PathBuf>, String> = match path {
                Some(p) => p.into_path().map(Some).map_err(|e| e.to_string()),
                None => Ok(None),
            };
            let _ = tx.send(result);
        });
    let Some(path) = rx
        .await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))??
    else {
        return Ok(GlossaryImportResultDto {
            imported: 0,
            skipped: 0,
            cancelled: true,
        });
    };

    let text = std::fs::read_to_string(&path).map_err(|e| format!("词典文件读取失败：{e}"))?;
    let incoming = parse_glossary_import(&text);

    let existing: Vec<GlossaryEntry> = read_meta::<serde_json::Value>(&root, "glossary")
        .map(|values| {
            values
                .into_iter()
                .filter_map(|value| serde_json::from_value::<GlossaryEntry>(value).ok())
                .collect()
        })
        .unwrap_or_default();

    let (merged, imported, skipped) = merge_glossary_entries(existing, incoming);
    if imported > 0 {
        let entries: Vec<serde_json::Value> = merged
            .iter()
            .map(|e| serde_json::to_value(e).map_err(|err| err.to_string()))
            .collect::<Result<_, _>>()?;
        write_meta(&root, "glossary", &entries).map_err(vault_err)?;
        emit_vault_changed(&app, ".luna/glossary.json", false);
    }
    Ok(GlossaryImportResultDto {
        imported,
        skipped,
        cancelled: false,
    })
}

/// 单条词条落库的结果（`upsert_glossary_entry`）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct GlossaryUpsertDto {
    /// 该词条此前已存在，本次是覆盖（`None` = 新增）。
    pub previous: Option<String>,
}

/// 新增或更新一条词条（`PRD-02` F-GAP-01 / `PRD-07-061`「在看见错字的地方就地加词」）。
///
/// **是 upsert 而不是 merge**：批量导入按"已有词条不覆盖"合并（一份外部清单不该
/// 悄悄改掉用户手工校准过的条目），而这里用户是**逐条、当场、明确地**输入了正确形式，
/// 那就是他此刻的意图。两条路径的规则不同，故不共用 `merge_glossary_entries`。
///
/// 覆盖时把旧值随结果返回，界面据此如实回话（"已更新，原为「X」"），而不是
/// 静默改掉一条用户可能忘了自己加过的词。
///
/// # Errors
/// Vault 未打开、词条为空或写回失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn upsert_glossary_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    term: String,
    canonical: String,
) -> Result<GlossaryUpsertDto, String> {
    let term = term.trim().to_owned();
    let canonical = canonical.trim().to_owned();
    if term.is_empty() || canonical.is_empty() {
        return Err(coded_error(
            &app,
            "GLOSSARY_EMPTY_FIELD",
            "glossary-empty-field",
        ));
    }
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| state.vault_not_open())?
            .root()
            .to_path_buf()
    };

    let mut entries: Vec<GlossaryEntry> = read_meta::<serde_json::Value>(&root, "glossary")
        .map(|values| {
            values
                .into_iter()
                .filter_map(|value| serde_json::from_value::<GlossaryEntry>(value).ok())
                .collect()
        })
        .unwrap_or_default();

    let previous = match entries.iter_mut().find(|entry| entry.term == term) {
        Some(existing) => Some(std::mem::replace(&mut existing.canonical, canonical)),
        None => {
            entries.push(GlossaryEntry { term, canonical });
            None
        }
    };

    let values: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| serde_json::to_value(e).map_err(|err| err.to_string()))
        .collect::<Result<_, _>>()?;
    write_meta(&root, "glossary", &values).map_err(vault_err)?;
    emit_vault_changed(&app, ".luna/glossary.json", false);
    Ok(GlossaryUpsertDto { previous })
}
