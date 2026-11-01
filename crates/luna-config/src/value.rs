//! 设置值（弱类型，TOML 友好）。

use serde::{Deserialize, Serialize};

/// 一个设置的取值。`untagged` 以便在 TOML 里自然表现为 bool / int / string。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    /// 布尔。
    Bool(bool),
    /// 整数。
    Int(i64),
    /// 字符串。
    Str(String),
    /// 有序字符串列表（TOML 原生数组，TDD-05 §7.5a）。
    ///
    /// **不编码成逗号串**：那样 TOML 里躺着的是一个人读不懂、工具也校验不了的私有
    /// 格式，而"顺序"和"有没有重复"这两件本设置的全部意义都要靠字符串切分来回答。
    /// 顺序有意义（快速开始快捷项的排列即屏幕顺序），故它是 `Vec` 而不是集合。
    StringList(Vec<String>),
}

impl SettingValue {
    /// 稳定类型名（供校验 / 错误信息使用）。
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Str(_) => "str",
            Self::StringList(_) => "string_list",
        }
    }

    /// 是否与另一个设置值属于同一基础类型。
    #[must_use]
    pub const fn same_type_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Bool(_), Self::Bool(_))
                | (Self::Int(_), Self::Int(_))
                | (Self::Str(_), Self::Str(_))
                | (Self::StringList(_), Self::StringList(_))
        )
    }

    /// 设置 IPC 当前以 TS `number` 表达整数，持久化层只接受可安全投影的 i32 范围。
    #[must_use]
    pub const fn fits_i32(&self) -> bool {
        match self {
            Self::Int(value) => *value >= i32::MIN as i64 && *value <= i32::MAX as i64,
            Self::Bool(_) | Self::Str(_) | Self::StringList(_) => true,
        }
    }

    /// 取布尔。
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// 取整数。
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
    /// 取字符串切片。
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
    /// 取有序字符串列表。
    #[must_use]
    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            Self::StringList(items) => Some(items),
            _ => None,
        }
    }
}

impl From<Vec<String>> for SettingValue {
    fn from(v: Vec<String>) -> Self {
        Self::StringList(v)
    }
}

impl From<bool> for SettingValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<i64> for SettingValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}
impl From<&str> for SettingValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}
impl From<String> for SettingValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors() {
        assert_eq!(SettingValue::Bool(true).as_bool(), Some(true));
        assert_eq!(SettingValue::Int(7).as_int(), Some(7));
        assert_eq!(SettingValue::from("zh").as_str(), Some("zh"));
        assert_eq!(SettingValue::Bool(true).as_int(), None);
        assert_eq!(SettingValue::Bool(true).kind(), "bool");
        assert!(SettingValue::Str("a".into()).same_type_as(&SettingValue::Str("b".into())));
        assert!(!SettingValue::Int(1).same_type_as(&SettingValue::Bool(true)));
        assert!(SettingValue::Int(i64::from(i32::MAX)).fits_i32());
        assert!(!SettingValue::Int(i64::from(i32::MAX) + 1).fits_i32());
        let list = SettingValue::from(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(list.kind(), "string_list");
        assert_eq!(list.as_string_list().map(<[String]>::len), Some(2));
        assert_eq!(
            list.as_str(),
            None,
            "列表不是字符串，别让它悄悄当成字符串用"
        );
        assert!(list.same_type_as(&SettingValue::StringList(Vec::new())));
        assert!(!list.same_type_as(&SettingValue::Str("a".into())));
    }

    /// TOML 里必须是**原生数组**：`untagged` 让它自然落成 `key = ["a", "b"]`，
    /// 顺序与条目都是人读得懂、工具也校验得了的。
    #[test]
    fn a_string_list_round_trips_through_toml_as_a_native_array() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Row {
            v: SettingValue,
        }
        let row = Row {
            v: SettingValue::StringList(vec!["text_quick_note".into(), "new_note".into()]),
        };
        let text = toml::to_string(&row).unwrap();
        assert!(
            text.contains(r#"v = ["text_quick_note", "new_note"]"#),
            "{text}"
        );
        assert_eq!(toml::from_str::<Row>(&text).unwrap(), row);
    }
}
