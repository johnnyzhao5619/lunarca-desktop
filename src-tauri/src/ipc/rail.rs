//! 活动栏布局（PRD-07-003 / ADR-087 ⑥⑦）：空间 id 的 IPC 枚举、归一化后的布局查询、显隐写入。
//!
//! 空间 id 的权威词表在 `luna_config::workspace_space`（ADR-087 ⑦）；本模块只把它投影成
//! typed IPC。前端**不解析** `workspace.rail_layout` 的 token：它拿到的是已归一化的
//! `Vec<RailLayoutItemDto>`（缺项补回、退役 id 丢弃、锚点强制显示，ADR-003 前端零业务）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event as _;

use luna_config::apps::unavailable_spaces;
use luna_config::workspace_space::{
    is_rail_anchor, rail_layout_tokens, reorder_rail_visible, resolve_rail_layout, RailEntry,
    RailReorderError, RAIL_SPACE_IDS,
};

use super::settings::{apply_setting_side_effects, set_setting_value, SettingValueDto};
use super::{AppState, SettingsChanged};

/// 一级空间的稳定 id（与 `luna_config::workspace_space::RAIL_SPACE_IDS` 穷尽映射；系统区三格
/// 不参与活动栏配置但同样是合法目的地）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSpaceDto {
    Search,
    Notes,
    Knowledge,
    QuickNotes,
    Recordings,
    Transcripts,
    Chat,
    /// 创作空间（ADR-162）：归一方应用「创作」，应用未启用时不在活动栏上（ADR-165 ③）。
    Creator,
    /// 「应用」目的地（ADR-165 ⑤）：系统区，贴底在回收站之上。
    Apps,
    Trash,
    Settings,
}

impl WorkspaceSpaceDto {
    /// 由词表的稳定 id 还原（未知 id → `None`）。
    ///
    /// **只有这一个方向是手写的**：反方向就是 serde 的 `snake_case` 序列化本身，那才是
    /// 真正的 wire 契约；round-trip 测试比对的是 serde 的输出，不是另一张手写表。
    #[must_use]
    pub fn from_str(id: &str) -> Option<Self> {
        match id {
            "search" => Some(Self::Search),
            "notes" => Some(Self::Notes),
            "knowledge" => Some(Self::Knowledge),
            "quick_notes" => Some(Self::QuickNotes),
            "recordings" => Some(Self::Recordings),
            "transcripts" => Some(Self::Transcripts),
            "chat" => Some(Self::Chat),
            "creator" => Some(Self::Creator),
            "apps" => Some(Self::Apps),
            "trash" => Some(Self::Trash),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    /// 它在活动栏配置里的 id；系统区三格不可配置 → `None`。
    #[must_use]
    fn rail_id(self) -> Option<&'static str> {
        let wire = serde_json::to_string(&self).ok()?;
        let id = wire.trim_matches('"');
        RAIL_SPACE_IDS.iter().copied().find(|known| *known == id)
    }
}

/// 活动栏上的一格（归一化后）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RailLayoutItemDto {
    pub space: WorkspaceSpaceDto,
    /// 用户是否让它显示（临时显示由前端按当前位置叠加，不写回设置）。
    pub visible: bool,
    /// 锚点：可排序、不可隐藏（PRD-07-003）。
    pub locked: bool,
}

const RAIL_LAYOUT_KEY: &str = "workspace.rail_layout";

fn current_entries(state: &AppState) -> Vec<RailEntry> {
    let tokens = state
        .settings_snapshot()
        .get(RAIL_LAYOUT_KEY)
        .and_then(|value| value.as_string_list().map(<[String]>::to_vec))
        .unwrap_or_default();
    resolve_rail_layout(&tokens)
}

/// 归一化布局 → DTO；所属应用未启用的空间整格不下发（不只是收起：命令面板、临时显示都没有它）。
fn items_of(entries: &[RailEntry], frozen: &[&str]) -> Vec<RailLayoutItemDto> {
    entries
        .iter()
        .filter(|entry| !frozen.contains(&entry.id))
        .filter_map(|entry| {
            // 词表与枚举的穷尽由 round-trip 测试锁住；这里的 `filter_map` 只是不为一个
            // 已被测试排除的情况写 `expect`。
            WorkspaceSpaceDto::from_str(entry.id).map(|space| RailLayoutItemDto {
                space,
                visible: entry.visible,
                locked: is_rail_anchor(entry.id),
            })
        })
        .collect()
}

/// 归一化后的活动栏布局（含隐藏项及其位置——前端据此在原位临时显示）。
#[tauri::command]
#[specta::specta]
pub(crate) fn workspace_rail_layout(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<RailLayoutItemDto>, String> {
    let frozen = unavailable_spaces(&state.settings_snapshot());
    Ok(items_of(&current_entries(&state), &frozen))
}

/// 活动栏右键「从侧栏隐藏 / 显示已隐藏的空间」的写入口：改一位开关、整份写回。
///
/// 走与设置页同一条 `set_setting_value` 通路（归一化 + 校验 + 落盘 + 广播），不另开
/// 增量协议——锚点被要求隐藏时归一化会把它重新置为显示，与设置页的锁定行为同源。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_workspace_space_visible(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    space: WorkspaceSpaceDto,
    visible: bool,
) -> Result<(), String> {
    let Some(id) = space.rail_id() else {
        return Err("SETTING_OPTION: system spaces are not configurable".to_owned());
    };
    let mut entries = current_entries(&state);
    if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
        entry.visible = visible;
    }
    persist_entries(&app, &state, &entries)
}

/// 活动栏拖拽排序（PRD-07-003 就地入口）：`expected` 是拖拽前看到的可见顺序，`ordered` 是
/// 拖出来的；一次 compare-and-set，期望过期即拒绝并让前端重读（不盲写、不逐次发请求）。
#[tauri::command]
#[specta::specta]
pub(crate) fn reorder_workspace_rail(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    expected: Vec<WorkspaceSpaceDto>,
    ordered: Vec<WorkspaceSpaceDto>,
) -> Result<(), String> {
    let ids = |spaces: &[WorkspaceSpaceDto]| -> Result<Vec<&'static str>, String> {
        spaces
            .iter()
            .map(|space| {
                space
                    .rail_id()
                    .ok_or_else(|| "SETTING_OPTION: system spaces are not configurable".to_owned())
            })
            .collect()
    };
    let expected = ids(&expected)?;
    let ordered = ids(&ordered)?;
    let entries = current_entries(&state);
    let frozen = unavailable_spaces(&state.settings_snapshot());
    let next =
        reorder_rail_visible(&entries, &frozen, &expected, &ordered).map_err(|err| match err {
            RailReorderError::Stale => "RAIL_ORDER_STALE: rail changed meanwhile".to_owned(),
            RailReorderError::NotAPermutation => {
                "RAIL_ORDER_INVALID: not a permutation of the visible spaces".to_owned()
            }
        })?;
    persist_entries(&app, &state, &next)
}

/// 布局写回的唯一出口：落设置 + 副作用 + 广播（显隐与排序都经这里）。
fn persist_entries(
    app: &tauri::AppHandle,
    state: &AppState,
    entries: &[RailEntry],
) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        RAIL_LAYOUT_KEY,
        SettingValueDto::StringList(rail_layout_tokens(entries)),
    )?;
    apply_setting_side_effects(app, state, RAIL_LAYOUT_KEY);
    let _ = SettingsChanged {
        key: RAIL_LAYOUT_KEY.to_owned(),
    }
    .emit(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_and_the_config_vocabulary_are_an_exhaustive_round_trip() {
        // 两份 id 清单是必然要漂的东西：设置归一化用 `luna-config` 那份，IPC 用这份。
        // 少一个，那个空间就永远进不了活动栏布局，而没有任何东西会变红。
        for id in RAIL_SPACE_IDS {
            let dto = WorkspaceSpaceDto::from_str(id)
                .unwrap_or_else(|| panic!("IPC 侧缺少空间 id：{id}"));
            let wire = serde_json::to_string(&dto).unwrap();
            assert_eq!(wire.trim_matches('"'), *id, "round-trip 不闭合：{id}");
            assert_eq!(dto.rail_id(), Some(*id));
        }
        assert_eq!(WorkspaceSpaceDto::from_str("materials"), None);
        // 系统区三格是合法目的地，但不参与活动栏配置。
        assert_eq!(WorkspaceSpaceDto::Apps.rail_id(), None);
        assert_eq!(WorkspaceSpaceDto::Trash.rail_id(), None);
        assert_eq!(WorkspaceSpaceDto::Settings.rail_id(), None);
    }

    #[test]
    fn items_carry_lock_state_from_the_vocabulary() {
        let items = items_of(&resolve_rail_layout(&[]), &[]);
        assert_eq!(items.len(), RAIL_SPACE_IDS.len());
        let locked: Vec<WorkspaceSpaceDto> = items
            .iter()
            .filter(|it| it.locked)
            .map(|it| it.space)
            .collect();
        assert_eq!(
            locked,
            [WorkspaceSpaceDto::Notes, WorkspaceSpaceDto::Knowledge]
        );
        assert!(items.iter().all(|it| it.visible));
    }

    #[test]
    fn a_space_of_a_disabled_app_is_not_on_the_rail_at_all() {
        // 默认设置：创作未启用 → 整格不下发，其余照旧（ADR-165 ③ ④）。
        let off = luna_config::Settings::default();
        let frozen = unavailable_spaces(&off);
        assert_eq!(frozen, ["creator"]);
        let items = items_of(&resolve_rail_layout(&[]), &frozen);
        assert!(items
            .iter()
            .all(|it| it.space != WorkspaceSpaceDto::Creator));
        assert_eq!(items.len(), RAIL_SPACE_IDS.len() - 1);

        let mut on = luna_config::Settings::default();
        for key in [
            luna_config::apps::APPS_INSTALLED_KEY,
            luna_config::apps::APPS_ENABLED_KEY,
        ] {
            on.set(
                key,
                luna_config::SettingValue::StringList(vec!["creator".to_owned()]),
            );
        }
        assert!(unavailable_spaces(&on).is_empty());
    }
}
