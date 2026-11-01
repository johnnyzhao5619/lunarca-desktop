//! 数据足迹与彻底清除（PRD-09-026，TDD-12 §11）+ 设置导入 / 导出（PRD-09-050，TDD-05 §7.4）。
//!
//! 「数据归你所有、可证明地走干净」：**单一处**枚举 Lunarca 在这台机器上留下的全部痕迹
//! 与各自占用，并给分项清除。边界（TDD-12 §11 表）：
//! - **Vault 是用户的**：只报路径与占用，绝不删；能清的只有可重建缓存与回收站。
//! - **模型**：清除全部本地模型走与逐个删除同一条路径（分配槽先解除，捆绑模型不动）。
//! - **凭据**：钥匙串条目不随应用删除而清，此处给一键清除。
//! - **重置应用** = 备份旧配置 + 设备级设置回默认 + 清应用缓存 / 诊断文件；**不碰 Vault、
//!   模型、凭据与本机记录**（各有自己的清除入口），之后需重启。
//!
//! 占用的统计在阻塞线程里做（模型目录可达数 GB），不挡 IPC。

use super::*;

/// 足迹里的一项：`id` 是稳定码（前端据此取文案），路径与占用是事实，`clearable` 说明
/// 本面板有没有它的清除动作（没有的在文案里指向它自己的入口）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct FootprintEntryDto {
    pub id: String,
    pub path: Option<String>,
    /// 磁盘占用（字节；`f64` 因 specta 无 u64）。`None` = 不适用 / 不存在。
    pub bytes: Option<f64>,
    /// 条目数（回收站条数、凭据条数）。
    pub count: Option<u32>,
    pub clearable: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct DataFootprintDto {
    pub entries: Vec<FootprintEntryDto>,
}

/// 设置导入结果：写入了多少设备级 / Vault 级键。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SettingsImportSummaryDto {
    pub device: u32,
    pub vault: u32,
}

/// 一条将被导入改变的设置（预览用；值本身不过界——界面只需知道「哪几项会变」）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SettingsImportChangeDto {
    /// 设置 key（确认时原样交回，作为核对依据）。
    pub key: String,
    /// 标签的 i18n key（界面直接翻译，不按 key 拼）。
    pub i18n_key: String,
    /// `device` / `vault`。
    pub scope: String,
}

/// 设置导入的**预览**（2026-10-08 C46）：文件已通过与导入相同的全量校验，
/// 列出与此刻不同的项，用户确认后才写。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SettingsImportPreviewDto {
    /// 选中的文件（确认时原样交回；导入那一刻会重读重验，不信任预览期间的内容）。
    pub path: String,
    /// 会改变的项（按注册表顺序）。
    pub changes: Vec<SettingsImportChangeDto>,
    /// 文件里与此刻相同、导入后不变的项数。
    pub unchanged: u32,
}

/// 一次导入判定：已校验的导入内容、会改变的 `(key, scope)` 与不变项计数。
struct ImportChanges {
    import: luna_config::SettingsImport,
    changes: Vec<(String, &'static str)>,
    unchanged: u32,
}

/// 读文件 → 全量校验 → 与当前设置比较。导入与预览共用一个判定。
fn import_changes(state: &AppState, path: &std::path::Path) -> Result<ImportChanges, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("SETTINGS_IMPORT_READ_FAILED: {e}"))?;
    let import = luna_config::parse_export_toml(&text)
        .map_err(|e| format!("SETTINGS_IMPORT_REJECTED: {e}"))?;
    if import.vault.is_some() && state.vault_root().is_none() {
        return Err(state.vault_not_open());
    }
    let current = state.settings_snapshot();
    let mut changes = Vec::new();
    let mut unchanged = 0u32;
    let entries = import
        .device
        .iter()
        .map(|entry| (entry, "device"))
        .chain(import.vault.iter().flatten().map(|entry| (entry, "vault")));
    for ((key, value), scope) in entries {
        if current.get(key).as_ref() == Some(value) {
            unchanged += 1;
        } else {
            changes.push((key.clone(), scope));
        }
    }
    Ok(ImportChanges {
        import,
        changes,
        unchanged,
    })
}

/// 两份「会改变的项」是否一致（顺序无关，**纯函数**）。
fn same_changes(actual: &[(String, &'static str)], confirmed: &[String]) -> bool {
    actual.len() == confirmed.len()
        && actual
            .iter()
            .all(|(key, _)| confirmed.iter().any(|item| item == key))
}

/// 目录占用（跟随不了符号链接就跳过；`skip` 子树不计）。
use luna_vault::diskusage::path_bytes;

/// 一条路径占多少磁盘——**委派 `luna_vault::diskusage`**。
///
/// 回收站列表要报「这一条占多少」，本面板要报「回收站一共占多少」：同一个问题两处
/// 各写一份递归求和，迟早在"符号链接算不算""跳过哪些子树"上分叉，而用户会在同一屏上
/// 看到两个对不上的数字。判据只在那一处。
fn dir_bytes(path: &Path, skip: &[PathBuf]) -> u64 {
    path_bytes(path, skip)
}

/// 只算目录**顶层**里名字满足 `keep` 的常规文件（配置文件那一层）。
fn top_level_files_bytes(dir: &Path, keep: &dyn Fn(&str) -> bool) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter(|entry| keep(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

fn entry(
    id: &str,
    path: Option<&Path>,
    bytes: Option<u64>,
    count: Option<u32>,
    clearable: bool,
) -> FootprintEntryDto {
    FootprintEntryDto {
        id: id.to_owned(),
        path: path.map(|p| p.to_string_lossy().into_owned()),
        bytes: bytes.map(|b| b as f64),
        count,
        clearable,
    }
}

/// 全部数据位置与占用（PRD-09-026）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn data_footprint(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<DataFootprintDto, String> {
    let vault_root = state.vault_root();
    let trash_count = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard.as_ref().map(|vault| vault.list_trash().len())
    };
    let models_root = state.models_root(&app);
    let config_dir = app.path().app_config_dir().ok();
    let data_dir = app.path().app_data_dir().ok();

    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = Vec::new();
        if let Some(root) = vault_root.as_deref() {
            let luna = root.join(".luna");
            let cache_bytes = dir_bytes(&luna.join("cache"), &[])
                + top_level_files_bytes(&luna, &|name| name.starts_with("index.sqlite"));
            entries.push(entry(
                "vault",
                Some(root),
                Some(dir_bytes(root, &[])),
                None,
                false,
            ));
            entries.push(entry(
                "vault_cache",
                Some(&luna),
                Some(cache_bytes),
                None,
                true,
            ));
            let trash = root.join(".trash");
            entries.push(entry(
                "vault_trash",
                Some(&trash),
                Some(dir_bytes(&trash, &[])),
                trash_count.map(|n| n as u32),
                true,
            ));
        } else {
            entries.push(entry("vault", None, None, None, false));
        }
        entries.push(entry(
            "models",
            Some(&models_root),
            Some(dir_bytes(&models_root, &[])),
            None,
            true,
        ));
        if let Some(dir) = config_dir.as_deref() {
            let config_bytes = top_level_files_bytes(dir, &|name| !name.contains(".sqlite"));
            entries.push(entry(
                "app_config",
                Some(dir),
                Some(config_bytes),
                None,
                false,
            ));
        }
        if let Some(dir) = data_dir.as_deref() {
            let records = dir_bytes(&dir.join("metrics"), &[])
                + top_level_files_bytes(dir, &|name| name.contains(".sqlite"));
            entries.push(entry(
                "device_records",
                Some(dir),
                Some(records),
                None,
                false,
            ));
        }
        Ok(DataFootprintDto { entries })
    })
    .await
    .map_err(|e| format!("FOOTPRINT_FAILED: {e}"))?
}

/// 清可重建缓存：`.luna/cache/` + 派生索引行，随后走开库同一条后台扫描重建索引。
/// 快照 / 边车 / 用量 / 设置（durable）一律不动（TDD-06 §4.7）。
#[tauri::command]
#[specta::specta]
pub(crate) fn clear_vault_cache(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.clear_cache().map_err(vault_err)?;
    }
    rebuild_vault_index(app, state)
}

/// 重置应用（PRD-09-026 / TDD-12 §11）：备份旧 `config.toml` → 设备级设置全部回默认 →
/// 清应用缓存目录与启动诊断文件。返回备份路径（内存态配置时为空串）。之后调用方重启。
#[tauri::command]
#[specta::specta]
pub(crate) fn reset_app(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let backup = match &state.settings_persistence {
        SettingsPersistence::Writable(path) if path.is_file() => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = path.with_file_name(format!(
                "{}.bak-{stamp}",
                luna_config::DEVICE_CONFIG_FILENAME
            ));
            std::fs::copy(path, &backup).map_err(|e| format!("RESET_BACKUP_FAILED: {e}"))?;
            backup.to_string_lossy().into_owned()
        }
        _ => String::new(),
    };
    let keys: Vec<&'static str> = luna_config::SETTINGS
        .iter()
        .filter(|spec| spec.scope == SettingScope::Device)
        .map(|spec| spec.key)
        .collect();
    reset_setting_values(&state.settings, &state.settings_persistence, &keys)?;
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join(crate::startup::DIAGNOSTICS_FILE));
    }
    if let Ok(cache) = app.path().app_cache_dir() {
        let _ = std::fs::remove_dir_all(cache);
    }
    Ok(backup)
}

/// 重启应用（重置后由前端在确认页调用）。
///
/// 在主线程上重启：`restart` = 拉起新进程 + `exit`，从 IPC 工作线程直接 `exit` 时主线程
/// 与各常驻线程（事件 tap、采集）还在跑，真机 2026-09-06 就是这样撞出 SIGABRT 的。
#[tauri::command]
#[specta::specta]
pub(crate) fn restart_app(app: tauri::AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        handle.restart();
    });
}

/// 导出设置（PRD-09-050）：设备级必带，`include_vault` 时附带当前库的 Vault 级值；
/// 密钥被注册表 `secret` 标志结构化排除。返回写入路径；用户取消返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    include_vault: bool,
) -> Result<Option<String>, String> {
    if include_vault && state.vault_root().is_none() {
        return Err(state.vault_not_open());
    }
    let toml = state
        .settings_snapshot()
        .export_toml(include_vault)
        .map_err(|e| format!("SETTINGS_EXPORT_FAILED: {e}"))?;
    let Some(dest) =
        super::playback::pick_export_destination(&app, "lunarca-settings", "toml").await?
    else {
        return Ok(None);
    };
    std::fs::write(&dest, toml).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

/// 导入设置第一步（PRD-09-050，2026-10-08 C46）：选文件 → 与导入相同的全量严格校验 →
/// 列出与此刻不同的项。**不写任何东西**；用户取消选择返回 `None`。
///
/// # Errors
/// 读文件失败、校验不过（`SETTINGS_IMPORT_REJECTED`）、带库级设置而没开库时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_settings_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<SettingsImportPreviewDto>, String> {
    let Some(path) = pick_file(app.clone()).await? else {
        return Ok(None);
    };
    let ImportChanges {
        changes, unchanged, ..
    } = import_changes(&state, std::path::Path::new(&path))?;
    Ok(Some(SettingsImportPreviewDto {
        path,
        changes: changes
            .into_iter()
            .map(|(key, scope)| SettingsImportChangeDto {
                i18n_key: luna_config::spec(&key)
                    .map_or_else(|| key.clone(), |spec| spec.i18n_key.to_owned()),
                key,
                scope: scope.to_owned(),
            })
            .collect(),
        unchanged,
    }))
}

/// 导入设置第二步：**重读重验**同一个文件 → 与用户看过的变更清单核对 → 原子写回。
/// 文件或当前设置在两步之间变了（清单对不上）即 `SETTINGS_IMPORT_PLAN_CHANGED`，一项都不改，
/// 界面重新预览。任何一处校验不过同样不改现有配置。
///
/// # Errors
/// 同 [`preview_settings_import`]，外加清单不一致与落盘失败。
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    confirmed: Vec<String>,
) -> Result<SettingsImportSummaryDto, String> {
    let ImportChanges {
        import, changes, ..
    } = import_changes(&state, std::path::Path::new(&path))?;
    if !same_changes(&changes, &confirmed) {
        return Err("SETTINGS_IMPORT_PLAN_CHANGED".to_owned());
    }
    let mut keys: Vec<String> = Vec::new();
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "Settings 锁中毒".to_string())?;
        let mut next = guard.clone();
        for (key, value) in &import.device {
            next.set(key.clone(), value.clone());
            keys.push(key.clone());
        }
        for (key, value) in import.vault.iter().flatten() {
            next.set(key.clone(), value.clone());
            keys.push(key.clone());
        }
        persist_device_settings(&next, &state.settings_persistence)?;
        *guard = next;
    }
    if import.vault.is_some() {
        persist_vault_settings(&state)?;
    }
    for key in &keys {
        apply_setting_side_effects(&app, &state, key);
        let _ = SettingsChanged { key: key.clone() }.emit(&app);
    }
    Ok(SettingsImportSummaryDto {
        device: import.device.len() as u32,
        vault: import.vault.as_ref().map_or(0, |v| v.len() as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确认的清单与此刻算出的清单逐项相等才写：多一项、少一项都要回到预览。
    #[test]
    fn import_requires_the_confirmed_change_list() {
        let actual = vec![
            ("appearance.theme".to_owned(), "device"),
            ("vault.delete_mode".to_owned(), "vault"),
        ];
        let ids = |list: &[&str]| list.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        assert!(same_changes(
            &actual,
            &ids(&["vault.delete_mode", "appearance.theme"])
        ));
        assert!(!same_changes(&actual, &ids(&["appearance.theme"])));
        assert!(!same_changes(
            &actual[..1],
            &ids(&["appearance.theme", "vault.delete_mode"])
        ));
        assert!(same_changes(&[], &[]));
    }

    #[test]
    fn top_level_files_bytes_only_counts_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), [0u8; 7]).unwrap();
        std::fs::write(dir.path().join("usage.sqlite"), [0u8; 9]).unwrap();
        std::fs::create_dir(dir.path().join("models")).unwrap();
        std::fs::write(dir.path().join("models/x"), [0u8; 50]).unwrap();
        assert_eq!(
            top_level_files_bytes(dir.path(), &|n| !n.contains(".sqlite")),
            7
        );
        assert_eq!(
            top_level_files_bytes(dir.path(), &|n| n.contains(".sqlite")),
            9
        );
    }
}
