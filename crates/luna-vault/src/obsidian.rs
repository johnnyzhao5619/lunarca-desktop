//! 既有 Obsidian 库的设置建议（PRD-06-064，笔记库对标轮 V4c）。
//!
//! 打开一个也在用 Obsidian 的库（有 `.obsidian/`）时，读它的 `app.json`，给一次建议：附件
//! 存放位置与排除列表要不要沿用。**只读 `.obsidian/`，从不写**；建议只给一次——用户沿用或
//! 不用都记在库里（`.luna/interop_decisions.json`），换台设备打开同一个库也不再问。
//!
//! `app.json` 里只看两项：
//! - `attachmentFolderPath`：`/`（或缺省，Obsidian 的默认）= 库根；`./` = 笔记所在文件夹；
//!   `./子目录` = 笔记所在文件夹下的子目录；其余 = 库里指定的文件夹。正好对上 Lunarca 的
//!   `vault.attachment_location` 四档。
//! - `userIgnoreFilters`：普通路径转成排除规则（[`crate::exclusions`]）；`/…/` 形式的正则
//!   Lunarca 不支持，跳过并如实报数。Obsidian 按前缀匹配（`Arch` 也覆盖 `Archive`），这里按
//!   文件夹边界——更窄，不会多藏东西。

use serde::Deserialize;

use crate::vault::Vault;
use crate::Result;

/// 决定记录里 Obsidian 那一条的键。
const OBSIDIAN_DECISION: &str = "obsidian";
/// 决定记录的 meta 名（`.luna/<name>.json`）。
const DECISIONS_META: &str = "interop_decisions";

/// Lunarca 的附件位置设置（`vault.attachment_location` / `vault.attachment_folder`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentSetting {
    /// `vault_root` / `note_folder` / `note_subfolder` / `specified_folder`。
    pub location: String,
    /// 子目录名或指定文件夹（前两档为 `None`，沿用时保持当前值不动）。
    pub folder: Option<String>,
}

/// 一次建议的内容：只列出与当前不同的部分。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObsidianSuggestion {
    /// 与当前设置不同时才有。
    pub attachment: Option<AttachmentSetting>,
    /// 尚未排除的那几条。
    pub exclusions: Vec<String>,
    /// 正则写法的排除条目数（无法沿用）。
    pub skipped_patterns: u32,
}

impl ObsidianSuggestion {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.attachment.is_none() && self.exclusions.is_empty()
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AppJson {
    attachment_folder_path: Option<String>,
    #[serde(default)]
    user_ignore_filters: Vec<String>,
}

/// `attachmentFolderPath` → Lunarca 的四档。
#[must_use]
pub fn attachment_setting_of(obsidian: &str) -> AttachmentSetting {
    let trimmed = obsidian.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return AttachmentSetting {
            location: "vault_root".into(),
            folder: None,
        };
    }
    if let Some(rest) = trimmed.strip_prefix("./") {
        let sub = rest.trim_matches('/');
        return if sub.is_empty() {
            AttachmentSetting {
                location: "note_folder".into(),
                folder: None,
            }
        } else {
            AttachmentSetting {
                location: "note_subfolder".into(),
                folder: Some(sub.to_owned()),
            }
        };
    }
    AttachmentSetting {
        location: "specified_folder".into(),
        folder: Some(trimmed.trim_matches('/').to_owned()),
    }
}

/// 一条 `userIgnoreFilters` 能不能当成路径规则：`/…/` 是正则，跳过。
fn ignore_filter_path(filter: &str) -> Option<String> {
    let trimmed = filter.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('/') && trimmed.ends_with('/') {
        return None;
    }
    let path = trimmed.trim_matches('/');
    (!path.is_empty() && !path.split('/').any(|part| part == "..")).then(|| path.to_owned())
}

impl Vault {
    /// 给这个库的 Obsidian 设置建议；不是 Obsidian 库、已经问过、或没有任何不同时返回 `None`。
    ///
    /// `current` 是此刻的 Lunarca 附件设置（设置值在应用层，`Vault` 不读设置）。
    ///
    /// # Errors
    /// 决定记录读不了时返回错误（宁可不问，也不反复打扰）。
    pub fn obsidian_suggestion(
        &self,
        current: &AttachmentSetting,
    ) -> Result<Option<ObsidianSuggestion>> {
        let dir = self.root().join(".obsidian");
        if !dir.is_dir() || self.obsidian_decided()? {
            return Ok(None);
        }
        // 没有 app.json 是 Obsidian 全用默认值的库；读坏了按默认处理（不因它报错）。
        let app: AppJson = std::fs::read(dir.join("app.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let wanted = attachment_setting_of(app.attachment_folder_path.as_deref().unwrap_or("/"));
        let same_location = wanted.location == current.location
            && (wanted.folder.is_none() || wanted.folder == current.folder);
        let existing = self.exclusions();
        let mut suggestion = ObsidianSuggestion {
            attachment: (!same_location).then_some(wanted),
            ..ObsidianSuggestion::default()
        };
        for filter in &app.user_ignore_filters {
            match ignore_filter_path(filter) {
                Some(path) if !existing.covers(&path) && !suggestion.exclusions.contains(&path) => {
                    suggestion.exclusions.push(path);
                }
                Some(_) => {}
                None => suggestion.skipped_patterns += 1,
            }
        }
        Ok((!suggestion.is_empty()).then_some(suggestion))
    }

    /// 记下「这个库的 Obsidian 建议已经问过了」（沿用或不用都记）。
    ///
    /// # Errors
    /// 写入失败。
    pub fn record_obsidian_decision(&self) -> Result<()> {
        if self.obsidian_decided()? {
            return Ok(());
        }
        let mut decided: Vec<String> = crate::meta::read_meta(self.root(), DECISIONS_META)?;
        decided.push(OBSIDIAN_DECISION.to_owned());
        crate::meta::write_meta(self.root(), DECISIONS_META, &decided)
    }

    fn obsidian_decided(&self) -> Result<bool> {
        let decided: Vec<String> = crate::meta::read_meta(self.root(), DECISIONS_META)?;
        Ok(decided.iter().any(|entry| entry == OBSIDIAN_DECISION))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(location: &str, folder: Option<&str>) -> AttachmentSetting {
        AttachmentSetting {
            location: location.into(),
            folder: folder.map(Into::into),
        }
    }

    #[test]
    fn attachment_paths_map_onto_the_four_lunarca_modes() {
        assert_eq!(attachment_setting_of("/"), setting("vault_root", None));
        assert_eq!(attachment_setting_of(""), setting("vault_root", None));
        assert_eq!(attachment_setting_of("./"), setting("note_folder", None));
        assert_eq!(
            attachment_setting_of("./附件"),
            setting("note_subfolder", Some("附件"))
        );
        assert_eq!(
            attachment_setting_of("资源/图片"),
            setting("specified_folder", Some("资源/图片"))
        );
    }

    #[test]
    fn suggests_only_what_differs_once_and_never_writes_obsidian_files() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let current = setting("note_subfolder", Some("assets"));
        assert_eq!(
            vault.obsidian_suggestion(&current).unwrap(),
            None,
            "不是 Obsidian 库"
        );

        std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        let app =
            r#"{"attachmentFolderPath":"./附件","userIgnoreFilters":["归档/","/^草稿.*/","模板"]}"#;
        std::fs::write(dir.path().join(".obsidian/app.json"), app).unwrap();
        vault.set_excluded("模板", true).unwrap();

        let suggestion = vault.obsidian_suggestion(&current).unwrap().unwrap();
        assert_eq!(
            suggestion.attachment,
            Some(setting("note_subfolder", Some("附件")))
        );
        assert_eq!(suggestion.exclusions, vec!["归档"], "已排除的不再建议");
        assert_eq!(suggestion.skipped_patterns, 1);

        // 附件位置已一致、排除也都有了：没什么可建议的。
        vault.set_excluded("归档", true).unwrap();
        assert_eq!(
            vault
                .obsidian_suggestion(&setting("note_subfolder", Some("附件")))
                .unwrap()
                .map(|s| s.is_empty()),
            None
        );

        // 问过一次就不再问。
        vault.record_obsidian_decision().unwrap();
        assert_eq!(vault.obsidian_suggestion(&current).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".obsidian/app.json")).unwrap(),
            app
        );
    }

    #[test]
    fn an_obsidian_vault_without_app_json_uses_obsidian_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        let suggestion = vault
            .obsidian_suggestion(&setting("note_subfolder", Some("assets")))
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.attachment, Some(setting("vault_root", None)));
        assert!(suggestion.exclusions.is_empty());
    }
}
