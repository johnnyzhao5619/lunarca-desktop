//! Vault-level metadata store (durable·sync-friendly, same discipline as .luna/settings.toml, TDD-06 §1/§4.7).
//!
//! Currently holds: glossary (personal dictionary, PRD-02 F-GAP-01), the workspace
//! main-area layout snapshot (tabs/panes, PRD-07-035/036), the AI-tag exclusion list
//! (PRD-06-035c) and user distill templates (PRD-06-051). Atomic writes, lazy creation.
//! name whitelist enforced below. Schema version same discipline as sidecar.

use crate::atomic::write_atomic;
use crate::error::{Result, VaultError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::Path;

const META_SCHEMA: u32 = 1;
const ALLOWED_NAMES: &[&str] = &[
    // 创作的来源指南缓存（ADR-167 ⑥，`crate::research`）：按路径 + 正文指纹，随库同步。
    "creator_source_guides",
    "glossary",
    "workspace",
    "graph_scope",
    // 排除路径（V4a，`crate::exclusions`）：用户划出的降噪区，同 `graph_scope` 随库迁移。
    "excluded_paths",
    // 一次性互操作建议的决定（V4c，`crate::obsidian`）：沿用或不用都记，随库走，换设备不再问。
    "interop_decisions",
    // 书签（V5b，`crate::bookmarks`；ADR-140 ①：内容怎么组织，随库走）。
    "bookmarks",
    "distill_templates",
    // 用户对主题页建议按下的「不用了」（PRD-16-016）。**权威数据**，故归 meta store 而非
    // 可重建的 index.sqlite——若随索引重建而复活，就是「我明明关掉了它又回来了」这个真 bug。
    "topic_ignores",
    // 用户对某篇笔记的归入建议按下的 ✕（PRD-16-025）。同 `topic_ignores` 的分层判据：
    // 计划本身是派生的（重跑一次 LLM 就有，故落 index.sqlite），而「我不要这条」是
    // **权威决定**——随派生索引重建而复活就是「我明明关掉了它又回来了」这个真 bug。
    "ingest_ignores",
    // 新证据主题的逐版本忽略/否决是用户决定，不能放在可删除的 index.sqlite。
    "knowledge_ingest_decisions",
    // 用户对某条体检发现按下的 ✕（PRD-16-032 忽略项持久化）。同上分层判据。
    "checkup_ignores",
    // 全库忽略的 AI 关系标签（PRD-16-051）。**权威数据**：派生索引里只放副本，重建后按它
    // 重新压下；放进 index.sqlite 就会随重建「我明明忽略了它又回来了」。
    "ai_tag_ignores",
    // 上一次深度体检的完成时刻（PRD-16-032 周期提醒的唯一依据）。**发生过的事实**，
    // 不是可从笔记重算的派生值——落派生索引会在一次重建后凭空"从没体检过"。
    "checkup_state",
    "ai_presets",
    // 最近打开的一页知识产物（工作台「继续阅读」）。与 `workspace` 布局快照同类的界面现场，
    // 只存库内相对路径，不存正文或阅读位置。
    "knowledge_reading",
    // 用户本人的口播语速（PRD-19-012，`crate::speaking_rate`）：只有聚合出的两个数与样本规模，
    // 不含任何转写文字；随库走——同一个人在别的设备上念得一样快。
    "speaking_rate",
    // 用户新建的发布渠道（ADR-166，`crate::channel`）：名字、形态、上限与一句风格说明；
    // 随库走——换设备也是同一套渠道。
    "creator_channels",
    // 「我的渠道」与新项目默认渠道（ADR-166 ⑦）：怎么发是这个创作者 / 这个库的事，随库走。
    "creator_channel_favorites",
];
const LUNA_META_DIR: &str = ".luna";

#[derive(Serialize, Deserialize)]
struct MetaWrapper<T> {
    schema: u32,
    entries: Vec<T>,
}

fn validate_name(name: &str) -> Result<()> {
    if !ALLOWED_NAMES.contains(&name) {
        return Err(VaultError::InvalidMetaName(name.to_owned()));
    }
    // Path traversal protection: reject non-identifier chars, "..", and "/"
    if name.contains("..")
        || name.contains('/')
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(VaultError::InvalidMetaName(name.to_owned()));
    }
    Ok(())
}

pub fn vault_meta_path(root: &Path, name: &str) -> std::path::PathBuf {
    root.join(LUNA_META_DIR).join(name)
}

fn meta_path(root: &Path, name: &str) -> Result<std::path::PathBuf> {
    validate_name(name)?;
    Ok(vault_meta_path(root, &format!("{name}.json")))
}

pub fn read_meta<T: DeserializeOwned>(root: &Path, name: &str) -> Result<Vec<T>> {
    let path = meta_path(root, name)?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let wrapper: MetaWrapper<T> =
                serde_json::from_slice(&bytes).map_err(|e| VaultError::Decode(e.to_string()))?;
            if wrapper.schema != META_SCHEMA {
                return Err(VaultError::Decode(format!(
                    "unsupported vault meta schema: {}",
                    wrapper.schema
                )));
            }
            Ok(wrapper.entries)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(VaultError::Io(e)),
    }
}

pub fn write_meta<T: Serialize + Clone>(root: &Path, name: &str, value: &[T]) -> Result<()> {
    let path = meta_path(root, name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wrapper = MetaWrapper {
        schema: META_SCHEMA,
        entries: value.to_vec(),
    };
    let bytes =
        serde_json::to_vec_pretty(&wrapper).map_err(|e| VaultError::Decode(e.to_string()))?;
    write_atomic(&path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Glos {
        term: String,
        canonical: String,
    }

    #[test]
    fn read_meta_returns_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        let got: Vec<Glos> = read_meta(dir.path(), "glossary").unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn luna_meta_path_is_luna_directory_only() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".luna")).unwrap();

        let path = meta_path(root.path(), "glossary").unwrap();

        assert_eq!(path, root.path().join(".luna").join("glossary.json"));
    }

    #[test]
    fn luna_meta_reader_reads_luna_data() {
        let root = tempdir().unwrap();
        let luna_entries = vec![Glos {
            term: "luna".into(),
            canonical: "新目录内容".into(),
        }];

        std::fs::create_dir_all(root.path().join(".luna")).unwrap();
        std::fs::write(
            root.path().join(".luna/glossary.json"),
            serde_json::to_vec(&MetaWrapper {
                schema: META_SCHEMA,
                entries: luna_entries.clone(),
            })
            .unwrap(),
        )
        .unwrap();

        let loaded: Vec<Glos> = read_meta(root.path(), "glossary").unwrap();
        assert_eq!(loaded, luna_entries);
    }

    #[test]
    fn write_meta_persists_atomically_and_reads_back() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        let entries = vec![Glos {
            term: "AI".into(),
            canonical: "人工智能".into(),
        }];
        write_meta(dir.path(), "glossary", &entries).unwrap();
        let got: Vec<Glos> = read_meta(dir.path(), "glossary").unwrap();
        assert_eq!(got, entries);
        let raw = std::fs::read_to_string(dir.path().join(".luna/glossary.json")).unwrap();
        assert!(raw.contains("\"schema\": 1"));
    }

    #[test]
    fn unknown_schema_is_not_read_as_current_user_decisions() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        let path = dir.path().join(".luna/knowledge_ingest_decisions.json");
        std::fs::write(&path, r#"{"schema":2,"entries":[]}"#).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            read_meta::<serde_json::Value>(dir.path(), "knowledge_ingest_decisions"),
            Err(VaultError::Decode(_))
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn rejects_invalid_meta_name() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        assert!(read_meta::<serde_json::Value>(dir.path(), "../evil").is_err());
        assert!(read_meta::<serde_json::Value>(dir.path(), "FOO").is_err());
        assert!(read_meta::<serde_json::Value>(dir.path(), "a/b").is_err());
    }
}
