//! 导出设置注册表元数据为 JSON（供 CI 的 PRD-09 一致性检查消费，TDD-13）。
//!
//! 与生成 TS 绑定同纪律：注册表是唯一事实源，CI 据此校验它与双语文案、前端信息架构不漂移。
//!
//! 输出的是**注册表元数据**——key/group/scope/i18n_key/说明 key/控件契约（含选项集合与
//! 取值区间）/requires_restart/secret/internal/visibility/surface。控件契约随之导出，
//! CI 才能校验「每个选项都有本地化标签」（否则下拉里会出现裸取值）；可见性与「是否渲染为
//! 设置行」随之导出，CI 才能校验「某分组在标准投影下一行都不剩时必须登记为面板型分组」
//! （否则整组从设置页消失，而两侧编译与全部用例都不会红）。
//!
//! **这里没有任何用户的东西**：`min` / `max` 与 `options` 都是注册表里写死的**常量约束**
//! （「这一项合法取值是哪些、区间到哪里」），与用户此刻存了什么值无关；用户的取值
//! （`current_value`）不在导出面上，配置文件与钥匙串更不在。这一句要写清楚，否则加了
//! `min`/`max` 之后"仅输出元数据、不含任何用户值"读起来像自相矛盾。

use luna_config::{SettingControl, SETTINGS};
use serde::Serialize;

#[derive(Serialize)]
struct SettingMeta {
    key: &'static str,
    group: &'static str,
    scope: &'static str,
    i18n_key: &'static str,
    desc_i18n_key: String,
    /// 说明的呈现方式（`inline` / `disclosure` / `label_only`）。CI 据此校验「不常显的说明
    /// 照样双语在场」——它们仍是搜索语料（2026-10-08 D1）。
    desc_presentation: &'static str,
    control: &'static str,
    options: Vec<&'static str>,
    /// 有序开关列表的选项（id / 名字键 / 是否锁定）。名字键是 `ns:key`，CI 据此校验它在
    /// 两种语言的对应命名空间里真的存在——这种控件**不用** `option.<key>.<id>` 标签，
    /// 它复用各对象已有的那一个名字（PRD-07-003）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    toggle_options: Vec<ToggleOptionMeta>,
    /// 取值下界（`Number` 的区间下限 / `OrderedMultiSelect` 的最少项数）。
    ///
    /// 注册表常量，不是用户值——见文件头。`None` = 该控件没有这个约束。
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<i64>,
    /// 取值上界（同上）。
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<i64>,
    requires_restart: bool,
    internal: bool,
    /// 设置中心显示层级（`standard` / `advanced`）。
    visibility: &'static str,
    /// 组内分节 id（`PRD-09-070` ⑦；CI 据此校验节标题双语在场）。
    section: Option<&'static str>,
    /// 该键是否渲染为独立设置行（内部键 / 面板承载 / 全局投影控件均为 `false`）。
    /// `row` / `panel` / `internal`——见 `luna_config::SettingSurface`。
    surface: &'static str,
}

#[derive(Serialize)]
struct ToggleOptionMeta {
    id: &'static str,
    label_key: &'static str,
    locked: bool,
    default_visible: bool,
}

fn main() {
    let metas: Vec<SettingMeta> = SETTINGS
        .iter()
        .map(|spec| SettingMeta {
            key: spec.key,
            group: luna_config::setting_page(spec.key).as_str(),
            scope: spec.scope.as_str(),
            i18n_key: spec.i18n_key,
            desc_i18n_key: spec.desc_i18n_key(),
            desc_presentation: spec.desc_presentation().as_str(),
            control: spec.control.kind(),
            // `OrderedMultiSelect` 也是**枚举型**：它的 `options` 同样会以标签的形式
            // 出现在界面上（已选列表、替换下拉、增项下拉）。此前这一支落进 `_ => Vec::new()`，
            // 于是 CI 的选项 i18n 门禁（`check-settings-registry.py` 遍历的正是
            // `options`）对五个创建动作 id **空跑**——少一条标签，用户看到的是裸取值
            // `text_quick_note`，而没有任何东西会红。
            options: match spec.control {
                SettingControl::Select { options, .. }
                | SettingControl::OrderedMultiSelect { options, .. } => options.to_vec(),
                _ => Vec::new(),
            },
            toggle_options: match spec.control {
                SettingControl::OrderedToggleList { options, .. } => options
                    .iter()
                    .map(|o| ToggleOptionMeta {
                        id: o.id,
                        label_key: o.label_key,
                        locked: o.locked,
                        default_visible: o.default_visible,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            min: match spec.control {
                SettingControl::Number { min, .. } => Some(min),
                SettingControl::OrderedMultiSelect { min, .. } => Some(min as i64),
                // 有序开关列表的下限是「至少显示几项」（看板至少一列，ADR-162）。
                SettingControl::OrderedToggleList { min_visible, .. } => Some(min_visible as i64),
                _ => None,
            },
            max: match spec.control {
                SettingControl::Number { max, .. } => Some(max),
                SettingControl::OrderedMultiSelect { max, .. } => Some(max as i64),
                _ => None,
            },
            requires_restart: spec.requires_restart,
            internal: luna_config::registry::is_internal(spec.key),
            visibility: spec.visibility().as_str(),
            section: luna_config::setting_page_section(spec),
            surface: match luna_config::setting_surface(spec.key) {
                luna_config::SettingSurface::Row => "row",
                luna_config::SettingSurface::Panel => "panel",
                luna_config::SettingSurface::Internal => "internal",
            },
        })
        .collect();
    let json = serde_json::to_string_pretty(&metas).expect("序列化设置注册表失败");
    println!("{json}");
}
