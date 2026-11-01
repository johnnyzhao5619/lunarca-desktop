//! 主题目录的条目（ADR-159 ③ ⑤，纵切 7a）：签名目录 `themes` 里的一条 = 一个主题包 + 它要求的
//! 最低客户端版本。
//!
//! 主题包很小，内联在条目里，目录签名即它的完整性来源——不另带 sha256、不另起下载。服务端冻结 /
//! 签名前跑 [`validate_theme_catalog`]，客户端拿到签名目录后用同一个函数逐条判，于是审核放过的
//! 条目到客户端那边不会被单条跳过。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::revocation::revocation_problems;
use crate::{is_builtin_theme, PackVersion, Revocation, ThemePackage};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeCatalogEntry {
    /// 主题包本体（与本地导入、内置主题同一个类型、同一套校验）。
    pub package: ThemePackage,
    /// 低于它的客户端只显示「需要更新 Lunarca」，不给安装（PRD-18-007）。
    pub min_client_version: PackVersion,
}

impl ThemeCatalogEntry {
    /// 这台客户端装得了它吗。
    #[must_use]
    pub fn supports(&self, client_version: PackVersion) -> bool {
        client_version >= self.min_client_version
    }
}

/// 控制台起草种子里的占位 id：合法、但发布不了（[`validate_theme_entry`] 点名要改）。
pub const THEME_ID_PLACEHOLDER: &str = "change.me.theme";

/// 拒绝时给运营的那句示例：改成什么样的 id。
const ID_HINT: &str = "set package.id to your own reverse-domain id, e.g. com.yourstudio.solar";

/// 控制台起草第一版主题目录的种子：**一条**示例主题，以内置 Lunarca 的配色为底、id 是
/// [`THEME_ID_PLACEHOLDER`]。运营改 id、名字与配色后发布；要上架多套就复制这一条。
///
/// 此前种子是全部内置主题、id 原样：运营只改了第一条的名字，就收到两条「内置 id」的拒绝，其中一条
/// 是他根本没打算发布的那套（2026-10-21 真机）。一条示例 + 占位 id 让「要改的只有这几格」一目了然。
///
/// `min_client_version` 取本 crate 的版本——认得这份条目格式的就是与它同版发布的客户端。
///
/// # Panics
/// 本 crate 的版本号不是 `x.y.z`、或占位 id 不合法时——那是发布配置错误，会被本模块的测试拦下。
#[must_use]
pub fn starter_theme_entry() -> ThemeCatalogEntry {
    let mut package = crate::builtin_themes()[0].clone();
    package.id = crate::ContentId::parse(THEME_ID_PLACEHOLDER).expect("占位 id 合法");
    package.name = "New theme".to_owned();
    ThemeCatalogEntry {
        package,
        min_client_version: PackVersion::parse(env!("CARGO_PKG_VERSION"))
            .expect("crate version is x.y.z"),
    }
}

/// 单条的问题（不含条目序号；整份目录的校验另带序号）。
///
/// # Errors
/// 主题包不合格、或占用了内置主题的 id 时。
pub fn validate_theme_entry(entry: &ThemeCatalogEntry) -> Result<(), String> {
    entry
        .package
        .validate()
        .map_err(|error| error.to_string())?;
    let id = entry.package.id.as_str();
    if id == THEME_ID_PLACEHOLDER {
        return Err(format!(
            "theme id {id} is the starter placeholder: {ID_HINT}"
        ));
    }
    if is_builtin_theme(id) {
        return Err(format!(
            "theme id {id} belongs to a built-in theme: {ID_HINT}"
        ));
    }
    Ok(())
}

/// 整份目录的问题清单（空 = 合格）：逐条 [`validate_theme_entry`]，外加 id 在目录里唯一——同一个
/// id 出现两次时，客户端不知道该装哪一条。
#[must_use]
pub fn validate_theme_catalog(entries: &[ThemeCatalogEntry]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Err(problem) = validate_theme_entry(entry) {
            problems.push(format!("entry #{index}: {problem}"));
        }
        if !seen.insert(entry.package.id.as_str()) {
            problems.push(format!(
                "entry #{index}: theme id {} appears more than once",
                entry.package.id.as_str()
            ));
        }
    }
    problems
}

/// 主题目录一个版本的内容：可获取的条目 + 封禁列表（[`Revocation`]，被封的已装主题回落内置 Lunarca）。
/// 签进同一份目录信封（`entries` 与 `revoked` 并列）；没有封禁时 `revoked` 可以不写。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeRelease {
    pub entries: Vec<ThemeCatalogEntry>,
    #[serde(default)]
    pub revoked: Vec<Revocation>,
}

/// 控制台起草的种子：一条示例主题、没有封禁（见 [`starter_theme_entry`]）。
#[must_use]
pub fn starter_theme_release() -> ThemeRelease {
    ThemeRelease {
        entries: vec![starter_theme_entry()],
        revoked: Vec::new(),
    }
}

/// 整个版本的问题清单（空 = 合格）：条目按 [`validate_theme_catalog`]，封禁按两类目录共用的那一套
/// （[`crate::revocation`]）。
#[must_use]
pub fn validate_theme_release(release: &ThemeRelease) -> Vec<String> {
    let mut problems = validate_theme_catalog(&release.entries);
    let listed: HashSet<&str> = release
        .entries
        .iter()
        .map(|entry| entry.package.id.as_str())
        .collect();
    problems.extend(revocation_problems(&listed, &release.revoked, "theme"));
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_themes;

    fn revocation(id: &str, reason: &str) -> Revocation {
        Revocation {
            id: crate::ContentId::parse(id).unwrap(),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn a_release_bans_by_id_with_a_reason_and_never_lists_and_bans_the_same_id() {
        let ok = ThemeRelease {
            entries: vec![entry("com.example.dusk")],
            revoked: vec![revocation("com.example.bad", "contained a misleading name")],
        };
        assert!(validate_theme_release(&ok).is_empty());

        let bad = ThemeRelease {
            entries: vec![entry("com.example.dusk")],
            revoked: vec![
                revocation("com.example.dusk", "x"),
                revocation("com.example.gone", " "),
                revocation("com.example.gone", &"长".repeat(201)),
            ],
        };
        let problems = validate_theme_release(&bad);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].contains("also listed in entries"));
        assert!(problems[1].contains("reason"));
        assert!(problems[2].contains("reason"));
        assert!(problems[3].contains("more than once"));
    }

    #[test]
    fn revoked_may_be_omitted_but_nothing_else_may_be_added() {
        let release: ThemeRelease = serde_json::from_value(serde_json::json!({
            "entries": [serde_json::to_value(entry("com.example.dusk")).unwrap()],
        }))
        .unwrap();
        assert!(release.revoked.is_empty());
        assert!(serde_json::from_value::<ThemeRelease>(serde_json::json!({
            "entries": [], "variants": [],
        }))
        .is_err());
        assert!(validate_theme_release(&starter_theme_release())
            .iter()
            .all(|problem| problem.contains("placeholder")));
    }

    fn entry(id: &str) -> ThemeCatalogEntry {
        let mut package = builtin_themes()[1].clone();
        package.id = crate::ContentId::parse(id).expect("valid id");
        ThemeCatalogEntry {
            package,
            min_client_version: PackVersion::parse("1.0.0").unwrap(),
        }
    }

    #[test]
    fn a_catalog_of_distinct_valid_themes_passes() {
        assert!(
            validate_theme_catalog(&[entry("com.example.dusk"), entry("com.example.dawn")])
                .is_empty()
        );
    }

    #[test]
    fn built_in_ids_duplicates_and_broken_packages_are_named_by_entry() {
        let mut broken = entry("com.example.broken");
        broken.package.light.canvas = "red".to_owned();
        let problems = validate_theme_catalog(&[
            entry("nebula"),
            entry("com.example.twice"),
            entry("com.example.twice"),
            broken,
        ]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("entry #0:") && problems[0].contains("built-in"));
        assert!(problems[1].starts_with("entry #2:") && problems[1].contains("more than once"));
        assert!(problems[2].starts_with("entry #3:"));
    }

    #[test]
    fn the_wire_shape_is_json_with_no_room_for_extra_fields() {
        let json = serde_json::to_value(entry("com.example.dusk")).unwrap();
        let back: ThemeCatalogEntry = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, entry("com.example.dusk"));
        let mut extra = json;
        extra["price"] = serde_json::json!(0);
        assert!(serde_json::from_value::<ThemeCatalogEntry>(extra).is_err());
    }

    #[test]
    /// 种子是一条示例：原样发布不了，拒绝的那句点名占位 id 并给出改法；只改了 id 就能发布。
    fn the_console_seed_is_one_starter_that_ships_once_its_id_is_changed() {
        let seed = starter_theme_entry();
        let problems = validate_theme_catalog(std::slice::from_ref(&seed));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("placeholder") && problems[0].contains("com.yourstudio"));
        let mut mine = seed;
        mine.package.id = crate::ContentId::parse("com.yourstudio.solar").unwrap();
        assert!(validate_theme_catalog(&[mine]).is_empty());
    }

    #[test]
    fn older_clients_are_told_to_update() {
        let mut needs_two = entry("com.example.dusk");
        needs_two.min_client_version = PackVersion::parse("2.0.0").unwrap();
        assert!(!needs_two.supports(PackVersion::parse("1.9.9").unwrap()));
        assert!(needs_two.supports(PackVersion::parse("2.0.0").unwrap()));
    }
}
