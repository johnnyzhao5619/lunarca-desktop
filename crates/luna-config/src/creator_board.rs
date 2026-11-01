//! 创作空间看板的列配置（ADR-162 / PRD-19-003）：七个阶段的词表、默认值与归一化。
//!
//! 住在 `luna-config`：设置注册表要用它声明 `creator.board_stages` 的选项与默认值，而
//! `luna-config` 不依赖 `luna-vault`。阶段的**语义**（frontmatter 写法、缺省归位）归
//! `luna_vault::ProjectStage`；两份 id 清单由 IPC 层的测试钉成同一份、同一顺序。

use crate::registry::{resolve_ordered_toggle_list, toggle_token, ToggleOption};

/// 七个阶段的 id——顺序即**默认列序**，与 `luna_vault::ProjectStage::ALL` 逐一相同。
pub const CREATOR_STAGE_IDS: &[&str] = &[
    "idea",
    "research",
    "writing",
    "recording",
    "editing",
    "publishing",
    "published",
];

/// 隐藏列的值语法前缀（与活动栏布局同一写法）。
pub const CREATOR_STAGE_HIDDEN_PREFIX: &str = "hidden:";

/// 默认：七列全部显示，默认顺序。
pub const CREATOR_BOARD_DEFAULT: &[&str] = CREATOR_STAGE_IDS;

/// 设置控件的选项：名字键直接用看板上的阶段名（`workspace:creator.stage.*`），不另起标签。
pub const CREATOR_STAGE_OPTIONS: &[ToggleOption] = &[
    ToggleOption {
        id: "idea",
        label_key: "workspace:creator.stage.idea",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "research",
        label_key: "workspace:creator.stage.research",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "writing",
        label_key: "workspace:creator.stage.writing",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "recording",
        label_key: "workspace:creator.stage.recording",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "editing",
        label_key: "workspace:creator.stage.editing",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "publishing",
        label_key: "workspace:creator.stage.publishing",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "published",
        label_key: "workspace:creator.stage.published",
        locked: false,
        default_visible: true,
    },
];

/// 看板的四种视图（设置 `creator.board_view` 的合法取值，顺序即切换控件的顺序）。日历按计划
/// 发布日期排（PRD-19-040）。
pub const CREATOR_BOARD_VIEWS: &[&str] = &["board", "list", "table", "calendar"];

/// 至少显示几列（看板不能一列都没有）。
pub const CREATOR_BOARD_MIN_VISIBLE: usize = 1;

/// 归一化后的列配置：全部七个阶段，按用户的顺序，各自显示与否。
///
/// 与设置写入时的归一化是同一个函数（[`resolve_ordered_toggle_list`]），读与写两边看到的
/// 永远是同一份：未知 id 丢弃、重复只留首次、缺项按默认位置补回、显示的不足一列时把最前面
/// 的那列改回显示。
#[must_use]
pub fn resolve_board_stages(tokens: &[String]) -> Vec<(&'static str, bool)> {
    resolve_ordered_toggle_list(
        CREATOR_STAGE_OPTIONS,
        CREATOR_STAGE_HIDDEN_PREFIX,
        CREATOR_BOARD_MIN_VISIBLE,
        tokens,
    )
}

/// 列配置 → 值 token（写回设置用）。
#[must_use]
pub fn board_stage_tokens(entries: &[(&str, bool)]) -> Vec<String> {
    entries
        .iter()
        .map(|(id, visible)| toggle_token(CREATOR_STAGE_HIDDEN_PREFIX, id, *visible))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn options_ids_and_default_are_the_same_list() {
        let ids: Vec<&str> = CREATOR_STAGE_OPTIONS.iter().map(|o| o.id).collect();
        assert_eq!(ids, CREATOR_STAGE_IDS);
        assert_eq!(CREATOR_BOARD_DEFAULT, CREATOR_STAGE_IDS);
        assert!(CREATOR_STAGE_OPTIONS
            .iter()
            .all(|o| !o.locked && o.default_visible));
    }

    #[test]
    fn the_default_is_all_seven_columns_in_order() {
        let entries = resolve_board_stages(&tokens(CREATOR_BOARD_DEFAULT));
        assert_eq!(
            entries.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            CREATOR_STAGE_IDS
        );
        assert!(entries.iter().all(|(_, visible)| *visible));
        assert_eq!(resolve_board_stages(&[]), entries);
    }

    #[test]
    fn hiding_every_column_keeps_the_first_one_in_the_users_order() {
        // 用户把「撰稿」排到最前后全部关掉：留下的是他排在最前的那一列，不是默认的第一列。
        let all_hidden: Vec<String> = [
            "writing",
            "idea",
            "research",
            "recording",
            "editing",
            "publishing",
            "published",
        ]
        .iter()
        .map(|id| format!("{CREATOR_STAGE_HIDDEN_PREFIX}{id}"))
        .collect();
        let entries = resolve_board_stages(&all_hidden);
        let visible: Vec<&str> = entries
            .iter()
            .filter(|(_, v)| *v)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(visible, ["writing"]);
    }

    #[test]
    fn tokens_round_trip_and_unknown_ids_drop() {
        let entries = resolve_board_stages(&tokens(&["hidden:recording", "bogus", "editing"]));
        assert_eq!(resolve_board_stages(&board_stage_tokens(&entries)), entries);
        assert!(entries.iter().any(|(id, v)| *id == "recording" && !v));
        assert_eq!(entries.len(), CREATOR_STAGE_IDS.len());
    }
}
