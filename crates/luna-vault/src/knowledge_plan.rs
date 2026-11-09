//! 主题增量计划的封闭动作与版本闸门（ADR-134 / TDD-06 §3.9）。
//!
//! 本模块只准备和验证计划，不自行应用模型输出。预览与应用应复用同一个 `IngestPlan`；
//! 将来接入持久队列时必须在最终提交边界再次调用 `check_ingest_plan`。

use crate::frontmatter::body_of;
use crate::knowledge_evidence::{
    render_generated_claim, ClaimKind, ClaimSource, EvidenceLocator, TopicMaintenanceState,
    TopicScope,
};
use crate::{Vault, VaultError};
use luna_sync_core::hashing::ContentHash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use uuid::Uuid;

/// 预览时固定的目标和库身份；完整文件 hash 同时覆盖正文与 frontmatter。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanBaseline {
    pub vault_identity: String,
    pub target_path: String,
    pub target_hash: String,
    pub scope_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEvidence {
    pub source_path: String,
    pub source_title: String,
    pub source_hash: String,
    pub locator: EvidenceLocator,
    pub complete_input: bool,
}

/// 模型只能引用 `evidence` 中的序号，不提供路径或 Markdown 链接。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanClaim {
    pub id: Uuid,
    pub text: String,
    pub kind: ClaimKind,
    pub evidence_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum IngestAction {
    AppendClaim {
        claim: PlanClaim,
    },
    ReplaceClaim {
        original_id: Uuid,
        original_block_hash: String,
        claim: PlanClaim,
    },
    MarkConflict {
        related_id: Uuid,
        related_block_hash: String,
        claim: PlanClaim,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestPlan {
    pub operation_id: Uuid,
    pub baseline: PlanBaseline,
    pub evidence: Vec<PlanEvidence>,
    pub actions: Vec<IngestAction>,
    pub reason: String,
    pub input_complete: bool,
    pub output_complete: bool,
    pub model_revision: String,
    pub prompt_revision: String,
    pub pipeline_revision: String,
    /// Rust 从预览的完整 Markdown 计算；持久队列重试据此识别已落盘结果。
    pub result_hash: String,
}

// 单目标计划只承载一轮增量；大批量须拆成多个有独立基线的目标计划。
const MAX_PLAN_BYTES: usize = 1024 * 1024;
const MAX_PLAN_ACTIONS: usize = 24;
const MAX_PLAN_EVIDENCE: usize = 64;
const MAX_CITATIONS_PER_ACTION: usize = 8;
const MAX_CLAIM_CHARS: usize = 4096;
const PLAN_ARTIFACT_SCHEMA: u32 = 1;
// 列举某一主题的封存计划时最多看这么多份；正常情况下每个主题同时只有一两份。
const MAX_LISTED_PLAN_ARTIFACTS: usize = 256;

/// 生成计划时实际生效的依赖版本；任一变化都不能复用旧计划（TDD-06 §3.9.4）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRevisions {
    pub model_revision: String,
    pub prompt_revision: String,
    pub pipeline_revision: String,
}

/// 增量生成前从当前盘上主题取得的封闭上下文：基线凭据、完整范围与既有声明纯文本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicUpdateContext {
    pub baseline: PlanBaseline,
    pub scope: TopicScope,
    pub existing: Vec<crate::TopicClaimStatement>,
}

/// 已确认计划的设备本地载荷；队列续跑档只需持有 `operation_id`，不复制声明文本。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanArtifact {
    schema: u32,
    plan: IngestPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanApplyOutcome {
    AlreadyApplied {
        index_updated: bool,
        baseline_recorded: bool,
    },
    Applied {
        index_updated: bool,
        baseline_recorded: bool,
    },
}

/// 用户对同一目标、范围和来源版本的选择；模型换一种措辞也不得自动重放。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestDecisionKind {
    Ignored,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestDecision {
    pub target_path: String,
    pub scope_hash: String,
    pub source_versions: Vec<(String, String)>,
    pub kind: IngestDecisionKind,
    /// 只保存动作类型与声明数量的摘要，不复制模型声明或笔记正文。
    pub action_summary: Vec<(String, u32)>,
    /// 拒绝时那份动作的指纹仅供审计；匹配抑制不依赖它，避免同版本换措辞复活。
    pub action_digest: String,
}

impl IngestDecision {
    fn from_plan(plan: &IngestPlan, kind: IngestDecisionKind) -> Self {
        let source_versions = plan
            .evidence
            .iter()
            .map(|item| (item.source_path.clone(), item.source_hash.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut counts = std::collections::BTreeMap::new();
        for action in &plan.actions {
            let name = match action {
                IngestAction::AppendClaim { .. } => "append_claim",
                IngestAction::ReplaceClaim { .. } => "replace_claim",
                IngestAction::MarkConflict { .. } => "mark_conflict",
            };
            *counts.entry(name.to_owned()).or_insert(0) += 1;
        }
        Self {
            target_path: plan.baseline.target_path.clone(),
            scope_hash: plan.baseline.scope_hash.clone(),
            source_versions,
            kind,
            action_summary: counts.into_iter().collect(),
            action_digest: ContentHash::of(
                &serde_json::to_vec(&plan.actions).expect("IngestAction must serialize"),
            )
            .to_string(),
        }
    }

    /// 逐个来源版本抑制：计划只要用到任一被否决/忽略的来源版本就算命中。
    ///
    /// 不能要求整组来源完全相同——那样把被否决的来源和一篇新来源捆在一起，同一条
    /// 内容就会换个组合重新推荐（真机 2026-09-24 复现）。
    fn matches_plan(&self, plan: &IngestPlan) -> bool {
        self.target_path == plan.baseline.target_path
            && self.scope_hash == plan.baseline.scope_hash
            && plan.evidence.iter().any(|evidence| {
                self.source_versions.iter().any(|(path, hash)| {
                    *path == evidence.source_path && *hash == evidence.source_hash
                })
            })
    }

    /// 同一目标、范围与整组来源版本的重复决定（记录去重用，不用于抑制判定）。
    fn same_decision(&self, plan: &IngestPlan) -> bool {
        self.target_path == plan.baseline.target_path
            && self.scope_hash == plan.baseline.scope_hash
            && self.source_versions == Self::from_plan(plan, self.kind).source_versions
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanRejection {
    #[error("invalid plan structure")]
    InvalidPlan,
    #[error("model output was incomplete")]
    IncompleteOutput,
    #[error("plan exceeded the per-topic write budget")]
    BudgetExceeded,
    #[error("plan belongs to another vault")]
    WrongVault,
    #[error("topic scope changed")]
    ScopeChanged,
    #[error("topic file changed")]
    TargetChanged,
    #[error("topic is not ready for automatic maintenance: {0:?}")]
    TargetNotReady(TopicMaintenanceState),
    #[error("source changed: {0}")]
    SourceChanged(String),
    #[error("source disappeared: {0}")]
    SourceMissing(String),
    #[error("topic disappeared")]
    TargetMissing,
    #[error("original claim changed or disappeared: {0}")]
    OriginalClaimChanged(Uuid),
    #[error("vault read failed: {0}")]
    VaultRead(String),
    #[error("plan could not be rendered safely")]
    RenderFailed,
    #[error("topic write failed")]
    WriteFailed,
    #[error("user suppressed this source version for the topic")]
    UserSuppressed,
    #[error("durable decision store failed: {0}")]
    DecisionStore(String),
    #[error("knowledge commit lock failed: {0}")]
    CommitLock(String),
    #[error("sealed plan artifact failed: {0}")]
    PlanStore(String),
    /// 载荷不存在：从未封存、已应用后清理或已被用户处置；与存储故障分开，界面不误报磁盘问题。
    #[error("sealed plan artifact is missing")]
    PlanMissing,
}

/// 给 IPC 映射使用的封闭原因码；错误 Display 仅供诊断，前端不得解析它决定文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRejectionCode {
    InvalidPlan,
    IncompleteOutput,
    BudgetExceeded,
    WrongVault,
    ScopeChanged,
    TargetChanged,
    TargetNotReady,
    SourceChanged,
    SourceMissing,
    TargetMissing,
    OriginalClaimChanged,
    VaultReadFailed,
    RenderFailed,
    WriteFailed,
    UserSuppressed,
    DecisionStoreUnavailable,
    CommitLockUnavailable,
    PlanStoreUnavailable,
    PlanMissing,
}

impl PlanRejection {
    #[must_use]
    pub fn code(&self) -> PlanRejectionCode {
        match self {
            Self::InvalidPlan => PlanRejectionCode::InvalidPlan,
            Self::IncompleteOutput => PlanRejectionCode::IncompleteOutput,
            Self::BudgetExceeded => PlanRejectionCode::BudgetExceeded,
            Self::WrongVault => PlanRejectionCode::WrongVault,
            Self::ScopeChanged => PlanRejectionCode::ScopeChanged,
            Self::TargetChanged => PlanRejectionCode::TargetChanged,
            Self::TargetNotReady(_) => PlanRejectionCode::TargetNotReady,
            Self::SourceChanged(_) => PlanRejectionCode::SourceChanged,
            Self::SourceMissing(_) => PlanRejectionCode::SourceMissing,
            Self::TargetMissing => PlanRejectionCode::TargetMissing,
            Self::OriginalClaimChanged(_) => PlanRejectionCode::OriginalClaimChanged,
            Self::VaultRead(_) => PlanRejectionCode::VaultReadFailed,
            Self::RenderFailed => PlanRejectionCode::RenderFailed,
            Self::WriteFailed => PlanRejectionCode::WriteFailed,
            Self::UserSuppressed => PlanRejectionCode::UserSuppressed,
            Self::DecisionStore(_) => PlanRejectionCode::DecisionStoreUnavailable,
            Self::CommitLock(_) => PlanRejectionCode::CommitLockUnavailable,
            Self::PlanStore(_) => PlanRejectionCode::PlanStoreUnavailable,
            Self::PlanMissing => PlanRejectionCode::PlanMissing,
        }
    }

    /// 只携带受影响实体的路径，不回传底层 IO 诊断或模型正文。
    #[must_use]
    pub fn affected_source_path(&self) -> Option<&str> {
        match self {
            Self::SourceChanged(path) | Self::SourceMissing(path) => Some(path),
            _ => None,
        }
    }
}

impl Vault {
    /// 本库的设备侧身份（规范化根路径的指纹）：计划基线与设备私有的来源授权回执都绑定它，
    /// 同一路径换了库或库被搬走都不会沿用旧授权。
    #[must_use]
    pub fn knowledge_vault_identity(&self) -> String {
        vault_identity(self)
    }
}

fn vault_identity(vault: &Vault) -> String {
    let root = std::fs::canonicalize(vault.root()).unwrap_or_else(|_| vault.root().to_owned());
    ContentHash::of(root.to_string_lossy().as_bytes()).to_string()
}

fn scope_hash(scope: &TopicScope) -> String {
    // 仅含已知的 String/Vec 字段，JSON 序列化没有失败输入；字段序来自结构定义。
    let bytes = serde_json::to_vec(scope).expect("TopicScope must serialize to JSON");
    ContentHash::of(&bytes).to_string()
}

fn claim_block_hash(content: &str, id: Uuid) -> Option<String> {
    let start = format!("<!-- luna:claim:{id} -->");
    let mut block = String::new();
    let mut inside = false;
    for line in body_of(content).lines() {
        if line == start {
            inside = true;
        }
        if inside {
            block.push_str(line);
            block.push('\n');
        }
        if inside && line == "<!-- /luna:claim -->" {
            return Some(ContentHash::of(block.as_bytes()).to_string());
        }
    }
    None
}

/// 同一 Vault 的计划提交与用户决定共用锁，跨进程串行化检查、写入和恢复。
/// 锁文件放在不随设备同步且不被清缓存删除的 sidecars 目录；它必须稳定存在，
/// 删除并重建会让新旧进程锁住不同 inode。
pub(crate) fn knowledge_commit_lock(vault: &Vault) -> Result<File, PlanRejection> {
    let path = vault.root().join(".luna/sidecars/knowledge_commit.lock");
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| PlanRejection::CommitLock(error.to_string()))?;
    file.lock()
        .map_err(|error| PlanRejection::CommitLock(error.to_string()))?;
    Ok(file)
}

/// 已交给模型看过的来源版本（设备本地、可清理的模型计算缓存）：主题页 → 口径指纹（范围 +
/// 提示词版本 + 流水线版本）+ {来源路径 → 正文版本}。读不出、口径变了一律按「都没看过」——
/// 换了范围或提示词，对旧口径说过的「没有新增」不能沿用（TDD-06 §3.9.4）。
const PREPARED_STATE: &str = ".luna/sidecars/knowledge-prepared.json";

#[derive(Default, Serialize, Deserialize)]
struct ConsideredSources {
    scope: String,
    sources: std::collections::BTreeMap<String, ConsideredSource>,
    /// 模型对这一版来源的输出**不可用**过一次（路径 → 正文版本）：还不算看过，下一次触发
    /// 重试；同一版再不可用一次才记为看过，后台不再为它反复调模型（重试预算 = 1）。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    unusable: std::collections::BTreeMap<String, String>,
}

/// 一篇来源看过的那一版；`next_fragment` 有值 = 这一版只读了前半，下一轮从这里接着读。
#[derive(Clone, Serialize, Deserialize)]
struct ConsideredSource {
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_fragment: Option<u32>,
}

type ConsideredState = std::collections::BTreeMap<String, ConsideredSources>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Consideration {
    Unseen,
    Partly,
    Whole,
}

fn store_considered_state(vault: &Vault, state: &ConsideredState) -> Result<(), PlanRejection> {
    let path = vault.root().join(PREPARED_STATE);
    let bytes =
        serde_json::to_vec(state).map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
    std::fs::create_dir_all(path.parent().expect("prepared state has parent"))
        .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
    crate::atomic::write_atomic(&path, &bytes)
        .map_err(|error| PlanRejection::PlanStore(error.to_string()))
}

fn load_considered_state(vault: &Vault) -> ConsideredState {
    std::fs::read(vault.root().join(PREPARED_STATE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 「看过」的口径：这一页当前的范围与声明集合（结构），加上调用方给的提示词与流水线版本。
/// 「这一批没有新增」是相对当时已有的声明说的，页上声明变了（应用了计划、删了声明）就不能
/// 沿用。旧格式页或坏元数据的这一段为空串（无从谈起，看过的记录照样按页存）。
fn considered_basis(content: &str, revision: &str) -> String {
    let structure = crate::validate_topic_document(content)
        .map(|knowledge| {
            let claims: Vec<&str> = knowledge.claims.keys().map(String::as_str).collect();
            format!("{}\n{}", scope_hash(&knowledge.scope), claims.join(","))
        })
        .unwrap_or_default();
    ContentHash::of(format!("{structure}\n{revision}").as_bytes()).to_string()
}

fn plan_artifact_path(vault: &Vault, operation_id: Uuid) -> PathBuf {
    vault
        .root()
        .join(".luna/sidecars/knowledge-plans")
        .join(format!("{operation_id}.json"))
}

impl Vault {
    /// 封存计划以 operation ID 寻址。先验证同一预览，再原子落到设备本地目录；
    /// 同 ID 不允许改写成另一份模型结果，避免确认后重算时悄悄换稿。
    pub fn persist_sealed_ingest_plan(&self, plan: &IngestPlan) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        if vault_identity(self) != plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        if plan.result_hash.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        let artifact = PlanArtifact {
            schema: PLAN_ARTIFACT_SCHEMA,
            plan: plan.clone(),
        };
        let bytes = serde_json::to_vec(&artifact)
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        if bytes.len() > MAX_PLAN_BYTES + 256 {
            return Err(PlanRejection::BudgetExceeded);
        }
        let path = plan_artifact_path(self, plan.operation_id);
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(()),
            Ok(_) => return Err(PlanRejection::InvalidPlan),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(PlanRejection::PlanStore(error.to_string())),
        }
        let preview = self.preview_ingest_plan(plan)?;
        if ContentHash::of(preview.as_bytes()).as_str() != plan.result_hash {
            return Err(PlanRejection::InvalidPlan);
        }
        std::fs::create_dir_all(path.parent().expect("plan artifact has parent"))
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        crate::atomic::write_atomic(&path, &bytes)
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))
    }

    /// 某一主题仍在盘上的封存计划 ID，按生成先后（UUID v7 时间序）从新到旧。
    ///
    /// 界面离开再回来时据此恢复同一份计划，不重跑模型；读不出或不属于本库的载荷跳过
    /// （它们的去留由处置与终态清理负责，不在列举时改动）。最多扫描
    /// `MAX_LISTED_PLAN_ARTIFACTS` 份，防止异常堆积拖慢主题页打开。
    pub fn sealed_plan_ids_for_target(&self, target_path: &str) -> Vec<Uuid> {
        let dir = self.root().join(".luna/sidecars/knowledge-plans");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut ids: Vec<Uuid> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                Uuid::parse_str(name.strip_suffix(".json")?).ok()
            })
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.truncate(MAX_LISTED_PLAN_ARTIFACTS);
        ids.into_iter()
            .filter(|id| {
                self.load_sealed_ingest_plan(*id)
                    .is_ok_and(|plan| plan.baseline.target_path == target_path)
            })
            .collect()
    }

    /// 续跑时只按 operation ID 取回原计划；目标和来源可能已过期，应用边界另行复核。
    pub fn load_sealed_ingest_plan(&self, operation_id: Uuid) -> Result<IngestPlan, PlanRejection> {
        let path = plan_artifact_path(self, operation_id);
        let bytes = std::fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PlanRejection::PlanMissing
            } else {
                PlanRejection::PlanStore(error.to_string())
            }
        })?;
        if bytes.len() > MAX_PLAN_BYTES + 256 {
            return Err(PlanRejection::BudgetExceeded);
        }
        let artifact: PlanArtifact = serde_json::from_slice(&bytes)
            .map_err(|error| PlanRejection::PlanStore(error.to_string()))?;
        if artifact.schema != PLAN_ARTIFACT_SCHEMA {
            return Err(PlanRejection::PlanStore(
                "unknown plan artifact schema".into(),
            ));
        }
        if artifact.plan.operation_id != operation_id || artifact.plan.result_hash.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        if vault_identity(self) != artifact.plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        Ok(artifact.plan)
    }

    /// 终态清理：只删设备本地载荷，不碰主题、快照或随库决定；已不存在视为完成。
    /// 调用方须先让队列续跑档不再引用该 ID，否则崩溃后续跑会读到缺失载荷。
    pub fn discard_sealed_ingest_plan(&self, operation_id: Uuid) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        match std::fs::remove_file(plan_artifact_path(self, operation_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlanRejection::PlanStore(error.to_string())),
        }
    }

    /// 逐版本用户决定独立于派生索引，删除 `index.sqlite` 后仍有效。
    pub fn ingest_decisions(&self) -> crate::Result<Vec<IngestDecision>> {
        crate::meta::read_meta(self.root(), "knowledge_ingest_decisions")
    }

    /// 仅对已封存、仍可预览的计划记录用户决定；同版本重试不覆盖已有选择。
    pub fn record_ingest_decision(
        &self,
        plan: &IngestPlan,
        kind: IngestDecisionKind,
    ) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        if vault_identity(self) != plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        if plan.result_hash.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        let mut decisions = self
            .ingest_decisions()
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))?;
        if decisions
            .iter()
            .any(|decision| decision.same_decision(plan))
        {
            return Ok(());
        }
        let preview = self.preview_ingest_plan(plan)?;
        if ContentHash::of(preview.as_bytes()).as_str() != plan.result_hash {
            return Err(PlanRejection::InvalidPlan);
        }
        decisions.push(IngestDecision::from_plan(plan, kind));
        crate::meta::write_meta(self.root(), "knowledge_ingest_decisions", &decisions)
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))
    }

    /// 对某一目标与范围已被用户忽略/否决的来源版本；生成前据此剔除证据，
    /// 模型不会再看到被否决的内容，也不白跑一次必然被拒的生成。
    pub fn suppressed_source_versions(
        &self,
        target_path: &str,
        scope_hash_value: &str,
    ) -> Result<BTreeSet<(String, String)>, PlanRejection> {
        Ok(self
            .ingest_decisions()
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))?
            .into_iter()
            .filter(|decision| {
                decision.target_path == target_path && decision.scope_hash == scope_hash_value
            })
            .flat_map(|decision| decision.source_versions)
            .collect())
    }

    /// 主题的候选来源：共享标签且尚未被本页引用（与目录计数同一底层查询），再剔除
    /// **当前版本**已被用户忽略/否决的来源。目录「候选 ≥N」与「检查可添加内容」
    /// 都只调这一个函数，否则目录会把点下去必然被拦的来源也算进去（真机 2026-09-24）。
    /// 旧格式页没有范围，也就不可能有决定，原样返回。
    ///
    /// `policy` 允许时一并纳入已完成权威转写的录音/转写（PRD-16-047）；随后剔除**派生重复**：
    /// 某篇蒸馏笔记的源资料已在候选里或已被本页引用时，那篇蒸馏笔记不再作为独立来源——
    /// 针对来源写出的摘要不能与来源本身各算一份证据（PRD-16-045/046）。
    pub fn topic_candidate_sources_of(
        &self,
        topic_rel: &str,
        content: &str,
        policy: crate::SourcePolicy,
    ) -> Result<Vec<String>, PlanRejection> {
        let pending = self.topic_candidate_pool_of(topic_rel, content, policy)?;
        let Ok(knowledge) = crate::validate_topic_document(content) else {
            return Ok(pending);
        };
        let suppressed =
            self.suppressed_source_versions(topic_rel, &scope_hash(&knowledge.scope))?;
        if suppressed.is_empty() {
            return Ok(pending);
        }
        Ok(pending
            .into_iter()
            .filter(|path| match self.body_fingerprint(path) {
                Some(hash) => !suppressed.contains(&(path.clone(), hash)),
                None => true,
            })
            .collect())
    }

    /// 去重后、尚未剔除用户否决的候选池。「没有候选」与「候选都被否决过」是两句不同的话，
    /// 生成入口据此分辨；目录与生成真正使用的仍是 [`Self::topic_candidate_sources_of`]。
    pub fn topic_candidate_pool_of(
        &self,
        topic_rel: &str,
        content: &str,
        policy: crate::SourcePolicy,
    ) -> Result<Vec<String>, PlanRejection> {
        let read_error = |error: VaultError| PlanRejection::VaultRead(error.to_string());
        let mut pending = self
            .topic_ingest_pending_of(topic_rel, content)
            .map_err(read_error)?;
        // 自动找来的候选不含排除路径（V4a；共享标签那一路在 `topic_ingest_pending_of` 里滤）。
        let excluded = self.exclusions();
        let found = |paths: Vec<String>| -> Vec<String> {
            paths
                .into_iter()
                .filter(|path| !excluded.covers(path))
                .collect()
        };
        if policy.transcripts {
            pending.extend(found(
                self.topic_transcript_candidates_of(topic_rel, content)
                    .map_err(read_error)?,
            ));
        }
        if policy.quick_notes {
            pending.extend(found(
                self.topic_quick_note_candidates_of(topic_rel, content)
                    .map_err(read_error)?,
            ));
        }
        pending.sort();
        pending.dedup();
        Ok(self.without_derived_duplicates(content, pending))
    }

    /// 这一页在当前范围下已交给模型看过的来源版本（路径 → 版本与断点）。
    fn considered_sources(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
    ) -> std::collections::BTreeMap<String, ConsideredSource> {
        let scope = considered_basis(content, revision);
        load_considered_state(self)
            .remove(topic_rel)
            .filter(|entry| entry.scope == scope)
            .map(|entry| entry.sources)
            .unwrap_or_default()
    }

    /// 候选里还有没交给模型看过的版本吗（后台准备据此决定要不要调模型，TDD-06 §3.9.3）。
    /// 看过的来源改了正文就又算没看过；看过且没变的整组再触发也不调模型。
    #[must_use]
    pub fn has_unconsidered_sources(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
        paths: &[String],
    ) -> bool {
        let considered = self.considered_sources(topic_rel, content, revision);
        paths
            .iter()
            .any(|path| self.consideration_of(&considered, path) != Consideration::Whole)
    }

    /// 一篇来源在「看过」记录里的状态：当前正文这一版看完了、只看了前半、还是没看过。
    fn consideration_of(
        &self,
        considered: &std::collections::BTreeMap<String, ConsideredSource>,
        path: &str,
    ) -> Consideration {
        match (self.body_fingerprint(path), considered.get(path)) {
            (Some(version), Some(seen)) if seen.version == version => {
                if seen.next_fragment.is_some() {
                    Consideration::Partly
                } else {
                    Consideration::Whole
                }
            }
            _ => Consideration::Unseen,
        }
    }

    /// 上一轮只读了前半的来源从哪里接着读：路径 →（那一版的正文版本，下一个片段序号）。
    /// 交给 [`Self::topic_evidence_resuming`]；来源之后改了，版本对不上，自然从头读。
    #[must_use]
    pub fn resume_points(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
    ) -> std::collections::BTreeMap<String, (String, u32)> {
        self.considered_sources(topic_rel, content, revision)
            .into_iter()
            .filter_map(|(path, seen)| Some((path, (seen.version, seen.next_fragment?))))
            .collect()
    }

    /// 把没看过的来源排到前面，其次是只读了前半的，看完的最后（各组内保持原序）。一次请求
    /// 只容得下有界的一批：否则前一批「没有新增」的来源永远占着队头，后面的来源永远轮不到；
    /// 读了前半的排在没看过的之后，一篇超长来源也就不会一轮轮霸占整批。
    #[must_use]
    pub fn unconsidered_sources_first(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
        paths: Vec<String>,
    ) -> Vec<String> {
        let considered = self.considered_sources(topic_rel, content, revision);
        let mut groups: [Vec<String>; 3] = Default::default();
        for path in paths {
            let group = match self.consideration_of(&considered, &path) {
                Consideration::Unseen => 0,
                Consideration::Partly => 1,
                Consideration::Whole => 2,
            };
            groups[group].push(path);
        }
        groups.into_iter().flatten().collect()
    }

    /// 模型对这一批的输出不可用（无效或被截断）：第一次不记看过——输出有随机性，下次触发
    /// 重试，续读断点也不前移；同一版来源第二次不可用才按 [`Self::record_considered_sources`]
    /// 记为看过，免得后台对着同一批内容一次次白调模型。手动检查不受影响，随时可重跑。
    /// 返回这一次是否已记为看过。
    pub fn record_unusable_output(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
        seen: &[crate::SourceCoverage],
        candidates: &[String],
    ) -> Result<bool, PlanRejection> {
        let exhausted = {
            let _commit_lock = knowledge_commit_lock(self)?;
            let scope = considered_basis(content, revision);
            let mut state = load_considered_state(self);
            let entry = state.entry(topic_rel.to_owned()).or_default();
            if entry.scope != scope {
                *entry = ConsideredSources {
                    scope,
                    ..ConsideredSources::default()
                };
            }
            let exhausted = !seen.is_empty()
                && seen
                    .iter()
                    .all(|coverage| entry.unusable.get(&coverage.path) == Some(&coverage.version));
            if !exhausted {
                for coverage in seen {
                    entry
                        .unusable
                        .insert(coverage.path.clone(), coverage.version.clone());
                }
                entry
                    .unusable
                    .retain(|path, _| candidates.iter().any(|candidate| candidate == path));
                store_considered_state(self, &state)?;
            }
            exhausted
        };
        if exhausted {
            self.record_considered_sources(topic_rel, content, revision, seen, candidates)?;
        }
        Ok(exhausted)
    }

    /// 记下这一批已交给模型看过（计划、无新增，或同一版第二次输出不可用），连同每篇读到
    /// 哪里；只保留仍是候选的来源，记录不随历史无界增长。
    pub fn record_considered_sources(
        &self,
        topic_rel: &str,
        content: &str,
        revision: &str,
        seen: &[crate::SourceCoverage],
        candidates: &[String],
    ) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        let scope = considered_basis(content, revision);
        let mut state = load_considered_state(self);
        let entry = state.entry(topic_rel.to_owned()).or_default();
        if entry.scope != scope {
            *entry = ConsideredSources {
                scope,
                ..ConsideredSources::default()
            };
        }
        entry
            .sources
            .retain(|path, _| candidates.iter().any(|candidate| candidate == path));
        for coverage in seen {
            entry.sources.insert(
                coverage.path.clone(),
                ConsideredSource {
                    version: coverage.version.clone(),
                    next_fragment: coverage.next_fragment,
                },
            );
            // 看过了：之前不可用的那一次不再计数。
            entry.unusable.remove(&coverage.path);
        }
        store_considered_state(self, &state)
    }

    /// 把 `source_rel` 当作候选来源的新格式主题页（后台准备建议的触发范围）。旧格式页走旧归入，
    /// 不在此列；读不出的页跳过。
    #[must_use]
    pub fn topics_with_candidate(
        &self,
        source_rel: &str,
        policy: crate::SourcePolicy,
    ) -> Vec<String> {
        let Ok(pages) = self.topic_pages() else {
            return Vec::new();
        };
        pages
            .into_iter()
            .filter(|topic| {
                let Ok(doc) = self.read(topic) else {
                    return false;
                };
                crate::validate_topic_document(&doc.content).is_ok()
                    && self
                        .topic_candidate_sources_of(topic, &doc.content, policy)
                        .is_ok_and(|sources| sources.iter().any(|path| path == source_rel))
            })
            .collect()
    }

    /// 新建主题的候选（还没有页面、没有被引用的来源）剔除派生重复，判据与已有页同一份。
    #[must_use]
    pub fn drop_derived_duplicates(&self, paths: Vec<String>) -> Vec<String> {
        self.without_derived_duplicates("", paths)
    }

    fn without_derived_duplicates(&self, content: &str, paths: Vec<String>) -> Vec<String> {
        let cited = crate::cited_targets(crate::body_of(content));
        let present: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let keep: Vec<bool> = paths
            .iter()
            .map(
                |path| match self.read(path).ok().and_then(|doc| doc.meta.distill_source) {
                    Some(source) => {
                        !(present.contains(source.as_str()) || crate::cites_any_of(&cited, &source))
                    }
                    None => true,
                },
            )
            .collect();
        paths
            .into_iter()
            .zip(keep)
            .filter_map(|(path, keep)| keep.then_some(path))
            .collect()
    }

    /// 明确恢复某一来源/范围版本后，该版本才可再次预览并应用。
    pub fn restore_ingest_decision(&self, plan: &IngestPlan) -> Result<(), PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        if vault_identity(self) != plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        let mut decisions = self
            .ingest_decisions()
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))?;
        let before = decisions.len();
        decisions.retain(|decision| !decision.matches_plan(plan));
        if decisions.len() == before {
            return Ok(());
        }
        crate::meta::write_meta(self.root(), "knowledge_ingest_decisions", &decisions)
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))
    }

    /// 只从当前盘上文件生成计划基线；旧主题或待复核页不给预览凭据。
    pub fn topic_plan_baseline(&self, target_path: &str) -> crate::Result<PlanBaseline> {
        let state = self.topic_maintenance_state(target_path)?;
        if state != TopicMaintenanceState::Ready {
            return Err(VaultError::InvalidReplace(format!(
                "topic is not ready for planning: {state:?}"
            )));
        }
        let target = self.read(target_path)?;
        let knowledge = crate::validate_topic_document(&target.content)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        Ok(PlanBaseline {
            vault_identity: vault_identity(self),
            target_path: target_path.to_owned(),
            target_hash: ContentHash::of(target.content.as_bytes()).to_string(),
            scope_hash: scope_hash(&knowledge.scope),
        })
    }

    /// 只对维护就绪的新格式主题给增量上下文；旧格式、人工改动、缺底版都在调用模型前拒绝。
    pub fn topic_update_context(
        &self,
        target_path: &str,
    ) -> Result<TopicUpdateContext, PlanRejection> {
        let state = match self.topic_maintenance_state(target_path) {
            Ok(state) => state,
            Err(VaultError::NotFound(_)) => return Err(PlanRejection::TargetMissing),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        if state != TopicMaintenanceState::Ready {
            return Err(PlanRejection::TargetNotReady(state));
        }
        let target = match self.read(target_path) {
            Ok(target) => target,
            Err(VaultError::NotFound(_)) => return Err(PlanRejection::TargetMissing),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        let knowledge = crate::validate_topic_document(&target.content)
            .map_err(|_| PlanRejection::TargetNotReady(TopicMaintenanceState::InvalidEvidence))?;
        let existing = crate::topic_claim_statements(&target.content)
            .map_err(|_| PlanRejection::TargetNotReady(TopicMaintenanceState::InvalidEvidence))?;
        Ok(TopicUpdateContext {
            baseline: PlanBaseline {
                vault_identity: vault_identity(self),
                target_path: target_path.to_owned(),
                target_hash: ContentHash::of(target.content.as_bytes()).to_string(),
                scope_hash: scope_hash(&knowledge.scope),
            },
            scope: knowledge.scope,
            existing,
        })
    }

    /// 把一次已校验的增量结果封存成同一份可预览、可确认的计划。只收录被引用的证据，
    /// 分歧指向的既有块按生成前的基线正文取 hash；目标在模型等待期间变了即拒绝。
    pub fn seal_topic_update_plan(
        &self,
        baseline: PlanBaseline,
        evidence: &[crate::TopicEvidenceInput],
        input_complete: bool,
        claims: &[crate::GroundedUpdate],
        revisions: PlanRevisions,
    ) -> Result<IngestPlan, PlanRejection> {
        if claims.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        let target = match self.read(&baseline.target_path) {
            Ok(target) => target,
            Err(VaultError::NotFound(_)) => return Err(PlanRejection::TargetMissing),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        if ContentHash::of(target.content.as_bytes()).as_str() != baseline.target_hash {
            return Err(PlanRejection::TargetChanged);
        }
        let mut remap = std::collections::BTreeMap::new();
        let mut plan_evidence = Vec::new();
        let mut actions = Vec::with_capacity(claims.len());
        for update in claims {
            let mut evidence_indices = Vec::with_capacity(update.claim.evidence_indices.len());
            for index in &update.claim.evidence_indices {
                let item = evidence.get(*index).ok_or(PlanRejection::InvalidPlan)?;
                let mapped = *remap.entry(*index).or_insert_with(|| {
                    plan_evidence.push(item.source.clone());
                    plan_evidence.len() - 1
                });
                evidence_indices.push(mapped);
            }
            let claim = PlanClaim {
                id: update.claim.id,
                text: update.claim.text.clone(),
                kind: update.claim.kind,
                evidence_indices,
            };
            actions.push(match (update.conflicts_with, update.replaces) {
                (Some(related_id), _) => IngestAction::MarkConflict {
                    related_id,
                    related_block_hash: claim_block_hash(&target.content, related_id)
                        .ok_or(PlanRejection::OriginalClaimChanged(related_id))?,
                    claim,
                },
                // 更新版沿用原声明 ID、只换那一块；块指纹按生成前的底版取，等待期间被人改过即拒绝。
                (None, Some(original_id)) => IngestAction::ReplaceClaim {
                    original_id,
                    original_block_hash: claim_block_hash(&target.content, original_id)
                        .ok_or(PlanRejection::OriginalClaimChanged(original_id))?,
                    claim,
                },
                (None, None) => IngestAction::AppendClaim { claim },
            });
        }
        self.seal_ingest_plan(IngestPlan {
            operation_id: Uuid::now_v7(),
            baseline,
            evidence: plan_evidence,
            actions,
            reason: "topic_update".to_owned(),
            input_complete,
            output_complete: true,
            model_revision: revisions.model_revision,
            prompt_revision: revisions.prompt_revision,
            pipeline_revision: revisions.pipeline_revision,
            result_hash: String::new(),
        })
    }

    /// 同一不可变计划的预览/应用前检查。此处不写文件，不把过期计划重新解释为 no-op。
    pub fn check_ingest_plan(&self, plan: &IngestPlan) -> Result<(), PlanRejection> {
        if plan.actions.len() > MAX_PLAN_ACTIONS
            || plan.evidence.len() > MAX_PLAN_EVIDENCE
            || serde_json::to_vec(plan)
                .map_err(|_| PlanRejection::InvalidPlan)?
                .len()
                > MAX_PLAN_BYTES
        {
            return Err(PlanRejection::BudgetExceeded);
        }
        if plan.operation_id.is_nil()
            || plan.actions.is_empty()
            || plan.evidence.is_empty()
            || plan.reason.trim().is_empty()
            || plan.model_revision.trim().is_empty()
            || plan.prompt_revision.trim().is_empty()
            || plan.pipeline_revision.trim().is_empty()
            || plan.baseline.target_path.trim().is_empty()
        {
            return Err(PlanRejection::InvalidPlan);
        }
        if !plan.output_complete {
            return Err(PlanRejection::IncompleteOutput);
        }
        if plan.input_complete && plan.evidence.iter().any(|item| !item.complete_input) {
            return Err(PlanRejection::InvalidPlan);
        }
        let mut ids = BTreeSet::new();
        for action in &plan.actions {
            let claim = match action {
                IngestAction::AppendClaim { claim }
                | IngestAction::ReplaceClaim { claim, .. }
                | IngestAction::MarkConflict { claim, .. } => claim,
            };
            if claim.text.chars().count() > MAX_CLAIM_CHARS
                || claim.evidence_indices.len() > MAX_CITATIONS_PER_ACTION
            {
                return Err(PlanRejection::BudgetExceeded);
            }
            if claim.id.is_nil()
                || !ids.insert(claim.id)
                || claim.text.trim().is_empty()
                || claim.text.chars().any(char::is_control)
                || claim.evidence_indices.is_empty()
                || claim
                    .evidence_indices
                    .iter()
                    .any(|index| *index >= plan.evidence.len())
                || claim
                    .evidence_indices
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != claim.evidence_indices.len()
            {
                return Err(PlanRejection::InvalidPlan);
            }
            if matches!(action, IngestAction::MarkConflict { .. })
                != (claim.kind == ClaimKind::Conflict)
            {
                return Err(PlanRejection::InvalidPlan);
            }
        }
        if plan.evidence.iter().any(|item| {
            item.source_path.trim().is_empty()
                || item.source_title.trim().is_empty()
                || item.source_hash.trim().is_empty()
                || !item.locator.valid()
        }) {
            return Err(PlanRejection::InvalidPlan);
        }
        if vault_identity(self) != plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        let target = match self.read(&plan.baseline.target_path) {
            Ok(target) => target,
            Err(VaultError::NotFound(_)) => return Err(PlanRejection::TargetMissing),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        let knowledge = crate::validate_topic_document(&target.content).map_err(|error| {
            PlanRejection::TargetNotReady(match error {
                crate::KnowledgeError::NeedsUpgrade => TopicMaintenanceState::NeedsUpgrade,
                crate::KnowledgeError::UnknownSchema(_) => TopicMaintenanceState::UnknownSchema,
                crate::KnowledgeError::NotTopic => TopicMaintenanceState::NotTopic,
                _ => TopicMaintenanceState::InvalidEvidence,
            })
        })?;
        if scope_hash(&knowledge.scope) != plan.baseline.scope_hash {
            return Err(PlanRejection::ScopeChanged);
        }
        if ContentHash::of(target.content.as_bytes()).as_str() != plan.baseline.target_hash {
            return Err(PlanRejection::TargetChanged);
        }
        let state = self
            .topic_maintenance_state(&plan.baseline.target_path)
            .map_err(|error| PlanRejection::VaultRead(error.to_string()))?;
        if state != TopicMaintenanceState::Ready {
            return Err(PlanRejection::TargetNotReady(state));
        }
        for action in &plan.actions {
            let proposed = match action {
                IngestAction::AppendClaim { claim }
                | IngestAction::ReplaceClaim { claim, .. }
                | IngestAction::MarkConflict { claim, .. } => claim,
            };
            let existing = knowledge.claims.contains_key(&proposed.id.to_string());
            match action {
                IngestAction::ReplaceClaim { original_id, .. }
                    if *original_id == proposed.id && existing => {}
                IngestAction::ReplaceClaim { .. } => return Err(PlanRejection::InvalidPlan),
                _ if existing => return Err(PlanRejection::InvalidPlan),
                _ => {}
            }
            let (original_id, original_hash) = match action {
                IngestAction::AppendClaim { .. } => continue,
                IngestAction::ReplaceClaim {
                    original_id,
                    original_block_hash,
                    ..
                } => (original_id, original_block_hash),
                IngestAction::MarkConflict {
                    related_id,
                    related_block_hash,
                    ..
                } => (related_id, related_block_hash),
            };
            if claim_block_hash(&target.content, *original_id).as_deref()
                != Some(original_hash.as_str())
            {
                return Err(PlanRejection::OriginalClaimChanged(*original_id));
            }
        }
        for evidence in &plan.evidence {
            let source = match self.read(&evidence.source_path) {
                Ok(source) => source,
                Err(VaultError::NotFound(_)) => {
                    return Err(PlanRejection::SourceMissing(evidence.source_path.clone()));
                }
                Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
            };
            let current = ContentHash::of(body_of(&source.content).as_bytes());
            if current.as_str() != evidence.source_hash {
                return Err(PlanRejection::SourceChanged(evidence.source_path.clone()));
            }
            if !crate::knowledge_evidence::citation_quote_is_unique_or_hinted(
                &source.content,
                &evidence.locator,
            ) {
                return Err(PlanRejection::InvalidPlan);
            }
        }
        if self
            .ingest_decisions()
            .map_err(|error| PlanRejection::DecisionStore(error.to_string()))?
            .iter()
            .any(|decision| decision.matches_plan(plan))
        {
            return Err(PlanRejection::UserSuppressed);
        }
        Ok(())
    }

    /// 对同一份不可变计划生成预览，不写盘；计划依赖变化时返回过期原因。
    pub fn preview_ingest_plan(&self, plan: &IngestPlan) -> Result<String, PlanRejection> {
        self.check_ingest_plan(plan)?;
        let target = self
            .read(&plan.baseline.target_path)
            .map_err(|error| PlanRejection::VaultRead(error.to_string()))?;
        let mut meta = crate::frontmatter::read_luna_checked(&target.content)
            .map_err(|_| PlanRejection::RenderFailed)?;
        let mut knowledge = crate::validate_topic_document(&target.content)
            .map_err(|_| PlanRejection::RenderFailed)?;
        let original_body = body_of(&target.content);
        let mut body = original_body.to_owned();
        for action in &plan.actions {
            let claim = match action {
                IngestAction::AppendClaim { claim }
                | IngestAction::ReplaceClaim { claim, .. }
                | IngestAction::MarkConflict { claim, .. } => claim,
            };
            let sources: Vec<ClaimSource<'_>> = claim
                .evidence_indices
                .iter()
                .map(|index| {
                    let evidence = &plan.evidence[*index];
                    ClaimSource {
                        path: &evidence.source_path,
                        title: &evidence.source_title,
                        source_hash: &evidence.source_hash,
                        locator: evidence.locator.clone(),
                        complete_input: evidence.complete_input,
                    }
                })
                .collect();
            let (block, mut claim_meta, citations) = render_generated_claim(
                claim.id,
                &claim.text,
                claim.kind,
                &plan.baseline.target_path,
                &sources,
            )
            .map_err(|_| PlanRejection::RenderFailed)?;
            if let IngestAction::MarkConflict { related_id, .. } = action {
                claim_meta.conflicts_with = Some(related_id.to_string());
            }
            match action {
                IngestAction::AppendClaim { .. } | IngestAction::MarkConflict { .. } => {
                    if !body.ends_with('\n') {
                        body.push('\n');
                    }
                    if !body.ends_with("\n\n") {
                        body.push('\n');
                    }
                    body.push_str(&block);
                    body.push('\n');
                }
                IngestAction::ReplaceClaim { original_id, .. } => {
                    body = replace_claim_block(&body, *original_id, &block)
                        .ok_or(PlanRejection::RenderFailed)?;
                    let prefix = format!("{original_id}:");
                    knowledge
                        .citations
                        .retain(|key, _| !key.starts_with(&prefix));
                }
            }
            knowledge.claims.insert(claim.id.to_string(), claim_meta);
            knowledge.citations.extend(citations);
        }
        let prefix = &target.content[..target.content.len() - original_body.len()];
        let changed = format!("{prefix}{body}");
        meta.knowledge =
            Some(serde_yaml_ng::to_value(&knowledge).map_err(|_| PlanRejection::RenderFailed)?);
        let staged =
            crate::update_luna(&changed, &meta).map_err(|_| PlanRejection::RenderFailed)?;
        meta.synth_body = Some(ContentHash::of(body_of(&staged).as_bytes()).to_string());
        let preview =
            crate::update_luna(&staged, &meta).map_err(|_| PlanRejection::RenderFailed)?;
        crate::validate_topic_document(&preview).map_err(|_| PlanRejection::RenderFailed)?;
        if !plan.result_hash.is_empty()
            && ContentHash::of(preview.as_bytes()).as_str() != plan.result_hash
        {
            return Err(PlanRejection::InvalidPlan);
        }
        Ok(preview)
    }

    /// 只由 Rust 封存预览结果；模型输出不能指定应用结果的文件指纹。
    pub fn seal_ingest_plan(&self, mut plan: IngestPlan) -> Result<IngestPlan, PlanRejection> {
        if !plan.result_hash.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        let preview = self.preview_ingest_plan(&plan)?;
        plan.result_hash = ContentHash::of(preview.as_bytes()).to_string();
        Ok(plan)
    }

    /// 用同一份封存计划应用；重试先识别已落盘结果，避免重复追加声明。
    /// 同一应用的不同进程共享提交锁；外部编辑器不参与此锁，最终读/写间的外部并发
    /// 窗口仍由 K2/K3 的执行协调收口，此处不宣称跨进程事务。
    pub fn apply_ingest_plan(&self, plan: &IngestPlan) -> Result<PlanApplyOutcome, PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        if plan.result_hash.is_empty() {
            return Err(PlanRejection::InvalidPlan);
        }
        if vault_identity(self) != plan.baseline.vault_identity {
            return Err(PlanRejection::WrongVault);
        }
        let current = match self.read(&plan.baseline.target_path) {
            Ok(current) => current,
            Err(VaultError::NotFound(_)) => return Err(PlanRejection::TargetMissing),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        if ContentHash::of(current.content.as_bytes()).as_str() == plan.result_hash {
            let (index_updated, baseline_recorded) =
                self.recover_committed_topic(&plan.baseline.target_path, &current.content);
            return Ok(PlanApplyOutcome::AlreadyApplied {
                index_updated,
                baseline_recorded,
            });
        }
        let preview = self.preview_ingest_plan(plan)?;
        // 模型等待后与用户确认后都可能改稿；把检查放到 write 的紧前面，
        // 写入本身再按同一次读取的整文件 hash 条件覆盖。
        self.check_ingest_plan(plan)?;
        let write_result = match self.write_if_current(
            &plan.baseline.target_path,
            &plan.baseline.target_hash,
            &preview,
        ) {
            Ok(false) => return Err(PlanRejection::TargetChanged),
            Ok(true) => Ok(()),
            Err(error) => Err(error),
        };
        let committed = self
            .read(&plan.baseline.target_path)
            .is_ok_and(|doc| ContentHash::of(doc.content.as_bytes()).as_str() == plan.result_hash);
        if !committed {
            return Err(PlanRejection::WriteFailed);
        }
        let baseline_recorded = self.record_topic_baseline(&plan.baseline.target_path, &preview);
        Ok(PlanApplyOutcome::Applied {
            index_updated: write_result.is_ok(),
            baseline_recorded,
        })
    }
}

/// 撤销一次计划写入的结局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanUndoOutcome {
    /// 已恢复成写入前的那一版。
    Restored,
    /// 写入之后这一页又被改过（或已不在）：撤销会覆盖之后的改动，不做。
    Changed,
    /// 找不到写入前的那一版（版本链被清理）。
    Unavailable,
}

impl Vault {
    /// 撤销一次计划写入（K4「撤销恢复精确写前版本，后续有修改则先说明冲突」）：只有这一页
    /// 仍**逐字节**是写入后的那一版时，才用版本链里写入前那一版条件覆盖回去，并重新登记
    /// 系统底版；之后有任何修改就返回 `Changed`、一个字节不动。
    pub fn undo_applied_plan(
        &self,
        target: &str,
        before_hash: &str,
        after_hash: &str,
    ) -> Result<PlanUndoOutcome, PlanRejection> {
        let _commit_lock = knowledge_commit_lock(self)?;
        let current = match self.read(target) {
            Ok(current) => current,
            Err(VaultError::NotFound(_)) => return Ok(PlanUndoOutcome::Changed),
            Err(error) => return Err(PlanRejection::VaultRead(error.to_string())),
        };
        if ContentHash::of(current.content.as_bytes()).as_str() != after_hash {
            return Ok(PlanUndoOutcome::Changed);
        }
        let Some(before) = self.snapshot_text_with_hash(target, before_hash) else {
            return Ok(PlanUndoOutcome::Unavailable);
        };
        match self.write_if_current(target, after_hash, &before) {
            Ok(true) => {
                let _ = self.record_topic_baseline(target, &before);
                Ok(PlanUndoOutcome::Restored)
            }
            Ok(false) => Ok(PlanUndoOutcome::Changed),
            Err(_) => Err(PlanRejection::WriteFailed),
        }
    }
}

fn replace_claim_block(body: &str, id: Uuid, replacement: &str) -> Option<String> {
    let start_marker = format!("<!-- luna:claim:{id} -->");
    let end_marker = "<!-- /luna:claim -->";
    let mut offset = 0;
    let mut start = None;
    for segment in body.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if line == start_marker {
            start = Some(offset);
        } else if line == end_marker {
            if let Some(start) = start {
                let end = offset + line.len();
                return Some(format!("{}{}{}", &body[..start], replacement, &body[end..]));
            }
        }
        offset += segment.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_evidence::{ClaimSource, NewClaim};

    const SOURCE: &str = "Notes/会议.md";
    const TARGET: &str = "Knowledge/项目.md";
    const SOURCE_BODY: &str = "# 会议\n\n原型已完成。\n";

    fn locator() -> EvidenceLocator {
        EvidenceLocator {
            heading_path: vec!["会议".into()],
            quote: "原型已完成".into(),
            context: String::new(),
            line_hint: Some(3),
            time_start_ms: None,
            time_end_ms: None,
        }
    }

    fn fixture() -> (tempfile::TempDir, Vault, IngestPlan, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write(SOURCE, SOURCE_BODY).unwrap();
        let source_hash = vault.body_fingerprint(SOURCE).unwrap();
        let source = [ClaimSource {
            path: SOURCE,
            title: "会议",
            source_hash: &source_hash,
            locator: locator(),
            complete_input: true,
        }];
        let existing_id = Uuid::now_v7();
        let existing = NewClaim {
            id: existing_id,
            text: "原型已完成",
            kind: ClaimKind::Synthesis,
            sources: &source,
        };
        let content = vault
            .scoped_topic_content(
                TARGET,
                Some("项目"),
                TopicScope::from_tag("项目"),
                &[existing],
            )
            .unwrap();
        vault.publish_scoped_topic(TARGET, &content).unwrap();
        let plan = IngestPlan {
            operation_id: Uuid::now_v7(),
            baseline: vault.topic_plan_baseline(TARGET).unwrap(),
            evidence: vec![PlanEvidence {
                source_path: SOURCE.into(),
                source_title: "会议".into(),
                source_hash,
                locator: locator(),
                complete_input: true,
            }],
            actions: vec![IngestAction::AppendClaim {
                claim: PlanClaim {
                    id: Uuid::now_v7(),
                    text: "预算已批准".into(),
                    kind: ClaimKind::Synthesis,
                    evidence_indices: vec![0],
                },
            }],
            reason: "新的项目进展".into(),
            input_complete: true,
            output_complete: true,
            model_revision: "model-1".into(),
            prompt_revision: "prompt-2".into(),
            pipeline_revision: "pipeline-1".into(),
            result_hash: String::new(),
        };
        (dir, vault, plan, existing_id)
    }

    #[test]
    fn immutable_plan_rejects_source_target_scope_and_vault_drift() {
        let (dir, vault, plan, _) = fixture();
        assert_eq!(vault.check_ingest_plan(&plan), Ok(()));

        vault.write(SOURCE, "# 会议\n\n原型被取消。\n").unwrap();
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::SourceChanged(SOURCE.into()))
        );
        let rejection = vault.check_ingest_plan(&plan).unwrap_err();
        assert_eq!(rejection.code(), PlanRejectionCode::SourceChanged);
        assert_eq!(rejection.affected_source_path(), Some(SOURCE));
        vault.write(SOURCE, SOURCE_BODY).unwrap();

        let original = vault.read(TARGET).unwrap().content;
        let mut meta = crate::read_luna(&original);
        let mut knowledge: crate::KnowledgeMeta =
            serde_yaml_ng::from_value(meta.knowledge.clone().unwrap()).unwrap();
        knowledge.scope.title = "被改过的主题".into();
        meta.knowledge = Some(serde_yaml_ng::to_value(knowledge).unwrap());
        let changed_scope = crate::update_luna(&original, &meta).unwrap();
        std::fs::write(dir.path().join(TARGET), changed_scope).unwrap();
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::ScopeChanged)
        );
        assert_eq!(
            vault.check_ingest_plan(&plan).unwrap_err().code(),
            PlanRejectionCode::ScopeChanged
        );

        std::fs::write(
            dir.path().join(TARGET),
            original.replace("原型已完成", "用户改稿"),
        )
        .unwrap();
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::TargetChanged)
        );

        let other_dir = tempfile::tempdir().unwrap();
        let other_vault = Vault::open(other_dir.path()).unwrap();
        assert_eq!(
            other_vault.check_ingest_plan(&plan),
            Err(PlanRejection::WrongVault)
        );

        std::fs::write(dir.path().join(TARGET), &original).unwrap();
        std::fs::remove_file(dir.path().join(SOURCE)).unwrap();
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::SourceMissing(SOURCE.into()))
        );
        std::fs::remove_file(dir.path().join(TARGET)).unwrap();
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::TargetMissing)
        );
    }

    #[test]
    fn plan_shape_and_original_claim_are_closed() {
        let (_dir, vault, mut plan, existing_id) = fixture();
        let encoded = serde_json::to_string(&plan).unwrap();
        assert_eq!(serde_json::from_str::<IngestPlan>(&encoded).unwrap(), plan);
        plan.output_complete = false;
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::IncompleteOutput)
        );
        plan.output_complete = true;
        if let IngestAction::AppendClaim { claim } = &mut plan.actions[0] {
            claim.evidence_indices = vec![1];
        }
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::InvalidPlan)
        );
        if let IngestAction::AppendClaim { claim } = &mut plan.actions[0] {
            claim.evidence_indices = vec![0];
        }
        let claim = match plan.actions.remove(0) {
            IngestAction::AppendClaim { mut claim } => {
                claim.id = existing_id;
                claim
            }
            _ => unreachable!(),
        };
        plan.actions.push(IngestAction::ReplaceClaim {
            original_id: existing_id,
            original_block_hash: "stale".into(),
            claim,
        });
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(PlanRejection::OriginalClaimChanged(existing_id))
        );
        let original = vault.read(TARGET).unwrap();
        if let IngestAction::ReplaceClaim {
            original_block_hash,
            ..
        } = &mut plan.actions[0]
        {
            *original_block_hash = claim_block_hash(&original.content, existing_id).unwrap();
        }
        assert_eq!(vault.check_ingest_plan(&plan), Ok(()));
    }

    #[test]
    fn preview_uses_the_same_plan_without_writing_and_preserves_unrelated_bytes() {
        let (_dir, vault, mut plan, existing_id) = fixture();
        let original = vault.read(TARGET).unwrap().content;
        let original_body = body_of(&original);
        let appended = vault.preview_ingest_plan(&plan).unwrap();
        assert_eq!(vault.read(TARGET).unwrap().content, original);
        assert!(body_of(&appended).starts_with(original_body));
        assert_eq!(
            crate::validate_topic_document(&appended)
                .unwrap()
                .claims
                .len(),
            2
        );
        assert!(appended.contains("预算已批准"));

        let mut claim = match plan.actions.remove(0) {
            IngestAction::AppendClaim { claim } => claim,
            _ => unreachable!(),
        };
        claim.id = existing_id;
        claim.text = "原型按期完成".into();
        plan.actions.push(IngestAction::ReplaceClaim {
            original_id: existing_id,
            original_block_hash: claim_block_hash(&original, existing_id).unwrap(),
            claim,
        });
        let replaced = vault.preview_ingest_plan(&plan).unwrap();
        let body = body_of(&replaced);
        assert!(body.trim_start().starts_with("# 项目"));
        assert!(body.contains("原型按期完成"));
        assert!(!body.contains("- 原型已完成"));
        assert_eq!(
            crate::validate_topic_document(&replaced)
                .unwrap()
                .claims
                .len(),
            1
        );

        let conflict_id = Uuid::now_v7();
        plan.actions = vec![IngestAction::MarkConflict {
            related_id: existing_id,
            related_block_hash: claim_block_hash(&original, existing_id).unwrap(),
            claim: PlanClaim {
                id: conflict_id,
                text: "另一份资料给出相反进展".into(),
                kind: ClaimKind::Conflict,
                evidence_indices: vec![0],
            },
        }];
        let conflict = vault.preview_ingest_plan(&plan).unwrap();
        let knowledge = crate::validate_topic_document(&conflict).unwrap();
        assert_eq!(knowledge.claims.len(), 2);
        assert_eq!(
            knowledge.claims[&conflict_id.to_string()].kind,
            ClaimKind::Conflict
        );
        assert_eq!(vault.read(TARGET).unwrap().content, original);
    }

    #[test]
    fn sealed_plan_applies_once_and_records_a_new_baseline() {
        let (_dir, vault, plan, _) = fixture();
        assert_eq!(
            vault.apply_ingest_plan(&plan),
            Err(PlanRejection::InvalidPlan)
        );
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let preview = vault.preview_ingest_plan(&sealed).unwrap();
        assert_eq!(
            ContentHash::of(preview.as_bytes()).as_str(),
            sealed.result_hash
        );
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Ok(PlanApplyOutcome::Applied {
                index_updated: true,
                baseline_recorded: true,
            })
        );
        assert_eq!(vault.read(TARGET).unwrap().content, preview);
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::Ready
        );
        let snapshots = vault.snapshots(TARGET).len();
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Ok(PlanApplyOutcome::AlreadyApplied {
                index_updated: true,
                baseline_recorded: true,
            })
        );
        assert_eq!(vault.read(TARGET).unwrap().content, preview);
        assert_eq!(vault.snapshots(TARGET).len(), snapshots);
    }

    #[test]
    fn sealed_plan_never_overwrites_a_changed_source_or_topic() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let original = vault.read(TARGET).unwrap().content;
        vault.write(SOURCE, "# 会议\n\n计划取消。\n").unwrap();
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Err(PlanRejection::SourceChanged(SOURCE.into()))
        );
        assert_eq!(vault.read(TARGET).unwrap().content, original);
        vault.write(SOURCE, SOURCE_BODY).unwrap();
        let edited = original.replace("原型已完成", "用户改写的内容");
        std::fs::write(dir.path().join(TARGET), &edited).unwrap();
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Err(PlanRejection::TargetChanged)
        );
        assert_eq!(vault.read(TARGET).unwrap().content, edited);
        std::fs::remove_file(dir.path().join(TARGET)).unwrap();
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Err(PlanRejection::TargetMissing)
        );
    }

    /// 版本历史回滚到系统写过的某一版（撤销入口已不在时的退路）：页面仍是系统内容，维护
    /// 照常就绪；回滚到的若是人改过的版本，仍按人工改动停下。
    #[test]
    fn rolling_back_to_an_earlier_system_version_keeps_the_topic_maintainable() {
        let (_dir, vault, plan, _) = fixture();
        let before = vault.read(TARGET).unwrap().content;
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault.apply_ingest_plan(&sealed).unwrap();
        let before_hash = ContentHash::of(before.as_bytes()).to_string();
        let prior = vault
            .snapshots(TARGET)
            .into_iter()
            .rev()
            .find(|meta| meta.hash == before_hash)
            .expect("写入前那一版留在版本链里");
        vault.restore_snapshot(TARGET, &prior.id).unwrap();
        assert_eq!(vault.read(TARGET).unwrap().content, before);
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::Ready
        );

        // 人改过的正文再回滚回来也不算系统内容。
        let edited = before.replacen("原型已完成", "原型已完成（人工补注）", 1);
        assert_ne!(edited, before, "夹具须真的改了正文");
        vault.write(TARGET, &edited).unwrap();
        assert_ne!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::Ready
        );
    }

    #[test]
    fn retry_repairs_missing_baseline_without_rewriting_the_topic() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault.apply_ingest_plan(&sealed).unwrap();
        let applied = vault.read(TARGET).unwrap().content;
        let snapshot_index = dir
            .path()
            .join(".luna/snapshots/index")
            .join(format!("{}.json", ContentHash::of(TARGET.as_bytes())));
        std::fs::remove_file(snapshot_index).unwrap();
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::MissingBaseline
        );

        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Ok(PlanApplyOutcome::AlreadyApplied {
                index_updated: true,
                baseline_recorded: true,
            })
        );
        assert_eq!(vault.read(TARGET).unwrap().content, applied);
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::Ready
        );
    }

    #[test]
    fn oversized_plan_is_rejected_before_any_write() {
        let (_dir, vault, mut plan, _) = fixture();
        let original = vault.read(TARGET).unwrap().content;
        plan.actions = vec![plan.actions[0].clone(); MAX_PLAN_ACTIONS + 1];
        assert_eq!(
            vault.seal_ingest_plan(plan.clone()),
            Err(PlanRejection::BudgetExceeded)
        );
        plan.actions.truncate(1);
        plan.evidence = vec![plan.evidence[0].clone(); MAX_PLAN_EVIDENCE + 1];
        assert_eq!(
            vault.preview_ingest_plan(&plan),
            Err(PlanRejection::BudgetExceeded)
        );
        plan.evidence.truncate(1);
        if let IngestAction::AppendClaim { claim } = &mut plan.actions[0] {
            claim.text = "字".repeat(MAX_CLAIM_CHARS + 1);
        }
        assert_eq!(
            vault.preview_ingest_plan(&plan),
            Err(PlanRejection::BudgetExceeded)
        );
        if let IngestAction::AppendClaim { claim } = &mut plan.actions[0] {
            claim.text = "预算已批准".into();
            claim.evidence_indices = vec![0; MAX_CITATIONS_PER_ACTION + 1];
        }
        assert_eq!(
            vault.preview_ingest_plan(&plan),
            Err(PlanRejection::BudgetExceeded)
        );
        if let IngestAction::AppendClaim { claim } = &mut plan.actions[0] {
            claim.evidence_indices = vec![0];
        }
        plan.reason = "x".repeat(MAX_PLAN_BYTES);
        assert_eq!(
            vault.preview_ingest_plan(&plan),
            Err(PlanRejection::BudgetExceeded)
        );
        assert_eq!(vault.read(TARGET).unwrap().content, original);
    }

    #[test]
    fn user_rejection_survives_index_rebuild_and_blocks_same_version_replay() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let before = vault.read(TARGET).unwrap().content;
        vault
            .record_ingest_decision(&sealed, IngestDecisionKind::Rejected)
            .unwrap();
        assert_eq!(
            vault.check_ingest_plan(&sealed),
            Err(PlanRejection::UserSuppressed)
        );
        assert_eq!(
            vault.check_ingest_plan(&sealed).unwrap_err().code(),
            PlanRejectionCode::UserSuppressed
        );
        assert_eq!(
            vault.apply_ingest_plan(&sealed),
            Err(PlanRejection::UserSuppressed)
        );
        assert_eq!(vault.read(TARGET).unwrap().content, before);
        let mut retry = sealed.clone();
        if let IngestAction::AppendClaim { claim } = &mut retry.actions[0] {
            claim.text = "改写措辞的同版本建议".into();
            claim.id = Uuid::now_v7();
        }
        assert_eq!(
            vault.check_ingest_plan(&retry),
            Err(PlanRejection::UserSuppressed)
        );
        let stored =
            std::fs::read_to_string(dir.path().join(".luna/knowledge_ingest_decisions.json"))
                .unwrap();
        assert!(
            !stored.contains("预算已批准"),
            "用户决定不得复制模型声明正文"
        );
        assert!(!stored.contains(SOURCE_BODY), "用户决定不得复制来源正文");
        drop(vault);

        std::fs::remove_file(dir.path().join(".luna/index.sqlite")).unwrap();
        let reopened = Vault::open(dir.path()).unwrap();
        reopened.scan().unwrap();
        assert_eq!(
            reopened.check_ingest_plan(&sealed),
            Err(PlanRejection::UserSuppressed),
            "重建派生索引不能让用户否决的同版本计划复活"
        );
        let decisions = reopened.ingest_decisions().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].action_summary, [("append_claim".into(), 1)]);
        assert!(!decisions[0].action_digest.is_empty());
    }

    #[test]
    fn explicit_restore_or_new_source_version_allows_planning_again() {
        let (_dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault
            .record_ingest_decision(&sealed, IngestDecisionKind::Ignored)
            .unwrap();
        assert_eq!(
            vault.check_ingest_plan(&sealed),
            Err(PlanRejection::UserSuppressed)
        );
        vault.restore_ingest_decision(&sealed).unwrap();
        assert_eq!(vault.check_ingest_plan(&sealed), Ok(()));

        vault
            .record_ingest_decision(&sealed, IngestDecisionKind::Rejected)
            .unwrap();
        vault
            .write(SOURCE, "# 会议\n\n原型已完成。\n\n新增的后续事实。\n")
            .unwrap();
        let mut newer = sealed.clone();
        newer.evidence[0].source_hash = vault.body_fingerprint(SOURCE).unwrap();
        newer.result_hash.clear();
        let newer = vault.seal_ingest_plan(newer).unwrap();
        assert_eq!(vault.check_ingest_plan(&newer), Ok(()));
        assert_eq!(vault.ingest_decisions().unwrap().len(), 1);
    }

    #[test]
    fn unknown_decision_schema_blocks_automatic_plan_application() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let original = vault.read(TARGET).unwrap().content;
        std::fs::write(
            dir.path().join(".luna/knowledge_ingest_decisions.json"),
            r#"{"schema":2,"entries":[]}"#,
        )
        .unwrap();
        assert!(matches!(
            vault.apply_ingest_plan(&sealed),
            Err(PlanRejection::DecisionStore(_))
        ));
        assert_eq!(vault.read(TARGET).unwrap().content, original);
    }

    #[test]
    fn commit_lock_serializes_decision_writes_from_separate_vault_handles() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let guard = knowledge_commit_lock(&vault).unwrap();
        let root = dir.path().to_owned();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let other = Vault::open(root).unwrap();
            started_tx.send(()).unwrap();
            done_tx
                .send(other.record_ingest_decision(&sealed, IngestDecisionKind::Rejected))
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(guard);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(vault.ingest_decisions().unwrap().len(), 1);
    }

    #[test]
    fn competing_plans_from_one_baseline_cannot_both_commit() {
        let (dir, vault, plan, _) = fixture();
        let first = vault.seal_ingest_plan(plan.clone()).unwrap();
        let mut alternative = plan;
        alternative.operation_id = Uuid::now_v7();
        if let IngestAction::AppendClaim { claim } = &mut alternative.actions[0] {
            claim.id = Uuid::now_v7();
            claim.text = "预算调整待确认".into();
        }
        let second = vault.seal_ingest_plan(alternative).unwrap();
        drop(vault);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers = [first, second].map(|sealed| {
            let root = dir.path().to_owned();
            let ready = barrier.clone();
            std::thread::spawn(move || {
                let vault = Vault::open(root).unwrap();
                ready.wait();
                vault.apply_ingest_plan(&sealed)
            })
        });
        let [left, right] = workers.map(|worker| worker.join().unwrap());
        assert_eq!(
            [left.clone(), right.clone()]
                .into_iter()
                .filter(|result| matches!(result, Ok(PlanApplyOutcome::Applied { .. })))
                .count(),
            1
        );
        assert!(
            [left, right]
                .into_iter()
                .any(|result| result == Err(PlanRejection::TargetChanged)),
            "失去底版的竞争者必须拒绝，不能覆盖先提交的声明"
        );
    }

    #[test]
    fn sealed_plan_artifact_survives_restart_and_index_rebuild() {
        let (dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        let path = plan_artifact_path(&vault, sealed.operation_id);
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(SOURCE_BODY), "续跑载荷不复制完整来源正文");
        drop(vault);

        std::fs::remove_file(dir.path().join(".luna/index.sqlite")).unwrap();
        let reopened = Vault::open(dir.path()).unwrap();
        reopened.scan().unwrap();
        let recovered = reopened
            .load_sealed_ingest_plan(sealed.operation_id)
            .unwrap();
        assert_eq!(recovered, sealed);
        assert!(matches!(
            reopened.apply_ingest_plan(&recovered),
            Ok(PlanApplyOutcome::Applied { .. })
        ));
        assert_eq!(
            reopened.apply_ingest_plan(&recovered),
            Ok(PlanApplyOutcome::AlreadyApplied {
                index_updated: true,
                baseline_recorded: true,
            })
        );
    }

    #[test]
    fn sealed_plan_id_cannot_be_reused_to_swap_confirmed_content() {
        let (_dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan.clone()).unwrap();
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        let path = plan_artifact_path(&vault, sealed.operation_id);
        let before = std::fs::read(&path).unwrap();
        let mut changed = plan;
        if let IngestAction::AppendClaim { claim } = &mut changed.actions[0] {
            claim.text = "换成另一份声明".into();
        }
        let changed = vault.seal_ingest_plan(changed).unwrap();
        assert_eq!(
            vault.persist_sealed_ingest_plan(&changed),
            Err(PlanRejection::InvalidPlan)
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            vault.load_sealed_ingest_plan(sealed.operation_id).unwrap(),
            sealed
        );
    }

    #[test]
    fn unknown_or_corrupt_plan_artifact_is_preserved_and_rejected() {
        let (_dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        let path = plan_artifact_path(&vault, sealed.operation_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let unknown = serde_json::json!({"schema": 2, "plan": sealed});
        let bytes = serde_json::to_vec(&unknown).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            vault.load_sealed_ingest_plan(sealed.operation_id),
            Err(PlanRejection::PlanStore(_))
        ));
        assert_eq!(
            vault.persist_sealed_ingest_plan(&sealed),
            Err(PlanRejection::InvalidPlan)
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        std::fs::write(&path, b"{truncated").unwrap();
        assert!(matches!(
            vault.load_sealed_ingest_plan(sealed.operation_id),
            Err(PlanRejection::PlanStore(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"{truncated");
    }

    #[test]
    fn discarded_artifact_reports_missing_not_store_failure() {
        let (_dir, vault, plan, _) = fixture();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        let target_before = vault.read(TARGET).unwrap().content;
        vault
            .discard_sealed_ingest_plan(sealed.operation_id)
            .unwrap();
        vault
            .discard_sealed_ingest_plan(sealed.operation_id)
            .unwrap();
        let missing = vault
            .load_sealed_ingest_plan(sealed.operation_id)
            .unwrap_err();
        assert_eq!(missing, PlanRejection::PlanMissing);
        assert_eq!(missing.code(), PlanRejectionCode::PlanMissing);
        assert_eq!(vault.read(TARGET).unwrap().content, target_before);
    }

    #[test]
    fn conditional_write_refuses_bytes_it_did_not_check() {
        let (dir, vault, _plan, _) = fixture();
        let original = vault.read(TARGET).unwrap().content;
        let stale = ContentHash::of(b"not the current topic").to_string();
        assert!(!vault
            .write_if_current(TARGET, &stale, "replacement")
            .unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(TARGET)).unwrap(),
            original
        );
        let current = ContentHash::of(original.as_bytes()).to_string();
        let changed = original.replace("原型已完成", "原型已完成。");
        assert!(vault.write_if_current(TARGET, &current, &changed).unwrap());
        assert_eq!(vault.read(TARGET).unwrap().content, changed);
        assert!(matches!(
            vault.write_if_current("Knowledge/缺失.md", &current, "x"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn a_rejected_source_version_stays_suppressed_when_bundled_with_a_new_source() {
        let (_dir, vault, plan, _) = fixture();
        let rejected = vault.seal_ingest_plan(plan.clone()).unwrap();
        vault
            .record_ingest_decision(&rejected, IngestDecisionKind::Rejected)
            .unwrap();
        vault
            .write("Notes/周报.md", "# 周报\n\n崩溃率降低。\n")
            .unwrap();
        let mut bundled = plan;
        bundled.operation_id = Uuid::now_v7();
        bundled.evidence.push(PlanEvidence {
            source_path: "Notes/周报.md".into(),
            source_title: "周报".into(),
            source_hash: vault.body_fingerprint("Notes/周报.md").unwrap(),
            locator: EvidenceLocator {
                heading_path: vec!["周报".into()],
                quote: "崩溃率降低".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        });
        assert_eq!(
            vault.check_ingest_plan(&bundled),
            Err(PlanRejection::UserSuppressed),
            "被否决的来源版本换个组合也不能重新推荐"
        );
        let suppressed = vault
            .suppressed_source_versions(TARGET, &bundled.baseline.scope_hash)
            .unwrap();
        assert_eq!(suppressed.len(), 1);
        assert!(suppressed.iter().any(|(path, _)| path == SOURCE));
        assert!(vault
            .suppressed_source_versions(TARGET, "another-scope")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn candidate_sources_drop_rejected_versions_until_the_source_changes() {
        let (_dir, vault, mut plan, _) = fixture();
        let report = "Notes/周报.md";
        vault
            .write(report, "---\ntags: [项目]\n---\n# 周报\n\n崩溃率降低。\n")
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        assert_eq!(
            vault
                .topic_candidate_sources_of(TARGET, &content, crate::SourcePolicy::FORMAL_ONLY)
                .unwrap(),
            vec![report.to_owned()]
        );
        plan.evidence = vec![PlanEvidence {
            source_path: report.into(),
            source_title: "周报".into(),
            source_hash: vault.body_fingerprint(report).unwrap(),
            locator: EvidenceLocator {
                heading_path: vec!["周报".into()],
                quote: "崩溃率降低".into(),
                context: String::new(),
                line_hint: Some(6),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault
            .record_ingest_decision(&sealed, IngestDecisionKind::Rejected)
            .unwrap();
        assert!(
            vault
                .topic_candidate_sources_of(TARGET, &content, crate::SourcePolicy::FORMAL_ONLY)
                .unwrap()
                .is_empty(),
            "目录计数与生成都不再把被否决的当前版本算作候选"
        );
        vault
            .write(
                report,
                "---\ntags: [项目]\n---\n# 周报\n\n崩溃率降到 0.1%。\n",
            )
            .unwrap();
        assert_eq!(
            vault
                .topic_candidate_sources_of(TARGET, &content, crate::SourcePolicy::FORMAL_ONLY)
                .unwrap(),
            vec![report.to_owned()],
            "来源出现新版本后重新成为候选"
        );
    }

    const TRANSCRIPT: &str = "Transcripts/周会录音.md";
    const DIGEST: &str = "Notes/周会纪要.md";

    fn transcript_note(authoritative: bool) -> String {
        format!(
            "---\ntags: [项目]\nluna:\n  schema: 1\n  type: transcript\n{}---\n# 周会录音\n\n\
             <!-- luna:transcript -->\n[00:00:05.000] 发布推迟到下个月。\n\
             [00:00:09.500] \u{1F516} 关键\n[00:00:12.000] 预算已经批准。\n\
             <!-- /luna:transcript -->\n",
            if authoritative {
                "  transcript_authoritative: true\n"
            } else {
                ""
            }
        )
    }

    #[test]
    fn authorized_transcripts_join_candidates_and_replace_their_digests() {
        let (_dir, vault, _plan, _) = fixture();
        vault.write(TRANSCRIPT, &transcript_note(true)).unwrap();
        vault
            .write(
                DIGEST,
                &format!(
                    "---\ntags: [项目]\nluna:\n  schema: 1\n  distill_source: {TRANSCRIPT}\n---\n\
                     # 周会纪要\n\n发布推迟。\n"
                ),
            )
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        // 未授权：转写不进候选，它的蒸馏笔记照常作为正式笔记参与。
        assert_eq!(
            vault
                .topic_candidate_sources_of(TARGET, &content, crate::SourcePolicy::FORMAL_ONLY)
                .unwrap(),
            vec![DIGEST.to_owned()]
        );
        assert_eq!(
            vault
                .topic_transcript_candidates_of(TARGET, &content)
                .unwrap(),
            vec![TRANSCRIPT.to_owned()],
            "未授权时仍能告诉用户还有这篇可作来源"
        );
        // 授权后：权威转写进入候选，针对它写的蒸馏笔记不再算一份独立证据。
        let authorized = crate::SourcePolicy {
            transcripts: true,
            quick_notes: false,
        };
        assert_eq!(
            vault
                .topic_candidate_sources_of(TARGET, &content, authorized)
                .unwrap(),
            vec![TRANSCRIPT.to_owned()]
        );
        // 证据按带时间码的源文行提取，书签行不算，标为转写记载。
        let batch = vault
            .topic_evidence(&[TRANSCRIPT.to_owned()], authorized)
            .unwrap();
        assert!(batch.input_complete);
        let quotes: Vec<_> = batch
            .evidence
            .iter()
            .map(|item| item.excerpt.as_str())
            .collect();
        assert_eq!(quotes, vec!["发布推迟到下个月。", "预算已经批准。"]);
        assert!(batch
            .evidence
            .iter()
            .all(|item| item.kind == crate::TopicEvidenceKind::AuthoritativeTranscript));
        let first = &batch.evidence[0].source.locator;
        assert_eq!(
            (first.time_start_ms, first.time_end_ms),
            (Some(5_000), Some(12_000))
        );
        // 未授权时同一篇只计遗漏，不偷偷进证据。
        let formal = vault
            .topic_evidence(&[TRANSCRIPT.to_owned()], crate::SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert!(formal.evidence.is_empty() && formal.omitted_sources == 1);
    }

    /// 权威但没有任何源文行的转写（空文本、只剩书签/译文）不是来源：没有可引的时间码行，
    /// 进了候选也只会让模型对着空证据综合。
    #[test]
    fn empty_authoritative_transcripts_never_become_sources() {
        let (_dir, vault, _plan, _) = fixture();
        vault
            .write(
                TRANSCRIPT,
                "---\ntags: [项目]\nluna:\n  schema: 1\n  type: transcript\n  transcript_authoritative: true\n---\n\
                 # 周会录音\n\n<!-- luna:transcript -->\n[00:00:09.500] \u{1F516} 关键\n<!-- /luna:transcript -->\n",
            )
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        assert!(vault
            .topic_transcript_candidates_of(TARGET, &content)
            .unwrap()
            .is_empty());
        assert!(vault
            .topics_with_candidate(
                TRANSCRIPT,
                crate::SourcePolicy {
                    transcripts: true,
                    quick_notes: false
                }
            )
            .is_empty());
    }

    #[test]
    fn preview_transcripts_never_become_sources_even_when_authorized() {
        let (_dir, vault, _plan, _) = fixture();
        vault.write(TRANSCRIPT, &transcript_note(false)).unwrap();
        let content = vault.read(TARGET).unwrap().content;
        assert!(vault
            .topic_transcript_candidates_of(TARGET, &content)
            .unwrap()
            .is_empty());
        assert!(vault
            .topic_candidate_sources_of(
                TARGET,
                &content,
                crate::SourcePolicy {
                    transcripts: true,
                    quick_notes: false
                }
            )
            .unwrap()
            .is_empty());
    }

    /// 速记凭来源范围直接作证据（ADR-136 ①，PRD-16-057）：标「速记记载」，交给模型时按用户
    /// 自己写的笔记呈现；普通「待整理」笔记仍只在显式完成整理后才成为候选（PRD-16-047）；
    /// 空速记不是来源。
    #[test]
    fn quick_notes_are_sources_but_drafts_join_only_after_an_explicit_settle() {
        let (_dir, vault, _plan, _) = fixture();
        let quick = crate::SourcePolicy {
            transcripts: false,
            quick_notes: true,
        };
        vault
            .write(
                "QuickNotes/空的.md",
                "---\ntags: [项目]\nluna:\n  schema: 1\n  type: quick-note\n---\n\n",
            )
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        assert!(vault
            .topic_quick_note_candidates_of(TARGET, &content)
            .unwrap()
            .is_empty());
        let policy = crate::SourcePolicy::FORMAL_ONLY;
        vault
            .write(
                "QuickNotes/客户访谈.md",
                "---\ntags: [项目]\nluna:\n  schema: 1\n  type: quick-note\n---\n# 客户访谈\n\n希望支持批量导入。\n",
            )
            .unwrap();
        vault
            .write(
                "Notes/待整理.md",
                "---\ntags: [项目]\nluna:\n  schema: 1\n  stage: draft\n---\n# 待整理\n\n预算草案。\n",
            )
            .unwrap();
        assert!(vault
            .topics_with_candidate("QuickNotes/客户访谈.md", policy)
            .is_empty());
        assert!(vault
            .topics_with_candidate("Notes/待整理.md", policy)
            .is_empty());
        // 速记凭来源范围进入候选与证据，普通待整理笔记照旧挡在外面。
        assert_eq!(
            vault.topics_with_candidate("QuickNotes/客户访谈.md", quick),
            vec![TARGET.to_owned()]
        );
        assert!(vault
            .topics_with_candidate("Notes/待整理.md", quick)
            .is_empty());
        let batch = vault
            .topic_evidence(&["QuickNotes/客户访谈.md".to_owned()], quick)
            .unwrap();
        assert_eq!(batch.evidence.len(), 1);
        assert_eq!(batch.evidence[0].kind, crate::TopicEvidenceKind::QuickNote);
        assert_eq!(
            batch.evidence[0].kind.model_facing(),
            crate::TopicEvidenceKind::FormalNote,
            "模型协议里没有速记这一类，按用户自己写的笔记呈现"
        );
        let formal = vault
            .topic_evidence(&["QuickNotes/客户访谈.md".to_owned()], policy)
            .unwrap();
        assert!(formal.evidence.is_empty() && formal.omitted_sources == 1);

        vault
            .move_("QuickNotes/客户访谈.md", "Notes/客户访谈.md")
            .unwrap();
        vault
            .set_note_type("Notes/客户访谈.md", crate::NoteType::Note)
            .unwrap();
        assert_eq!(
            vault.topics_with_candidate("Notes/客户访谈.md", policy),
            vec![TARGET.to_owned()]
        );
    }

    /// 正文写到了主题名的来源也是候选（PRD-16-063）：没有标签的笔记、速记、已授权的权威转写
    /// 按内容到达主题；写的是别的项目、或带排除标签的不进；两字名不用来圈候选。
    #[test]
    fn sources_that_mention_the_topic_name_join_without_a_tag() {
        let (_dir, vault, _plan, _) = fixture();
        let named = "Knowledge/项目甲.md";
        let source_hash = vault.body_fingerprint(SOURCE).unwrap();
        let source = [ClaimSource {
            path: SOURCE,
            title: "会议",
            source_hash: &source_hash,
            locator: locator(),
            complete_input: true,
        }];
        let claim = NewClaim {
            id: Uuid::now_v7(),
            text: "原型已完成",
            kind: ClaimKind::Synthesis,
            sources: &source,
        };
        let mut scope = TopicScope::from_tag("项目/甲");
        scope.excludes = vec!["归档".into()];
        let content = vault
            .scoped_topic_content(named, Some("项目/甲"), scope, &[claim])
            .unwrap();
        vault.publish_scoped_topic(named, &content).unwrap();
        vault
            .write("Notes/周会.md", "# 周会\n\n项目甲的后端改用 PostgreSQL。\n")
            .unwrap();
        vault
            .write("Notes/别的.md", "# 周会\n\n项目乙的预算追加了两成。\n")
            .unwrap();
        vault
            .write(
                "Notes/归档的.md",
                "---\ntags: [归档]\n---\n# 旧\n\n项目甲去年的计划。\n",
            )
            .unwrap();
        vault
            .write(
                "QuickNotes/速记.md",
                "---\nluna:\n  schema: 1\n  type: quick-note\n---\n客户说项目甲要支持导出。\n",
            )
            .unwrap();
        let content = vault.read(named).unwrap().content;
        let quick = crate::SourcePolicy {
            transcripts: false,
            quick_notes: true,
        };
        let candidates = vault
            .topic_candidate_sources_of(named, &content, quick)
            .unwrap();
        assert!(
            candidates.contains(&"Notes/周会.md".to_owned()),
            "{candidates:?}"
        );
        assert!(
            candidates.contains(&"QuickNotes/速记.md".to_owned()),
            "{candidates:?}"
        );
        assert!(
            !candidates.contains(&"Notes/别的.md".to_owned()),
            "写的是别的项目"
        );
        assert!(
            !candidates.contains(&"Notes/归档的.md".to_owned()),
            "带排除标签"
        );
        assert_eq!(
            vault.topics_with_candidate("Notes/周会.md", quick),
            vec![named.to_owned()],
            "反方向同一口径：这篇一改就牵动写到它名字的主题"
        );
        // 两字名的主题（「项目」）不按名字圈：写到「项目」二字的笔记不因此成为它的候选。
        let content = vault.read(TARGET).unwrap().content;
        assert!(!vault
            .topic_candidate_sources_of(TARGET, &content, quick)
            .unwrap()
            .contains(&"Notes/周会.md".to_owned()));
    }

    /// 产物更新不重入根任务（TDD-06 §3.9.3）：主题页、第二个同标签主题页、知识地图的写回
    /// 走到去抖触发时牵动不到任何主题——它们不是候选来源，后台准备不会拿产物喂产物。
    #[test]
    fn product_pages_never_trigger_topic_preparation() {
        let (_dir, vault, _plan, _) = fixture();
        let other = "Knowledge/项目-二.md";
        let source_hash = vault.body_fingerprint(SOURCE).unwrap();
        let source = [ClaimSource {
            path: SOURCE,
            title: "会议",
            source_hash: &source_hash,
            locator: locator(),
            complete_input: true,
        }];
        let claim = NewClaim {
            id: Uuid::now_v7(),
            text: "原型已完成",
            kind: ClaimKind::Synthesis,
            sources: &source,
        };
        let content = vault
            .scoped_topic_content(other, Some("项目"), TopicScope::from_tag("项目"), &[claim])
            .unwrap();
        vault.publish_scoped_topic(other, &content).unwrap();
        vault
            .write(
                "Knowledge/知识地图.md",
                "---\ntags: [项目]\nluna:\n  schema: 1\n  knowledge_map: true\n---\n# 知识地图\n\n- 项目\n",
            )
            .unwrap();
        let policy = crate::SourcePolicy {
            transcripts: true,
            quick_notes: false,
        };
        for product in [TARGET, other, "Knowledge/知识地图.md"] {
            assert!(
                vault.topics_with_candidate(product, policy).is_empty(),
                "{product} 不该牵动任何主题"
            );
        }
    }

    /// 证据格式页的候选只按声明的范围取（ADR-134 范围先于总结）：系统给主题页自动打上的
    /// `ai_tags` 不扩大范围，带排除标签的笔记也不进来。
    #[test]
    fn evidence_topics_take_candidates_from_their_declared_scope_only() {
        let (_dir, vault, _plan, _) = fixture();
        let page = vault.read(TARGET).unwrap().content;
        let tagged = page.replacen("luna:\n", "luna:\n  ai_tags:\n  - 功能/网页端\n", 1);
        assert_ne!(tagged, page, "夹具须真的给主题页加上 AI 标签");
        vault.write(TARGET, &tagged).unwrap();
        vault
            .write(
                "Notes/别的项目.md",
                "---\ntags: [功能/网页端]\n---\n# 别的项目\n\n网页端改版。\n",
            )
            .unwrap();
        vault
            .write(
                "Notes/新进展.md",
                "---\ntags: [项目]\n---\n# 新进展\n\n预算已批准。\n",
            )
            .unwrap();
        let policy = crate::SourcePolicy::FORMAL_ONLY;
        let content = vault.read(TARGET).unwrap().content;
        assert!(crate::validate_topic_document(&content).is_ok());
        let candidates = vault
            .topic_candidate_sources_of(TARGET, &content, policy)
            .unwrap();
        assert!(candidates.contains(&"Notes/新进展.md".to_owned()));
        assert!(
            !candidates.contains(&"Notes/别的项目.md".to_owned()),
            "主题页自己的 AI 标签不是范围"
        );
        assert!(vault
            .topics_with_candidate("Notes/别的项目.md", policy)
            .is_empty());

        // 排除范围：带排除标签的笔记即使也带包含标签，也不进来。
        let other = "Knowledge/项目-对外.md";
        let source_hash = vault.body_fingerprint(SOURCE).unwrap();
        let source = [ClaimSource {
            path: SOURCE,
            title: "会议",
            source_hash: &source_hash,
            locator: locator(),
            complete_input: true,
        }];
        let claim = NewClaim {
            id: Uuid::now_v7(),
            text: "原型已完成",
            kind: ClaimKind::Synthesis,
            sources: &source,
        };
        let mut scope = TopicScope::from_tag("项目");
        scope.excludes = vec!["项目/内部".to_owned()];
        let scoped = vault
            .scoped_topic_content(other, Some("项目"), scope, &[claim])
            .unwrap();
        vault.publish_scoped_topic(other, &scoped).unwrap();
        vault
            .write(
                "Notes/内部.md",
                "---\ntags: [项目, 项目/内部]\n---\n# 内部\n\n人事调整。\n",
            )
            .unwrap();
        let content = vault.read(other).unwrap().content;
        let candidates = vault
            .topic_candidate_sources_of(other, &content, policy)
            .unwrap();
        assert!(candidates.contains(&"Notes/新进展.md".to_owned()));
        assert!(
            !candidates.contains(&"Notes/内部.md".to_owned()),
            "排除标签生效"
        );
    }

    /// 候选页覆盖优先于新建（ADR-134）：已有一页的声明范围包含某个标签时，不再为它建议
    /// 另建一页；标签页与新建入口都指向那一页。
    #[test]
    fn a_tag_already_in_a_topic_scope_points_to_that_page_instead_of_a_new_one() {
        let (_dir, vault, _plan, _) = fixture();
        for index in 0..crate::TOPIC_SUGGESTION_MIN_NOTES {
            vault
                .write(
                    &format!("Notes/C-{index}.md"),
                    &format!("---\ntags: [项目/C]\n---\n# C {index}\n\n第 {index} 条进展。\n"),
                )
                .unwrap();
        }
        assert!(
            vault
                .topic_suggestions(&[])
                .unwrap()
                .iter()
                .any(|s| s.tag == "项目/C"),
            "夹具须先让「项目/C」够格被建议"
        );
        assert_eq!(vault.topic_page_for_tag("项目/C").unwrap(), None);

        let umbrella = "Knowledge/项目群.md";
        let source_hash = vault.body_fingerprint(SOURCE).unwrap();
        let source = [ClaimSource {
            path: SOURCE,
            title: "会议",
            source_hash: &source_hash,
            locator: locator(),
            complete_input: true,
        }];
        let claim = NewClaim {
            id: Uuid::now_v7(),
            text: "原型已完成",
            kind: ClaimKind::Synthesis,
            sources: &source,
        };
        let mut scope = TopicScope::from_tag("项目群");
        scope.includes.push("项目/C".to_owned());
        let content = vault
            .scoped_topic_content(umbrella, Some("项目群"), scope, &[claim])
            .unwrap();
        vault.publish_scoped_topic(umbrella, &content).unwrap();

        assert!(!vault
            .topic_suggestions(&[])
            .unwrap()
            .iter()
            .any(|s| s.tag == "项目/C"));
        assert_eq!(
            vault.topic_page_for_tag("项目/C").unwrap().as_deref(),
            Some(umbrella)
        );
    }

    /// 已有引用不屏蔽新版本（K4）：本页引用过的来源在没变时不是候选；来源改了之后重新成为
    /// 候选，新版本交给模型看，旧声明仍标「来源已变」待复核而不被悄悄替换。
    #[test]
    fn a_cited_source_returns_as_a_candidate_once_it_changes() {
        let (_dir, vault, _plan, _) = fixture();
        let policy = crate::SourcePolicy::FORMAL_ONLY;
        // 只加文件头标签：正文版本不变，仍是本页引用的那一版。
        vault
            .write(SOURCE, &format!("---\ntags: [项目]\n---\n{SOURCE_BODY}"))
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        assert!(
            !vault
                .topic_candidate_sources_of(TARGET, &content, policy)
                .unwrap()
                .contains(&SOURCE.to_owned()),
            "没变的已引用来源不是候选"
        );
        vault
            .write(
                SOURCE,
                &format!("---\ntags: [项目]\n---\n{SOURCE_BODY}\n补充：预算追加一成。\n"),
            )
            .unwrap();
        assert!(vault
            .topic_candidate_sources_of(TARGET, &content, policy)
            .unwrap()
            .contains(&SOURCE.to_owned()));
        assert_eq!(
            vault.topics_with_candidate(SOURCE, policy),
            vec![TARGET.to_owned()],
            "改了的已引用来源会牵动后台准备"
        );
        assert!(vault
            .topic_evidence_states(TARGET)
            .unwrap()
            .iter()
            .all(|entry| entry.state == crate::knowledge_evidence::EvidenceState::SourceChanged));
    }

    /// 问答不把出处已失效的旧声明当作当前已核实的事实（K4）：主题页摘录里这类声明带行内
    /// 说明，来源变了与来源没了各一句；不给说明时摘录原样；主题页本身不动。
    #[test]
    fn answers_see_unverified_topic_claims_marked_in_place() {
        let (_dir, vault, _plan, _) = fixture();
        let notices = crate::ClaimNotices {
            source_changed: "〔来源已变〕".into(),
            missing: "〔出处已不存在〕".into(),
            conflict: "〔有分歧〕".into(),
        };
        let topic_passage = |notices: &crate::ClaimNotices| {
            let options = crate::RetrieveOptions {
                claim_notices: notices.clone(),
                ..crate::RetrieveOptions::default()
            };
            vault
                .retrieve("原型已完成", &options)
                .unwrap()
                .passages
                .into_iter()
                .find(|passage| passage.path == TARGET)
                .map(|passage| passage.text)
                .expect("主题页应被检索到")
        };
        let before = vault.read(TARGET).unwrap().content;
        assert!(
            !topic_passage(&notices).contains('〔'),
            "出处当前有效时不加注"
        );

        vault
            .write(SOURCE, &format!("{SOURCE_BODY}\n补充：预算追加一成。\n"))
            .unwrap();
        assert!(topic_passage(&notices).contains("〔来源已变〕原型已完成"));
        assert!(
            !topic_passage(&crate::ClaimNotices::default()).contains('〔'),
            "不给说明时摘录原样"
        );

        vault.delete(SOURCE, crate::DeleteMode::Permanent).unwrap();
        assert!(topic_passage(&notices).contains("〔出处已不存在〕原型已完成"));
        assert_eq!(
            vault.read(TARGET).unwrap().content,
            before,
            "主题页一个字节不动"
        );
    }

    /// 问答不把有分歧的说法当成定论（K5）：分歧写入后，分歧本身和它并列的原说法在摘录里
    /// 都带说明；出处失效与分歧同时成立时两句都在；主题页本身不动。
    #[test]
    fn answers_see_both_sides_of_a_conflict_marked() {
        let (_dir, vault, _plan, existing_id) = fixture();
        let context = vault.topic_update_context(TARGET).unwrap();
        let conflict = crate::GroundedUpdate {
            claim: crate::GroundedClaim {
                id: Uuid::now_v7(),
                text: "另一份记录称原型尚未完成".into(),
                kind: ClaimKind::Conflict,
                evidence_indices: vec![0],
            },
            conflicts_with: Some(existing_id),
            replaces: None,
        };
        let sealed = vault
            .seal_topic_update_plan(
                context.baseline,
                &update_evidence(&vault),
                true,
                &[conflict],
                revisions(),
            )
            .unwrap();
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        vault.apply_ingest_plan(&sealed).unwrap();
        let notices = crate::ClaimNotices {
            source_changed: "〔来源已变〕".into(),
            missing: "〔出处已不存在〕".into(),
            conflict: "〔有分歧〕".into(),
        };
        let options = crate::RetrieveOptions {
            claim_notices: notices,
            ..crate::RetrieveOptions::default()
        };
        let topic_passage = |query: &str| {
            vault
                .retrieve(query, &options)
                .unwrap()
                .passages
                .into_iter()
                .find(|passage| passage.path == TARGET)
                .map(|passage| passage.text)
                .expect("主题页应被检索到")
        };
        let before = vault.read(TARGET).unwrap().content;
        let text = topic_passage("原型已完成");
        assert!(text.contains("〔有分歧〕原型已完成"), "原说法也标：{text}");
        let text = topic_passage("原型尚未完成");
        assert!(
            text.contains("〔有分歧〕另一份记录称原型尚未完成"),
            "{text}"
        );

        vault
            .write(SOURCE, &format!("{SOURCE_BODY}\n补充：预算追加一成。\n"))
            .unwrap();
        assert!(topic_passage("原型已完成").contains("〔来源已变〕〔有分歧〕原型已完成"));
        assert_eq!(
            vault.read(TARGET).unwrap().content,
            before,
            "主题页一个字节不动"
        );
    }

    /// 撤销计划写入（K4）：仍是写入后那一版时逐字节恢复写入前版本、维护仍就绪；之后又改过
    /// 则一个字节不动；写入前版本找不到时如实说找不到。
    #[test]
    fn undoing_an_applied_plan_restores_the_exact_prior_bytes_or_refuses() {
        let (_dir, vault, plan, _) = fixture();
        let before = vault.read(TARGET).unwrap().content;
        let before_hash = ContentHash::of(before.as_bytes()).to_string();
        let sealed = vault.seal_ingest_plan(plan).unwrap();
        vault.apply_ingest_plan(&sealed).unwrap();
        let after_hash = sealed.result_hash.clone();
        assert_ne!(before_hash, after_hash);

        // 找不到写入前那一版：不动。
        assert_eq!(
            vault.undo_applied_plan(TARGET, "sha256:不存在", &after_hash),
            Ok(PlanUndoOutcome::Unavailable)
        );
        assert_eq!(
            ContentHash::of(vault.read(TARGET).unwrap().content.as_bytes()).as_str(),
            after_hash
        );
        assert_eq!(
            vault.undo_applied_plan(TARGET, &before_hash, &after_hash),
            Ok(PlanUndoOutcome::Restored)
        );
        assert_eq!(
            vault.read(TARGET).unwrap().content,
            before,
            "逐字节回到写入前"
        );
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            crate::TopicMaintenanceState::Ready,
            "撤销后仍由系统维护"
        );

        // 再写一次，之后人工改动：撤销拒绝且不覆盖那次改动。
        let (_d2, other, plan, _) = fixture();
        let before2 = other.read(TARGET).unwrap().content;
        let sealed = other.seal_ingest_plan(plan).unwrap();
        other.apply_ingest_plan(&sealed).unwrap();
        let edited = format!("{}\n人工补一句。\n", other.read(TARGET).unwrap().content);
        other.write(TARGET, &edited).unwrap();
        assert_eq!(
            other.undo_applied_plan(
                TARGET,
                ContentHash::of(before2.as_bytes()).as_str(),
                &sealed.result_hash
            ),
            Ok(PlanUndoOutcome::Changed)
        );
        assert_eq!(
            other.read(TARGET).unwrap().content,
            edited,
            "之后的改动不被撤销覆盖"
        );
    }

    /// 「看过」记录的口径里提示词与流水线那一段（调用方给的不透明串）。
    const REV: &str = "topic_update@fixture\ntopic-update-v1";

    /// 模型输出不可用的重试预算（2026-09-27 裁决，对照批处理「可重试失败有预算、超出即
    /// 搁置」的通行做法）：第一次不可用不记看过、后台下次触发会重试；同一版第二次不可用
    /// 才记为看过；中间成功一次就清掉计数；来源改了版本从头计。
    #[test]
    fn unusable_output_gets_one_retry_before_it_counts_as_considered() {
        let (_dir, vault, _plan, _) = fixture();
        let content = vault.read(TARGET).unwrap().content;
        let paths = vec![SOURCE.to_owned()];
        let seen = vec![crate::SourceCoverage::whole(
            SOURCE,
            vault.body_fingerprint(SOURCE).unwrap(),
        )];
        assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &paths));
        assert!(!vault
            .record_unusable_output(TARGET, &content, REV, &seen, &paths)
            .unwrap());
        assert!(
            vault.has_unconsidered_sources(TARGET, &content, REV, &paths),
            "第一次不可用：仍算没看过，后台会重试"
        );
        assert!(vault
            .record_unusable_output(TARGET, &content, REV, &seen, &paths)
            .unwrap());
        assert!(
            !vault.has_unconsidered_sources(TARGET, &content, REV, &paths),
            "同一版第二次不可用：记为看过，不再白调模型"
        );

        // 成功一次会清掉计数：之后再不可用又从第一次算起。
        let (_d2, other, _p2, _) = fixture();
        let content = other.read(TARGET).unwrap().content;
        let seen = vec![crate::SourceCoverage::whole(
            SOURCE,
            other.body_fingerprint(SOURCE).unwrap(),
        )];
        other
            .record_unusable_output(TARGET, &content, REV, &seen, &paths)
            .unwrap();
        other
            .record_considered_sources(TARGET, &content, REV, &seen, &paths)
            .unwrap();
        other
            .write(SOURCE, &format!("{SOURCE_BODY}\n补充：预算追加一成。\n"))
            .unwrap();
        let changed = vec![crate::SourceCoverage::whole(
            SOURCE,
            other.body_fingerprint(SOURCE).unwrap(),
        )];
        assert!(!other
            .record_unusable_output(TARGET, &content, REV, &changed, &paths)
            .unwrap());
        assert!(other.has_unconsidered_sources(TARGET, &content, REV, &paths));
    }

    /// 单篇超长来源分几轮读完（K3「长文本分块」）：一批放不下时记下读到第几个片段，下一轮
    /// 从断点接着读，读完才算看过；来源改了断点作废、从头读；没看过的来源排在读了一半的之前。
    #[test]
    fn a_long_source_is_read_across_rounds_from_where_it_stopped() {
        let (_dir, vault, _plan, _) = fixture();
        let paragraphs: Vec<String> = (0..30)
            .map(|index| format!("第{index:02}段{}", "甲".repeat(600)))
            .collect();
        let long = format!(
            "---\ntags: [项目]\n---\n# 长记录\n\n{}\n",
            paragraphs.join("\n\n")
        );
        vault.write("Notes/长记录.md", &long).unwrap();
        vault
            .write(
                "Notes/短消息.md",
                "---\ntags: [项目]\n---\n# 短消息\n\n场地已订。\n",
            )
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        let policy = crate::SourcePolicy::FORMAL_ONLY;
        let long_path = vec!["Notes/长记录.md".to_owned()];
        let paths = vec!["Notes/长记录.md".to_owned(), "Notes/短消息.md".to_owned()];

        let mut read = Vec::new();
        for round in 0..10 {
            let resume = vault.resume_points(TARGET, &content, REV);
            let batch = vault
                .topic_evidence_resuming(&long_path, policy, &resume)
                .unwrap();
            assert_eq!(batch.coverage.len(), 1);
            assert!(!batch.input_complete, "分段读的每一轮都不是整篇输入");
            read.extend(
                batch
                    .evidence
                    .iter()
                    .map(|item| item.excerpt.chars().take(4).collect::<String>()),
            );
            vault
                .record_considered_sources(TARGET, &content, REV, &batch.coverage, &paths)
                .unwrap();
            if round == 0 {
                assert!(batch.coverage[0].next_fragment.is_some(), "一批放不下整篇");
                // 读了一半的排在没看过的之后，不霸占下一批。
                assert_eq!(
                    vault.unconsidered_sources_first(TARGET, &content, REV, paths.clone()),
                    vec!["Notes/短消息.md".to_owned(), "Notes/长记录.md".to_owned()]
                );
            }
            if batch.coverage[0].next_fragment.is_none() {
                break;
            }
            assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &long_path));
        }
        let expected: Vec<String> = (0..30).map(|index| format!("第{index:02}段")).collect();
        assert_eq!(read, expected, "每段恰好读一次、按原顺序");
        assert!(!vault.has_unconsidered_sources(TARGET, &content, REV, &long_path));

        // 来源改了：断点作废，从头读。
        vault
            .write("Notes/长记录.md", &long.replace("第00段", "第〇段"))
            .unwrap();
        assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &long_path));
        let resume = vault.resume_points(TARGET, &content, REV);
        let batch = vault
            .topic_evidence_resuming(&long_path, policy, &resume)
            .unwrap();
        assert!(batch.evidence[0].excerpt.starts_with("第〇段"));
    }

    #[test]
    fn background_preparation_is_keyed_by_the_candidate_versions() {
        let (_dir, vault, _plan, _) = fixture();
        vault
            .write(
                "Notes/新进展.md",
                "---\ntags: [项目]\n---\n# 新进展\n\n预算已批准。\n",
            )
            .unwrap();
        let policy = crate::SourcePolicy::FORMAL_ONLY;
        assert_eq!(
            vault.topics_with_candidate("Notes/新进展.md", policy),
            vec![TARGET.to_owned()]
        );
        assert!(vault
            .topics_with_candidate("Notes/不存在.md", policy)
            .is_empty());
        vault
            .write(
                "Notes/旧消息.md",
                "---\ntags: [项目]\n---\n# 旧消息\n\n场地已订。\n",
            )
            .unwrap();
        let content = vault.read(TARGET).unwrap().content;
        let paths = vec!["Notes/新进展.md".to_owned(), "Notes/旧消息.md".to_owned()];
        assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &paths));
        // 模型看过「新进展」的这一版：它排到后面，没看过的「旧消息」先轮到。
        let seen = vec![crate::SourceCoverage::whole(
            "Notes/新进展.md",
            vault.body_fingerprint("Notes/新进展.md").unwrap(),
        )];
        vault
            .record_considered_sources(TARGET, &content, REV, &seen, &paths)
            .unwrap();
        assert_eq!(
            vault.unconsidered_sources_first(TARGET, &content, REV, paths.clone()),
            vec!["Notes/旧消息.md".to_owned(), "Notes/新进展.md".to_owned()]
        );
        assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &paths));
        // 两篇都看过、都没变：再触发也不调模型。
        let all: Vec<crate::SourceCoverage> = paths
            .iter()
            .map(|path| crate::SourceCoverage::whole(path, vault.body_fingerprint(path).unwrap()))
            .collect();
        vault
            .record_considered_sources(TARGET, &content, REV, &all, &paths)
            .unwrap();
        assert!(!vault.has_unconsidered_sources(TARGET, &content, REV, &paths));
        assert!(
            vault.has_unconsidered_sources(TARGET, &content, "topic_update@新模板", &paths),
            "提示词或流水线换了，旧口径的「看过」不沿用"
        );
        // 页上声明变了（应用了一份计划）：对旧声明集说的「没有新增」不沿用。
        let (_d2, other, plan, _) = fixture();
        let before = other.read(TARGET).unwrap().content;
        let versions = vec![crate::SourceCoverage::whole(
            SOURCE,
            other.body_fingerprint(SOURCE).unwrap(),
        )];
        let sources = vec![SOURCE.to_owned()];
        other
            .record_considered_sources(TARGET, &before, REV, &versions, &sources)
            .unwrap();
        assert!(!other.has_unconsidered_sources(TARGET, &before, REV, &sources));
        let sealed = other.seal_ingest_plan(plan).unwrap();
        other.apply_ingest_plan(&sealed).unwrap();
        let after = other.read(TARGET).unwrap().content;
        assert!(other.has_unconsidered_sources(TARGET, &after, REV, &sources));
        // 来源正文变了就又算没看过。
        vault
            .write(
                "Notes/新进展.md",
                "---\ntags: [项目]\n---\n# 新进展\n\n预算已追加。\n",
            )
            .unwrap();
        assert!(vault.has_unconsidered_sources(TARGET, &content, REV, &paths));
        // 范围变了，看过的记录作废（对另一份范围说的话不能沿用）。
        let rescoped = content.replacen("title: 项目", "title: 项目（新范围）", 1);
        assert_ne!(rescoped, content, "夹具须真的改了范围");
        assert_eq!(
            vault.unconsidered_sources_first(
                TARGET,
                &rescoped,
                REV,
                vec!["Notes/旧消息.md".to_owned()]
            ),
            vec!["Notes/旧消息.md".to_owned()]
        );
        assert!(vault.has_unconsidered_sources(
            TARGET,
            &rescoped,
            REV,
            &["Notes/旧消息.md".to_owned()]
        ));
    }

    #[test]
    fn sealed_plans_are_listed_per_target_newest_first() {
        let (_dir, vault, plan, _) = fixture();
        let first = vault.seal_ingest_plan(plan.clone()).unwrap();
        vault.persist_sealed_ingest_plan(&first).unwrap();
        let mut again = plan;
        again.operation_id = Uuid::now_v7();
        let second = vault.seal_ingest_plan(again).unwrap();
        vault.persist_sealed_ingest_plan(&second).unwrap();
        assert_eq!(
            vault.sealed_plan_ids_for_target(TARGET),
            vec![second.operation_id, first.operation_id]
        );
        assert!(vault
            .sealed_plan_ids_for_target("Knowledge/别的.md")
            .is_empty());
        vault
            .discard_sealed_ingest_plan(second.operation_id)
            .unwrap();
        assert_eq!(
            vault.sealed_plan_ids_for_target(TARGET),
            vec![first.operation_id]
        );
    }

    fn update_evidence(vault: &Vault) -> Vec<crate::TopicEvidenceInput> {
        vec![crate::TopicEvidenceInput {
            id: "e0000".into(),
            excerpt: "原型已完成".into(),
            kind: crate::TopicEvidenceKind::FormalNote,
            source: PlanEvidence {
                source_path: SOURCE.into(),
                source_title: "会议".into(),
                source_hash: vault.body_fingerprint(SOURCE).unwrap(),
                locator: locator(),
                complete_input: true,
            },
        }]
    }

    fn revisions() -> PlanRevisions {
        PlanRevisions {
            model_revision: "model-1".into(),
            prompt_revision: "prompt-x".into(),
            pipeline_revision: "topic-update-v1".into(),
        }
    }

    #[test]
    fn topic_update_seals_append_and_conflict_into_one_previewable_plan() {
        let (_dir, vault, _plan, existing_id) = fixture();
        let context = vault.topic_update_context(TARGET).unwrap();
        assert_eq!(context.existing.len(), 1);
        assert_eq!(context.existing[0].id, existing_id);
        assert_eq!(context.existing[0].text, "原型已完成");
        let evidence = update_evidence(&vault);
        let append = crate::GroundedUpdate {
            claim: crate::GroundedClaim {
                id: Uuid::now_v7(),
                text: "原型已进入内测".into(),
                kind: ClaimKind::Synthesis,
                evidence_indices: vec![0],
            },
            conflicts_with: None,
            replaces: None,
        };
        let conflict = crate::GroundedUpdate {
            claim: crate::GroundedClaim {
                id: Uuid::now_v7(),
                text: "另一份记录称原型尚未完成".into(),
                kind: ClaimKind::Conflict,
                evidence_indices: vec![0],
            },
            conflicts_with: Some(existing_id),
            replaces: None,
        };
        let before = vault.read(TARGET).unwrap().content;
        let sealed = vault
            .seal_topic_update_plan(
                context.baseline.clone(),
                &evidence,
                true,
                &[append, conflict],
                revisions(),
            )
            .unwrap();
        assert_eq!(sealed.evidence.len(), 1, "两条声明共享同一片段只收录一次");
        assert!(matches!(
            sealed.actions[0],
            IngestAction::AppendClaim { .. }
        ));
        assert!(matches!(
            sealed.actions[1],
            IngestAction::MarkConflict { related_id, .. } if related_id == existing_id
        ));
        assert_eq!(sealed.prompt_revision, "prompt-x");
        let preview = vault.preview_ingest_plan(&sealed).unwrap();
        assert!(preview.contains("原型已进入内测") && preview.contains("另一份记录称原型尚未完成"));
        assert!(
            preview.contains("原型已完成"),
            "既有声明保留，分歧并列而不是覆盖"
        );
        assert_eq!(vault.read(TARGET).unwrap().content, before, "封存不写主题");
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        assert!(matches!(
            vault.apply_ingest_plan(&sealed),
            Ok(PlanApplyOutcome::Applied { .. })
        ));
        // 写入后分歧仍知道自己与哪条并列；普通追加不写这个键。
        let knowledge =
            crate::validate_topic_document(&vault.read(TARGET).unwrap().content).unwrap();
        let conflict_id = match &sealed.actions[1] {
            IngestAction::MarkConflict { claim, .. } => claim.id.to_string(),
            _ => unreachable!(),
        };
        let append_id = match &sealed.actions[0] {
            IngestAction::AppendClaim { claim } => claim.id.to_string(),
            _ => unreachable!(),
        };
        assert_eq!(
            knowledge.claims[&conflict_id].conflicts_with,
            Some(existing_id.to_string())
        );
        assert_eq!(knowledge.claims[&append_id].conflicts_with, None);
        assert_eq!(
            knowledge.claims[&existing_id.to_string()].conflicts_with,
            None
        );
    }

    /// K4 替换：更新版只换原声明那一块、沿用原 ID，块外字节与其它声明不动；应用后仍可维护；
    /// 等待期间原声明被人改过即拒绝；撤销逐字节回到写入前。
    #[test]
    fn topic_update_replaces_one_block_in_place_and_undo_restores_it() {
        let (_dir, vault, _plan, existing_id) = fixture();
        let context = vault.topic_update_context(TARGET).unwrap();
        let before = vault.read(TARGET).unwrap().content;
        let replacement = crate::GroundedUpdate {
            claim: crate::GroundedClaim {
                id: existing_id,
                text: "原型已进入内测".into(),
                kind: ClaimKind::Synthesis,
                evidence_indices: vec![0],
            },
            conflicts_with: None,
            replaces: Some(existing_id),
        };
        let sealed = vault
            .seal_topic_update_plan(
                context.baseline.clone(),
                &update_evidence(&vault),
                true,
                std::slice::from_ref(&replacement),
                revisions(),
            )
            .unwrap();
        assert!(matches!(
            sealed.actions[0],
            IngestAction::ReplaceClaim { original_id, .. } if original_id == existing_id
        ));
        let preview = vault.preview_ingest_plan(&sealed).unwrap();
        // 元数据里的来源引句仍是来源原话，只看正文的声明行。
        let body = crate::body_of(&preview);
        assert!(
            body.contains("- 原型已进入内测") && !body.contains("- 原型已完成"),
            "{body}"
        );
        vault.persist_sealed_ingest_plan(&sealed).unwrap();
        vault.apply_ingest_plan(&sealed).unwrap();
        let after = vault.read(TARGET).unwrap().content;
        let knowledge = crate::validate_topic_document(&after).unwrap();
        assert_eq!(knowledge.claims.len(), 1, "替换不增加声明");
        assert!(knowledge.claims.contains_key(&existing_id.to_string()));
        assert_eq!(
            vault.topic_maintenance_state(TARGET).unwrap(),
            TopicMaintenanceState::Ready
        );
        let before_body = crate::body_of(&before);
        let after_body = crate::body_of(&after);
        let head = before_body
            .find(&format!("<!-- luna:claim:{existing_id} -->"))
            .unwrap();
        assert_eq!(&after_body[..head], &before_body[..head], "块前字节不动");

        // 撤销：逐字节回到写入前。
        assert_eq!(
            vault.undo_applied_plan(
                TARGET,
                &ContentHash::of(before.as_bytes()).to_string(),
                &sealed.result_hash
            ),
            Ok(PlanUndoOutcome::Restored)
        );
        assert_eq!(vault.read(TARGET).unwrap().content, before);

        // 等待期间原声明被改：封存拒绝，不基于新正文算块指纹。
        let context = vault.topic_update_context(TARGET).unwrap();
        std::fs::write(
            vault.root().join(TARGET),
            before.replacen("原型已完成", "原型已完成（人工补注）", 1),
        )
        .unwrap();
        assert!(vault
            .seal_topic_update_plan(
                context.baseline,
                &update_evidence(&vault),
                true,
                &[replacement],
                revisions(),
            )
            .is_err());
    }

    #[test]
    fn topic_update_refuses_targets_that_changed_or_are_not_ready() {
        let (dir, vault, _plan, _) = fixture();
        let context = vault.topic_update_context(TARGET).unwrap();
        let evidence = update_evidence(&vault);
        let append = crate::GroundedUpdate {
            claim: crate::GroundedClaim {
                id: Uuid::now_v7(),
                text: "新增".into(),
                kind: ClaimKind::Synthesis,
                evidence_indices: vec![0],
            },
            conflicts_with: None,
            replaces: None,
        };
        let original = vault.read(TARGET).unwrap().content;
        // 模型等待期间用户在外部编辑器里改了主题：封存必须拒绝，而不是基于新正文算块 hash。
        std::fs::write(
            dir.path().join(TARGET),
            original.replace("原型已完成", "用户改稿"),
        )
        .unwrap();
        assert_eq!(
            vault.seal_topic_update_plan(
                context.baseline,
                &evidence,
                true,
                std::slice::from_ref(&append),
                revisions(),
            ),
            Err(PlanRejection::TargetChanged)
        );
        assert_eq!(
            vault.topic_update_context(TARGET),
            Err(PlanRejection::TargetNotReady(
                TopicMaintenanceState::BodyChanged
            ))
        );
        assert_eq!(
            vault.topic_update_context("Knowledge/不存在.md"),
            Err(PlanRejection::TargetMissing)
        );
    }
}
