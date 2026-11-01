//! 条目 id 与包内相对路径：两个商品域共用的两种写法。
//!
//! 二者都会被拼进别的东西里——id 进样式选择器（`[data-theme-family="<id>"]`）与内容层目录名，
//! 相对路径进解压落点与库内目录——所以字符集是**白名单**，不是转义：能出现的字符本身就不可能
//! 打断选择器或逃出目录。

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// 条目 id 的最大长度。
const MAX_ID_LEN: usize = 64;

/// 条目 id：小写字母、数字与 `.` `-` `_`，以字母或数字开头，1–64 个字符，不含 `..`。
///
/// 反向域名（`com.example.cozy`）与短名（`nebula`）都合法。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ContentId(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContentIdError {
    #[error("content id must be 1-{MAX_ID_LEN} characters")]
    Length,
    #[error("content id may only use a-z, 0-9, '.', '-' and '_', starting with a letter or digit")]
    Charset,
}

impl ContentId {
    /// 解析一个 id。
    ///
    /// # Errors
    /// 长度或字符集不合规时。
    pub fn parse(value: &str) -> Result<Self, ContentIdError> {
        if value.is_empty() || value.len() > MAX_ID_LEN {
            return Err(ContentIdError::Length);
        }
        let mut chars = value.chars();
        let starts_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
        if !starts_ok || !rest_ok || value.contains("..") {
            return Err(ContentIdError::Charset);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// 包内（或库内）的相对路径：`/` 分隔，不以 `/` 开头，没有空段、`.` 段或 `..` 段，不含反斜杠、
/// 冒号与控制字符，不进 `.luna/`（库的内部目录）。符号链接由解压那一层（`luna-content`）拒绝——
/// 这里只看字符串。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RelPath(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelPathError {
    #[error("path must be relative and non-empty")]
    NotRelative,
    #[error("path may not contain empty, '.' or '..' segments")]
    BadSegment,
    #[error("path may not contain backslashes, colons or control characters")]
    BadCharacter,
    #[error("path may not point into .luna/")]
    Internal,
}

/// 相对路径的最大长度（字节）。
const MAX_PATH_LEN: usize = 255;

impl RelPath {
    /// 解析一条相对路径。
    ///
    /// # Errors
    /// 见 [`RelPathError`]。
    pub fn parse(value: &str) -> Result<Self, RelPathError> {
        if value.is_empty() || value.starts_with('/') || value.len() > MAX_PATH_LEN {
            return Err(RelPathError::NotRelative);
        }
        if value
            .chars()
            .any(|c| c == '\\' || c == ':' || c.is_control())
        {
            return Err(RelPathError::BadCharacter);
        }
        if value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return Err(RelPathError::BadSegment);
        }
        if value == ".luna" || value.starts_with(".luna/") {
            return Err(RelPathError::Internal);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_a_whitelist_that_cannot_break_a_selector_or_a_directory() {
        for ok in ["luna", "nebula", "com.example.cozy-theme", "a_1", "9lives"] {
            assert!(ContentId::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Luna",
            "-luna",
            ".luna",
            "a..b",
            "a\"]{}",
            "a b",
            "a/b",
            "主题",
            &"x".repeat(65),
        ] {
            assert!(ContentId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn relative_paths_cannot_escape_or_reach_internal_folders() {
        for ok in [
            "templates/weekly.md",
            "Projects",
            "a/b/c.toml",
            "笔记/周报.md",
        ] {
            assert!(RelPath::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../up",
            "a/../b",
            "a//b",
            "./a",
            "a/",
            "C:/x",
            "a\\b",
            ".luna/settings.toml",
            ".luna",
            "a\u{0}b",
        ] {
            assert!(RelPath::parse(bad).is_err(), "{bad}");
        }
    }
}
