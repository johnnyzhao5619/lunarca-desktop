//! 语言标签（零 IO）。

use serde::{Deserialize, Serialize};
use std::fmt;

/// BCP-47 风格语言标签（如 `"zh"` / `"en"` / `"ja"`）。
///
/// 注意区分**界面语言**（i18n）与 **AI 处理目标语言**（翻译 target / 转写 lang），
/// 二者独立配置、勿混（TDD-11 §1.4）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lang(String);

impl Lang {
    /// 由任意可转为 `String` 的标签构造。
    #[must_use]
    pub fn new(tag: impl Into<String>) -> Self {
        Self(tag.into())
    }

    /// 底层标签切片。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 语言的**英文全称**（喂给 LLM 翻译 prompt 用）。
    ///
    /// 为何不直接用 BCP-47 代码：小参数量模型对 `into en` / `into zh` 这类语言代码指令
    /// 极不敏感，常退化为「照抄/改写源语言」；换成 `into English` / `into Chinese` 的全称
    /// 后指令遵从度显著提升（TDD-03 §3.3）。取主子标签（`-`/`_` 前）大小写不敏感匹配；
    /// 未收录的标签回退为其大写形式（不静默丢信息）。
    #[must_use]
    pub fn display_name(&self) -> &str {
        let primary = self
            .0
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "zh" => "Chinese",
            "en" => "English",
            "ja" => "Japanese",
            "ko" => "Korean",
            "fr" => "French",
            "de" => "German",
            "es" => "Spanish",
            "pt" => "Portuguese",
            "ru" => "Russian",
            "it" => "Italian",
            "ar" => "Arabic",
            "hi" => "Hindi",
            "th" => "Thai",
            "vi" => "Vietnamese",
            "id" => "Indonesian",
            "nl" => "Dutch",
            "pl" => "Polish",
            "tr" => "Turkish",
            // 未收录：回退原标签（保留可读信息，不静默丢失）。
            _ => self.0.as_str(),
        }
    }
}

impl From<&str> for Lang {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_maps_known_codes_case_and_subtag_insensitive() {
        assert_eq!(Lang::from("en").display_name(), "English");
        assert_eq!(Lang::from("ZH").display_name(), "Chinese");
        assert_eq!(Lang::from("zh-Hans").display_name(), "Chinese");
        assert_eq!(Lang::from("en_US").display_name(), "English");
        assert_eq!(Lang::from("ja").display_name(), "Japanese");
    }

    #[test]
    fn display_name_falls_back_to_tag_for_unknown() {
        assert_eq!(Lang::from("xx").display_name(), "xx");
    }
}
