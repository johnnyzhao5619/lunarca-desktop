//! 快速切换器（PRD-07-040，笔记库对标轮 W1）：条目、最近打开、按名字新建。
//!
//! 只服务主窗口（副库窗口没有切换器）。条目与拼音键由 `luna_vault::switcher` 现算，最近
//! 打开是设备状态（ADR-140，存在应用配置目录、按库身份分存）。

use super::*;
use crate::device_state::DeviceStateKind;

/// 最近打开最多记几篇。
const RECENT_LIMIT: usize = 30;

/// 一条切换器条目（[`luna_vault::SwitcherEntry`] 的 IPC 形状）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SwitcherEntryDto {
    pub path: String,
    pub title: String,
    pub aliases: Vec<String>,
    /// 标题与别名的拼音检索键（全拼与首字母）。
    pub pinyin: Vec<String>,
    /// 资料区（ADR-024）：降权不排除。
    pub is_source_material: bool,
}

/// 打开切换器时要的全部数据。
#[derive(Debug, Clone, Serialize, Type)]
pub struct QuickSwitcherDto {
    pub entries: Vec<SwitcherEntryDto>,
    /// 最近打开（新在前，已不存在的剪掉）：空查询时列这一组。
    pub recent: Vec<String>,
}

/// 一次「按名字新建」的结果。
#[derive(Debug, Clone, Serialize, Type)]
pub struct NamedNoteDto {
    pub path: String,
    /// 同名笔记已在目标目录里：打开它，没有新建。
    pub existed: bool,
}

/// 库相对路径是否安全（不越界、不是绝对路径）。
fn is_inside_vault(rel: &str) -> bool {
    !rel.starts_with('/') && !rel.split('/').any(|part| part == "..")
}

/// 把一次打开记进最近列表：去重、置顶、截断。
fn push_recent(recent: &mut Vec<String>, path: String) {
    recent.retain(|rel| rel != &path);
    recent.insert(0, path);
    recent.truncate(RECENT_LIMIT);
}

/// 快速切换器的条目与最近打开。
#[tauri::command]
#[specta::specta]
pub(crate) fn quick_switcher(
    state: tauri::State<'_, AppState>,
) -> Result<QuickSwitcherDto, String> {
    let (entries, id, root) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        (
            vault.switcher_entries().map_err(vault_err)?,
            vault.id(),
            vault.root().to_path_buf(),
        )
    };
    let recent = state
        .device_state
        .read::<Vec<String>>(id, DeviceStateKind::Recent)
        .unwrap_or_default()
        .into_iter()
        .filter(|rel| is_inside_vault(rel) && root.join(rel).is_file())
        .collect();
    Ok(QuickSwitcherDto {
        entries: entries
            .into_iter()
            .map(|entry| SwitcherEntryDto {
                path: entry.path,
                title: entry.title,
                aliases: entry.aliases,
                pinyin: entry.pinyin,
                is_source_material: entry.is_source_material,
            })
            .collect(),
        recent,
    })
}

/// 记一次「打开了这个文件」（标签切到它时由前端调）。文件夹卡片、新标签页等不是文件的
/// 一律忽略——最近打开只列能打开的东西。
#[tauri::command]
#[specta::specta]
pub(crate) fn record_recent_note(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let (id, root) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        (vault.id(), vault.root().to_path_buf())
    };
    if !is_inside_vault(&path) || !root.join(&path).is_file() {
        return Ok(());
    }
    state
        .device_state
        .update::<Vec<String>>(id, DeviceStateKind::Recent, |recent| {
            push_recent(recent, path)
        })
}

/// 按名字新建一篇笔记（快速切换器 `⇧Enter`，PRD-07-040 ③）：落默认新笔记目录。名字的
/// 校验与改名同一口径（[`renamed_entry_path`]：不许路径分隔符、空名，保留用户的空格）。
/// 目标目录里已有同名笔记就打开它，不造一篇「名字-1」——用户打的就是这个名字。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_note_named(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<NamedNoteDto, String> {
    let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
    let target = renamed_entry_path(state.copy(), &format!("{dir}/_.md"), name.trim(), false)?;
    let created = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        match vault.create(&target, NoteType::Note, &TemplateVars::default()) {
            Ok(doc) => Some(doc.path),
            Err(VaultError::AlreadyExists(_)) => None,
            Err(error) => return Err(vault_err(error)),
        }
    };
    let Some(path) = created else {
        return Ok(NamedNoteDto {
            path: target,
            existed: true,
        });
    };
    emit_vault_changed(&app, &path, false);
    Ok(NamedNoteDto {
        path,
        existed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_paths_must_stay_inside_the_vault() {
        assert!(is_inside_vault("项目/甲.md"));
        assert!(!is_inside_vault("../外面.md"));
        assert!(!is_inside_vault("项目/../../外面.md"));
        assert!(!is_inside_vault("/etc/hosts"));
    }

    #[test]
    fn recent_moves_a_reopened_note_to_the_top_and_stays_bounded() {
        let mut recent = vec!["a.md".to_owned(), "b.md".to_owned()];
        push_recent(&mut recent, "b.md".to_owned());
        assert_eq!(recent, vec!["b.md".to_owned(), "a.md".to_owned()]);
        for index in 0..(RECENT_LIMIT + 5) {
            push_recent(&mut recent, format!("{index}.md"));
        }
        assert_eq!(recent.len(), RECENT_LIMIT);
        assert_eq!(recent[0], format!("{}.md", RECENT_LIMIT + 4));
    }
}
