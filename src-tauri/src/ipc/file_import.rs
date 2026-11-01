//! 从系统拖进文件树（PRD-07-008，笔记库对标轮 W5a）。复制规则在 `luna_vault::file_import`；这里只
//! 按窗口路由到对应的库、把结果交给界面、广播变更。

use super::*;

/// 一个文件没被复制进来的理由（界面按它说人话）。
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportSkipDto {
    NotAFile,
    Hidden,
    InsideVault,
    NotUtf8,
    Failed,
}

impl From<luna_vault::ImportSkip> for ImportSkipDto {
    fn from(skip: luna_vault::ImportSkip) -> Self {
        match skip {
            luna_vault::ImportSkip::NotAFile => Self::NotAFile,
            luna_vault::ImportSkip::Hidden => Self::Hidden,
            luna_vault::ImportSkip::InsideVault => Self::InsideVault,
            luna_vault::ImportSkip::NotUtf8 => Self::NotUtf8,
            luna_vault::ImportSkip::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ImportSkippedDto {
    /// 文件名（不是绝对路径：界面只说「哪一个」）。
    pub name: String,
    pub reason: ImportSkipDto,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ImportReportDto {
    /// 复制进来的文件（库相对路径）。
    pub imported: Vec<String>,
    pub skipped: Vec<ImportSkippedDto>,
}

/// 把系统拖进来的文件复制进 `folder`（库相对路径，`""` = 库根）。`paths` 是系统给的绝对路径，
/// 原样转发；收不收、叫什么名字全在 Rust 判。
#[tauri::command]
#[specta::specta]
pub(crate) fn import_files_into_folder(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    folder: String,
) -> Result<ImportReportDto, String> {
    let sources: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let report = state.with_vault(window.label(), |vault| {
        vault.import_files(&sources, &folder).map_err(vault_err)
    })?;
    if !report.imported.is_empty() {
        emit_vault_changes_in_scope(
            &app,
            report.imported.iter().map(|path| (path.clone(), false)),
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(ImportReportDto {
        imported: report.imported,
        skipped: report
            .skipped
            .into_iter()
            .map(|(name, reason)| ImportSkippedDto {
                name,
                reason: reason.into(),
            })
            .collect(),
    })
}
