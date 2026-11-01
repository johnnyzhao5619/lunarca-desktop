//! 用量与通知中心命令域（PRD-15，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

#[tauri::command]
#[specta::specta]
pub(crate) fn list_usage(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
) -> Vec<UsageRecordDto> {
    state
        .list_usage_records(UsageRange {
            limit: limit.map(|value| value as usize),
            ..Default::default()
        })
        .into_iter()
        .map(UsageRecordDto::from)
        .collect()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn usage_summary(state: tauri::State<'_, AppState>) -> UsageSummaryDto {
    // 「本地节省」估算在 Rust 侧算好（含价格解析），前端只渲染（ADR-003）。
    state.usage_summary_dto()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn clear_usage(state: tauri::State<'_, AppState>) -> u32 {
    state.clear_usage_records() as u32
}

/// 一页通知历史 + 页头要的两个总数。
///
/// **总数与列表同一次调用取齐**：页头此前直接渲染已加载条数，超过一页就在说谎
/// （表里 500 条、页头写「100 条」）；未读数同理——只数已加载的那一页，会与状态栏角标
/// 对不上。与速记流「徽标与列表出自同一快照」同一条理由。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NotificationPageDto {
    /// 这一页的条目（时间倒序）。
    pub items: Vec<NotificationHistoryDto>,
    /// 下一页的游标（`None` = 已经到底）。
    pub next_cursor: Option<NotificationCursorDto>,
    /// 全部条数（不只这一页）。
    pub total: u32,
    /// 全部未读数（不只这一页）。
    pub unread: u32,
}

/// 翻页游标 = 上一页最后一条的 `(created_at_ms, id)`。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NotificationCursorDto {
    /// 上一页最后一条的时刻（u64 按本仓约定走字符串）。
    pub created_at_ms: String,
    /// 上一页最后一条的 id（同毫秒的兄弟条目靠它定序）。
    pub id: String,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn notification_history(
    center: tauri::State<'_, NotificationCenter>,
    limit: Option<u32>,
    after: Option<NotificationCursorDto>,
) -> NotificationPageDto {
    let limit = limit.map_or(100, |value| value as usize).clamp(1, 500);
    let cursor = after.and_then(|cursor| {
        cursor
            .created_at_ms
            .parse::<i64>()
            .ok()
            .map(|created_at_ms| HistoryCursor {
                created_at_ms,
                id: cursor.id,
            })
    });
    let records = center.history(limit, cursor.as_ref());
    // 恰好取满一页才可能还有下一页；不足一页说明已经到底，不给游标（否则界面会一直
    // 显示「加载更多」，点下去永远是空的）。
    let next_cursor = (records.len() == limit)
        .then(|| records.last())
        .flatten()
        .map(|last| NotificationCursorDto {
            created_at_ms: last.created_at_ms.to_string(),
            id: last.id.clone(),
        });
    NotificationPageDto {
        items: records
            .into_iter()
            .map(NotificationHistoryDto::from)
            .collect(),
        next_cursor,
        total: center.total() as u32,
        unread: center.unread_count() as u32,
    }
}

/// 标为已读；`id=None` = 全部已读。返回**实际改动了几条**（界面据此扣减未读角标——
/// 直接减 1 会在"本来就已读"的条目上把角标扣穿）。
#[tauri::command]
#[specta::specta]
pub(crate) fn notification_mark_read(
    center: tauri::State<'_, NotificationCenter>,
    id: Option<String>,
) -> u32 {
    center.mark_read(id.as_deref()) as u32
}

#[tauri::command]
#[specta::specta]
pub(crate) fn notification_unread_count(center: tauri::State<'_, NotificationCenter>) -> u32 {
    center.unread_count() as u32
}

#[tauri::command]
#[specta::specta]
pub(crate) fn clear_notification_history(center: tauri::State<'_, NotificationCenter>) -> u32 {
    center.clear_history() as u32
}

/// 只清掉已读的那些（PRD-15-065 的非核爆出口）；返回清掉几条。
#[tauri::command]
#[specta::specta]
pub(crate) fn clear_read_notification_history(center: tauri::State<'_, NotificationCenter>) -> u32 {
    center.clear_read_history() as u32
}
