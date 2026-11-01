//! 内容的版本号：主题包与模板包共用的 `x.y.z`（ADR-169）。
//!
//! 同一个 id 的内容再次安装时按它比较：更高即更新，相同或更低不覆盖（PRD-18-024 假设）。

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("version must look like 1.2.3")]
pub struct VersionError;

/// `x.y.z`：只收三段非负整数，不收预发布与构建后缀。
///
/// > 假设：市场内容不发预发布版本（理由：灰度由目录的 `variants` 承担，ADR-050 修订 ④；版本号
/// > 只需要可比较、不可复用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(into = "String")]
pub struct PackVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl PackVersion {
    /// # Errors
    /// 不是三段非负整数时。
    pub fn parse(value: &str) -> Result<Self, VersionError> {
        let mut parts = value.split('.');
        let mut next = || -> Result<u64, VersionError> {
            let part = parts.next().ok_or(VersionError)?;
            if part.is_empty()
                || !part.chars().all(|c| c.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err(VersionError);
            }
            part.parse().map_err(|_| VersionError)
        };
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(VersionError);
        }
        Ok(version)
    }
}

impl std::fmt::Display for PackVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl From<PackVersion> for String {
    fn from(version: PackVersion) -> Self {
        version.to_string()
    }
}

impl<'de> Deserialize<'de> for PackVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_and_reject_anything_else() {
        let parse = |v: &str| PackVersion::parse(v).expect(v);
        assert!(parse("1.10.0") > parse("1.9.9"));
        assert!(parse("2.0.0") > parse("1.99.99"));
        assert_eq!(parse("0.1.0").to_string(), "0.1.0");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "1.2.3-beta",
            "-1.2.3",
            "1..3",
        ] {
            assert!(PackVersion::parse(bad).is_err(), "{bad}");
        }
    }
}
