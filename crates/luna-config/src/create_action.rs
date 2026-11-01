//! 创建动作目录（PRD-07-067/068/069 / TDD-05 §7.5a）：**唯一动作词表**。
//!
//! 「添加内容」是完整目录，启动概览「快速开始」是从同一目录选出的子集——两处消费同一份
//! 数据，故这份清单只能有一处。它是**纯数据**：没有图标、没有文案、没有 handler，那些各
//! 归其位（图标与 i18n 在前端，可用性与修复动作在 `ipc/create_actions.rs`）。
//!
//! **住在 `luna-config` 而不是 `src-tauri`**：设置注册表要用它声明「快速开始」的合法
//! 选项与分类约束，而 `luna-config` 不依赖 tauri。反过来把词表放在 IPC 层，设置校验就得
//! 抄一份 id 清单——那正是"两份清单迟早对不上"的经典形状。

/// 动作的产物类别（PRD-07-068：概览至少保留一个写作入口和一个资料入口）。
///
/// **约束的理由不是"分类好看"**：四个格子全填成资料入口的用户，第二天想随手写一句话
/// 会发现概览上没有任何入口——而「快速开始」的全部意义就是"我最常从哪里开始"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateActionCategory {
    /// 写作入口：产物是一篇（或一条）由用户敲出来的文字。
    Writing,
    /// 资料入口：产物是待转写 / 待整理的素材。
    Material,
}

/// 目录里的一条。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateAction {
    /// 稳定 id（设置值、IPC 枚举与前端映射共用；**不认别名、不认旧 id**）。
    pub id: &'static str,
    /// 产物类别。
    pub category: CreateActionCategory,
}

/// 五类创建动作——顺序即「添加内容」里的呈现顺序。
///
/// 顺序不是随意的：两个速记在最前（最高频、最低成本），新建笔记居中，两个资料入口在后。
pub const CREATE_ACTIONS: &[CreateAction] = &[
    CreateAction {
        id: "text_quick_note",
        category: CreateActionCategory::Writing,
    },
    CreateAction {
        id: "voice_quick_note",
        category: CreateActionCategory::Writing,
    },
    CreateAction {
        id: "new_note",
        category: CreateActionCategory::Writing,
    },
    CreateAction {
        id: "start_recording",
        category: CreateActionCategory::Material,
    },
    CreateAction {
        id: "import_material",
        category: CreateActionCategory::Material,
    },
];

/// 全部动作 id（设置控件的 `options`，与 [`CREATE_ACTIONS`] 同序）。
pub const CREATE_ACTION_IDS: &[&str] = &[
    "text_quick_note",
    "voice_quick_note",
    "new_note",
    "start_recording",
    "import_material",
];

/// 「快速开始」默认四项（TDD-05 §7.5a）。
///
/// 刻意**不含开始录音**：录音是一次分钟级的持续动作，把它放在开箱即见的第一屏，等于建议
/// 新用户"从录一段开始"——而导入资料同样是资料入口，代价却低得多。
pub const QUICK_START_DEFAULT: &[&str] = &[
    "text_quick_note",
    "voice_quick_note",
    "new_note",
    "import_material",
];

/// 按 id 查类别（未知 id → `None`，调用方据此拒绝）。
#[must_use]
pub fn create_action_category(id: &str) -> Option<CreateActionCategory> {
    CREATE_ACTIONS
        .iter()
        .find(|action| action.id == id)
        .map(|action| action.category)
}

/// 「添加内容」的排列（PRD-07-068）：用户在「快速开始」设置里排好的顺序在前，其余动作按目录
/// 顺序在后。两处消费同一份顺序，用户才不会在菜单与概览上看到两种先后。
///
/// 未知 id 丢弃、重复只留首次——设置值本身由注册表校验，这里只对齐两处顺序，不是第二个
/// 校验点；空值（从未配置）就是目录顺序。
#[must_use]
pub fn ordered_create_action_ids(preferred: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::with_capacity(CREATE_ACTION_IDS.len());
    for id in preferred {
        if let Some(known) = CREATE_ACTION_IDS
            .iter()
            .copied()
            .find(|known| *known == id.as_str())
        {
            if !out.contains(&known) {
                out.push(known);
            }
        }
    }
    for id in CREATE_ACTION_IDS {
        if !out.contains(id) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_list_and_the_catalog_are_the_same_thing_in_the_same_order() {
        // 两份清单并存是必然要漂的：控件的 `options` 取前者、分类约束查后者，
        // 顺序或成员一旦不一致，用户就会遇到"下拉里有、保存却被拒"。
        let from_catalog: Vec<&str> = CREATE_ACTIONS.iter().map(|a| a.id).collect();
        assert_eq!(from_catalog, CREATE_ACTION_IDS);
    }

    #[test]
    fn ids_are_unique_and_every_id_has_a_category() {
        let unique: std::collections::BTreeSet<&str> = CREATE_ACTION_IDS.iter().copied().collect();
        assert_eq!(unique.len(), CREATE_ACTION_IDS.len(), "id 重复");
        for id in CREATE_ACTION_IDS {
            assert!(create_action_category(id).is_some(), "{id} 没有类别");
        }
        assert_eq!(create_action_category("nope"), None);
    }

    #[test]
    fn both_categories_exist_so_the_at_least_one_of_each_rule_is_satisfiable() {
        // 约束要求默认值里两类都在场；若目录里某一类为空，这条约束就成了永远无法满足的
        // 死规则——而那只会在用户第一次改配置时才暴露。
        assert!(CREATE_ACTIONS
            .iter()
            .any(|a| a.category == CreateActionCategory::Writing));
        assert!(CREATE_ACTIONS
            .iter()
            .any(|a| a.category == CreateActionCategory::Material));
        assert!(QUICK_START_DEFAULT
            .iter()
            .any(|id| create_action_category(id) == Some(CreateActionCategory::Writing)));
        assert!(QUICK_START_DEFAULT
            .iter()
            .any(|id| create_action_category(id) == Some(CreateActionCategory::Material)));
    }

    #[test]
    fn the_add_content_order_follows_the_quick_start_setting_then_the_catalog() {
        let owned = |ids: &[&str]| ids.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // 默认四项在前（用户顺序），目录里剩下的那一个跟在后面。
        assert_eq!(
            ordered_create_action_ids(&owned(QUICK_START_DEFAULT)),
            [
                "text_quick_note",
                "voice_quick_note",
                "new_note",
                "import_material",
                "start_recording"
            ]
        );
        // 用户把导入排到最前：菜单跟着变。
        assert_eq!(
            ordered_create_action_ids(&owned(&["import_material", "new_note"])),
            [
                "import_material",
                "new_note",
                "text_quick_note",
                "voice_quick_note",
                "start_recording"
            ]
        );
        // 从未配置 = 目录顺序；未知 id 与重复不影响结果。
        assert_eq!(ordered_create_action_ids(&[]), CREATE_ACTION_IDS);
        assert_eq!(
            ordered_create_action_ids(&owned(&["nope", "new_note", "new_note"])),
            [
                "new_note",
                "text_quick_note",
                "voice_quick_note",
                "start_recording",
                "import_material"
            ]
        );
    }

    #[test]
    fn the_default_is_a_legal_value_of_the_setting_it_defaults() {
        assert!(QUICK_START_DEFAULT.len() >= 2 && QUICK_START_DEFAULT.len() <= 4);
        for id in QUICK_START_DEFAULT {
            assert!(CREATE_ACTION_IDS.contains(id), "默认值里有未知 id：{id}");
        }
        let unique: std::collections::BTreeSet<&&str> = QUICK_START_DEFAULT.iter().collect();
        assert_eq!(unique.len(), QUICK_START_DEFAULT.len(), "默认值里有重复");
    }
}
