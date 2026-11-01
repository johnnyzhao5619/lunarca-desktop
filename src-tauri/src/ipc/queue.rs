//! 处理队列的**控制面**命令（PRD-04-055 / PRD-05-010 / PRD-04-062）。
//!
//! 与 [`super::processing`]（队列的执行面：worker、各类任务的处理流程、终态落盘）分开：
//! 这里只有面板按得到的七个动作——取快照、调序、移除、取消、重试、清两段。它们此前
//! 寄居在 `transcribe.rs`，把那个文件顶过了单文件命令数上限；而队列控制本就不属于转写域
//! （队列里跑的是导入 / 抓取 / 蒸馏 / 笔记 AI，转写只是其中一类）。
//!
//! 纵切口径见 TDD-15 §2.4：`use super::*` 取共享项、命令标 `pub(crate)`、`mod.rs` 以
//! `use queue::*` 让它们回到 `collect_commands!` 作用域——`bindings.ts` 必须零 diff。

use super::*;

/// 当前处理队列快照（前端打开面板时拉取一次；后续增量由事件推送）。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_snapshot(
    state: tauri::State<'_, AppState>,
) -> Vec<crate::processing_queue::ProcTaskDto> {
    state.processing.snapshot()
}

/// 排队项上/下移一位（仅排队态可调序，运行中/终态不动，PRD-04-055）。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_reorder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    up: bool,
) {
    if let Ok(id) = id.parse::<u64>() {
        if state.processing.reorder(id, up) {
            emit_processing_queue(&app);
        }
    }
}

/// 移除一个排队项或未成功项（运行中 / 已完成不可移除，PRD-04-055）。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_remove(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) {
    if let Ok(id) = id.parse::<u64>() {
        let history_id = state.processing.history_id_of(id);
        if state.processing.remove(id) {
            if history_id.is_some() {
                super::processing::forget_queue_history(&app, history_id);
            }
            emit_processing_queue(&app);
        }
    }
}

/// 取消**运行中**的任务（PRD-05-010）：一路传播到 whisper 解码边界的 abort 轮询，
/// 不是「不再等待」——CPU 当即释放。排队项请用 `processing_queue_remove`。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_cancel(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) {
    if let Ok(id) = id.parse::<u64>() {
        if state.processing.request_cancel(id) {
            emit_processing_queue(&app);
        }
    }
}

/// 重跑**失败 / 已取消**的任务（PRD-05-010）：原地回到队尾排队，载荷不变。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_retry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) {
    if let Ok(id) = id.parse::<u64>() {
        if state.processing.retry(id) {
            emit_processing_queue(&app);
            start_processing_worker_if_idle(&app);
        }
    }
}

/// 清除队列中的**已完成**项（只清成功；失败 / 取消留待重试，PRD-04-055 三段修订）。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_clear_done(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) {
    state.processing.clear_done();
    emit_processing_queue(&app);
}

/// 清除队列中的**未成功**项（失败 / 取消）。二次确认在前端；这里只执行。
#[tauri::command]
#[specta::specta]
pub(crate) fn processing_queue_clear_unsuccessful(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) {
    state.processing.clear_unsuccessful();
    super::processing::forget_queue_history(&app, None);
    emit_processing_queue(&app);
}
