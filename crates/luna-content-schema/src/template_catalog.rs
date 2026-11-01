//! 模板目录的条目（ADR-159 ③ ⑥、ADR-161 ③，纵切 7b）：签名目录 `templates` 里的一条 = 一个模板包的摘要、
//! 由内容推导出的能力、包体的 sha256 与大小、要求的最低客户端版本。
//!
//! 与主题不同，模板包**不内联**：包体（zip）另存，按 sha256 取（内容寻址，下载位置由协议约定，见
//! `luna-cloud` 的 `CatalogKind::Templates`）。目录签名覆盖 sha256，于是包体完整性跟着签名走。
//!
//! 条目由 [`TemplateCatalogEntry::derive`] 从包体算出——服务端收到上传时算一遍（存进草稿），冻结 / 签名前
//! 对存着的包体再算一遍与草稿比对；客户端下载后再算一遍与目录比对（[`TemplateCatalogEntry::same_package`]）。
//! 三处同一个函数，所以审核时看到的能力就是安装时的能力。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::revocation::revocation_problems;
use crate::template_archive::{sha256_hex, unpack_template_pack, PackArchiveError};
use crate::{
    Capabilities, ContentApiRange, ContentId, PackAssetKind, PackVersion, Revocation,
    MAX_PACK_BYTES,
};

/// 包里每一类内容各有几项（「获取更多模板」的卡片据此写「3 个笔记模板 · 1 个配方」）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackContentCounts {
    pub note_templates: u32,
    pub ai_presets: u32,
    pub distill_templates: u32,
    pub prompts: u32,
    pub recipes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateCatalogEntry {
    pub id: ContentId,
    pub version: PackVersion,
    pub name: String,
    pub publisher: String,
    /// SPDX 表达式（清单原样）。
    pub license: String,
    #[serde(default)]
    pub locales: Vec<String>,
    /// 包要求的内容接口区间（清单原样，如 `">=1, <2"`）；宿主不在区间里时只显示「需要更新 Lunarca」。
    pub content_api: String,
    pub contents: PackContentCounts,
    /// 由内容推导，不由作者声明（ADR-161 ③）。
    pub capabilities: Capabilities,
    /// 包体（zip）的小写十六进制 sha256。
    pub sha256: String,
    /// 包体字节数。
    pub bytes: u64,
    /// 低于它的客户端只显示「需要更新 Lunarca」，不给安装（PRD-18-007）。由运营在控制台定，不来自包。
    pub min_client_version: PackVersion,
}

impl TemplateCatalogEntry {
    /// 从包体算出条目（**纯函数**）：解包核对、解析配方、推导能力、数内容、算包体 sha256。
    ///
    /// # Errors
    /// 包不合格时（见 [`PackArchiveError`]）。
    pub fn derive(
        package: &[u8],
        min_client_version: PackVersion,
    ) -> Result<Self, PackArchiveError> {
        let pack = unpack_template_pack(package)?;
        let capabilities = pack.capabilities()?;
        let manifest = &pack.manifest;
        let count = |kind| u32::try_from(manifest.assets_of(kind).count()).unwrap_or(u32::MAX);
        Ok(Self {
            id: manifest.id.clone(),
            version: manifest.version,
            name: manifest.name.clone(),
            publisher: manifest.publisher.clone(),
            license: manifest.license.clone(),
            locales: manifest.locales.clone(),
            content_api: format!(
                ">={}, <{}",
                manifest.content_api.min, manifest.content_api.max_exclusive
            ),
            contents: PackContentCounts {
                note_templates: count(PackAssetKind::NoteTemplate),
                ai_presets: count(PackAssetKind::AiPreset),
                distill_templates: count(PackAssetKind::DistillTemplate),
                prompts: count(PackAssetKind::Prompt),
                recipes: count(PackAssetKind::Recipe),
            },
            capabilities,
            sha256: sha256_hex(package),
            bytes: package.len() as u64,
            min_client_version,
        })
    }

    /// 两条说的是不是同一个包、同样的能力（比对除 `min_client_version` 以外的全部字段——那一格是运营定的）。
    /// 服务端冻结前拿存着的包体重算一遍与草稿比，客户端下载后重算一遍与目录比，对不上即拒（ADR-161 ③）。
    #[must_use]
    pub fn same_package(&self, other: &Self) -> bool {
        Self {
            min_client_version: other.min_client_version,
            ..self.clone()
        } == *other
    }

    /// 这台客户端装得了它吗：客户端版本够、宿主的内容接口在包要求的区间里。
    #[must_use]
    pub fn supports(&self, client_version: PackVersion, host_content_api: u32) -> bool {
        client_version >= self.min_client_version
            && ContentApiRange::parse(&self.content_api)
                .is_ok_and(|range| range.accepts(host_content_api))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 单条的问题（不含条目序号）：形状上的——包体本身对不对，由服务端拿存着的包体经
/// [`TemplateCatalogEntry::derive`] + [`TemplateCatalogEntry::same_package`] 判。
///
/// # Errors
/// sha256 写法不对、大小越界、`content_api` 写法不对、名字等为空时。
pub fn validate_template_entry(entry: &TemplateCatalogEntry) -> Result<(), String> {
    if !is_sha256(&entry.sha256) {
        return Err("sha256 must be 64 lowercase hex characters".to_owned());
    }
    if entry.bytes == 0 || entry.bytes > MAX_PACK_BYTES {
        return Err(format!("bytes must be 1-{MAX_PACK_BYTES}"));
    }
    ContentApiRange::parse(&entry.content_api).map_err(|error| error.to_string())?;
    for (field, value) in [
        ("name", &entry.name),
        ("publisher", &entry.publisher),
        ("license", &entry.license),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{field} must not be empty"));
        }
    }
    Ok(())
}

/// 整份目录的问题清单（空 = 合格）：逐条 [`validate_template_entry`]，外加 id 在目录里唯一。
#[must_use]
pub fn validate_template_catalog(entries: &[TemplateCatalogEntry]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Err(problem) = validate_template_entry(entry) {
            problems.push(format!("entry #{index}: {problem}"));
        }
        if !seen.insert(entry.id.as_str()) {
            problems.push(format!(
                "entry #{index}: template pack id {} appears more than once",
                entry.id.as_str()
            ));
        }
    }
    problems
}

/// 模板目录一个版本的内容：可获取的条目 + 封禁列表（[`Revocation`]，被封的已装包从所有选择面消失）。
/// 签进同一份目录信封；没有封禁时 `revoked` 可以不写。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateRelease {
    pub entries: Vec<TemplateCatalogEntry>,
    #[serde(default)]
    pub revoked: Vec<Revocation>,
}

/// 整个版本的问题清单（空 = 合格）：条目按 [`validate_template_catalog`]，封禁按两类目录共用的那一套。
#[must_use]
pub fn validate_template_release(release: &TemplateRelease) -> Vec<String> {
    let mut problems = validate_template_catalog(&release.entries);
    let listed: HashSet<&str> = release
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    problems.extend(revocation_problems(
        &listed,
        &release.revoked,
        "template pack",
    ));
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_archive::tests::good_pack;

    fn v(text: &str) -> PackVersion {
        PackVersion::parse(text).unwrap()
    }

    fn entry() -> TemplateCatalogEntry {
        TemplateCatalogEntry::derive(&good_pack(), v("1.1.0")).unwrap()
    }

    #[test]
    fn an_entry_is_derived_from_the_package_bytes() {
        let package = good_pack();
        let entry = TemplateCatalogEntry::derive(&package, v("1.1.0")).unwrap();
        assert_eq!(entry.id.as_str(), "com.example.studio");
        assert_eq!(entry.version, v("1.2.0"));
        assert_eq!(entry.content_api, ">=1, <2");
        assert_eq!(
            entry.contents,
            PackContentCounts {
                note_templates: 1,
                prompts: 1,
                recipes: 1,
                ..PackContentCounts::default()
            }
        );
        assert!(entry.capabilities.uses_ai);
        assert_eq!(entry.sha256, sha256_hex(&package));
        assert_eq!(entry.bytes, package.len() as u64);
        assert!(validate_template_catalog(&[entry]).is_empty());
    }

    #[test]
    fn same_package_ignores_only_the_operator_set_minimum_version() {
        let a = entry();
        let mut b = a.clone();
        b.min_client_version = v("9.0.0");
        assert!(a.same_package(&b));
        // 草稿里把能力改小——比对不过。
        b.capabilities.uses_ai = false;
        assert!(!a.same_package(&b));
        let mut c = a.clone();
        c.sha256 = "0".repeat(64);
        assert!(!a.same_package(&c));
    }

    #[test]
    fn hosts_too_old_or_outside_the_content_api_cannot_install() {
        let entry = entry();
        assert!(entry.supports(v("1.1.0"), 1));
        assert!(!entry.supports(v("1.0.9"), 1));
        assert!(!entry.supports(v("1.1.0"), 2));
    }

    #[test]
    fn malformed_entries_duplicates_and_bans_are_named() {
        let mut bad = entry();
        bad.id = ContentId::parse("com.example.bad").unwrap();
        bad.sha256 = "ABC".to_owned();
        let mut empty = entry();
        empty.id = ContentId::parse("com.example.empty").unwrap();
        empty.bytes = 0;
        let release = TemplateRelease {
            entries: vec![entry(), entry(), bad, empty],
            revoked: vec![Revocation {
                id: ContentId::parse("com.example.studio").unwrap(),
                reason: "copied someone else's pack".to_owned(),
            }],
        };
        let problems = validate_template_release(&release);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].starts_with("entry #1:") && problems[0].contains("more than once"));
        assert!(problems[1].starts_with("entry #2:") && problems[1].contains("sha256"));
        assert!(problems[2].starts_with("entry #3:") && problems[2].contains("bytes"));
        assert!(problems[3].contains("also listed in entries"));
    }

    #[test]
    fn the_wire_shape_is_json_with_no_room_for_extra_fields() {
        let json = serde_json::to_value(entry()).unwrap();
        let back: TemplateCatalogEntry = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, entry());
        let mut extra = json.clone();
        extra["url"] = serde_json::json!("https://elsewhere.example/pack.zip");
        assert!(serde_json::from_value::<TemplateCatalogEntry>(extra).is_err());
        let mut sneaky = json;
        sneaky["capabilities"]["runs_commands"] = serde_json::json!(true);
        assert!(serde_json::from_value::<TemplateCatalogEntry>(sneaky).is_err());
        let release: TemplateRelease =
            serde_json::from_value(serde_json::json!({ "entries": [] })).unwrap();
        assert!(release.revoked.is_empty());
    }
}
