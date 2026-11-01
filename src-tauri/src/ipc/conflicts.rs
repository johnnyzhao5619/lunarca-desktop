//! 同步盘痕迹（TDD-06 §4.1）：冲突副本（①，V3b）与 iCloud 未下载占位（②，V3c）。
//!
//! 判定全在 `luna_vault::conflicts`；这里只做 IPC 形状、删除策略取值与广播。只标出、给对比、
//! 由用户选保留哪份，不自动合并、不自动删除。

use super::*;

/// 冲突副本从哪来。
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKindDto {
    Syncthing,
    Dropbox,
    Icloud,
    GitMarkers,
}

impl From<luna_vault::ConflictKind> for ConflictKindDto {
    fn from(kind: luna_vault::ConflictKind) -> Self {
        match kind {
            luna_vault::ConflictKind::Syncthing => Self::Syncthing,
            luna_vault::ConflictKind::Dropbox => Self::Dropbox,
            luna_vault::ConflictKind::ICloud => Self::Icloud,
            luna_vault::ConflictKind::GitMarkers => Self::GitMarkers,
        }
    }
}

/// 一份冲突副本。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ConflictCopyDto {
    pub path: String,
    pub kind: ConflictKindDto,
    /// 它是哪一篇的副本；git 冲突标记在原文自己身上，为 `None`。
    pub original: Option<String>,
}

impl From<luna_vault::ConflictCopy> for ConflictCopyDto {
    fn from(copy: luna_vault::ConflictCopy) -> Self {
        Self {
            path: copy.path,
            kind: copy.kind.into(),
            original: copy.original,
        }
    }
}

/// 这一篇是不是冲突副本（打开笔记时问；含正文里的 git 冲突标记）。
#[tauri::command]
#[specta::specta]
pub(crate) fn conflict_copy_info(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Option<ConflictCopyDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .conflict_copy_of(&path)
        .map_err(vault_err)?
        .map(Into::into))
}

/// 库里按文件名认得出的冲突副本（文件树打标记）。
#[tauri::command]
#[specta::specta]
pub(crate) fn conflict_copies(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ConflictCopyDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .conflict_copies()
        .map_err(vault_err)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 选保留哪份：`keep_copy` = 用副本替换原文（原文留版本可回滚），否则保留原文；副本都按
/// 当前删除策略移走（可还原）。返回原文路径（前端据此把标签切过去）。
#[tauri::command]
#[specta::specta]
pub(crate) fn resolve_conflict_copy(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    keep_copy: bool,
) -> Result<String, String> {
    let mode = vault_delete_mode_from_settings(&state.settings_snapshot());
    let original = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .resolve_conflict_copy(&path, keep_copy, mode)
            .map_err(vault_err)?
    };
    emit_vault_changes(&app, [(original.clone(), false), (path, true)]);
    Ok(original)
}

/// 请 macOS 把一个 iCloud 未下载的文件拉回本机（`brctl download`，不等它下完）。下载回来后
/// 文件监听会看到真文件出现，树与打开着的那一篇随之刷新。
pub(crate) fn start_cloud_download(root: &std::path::Path, rel: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/brctl")
            .arg("download")
            .arg(root.join(rel))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut child| {
                // 不等下载完，但要回收子进程，免得留下僵尸进程。
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
            .map_err(|error| format!("VAULT_IO: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, rel);
        Err("VAULT_IO: iCloud download is only available on macOS".to_owned())
    }
}

/// 文件树上点「未下载」的那一行（TDD-06 §4.1 ②）：请系统下载。
#[tauri::command]
#[specta::specta]
pub(crate) fn request_cloud_download(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if !vault.is_cloud_placeholder(&path) {
            return Ok(()); // 已经下载回来了（或从来不是占位）：没什么要做的。
        }
        vault.root().to_path_buf()
    };
    start_cloud_download(&root, &path)
}
