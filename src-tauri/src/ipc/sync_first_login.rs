//! 登录官方云后的一屏（ADR-178 ② ④；TDD-14 §5.2）：这台设备此刻打开的库要不要同步、与云端怎么合。
//!
//! 未登录时 `gate` 直接失败，零网络。决定按账户加库记在本机，问过一次就不再问；已经同步的库、
//! 连在别的账户下的库、放在第三方同步盘里的库都不问（后者由用户在账户页自己决定，ADR-148 ⑨）。

use std::path::Path;

use luna_sync::preview::merge_preview;
use luna_sync_core::crypto::KeyRing;
use luna_sync_core::hashing::ContentHash;
use luna_sync_core::manifest::{open_entry, EntryPlain};
use uuid::Uuid;

use super::*;
use crate::cloud_sync::{managed_keys, remote_error};
use crate::sync_service::bindable;

const DECIDED_FILE: &str = "sync-first-login.json";

/// 登录后要问的那个库。
#[derive(Debug, Clone, Serialize, Type)]
pub struct FirstLoginOfferDto {
    pub vault_path: String,
    pub vault_name: String,
    /// 同步范围的本地总字节。
    pub local_bytes: f64,
    /// 其中音视频的字节（放不下时默认不上传）。
    pub recording_bytes: f64,
}

/// 合并预览（文件数与将上传的字节）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct MergePreviewDto {
    pub download: u32,
    pub upload: u32,
    pub upload_bytes: f64,
    pub same: u32,
    pub differ: u32,
}

fn decided_path(
    app: &tauri::AppHandle,
    owner: &luna_sync::state::BindingOwner,
) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "sync_vault_unavailable".to_owned())?;
    Ok(crate::account_scope::marker_dir(&config_dir, owner).join(DECIDED_FILE))
}

fn read_decided(path: &Path) -> Vec<String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 登录后要不要弹那一屏；不弹 = `None`（没有任何失败要告诉用户：账户页随时能手动开）。
#[tauri::command]
#[specta::specta]
pub async fn sync_first_login_offer(
    app: tauri::AppHandle,
) -> Result<Option<FirstLoginOfferDto>, String> {
    let Ok(client) = gate(&app) else {
        return Ok(None);
    };
    if !client.server_is_official() {
        return Ok(None);
    }
    let Some(owner) = crate::account_scope::current(&client) else {
        return Ok(None);
    };
    let Some(root) = app.state::<AppState>().vault_root() else {
        return Ok(None);
    };
    if luna_sync::policy::in_third_party_sync_folder(&root) {
        return Ok(None);
    }
    let Ok(Target { id, root, name }) = target(&app, &root.to_string_lossy()) else {
        return Ok(None);
    };
    if read_decided(&decided_path(&app, &owner)?).contains(&id)
        || bindable(&state_path(&app, &id)?, &owner, false).is_err()
        || require_entitlement(&client).await.is_err()
    {
        return Ok(None);
    }
    let vault_path = root.to_string_lossy().into_owned();
    let (local_bytes, recording_bytes) = tauri::async_runtime::spawn_blocking(move || {
        let files = luna_vault::sync_scope_files(&root).unwrap_or_default();
        files.iter().fold((0u64, 0u64), |(all, media), file| {
            let recording = if luna_sync::policy::is_media(&file.rel) {
                file.size
            } else {
                0
            };
            (all + file.size, media + recording)
        })
    })
    .await
    .map_err(|_| "sync_vault_unavailable".to_owned())?;
    Ok(Some(FirstLoginOfferDto {
        vault_path,
        vault_name: name,
        local_bytes: local_bytes as f64,
        recording_bytes: recording_bytes as f64,
    }))
}

/// 用户对那一屏作了回应（选了什么都算）：这个库不再问。
#[tauri::command]
#[specta::specta]
pub async fn sync_first_login_decided(
    app: tauri::AppHandle,
    vault_path: String,
) -> Result<(), String> {
    let client = gate(&app)?;
    let owner =
        crate::account_scope::current(&client).ok_or_else(|| "sync_signed_out".to_owned())?;
    let Target { id, .. } = target(&app, &vault_path)?;
    let path = decided_path(&app, &owner)?;
    let mut decided = read_decided(&path);
    if !decided.contains(&id) {
        decided.push(id);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "sync_vault_unavailable".to_owned())?;
    }
    let bytes = serde_json::to_vec(&decided).map_err(|_| "internal".to_owned())?;
    std::fs::write(&path, bytes).map_err(|_| "sync_vault_unavailable".to_owned())
}

/// 加入一个云端库会做什么：与同步轮同一份判定，不写盘、不落绑定（ADR-178 ④）。
#[tauri::command]
#[specta::specta]
pub async fn sync_merge_preview(
    app: tauri::AppHandle,
    vault_path: String,
    remote_vault_id: String,
) -> Result<MergePreviewDto, String> {
    let client = gate(&app)?;
    let Target { root, .. } = target(&app, &vault_path)?;
    let remote = Uuid::parse_str(&remote_vault_id).map_err(|_| "invalid_input".to_owned())?;
    let cmks = managed_keys(&client, remote).await.map_err(|e| e.code())?;
    let ring = KeyRing::new(remote, &cmks).ok_or_else(|| "sync_no_managed_key".to_owned())?;
    let manifest = client
        .sync_manifest(&remote_vault_id, None)
        .await
        .map_err(|e| remote_error(e).code())?;
    let corrupt = || "sync_remote_corrupt".to_owned();
    let mut entries: Vec<EntryPlain> = Vec::with_capacity(manifest.entries.len());
    for entry in manifest.entries.iter().filter(|entry| !entry.deleted) {
        let file_id = Uuid::parse_str(&entry.file_id).map_err(|_| corrupt())?;
        let sealed = entry.entry_ct.as_ref().ok_or_else(corrupt)?;
        entries.push(open_entry(&ring, file_id, entry.rev, &sealed.0).map_err(|_| corrupt())?);
    }
    let preview = tauri::async_runtime::spawn_blocking(move || {
        let local = local_files(&root, &entries);
        merge_preview(local, entries)
    })
    .await
    .map_err(|_| "sync_vault_unavailable".to_owned())?;
    Ok(MergePreviewDto {
        download: preview.download as u32,
        upload: preview.upload as u32,
        upload_bytes: preview.upload_bytes as f64,
        same: preview.same as u32,
        differ: preview.differ as u32,
    })
}

/// 本地文件清单。只给与云端同路径、同大小的文件算哈希：其余不算也知道不一样，整库哈希太慢。
fn local_files(root: &Path, remote: &[EntryPlain]) -> Vec<luna_sync::core::LocalFile> {
    use luna_sync::state::path_key;
    let sizes: std::collections::HashMap<String, u64> = remote
        .iter()
        .map(|entry| (path_key(&entry.path), entry.size))
        .collect();
    luna_vault::sync_scope_files(root)
        .unwrap_or_default()
        .into_iter()
        .map(|file| {
            let path = luna_sync_core::path::nfc(&file.rel).into_owned();
            let content_hash = (sizes.get(&path_key(&path)) == Some(&file.size))
                .then(|| std::fs::File::open(root.join(&file.rel)).ok())
                .flatten()
                .and_then(|reader| ContentHash::of_reader(reader).ok())
                .map(|hash| hash.as_str().to_owned())
                .unwrap_or_default();
            luna_sync::core::LocalFile {
                path,
                content_hash,
                size: file.size,
                mtime_ms: file.modified_ms,
                settling: false,
            }
        })
        .collect()
}
