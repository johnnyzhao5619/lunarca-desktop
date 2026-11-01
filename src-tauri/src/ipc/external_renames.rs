//! 库外改名的提示与修复（PRD-06-030 ⑤，笔记库对标轮 V3a）。
//!
//! 两个来源收集「消失的笔记 + 它原来的内容指纹」与「新出现的笔记 + 指纹」：运行时的文件监听
//! （[`record_external_change`]，在同步索引**之前**取旧指纹——同步之后那一行就没了），以及
//! 启动时的增量扫描（[`report_scan_renames`]）。监听这一路攒一阵（安静 1.5 秒）再结算：配对、
//! 算还有几处链接指向旧名，没有断链就什么都不说。
//!
//! **只提示、不自动改**（用户裁决 Q4）。提示存在 [`ExternalRenameWatch`] 里，前端挂载时来取
//! ——启动扫描结束得可能比工作区挂上监听更早。`git checkout` / `git pull` / 同步盘首次下载
//! 这类批量变动（一个窗口内变更数超过阈值）合并成一条，不逐条弹。
//!
//! 提示放在编辑区底部的条子上，而不是通知：通知只能带跳转目标，带不了「修复」这个动作。

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// 监听这一路攒多久再结算（毫秒）：一次 `git mv` 的「删 + 建」可能分在两个去抖批次里。
const SETTLE_MS: u64 = 1_500;

/// 一个窗口内变更超过这么多条，就算批量外部变动，合成一条提示。
const BATCH_THRESHOLD: usize = 20;

/// 一次被认出的库外改名。
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ExternalRenameDto {
    pub from: String,
    pub to: String,
    pub notes: u32,
    pub links: u32,
}

/// 当前的库外改名提示。
#[derive(Debug, Clone, Default, Serialize, Type, PartialEq, Eq)]
pub struct ExternalRenamePromptDto {
    pub renames: Vec<ExternalRenameDto>,
    /// 来自一次批量外部变动（措辞不同：「检测到批量外部变动……」）。
    pub batch: bool,
    /// 那一阵外部变更的总条数（批量时说给用户听）。
    pub changes: u32,
}

/// 一次改名对（修复 / 忽略的入参）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RenamePairDto {
    pub from: String,
    pub to: String,
}

/// 修复的结果。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ExternalRepairDto {
    pub notes: u32,
    pub links: u32,
}

/// 一个结算窗口里攒下的事实。
#[derive(Default)]
struct Window {
    vanished: Vec<(String, String)>,
    appeared: Vec<(String, String)>,
    pairs: Vec<(String, String)>,
    changes: usize,
}

/// 库外改名的收集与当前提示（AppState 持有）。
#[derive(Default)]
pub(crate) struct ExternalRenameWatch {
    window: Mutex<Window>,
    seq: AtomicU64,
    prompt: Mutex<Option<ExternalRenamePromptDto>>,
}

impl ExternalRenameWatch {
    /// 切库：旧库的改名对在新库里没有意义。清空攒着的与已提示的，并让排上的结算作废。
    pub(crate) fn reset(&self) {
        self.seq.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut window) = self.window.lock() {
            *window = Window::default();
        }
        if let Ok(mut prompt) = self.prompt.lock() {
            *prompt = None;
        }
    }
}

/// 库外改名提示变了（有了 / 被修复或忽略）：前端据此重取。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "vault://external-renames")]
pub struct ExternalRenamesChanged;

/// 文件监听来的一条外部变更：在同步索引之前调用。
pub(crate) fn record_external_change(
    app: &tauri::AppHandle,
    vault: &Vault,
    change: &luna_vault::VaultChange,
) {
    let state = app.state::<AppState>();
    let Ok(mut window) = state.external_renames.window.lock() else {
        return;
    };
    window.changes += 1;
    if let Some(from) = change.moved_from.as_ref() {
        window.pairs.push((from.clone(), change.rel_path.clone()));
    } else if let Some(disk) = change.disk_hash.as_ref() {
        if matches!(vault.indexed_hash(&change.rel_path), Ok(None)) {
            window
                .appeared
                .push((change.rel_path.clone(), disk.as_str().to_owned()));
        }
    } else if let Ok(Some(hash)) = vault.indexed_hash(&change.rel_path) {
        window.vanished.push((change.rel_path.clone(), hash));
    }
    drop(window);
    let seq = state.external_renames.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(SETTLE_MS));
        let state = app.state::<AppState>();
        if state.external_renames.seq.load(Ordering::SeqCst) != seq {
            return; // 这一阵还没停：等最后一条之后的那次结算。
        }
        let window = state
            .external_renames
            .window
            .lock()
            .map(|mut window| std::mem::take(&mut *window))
            .unwrap_or_default();
        let mut pairs = window.pairs;
        pairs.extend(luna_vault::pair_external_renames(
            &window.vanished,
            &window.appeared,
        ));
        settle(&app, &pairs, window.changes);
    });
}

/// 启动时增量扫描配出来的改名（扫描本身已攒成一窗，直接结算）。
pub(crate) fn report_scan_renames(
    app: &tauri::AppHandle,
    vanished: &[(String, String)],
    appeared: &[(String, String)],
    changes: usize,
) {
    let pairs = luna_vault::pair_external_renames(vanished, appeared);
    settle(app, &pairs, changes);
}

/// 算断链，有就并进当前提示并通知前端。
fn settle(app: &tauri::AppHandle, pairs: &[(String, String)], changes: usize) {
    if pairs.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let impact = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        match vault.external_rename_impact(pairs) {
            Ok(impact) => impact,
            Err(error) => {
                eprintln!("[vault] 库外改名估算失败：{error}");
                return;
            }
        }
    };
    if impact.is_empty() {
        return;
    }
    if let Ok(mut prompt) = state.external_renames.prompt.lock() {
        let current = prompt.get_or_insert_with(ExternalRenamePromptDto::default);
        for rename in impact {
            current.renames.retain(|known| known.from != rename.from);
            current.renames.push(ExternalRenameDto {
                from: rename.from,
                to: rename.to,
                notes: u32::try_from(rename.notes).unwrap_or(u32::MAX),
                links: u32::try_from(rename.links).unwrap_or(u32::MAX),
            });
        }
        current.batch |= changes > BATCH_THRESHOLD;
        current.changes = current
            .changes
            .saturating_add(u32::try_from(changes).unwrap_or(u32::MAX));
    }
    let _ = ExternalRenamesChanged.emit(app);
}

/// 从提示里去掉这几对（修复了或忽略了）。
fn forget(state: &AppState, pairs: &[RenamePairDto]) {
    if let Ok(mut prompt) = state.external_renames.prompt.lock() {
        if let Some(current) = prompt.as_mut() {
            current
                .renames
                .retain(|known| !pairs.iter().any(|pair| pair.from == known.from));
            if current.renames.is_empty() {
                *prompt = None;
            }
        }
    }
}

/// 当前的库外改名提示（没有为 `None`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn external_rename_prompt(
    state: tauri::State<'_, AppState>,
) -> Option<ExternalRenamePromptDto> {
    state
        .external_renames
        .prompt
        .lock()
        .ok()
        .and_then(|prompt| prompt.clone())
}

/// 修复：把指向旧名的链接改成指向新名（与应用内改名同一个改写器，PRD-06-030 ⑤）。
#[tauri::command]
#[specta::specta]
pub(crate) fn repair_external_renames(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    pairs: Vec<RenamePairDto>,
) -> Result<ExternalRepairDto, String> {
    let raw: Vec<(String, String)> = pairs
        .iter()
        .map(|pair| (pair.from.clone(), pair.to.clone()))
        .collect();
    let (repair, root) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        (
            vault.repair_external_renames(&raw).map_err(vault_err)?,
            vault.root().to_path_buf(),
        )
    };
    forget(&state, &pairs);
    // 打开着的引用方即时刷新（与应用内改名同一条广播，V0b）。
    broadcast_relinked(&app, "main", &root, &repair.rewritten);
    emit_vault_changes(
        &app,
        repair.rewritten.iter().map(|path| (path.clone(), false)),
    );
    let _ = ExternalRenamesChanged.emit(&app);
    Ok(ExternalRepairDto {
        notes: u32::try_from(repair.rewritten.len()).unwrap_or(u32::MAX),
        links: u32::try_from(repair.links).unwrap_or(u32::MAX),
    })
}

/// 忽略这几对：不再提示（链接保持原样）。
#[tauri::command]
#[specta::specta]
pub(crate) fn dismiss_external_renames(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    pairs: Vec<RenamePairDto>,
) {
    forget(&state, &pairs);
    let _ = ExternalRenamesChanged.emit(&app);
}

#[cfg(test)]
mod tests {
    /// 收集必须在同步索引**之前**（PRD-06-030 ⑤）：同步会删掉「消失的那篇」在索引里的一行，
    /// 之后就查不到它原来的内容指纹，库外改名静默认不出来——运行时不会报任何错。
    #[test]
    fn external_changes_are_recorded_before_the_index_is_synced() {
        let source = crate::ipc::source_without_tests(include_str!("mod.rs"));
        let body = source
            .split("fn handle_external_vault_change(")
            .nth(1)
            .expect("找得到 handle_external_vault_change");
        let record = body
            .find("record_external_change(app, vault, &change)")
            .expect("外部变更没有交给库外改名收集");
        let sync = body.find("sync_external_note(").expect("找得到同步");
        assert!(record < sync, "收集排在了同步索引之后");
    }
}
