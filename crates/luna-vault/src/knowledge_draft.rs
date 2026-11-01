//! 新建证据主题的封存初稿（ADR-134 / TDD-06 §3.9.4，K2「初稿命令」）。
//!
//! 与增量计划同一纪律：模型只在准备阶段跑一次，结果连同证据、逐条声明与版本号封存到设备本地
//! `.luna/sidecars/knowledge-drafts/<operation_id>.json`；用户确认时只凭 operation ID 与预览
//! 指纹发布**同一份字节**，不重跑模型。发布前复核库身份、目标仍不存在、来源版本未变，再走
//! [`Vault::publish_scoped_topic`] 的原子占位与底版登记。

use crate::knowledge_generation::{TopicEvidenceInput, TopicGeneration};
use crate::knowledge_plan::knowledge_commit_lock;
use crate::{ClaimKind, ContentHash, PlanEvidence, PlanRejection, PlanRevisions, TopicScope};
use crate::{Vault, VaultError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

const DRAFT_ARTIFACT_SCHEMA: u32 = 1;
/// 初稿含整页 Markdown 与证据；超过这个大小不落盘（与增量计划同量级的防异常上限）。
const MAX_DRAFT_BYTES: usize = 2 * 1024 * 1024;

/// 初稿里的一条声明：文本、证据身份与所引证据序号（指向 [`TopicDraft::evidence`]）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftClaim {
    pub text: String,
    pub kind: ClaimKind,
    pub evidence_indices: Vec<usize>,
}

/// 一份已封存、待用户确认的新主题初稿。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicDraft {
    pub operation_id: Uuid,
    pub vault_identity: String,
    pub target_path: String,
    pub topic_src: String,
    pub content: String,
    pub result_hash: String,
    pub evidence: Vec<PlanEvidence>,
    pub claims: Vec<DraftClaim>,
    pub input_complete: bool,
    pub omitted_sources: u32,
    pub model_revision: String,
    pub prompt_revision: String,
    pub pipeline_revision: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftArtifact {
    schema: u32,
    draft: TopicDraft,
}

/// 发布结果：`index_updated`/`baseline_recorded` 与增量应用同一口径分别回执。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicDraftPublished {
    pub path: String,
    pub already_published: bool,
}

fn draft_path(vault: &Vault, operation_id: Uuid) -> PathBuf {
    vault
        .root()
        .join(".luna/sidecars/knowledge-drafts")
        .join(format!("{operation_id}.json"))
}

impl Vault {
    /// 把一次已校验的模型结果封存为初稿。无新增（no-op）不封存；同 ID 不允许换稿。
    #[allow(clippy::too_many_arguments)]
    pub fn seal_topic_draft(
        &self,
        target_path: &str,
        topic_src: &str,
        scope: TopicScope,
        evidence: &[TopicEvidenceInput],
        generation: &TopicGeneration,
        input_complete: bool,
        omitted_sources: u32,
        revisions: PlanRevisions,
    ) -> Result<TopicDraft, PlanRejection> {
        let TopicGeneration::Claims { claims } = generation else {
            return Err(PlanRejection::InvalidPlan);
        };
        let content = self
            .scoped_topic_preview_from_generation(
                target_path,
                Some(topic_src),
                scope,
                evidence,
                generation,
            )
            .map_err(|_| PlanRejection::RenderFailed)?
            .ok_or(PlanRejection::InvalidPlan)?;
        let draft = TopicDraft {
            operation_id: Uuid::now_v7(),
            vault_identity: self.knowledge_vault_identity(),
            target_path: target_path.to_owned(),
            topic_src: topic_src.to_owned(),
            result_hash: ContentHash::of(content.as_bytes()).to_string(),
            content,
            evidence: evidence.iter().map(|item| item.source.clone()).collect(),
            claims: claims
                .iter()
                .map(|claim| DraftClaim {
                    text: claim.text.clone(),
                    kind: claim.kind,
                    evidence_indices: claim.evidence_indices.clone(),
                })
                .collect(),
            input_complete,
            omitted_sources,
            model_revision: revisions.model_revision,
            prompt_revision: revisions.prompt_revision,
            pipeline_revision: revisions.pipeline_revision,
        };
        let bytes = serde_json::to_vec(&DraftArtifact {
            schema: DRAFT_ARTIFACT_SCHEMA,
            draft: draft.clone(),
        })
        .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        if bytes.len() > MAX_DRAFT_BYTES {
            return Err(PlanRejection::BudgetExceeded);
        }
        let _commit_lock = knowledge_commit_lock(self)?;
        let path = draft_path(self, draft.operation_id);
        std::fs::create_dir_all(path.parent().expect("draft artifact has parent"))
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        crate::atomic::write_atomic(&path, &bytes)
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        Ok(draft)
    }

    /// 按 operation ID 取回封存初稿；缺失、损坏、未知版本与他库分别报原因。
    pub fn load_topic_draft(&self, operation_id: Uuid) -> Result<TopicDraft, PlanRejection> {
        let bytes = std::fs::read(draft_path(self, operation_id)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PlanRejection::PlanMissing
            } else {
                PlanRejection::PlanStore(error.to_string())
            }
        })?;
        if bytes.len() > MAX_DRAFT_BYTES {
            return Err(PlanRejection::BudgetExceeded);
        }
        let artifact: DraftArtifact = serde_json::from_slice(&bytes)
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        if artifact.schema != DRAFT_ARTIFACT_SCHEMA {
            return Err(PlanRejection::PlanStore(
                "unknown draft artifact schema".into(),
            ));
        }
        if artifact.draft.operation_id != operation_id {
            return Err(PlanRejection::InvalidPlan);
        }
        if artifact.draft.vault_identity != self.knowledge_vault_identity() {
            return Err(PlanRejection::WrongVault);
        }
        Ok(artifact.draft)
    }

    /// 这个标签此刻**仍可直接发布**的那份封存初稿（后台起草，PRD-16-060）：库身份对得上、
    /// 目标页还不存在、每条证据的来源正文仍是生成时那一版；有几份取最新的（操作 ID 按时间有序）。
    /// 不成立的初稿不在这里删——发布与放弃各有自己的清理口径。
    #[must_use]
    pub fn ready_topic_draft_for(&self, tag: &str) -> Option<TopicDraft> {
        let mut drafts: Vec<TopicDraft> = self
            .topic_draft_ids()
            .into_iter()
            .filter_map(|id| self.load_topic_draft(id).ok())
            .filter(|draft| draft.topic_src.eq_ignore_ascii_case(tag))
            .filter(|draft| matches!(self.read(&draft.target_path), Err(VaultError::NotFound(_))))
            .filter(|draft| {
                draft.evidence.iter().all(|item| {
                    self.body_fingerprint(&item.source_path).as_deref()
                        == Some(item.source_hash.as_str())
                })
            })
            .collect();
        drafts.sort_by_key(|draft| draft.operation_id);
        drafts.pop()
    }

    /// 丢弃这个标签的全部封存初稿（用户忽略了这条主题建议，或来源已变、后台要重备）。
    pub fn discard_topic_drafts_for(&self, tag: &str) {
        for id in self.topic_draft_ids() {
            if self
                .load_topic_draft(id)
                .is_ok_and(|draft| draft.topic_src.eq_ignore_ascii_case(tag))
            {
                let _ = self.discard_topic_draft(id);
            }
        }
    }

    fn topic_draft_ids(&self) -> Vec<Uuid> {
        let Ok(entries) = std::fs::read_dir(self.root().join(".luna/sidecars/knowledge-drafts"))
        else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .and_then(|stem| Uuid::parse_str(stem).ok())
            })
            .collect()
    }

    /// 丢弃封存初稿（用户不要了，或已发布后清理）；已不存在视为完成。
    pub fn discard_topic_draft(&self, operation_id: Uuid) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        match std::fs::remove_file(draft_path(self, operation_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlanRejection::PlanStore(error.to_string())),
        }
    }

    /// 发布用户确认的那份初稿。指纹不符、目标已被占用、来源变了都拒绝且不写；目标已是
    /// 这份字节（上次发布后崩溃在清理前）按已发布处理。成功后清理载荷（失败不影响结果）。
    pub fn publish_topic_draft(
        &self,
        operation_id: Uuid,
        result_hash: &str,
    ) -> Result<TopicDraftPublished, PlanRejection> {
        let draft = self.load_topic_draft(operation_id)?;
        if draft.result_hash != result_hash {
            return Err(PlanRejection::InvalidPlan);
        }
        let published = {
            let _commit_lock = knowledge_commit_lock(self)?;
            match self.read(&draft.target_path) {
                Ok(existing) if existing.content == draft.content => TopicDraftPublished {
                    path: draft.target_path.clone(),
                    already_published: true,
                },
                Ok(_) => return Err(PlanRejection::TargetChanged),
                Err(VaultError::NotFound(_)) => {
                    for item in &draft.evidence {
                        match self.body_fingerprint(&item.source_path) {
                            Some(hash) if hash == item.source_hash => {}
                            Some(_) => {
                                return Err(PlanRejection::SourceChanged(item.source_path.clone()))
                            }
                            None => {
                                return Err(PlanRejection::SourceMissing(item.source_path.clone()))
                            }
                        }
                    }
                    self.publish_scoped_topic(&draft.target_path, &draft.content)
                        .map_err(|error| match error {
                            VaultError::InvalidReplace(_) => {
                                PlanRejection::SourceChanged(String::new())
                            }
                            _ => PlanRejection::WriteFailed,
                        })?;
                    TopicDraftPublished {
                        path: draft.target_path.clone(),
                        already_published: false,
                    }
                }
                Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
            }
        };
        let _ = self.discard_topic_draft(operation_id);
        Ok(published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GroundedClaim, SourcePolicy};

    const SOURCE: &str = "Notes/周会.md";
    const TARGET: &str = "Knowledge/项目.md";

    fn sealed(vault: &Vault) -> TopicDraft {
        vault
            .write(SOURCE, "---\ntags: [项目]\n---\n# 周会\n\n原型已经完成。\n")
            .unwrap();
        let batch = vault
            .topic_evidence(&[SOURCE.to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        let generation = TopicGeneration::Claims {
            claims: vec![GroundedClaim {
                id: Uuid::now_v7(),
                text: "原型已经完成".into(),
                kind: ClaimKind::Synthesis,
                evidence_indices: vec![0],
            }],
        };
        vault
            .seal_topic_draft(
                TARGET,
                "项目",
                TopicScope::from_tag("项目"),
                &batch.evidence,
                &generation,
                batch.input_complete,
                batch.omitted_sources,
                PlanRevisions {
                    model_revision: "m".into(),
                    prompt_revision: "p".into(),
                    pipeline_revision: "topic-draft-v1".into(),
                },
            )
            .unwrap()
    }

    #[test]
    fn sealed_draft_publishes_the_exact_previewed_bytes_once() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let draft = sealed(&vault);
        assert_eq!(vault.load_topic_draft(draft.operation_id).unwrap(), draft);
        assert!(vault.read(TARGET).is_err(), "封存不写主题");
        let published = vault
            .publish_topic_draft(draft.operation_id, &draft.result_hash)
            .unwrap();
        assert_eq!(published.path, TARGET);
        assert!(!published.already_published);
        assert_eq!(vault.read(TARGET).unwrap().content, draft.content);
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            crate::TopicMaintenanceState::Ready
        );
        assert_eq!(
            vault.load_topic_draft(draft.operation_id),
            Err(PlanRejection::PlanMissing),
            "发布后清理载荷"
        );
    }

    /// 后台起草的初稿（PRD-16-060）：按标签找回仍可直接发布的那一份；来源一改、目标被占，
    /// 它就不再算「已备好」；忽略这条建议时整份丢弃。
    #[test]
    fn ready_drafts_are_found_by_tag_only_while_still_publishable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let draft = sealed(&vault);
        assert_eq!(vault.ready_topic_draft_for("项目"), Some(draft.clone()));
        assert_eq!(vault.ready_topic_draft_for("项目/别的"), None);
        vault
            .write(SOURCE, "---\ntags: [项目]\n---\n# 周会\n\n原型推迟。\n")
            .unwrap();
        assert_eq!(
            vault.ready_topic_draft_for("项目"),
            None,
            "来源改了就不算备好"
        );
        let again = sealed(&vault);
        assert_eq!(vault.ready_topic_draft_for("项目"), Some(again.clone()));
        vault.discard_topic_drafts_for("项目");
        assert_eq!(vault.ready_topic_draft_for("项目"), None);
        assert!(vault.load_topic_draft(again.operation_id).is_err());
        assert!(vault.load_topic_draft(draft.operation_id).is_err());
    }

    #[test]
    fn changed_sources_wrong_hash_and_occupied_targets_never_write() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let draft = sealed(&vault);
        assert_eq!(
            vault.publish_topic_draft(draft.operation_id, "sha256:other"),
            Err(PlanRejection::InvalidPlan)
        );
        vault
            .write(SOURCE, "---\ntags: [项目]\n---\n# 周会\n\n原型推迟。\n")
            .unwrap();
        assert!(matches!(
            vault.publish_topic_draft(draft.operation_id, &draft.result_hash),
            Err(PlanRejection::SourceChanged(_))
        ));
        assert!(vault.read(TARGET).is_err());

        let other = sealed(&vault);
        vault.write(TARGET, "# 我自己写的\n").unwrap();
        assert_eq!(
            vault.publish_topic_draft(other.operation_id, &other.result_hash),
            Err(PlanRejection::TargetChanged)
        );
        assert_eq!(vault.read(TARGET).unwrap().content, "# 我自己写的\n");
    }
}
