//! 官方同步的命令面（ADR-148、ADR-150、ADR-151；TDD-14 §5）。业务在 `luna-sync` 与
//! [`crate::sync_service`]，这里只做闸门（严格本地、登录、权益）、逐库开启 / 加入 / 暂停 / 停止的
//! 编排与稳定码翻译。库一律按库根路径指认（库注册表的唯一键），与它此刻是否打开无关。
//!
//! 同步跟着账户走（ADR-150）：开启与加入都不收任何秘密——库主密钥由账户托管，登录同一账户的
//! 设备从服务端取回。绑定归账户且排他（ADR-151 修订 2026-10-15）：每条命令只认此刻登录的账户
//! 名下的绑定；连在另一个账户下的库要显式「改为同步到当前账户」（`take_over`）才能开启，新绑定
//! 落下时才解除原来那一份。
//!
//! 错误一律是稳定码字符串（界面据此取文案）：`sync_strict_local`、`sync_signed_out`、
//! `sync_not_entitled`、`sync_no_vault`、`sync_vault_unavailable`、`sync_already_enabled`、
//! `sync_bound_elsewhere`、`sync_not_enabled`、`sync_remote_in_use`、`sync_no_managed_key`、
//! `sync_keychain`、`sync_state_db`，以及服务端的同步码（TDD-16 §9）。

use std::path::{Path, PathBuf};

use luna_cloud::client::CloudClient;
use luna_cloud::sync_client::{Base64Bytes, SyncCreateVault, SyncEnvelopeInput};
use luna_sync::keystore::CmkStore;
use luna_sync::state::{Binding, BindingOwner, SyncState};
use luna_sync_core::crypto::{Cmk, KeyRing};
use uuid::Uuid;

use super::*;
use crate::cloud_sync::{managed_keys, remote_error, CredentialCmkStore, MANAGED_SLOT};
use crate::sync_service::{
    bind_local, bindable, identity_of, release_vault, state_dir, unpin_all, write_pause,
    SyncOverviewDto, SyncService,
};

/// 一个远端库（加入已有库时选）。库名用托管的库主密钥解开；解不开（端到端的高级档、
/// 信封取不到）时为 `None`，界面退回按创建时间认。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RemoteVaultDto {
    pub id: String,
    pub name: Option<String>,
    pub created_at: String,
    /// 已存密文字节（`f64` 因 specta 不收 64 位整数）。
    pub bytes_stored: f64,
}

pub(super) fn service(app: &tauri::AppHandle) -> Result<Arc<SyncService>, String> {
    app.try_state::<Arc<SyncService>>()
        .map(|s| Arc::clone(&s))
        .ok_or_else(|| "sync_unavailable".to_owned())
}

/// 此刻登录的账户（绑定的归属）；认不出是谁 = `sync_signed_out`。
pub(super) fn signed_in_owner(client: &CloudClient) -> Result<BindingOwner, String> {
    crate::account_scope::current(client).ok_or_else(|| "sync_signed_out".to_owned())
}

/// 这份库在此刻账户名下的绑定；没登录 = `sync_signed_out`，没有或归别的账户 = `None`。
fn own_binding(app: &tauri::AppHandle, local_vault_id: &str) -> Result<Option<Binding>, String> {
    let owner =
        crate::account_scope::current_in(app).ok_or_else(|| "sync_signed_out".to_owned())?;
    let path = state_path(app, local_vault_id)?;
    if !path.is_file() {
        return Ok(None);
    }
    Ok(SyncState::open(&path)
        .ok()
        .and_then(|state| state.binding().ok().flatten())
        .filter(|binding| binding.owner.same_account(&owner)))
}

/// 严格本地 → 登录 → 拿到云客户端。
pub(super) fn gate(app: &tauri::AppHandle) -> Result<Arc<CloudClient>, String> {
    let state = app.state::<AppState>();
    if state.strict_local() {
        return Err("sync_strict_local".into());
    }
    let client = app
        .try_state::<luna_cloud::commands::CloudState>()
        .map(|s| Arc::clone(&s.0))
        .ok_or_else(|| "sync_signed_out".to_owned())?;
    if !client.logged_in() {
        return Err("sync_signed_out".into());
    }
    Ok(client)
}

/// 权益里有同步（与服务端写入闸同一条规则，`/v1/entitlements` 的 `sync.enabled`）。
pub(super) async fn require_entitlement(client: &CloudClient) -> Result<(), String> {
    let entitlements = client
        .entitlements()
        .await
        .map_err(|e| remote_error(e).code())?;
    let enabled = entitlements
        .entitlements
        .iter()
        .any(|e| e.key == "sync.enabled" && e.value_json.trim() == "true");
    if enabled {
        Ok(())
    } else {
        Err("sync_not_entitled".into())
    }
}

/// 一个已知库（按库根路径指认）。
pub(super) struct Target {
    pub(super) id: String,
    pub(super) root: PathBuf,
    pub(super) name: String,
}

/// 按库根路径找到库注册表里的那一份。身份还没登记过（本版本没打开过）的库当场打开一次补上身份。
pub(super) fn target(app: &tauri::AppHandle, vault_path: &str) -> Result<Target, String> {
    let entry = app
        .state::<AppState>()
        .known_vaults()
        .into_iter()
        .find(|e| e.path == std::path::Path::new(vault_path))
        .ok_or_else(|| "sync_no_vault".to_owned())?;
    let id = match identity_of(&entry) {
        Some(id) => id,
        None => Vault::open(&entry.path)
            .map(|v| v.id().to_string())
            .map_err(|_| "sync_vault_unavailable".to_owned())?,
    };
    Ok(Target {
        id,
        root: entry.path,
        name: entry.name,
    })
}

pub(super) fn state_path(app: &tauri::AppHandle, local_vault_id: &str) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "sync_state_db".to_owned())?;
    Ok(state_dir(&config_dir, local_vault_id).join("state.sqlite"))
}

pub(super) fn keys() -> CredentialCmkStore {
    crate::secrets::cmk_store()
}

/// 停止这台设备上这份库的同步，不论它归哪个账户：摘下会话、忘掉主密钥缓存、解掉基线钉住、删掉
/// 同步目录（状态库与块缓存）。云端库与本地文件都不动。返回原来绑定的远端库；本来就没有绑定 =
/// `None`。「停止同步」、「管理库」移除与账户页删除云端库共用这一处（ADR-151 修订 2026-10-15）。
///
/// # Errors
/// `sync_unavailable`、`sync_state_db`、`sync_keychain`。
pub(crate) async fn release_local(
    app: &tauri::AppHandle,
    local_vault_id: &str,
    root: &Path,
) -> Result<Option<Uuid>, String> {
    let path = state_path(app, local_vault_id)?;
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "sync_state_db".to_owned())?;
    let detached = service(app)?.detach(app, local_vault_id).await;
    release_vault(&dir, detached, &keys(), |holder| {
        unpin_all(app, local_vault_id, root, holder);
    })
}

/// 存钥匙、落绑定（归此刻的账户；`replaced` 是改连过来的原账户绑定，此刻才解除）、装进会话，
/// 并把这个库列进绑定账户的库列表（开启、加入、改连、下载到本机都经这里；ADR-151 修订 2026-10-15
/// 「续二」）。改连时原账户的列表不动：它再登录时看到的是「已连接到另一个账户」。
#[allow(clippy::too_many_arguments)]
pub(super) async fn bind_and_install(
    app: &tauri::AppHandle,
    client: &CloudClient,
    owner: BindingOwner,
    local_vault_id: &str,
    root: PathBuf,
    remote: Uuid,
    cmks: &[Cmk],
    replaced: Option<Binding>,
) -> Result<(), String> {
    let account = crate::account_scope::account_key(&owner);
    let store = keys();
    for cmk in cmks {
        store
            .save(remote, cmk)
            .map_err(|_| "sync_keychain".to_owned())?;
    }
    let path = state_path(app, local_vault_id)?;
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "sync_state_db".to_owned())?;
    let state = bind_local(
        &dir,
        &Binding {
            remote_vault_id: remote,
            server_origin: client.config().api_base.clone(),
            local_vault_id: local_vault_id.to_owned(),
            created_ms: chrono::Utc::now().timestamp_millis(),
            owner,
        },
        replaced.as_ref(),
        &store,
        |holder| unpin_all(app, local_vault_id, &root, holder),
    )?;
    let ring = KeyRing::new(remote, cmks).ok_or_else(|| "sync_keychain".to_owned())?;
    app.state::<AppState>()
        .vault_registry
        .add_account(&root, &account);
    service(app)?
        .install(app, local_vault_id, root, remote, state, ring)
        .await;
    Ok(())
}

/// 同步总览：准入门、全部暂停与每个已知库一行（零 I/O）。
#[tauri::command]
#[specta::specta]
pub async fn sync_overview(app: tauri::AppHandle) -> Result<SyncOverviewDto, String> {
    Ok(service(&app)?.overview())
}

/// 本账户的远端库（加入已有库时选），带解开的库名。
#[tauri::command]
#[specta::specta]
pub async fn sync_remote_vaults(app: tauri::AppHandle) -> Result<Vec<RemoteVaultDto>, String> {
    let client = gate(&app)?;
    let vaults = client
        .sync_vaults()
        .await
        .map_err(|e| remote_error(e).code())?;
    let mut out = Vec::with_capacity(vaults.len());
    // 已删除的库（恢复期内）加入不了：数据面一律 410，恢复在账户页「云端库」。
    for vault in vaults.into_iter().filter(|v| !v.is_deleted()) {
        let name = match Uuid::parse_str(&vault.id) {
            Ok(remote) => vault_name(&client, remote, &vault.name_ct.0).await,
            Err(_) => None,
        };
        out.push(RemoteVaultDto {
            id: vault.id,
            name,
            created_at: vault.created_at,
            bytes_stored: vault.bytes_stored as f64,
        });
    }
    Ok(out)
}

/// 用托管的库主密钥解开库名；任何一步失败都退回 `None`（名字只是帮用户认库）。
pub(super) async fn vault_name(
    client: &CloudClient,
    remote: Uuid,
    name_ct: &[u8],
) -> Option<String> {
    let cmks = managed_keys(client, remote).await.ok()?;
    let ring = KeyRing::new(remote, &cmks)?;
    ring.for_ciphertext(name_ct)
        .ok()?
        .open_vault_name(name_ct)
        .ok()
}

/// 为一个已知库新建云端库并开启同步。库主密钥在本机生成，作为账户托管信封交给服务端
/// （ADR-150 ①）——不设密码，没有恢复密钥。`take_over` = 这个库连在另一个账户下、用户确认了
/// 改为同步到当前账户（ADR-151 修订 2026-10-15）。
#[tauri::command]
#[specta::specta]
pub async fn sync_enable(
    app: tauri::AppHandle,
    vault_path: String,
    take_over: bool,
) -> Result<(), String> {
    let client = gate(&app)?;
    let owner = signed_in_owner(&client)?;
    let Target { id, root, name } = target(&app, &vault_path)?;
    // 先在本机判排他（零网络）：连在另一个账户下又没确认改连，就不为它发任何请求。
    let replaced = bindable(&state_path(&app, &id)?, &owner, take_over)?;
    require_entitlement(&client).await?;

    let remote = Uuid::now_v7();
    let cmk = Cmk::generate(1);
    let vault_id = remote.to_string();
    client
        .sync_create_vault(&SyncCreateVault {
            id: vault_id,
            name_ct: Base64Bytes(cmk.derive(remote).seal_vault_name(&name)),
            envelope: SyncEnvelopeInput {
                slot: MANAGED_SLOT.into(),
                cmk_version: 1,
                wrapped_cmk: Base64Bytes(cmk.expose_bytes().to_vec()),
                kdf_params: serde_json::json!({}),
            },
        })
        .await
        .map_err(|e| remote_error(e).code())?;
    bind_and_install(&app, &client, owner, &id, root, remote, &[cmk], replaced).await
}

/// 让一个已知库加入本账户的一个已有云端库：库主密钥从账户托管信封取回。
///
/// 同一个云端库在这台设备上只对应一个本地库——两份本地文件夹同步到同一个云端库，它们就成了
/// 彼此的镜像，删一边另一边跟着删，那不是用户想要的「多选」。`take_over` 同 [`sync_enable`]。
#[tauri::command]
#[specta::specta]
pub async fn sync_join(
    app: tauri::AppHandle,
    vault_path: String,
    remote_vault_id: String,
    take_over: bool,
) -> Result<(), String> {
    let client = gate(&app)?;
    let owner = signed_in_owner(&client)?;
    let Target { id, root, .. } = target(&app, &vault_path)?;
    let remote = Uuid::parse_str(&remote_vault_id).map_err(|_| "invalid_input".to_owned())?;
    let replaced = bindable(&state_path(&app, &id)?, &owner, take_over)?;
    require_entitlement(&client).await?;
    let in_use = service(&app)?
        .overview()
        .vaults
        .iter()
        .any(|v| v.remote_vault_id.as_deref() == Some(remote_vault_id.as_str()));
    if in_use {
        return Err("sync_remote_in_use".into());
    }
    let cmks = managed_keys(&client, remote).await.map_err(|e| e.code())?;
    if cmks.is_empty() {
        return Err("sync_no_managed_key".into());
    }
    bind_and_install(&app, &client, owner, &id, root, remote, &cmks, replaced).await
}

/// 立即同步一轮。
#[tauri::command]
#[specta::specta]
pub async fn sync_now(app: tauri::AppHandle) -> Result<(), String> {
    service(&app)?.nudge();
    Ok(())
}

/// 按需下载 / 释放本机空间的结果（ADR-156）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OnDemandDto {
    Done,
    /// 本来就在本机（下载）/ 本来就只在云端（释放）。
    AlreadyThere,
    /// 同步里没有这个文件，或云端已没有这一版（下一轮同步会更新）。
    NotFound,
    /// 那个位置有别的文件，或本机这份改过还没同步完（不能释放）。
    Changed,
    Failed,
}

impl From<luna_sync::engine::OnDemand> for OnDemandDto {
    fn from(value: luna_sync::engine::OnDemand) -> Self {
        use luna_sync::engine::OnDemand as O;
        match value {
            O::Done => Self::Done,
            O::AlreadyThere => Self::AlreadyThere,
            O::NotFound => Self::NotFound,
            O::Changed => Self::Changed,
            O::Failed => Self::Failed,
        }
    }
}

/// 打开的库里按需下载一个只在云端的文件（ADR-156；进度经 `sync://download-progress`）。
#[tauri::command]
#[specta::specta]
pub async fn sync_materialize(app: tauri::AppHandle, path: String) -> Result<OnDemandDto, String> {
    service(&app)?
        .on_demand(&app, &path, false)
        .await
        .map(Into::into)
}

/// 释放本机空间：本机这份与云端一致时移除本机副本，变回只在云端（ADR-156；不进回收站）。
#[tauri::command]
#[specta::specta]
pub async fn sync_evict(app: tauri::AppHandle, path: String) -> Result<OnDemandDto, String> {
    service(&app)?
        .on_demand(&app, &path, true)
        .await
        .map(Into::into)
}

/// 单库暂停 / 继续（ADR-151 ③；跨重启保持）。
#[tauri::command]
#[specta::specta]
pub async fn sync_set_vault_paused(
    app: tauri::AppHandle,
    vault_path: String,
    paused: bool,
) -> Result<(), String> {
    let Target { id, .. } = target(&app, &vault_path)?;
    let service = service(&app)?;
    service.set_vault_paused(&id, paused).await?;
    service.refresh(&app).await;
    Ok(())
}

/// 全部暂停（ADR-151 ③）：`hours` 给了就到点自动恢复，`None` = 直到用户恢复。跨重启保持。
#[tauri::command]
#[specta::specta]
pub async fn sync_pause_all(app: tauri::AppHandle, hours: Option<u32>) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "sync_state_db".to_owned())?;
    let until = hours.map(|h| chrono::Utc::now().timestamp_millis() + i64::from(h) * 3_600_000);
    write_pause(&config_dir, Some(until)).map_err(|_| "sync_state_db".to_owned())?;
    service(&app)?.refresh(&app).await;
    Ok(())
}

/// 恢复全部同步。
#[tauri::command]
#[specta::specta]
pub async fn sync_resume_all(app: tauri::AppHandle) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "sync_state_db".to_owned())?;
    write_pause(&config_dir, None).map_err(|_| "sync_state_db".to_owned())?;
    let service = service(&app)?;
    service.refresh(&app).await;
    service.nudge();
    Ok(())
}

/// 用户确认了这个库的批量删除：下一轮放行。
#[tauri::command]
#[specta::specta]
pub async fn sync_confirm_bulk_delete(
    app: tauri::AppHandle,
    vault_path: String,
) -> Result<(), String> {
    let Target { id, .. } = target(&app, &vault_path)?;
    service(&app)?.confirm_bulk_delete(&id).await;
    Ok(())
}

/// 停止同步这个库（ADR-151 ④；只停这台设备）：本机忘掉主密钥、解掉基线钉住、删掉本机同步状态与
/// 块缓存（[`release_local`]）。云端库与本地文件都不动，其他设备照常同步，之后可以重新加入。
/// 只停此刻账户名下的绑定；连在另一个账户下的库 = `sync_bound_elsewhere`（ADR-151 修订
/// 2026-10-15）。
#[tauri::command]
#[specta::specta]
pub async fn sync_disable(app: tauri::AppHandle, vault_path: String) -> Result<(), String> {
    let Target { id, root, .. } = target(&app, &vault_path)?;
    let path = state_path(&app, &id)?;
    let bound = own_binding(&app, &id)?;
    if bound.is_none() && path.is_file() {
        let elsewhere = luna_sync::state::peek_owner(&path).ok().flatten().is_some();
        if elsewhere {
            return Err("sync_bound_elsewhere".into());
        }
    }
    if release_local(&app, &id, &root).await?.is_none() {
        return Err("sync_not_enabled".into());
    }
    // 断开 = 回到本机库：不再属于任何账户，谁都看得见（ADR-151 修订 2026-10-15「续二」）。
    app.state::<AppState>().vault_registry.make_device(&root);
    service(&app)?.nudge();
    Ok(())
}

// ── 整库时间点恢复（PRD-13-007、ADR-148 ⑪）─────────────────────────────────────

/// 时间点恢复的预览：那一刻对应的状态与默认落点。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RestorePointDto {
    /// 要恢复的序号（`f64` 因 specta 不收 64 位整数）。
    pub at_seq: f64,
    /// 那一刻最后一次提交的时刻（Unix 毫秒）；早于第一次提交为 `None`（会恢复出空文件夹）。
    pub committed_at_ms: Option<f64>,
    /// 最早可恢复到的时刻（历史保留下限）。
    pub earliest_ms: f64,
    /// 默认恢复到的新文件夹（库旁边，不存在的名字）。
    pub destination: String,
}

/// 时间点恢复的结果。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RestoreReportDto {
    pub files: f64,
    pub bytes: f64,
    pub skipped: f64,
    pub destination: String,
}

/// 恢复进度（每个文件之后一次）。
#[derive(Debug, Clone, Serialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "sync://restore-progress")]
pub struct SyncRestoreProgress {
    pub done: f64,
    pub total: f64,
}

/// 这份库在此刻账户名下绑定的远端库。
fn bound_remote(app: &tauri::AppHandle, local_vault_id: &str) -> Result<Uuid, String> {
    own_binding(app, local_vault_id)?
        .map(|binding| binding.remote_vault_id)
        .ok_or_else(|| "sync_not_enabled".to_owned())
}

fn rfc3339_ms(raw: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|t| t.timestamp_millis() as f64)
}

/// `parent` 里一个还不存在的文件夹：`<库名>（恢复到 2026-10-12 1530）`（措辞走 Fluent
/// `sync-restore-folder-name`），重名加序号。时间取**所选时刻**，不是点下恢复的时刻。
fn default_destination(i18n: BackendCopy<'_>, parent: &Path, name: &str, at_ms: i64) -> PathBuf {
    let stamp = chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H%M")
                .to_string()
        })
        .unwrap_or_default();
    let base = i18n
        .and_then(|i18n| {
            i18n.translate_args(
                "sync-restore-folder-name",
                &[("name", name.to_owned()), ("time", stamp.clone())],
            )
        })
        .unwrap_or_else(|| format!("{name} ({stamp})"));
    let mut candidate = parent.join(&base);
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{base} {n}"));
        n += 1;
    }
    candidate
}

/// 预览：`at_ms` 那一刻对应的状态，以及恢复到哪个新文件夹（`parent` 缺省 = 库旁边）。
/// 读操作，失去同步权益（只读宽限）时照样可用。
#[tauri::command]
#[specta::specta]
pub async fn sync_restore_point(
    app: tauri::AppHandle,
    vault_path: String,
    at_ms: f64,
    parent: Option<String>,
) -> Result<RestorePointDto, String> {
    let client = gate(&app)?;
    let Target { id, root, name } = target(&app, &vault_path)?;
    let remote = bound_remote(&app, &id)?;
    #[allow(clippy::cast_possible_truncation)]
    let at_ms = at_ms as i64;
    let at = chrono::DateTime::from_timestamp_millis(at_ms)
        .ok_or_else(|| "invalid_input".to_owned())?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let point = client
        .sync_restore_point(&remote.to_string(), &at)
        .await
        .map_err(|e| remote_error(e).code())?;
    Ok(RestorePointDto {
        at_seq: point.at_seq as f64,
        committed_at_ms: point.committed_at.as_deref().and_then(rfc3339_ms),
        earliest_ms: rfc3339_ms(&point.earliest_at).unwrap_or(0.0),
        destination: default_destination(
            app.state::<AppState>().copy(),
            parent
                .as_deref()
                .map_or_else(|| root.parent().unwrap_or(&root), Path::new),
            &name,
            at_ms,
        )
        .to_string_lossy()
        .into_owned(),
    })
}

/// 把这份库在 `at_seq` 时的样子恢复到 `destination`（新文件夹，不存在或为空），并登记进库清单
/// （不开同步），便于以副库窗口打开对比。从不碰当前库。
#[tauri::command]
#[specta::specta]
pub async fn sync_restore_to_folder(
    app: tauri::AppHandle,
    vault_path: String,
    at_seq: f64,
    destination: String,
) -> Result<RestoreReportDto, String> {
    use tauri_specta::Event as _;
    let client = gate(&app)?;
    let Target { id, .. } = target(&app, &vault_path)?;
    let remote = bound_remote(&app, &id)?;
    let dest = PathBuf::from(&destination);
    if !dest.is_absolute() {
        return Err("invalid_input".into());
    }
    // 落进任何一份已知库里（或把它们装进来）都不行：那会让两份库互相嵌套。另一个账户的库也算。
    let known = app.state::<AppState>().vault_registry.list();
    if known
        .iter()
        .any(|e| luna_vault::registry::paths_overlap(&e.path, &dest))
    {
        return Err("restore_destination_inside_vault".into());
    }
    let cmks = managed_keys(&client, remote).await.map_err(|e| e.code())?;
    let ring = KeyRing::new(remote, &cmks).ok_or_else(|| "sync_no_managed_key".to_owned())?;
    let port = crate::cloud_sync::CloudRemote::new(Arc::clone(&client));
    #[allow(clippy::cast_possible_truncation)]
    let at_seq = at_seq as i64;
    let progress_app = app.clone();
    let report = luna_sync::restore::restore_to_folder(
        &port,
        &ring,
        remote,
        at_seq,
        &dest,
        |done, total| {
            let _ = SyncRestoreProgress {
                done: done as f64,
                total: total as f64,
            }
            .emit(&progress_app);
        },
    )
    .await
    .map_err(|e| e.code())?;
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned());
    let _ = app.state::<AppState>().vault_registry.register(&dest, name);
    Ok(RestoreReportDto {
        files: report.files as f64,
        bytes: report.bytes as f64,
        skipped: report.skipped as f64,
        destination,
    })
}

#[cfg(test)]
mod restore_tests {
    use super::default_destination;

    #[test]
    fn the_default_destination_sits_beside_the_vault_and_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let zh = crate::i18n::BackendI18n::new("zh-CN");
        let first = default_destination(Some(&zh), dir.path(), "I2X Next", 0);
        assert_eq!(first.parent(), Some(dir.path()));
        assert!(first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("I2X Next（恢复到 "));
        std::fs::create_dir_all(&first).unwrap();
        let second = default_destination(Some(&zh), dir.path(), "I2X Next", 0);
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with(" 2"));
    }
}
