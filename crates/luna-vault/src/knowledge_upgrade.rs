//! 旧主题到证据格式的显式升级（ADR-134 / TDD-06 §3.9.5）。
//!
//! 升级只改 frontmatter：补 `luna.knowledge`（schema 1、由出身标签确定的范围、空的声明与出处表），
//! 正文字节一个不动。旧正文因此全部落在声明块之外——它们没有逐条出处，界面按「未核验」呈现，
//! 这里绝不从旧链接或旧片段猜补出处。人工改过的旧页升级后仍停在复核态，不借升级洗成系统内容。
//! 写入前另存原件备份，文件仍等于升级结果时才允许回退。

use crate::frontmatter::body_of;
use crate::knowledge_evidence::{KnowledgeMeta, TopicMaintenanceState, TopicScope};
use crate::topic::{ownership_of, TopicOwnership};
use crate::{ContentHash, Vault, VaultError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

const UPGRADE_BACKUP_SCHEMA: u32 = 1;
/// 备份整份原件；超过该大小的旧页不在本轮升级，避免无界 sidecar 写入。
const MAX_UPGRADE_BYTES: usize = 2 * 1024 * 1024;

/// 一次升级的只读预览。`after_hash` 是确认时必须回传的指纹：用户确认的即将写入的字节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicUpgradePreview {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
    pub content: String,
    pub scope: TopicScope,
    /// 旧页正文已被人工改过：升级后仍停在复核态，不会开放自动维护。
    pub user_edited: bool,
    /// 声明块之外（因此未核验）的内容行数（不含标题，判据见 `unverified_content_lines`）。
    pub unverified_lines: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicUpgradeOutcome {
    pub backup_id: Uuid,
    pub index_updated: bool,
    pub baseline_recorded: bool,
    pub maintenance: TopicMaintenanceState,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TopicUpgradeRejection {
    #[error("not a topic page")]
    NotTopic,
    #[error("topic already uses the current evidence format")]
    AlreadyCurrent,
    #[error("topic uses an unknown evidence schema")]
    UnknownSchema,
    #[error("topic metadata or evidence blocks are invalid")]
    InvalidMetadata,
    #[error("legacy topic has no origin tag to derive a scope from")]
    MissingScope,
    #[error("topic exceeded the upgrade size budget")]
    BudgetExceeded,
    #[error("topic changed since the preview")]
    TargetChanged,
    #[error("topic disappeared")]
    TargetMissing,
    #[error("vault read failed: {0}")]
    VaultRead(String),
    #[error("topic write failed")]
    WriteFailed,
    #[error("upgrade backup failed: {0}")]
    BackupFailed(String),
    #[error("upgrade backup is missing")]
    BackupMissing,
    #[error("knowledge commit lock failed: {0}")]
    CommitLock(String),
}

/// 给 IPC 的封闭原因码；Display 只供诊断。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicUpgradeRejectionCode {
    NotTopic,
    AlreadyCurrent,
    UnknownSchema,
    InvalidMetadata,
    MissingScope,
    BudgetExceeded,
    TargetChanged,
    TargetMissing,
    VaultReadFailed,
    WriteFailed,
    BackupFailed,
    BackupMissing,
    CommitLockUnavailable,
}

impl TopicUpgradeRejection {
    #[must_use]
    pub fn code(&self) -> TopicUpgradeRejectionCode {
        match self {
            Self::NotTopic => TopicUpgradeRejectionCode::NotTopic,
            Self::AlreadyCurrent => TopicUpgradeRejectionCode::AlreadyCurrent,
            Self::UnknownSchema => TopicUpgradeRejectionCode::UnknownSchema,
            Self::InvalidMetadata => TopicUpgradeRejectionCode::InvalidMetadata,
            Self::MissingScope => TopicUpgradeRejectionCode::MissingScope,
            Self::BudgetExceeded => TopicUpgradeRejectionCode::BudgetExceeded,
            Self::TargetChanged => TopicUpgradeRejectionCode::TargetChanged,
            Self::TargetMissing => TopicUpgradeRejectionCode::TargetMissing,
            Self::VaultRead(_) => TopicUpgradeRejectionCode::VaultReadFailed,
            Self::WriteFailed => TopicUpgradeRejectionCode::WriteFailed,
            Self::BackupFailed(_) => TopicUpgradeRejectionCode::BackupFailed,
            Self::BackupMissing => TopicUpgradeRejectionCode::BackupMissing,
            Self::CommitLock(_) => TopicUpgradeRejectionCode::CommitLockUnavailable,
        }
    }
}

/// 设备本地的原件备份；回退只在文件仍是升级结果时进行。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpgradeBackup {
    schema: u32,
    path: String,
    original: String,
    upgraded_hash: String,
}

fn backup_path(vault: &Vault, id: Uuid) -> PathBuf {
    vault
        .root()
        .join(".luna/sidecars/topic-upgrade-backups")
        .join(format!("{id}.json"))
}

fn read_target(vault: &Vault, rel: &str) -> Result<String, TopicUpgradeRejection> {
    match vault.read(rel) {
        Ok(doc) => Ok(doc.content),
        Err(VaultError::NotFound(_)) => Err(TopicUpgradeRejection::TargetMissing),
        Err(error) => Err(TopicUpgradeRejection::VaultRead(error.to_string())),
    }
}

impl Vault {
    /// 只读：旧主题升级后的完整文件与影响。未知 schema、坏元数据、已是新格式都拒绝，
    /// 不把「写不了」解释成「可以重写一遍」。
    pub fn preview_topic_upgrade(
        &self,
        rel: &str,
    ) -> Result<TopicUpgradePreview, TopicUpgradeRejection> {
        let content = read_target(self, rel)?;
        upgrade_preview_of(rel, &content)
    }

    /// 应用用户确认过的那一份升级：前后指纹都须与预览一致；先落备份，再按同一次读取的
    /// 整文件 hash 条件写。人工改过的旧页不登记系统底版，升级后仍停在复核态。
    pub fn apply_topic_upgrade(
        &self,
        rel: &str,
        before_hash: &str,
        after_hash: &str,
    ) -> Result<TopicUpgradeOutcome, TopicUpgradeRejection> {
        let _lock = crate::knowledge_plan::knowledge_commit_lock(self)
            .map_err(|error| TopicUpgradeRejection::CommitLock(error.to_string()))?;
        let content = read_target(self, rel)?;
        if ContentHash::of(content.as_bytes()).as_str() != before_hash {
            return Err(TopicUpgradeRejection::TargetChanged);
        }
        let preview = upgrade_preview_of(rel, &content)?;
        if preview.after_hash != after_hash {
            return Err(TopicUpgradeRejection::TargetChanged);
        }
        let backup_id = Uuid::now_v7();
        let path = backup_path(self, backup_id);
        let bytes = serde_json::to_vec(&UpgradeBackup {
            schema: UPGRADE_BACKUP_SCHEMA,
            path: preview.path.clone(),
            original: content,
            upgraded_hash: preview.after_hash.clone(),
        })
        .map_err(|error| TopicUpgradeRejection::BackupFailed(error.to_string()))?;
        std::fs::create_dir_all(path.parent().expect("backup path has parent"))
            .and_then(|()| {
                crate::atomic::write_atomic(&path, &bytes).map_err(std::io::Error::other)
            })
            .map_err(|error| TopicUpgradeRejection::BackupFailed(error.to_string()))?;
        let write_result = match self.write_if_current(rel, before_hash, &preview.content) {
            Ok(false) => {
                let _ = std::fs::remove_file(&path);
                return Err(TopicUpgradeRejection::TargetChanged);
            }
            Ok(true) => Ok(()),
            Err(error) => Err(error),
        };
        let committed = self
            .read(rel)
            .is_ok_and(|doc| ContentHash::of(doc.content.as_bytes()).as_str() == after_hash);
        if !committed {
            // 未确认写成：备份保留，供人工排查；不能宣称已升级。
            return Err(TopicUpgradeRejection::WriteFailed);
        }
        let baseline_recorded =
            !preview.user_edited && self.record_topic_baseline(rel, &preview.content);
        let maintenance = self
            .topic_maintenance_state(rel)
            .unwrap_or(TopicMaintenanceState::InvalidEvidence);
        Ok(TopicUpgradeOutcome {
            backup_id,
            index_updated: write_result.is_ok(),
            baseline_recorded,
            maintenance,
        })
    }

    /// 回退一次升级：文件仍逐字节等于升级结果时恢复原件并删除备份；之后又被改过则拒绝，
    /// 不用旧字节覆盖后续内容。随库决定与该页无关（旧格式页不产生计划），无需一并回退。
    pub fn restore_topic_upgrade(&self, backup_id: Uuid) -> Result<String, TopicUpgradeRejection> {
        let _lock = crate::knowledge_plan::knowledge_commit_lock(self)
            .map_err(|error| TopicUpgradeRejection::CommitLock(error.to_string()))?;
        let path = backup_path(self, backup_id);
        let bytes = std::fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                TopicUpgradeRejection::BackupMissing
            } else {
                TopicUpgradeRejection::BackupFailed(error.to_string())
            }
        })?;
        let backup: UpgradeBackup = serde_json::from_slice(&bytes)
            .map_err(|error| TopicUpgradeRejection::BackupFailed(error.to_string()))?;
        if backup.schema != UPGRADE_BACKUP_SCHEMA {
            return Err(TopicUpgradeRejection::BackupFailed(
                "unknown upgrade backup schema".into(),
            ));
        }
        match self.write_if_current(&backup.path, &backup.upgraded_hash, &backup.original) {
            Ok(true) => {}
            Ok(false) => return Err(TopicUpgradeRejection::TargetChanged),
            Err(VaultError::NotFound(_)) => return Err(TopicUpgradeRejection::TargetMissing),
            // 索引失败时文件可能已恢复；下面按字节回读判定。
            Err(_) => {}
        }
        let restored = self
            .read(&backup.path)
            .is_ok_and(|doc| doc.content == backup.original);
        if !restored {
            return Err(TopicUpgradeRejection::WriteFailed);
        }
        let _ = std::fs::remove_file(&path);
        Ok(backup.path)
    }
}

fn upgrade_preview_of(
    rel: &str,
    content: &str,
) -> Result<TopicUpgradePreview, TopicUpgradeRejection> {
    if content.len() > MAX_UPGRADE_BYTES {
        return Err(TopicUpgradeRejection::BudgetExceeded);
    }
    let mut meta = crate::frontmatter::read_luna_checked(content)
        .map_err(|_| TopicUpgradeRejection::InvalidMetadata)?;
    if !meta.topic || meta.knowledge_map {
        return Err(TopicUpgradeRejection::NotTopic);
    }
    if meta.knowledge.is_some() {
        return Err(match crate::validate_topic_document(content) {
            Ok(_) => TopicUpgradeRejection::AlreadyCurrent,
            Err(crate::KnowledgeError::UnknownSchema(_)) => TopicUpgradeRejection::UnknownSchema,
            Err(_) => TopicUpgradeRejection::InvalidMetadata,
        });
    }
    let tag = meta
        .topic_src
        .as_deref()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .ok_or(TopicUpgradeRejection::MissingScope)?;
    let scope = TopicScope::from_tag(tag);
    if !scope.valid() {
        return Err(TopicUpgradeRejection::MissingScope);
    }
    let user_edited = ownership_of(content) != TopicOwnership::AiOwned;
    meta.knowledge = Some(
        serde_yaml_ng::to_value(KnowledgeMeta::new(scope.clone()))
            .map_err(|_| TopicUpgradeRejection::InvalidMetadata)?,
    );
    let upgraded =
        crate::update_luna(content, &meta).map_err(|_| TopicUpgradeRejection::InvalidMetadata)?;
    // 正文逐字节不变是本升级的全部承诺；做不到就不写。
    if body_of(&upgraded) != body_of(content) || crate::validate_topic_document(&upgraded).is_err()
    {
        return Err(TopicUpgradeRejection::InvalidMetadata);
    }
    let unverified_lines = crate::knowledge_evidence::unverified_content_lines(body_of(content));
    Ok(TopicUpgradePreview {
        path: rel.to_owned(),
        before_hash: ContentHash::of(content.as_bytes()).to_string(),
        after_hash: ContentHash::of(upgraded.as_bytes()).to_string(),
        content: upgraded,
        scope,
        user_edited,
        unverified_lines: u32::try_from(unverified_lines).unwrap_or(u32::MAX),
    })
}

impl Vault {
    /// 仍是旧格式、可以显式升级的主题页（按路径排序）。只看格式，不读快照、不判人工改动——
    /// 那些在预览时逐页说明；缺出身标签等不能升级的页也列出来，由预览如实给出原因。
    pub fn legacy_topic_pages(&self) -> crate::Result<Vec<String>> {
        let mut pages = Vec::new();
        for path in self.topic_pages()? {
            let Ok(doc) = self.read(&path) else {
                continue;
            };
            if matches!(
                crate::validate_topic_document(&doc.content),
                Err(crate::KnowledgeError::NeedsUpgrade)
            ) {
                pages.push(path);
            }
        }
        pages.sort();
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "Knowledge/项目.md";

    fn legacy(vault: &Vault) -> String {
        vault
            .write_topic(
                TOPIC,
                "# 项目\n\n## 进展\n\n- 原型已完成（[会议](../Notes/会议.md)）\n",
                Some("项目/A"),
            )
            .unwrap();
        vault.read(TOPIC).unwrap().content
    }

    #[test]
    fn upgrade_changes_only_frontmatter_and_can_be_restored() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let original = legacy(&vault);
        assert_eq!(
            vault.topic_maintenance_state(TOPIC).unwrap(),
            TopicMaintenanceState::NeedsUpgrade
        );
        let preview = vault.preview_topic_upgrade(TOPIC).unwrap();
        assert!(!preview.user_edited);
        assert_eq!(preview.scope.includes, vec!["项目/A".to_owned()]);
        // 两个标题不算；只有那条要点没有逐条出处。
        assert_eq!(preview.unverified_lines, 1);
        assert_eq!(vault.legacy_topic_pages().unwrap(), vec![TOPIC.to_owned()]);
        assert_eq!(body_of(&preview.content), body_of(&original));
        assert_eq!(vault.read(TOPIC).unwrap().content, original, "预览不写盘");

        let outcome = vault
            .apply_topic_upgrade(TOPIC, &preview.before_hash, &preview.after_hash)
            .unwrap();
        assert!(outcome.baseline_recorded);
        assert_eq!(outcome.maintenance, TopicMaintenanceState::Ready);
        let upgraded = vault.read(TOPIC).unwrap().content;
        assert_eq!(upgraded, preview.content);
        assert!(crate::topic_claim_statements(&upgraded).unwrap().is_empty());
        // 升级后阅读页头用同一判据标「未核验」：与预览说的是同一个数。
        assert_eq!(
            vault.topic_reading_summary(TOPIC).unwrap().unverified_lines,
            preview.unverified_lines
        );
        assert_eq!(
            vault.preview_topic_upgrade(TOPIC),
            Err(TopicUpgradeRejection::AlreadyCurrent)
        );

        assert_eq!(
            vault.restore_topic_upgrade(outcome.backup_id).unwrap(),
            TOPIC
        );
        assert_eq!(vault.read(TOPIC).unwrap().content, original);
        assert_eq!(
            vault.restore_topic_upgrade(outcome.backup_id),
            Err(TopicUpgradeRejection::BackupMissing)
        );
    }

    #[test]
    fn stale_confirmation_and_later_edits_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let original = legacy(&vault);
        let preview = vault.preview_topic_upgrade(TOPIC).unwrap();
        assert_eq!(
            vault.apply_topic_upgrade(TOPIC, &preview.before_hash, "another-preview"),
            Err(TopicUpgradeRejection::TargetChanged)
        );
        let edited = original.replace("原型已完成", "原型已完成（人工补充）");
        std::fs::write(dir.path().join(TOPIC), &edited).unwrap();
        assert_eq!(
            vault.apply_topic_upgrade(TOPIC, &preview.before_hash, &preview.after_hash),
            Err(TopicUpgradeRejection::TargetChanged)
        );
        assert_eq!(vault.read(TOPIC).unwrap().content, edited);

        // 人工改过的旧页可以显式升级格式，但不借升级洗成系统内容。
        let preview = vault.preview_topic_upgrade(TOPIC).unwrap();
        assert!(preview.user_edited);
        let outcome = vault
            .apply_topic_upgrade(TOPIC, &preview.before_hash, &preview.after_hash)
            .unwrap();
        assert!(!outcome.baseline_recorded);
        assert_eq!(outcome.maintenance, TopicMaintenanceState::BodyChanged);

        // 升级后又被改过：回退拒绝，不用旧字节覆盖后续内容。
        let later = vault
            .read(TOPIC)
            .unwrap()
            .content
            .replace("人工补充", "再次修改");
        std::fs::write(dir.path().join(TOPIC), &later).unwrap();
        assert_eq!(
            vault.restore_topic_upgrade(outcome.backup_id),
            Err(TopicUpgradeRejection::TargetChanged)
        );
        assert_eq!(vault.read(TOPIC).unwrap().content, later);
    }

    #[test]
    fn unknown_schema_broken_metadata_and_missing_scope_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let original = legacy(&vault);
        let mut meta = crate::read_luna(&original);
        meta.knowledge = Some(
            serde_yaml_ng::to_value(serde_json::json!({
                "schema": 2,
                "scope": {"title": "项目", "description": "", "includes": ["项目/A"]}
            }))
            .unwrap(),
        );
        let unknown = crate::update_luna(&original, &meta).unwrap();
        std::fs::write(dir.path().join(TOPIC), &unknown).unwrap();
        assert_eq!(
            vault.preview_topic_upgrade(TOPIC),
            Err(TopicUpgradeRejection::UnknownSchema)
        );

        let mut meta = crate::read_luna(&original);
        meta.topic_src = None;
        let orphan = crate::update_luna(&original, &meta).unwrap();
        std::fs::write(dir.path().join(TOPIC), &orphan).unwrap();
        assert_eq!(
            vault.preview_topic_upgrade(TOPIC),
            Err(TopicUpgradeRejection::MissingScope)
        );

        vault.write("Notes/普通.md", "# 普通\n").unwrap();
        assert_eq!(
            vault.preview_topic_upgrade("Notes/普通.md"),
            Err(TopicUpgradeRejection::NotTopic)
        );
        assert_eq!(
            vault.preview_topic_upgrade("Knowledge/缺失.md"),
            Err(TopicUpgradeRejection::TargetMissing)
        );
    }
}
