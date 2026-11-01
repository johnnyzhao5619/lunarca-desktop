//! 工作区一级空间词表与活动栏布局值规则（ADR-087 ⑥⑦ / PRD-07-002 / PRD-07-003 / TDD-05 §7.5a）。
//!
//! 住在 `luna-config` 而不是 `src-tauri`：设置注册表要用它声明 `workspace.rail_layout` 的合法
//! 选项、锁定项与默认值，而 `luna-config` 不依赖 tauri。IPC 层的 `WorkspaceSpaceDto` 与这份
//! 词表做穷尽映射并以 round-trip 测试锁住，不另抄一份 id 清单。
//!
//! **纯数据 + 纯函数**：没有图标、没有渲染。名字键作为选项元数据随注册表下发（与每个设置
//! 项自己的 `i18n_key` 同一做法），图标与侧栏渲染归前端各空间模块。

use crate::registry::{resolve_ordered_toggle_list, toggle_token, ToggleOption};

/// 活动栏上可配置的内容空间——顺序即**默认顺序**（PRD-07-002）。
///
/// 系统区（应用 / 回收站 / 设置）固定贴底、不参与配置，故不在此表。创作空间（ADR-162）排在
/// 最后；它归一方应用「创作」，应用未启用时整格不出现（ADR-165 ③，判定在 [`crate::apps`]）。
pub const RAIL_SPACE_IDS: &[&str] = &[
    "search",
    "notes",
    "knowledge",
    "quick_notes",
    "recordings",
    "transcripts",
    "chat",
    "creator",
];

/// 隐藏项的值语法前缀：`hidden:<space id>`（TDD-05 §7.5a）。
pub const RAIL_HIDDEN_PREFIX: &str = "hidden:";

/// 默认布局：默认顺序；创作空间默认隐藏（ADR-162 ①），其余全部显示。
///
/// 与 [`RAIL_SPACE_OPTIONS`] 的 `default_visible` 是同一个事实的两种写法，由
/// `the_default_layout_matches_each_options_default_visibility` 钉成一致。
pub const RAIL_LAYOUT_DEFAULT: &[&str] = &[
    "search",
    "notes",
    "knowledge",
    "quick_notes",
    "recordings",
    "transcripts",
    "chat",
    "creator",
];

/// 设置控件的选项元数据：id、名字键、是否锁定（锚点，PRD-07-003）、默认是否显示。
///
/// 名字键指向各空间**已有的**那一个名字（`workspace:nav.*` / `workspace:search.title` /
/// `chat:title`），不为设置页另起一套 `option.*` 标签——每个目的地只有一个名字键。
pub const RAIL_SPACE_OPTIONS: &[ToggleOption] = &[
    ToggleOption {
        id: "search",
        label_key: "workspace:search.title",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "notes",
        label_key: "workspace:nav.notes",
        locked: true,
        default_visible: true,
    },
    ToggleOption {
        id: "knowledge",
        label_key: "workspace:nav.knowledge",
        locked: true,
        default_visible: true,
    },
    ToggleOption {
        id: "quick_notes",
        label_key: "workspace:nav.quickNotes",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "recordings",
        label_key: "workspace:nav.recording",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "transcripts",
        label_key: "workspace:nav.transcripts",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "chat",
        label_key: "chat:title",
        locked: false,
        default_visible: true,
    },
    ToggleOption {
        id: "creator",
        label_key: "workspace:nav.creator",
        locked: false,
        // 「默认看不到创作」由应用默认未启用承担（ADR-165 ③）；启用后它像别的空间一样显示。
        default_visible: true,
    },
];

/// 锚点：可排序、不可隐藏（PRD-07-003）。判据只有一处——选项元数据的 `locked`。
#[must_use]
pub fn is_rail_anchor(id: &str) -> bool {
    RAIL_SPACE_OPTIONS
        .iter()
        .any(|option| option.id == id && option.locked)
}

/// 归一化后的布局中的一项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailEntry {
    /// 空间 id（必在 [`RAIL_SPACE_IDS`] 内）。
    pub id: &'static str,
    /// 是否显示在活动栏上。
    pub visible: bool,
}

/// 归一化（ADR-087 ⑥「读取时与注册表对齐」）：未知 id 丢弃、重复只留首次、锁定项强制显示、
/// 缺失的空间按默认顺序补回、显隐取该空间的默认可见性。
///
/// 这是设置归一化，不是兼容层（ADR-046）：它对任何输入都给出同一个封闭定义域内的答案，
/// 而不是为某个旧格式留一条分支。
#[must_use]
pub fn resolve_rail_layout(tokens: &[String]) -> Vec<RailEntry> {
    resolve_ordered_toggle_list(RAIL_SPACE_OPTIONS, RAIL_HIDDEN_PREFIX, 0, tokens)
        .into_iter()
        .map(|(id, visible)| RailEntry { id, visible })
        .collect()
}

/// 布局 → 值 token（写回设置用）。
/// 活动栏拖拽排序被拒的原因（PRD-07-003 就地入口）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RailReorderError {
    /// 调用方看到的可见顺序与当前落盘的不一致（设置页 / 另一处刚改过）：不盲写，让调用方重读。
    Stale,
    /// `ordered` 不是当前可见集合的一个排列（多了、少了、重复了）。
    NotAPermutation,
}

/// 按用户拖出的**可见顺序**重排整条布局（纯函数，一次 compare-and-set）。
///
/// - `expected` = 调用方拖拽前看到的可见顺序；与当前不等即 [`RailReorderError::Stale`]——
///   排位不是逐次拖拽发请求（ADR-097 ④ 同一精神）。
/// - 收起的空间**原位不动**：它们占着自己的槽位，可见项只在可见槽位之间换位；这样再显示出来时
///   仍在用户当初收起它的位置附近，而不是被挤到末尾。
/// - `frozen` = 此刻不在活动栏上的空间（所属一方应用未启用，ADR-165 ③）：调用方看不到它们，
///   它们与收起的空间一样原位不动，用户的显隐偏好也不被改写。
pub fn reorder_rail_visible(
    entries: &[RailEntry],
    frozen: &[&str],
    expected: &[&str],
    ordered: &[&str],
) -> Result<Vec<RailEntry>, RailReorderError> {
    let movable = |entry: &RailEntry| entry.visible && !frozen.contains(&entry.id);
    let current: Vec<&str> = entries
        .iter()
        .filter(|e| movable(e))
        .map(|e| e.id)
        .collect();
    if current != expected {
        return Err(RailReorderError::Stale);
    }
    if ordered.len() != current.len()
        || !current.iter().all(|id| ordered.contains(id))
        || (1..ordered.len()).any(|i| ordered[..i].contains(&ordered[i]))
    {
        return Err(RailReorderError::NotAPermutation);
    }
    let mut next_visible = ordered.iter();
    Ok(entries
        .iter()
        .map(|entry| {
            if movable(entry) {
                let id = next_visible.next().expect("可见槽位数与 ordered 等长");
                RailEntry {
                    id: RAIL_SPACE_IDS
                        .iter()
                        .copied()
                        .find(|known| known == id)
                        .expect("ordered 已验证为当前可见集合的排列"),
                    visible: true,
                }
            } else {
                *entry
            }
        })
        .collect())
}

/// 设置页写回「活动栏布局」时，把调用方看不到的空间（`frozen`：所属一方应用未启用，
/// ADR-165 ③）放回它原来的槽位、保持原来的显隐——设置页压根没画出它，写回的列表里自然没有它；
/// 不放回的话，归一化会把它补到默认位置，用户当初给它排的位置与收起状态就丢了。
#[must_use]
pub fn keep_frozen_slots(previous: &[String], next: &[String], frozen: &[&str]) -> Vec<String> {
    let before = resolve_rail_layout(previous);
    let mut merged: Vec<RailEntry> = resolve_rail_layout(next)
        .into_iter()
        .filter(|entry| !frozen.contains(&entry.id))
        .collect();
    for (index, entry) in before.iter().enumerate() {
        if frozen.contains(&entry.id) {
            merged.insert(index.min(merged.len()), *entry);
        }
    }
    rail_layout_tokens(&merged)
}

#[must_use]
pub fn rail_layout_tokens(entries: &[RailEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| toggle_token(RAIL_HIDDEN_PREFIX, entry.id, entry.visible))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reorder_keeps_hidden_slots_in_place_and_is_a_compare_and_set() {
        let entries = resolve_rail_layout(&tokens(&[
            "search",
            "notes",
            "knowledge",
            "hidden:quick_notes",
            "recordings",
            "transcripts",
            "chat",
            "creator",
        ]));
        // 创作所属的应用未启用：它不在可见顺序里，槽位原地冻结（ADR-165 ③）。
        let frozen = ["creator"];
        let visible = [
            "search",
            "notes",
            "knowledge",
            "recordings",
            "transcripts",
            "chat",
        ];
        // 把「录音」拖到最前：收起的「速记」仍在第 4 槽。
        let next = reorder_rail_visible(
            &entries,
            &frozen,
            &visible,
            &[
                "recordings",
                "search",
                "notes",
                "knowledge",
                "transcripts",
                "chat",
            ],
        )
        .unwrap();
        assert_eq!(
            rail_layout_tokens(&next),
            tokens(&[
                "recordings",
                "search",
                "notes",
                "hidden:quick_notes",
                "knowledge",
                "transcripts",
                "chat",
                "creator",
            ])
        );
        // 期望顺序过期 → 拒绝，让调用方重读。
        assert_eq!(
            reorder_rail_visible(&entries, &frozen, &["notes", "search"], &visible),
            Err(RailReorderError::Stale)
        );
        // 不冻结时创作是可见项，同一个期望就过期了。
        assert_eq!(
            reorder_rail_visible(&entries, &[], &visible, &visible),
            Err(RailReorderError::Stale)
        );
        // 不是排列（少一项 / 重复）→ 拒绝。
        assert_eq!(
            reorder_rail_visible(
                &entries,
                &frozen,
                &visible,
                &[
                    "search",
                    "search",
                    "notes",
                    "knowledge",
                    "recordings",
                    "transcripts"
                ]
            ),
            Err(RailReorderError::NotAPermutation)
        );
    }

    fn ids(entries: &[RailEntry]) -> Vec<&'static str> {
        entries.iter().map(|e| e.id).collect()
    }

    #[test]
    fn options_and_ids_are_the_same_list_in_the_same_order() {
        // 两份清单必然要漂：控件选项用 options，IPC round-trip 用 ids。少一个，那个空间
        // 就永远进不了设置页或永远到不了前端。
        let from_options: Vec<&str> = RAIL_SPACE_OPTIONS.iter().map(|o| o.id).collect();
        assert_eq!(from_options, RAIL_SPACE_IDS);
    }

    #[test]
    fn the_default_layout_matches_each_options_default_visibility() {
        // 默认布局常量与选项的 `default_visible` 是同一个事实：两边不一致，新装用户与
        // 「布局里缺了这一项」的老用户就会看到不同的活动栏。
        let expected: Vec<String> = RAIL_SPACE_OPTIONS
            .iter()
            .map(|o| toggle_token(RAIL_HIDDEN_PREFIX, o.id, o.default_visible))
            .collect();
        assert_eq!(tokens(RAIL_LAYOUT_DEFAULT), expected);
        assert!(
            RAIL_SPACE_OPTIONS.iter().all(|o| o.default_visible),
            "没有默认隐藏的空间：创作默认看不到由应用默认未启用承担（ADR-165 ③）"
        );
    }

    #[test]
    fn anchors_are_exactly_notes_and_knowledge() {
        let locked: Vec<&str> = RAIL_SPACE_OPTIONS
            .iter()
            .filter(|o| o.locked)
            .map(|o| o.id)
            .collect();
        assert_eq!(locked, ["notes", "knowledge"]);
        assert!(is_rail_anchor("notes"));
        assert!(!is_rail_anchor("search"));
        assert!(!is_rail_anchor("nope"));
    }

    #[test]
    fn the_default_resolves_in_default_order_all_shown() {
        let entries = resolve_rail_layout(&tokens(RAIL_LAYOUT_DEFAULT));
        assert_eq!(ids(&entries), RAIL_SPACE_IDS);
        assert!(entries.iter().all(|e| e.visible));
        // 空值（从未配置）同样得到默认。
        assert_eq!(resolve_rail_layout(&[]), entries);
    }

    #[test]
    fn hidden_tokens_keep_their_position_and_anchors_cannot_hide() {
        let entries = resolve_rail_layout(&tokens(&[
            "hidden:search",
            "notes",
            "hidden:knowledge",
            "quick_notes",
            "recordings",
            "transcripts",
            "chat",
            "creator",
        ]));
        assert_eq!(ids(&entries), RAIL_SPACE_IDS);
        assert!(!entries[0].visible, "search 按 token 隐藏");
        assert!(entries[7].visible, "用户显式显示过的创作空间照旧显示");
        assert!(entries[2].visible, "锚点 knowledge 强制显示");
    }

    #[test]
    fn unknown_and_duplicate_ids_are_dropped_and_missing_ones_return_to_their_default_slot() {
        // 用户把 chat 拖到最前、search 第二，其余丢了（比如来自更早版本的文件）。
        let entries = resolve_rail_layout(&tokens(&["chat", "search", "bogus", "chat"]));
        assert_eq!(
            ids(&entries),
            [
                "chat",
                // 创作缺失：补在它的默认前驱（chat）之后——chat 被拖到了最前，它就跟到第二格。
                "creator",
                "search",
                "notes",
                "knowledge",
                "quick_notes",
                "recordings",
                "transcripts"
            ]
        );
        // 缺失的项按各自的默认可见性补回（现在全是显示）。
        assert!(entries.iter().all(|e| e.visible));
    }

    #[test]
    fn a_frozen_space_keeps_its_slot_and_visibility_when_the_settings_page_writes_back() {
        // 创作被用户放在第二格并收起；应用停用期间设置页里看不到它，用户把「对话」挪到最前。
        let previous = tokens(&[
            "search",
            "hidden:creator",
            "notes",
            "knowledge",
            "quick_notes",
            "recordings",
            "transcripts",
            "chat",
        ]);
        let next = tokens(&[
            "chat",
            "search",
            "notes",
            "knowledge",
            "quick_notes",
            "recordings",
            "transcripts",
        ]);
        assert_eq!(
            keep_frozen_slots(&previous, &next, &["creator"]),
            tokens(&[
                "chat",
                "hidden:creator",
                "search",
                "notes",
                "knowledge",
                "quick_notes",
                "recordings",
                "transcripts",
            ])
        );
        // 没有冻结项时就是普通的归一化。
        assert_eq!(
            keep_frozen_slots(&previous, &next, &[]),
            rail_layout_tokens(&resolve_rail_layout(&next))
        );
    }

    #[test]
    fn tokens_round_trip() {
        let entries = resolve_rail_layout(&tokens(&["hidden:transcripts", "notes"]));
        let back = rail_layout_tokens(&entries);
        assert!(back.contains(&"hidden:transcripts".to_owned()));
        assert_eq!(resolve_rail_layout(&back), entries);
    }
}
