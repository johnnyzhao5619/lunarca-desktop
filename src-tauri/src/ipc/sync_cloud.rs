//! 账户页「云端库」：本账户名下全部云端库的清单，含不在这台设备上的与已删除（恢复期内）的；每行
//! 能做什么由这里裁决，界面只渲染（ADR-151 修订 2026-10-15；TDD-14 §5；ADR-003）。
//!
//! - **下载到本机**：选一个存放位置，在其中新建与云端库同名的文件夹作为库（同名的已有非空文件夹
//!   不碰，改用「名字 (2)」……；选中的本就是一个同名空文件夹则直接用它），登记进库清单（不切换），
//!   再走「加入已有云端库」同一条绑定路径——首次同步把云端的内容全部拉下来。
//! - **删除云端库**：所有设备停止同步、各设备的本地文件都保留；30 天内可在这里恢复，删除即不计
//!   容量（服务端协议 v15）。这台设备若正同步它，当场断开（本地文件不动）。
//! - **恢复**：恢复期内的已删除库回到删除前的状态；超容量 / 已过期如实说。
//!
//! 库名是客户端加密的：用库主密钥解开（本机钥匙串缓存优先，零网络；没有再取账户托管信封）。
//! 已删除的库取不到信封（数据面一律 410），于是解开过的名字按账户记在本机
//! （`accounts/<账户键>/sync-vault-names.json`），删除之后也认得出；从没解开过就如实说「名字暂不
//! 可读」，绝不编一个。
//!
//! 错误一律是稳定码字符串：`sync_folder_unavailable`、
//! `sync_folder_inside_vault`、`sync_manage_forbidden`、`sync_restore_over_capacity`、
//! `sync_restore_expired`，以及 [`super::sync_ipc`] 的那一套。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use luna_cloud::client::CloudClient;
use luna_cloud::error::CloudError;
use luna_cloud::sync_client::{SyncVaultDto, SyncVaultStateDto};
use luna_sync::crypto::{Cmk, KeyRing};
use luna_sync::keystore::CmkStore;
use luna_vault::registry::VaultEntry;
use uuid::Uuid;

use super::sync_ipc::{
    bind_and_install, gate, keys, release_local, require_entitlement, service, signed_in_owner,
    vault_name,
};
use super::*;
use crate::cloud_sync::{managed_keys, remote_error};
use crate::sync_service::{display_path, identity_of, linked_here};

/// 解开过的库名（按账户存，删除之后也认得出）。
const VAULT_NAMES_FILE: &str = "sync-vault-names.json";

/// 一个云端库在这台设备上的处境。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloudVaultPlaceDto {
    /// 这台设备正把它同步到这个本地库。
    Here {
        vault_path: String,
        local_name: String,
        display_path: String,
    },
    /// 不在这台设备上。
    NotHere,
    /// 已删除（所有设备都停止同步了）；`purge_after_ms` 之前可恢复，之后彻底清除。
    Deleted {
        deleted_ms: Option<f64>,
        purge_after_ms: Option<f64>,
    },
}

/// 「云端库」里的一行。时刻是 Unix 毫秒（`f64` 因 specta 不收 64 位整数）。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct CloudVaultDto {
    pub id: String,
    /// 解开的库名；解不开、也没解开过 = `None`（界面写「名字暂不可读」）。
    pub name: Option<String>,
    /// 删除前要照着输入的字：库名；名字读不出时是库 id 的前 8 位。
    pub confirm_text: String,
    /// 云端占用的密文字节（含历史版本）。已删除的库是恢复后会加回容量的量。
    pub cloud_bytes: f64,
    /// 最近一次提交的时刻；还没有提交 = 建库时刻。
    pub last_changed_ms: Option<f64>,
    pub place: CloudVaultPlaceDto,
    /// 可以「下载到本机」：没删、也不在这台设备上。
    pub can_download: bool,
    /// 可以「删除云端库」：还没删。成员能不能删组织的库由服务端判（403）。
    pub can_delete: bool,
    /// 可以「恢复」：已删除且还在恢复期内。
    pub can_restore: bool,
}

fn rfc3339_ms(raw: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|t| t.timestamp_millis() as f64)
}

/// 删除确认要输入的字：库名；读不出名字时用库 id 的前 8 位（界面同时写出来让人照着输）。
fn confirm_text(id: &str, name: Option<&str>) -> String {
    name.map_or_else(|| id.chars().take(8).collect(), str::to_owned)
}

/// 清单里的每一行与它能做什么（纯函数）。`here` 是这台设备上归此刻账户的绑定（只认库注册表里的
/// 库，见 [`linked_here`]）。
fn cloud_vault_rows(
    vaults: &[SyncVaultDto],
    names: &HashMap<String, String>,
    here: &HashMap<Uuid, VaultEntry>,
    home: Option<&Path>,
    now_ms: i64,
) -> Vec<CloudVaultDto> {
    vaults
        .iter()
        .map(|vault| {
            let name = names.get(&vault.id).cloned();
            let local = Uuid::parse_str(&vault.id)
                .ok()
                .and_then(|remote| here.get(&remote));
            let place = match (&vault.state, local) {
                (
                    SyncVaultStateDto::Deleted {
                        deleted_at,
                        purge_after,
                    },
                    _,
                ) => CloudVaultPlaceDto::Deleted {
                    deleted_ms: rfc3339_ms(deleted_at),
                    purge_after_ms: rfc3339_ms(purge_after),
                },
                (_, Some(entry)) => CloudVaultPlaceDto::Here {
                    vault_path: entry.path.to_string_lossy().into_owned(),
                    local_name: entry.name.clone(),
                    display_path: display_path(&entry.path, home),
                },
                (_, None) => CloudVaultPlaceDto::NotHere,
            };
            let can_restore = matches!(
                place,
                CloudVaultPlaceDto::Deleted { purge_after_ms: Some(at), .. } if at > now_ms as f64
            );
            CloudVaultDto {
                id: vault.id.clone(),
                confirm_text: confirm_text(&vault.id, name.as_deref()),
                name,
                cloud_bytes: vault.bytes_stored as f64,
                last_changed_ms: rfc3339_ms(&vault.last_changed_at),
                can_download: matches!(place, CloudVaultPlaceDto::NotHere),
                can_delete: !vault.is_deleted(),
                can_restore,
                place,
            }
        })
        .collect()
}

/// 这一次解开的名字与记下的合起来：解开了用新的（库名改过也跟上）；没解开退回记下的；清单里
/// 已经没有的库（期满清除了）不再记。返回（这一次用的名字，要记下的）。
fn merge_names(
    listed: &[String],
    opened: &HashMap<String, String>,
    remembered: &BTreeMap<String, String>,
) -> (HashMap<String, String>, BTreeMap<String, String>) {
    let mut names = HashMap::new();
    let mut keep = BTreeMap::new();
    for id in listed {
        if let Some(name) = opened.get(id).or_else(|| remembered.get(id)) {
            names.insert(id.clone(), name.clone());
            keep.insert(id.clone(), name.clone());
        }
    }
    (names, keep)
}

fn read_names(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir.join(VAULT_NAMES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_names(dir: &Path, names: &BTreeMap<String, String>) {
    let Ok(bytes) = serde_json::to_vec(names) else {
        return;
    };
    // 写不进只是下次删除后认不出名字，不影响清单本身。
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join(VAULT_NAMES_FILE), bytes);
    }
}

fn open_name(remote: Uuid, cmks: &[Cmk], name_ct: &[u8]) -> Option<String> {
    let ring = KeyRing::new(remote, cmks)?;
    ring.for_ciphertext(name_ct)
        .ok()?
        .open_vault_name(name_ct)
        .ok()
}

/// 解开一个库名：本机钥匙串里有它的主密钥（这台设备同步着它）就零网络解；否则取账户托管信封。
/// 已删除的库取不到信封（410），不去问。
async fn readable_name(client: &CloudClient, vault: &SyncVaultDto) -> Option<String> {
    let remote = Uuid::parse_str(&vault.id).ok()?;
    let cached = keys().load(remote).unwrap_or_default();
    if let Some(name) = open_name(remote, &cached, &vault.name_ct.0) {
        return Some(name);
    }
    if vault.is_deleted() {
        return None;
    }
    vault_name(client, remote, &vault.name_ct.0).await
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|_| "sync_state_db".to_owned())
}

/// 本账户的全部云端库（含不在这台设备上的与恢复期内已删除的），带每行能做什么。
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_vaults(app: tauri::AppHandle) -> Result<Vec<CloudVaultDto>, String> {
    let client = gate(&app)?;
    let owner = signed_in_owner(&client)?;
    let vaults = client
        .sync_vaults()
        .await
        .map_err(|e| remote_error(e).code())?;
    let config_dir = config_dir(&app)?;
    let known = app.state::<AppState>().known_vaults();
    let here = linked_here(&config_dir, &known, &owner);

    let mut opened = HashMap::new();
    for vault in &vaults {
        if let Some(name) = readable_name(&client, vault).await {
            opened.insert(vault.id.clone(), name);
        }
    }
    let names_dir = crate::account_scope::marker_dir(&config_dir, &owner);
    let remembered = read_names(&names_dir);
    let listed: Vec<String> = vaults.iter().map(|v| v.id.clone()).collect();
    let (names, keep) = merge_names(&listed, &opened, &remembered);
    if keep != remembered {
        write_names(&names_dir, &keep);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Ok(cloud_vault_rows(
        &vaults,
        &names,
        &here,
        home.as_deref(),
        chrono::Utc::now().timestamp_millis(),
    ))
}

/// 文件夹名里不许出现的字符：取 Windows 那一套（最严），各平台一致——库会同步到别的系统上。
const FORBIDDEN_IN_FOLDER_NAME: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows 的保留设备名（不分大小写、带不带扩展名都不能用作文件夹名）。
const RESERVED_FOLDER_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 文件夹名的字符上限：留足余量给「 (2)」后缀与 Windows 的路径长度。
const MAX_FOLDER_NAME_CHARS: usize = 80;

/// 云端库名 → 能在三个平台上当文件夹名的写法（纯函数）。非法字符与控制字符换成 `_`，去掉首尾空白、
/// 开头的点（免得成了隐藏文件夹）与结尾的点和空格（Windows 不认），按字符截到上限，撞上保留名就
/// 补一个 `_`。名字读不出（从没解开过）或清理后为空 → `Lunarca <库 id 前 8 位>`，不编一个像真名的。
fn folder_name_for(name: Option<&str>, remote: &Uuid) -> String {
    let fallback = || format!("Lunarca {}", &remote.simple().to_string()[..8]);
    let Some(name) = name else {
        return fallback();
    };
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN_IN_FOLDER_NAME.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trim = |text: &str| -> String {
        text.trim()
            .trim_start_matches('.')
            .trim_end_matches(['.', ' '])
            .trim()
            .to_owned()
    };
    let mut folder = trim(&cleaned);
    if folder.chars().count() > MAX_FOLDER_NAME_CHARS {
        folder = trim(
            &folder
                .chars()
                .take(MAX_FOLDER_NAME_CHARS)
                .collect::<String>(),
        );
    }
    if folder.is_empty() {
        return fallback();
    }
    let stem = folder
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    if RESERVED_FOLDER_NAMES.contains(&stem.as_str()) {
        folder.push('_');
    }
    folder
}

/// 文件夹是空的（`.DS_Store` 不算内容）。读不了 = 不算空。
fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|entries| {
        !entries
            .flatten()
            .any(|entry| entry.file_name() != ".DS_Store")
    })
}

/// 「下载到本机」的落点（用户裁决 2026-10-16，对齐 Obsidian / Logseq / 各家网盘客户端「选存放位置、
/// 在其中建同名文件夹」的做法）：在用户选的存放位置 `parent` 里新建与云端库同名的文件夹作为库。
///
/// - 选中的本就是一个与库同名的空文件夹（用户先建好了再选它）→ 直接用它，不再套一层；
/// - 同名文件夹已存在且是空的 → 用它；已存在但有东西 → 不碰，依次试「名字 (2)」「名字 (3)」……；
/// - 存放位置本身在某个已知库里 → 拒绝（库不能套库）；新建的那一层也不能与已知库重叠。
///
/// 返回（库根，是不是这一步新建的）：新建的那一层在后续失败时整个撤掉，原有的空文件夹只撤这一步写的。
fn download_target(
    parent: &Path,
    folder_name: &str,
    known: &[VaultEntry],
) -> Result<(PathBuf, bool), String> {
    if !parent.is_absolute() {
        return Err("invalid_input".into());
    }
    let parent = std::fs::canonicalize(parent).map_err(|_| "sync_folder_unavailable".to_owned())?;
    if !parent.is_dir() {
        return Err("sync_folder_unavailable".into());
    }
    if known.iter().any(|e| parent.starts_with(&e.path)) {
        return Err("sync_folder_inside_vault".into());
    }
    let clear_of_known = |path: &Path| {
        !known
            .iter()
            .any(|e| luna_vault::registry::paths_overlap(&e.path, path))
    };
    if parent.file_name().is_some_and(|n| n == folder_name) && is_empty_dir(&parent) {
        return Ok((parent, false));
    }
    for n in 1..=99 {
        let candidate = if n == 1 {
            parent.join(folder_name)
        } else {
            parent.join(format!("{folder_name} ({n})"))
        };
        if !clear_of_known(&candidate) {
            continue;
        }
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok((candidate, true)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if candidate.is_dir() && is_empty_dir(&candidate) {
                    return Ok((candidate, false));
                }
            }
            Err(_) => return Err("sync_folder_unavailable".into()),
        }
    }
    Err("sync_folder_unavailable".into())
}

/// 把一个不在这台设备上的云端库下载到本机：在用户选的存放位置 `folder` 里建与云端库同名的文件夹作
/// 为库（[`download_target`]），登记进库清单（不切换当前库），再加入这个云端库（库主密钥从账户托管
/// 信封取回）。首次同步随即把云端内容拉下来。返回新库的路径。
/// 中途失败就把这一步建的东西撤掉：新建的那一层整个删掉；原有的空文件夹只撤库清单那一行、`.luna/`
/// 与 `.gitignore`，回到原样。
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_vault_download(
    app: tauri::AppHandle,
    remote_vault_id: String,
    folder: String,
) -> Result<String, String> {
    let client = gate(&app)?;
    let owner = signed_in_owner(&client)?;
    let remote = Uuid::parse_str(&remote_vault_id).map_err(|_| "invalid_input".to_owned())?;
    let state = app.state::<AppState>();
    let known = state.known_vaults();
    let config_dir = config_dir(&app)?;
    if linked_here(&config_dir, &known, &owner).contains_key(&remote) {
        return Err("sync_remote_in_use".into());
    }
    require_entitlement(&client).await?;
    let cmks = managed_keys(&client, remote).await.map_err(|e| e.code())?;
    if cmks.is_empty() {
        return Err("sync_no_managed_key".into());
    }
    let name =
        read_names(&crate::account_scope::marker_dir(&config_dir, &owner)).remove(&remote_vault_id);
    // 落点不能与任何一份已知库重叠——另一个账户的库也是这台设备上的文件夹。网络与权益都过了再建
    // 文件夹，免得失败时留下一个空壳。
    let (target, created) = download_target(
        Path::new(&folder),
        &folder_name_for(name.as_deref(), &remote),
        &state.vault_registry.list(),
    )?;
    let undo = |root: &Path| {
        state.vault_registry.remove(root);
        if created {
            let _ = std::fs::remove_dir_all(root);
        } else {
            let _ = std::fs::remove_dir_all(root.join(".luna"));
            let _ = std::fs::remove_file(root.join(".gitignore"));
        }
    };
    let vault = match Vault::open(&target) {
        Ok(vault) => vault,
        Err(_) => {
            undo(&target);
            return Err("sync_folder_unavailable".into());
        }
    };
    let (id, root) = (vault.id().to_string(), vault.root().to_path_buf());
    drop(vault);
    if state.vault_registry.register(&root, name).is_err() {
        undo(&root);
        return Err("sync_folder_inside_vault".into());
    }
    if let Err(code) =
        bind_and_install(&app, &client, owner, &id, root.clone(), remote, &cmks, None).await
    {
        undo(&root);
        return Err(code);
    }
    Ok(root.to_string_lossy().into_owned())
}

/// 删除 / 恢复云端库时服务端说「不行」的那几种（纯函数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Manage {
    Delete,
    Restore,
}

fn manage_error(err: CloudError, action: Manage) -> String {
    match (&err, action) {
        // 组织成员删 / 恢复组织的库：只有所有者与管理员可以。
        (
            CloudError::Server {
                status: 403,
                error_code,
            },
            _,
        ) if error_code == "forbidden" => "sync_manage_forbidden".into(),
        (CloudError::Server { status: 402, .. }, Manage::Restore) => {
            "sync_restore_over_capacity".into()
        }
        // 恢复时 404 = 过了恢复期（已彻底清除或正在清除）。
        (CloudError::Server { status: 404, .. }, Manage::Restore) => "sync_restore_expired".into(),
        _ => remote_error(err).code(),
    }
}

/// 恢复的结果：已经不在「已删除」（别处先恢复了，409 `vault_not_deleted`）也算达成。
fn restore_outcome(result: Result<SyncVaultDto, CloudError>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        Err(CloudError::Server {
            status: 409,
            error_code,
        }) if error_code == "vault_not_deleted" => Ok(()),
        Err(err) => Err(manage_error(err, Manage::Restore)),
    }
}

/// 删除云端库：所有设备停止同步，各设备上的本地文件都保留；30 天内可在账户页恢复，期满彻底清除。
/// 这台设备正同步它就当场断开（[`release_local`]，本地文件不动），也不再收到「在另一台设备上被
/// 删除」的通知。
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_vault_delete(
    app: tauri::AppHandle,
    remote_vault_id: String,
) -> Result<(), String> {
    let client = gate(&app)?;
    let owner = signed_in_owner(&client)?;
    let remote = Uuid::parse_str(&remote_vault_id).map_err(|_| "invalid_input".to_owned())?;
    let service = service(&app)?;
    // 先记下：删除生效到本地断开之间，同步轮可能先撞上 410——那是这台设备自己删的，不该说成
    // 「在另一台设备上被删除」。
    service.expect_deletion(remote);
    if let Err(err) = client.sync_delete_vault(&remote_vault_id).await {
        service.reset_deletion_notice(remote);
        return Err(manage_error(err, Manage::Delete));
    }
    let known = app.state::<AppState>().known_vaults();
    let local = linked_here(&config_dir(&app)?, &known, &owner).remove(&remote);
    if let Some(entry) = local {
        if let Some(id) = identity_of(&entry) {
            release_local(&app, &id, &entry.path).await?;
            // 断开 = 回到本机库（ADR-151 修订 2026-10-15「续二」）。
            app.state::<AppState>()
                .vault_registry
                .make_device(&entry.path);
        }
    }
    service.nudge();
    Ok(())
}

/// 恢复一个已删除的云端库（恢复期内）。恢复后它回到删除前的状态，各设备可以重新加入或下载。
#[tauri::command]
#[specta::specta]
pub async fn sync_cloud_vault_restore(
    app: tauri::AppHandle,
    remote_vault_id: String,
) -> Result<(), String> {
    let client = gate(&app)?;
    let remote = Uuid::parse_str(&remote_vault_id).map_err(|_| "invalid_input".to_owned())?;
    restore_outcome(client.sync_restore_vault(&remote_vault_id).await)?;
    // 恢复之后若再被删除，照常说一次。
    service(&app)?.reset_deletion_notice(remote);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(id: &str, state: SyncVaultStateDto) -> SyncVaultDto {
        SyncVaultDto {
            id: id.into(),
            epoch: "e".into(),
            head_seq: 1,
            bytes_stored: 4096,
            name_ct: luna_cloud::sync_client::Base64Bytes(vec![1]),
            created_at: "2026-10-01T00:00:00Z".into(),
            last_changed_at: "2026-10-02T00:00:00Z".into(),
            state,
        }
    }

    const HERE: &str = "00000000-0000-0000-0000-000000000001";
    const AWAY: &str = "00000000-0000-0000-0000-000000000002";
    const GONE: &str = "00000000-0000-0000-0000-000000000003";
    const EXPIRED: &str = "00000000-0000-0000-0000-000000000004";

    fn deleted(purge_after: &str) -> SyncVaultStateDto {
        SyncVaultStateDto::Deleted {
            deleted_at: "2026-10-15T00:00:00Z".into(),
            purge_after: purge_after.into(),
        }
    }

    /// 每一行在这台设备上的处境与能做什么都由这里裁决：同步在这里 → 写出本地库；不在 → 可下载；
    /// 已删除 → 单独一种处境、只可恢复（过了恢复期连恢复也不行）；名字读不出时删除确认用 id 前 8 位。
    #[test]
    fn rows_carry_the_place_and_the_actions_that_apply() {
        let vaults = vec![
            vault(HERE, SyncVaultStateDto::Active),
            vault(AWAY, SyncVaultStateDto::ReadOnly),
            vault(GONE, deleted("2026-11-14T00:00:00Z")),
            vault(EXPIRED, deleted("2026-10-01T00:00:00Z")),
        ];
        let names = HashMap::from([
            (HERE.to_owned(), "工作".to_owned()),
            (GONE.to_owned(), "旧库".to_owned()),
        ]);
        let here = HashMap::from([(
            Uuid::parse_str(HERE).unwrap(),
            VaultEntry {
                path: PathBuf::from("/Users/me/Notes/Work"),
                name: "工作笔记".into(),
                last_opened_ms: 0,
                id: None,
                accounts: Vec::new(),
            },
        )]);
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-16T00:00:00Z")
            .unwrap()
            .timestamp_millis();
        let rows = cloud_vault_rows(&vaults, &names, &here, Some(Path::new("/Users/me")), now);

        assert_eq!(
            rows[0].place,
            CloudVaultPlaceDto::Here {
                vault_path: "/Users/me/Notes/Work".into(),
                local_name: "工作笔记".into(),
                display_path: "~/Notes/Work".into(),
            }
        );
        assert!(!rows[0].can_download && rows[0].can_delete && !rows[0].can_restore);
        assert_eq!(rows[0].confirm_text, "工作");
        assert!(rows[0].last_changed_ms.is_some());

        assert_eq!(rows[1].place, CloudVaultPlaceDto::NotHere);
        assert!(rows[1].can_download && rows[1].can_delete && !rows[1].can_restore);
        assert_eq!(rows[1].name, None, "解不开、也没记下：不编名字");
        assert_eq!(rows[1].confirm_text, "00000000");

        assert!(matches!(
            rows[2].place,
            CloudVaultPlaceDto::Deleted {
                purge_after_ms: Some(_),
                deleted_ms: Some(_)
            }
        ));
        assert!(!rows[2].can_download && !rows[2].can_delete && rows[2].can_restore);
        assert_eq!(rows[2].name.as_deref(), Some("旧库"));

        assert!(!rows[3].can_restore, "过了恢复期：只剩等清除，不给恢复入口");
        assert!(!rows[3].can_delete && !rows[3].can_download);
    }

    /// 只认库注册表里的绑定：已删除的库即使这台设备还绑着它，也只算「已删除」。
    #[test]
    fn a_deleted_vault_is_deleted_even_if_still_bound_here() {
        let vaults = vec![vault(GONE, deleted("2026-11-14T00:00:00Z"))];
        let here = HashMap::from([(
            Uuid::parse_str(GONE).unwrap(),
            VaultEntry {
                path: PathBuf::from("/v"),
                name: "v".into(),
                last_opened_ms: 0,
                id: None,
                accounts: Vec::new(),
            },
        )]);
        let rows = cloud_vault_rows(&vaults, &HashMap::new(), &here, None, 0);
        assert!(matches!(rows[0].place, CloudVaultPlaceDto::Deleted { .. }));
    }

    /// 名字：解开了用新的；没解开退回记下的；从没解开过就没有；清单里没有的库不再记。
    #[test]
    fn names_fall_back_to_the_remembered_ones_and_never_invent() {
        let listed = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let opened = HashMap::from([("a".to_owned(), "新名".to_owned())]);
        let remembered = BTreeMap::from([
            ("a".to_owned(), "旧名".to_owned()),
            ("b".to_owned(), "删前记下的".to_owned()),
            ("gone".to_owned(), "已清除".to_owned()),
        ]);
        let (names, keep) = merge_names(&listed, &opened, &remembered);
        assert_eq!(names.get("a").map(String::as_str), Some("新名"));
        assert_eq!(names.get("b").map(String::as_str), Some("删前记下的"));
        assert_eq!(names.get("c"), None);
        assert_eq!(
            keep,
            BTreeMap::from([
                ("a".to_owned(), "新名".to_owned()),
                ("b".to_owned(), "删前记下的".to_owned()),
            ])
        );
    }

    /// 解开库名：同一把主密钥封的能解开；钥匙不对就是 `None`。
    #[test]
    fn a_vault_name_opens_with_its_key_and_not_with_another() {
        let remote = Uuid::from_u128(7);
        let cmk = Cmk::generate(1);
        let sealed = cmk.derive(remote).seal_vault_name("读书");
        assert_eq!(
            open_name(remote, std::slice::from_ref(&cmk), &sealed).as_deref(),
            Some("读书")
        );
        assert_eq!(open_name(remote, &[Cmk::generate(1)], &sealed), None);
        assert_eq!(open_name(remote, &[], &sealed), None);
    }

    /// 删除 / 恢复失败的稳定码：成员删组织库、恢复超容量、恢复期已过各有一句；其余按同步码表。
    #[test]
    fn manage_errors_map_to_their_own_codes() {
        let server = |status: u16, code: &str| CloudError::Server {
            status,
            error_code: code.into(),
        };
        assert_eq!(
            manage_error(server(403, "forbidden"), Manage::Delete),
            "sync_manage_forbidden"
        );
        assert_eq!(
            manage_error(server(403, "forbidden"), Manage::Restore),
            "sync_manage_forbidden"
        );
        assert_eq!(
            manage_error(server(402, "sync_quota_exceeded"), Manage::Restore),
            "sync_restore_over_capacity"
        );
        assert_eq!(
            manage_error(server(404, "not_found"), Manage::Restore),
            "sync_restore_expired"
        );
        assert_eq!(
            manage_error(server(404, "not_found"), Manage::Delete),
            "not_found"
        );
        assert_eq!(
            manage_error(CloudError::AccountFrozen, Manage::Delete),
            "account_frozen"
        );
        assert_eq!(
            restore_outcome(Err(server(409, "vault_not_deleted"))),
            Ok(())
        );
        assert_eq!(
            restore_outcome(Err(server(402, "sync_quota_exceeded"))),
            Err("sync_restore_over_capacity".into())
        );
    }

    fn known(path: &Path) -> VaultEntry {
        VaultEntry {
            path: std::fs::canonicalize(path).unwrap(),
            name: "p".into(),
            last_opened_ms: 0,
            id: None,
            accounts: Vec::new(),
        }
    }

    /// 选的是存放位置：在其中建与云端库同名的文件夹（真机 2026-10-16：选了 `Downloads\\Notes`，
    /// 库就直接落在 `Notes` 里，名字对不上）。
    #[test]
    fn the_download_lands_in_a_folder_named_after_the_cloud_vault() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(parent.join("a.md"), b"x").unwrap();
        let (root, created) = download_target(&parent, "SyncTest", &[]).unwrap();
        assert_eq!(root, parent.join("SyncTest"));
        assert!(created && root.is_dir());
        // 同名的有内容：不碰，改用「名字 (2)」。
        std::fs::write(root.join("note.md"), b"x").unwrap();
        let (second, created) = download_target(&parent, "SyncTest", &[]).unwrap();
        assert_eq!(second, parent.join("SyncTest (2)"));
        assert!(created);
        // 同名的空文件夹：直接用，不算新建（失败时不整个删掉）。
        let empty = parent.join("Empty");
        std::fs::create_dir(&empty).unwrap();
        std::fs::write(empty.join(".DS_Store"), b"x").unwrap();
        assert_eq!(
            download_target(&parent, "Empty", &[]).unwrap(),
            (empty.clone(), false)
        );
    }

    /// 选中的本就是一个与库同名的空文件夹：直接用它，不再套一层 `SyncTest/SyncTest`。
    #[test]
    fn picking_an_empty_folder_already_named_like_the_vault_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let picked = std::fs::canonicalize(dir.path()).unwrap().join("SyncTest");
        std::fs::create_dir(&picked).unwrap();
        assert_eq!(
            download_target(&picked, "SyncTest", &[]).unwrap(),
            (picked.clone(), false)
        );
        // 同名但有内容：在它里面建。
        std::fs::write(picked.join("a.md"), b"x").unwrap();
        let (root, created) = download_target(&picked, "SyncTest", &[]).unwrap();
        assert_eq!(root, picked.join("SyncTest"));
        assert!(created);
    }

    /// 存放位置不能在已知库里；新建的那一层也不能撞上已知库；路径要绝对、要存在。
    #[test]
    fn the_target_stays_out_of_known_vaults() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(dir.path()).unwrap();
        let vault = parent.join("Vault");
        std::fs::create_dir(&vault).unwrap();
        assert_eq!(
            download_target(&vault, "SyncTest", &[known(&vault)]).unwrap_err(),
            "sync_folder_inside_vault"
        );
        // 存放位置里装着一个已知库是可以的：新建的同名文件夹与它不重叠。
        let (root, _) = download_target(&parent, "SyncTest", &[known(&vault)]).unwrap();
        assert_eq!(root, parent.join("SyncTest"));
        // 同名的那一格正好是个已知库（空的也不碰）：跳到下一个名字。
        let (next, _) = download_target(&parent, "Vault", &[known(&vault)]).unwrap();
        assert_eq!(next, parent.join("Vault (2)"));
        assert_eq!(
            download_target(&parent.join("missing"), "x", &[]).unwrap_err(),
            "sync_folder_unavailable"
        );
        assert_eq!(
            download_target(Path::new("relative"), "x", &[]).unwrap_err(),
            "invalid_input"
        );
    }

    /// 库名 → 三个平台都能用的文件夹名：非法字符换掉、保留名补后缀、按字符截、读不出用 id。
    #[test]
    fn cloud_vault_names_become_safe_folder_names() {
        let id = Uuid::parse_str("01a14296-6320-7c90-9ee9-dd85620adafe").unwrap();
        assert_eq!(folder_name_for(Some("SyncTest"), &id), "SyncTest");
        assert_eq!(
            folder_name_for(Some("工作 / 笔记: 2026?"), &id),
            "工作 _ 笔记_ 2026_"
        );
        assert_eq!(folder_name_for(Some("  .hidden. "), &id), "hidden");
        assert_eq!(folder_name_for(Some("con"), &id), "con_");
        assert_eq!(folder_name_for(Some("LPT1.txt"), &id), "LPT1.txt_");
        assert_eq!(folder_name_for(Some("a\tb"), &id), "a_b");
        assert_eq!(folder_name_for(Some("..."), &id), "Lunarca 01a14296");
        assert_eq!(folder_name_for(None, &id), "Lunarca 01a14296");
        let long = "长".repeat(100);
        assert_eq!(folder_name_for(Some(&long), &id).chars().count(), 80);
    }
}
