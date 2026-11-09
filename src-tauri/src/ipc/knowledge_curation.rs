//! 持续整理（ADR-134 / TDD-06 §3.9）的命令边界：封存计划的预览 / 确认 / 处置与队列应用、
//! 旧主题的显式升级与回退、计划与声明出处的定位打开。
//!
//! 从 `knowledge.rs` 纵切出来（TDD-15 §2.4 单文件命令上限）：判定全在 `luna_vault`，
//! 这里只做 IPC 装配、DTO 投影与写盘后的广播。共享的「按锚打开来源」留在 `knowledge.rs`，
//! 与对话出处同一条路。

use super::*;

/// 封存计划的拒绝码只由 Rust 判定；前端不得从错误 Display 或 Markdown 猜原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanRejectionCodeDto {
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
    /// 以下两项只在应用边界产生：确认未能写入持久队列，或排队中的应用尚未落定。
    IntentStoreUnavailable,
    ApplyPending,
}

impl From<luna_vault::PlanRejectionCode> for PlanRejectionCodeDto {
    fn from(value: luna_vault::PlanRejectionCode) -> Self {
        use luna_vault::PlanRejectionCode as Code;
        match value {
            Code::InvalidPlan => Self::InvalidPlan,
            Code::IncompleteOutput => Self::IncompleteOutput,
            Code::BudgetExceeded => Self::BudgetExceeded,
            Code::WrongVault => Self::WrongVault,
            Code::ScopeChanged => Self::ScopeChanged,
            Code::TargetChanged => Self::TargetChanged,
            Code::TargetNotReady => Self::TargetNotReady,
            Code::SourceChanged => Self::SourceChanged,
            Code::SourceMissing => Self::SourceMissing,
            Code::TargetMissing => Self::TargetMissing,
            Code::OriginalClaimChanged => Self::OriginalClaimChanged,
            Code::VaultReadFailed => Self::VaultReadFailed,
            Code::RenderFailed => Self::RenderFailed,
            Code::WriteFailed => Self::WriteFailed,
            Code::UserSuppressed => Self::UserSuppressed,
            Code::DecisionStoreUnavailable => Self::DecisionStoreUnavailable,
            Code::CommitLockUnavailable => Self::CommitLockUnavailable,
            Code::PlanStoreUnavailable => Self::PlanStoreUnavailable,
            Code::PlanMissing => Self::PlanMissing,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct PlanRejectionDto {
    pub code: PlanRejectionCodeDto,
    pub source_path: Option<String>,
    pub target_state: Option<TopicMaintenanceStateDto>,
    /// 重跑一次可能得到不同结果：来源、本页或范围在预览后变了，或模型这次的输出无效/被截断
    /// （模型输出有随机性）。界面据此给「重新检查」一键重算；维护暂停、用户否决、超出预算、
    /// 存储故障等重跑也得不到新结果的原因为 `false`。
    pub can_replan: bool,
}

impl PlanRejectionCodeDto {
    /// 「目标改变给重算」（TDD-07 §4.1e）的唯一判据：预览所依据的事实变了，或模型输出
    /// 本身不可用，才值得重跑模型。
    fn can_replan(self) -> bool {
        matches!(
            self,
            Self::InvalidPlan
                | Self::IncompleteOutput
                | Self::ScopeChanged
                | Self::TargetChanged
                | Self::SourceChanged
                | Self::SourceMissing
                | Self::OriginalClaimChanged
                | Self::PlanMissing
        )
    }
}

impl PlanRejectionDto {
    fn app(code: PlanRejectionCodeDto) -> Self {
        Self {
            code,
            source_path: None,
            target_state: None,
            can_replan: code.can_replan(),
        }
    }

    /// 模型等待期间有来源正文变化；按批复核只知道「有变化」，不点名具体路径。
    pub(crate) fn stale_sources() -> Self {
        Self::app(PlanRejectionCodeDto::SourceChanged)
    }
}

impl From<luna_vault::PlanRejection> for PlanRejectionDto {
    fn from(value: luna_vault::PlanRejection) -> Self {
        let target_state = match &value {
            luna_vault::PlanRejection::TargetNotReady(state) => Some((*state).into()),
            _ => None,
        };
        let code: PlanRejectionCodeDto = value.code().into();
        Self {
            code,
            source_path: value.affected_source_path().map(str::to_owned),
            target_state,
            can_replan: code.can_replan(),
        }
    }
}

/// 计划里一条动作的封闭类别；与 `IngestAction` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SealedPlanActionDto {
    Append,
    Replace,
    Conflict,
}

/// 计划出处的来源引用：路径用于打开原文，标题用于显示；不含定位细节与正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub(crate) struct PlanSourceRefDto {
    pub path: String,
    pub title: String,
    /// 该来源在本计划证据中的首个序号：打开出处时交回 Rust 取定位锚。
    pub evidence_index: u32,
}

/// 用户确认前核对的逐条内容；文本即将写进主题的那句（由 Rust 渲染时再转义）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct SealedPlanItemDto {
    pub action: SealedPlanActionDto,
    pub text: String,
    pub sources: Vec<PlanSourceRefDto>,
    /// 这条是「转写记载」：依据是录音里说过的话，不是已核实的事实（PRD-16-047）。
    pub transcript_record: bool,
    /// 这条**只由速记支撑**（「速记记载」，PRD-16-057）：依据是还没整理的随手记录。
    /// 由 Rust 按来源此刻的类型判定，不写进声明元数据——速记转为正式笔记后同一出处即不再标。
    pub quick_note_record: bool,
    /// 低风险（ADR-136 ③，Rust 确定性判定）：一条只引用正式笔记的新增——不是替换或分歧，
    /// 依据里没有转写或速记。「待你看」只许对全部条目都低风险的计划一键接受；这不是自动写入，
    /// 写入仍要用户点。
    pub low_risk: bool,
    /// 替换时是被替换的原文、分歧时是与之并列的原说法（确认前要看到改的是哪一条）；
    /// 新增为 `None`。
    pub related_text: Option<String>,
}

/// `existing`：目标页此刻的声明（ID → 文本），用来给替换与分歧带上原来那一条。
fn sealed_plan_items(
    plan: &luna_vault::IngestPlan,
    existing: &std::collections::HashMap<uuid::Uuid, String>,
    quick_notes: &std::collections::HashSet<String>,
) -> Vec<SealedPlanItemDto> {
    plan.actions
        .iter()
        .map(|action| {
            let (kind, claim, related) = match action {
                luna_vault::IngestAction::AppendClaim { claim } => {
                    (SealedPlanActionDto::Append, claim, None)
                }
                luna_vault::IngestAction::ReplaceClaim {
                    claim, original_id, ..
                } => (SealedPlanActionDto::Replace, claim, Some(original_id)),
                luna_vault::IngestAction::MarkConflict {
                    claim, related_id, ..
                } => (SealedPlanActionDto::Conflict, claim, Some(related_id)),
            };
            let mut item = plan_item(
                kind,
                &claim.text,
                claim.kind,
                &claim.evidence_indices,
                &plan.evidence,
                quick_notes,
            );
            item.related_text = related.and_then(|id| existing.get(id).cloned());
            item
        })
        .collect()
}

/// 一条声明的预览条目：同一来源只列一次，序号指向首个引用它的证据。增量计划与新初稿共用。
fn plan_item(
    action: SealedPlanActionDto,
    text: &str,
    kind: luna_vault::ClaimKind,
    evidence_indices: &[usize],
    evidence: &[luna_vault::PlanEvidence],
    quick_notes: &std::collections::HashSet<String>,
) -> SealedPlanItemDto {
    let mut sources: Vec<PlanSourceRefDto> = Vec::new();
    for index in evidence_indices {
        let Some(item) = evidence.get(*index) else {
            continue;
        };
        if !sources.iter().any(|known| known.path == item.source_path) {
            sources.push(PlanSourceRefDto {
                path: item.source_path.clone(),
                title: item.source_title.clone(),
                evidence_index: u32::try_from(*index).unwrap_or(u32::MAX),
            });
        }
    }
    let quick_note_record = !sources.is_empty()
        && sources
            .iter()
            .all(|source| quick_notes.contains(&source.path));
    let low_risk = action == SealedPlanActionDto::Append
        && kind == luna_vault::ClaimKind::Synthesis
        && !sources.is_empty()
        && sources
            .iter()
            .all(|source| !quick_notes.contains(&source.path));
    SealedPlanItemDto {
        action,
        text: text.to_owned(),
        sources,
        transcript_record: kind == luna_vault::ClaimKind::TranscriptRecord,
        quick_note_record,
        low_risk,
        related_text: None,
    }
}

/// 这份计划是不是每一条都低风险（见 [`SealedPlanItemDto::low_risk`]）。
fn plan_is_low_risk(vault: &luna_vault::Vault, plan: &luna_vault::IngestPlan) -> bool {
    let items = sealed_plan_items(
        plan,
        &std::collections::HashMap::new(),
        &quick_note_sources(vault, &plan.evidence),
    );
    !items.is_empty() && items.iter().all(|item| item.low_risk)
}

/// 记一次低风险计划的处置（影子资格，ADR-136 ③）；只计数，不改任何行为。
fn record_shadow(
    app: &tauri::AppHandle,
    vault: &luna_vault::Vault,
    event: crate::knowledge_shadow::ShadowEvent,
    operation_id: &str,
    result_hash: &str,
) {
    if let Ok(dir) = app.path().app_data_dir() {
        crate::knowledge_shadow::record(
            &dir,
            &vault.knowledge_vault_identity(),
            event,
            operation_id,
            result_hash,
        );
    }
}

/// 证据里此刻仍是速记的来源（「速记记载」标注的唯一判据）。读不出的来源不算速记。
fn quick_note_sources(
    vault: &luna_vault::Vault,
    evidence: &[luna_vault::PlanEvidence],
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for item in evidence {
        if out.contains(&item.source_path) {
            continue;
        }
        if vault
            .read(&item.source_path)
            .is_ok_and(|doc| doc.meta.note_type == luna_vault::NoteType::QuickNote)
        {
            out.insert(item.source_path.clone());
        }
    }
    out
}

/// 新建主题初稿的准备结果（`prepare_topic_draft`）。无候选、无证据、无新增都是结果，不是失败。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicDraftPlanDto {
    /// 这个标签已经有主题页：去那一页补充，不另造。
    Exists {
        path: String,
    },
    NoCandidates,
    NoEvidence {
        omitted_sources: u32,
    },
    NoOp {
        input_complete: bool,
        omitted_sources: u32,
    },
    Rejected {
        rejection: PlanRejectionDto,
    },
    Planned {
        operation_id: String,
        target_path: String,
        result_hash: String,
        items: Vec<SealedPlanItemDto>,
        input_complete: bool,
        omitted_sources: u32,
    },
}

pub(crate) fn topic_draft_planned(
    vault: &luna_vault::Vault,
    draft: &luna_vault::TopicDraft,
) -> TopicDraftPlanDto {
    let quick_notes = quick_note_sources(vault, &draft.evidence);
    TopicDraftPlanDto::Planned {
        operation_id: draft.operation_id.to_string(),
        target_path: draft.target_path.clone(),
        result_hash: draft.result_hash.clone(),
        items: draft
            .claims
            .iter()
            .map(|claim| {
                plan_item(
                    SealedPlanActionDto::Append,
                    &claim.text,
                    claim.kind,
                    &claim.evidence_indices,
                    &draft.evidence,
                    &quick_notes,
                )
            })
            .collect(),
        input_complete: draft.input_complete,
        omitted_sources: draft.omitted_sources,
    }
}

/// 后台已为这个标签备好、仍可直接发布的初稿（PRD-16-060）；没有即 `None`，对话框当场准备。
/// 只读：不调用模型、不写文件。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_draft_ready(
    state: tauri::State<'_, AppState>,
    tag: String,
) -> Result<Option<TopicDraftPlanDto>, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .ready_topic_draft_for(tag.trim())
        .map(|draft| topic_draft_planned(vault, &draft)))
}

/// 确认发布新主题初稿的结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicDraftPublishDto {
    Published { path: String },
    Rejected { rejection: PlanRejectionDto },
}

/// 发布用户确认的那份初稿：凭 operation ID 与预览指纹写同一份字节，不重跑模型。
/// 写前登记知识生命周期（嵌入、地图刷新与回执），发布失败即撤销登记。
#[tauri::command]
#[specta::specta]
pub(crate) fn confirm_topic_draft(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    operation_id: String,
    result_hash: String,
) -> Result<TopicDraftPublishDto, String> {
    let rejected = |rejection: luna_vault::PlanRejection| {
        Ok(TopicDraftPublishDto::Rejected {
            rejection: rejection.into(),
        })
    };
    let Ok(id) = uuid::Uuid::parse_str(&operation_id) else {
        return rejected(luna_vault::PlanRejection::InvalidPlan);
    };
    let (published, intent, written, accepted) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let draft = match vault.load_topic_draft(id) {
            Ok(draft) => draft,
            Err(rejection) => return rejected(rejection),
        };
        // 建议「已接受」与展示时同一枚令牌（标签 + 当时够格的知识笔记集），发布前算：
        // 发布后这一标签就有主题页了，不再是建议。
        let accepted = vault
            .tag_knowledge_notes(&draft.topic_src)
            .ok()
            .map(|notes| {
                (
                    vault.root().display().to_string(),
                    super::knowledge::suggestion_token(
                        vault.root(),
                        "topic",
                        &draft.topic_src,
                        &notes.join("\n"),
                    ),
                )
            });
        let body = luna_vault::body_of(&draft.content).to_owned();
        let intent = super::knowledge::prepare_knowledge_write_locked(
            &app,
            vault,
            &draft.target_path,
            &body,
            "topic_draft",
            true,
            true,
        )?
        .with_topic_src(&draft.topic_src);
        match vault.publish_topic_draft(id, &result_hash) {
            Ok(published) => {
                let written = written_contents(vault, std::slice::from_ref(&published.path));
                (published, intent, written, accepted)
            }
            Err(rejection) => {
                super::knowledge::abandon_knowledge_write(&app, intent);
                return rejected(rejection);
            }
        }
    };
    announce_topic_written(&app, written);
    if published.already_published {
        // 上次已发布、崩溃在清理前：文件是同一份字节，生命周期那一趟已登记过。
        super::knowledge::abandon_knowledge_write(&app, intent);
    } else {
        super::knowledge::commit_knowledge_write(&app, intent)?;
        if let Some((root, token)) = accepted {
            super::knowledge::record_suggestion_metric(
                &app,
                &root,
                &token,
                luna_metrics::SuggestionEvent::Accepted,
                None,
            );
        }
    }
    Ok(TopicDraftPublishDto::Published {
        path: published.path,
    })
}

/// 用户不要这份初稿：只删设备本地载荷，不写任何笔记，也不记抑制。
#[tauri::command]
#[specta::specta]
pub(crate) fn discard_topic_draft(
    state: tauri::State<'_, AppState>,
    operation_id: String,
) -> Result<(), String> {
    let Ok(id) = uuid::Uuid::parse_str(&operation_id) else {
        return Ok(());
    };
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .discard_topic_draft(id)
        .map_err(|error| error.to_string())
}

/// 同一 operation ID 只能取回已封存的预览；这条命令不调用模型、不写主题。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum SealedIngestPreviewDto {
    Ready {
        operation_id: String,
        target_path: String,
        /// 写入前这一页的整文件指纹（计划的底版）：应用后「撤销」凭它找回写入前那一版。
        before_hash: String,
        result_hash: String,
        markdown: String,
        items: Vec<SealedPlanItemDto>,
        ready_item_count: KnowledgeCountDto,
        evidence_count: u32,
    },
    Rejected {
        rejection: PlanRejectionDto,
    },
}

// 预览回传上限；计划本身的 1 MiB 上限不约束目标页既有正文大小。
const MAX_SEALED_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn sealed_ingest_preview_of(
    vault: &luna_vault::Vault,
    operation_id: &str,
) -> SealedIngestPreviewDto {
    let result = (|| {
        let id = uuid::Uuid::parse_str(operation_id)
            .map_err(|_| luna_vault::PlanRejection::InvalidPlan)?;
        let plan = vault.load_sealed_ingest_plan(id)?;
        let markdown = vault.preview_ingest_plan(&plan)?;
        if markdown.len() > MAX_SEALED_PREVIEW_BYTES {
            return Err(luna_vault::PlanRejection::BudgetExceeded);
        }
        // 预览通过即说明目标页仍是计划底版，按底版取原来的声明文本。
        let existing = vault
            .read(&plan.baseline.target_path)
            .ok()
            .and_then(|doc| luna_vault::topic_claim_statements(&doc.content).ok())
            .map(|statements| {
                statements
                    .into_iter()
                    .map(|statement| (statement.id, statement.text))
                    .collect()
            })
            .unwrap_or_default();
        let items = sealed_plan_items(&plan, &existing, &quick_note_sources(vault, &plan.evidence));
        Ok(SealedIngestPreviewDto::Ready {
            operation_id: id.to_string(),
            before_hash: plan.baseline.target_hash,
            target_path: plan.baseline.target_path,
            result_hash: plan.result_hash,
            markdown,
            items,
            ready_item_count: KnowledgeCountDto::Exact {
                count: u32::try_from(plan.actions.len()).unwrap_or(u32::MAX),
            },
            evidence_count: u32::try_from(plan.evidence.len()).unwrap_or(u32::MAX),
        })
    })();
    result.unwrap_or_else(|rejection| SealedIngestPreviewDto::Rejected {
        rejection: rejection.into(),
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn preview_sealed_ingest_plan(
    state: tauri::State<'_, AppState>,
    operation_id: String,
) -> Result<SealedIngestPreviewDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(sealed_ingest_preview_of(vault, &operation_id))
}

/// 按主题范围增量生成的结果（建议路径）。`Planned` 的预览与按 operation ID 取回的
/// 预览是同一份封存载荷；这条路径只封存、不应用，写入只能经用户确认进入持久队列。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicIngestPlanDto {
    /// 范围内没有尚未被本页引用的来源（与目录候选计数同一判据）。
    NoCandidates,
    /// 候选来源里没有可用作证据的正式笔记片段。
    NoEvidence {
        omitted_sources: u32,
    },
    /// 无可添加条目：`duplicates = 0` 表示模型判无新增，否则是与既有声明逐字重复的条数。
    NoOp {
        duplicates: u32,
        input_complete: bool,
        omitted_sources: u32,
        /// 还有没读到的内容，再检查一次会接着看：有来源没轮到这一批，或超长来源只读了前半。
        /// 界面据此不说「没有」，而不是自己拿计数去推。
        more_to_read: bool,
    },
    Planned {
        preview: SealedIngestPreviewDto,
        duplicates: u32,
        input_complete: bool,
        omitted_sources: u32,
    },
    Rejected {
        rejection: PlanRejectionDto,
    },
}

/// 模型返回之后、仍持 Vault 锁时的封存步骤：结果只在这里变成计划并持久化，
/// 返回的预览再经按 ID 取回的同一条路径生成，避免「生成时一份、确认时另一份」。
pub(crate) fn plan_from_topic_update_locked(
    vault: &luna_vault::Vault,
    context: luna_vault::TopicUpdateContext,
    batch: &luna_vault::TopicEvidenceBatch,
    update: luna_vault::TopicUpdate,
    revisions: luna_vault::PlanRevisions,
    is_applying: impl Fn(&str) -> bool,
) -> TopicIngestPlanDto {
    let (claims, duplicates) = match update {
        luna_vault::TopicUpdate::NoOp { .. } => (Vec::new(), 0),
        luna_vault::TopicUpdate::AllDuplicates { duplicates } => (Vec::new(), duplicates),
        luna_vault::TopicUpdate::Claims { claims, duplicates } => (claims, duplicates),
    };
    if claims.is_empty() {
        return TopicIngestPlanDto::NoOp {
            duplicates,
            input_complete: batch.input_complete,
            omitted_sources: batch.omitted_sources,
            more_to_read: batch.omitted_sources > 0
                || batch
                    .coverage
                    .iter()
                    .any(|source| source.next_fragment.is_some()),
        };
    }
    let sealed = vault
        .seal_topic_update_plan(
            context.baseline,
            &batch.evidence,
            batch.input_complete,
            &claims,
            revisions,
        )
        .and_then(|plan| vault.persist_sealed_ingest_plan(&plan).map(|()| plan));
    match sealed {
        Ok(plan) => {
            // 同一主题的旧计划已被这一份取代：它们基于同一或更早的底版，留着只会在
            // 回到页面时冒出一份过期计划。正在排队应用的那份不动，由队列终态收口。
            for older in vault.sealed_plan_ids_for_target(&plan.baseline.target_path) {
                if older != plan.operation_id && !is_applying(&older.to_string()) {
                    let _ = vault.discard_sealed_ingest_plan(older);
                }
            }
            TopicIngestPlanDto::Planned {
                preview: sealed_ingest_preview_of(vault, &plan.operation_id.to_string()),
                duplicates,
                input_complete: batch.input_complete,
                omitted_sources: batch.omitted_sources,
            }
        }
        Err(rejection) => TopicIngestPlanDto::Rejected {
            rejection: rejection.into(),
        },
    }
}

/// 本设备对当前库的来源范围（PRD-16-048）：正式笔记恒在，录音/转写须有设备私有的授权回执。
/// 回执读不出或应用数据目录不可用时一律按未授权处理。
pub(crate) fn source_policy(
    app: &tauri::AppHandle,
    vault: &luna_vault::Vault,
) -> luna_vault::SourcePolicy {
    let transcripts = transcript_source_state(app, vault)
        == Some(crate::knowledge_source_grant::TranscriptSourceState::Granted);
    // 速记恒可作证据（ADR-136 ①）：它是用户自己的话、只在本机，不涉及第三方的声音，
    // 也不改变它「待整理」的身份。
    luna_vault::SourcePolicy {
        transcripts,
        quick_notes: true,
    }
}

/// 录音/转写作为主题来源的现场：是否已授权，以及本页有几篇已完成权威转写、共享标签、
/// 尚未引用的录音/转写。未授权而 `available > 0` 时界面才说明影响并请求一次确认。
#[derive(Debug, Clone, Copy, Serialize, Type)]
pub(crate) struct TranscriptSourcesDto {
    pub authorized: bool,
    pub available: u32,
}

/// 此刻 AI 执行落点的类别（授权回执绑定它，ADR-136 ②）：主题整理走对话能力的路由。
fn current_compute_class(app: &tauri::AppHandle) -> &'static str {
    let state = app.state::<AppState>();
    crate::knowledge_source_grant::compute_class(super::models::current_chat_compute_location(
        app, &state,
    ))
}

/// 本设备对这个库、在当前落点下的录音/转写授权状态；应用数据目录不可用即 `None`（按未授权）。
fn transcript_source_state(
    app: &tauri::AppHandle,
    vault: &luna_vault::Vault,
) -> Option<crate::knowledge_source_grant::TranscriptSourceState> {
    let dir = app.path().app_data_dir().ok()?;
    Some(crate::knowledge_source_grant::transcript_source_state(
        &dir,
        &vault.knowledge_vault_identity(),
        current_compute_class(app),
    ))
}

/// 本设备对当前库使用录音/转写作为主题来源的决定（设备私有回执，不进 Vault）：
/// 允许、暂不（不再逐条询问）或撤回（回到从未问过）。三者都绑定此刻的算力落点。
#[derive(Debug, Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TranscriptSourceDecisionDto {
    Allow,
    NotNow,
    Revoke,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn set_transcript_sources(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    decision: TranscriptSourceDecisionDto,
) -> Result<(), String> {
    use crate::knowledge_source_grant::TranscriptSourceState as S;
    let identity = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.knowledge_vault_identity()
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|_| coded_error(&app, "SOURCE_GRANT_UNAVAILABLE", "source-grant-unavailable"))?;
    let next = match decision {
        TranscriptSourceDecisionDto::Allow => S::Granted,
        TranscriptSourceDecisionDto::NotNow => S::Declined,
        TranscriptSourceDecisionDto::Revoke => S::Unasked,
    };
    crate::knowledge_source_grant::set_transcript_source_state(
        &dir,
        &identity,
        current_compute_class(&app),
        next,
        super::checkup::now_ms(),
    )
    .map_err(|error| {
        eprintln!("[knowledge] 来源授权回执写入失败：{error}");
        coded_error(&app, "SOURCE_GRANT_UNAVAILABLE", "source-grant-unavailable")
    })
}

/// 录音/转写页上关于「它能不能成为主题依据」要说的话（PRD-16-058 / PRD-16-065）。
#[derive(Debug, Clone, Copy, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TranscriptKnowledgeNoticeDto {
    /// 没什么要说（已授权、选过暂不、不是录音/转写，或正文为空）。
    None,
    /// 只有录音时的实时稿：它不作为主题依据，重新转写一次即可（PRD-16-065）。
    PreviewOnly,
    /// 已完成权威转写、而这台设备在当前落点下还没回答过：在这里问一次（PRD-16-058）。
    /// `location`：此刻 AI 在哪处理，说明文案据此直说。
    Ask {
        location: crate::cloud::ComputeLocation,
    },
}

#[tauri::command]
#[specta::specta]
pub(crate) fn transcript_knowledge_notice(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<TranscriptKnowledgeNoticeDto, String> {
    use crate::knowledge_source_grant::TranscriptSourceState as S;
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let doc = vault.read(&note_rel).map_err(vault_err)?;
    if !doc.meta.note_type.is_source_material() || doc.meta.note_type == luna_vault::NoteType::Chat
    {
        return Ok(TranscriptKnowledgeNoticeDto::None);
    }
    if !luna_vault::has_authoritative_transcript(&doc.content) {
        // 空稿（录音刚开始、什么都没说）不值得一句提示；有实时稿才说它不作依据。
        return Ok(if luna_vault::has_transcript_text(&doc.content) {
            TranscriptKnowledgeNoticeDto::PreviewOnly
        } else {
            TranscriptKnowledgeNoticeDto::None
        });
    }
    Ok(match transcript_source_state(&app, vault) {
        Some(S::Unasked) => TranscriptKnowledgeNoticeDto::Ask {
            location: super::models::current_chat_compute_location(&app, &state),
        },
        _ => TranscriptKnowledgeNoticeDto::None,
    })
}

/// 应用阶段：面板「已加入队列」之后据此落定为「已写入」或带失败说明接回计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicApplyPhaseDto {
    /// 没有应用在排队，也没有失败待处理。
    Idle,
    /// 有一件应用在排队或运行。
    Queued,
    /// `pending` 那份计划上一次应用以失败结束（环境性故障用尽尝试预算，或写前复核被拒）；
    /// 载荷保留，可再次确认。
    Failed,
}

/// 主题页持续整理面板的现场：维护状态、仍在盘上的最近一份计划（按当前文件重新核对）
/// 与应用阶段。界面离开再回来时据此恢复，不重跑模型。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicCurationStatusDto {
    pub maintenance: TopicMaintenanceStateDto,
    /// 最近一份未在应用中的封存计划；过期时其预览为 `rejected`，界面据此提供丢弃。
    pub pending: Option<TopicIngestPlanDto>,
    /// `pending` 那份计划的 operation ID：过期时界面凭它丢弃载荷，不必再跑模型。
    pub pending_operation_id: Option<String>,
    pub apply_phase: TopicApplyPhaseDto,
    pub transcript_sources: TranscriptSourcesDto,
    /// 处理队列里正有一件「准备整理建议」在排队或运行（后台触发的那一件）。界面据此说
    /// 「正在后台准备」而不是再给一枚会重复调模型的「检查可添加内容」。
    pub background_preparing: bool,
}

fn topic_curation_status_of(
    vault: &luna_vault::Vault,
    topic_path: &str,
    transcripts_authorized: bool,
    apply_state: impl Fn(&str) -> Option<crate::processing_queue::PlanApplyState>,
) -> Result<TopicCurationStatusDto, String> {
    use crate::processing_queue::PlanApplyState;
    let maintenance = vault
        .topic_maintenance_state(topic_path)
        .map_err(vault_err)?;
    let mut apply_phase = TopicApplyPhaseDto::Idle;
    let mut pending = None;
    let mut pending_operation_id = None;
    for id in vault.sealed_plan_ids_for_target(topic_path) {
        let id_text = id.to_string();
        let state = apply_state(&id_text);
        if state == Some(PlanApplyState::Active) {
            apply_phase = TopicApplyPhaseDto::Queued;
            continue;
        }
        // 已成功应用、载荷尚待清理：不是待确认，也不是过期。
        if state == Some(PlanApplyState::Applied) {
            continue;
        }
        if pending.is_none() {
            if state == Some(PlanApplyState::Failed) && apply_phase == TopicApplyPhaseDto::Idle {
                apply_phase = TopicApplyPhaseDto::Failed;
            }
            let input_complete = vault
                .load_sealed_ingest_plan(id)
                .map(|plan| plan.input_complete)
                .unwrap_or(false);
            pending = Some(TopicIngestPlanDto::Planned {
                preview: sealed_ingest_preview_of(vault, &id_text),
                // 生成时的重复与遗漏计数不随载荷保存；恢复时如实给 0，界面只按
                // `input_complete` 提示覆盖不完整，不编一个数字。
                duplicates: 0,
                input_complete,
                omitted_sources: 0,
            });
            pending_operation_id = Some(id_text);
        }
    }
    Ok(TopicCurationStatusDto {
        maintenance: maintenance.into(),
        pending,
        pending_operation_id,
        apply_phase,
        background_preparing: false,
        transcript_sources: TranscriptSourcesDto {
            authorized: transcripts_authorized,
            available: vault
                .read(topic_path)
                .and_then(|doc| vault.topic_transcript_candidates_of(topic_path, &doc.content))
                .map_or(0, |paths| u32::try_from(paths.len()).unwrap_or(u32::MAX)),
        },
    })
}

/// 「待你看」里一页主题的待确认内容（PRD-16-059）：后台已准备好、仍然有效的那份封存计划。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ReviewGroupDto {
    pub topic_path: String,
    pub topic_title: String,
    pub operation_id: String,
    /// 确认与撤销要回传的两个指纹，与主题页面板同一份。
    pub before_hash: String,
    pub result_hash: String,
    pub items: Vec<SealedPlanItemDto>,
    /// 每一条都低风险：可在批量接受里一并应用（仍须用户点）。
    pub low_risk: bool,
    /// 这一批只看了一部分来源（还有没轮到的，或有来源只读了开头）。
    pub input_complete: bool,
}

/// 知识库首页「待你看」：各主题页上等你确认的整理建议，按主题分组（PRD-16-059）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct KnowledgeReviewInboxDto {
    pub groups: Vec<ReviewGroupDto>,
    /// 超出上限没列出的主题页数。
    pub omitted: u32,
}

/// 「待你看」一次最多列几页：这是首页上的一块，不是第二个主题目录。
const REVIEW_INBOX_LIMIT: usize = 20;

/// 只读：不调用模型、不写主题。列出每页主题最近一份仍然有效、没在排队应用的封存计划；
/// 失效的计划不在这里列（回到那一页会如实说明并给重新检查）。
#[tauri::command]
#[specta::specta]
pub(crate) fn knowledge_review_inbox(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<KnowledgeReviewInboxDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    review_inbox_of(
        vault,
        |id| state.processing.plan_apply_state(id),
        |operation_id, result_hash| {
            record_shadow(
                &app,
                vault,
                crate::knowledge_shadow::ShadowEvent::Shown,
                operation_id,
                result_hash,
            );
        },
    )
}

/// `apply_state`：队列里这份计划的应用阶段；排队中或已应用的不列。`shown`：列出了一份
/// 低风险计划（影子资格只数这一类被看到）。
fn review_inbox_of(
    vault: &luna_vault::Vault,
    apply_state: impl Fn(&str) -> Option<crate::processing_queue::PlanApplyState>,
    mut shown: impl FnMut(&str, &str),
) -> Result<KnowledgeReviewInboxDto, String> {
    use crate::processing_queue::PlanApplyState;
    let mut groups = Vec::new();
    let mut omitted = 0u32;
    let mut topics = vault.topic_pages().map_err(vault_err)?;
    topics.sort_by_key(|path| luna_sync_core::links::title_of(path));
    for topic_path in topics {
        let mut ready = None;
        for id in vault.sealed_plan_ids_for_target(&topic_path) {
            let id_text = id.to_string();
            if matches!(
                apply_state(&id_text),
                Some(PlanApplyState::Active | PlanApplyState::Applied)
            ) {
                continue;
            }
            if let SealedIngestPreviewDto::Ready {
                operation_id,
                before_hash,
                result_hash,
                items,
                ..
            } = sealed_ingest_preview_of(vault, &id_text)
            {
                let input_complete = vault
                    .load_sealed_ingest_plan(id)
                    .is_ok_and(|plan| plan.input_complete);
                ready = Some((
                    operation_id,
                    before_hash,
                    result_hash,
                    items,
                    input_complete,
                ));
                break;
            }
        }
        let Some((operation_id, before_hash, result_hash, items, input_complete)) = ready else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        if groups.len() >= REVIEW_INBOX_LIMIT {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let low_risk = items.iter().all(|item| item.low_risk);
        if low_risk {
            shown(&operation_id, &result_hash);
        }
        groups.push(ReviewGroupDto {
            topic_title: luna_sync_core::links::title_of(&topic_path).to_owned(),
            topic_path,
            operation_id,
            before_hash,
            result_hash,
            items,
            low_risk,
            input_complete,
        });
    }
    Ok(KnowledgeReviewInboxDto { groups, omitted })
}

/// 主题页持续整理的只读现场查询；不调用模型、不写文件。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_curation_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    topic_path: String,
) -> Result<TopicCurationStatusDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let authorized = source_policy(&app, vault).transcripts;
    let mut status = topic_curation_status_of(vault, &topic_path, authorized, |id| {
        state.processing.plan_apply_state(id)
    })?;
    status.background_preparing = state.processing.has_active(&topic_path, "topic_prepare");
    Ok(status)
}

/// 主题范围（阅读页头第一行）：页面回答的是哪一块、收哪些标签。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicScopeDto {
    pub title: String,
    pub description: String,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
}

/// 各状态的出处条数；同一声明两次引用同一来源算两条。
#[derive(Debug, Clone, Copy, Serialize, Type)]
pub(crate) struct TopicEvidenceTallyDto {
    pub current: u32,
    pub source_changed: u32,
    pub missing: u32,
    pub ambiguous: u32,
    pub partial: u32,
}

/// 一个来源在一种非当前状态下影响的声明数。`source_path` 为空 = 出处链接无法解析。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicSourceAttentionDto {
    pub source_path: Option<String>,
    pub source_title: Option<String>,
    pub state: TopicEvidenceStateDto,
    pub claims: u32,
}

/// 新格式主题的阅读汇总；判定全在 Rust，前端只按「有内容才显示」渲染。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicReadingSummaryDto {
    pub scope: TopicScopeDto,
    pub claims: u32,
    pub conflicts: u32,
    pub evidence: TopicEvidenceTallyDto,
    /// 需关注的来源，已按严重程度排序且有上限；超出的只计入 `attention_omitted`。
    pub attention: Vec<TopicSourceAttentionDto>,
    pub attention_omitted: u32,
    /// 至少有一条出处需要核对的声明数（去重）；「只看需核对」按钮的计数。
    pub attention_claims: u32,
    /// 声明块之外、没有逐条出处的内容行数：页头标「未核验」。
    pub unverified_lines: u32,
    /// 「转写记载」声明数：依据是录音里说过的话，不是已核实的事实。
    pub transcript_records: u32,
}

fn topic_reading_summary_of(
    vault: &luna_vault::Vault,
    topic_path: &str,
) -> Result<Option<TopicReadingSummaryDto>, String> {
    let summary = match vault.topic_reading_summary(topic_path) {
        Ok(summary) => summary,
        // 旧格式、未知 schema、坏证据：没有汇总，不是「零条声明」。维护状态由面板说明。
        Err(luna_vault::VaultError::InvalidReplace(_)) => return Ok(None),
        Err(error) => return Err(vault_err(error)),
    };
    let evidence = summary.evidence;
    Ok(Some(TopicReadingSummaryDto {
        scope: TopicScopeDto {
            title: summary.scope.title,
            description: summary.scope.description,
            includes: summary.scope.includes,
            excludes: summary.scope.excludes,
        },
        claims: summary.claims,
        conflicts: summary.conflicts,
        evidence: TopicEvidenceTallyDto {
            current: evidence.current,
            source_changed: evidence.source_changed,
            missing: evidence.missing,
            ambiguous: evidence.ambiguous,
            partial: evidence.partial,
        },
        attention: summary
            .attention
            .into_iter()
            .map(|item| TopicSourceAttentionDto {
                source_title: item
                    .source_path
                    .as_deref()
                    .map(|path| luna_sync_core::links::title_of(path).to_owned()),
                source_path: item.source_path,
                state: item.state.into(),
                claims: item.claims,
            })
            .collect(),
        attention_omitted: summary.attention_omitted,
        attention_claims: summary.attention_claims,
        unverified_lines: summary.unverified_lines,
        transcript_records: summary.transcript_records,
    }))
}

/// 阅读视图的声明筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicClaimFilterDto {
    Conflicts,
    NeedsCheck,
    TranscriptRecords,
}

/// 筛选清单里一条出处：状态由 Rust 按此刻的来源文件判定。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicClaimCitationDto {
    pub citation_id: String,
    pub source_path: Option<String>,
    pub source_title: Option<String>,
    pub state: TopicEvidenceStateDto,
    /// 生成时记录的原文引句；界面按需展开，来源已改或已删时用它对照当初的依据。
    pub quote: String,
    /// 来源笔记「录于何时」（Unix 毫秒，`luna.created`）；没记为 `None`，界面不显示时间。
    pub recorded_ms: Option<f64>,
}

/// 分歧并列的那条既有声明：两边各带出处与时间，由读者判断，界面不标谁对。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicConflictPeerDto {
    pub id: String,
    pub text: String,
    pub citations: Vec<TopicClaimCitationDto>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicClaimItemDto {
    pub id: String,
    pub text: String,
    pub conflict: bool,
    pub transcript_record: bool,
    pub citations: Vec<TopicClaimCitationDto>,
    pub conflicts_with: Option<TopicConflictPeerDto>,
}

fn topic_claim_citation_dto(citation: luna_vault::ClaimCitationView) -> TopicClaimCitationDto {
    TopicClaimCitationDto {
        source_title: citation
            .source_path
            .as_deref()
            .map(|path| luna_sync_core::links::title_of(path).to_owned()),
        citation_id: citation.citation_id,
        source_path: citation.source_path,
        state: citation.state.into(),
        quote: citation.quote,
        recorded_ms: citation.recorded_ms.map(|ms| ms as f64),
    }
}

/// 按页面顺序的筛选结果；超出上限的只计入 `omitted`。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicClaimListDto {
    pub claims: Vec<TopicClaimItemDto>,
    pub omitted: u32,
}

fn topic_claims_of(
    vault: &luna_vault::Vault,
    topic_path: &str,
    filter: TopicClaimFilterDto,
) -> Result<Option<TopicClaimListDto>, String> {
    let filter = match filter {
        TopicClaimFilterDto::Conflicts => luna_vault::ClaimFilter::Conflicts,
        TopicClaimFilterDto::NeedsCheck => luna_vault::ClaimFilter::NeedsCheck,
        TopicClaimFilterDto::TranscriptRecords => luna_vault::ClaimFilter::TranscriptRecords,
    };
    let (claims, omitted) = match vault.topic_claims_filtered(topic_path, filter) {
        Ok(result) => result,
        Err(luna_vault::VaultError::InvalidReplace(_)) => return Ok(None),
        Err(error) => return Err(vault_err(error)),
    };
    Ok(Some(TopicClaimListDto {
        claims: claims
            .into_iter()
            .map(|claim| TopicClaimItemDto {
                id: claim.id,
                text: claim.text,
                conflict: claim.kind == luna_vault::ClaimKind::Conflict,
                transcript_record: claim.kind == luna_vault::ClaimKind::TranscriptRecord,
                citations: claim
                    .citations
                    .into_iter()
                    .map(topic_claim_citation_dto)
                    .collect(),
                conflicts_with: claim.conflicts_with.map(|peer| TopicConflictPeerDto {
                    id: peer.id,
                    text: peer.text,
                    citations: peer
                        .citations
                        .into_iter()
                        .map(topic_claim_citation_dto)
                        .collect(),
                }),
            })
            .collect(),
        omitted,
    }))
}

/// 知识网络里主题节点的预览卡（PRD-16-062 / ADR-136 ④）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicPreviewDto {
    pub path: String,
    pub title: String,
    /// 与目录、地图同一条规则的一句摘要；空 = 还没有可读的正文段落。
    pub summary: String,
    /// 证据格式页的范围；旧格式页为 `None`（没有范围、声明与出处可摆）。
    pub scope: Option<TopicScopeDto>,
    pub claim_count: u32,
    pub conflict_count: u32,
    /// 至多 [`PREVIEW_CLAIMS`] 条站得住的结论（非分歧、出处都是当前），按页面顺序。
    pub claims: Vec<TopicPreviewClaimDto>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicPreviewClaimDto {
    pub text: String,
    pub citations: Vec<TopicPreviewCitationDto>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicPreviewCitationDto {
    /// 打开出处用（`open_topic_citation`）。
    pub citation_id: String,
    pub source_title: String,
}

/// 预览卡摆几条结论：卡片是一瞥，不是第二个阅读视图。
const PREVIEW_CLAIMS: usize = 3;

/// 主题节点单击时的预览：范围、摘要、三条带出处的结论。只读，不调用模型。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_preview(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    topic_path: String,
) -> Result<TopicPreviewDto, String> {
    state.with_vault(window.label(), |vault| topic_preview_of(vault, &topic_path))
}

fn topic_preview_of(
    vault: &luna_vault::Vault,
    topic_path: &str,
) -> Result<TopicPreviewDto, String> {
    let doc = vault.read(topic_path).map_err(vault_err)?;
    let summary = luna_vault::topic_summary(&doc.content);
    let title = luna_sync_core::links::title_of(topic_path).to_owned();
    let reading = match vault.topic_reading_summary(topic_path) {
        Ok(reading) => reading,
        // 旧格式、未知 schema、坏证据：只给页名与摘要，不编范围与结论。
        Err(luna_vault::VaultError::InvalidReplace(_)) => {
            return Ok(TopicPreviewDto {
                path: topic_path.to_owned(),
                title,
                summary,
                scope: None,
                claim_count: 0,
                conflict_count: 0,
                claims: Vec::new(),
            })
        }
        Err(error) => return Err(vault_err(error)),
    };
    let (settled, _) = vault
        .topic_claims_filtered(topic_path, luna_vault::ClaimFilter::Settled)
        .map_err(vault_err)?;
    let shown = &settled[..settled.len().min(PREVIEW_CLAIMS)];
    // 证据格式页的正文一开头就是声明块，摘要（首个有效段落）往往就是第一条结论：卡片上
    // 再摆一遍是同一句话出现两次（真机 2026-10-01）。摆出来的结论里已经有它就不另给摘要。
    let summary = if shown
        .iter()
        .any(|claim| summary.starts_with(claim.text.as_str()))
    {
        String::new()
    } else {
        summary
    };
    Ok(TopicPreviewDto {
        path: topic_path.to_owned(),
        title,
        summary,
        scope: Some(TopicScopeDto {
            title: reading.scope.title,
            description: reading.scope.description,
            includes: reading.scope.includes,
            excludes: reading.scope.excludes,
        }),
        claim_count: reading.claims,
        conflict_count: reading.conflicts,
        claims: settled
            .into_iter()
            .take(PREVIEW_CLAIMS)
            .map(|claim| TopicPreviewClaimDto {
                text: claim.text,
                citations: claim
                    .citations
                    .into_iter()
                    .map(|citation| TopicPreviewCitationDto {
                        source_title: citation
                            .source_path
                            .as_deref()
                            .map_or_else(String::new, |path| {
                                luna_sync_core::links::title_of(path).to_owned()
                            }),
                        citation_id: citation.citation_id,
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// 主题阅读视图「只看分歧 / 只看需核对」。只读，不调用模型；旧格式返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_claims(
    state: tauri::State<'_, AppState>,
    topic_path: String,
    filter: TopicClaimFilterDto,
) -> Result<Option<TopicClaimListDto>, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    topic_claims_of(vault, &topic_path, filter)
}

/// 主题阅读页头：范围、声明与分歧数、需核对的出处。只读，不调用模型。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_reading_summary(
    state: tauri::State<'_, AppState>,
    topic_path: String,
) -> Result<Option<TopicReadingSummaryDto>, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    topic_reading_summary_of(vault, &topic_path)
}

/// 知识库首页「近 7 天」的窗口（PRD-16-067）：滚动 7 天，不按自然周——周一一早不会是一片
/// 空白，也不必按地区约定一周从哪天开始。
const WEEK_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// 「近 7 天」里列几页主题、几篇还没进主题的来源：这是一块回顾，不是第二个目录。
const WEEK_LIST_LIMIT: usize = 5;

/// 「继续阅读」的目标页。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct KnowledgeVisitDto {
    pub path: String,
    pub title: String,
}

/// 「近 7 天」里有新结论的一页主题。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct WeekTopicDto {
    pub path: String,
    pub title: String,
    pub claims: u32,
    pub conflicts: u32,
}

/// 「近 7 天」收进来、还没被任何主题页引用的一篇来源。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct WeekSourceDto {
    pub path: String,
    pub title: String,
}

/// 知识库首页的「近 7 天」（PRD-16-067 / ADR-136 ⑦）：全部由既有事实确定性派生，不调用模型。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct KnowledgeWeekDto {
    /// 按「录于何时」收进来的来源篇数，分三类（录音含导入媒体的转写）。
    pub notes: u32,
    pub quick_notes: u32,
    pub recordings: u32,
    /// 这 7 天整理出、此刻仍在主题页上的结论数（其中分歧数）。
    pub claims: u32,
    pub conflicts: u32,
    /// 新结论最多的至多 5 页；其余只计数。
    pub topics: Vec<WeekTopicDto>,
    pub topics_omitted: u32,
    /// 这 7 天收进来、还没被任何主题页引用的来源（新近在前，至多 5 篇）与总数。
    pub unfiled: Vec<WeekSourceDto>,
    pub unfiled_count: u32,
}

/// 知识库工作台的回访入口：继续阅读 + 近 7 天。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct KnowledgeRecentActivityDto {
    pub continue_reading: Option<KnowledgeVisitDto>,
    pub week: KnowledgeWeekDto,
}

/// 知识库工作台的「继续阅读」与「近 7 天」。只读，不调用模型。
#[tauri::command]
#[specta::specta]
pub(crate) fn knowledge_recent_activity(
    state: tauri::State<'_, AppState>,
) -> Result<KnowledgeRecentActivityDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let since = u64::try_from(super::checkup::now_ms())
        .unwrap_or(0)
        .saturating_sub(WEEK_WINDOW_MS);
    Ok(KnowledgeRecentActivityDto {
        continue_reading: vault.last_knowledge_visit().map(|path| KnowledgeVisitDto {
            title: luna_sync_core::links::title_of(&path).to_owned(),
            path,
        }),
        week: knowledge_week_of(vault, since)?,
    })
}

fn knowledge_week_of(vault: &luna_vault::Vault, since_ms: u64) -> Result<KnowledgeWeekDto, String> {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let sources = vault
        .sources_created_since(i64::try_from(since_ms).unwrap_or(i64::MAX))
        .map_err(vault_err)?;
    let of_type = |types: &[luna_vault::NoteType]| {
        count(
            sources
                .iter()
                .filter(|source| types.contains(&source.note_type))
                .count(),
        )
    };
    let unfiled: Vec<&luna_vault::RecentSource> =
        sources.iter().filter(|source| !source.cited).collect();
    let topics = vault
        .topic_claims_added_since(since_ms)
        .map_err(vault_err)?;
    Ok(KnowledgeWeekDto {
        notes: of_type(&[luna_vault::NoteType::Note]),
        quick_notes: of_type(&[luna_vault::NoteType::QuickNote]),
        recordings: of_type(&[
            luna_vault::NoteType::Recording,
            luna_vault::NoteType::Transcript,
        ]),
        claims: topics.iter().map(|topic| topic.claims).sum(),
        conflicts: topics.iter().map(|topic| topic.conflicts).sum(),
        topics_omitted: count(topics.len().saturating_sub(WEEK_LIST_LIMIT)),
        topics: topics
            .into_iter()
            .take(WEEK_LIST_LIMIT)
            .map(|topic| WeekTopicDto {
                title: luna_sync_core::links::title_of(&topic.path).to_owned(),
                path: topic.path,
                claims: topic.claims,
                conflicts: topic.conflicts,
            })
            .collect(),
        unfiled_count: count(unfiled.len()),
        unfiled: unfiled
            .into_iter()
            .take(WEEK_LIST_LIMIT)
            .map(|source| WeekSourceDto {
                path: source.path.clone(),
                title: luna_sync_core::links::title_of(&source.path).to_owned(),
            })
            .collect(),
    })
}

/// 阅读视图打开了一页知识产物：记为「继续阅读」的目标（库内 `.luna` 界面现场，只存路径）。
#[tauri::command]
#[specta::specta]
pub(crate) fn record_knowledge_visit(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.record_knowledge_visit(&path).map_err(vault_err)
}

/// 旧主题升级的封闭原因码（Rust 判定，前端不从 Display 猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicUpgradeRejectionCodeDto {
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

impl From<luna_vault::TopicUpgradeRejectionCode> for TopicUpgradeRejectionCodeDto {
    fn from(value: luna_vault::TopicUpgradeRejectionCode) -> Self {
        use luna_vault::TopicUpgradeRejectionCode as Code;
        match value {
            Code::NotTopic => Self::NotTopic,
            Code::AlreadyCurrent => Self::AlreadyCurrent,
            Code::UnknownSchema => Self::UnknownSchema,
            Code::InvalidMetadata => Self::InvalidMetadata,
            Code::MissingScope => Self::MissingScope,
            Code::BudgetExceeded => Self::BudgetExceeded,
            Code::TargetChanged => Self::TargetChanged,
            Code::TargetMissing => Self::TargetMissing,
            Code::VaultReadFailed => Self::VaultReadFailed,
            Code::WriteFailed => Self::WriteFailed,
            Code::BackupFailed => Self::BackupFailed,
            Code::BackupMissing => Self::BackupMissing,
            Code::CommitLockUnavailable => Self::CommitLockUnavailable,
        }
    }
}

/// 一页旧主题的升级影响。正文逐字节不变，故不回传整页 Markdown；确认须原样带回两个指纹。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicUpgradePreviewDto {
    Ready {
        path: String,
        before_hash: String,
        after_hash: String,
        scope_title: String,
        scope_includes: Vec<String>,
        /// 人工改过的旧页：升级只换格式，之后仍停在复核态，不开放自动维护。
        user_edited: bool,
        /// 声明块之外、非标题的内容行数（`unverified_content_lines`），升级后页头同样标「未核验」。
        unverified_lines: u32,
    },
    Rejected {
        path: String,
        code: TopicUpgradeRejectionCodeDto,
    },
}

/// 一页升级的落盘回执；多页请求逐页独立，不承诺整批原子。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicUpgradeResultDto {
    Upgraded {
        path: String,
        backup_id: String,
        index_updated: bool,
        baseline_recorded: bool,
        maintenance: TopicMaintenanceStateDto,
    },
    Rejected {
        path: String,
        code: TopicUpgradeRejectionCodeDto,
    },
}

#[derive(Debug, Clone, Deserialize, Type)]
pub(crate) struct TopicUpgradeRequestDto {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
}

/// 单次升级请求的页数上限；更大的批次由调用方分批，每批仍逐页预览与确认。
const MAX_TOPIC_UPGRADE_BATCH: usize = 50;

fn topic_upgrade_previews(
    vault: &luna_vault::Vault,
    paths: &[String],
) -> Vec<TopicUpgradePreviewDto> {
    paths
        .iter()
        .map(|path| match vault.preview_topic_upgrade(path) {
            Ok(preview) => TopicUpgradePreviewDto::Ready {
                path: preview.path,
                before_hash: preview.before_hash,
                after_hash: preview.after_hash,
                scope_title: preview.scope.title,
                scope_includes: preview.scope.includes,
                user_edited: preview.user_edited,
                unverified_lines: preview.unverified_lines,
            },
            Err(rejection) => TopicUpgradePreviewDto::Rejected {
                path: path.clone(),
                code: rejection.code().into(),
            },
        })
        .collect()
}

fn topic_upgrade_results(
    vault: &luna_vault::Vault,
    items: &[TopicUpgradeRequestDto],
) -> Vec<TopicUpgradeResultDto> {
    items
        .iter()
        .map(|item| {
            match vault.apply_topic_upgrade(&item.path, &item.before_hash, &item.after_hash) {
                Ok(outcome) => TopicUpgradeResultDto::Upgraded {
                    path: item.path.clone(),
                    backup_id: outcome.backup_id.to_string(),
                    index_updated: outcome.index_updated,
                    baseline_recorded: outcome.baseline_recorded,
                    maintenance: outcome.maintenance.into(),
                },
                Err(rejection) => TopicUpgradeResultDto::Rejected {
                    path: item.path.clone(),
                    code: rejection.code().into(),
                },
            }
        })
        .collect()
}

/// 一页可升级的旧主题（批量选择清单的一行）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicUpgradeCandidateDto {
    pub path: String,
    pub title: String,
}

/// 仍是旧格式的主题页；超过一批上限的只计入 `omitted`，升级完这一批再列下一批。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicUpgradeCandidatesDto {
    pub pages: Vec<TopicUpgradeCandidateDto>,
    pub omitted: u32,
}

fn topic_upgrade_candidates_of(
    vault: &luna_vault::Vault,
) -> Result<TopicUpgradeCandidatesDto, String> {
    let all = vault.legacy_topic_pages().map_err(vault_err)?;
    let omitted =
        u32::try_from(all.len().saturating_sub(MAX_TOPIC_UPGRADE_BATCH)).unwrap_or(u32::MAX);
    Ok(TopicUpgradeCandidatesDto {
        pages: all
            .into_iter()
            .take(MAX_TOPIC_UPGRADE_BATCH)
            .map(|path| TopicUpgradeCandidateDto {
                title: luna_sync_core::links::title_of(&path).to_owned(),
                path,
            })
            .collect(),
        omitted,
    })
}

/// 批量升级的选择范围：列出旧格式主题页。只读，不写文件。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_upgrade_candidates(
    state: tauri::State<'_, AppState>,
) -> Result<TopicUpgradeCandidatesDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    topic_upgrade_candidates_of(vault)
}

/// 只读预览所选旧主题的升级影响；不写文件，不开库自动迁移。
#[tauri::command]
#[specta::specta]
pub(crate) fn preview_topic_upgrade(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<TopicUpgradePreviewDto>, String> {
    if paths.is_empty() || paths.len() > MAX_TOPIC_UPGRADE_BATCH {
        return Err(coded_error_args(
            &app,
            "TOPIC_UPGRADE_BATCH",
            "topic-upgrade-batch",
            &[("max", MAX_TOPIC_UPGRADE_BATCH.to_string())],
        ));
    }
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(topic_upgrade_previews(vault, &paths))
}

/// 应用用户逐页确认的升级。每页写前留原件备份，按预览指纹条件写；逐页回执。
#[tauri::command]
#[specta::specta]
pub(crate) fn apply_topic_upgrade(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    items: Vec<TopicUpgradeRequestDto>,
) -> Result<Vec<TopicUpgradeResultDto>, String> {
    if items.is_empty() || items.len() > MAX_TOPIC_UPGRADE_BATCH {
        return Err(coded_error_args(
            &app,
            "TOPIC_UPGRADE_BATCH",
            "topic-upgrade-batch",
            &[("max", MAX_TOPIC_UPGRADE_BATCH.to_string())],
        ));
    }
    let (results, written) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let results = topic_upgrade_results(vault, &items);
        let upgraded: Vec<String> = results
            .iter()
            .filter_map(|result| match result {
                TopicUpgradeResultDto::Upgraded { path, .. } => Some(path.clone()),
                TopicUpgradeResultDto::Rejected { .. } => None,
            })
            .collect();
        let written = written_contents(vault, &upgraded);
        (results, written)
    };
    announce_topic_written(&app, written);
    Ok(results)
}

/// 回退一次升级的结果：`Restored` 给出被恢复的页；文件已被再改时拒绝，不覆盖后续内容。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicUpgradeRestoreDto {
    Restored { path: String },
    Rejected { code: TopicUpgradeRejectionCodeDto },
}

fn restore_topic_upgrade_locked(
    vault: &luna_vault::Vault,
    backup_id: &str,
) -> TopicUpgradeRestoreDto {
    let Ok(id) = uuid::Uuid::parse_str(backup_id) else {
        return TopicUpgradeRestoreDto::Rejected {
            code: TopicUpgradeRejectionCodeDto::BackupMissing,
        };
    };
    match vault.restore_topic_upgrade(id) {
        Ok(path) => TopicUpgradeRestoreDto::Restored { path },
        Err(rejection) => TopicUpgradeRestoreDto::Rejected {
            code: rejection.code().into(),
        },
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn restore_topic_upgrade(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    backup_id: String,
) -> Result<TopicUpgradeRestoreDto, String> {
    let (result, written) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let result = restore_topic_upgrade_locked(vault, &backup_id);
        let written = match &result {
            TopicUpgradeRestoreDto::Restored { path } => {
                written_contents(vault, std::slice::from_ref(path))
            }
            TopicUpgradeRestoreDto::Rejected { .. } => Vec::new(),
        };
        (result, written)
    };
    announce_topic_written(&app, written);
    Ok(result)
}

/// 本应用写成主题文件后的双广播：`note://written` 让开着这一页的编辑器 pane 跟进新正文，
/// `vault://changed` 让阅读视图、目录与地图重取（自写回声抑制下文件监听不会替我们发）。
/// 与既有归入写入同一纪律；少发一个，开着的缓冲下一次保存就会把刚写入的内容覆盖掉。
fn announce_topic_written(app: &tauri::AppHandle, written: Vec<(String, String)>) {
    for (path, content) in written {
        let _ = (NoteWritten {
            path: path.clone(),
            content,
            origin: "main".to_owned(),
        })
        .emit(app);
        emit_vault_changed(app, &path, false);
    }
}

/// 撤销一次计划写入的结果（K4）。
#[derive(Debug, Clone, Copy, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicApplyUndoDto {
    /// 已恢复成写入前的那一版。
    Restored,
    /// 写入之后这一页又被改过：撤销会覆盖之后的改动，没有做；改从版本历史处理。
    Changed,
    /// 版本链里找不到写入前那一版。
    Unavailable,
}

/// 撤销刚才那次「应用到本页」：只在这一页仍逐字节是写入后的那一版时恢复写入前版本。
/// 两个指纹来自用户确认时看到的那份预览（`before_hash`）与计划结果（`result_hash`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn undo_topic_apply(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target_path: String,
    before_hash: String,
    after_hash: String,
) -> Result<TopicApplyUndoDto, String> {
    let (outcome, written) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 读写或提交锁失败属环境性故障，与应用同一错误码族（可重试）。
        let outcome = vault
            .undo_applied_plan(&target_path, &before_hash, &after_hash)
            .map_err(|_| {
                coded_error(
                    &app,
                    "KNOWLEDGE_APPLY_UNAVAILABLE",
                    "knowledge-apply-unavailable",
                )
            })?;
        let written = if outcome == luna_vault::PlanUndoOutcome::Restored {
            // 撤的若是一份被记为接受的低风险计划，记进影子资格（只认得出结果指纹）。
            record_shadow(
                &app,
                vault,
                crate::knowledge_shadow::ShadowEvent::Undone,
                "",
                &after_hash,
            );
            written_contents(vault, std::slice::from_ref(&target_path))
        } else {
            Vec::new()
        };
        (outcome, written)
    };
    announce_topic_written(&app, written);
    Ok(match outcome {
        luna_vault::PlanUndoOutcome::Restored => TopicApplyUndoDto::Restored,
        luna_vault::PlanUndoOutcome::Changed => TopicApplyUndoDto::Changed,
        luna_vault::PlanUndoOutcome::Unavailable => TopicApplyUndoDto::Unavailable,
    })
}

/// 在持锁时读出刚写成文件的正文，供释放锁后广播。
fn written_contents(vault: &luna_vault::Vault, paths: &[String]) -> Vec<(String, String)> {
    paths
        .iter()
        .filter_map(|path| vault.read(path).ok().map(|doc| (path.clone(), doc.content)))
        .collect()
}

/// 用户确认应用封存计划的即时结果：`Queued` 只表示应用意图已写入持久队列，
/// 不表示主题已更新；主题写入的事实由知识回执另行陈述。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum SealedIngestConfirmDto {
    Queued {
        operation_id: String,
        target_path: String,
    },
    Rejected {
        rejection: PlanRejectionDto,
    },
}

/// 确认时必须带上用户看到的那份预览指纹：同一 ID 的载荷不可改写，这里再把
/// 「看到的」与「将应用的」绑成同一份，而不是只凭 ID 信任。
fn check_confirmable_plan(
    vault: &luna_vault::Vault,
    operation_id: &str,
    result_hash: &str,
) -> Result<luna_vault::IngestPlan, luna_vault::PlanRejection> {
    let id =
        uuid::Uuid::parse_str(operation_id).map_err(|_| luna_vault::PlanRejection::InvalidPlan)?;
    let plan = vault.load_sealed_ingest_plan(id)?;
    if plan.result_hash != result_hash {
        return Err(luna_vault::PlanRejection::InvalidPlan);
    }
    vault.preview_ingest_plan(&plan)?;
    Ok(plan)
}

/// 用户确认后把应用意图写进持久队列；这条命令不调用模型、不写主题。
///
/// 锁序沿用 `vault → processing.inner`：复核与入队在同一次 Vault 锁内完成，
/// 切库不能插在「复核通过」与「意图入队」之间。
#[tauri::command]
#[specta::specta]
pub(crate) fn confirm_sealed_ingest_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    operation_id: String,
    result_hash: String,
) -> Result<SealedIngestConfirmDto, String> {
    let result = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        match check_confirmable_plan(vault, &operation_id, &result_hash) {
            Err(rejection) => SealedIngestConfirmDto::Rejected {
                rejection: rejection.into(),
            },
            Ok(plan) => {
                let target_path = plan.baseline.target_path.clone();
                let kind = crate::processing_queue::ProcTaskKind::ApplyIngestPlan {
                    operation_id: plan.operation_id.to_string(),
                    vault_generation: state.vault_generation(),
                    target_rel: target_path.clone(),
                };
                match state
                    .processing
                    .enqueue_plan_apply(kind, super::transcribe::note_task_label(&target_path))
                {
                    Ok(_) => {
                        if plan_is_low_risk(vault, &plan) {
                            record_shadow(
                                &app,
                                vault,
                                crate::knowledge_shadow::ShadowEvent::Accepted,
                                &plan.operation_id.to_string(),
                                &plan.result_hash,
                            );
                        }
                        SealedIngestConfirmDto::Queued {
                            operation_id: plan.operation_id.to_string(),
                            target_path,
                        }
                    }
                    Err(code) => {
                        eprintln!(
                            "[knowledge] 封存计划应用意图未能持久化：{}",
                            code.split(':').next().unwrap_or_default()
                        );
                        SealedIngestConfirmDto::Rejected {
                            rejection: PlanRejectionDto::app(
                                PlanRejectionCodeDto::IntentStoreUnavailable,
                            ),
                        }
                    }
                }
            }
        }
    };
    if matches!(result, SealedIngestConfirmDto::Queued { .. }) {
        super::transcribe::emit_processing_queue(&app);
        super::transcribe::start_processing_worker_if_idle(&app);
    }
    Ok(result)
}

/// 一次封存计划应用的落盘事实（队列 worker 据此发回执并决定是否清理载荷）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SealedApplyReport {
    pub target_path: String,
    /// 本次写入所依据的来源（去重、保序）；回执据此说「哪篇归入了哪页」。
    pub source_paths: Vec<String>,
    pub index_updated: bool,
    pub baseline_recorded: bool,
    /// 写进页面的条目数：计划里的每个动作恰好落成一条（已落盘的重跑同样如此）。
    pub applied_items: u32,
}

/// 应用核心：只按 ID 取回同一份载荷，复核后条件写；**不调用模型**。重复执行识别已落盘结果。
pub(crate) fn apply_sealed_ingest_plan_locked(
    vault: &luna_vault::Vault,
    operation_id: &str,
) -> Result<SealedApplyReport, luna_vault::PlanRejection> {
    let id =
        uuid::Uuid::parse_str(operation_id).map_err(|_| luna_vault::PlanRejection::InvalidPlan)?;
    let plan = vault.load_sealed_ingest_plan(id)?;
    let (index_updated, baseline_recorded) = match vault.apply_ingest_plan(&plan)? {
        luna_vault::PlanApplyOutcome::Applied {
            index_updated,
            baseline_recorded,
        }
        | luna_vault::PlanApplyOutcome::AlreadyApplied {
            index_updated,
            baseline_recorded,
        } => (index_updated, baseline_recorded),
    };
    let mut source_paths: Vec<String> = Vec::new();
    for evidence in &plan.evidence {
        if !source_paths.contains(&evidence.source_path) {
            source_paths.push(evidence.source_path.clone());
        }
    }
    Ok(SealedApplyReport {
        target_path: plan.baseline.target_path,
        source_paths,
        index_updated,
        baseline_recorded,
        applied_items: u32::try_from(plan.actions.len()).unwrap_or(u32::MAX),
    })
}

/// 应用被拒后原样重跑是否可能成功：只有锁、读写与存储这类环境性故障值得重试；
/// 来源/目标/范围变化与用户否决是版本事实，重试只会重复拒绝。
fn plan_rejection_is_retryable(code: luna_vault::PlanRejectionCode) -> bool {
    use luna_vault::PlanRejectionCode as Code;
    matches!(
        code,
        Code::VaultReadFailed
            | Code::WriteFailed
            | Code::DecisionStoreUnavailable
            | Code::CommitLockUnavailable
            | Code::PlanStoreUnavailable
    )
}

/// 一件应用任务的尝试预算（TDD-06 §3.9.3「尝试次数受后端预算约束」）：只对环境性故障
/// （提交锁被另一句柄占用、读写或存储暂时失败）在同一件任务里退避重试，版本事实类拒绝
/// 一次即终。重试间隔按下标取；次数用完仍失败即以可重试失败码结束，载荷保留，面板回来
/// 时照常恢复这份计划。
pub(crate) const APPLY_ATTEMPT_BUDGET: u32 = 3;
const APPLY_RETRY_BACKOFF_MS: [u64; 2] = [250, 1_000];

/// 一次应用尝试的失败：`None` = 库已切换或未打开（重试无意义），否则为拒绝码。
type ApplyAttemptError = Option<luna_vault::PlanRejectionCode>;

fn apply_attempt(
    app: &tauri::AppHandle,
    operation_id: &str,
    vault_generation: u64,
) -> Result<SealedApplyReport, ApplyAttemptError> {
    let state = app.state::<AppState>();
    let report = {
        let guard = state.vault.lock().map_err(|_| None)?;
        let Some(vault) = guard.as_ref() else {
            return Err(None);
        };
        // 代际与写入在同一把 Vault 锁内核对：切库不能插在两者之间。
        if !state.still_current_vault(vault_generation) {
            return Err(None);
        }
        let report = apply_sealed_ingest_plan_locked(vault, operation_id)
            .map_err(|rejection| Some(rejection.code()))?;
        let written = written_contents(vault, std::slice::from_ref(&report.target_path));
        (report, written)
    };
    announce_topic_written(app, report.1);
    Ok(report.0)
}

/// 在尝试预算内跑一次应用：环境性故障退避重试，其余结果（成功、版本事实拒绝、库已切换、
/// 已取消）立即返回。第二个值是实际尝试次数。
async fn retry_within_budget<T>(
    mut run: impl FnMut() -> Result<T, ApplyAttemptError>,
    is_cancelled: impl Fn() -> bool,
) -> (Result<T, ApplyAttemptError>, u32) {
    let mut attempt = 1;
    loop {
        match run() {
            Err(Some(code))
                if plan_rejection_is_retryable(code)
                    && attempt < APPLY_ATTEMPT_BUDGET
                    && !is_cancelled() =>
            {
                eprintln!(
                    "[knowledge] 封存计划应用暂时失败 code={code:?} attempt={attempt}，退避重试"
                );
                let backoff = APPLY_RETRY_BACKOFF_MS[(attempt - 1) as usize];
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                attempt += 1;
            }
            other => return (other, attempt),
        }
    }
}

/// 写进了、但索引或系统底版没跟上：回执把「文件已写入」与「这几步没做完」分开陈述
/// （K4「索引刷新失败与文件写入成功分别回执」），不把降级说成失败，也不把它藏起来。
fn apply_degraded_steps(report: &SealedApplyReport) -> Vec<KnowledgeMaintenanceStep> {
    let mut steps = Vec::new();
    if !report.index_updated {
        steps.push(KnowledgeMaintenanceStep::Index);
    }
    if !report.baseline_recorded {
        steps.push(KnowledgeMaintenanceStep::Baseline);
    }
    steps
}

/// 队列 worker 的应用执行体。成功时发知识回执；失败返回稳定失败码（不含正文或路径）。
pub(crate) async fn run_sealed_ingest_apply(
    app: &tauri::AppHandle,
    operation_id: &str,
    vault_generation: u64,
    is_cancelled: impl Fn() -> bool,
) -> Result<SealedApplyReport, String> {
    let (result, attempt) = retry_within_budget(
        || apply_attempt(app, operation_id, vault_generation),
        is_cancelled,
    )
    .await;
    match result {
        Ok(report) => {
            let degraded_steps = apply_degraded_steps(&report);
            // 回执的主笔记是来源而不是目标页：「《周会》已归入《项目 A》」，而不是目标页归入自己。
            let primary = report
                .source_paths
                .first()
                .unwrap_or(&report.target_path)
                .clone();
            emit_knowledge_receipt(
                app,
                operation_id,
                "ingest",
                &primary,
                KnowledgeReceiptFacts {
                    updated_topics: vec![report.target_path.clone()],
                    degraded_steps,
                    applied_items: Some(report.applied_items),
                    ..Default::default()
                },
            );
            Ok(report)
        }
        Err(None) => Err(coded_error(
            app,
            "KNOWLEDGE_APPLY_UNAVAILABLE",
            "knowledge-apply-unavailable",
        )),
        Err(Some(code)) => {
            eprintln!("[knowledge] 封存计划应用被拒 code={code:?} attempts={attempt}");
            Err(if plan_rejection_is_retryable(code) {
                coded_error(
                    app,
                    "KNOWLEDGE_APPLY_UNAVAILABLE",
                    "knowledge-apply-unavailable",
                )
            } else {
                coded_error(app, "KNOWLEDGE_APPLY_STALE", "knowledge-apply-stale")
            })
        }
    }
}

/// 应用任务到达成功终态且续跑档已确认不再引用它之后，清理设备本地载荷。
///
/// 顺序不可颠倒：先删载荷再落续跑档，崩溃后续跑会读到缺失载荷，把一次已完成的应用
/// 误报为失败。续跑档写不进时保留载荷，下次重跑识别 `AlreadyApplied` 后再清。
pub(crate) fn discard_applied_sealed_plan(app: &tauri::AppHandle, operation_id: &str) {
    let state = app.state::<AppState>();
    if let Err(error) = state.processing.persist_checked() {
        eprintln!(
            "[knowledge] 续跑档未确认落盘，保留封存计划载荷：{}",
            error.kind()
        );
        return;
    }
    let Ok(id) = uuid::Uuid::parse_str(operation_id) else {
        return;
    };
    let Ok(guard) = state.vault.lock() else {
        return;
    };
    if let Some(vault) = guard.as_ref() {
        if let Err(error) = vault.discard_sealed_ingest_plan(id) {
            eprintln!("[knowledge] 封存计划载荷清理失败 code={:?}", error.code());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SealedIngestDecisionKindDto {
    Ignored,
    Rejected,
}

/// 用户处置一份封存计划的结果。
///
/// - `Recorded`：随库记下同一来源/范围版本的决定并清理载荷，重扫与重启都不会自动重放；
/// - `Discarded`：计划在处置前已因版本变化失效，无法、也无须按这份版本抑制，只清理载荷；
/// - `Rejected`：环境性失败或仍有应用在排队，载荷保留。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum SealedIngestDecisionDto {
    Recorded,
    Discarded { rejection: PlanRejectionDto },
    Rejected { rejection: PlanRejectionDto },
}

fn decide_sealed_ingest_plan_locked(
    vault: &luna_vault::Vault,
    operation_id: &str,
    decision: SealedIngestDecisionKindDto,
    apply_pending: bool,
) -> SealedIngestDecisionDto {
    if apply_pending {
        return SealedIngestDecisionDto::Rejected {
            rejection: PlanRejectionDto::app(PlanRejectionCodeDto::ApplyPending),
        };
    }
    let Ok(id) = uuid::Uuid::parse_str(operation_id) else {
        return SealedIngestDecisionDto::Rejected {
            rejection: luna_vault::PlanRejection::InvalidPlan.into(),
        };
    };
    let plan = match vault.load_sealed_ingest_plan(id) {
        Ok(plan) => plan,
        Err(rejection) => {
            return SealedIngestDecisionDto::Rejected {
                rejection: rejection.into(),
            };
        }
    };
    let kind = match decision {
        SealedIngestDecisionKindDto::Ignored => luna_vault::IngestDecisionKind::Ignored,
        SealedIngestDecisionKindDto::Rejected => luna_vault::IngestDecisionKind::Rejected,
    };
    let outcome = match vault.record_ingest_decision(&plan, kind) {
        Ok(()) => SealedIngestDecisionDto::Recorded,
        Err(rejection) if !plan_rejection_is_retryable(rejection.code()) => {
            SealedIngestDecisionDto::Discarded {
                rejection: rejection.into(),
            }
        }
        Err(rejection) => {
            return SealedIngestDecisionDto::Rejected {
                rejection: rejection.into(),
            };
        }
    };
    match vault.discard_sealed_ingest_plan(id) {
        Ok(()) => outcome,
        Err(rejection) => SealedIngestDecisionDto::Rejected {
            rejection: rejection.into(),
        },
    }
}

/// 忽略或否决一份封存计划。这条命令不调用模型、不写主题；决定写进随库 meta store。
#[tauri::command]
#[specta::specta]
pub(crate) fn decide_sealed_ingest_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    operation_id: String,
    decision: SealedIngestDecisionKindDto,
) -> Result<SealedIngestDecisionDto, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let apply_pending = state.processing.has_active_plan_apply(&operation_id);
    // 处置会清理载荷：先判定它是不是低风险计划，再处置。
    let low_risk = uuid::Uuid::parse_str(&operation_id)
        .ok()
        .and_then(|id| vault.load_sealed_ingest_plan(id).ok())
        .filter(|plan| plan_is_low_risk(vault, plan))
        .map(|plan| plan.result_hash);
    let outcome = decide_sealed_ingest_plan_locked(vault, &operation_id, decision, apply_pending);
    if let (SealedIngestDecisionDto::Recorded, Some(result_hash)) = (&outcome, low_risk) {
        let event = match decision {
            SealedIngestDecisionKindDto::Ignored => crate::knowledge_shadow::ShadowEvent::Ignored,
            SealedIngestDecisionKindDto::Rejected => crate::knowledge_shadow::ShadowEvent::Rejected,
        };
        record_shadow(&app, vault, event, &operation_id, &result_hash);
    }
    Ok(outcome)
}

/// 阅读视图点开主题正文里的一条库内链接：落在声明块内就按该出处的定位锚打开。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TopicLinkOpenDto {
    /// 已由 Rust 导航；`title` 供非精确终态的提示使用。
    Opened {
        outcome: CitationOpenOutcome,
        title: String,
    },
    /// 不是声明出处（块外链接、旧格式页或来源不在库内）：前端按普通链接打开。
    NotCitation,
}

/// 主题正文中「第 `occurrence` 个指向 `href` 的链接」若是声明出处，按其定位锚、
/// 版本优先打开（与计划出处、对话出处同一条解析）。阅读器只报告点了哪一处链接。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_topic_link(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    topic_path: String,
    href: String,
    occurrence: u32,
) -> Result<TopicLinkOpenDto, String> {
    let citation = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let content = vault.read(&topic_path).map_err(vault_err)?.content;
        luna_vault::topic_citation_at_link(&topic_path, &content, &href, occurrence as usize)
            .ok()
            .flatten()
    };
    let Some(citation) = citation else {
        return Ok(TopicLinkOpenDto::NotCitation);
    };
    let Some(source_path) = citation.source_path else {
        return Ok(TopicLinkOpenDto::NotCitation);
    };
    let anchor = locator_anchor(&citation.meta.locator, &citation.meta.source_hash);
    let (_, outcome) = open_source_at_anchor(&app, &state, &source_path, &anchor, true)?;
    Ok(TopicLinkOpenDto::Opened {
        outcome,
        title: luna_sync_core::links::title_of(&source_path).to_owned(),
    })
}

/// 筛选清单里点一条出处：按出处 ID 取定位锚、版本优先打开（与正文链接同一条解析）。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_topic_citation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    topic_path: String,
    citation_id: String,
) -> Result<TopicLinkOpenDto, String> {
    let citation = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .topic_citation_by_id(&topic_path, &citation_id)
            .map_err(vault_err)?
    };
    let Some(citation) = citation else {
        return Ok(TopicLinkOpenDto::NotCitation);
    };
    let Some(source_path) = citation.source_path else {
        return Ok(TopicLinkOpenDto::NotCitation);
    };
    let anchor = locator_anchor(&citation.meta.locator, &citation.meta.source_hash);
    let (_, outcome) = open_source_at_anchor(&app, &state, &source_path, &anchor, true)?;
    Ok(TopicLinkOpenDto::Opened {
        outcome,
        title: luna_sync_core::links::title_of(&source_path).to_owned(),
    })
}

/// 计划出处的定位锚：由封存时的证据定位器构造，前端只传 operation ID 与序号。
fn plan_evidence_anchor(evidence: &luna_vault::PlanEvidence) -> luna_vault::Anchor {
    locator_anchor(&evidence.locator, &evidence.source_hash)
}

/// 打开计划中一条出处并定位到当初引用的那句（K5 / PRD-16 引用可点回原文）。
/// 不调用模型、不写文件；计划已应用清理或失效时返回错误，前端退回按路径打开。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_sealed_plan_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    operation_id: String,
    evidence_index: u32,
) -> Result<CitationOpenOutcome, String> {
    let (path, anchor) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 计划已应用清理、被处置或 ID 无效：同一个稳定码，前端据此退回按路径打开。
        let unavailable = || "PLAN_SOURCE_UNAVAILABLE".to_owned();
        let id = uuid::Uuid::parse_str(&operation_id).map_err(|_| unavailable())?;
        // 增量计划与新主题初稿共用这条打开出处的路：先认计划，没有再认初稿。
        let evidence = match vault.load_sealed_ingest_plan(id) {
            Ok(plan) => plan.evidence,
            Err(_) => {
                vault
                    .load_topic_draft(id)
                    .map_err(|_| unavailable())?
                    .evidence
            }
        };
        let evidence = evidence
            .get(evidence_index as usize)
            .ok_or_else(unavailable)?;
        (evidence.source_path.clone(), plan_evidence_anchor(evidence))
    };
    open_source_at_anchor(&app, &state, &path, &anchor, true).map(|(_, outcome)| outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_page_with_lagging_index_or_baseline_is_reported_as_degraded_not_failed() {
        let report = |index_updated, baseline_recorded| SealedApplyReport {
            target_path: "Knowledge/项目.md".into(),
            source_paths: vec!["Notes/会议.md".into()],
            applied_items: 2,
            index_updated,
            baseline_recorded,
        };
        assert!(apply_degraded_steps(&report(true, true)).is_empty());
        assert_eq!(
            apply_degraded_steps(&report(false, true)),
            vec![KnowledgeMaintenanceStep::Index]
        );
        assert_eq!(
            apply_degraded_steps(&report(false, false)),
            vec![
                KnowledgeMaintenanceStep::Index,
                KnowledgeMaintenanceStep::Baseline
            ]
        );
    }

    /// 新建主题页确认发布时记「建议已接受」，令牌与展示时同一口径（标签 + 够格笔记集），
    /// 否则展示与接受对不上，建议接受率只剩分母。
    #[test]
    fn publishing_a_topic_draft_accepts_the_same_suggestion_token_that_was_shown() {
        let source = include_str!("knowledge_curation.rs");
        let confirm = source
            .split("pub(crate) fn confirm_topic_draft(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap();
        assert!(confirm.contains("tag_knowledge_notes(&draft.topic_src)"));
        assert!(confirm.contains("\"topic\""));
        assert!(confirm.contains("SuggestionEvent::Accepted"));
        let shown = include_str!("knowledge.rs");
        assert!(
            shown.contains("suggestion_token(vault.root(), \"topic\", &tag, &notes.join(\"\\n\"))")
        );
    }

    const SEALED_SOURCE: &str = "Notes/会议.md";
    const SEALED_TARGET: &str = "Knowledge/项目.md";

    /// 已发布的新格式主题 + 一份已封存并持久化的追加计划（同一份载荷供预览、确认与应用）。
    fn sealed_fixture(dir: &std::path::Path) -> (luna_vault::Vault, luna_vault::IngestPlan) {
        let vault = luna_vault::Vault::open(dir).unwrap();
        let source_path = SEALED_SOURCE;
        let target_path = SEALED_TARGET;
        vault
            .write(source_path, "# 会议\n\n原型已完成。\n")
            .unwrap();
        let source_hash = vault.body_fingerprint(source_path).unwrap();
        let locator = luna_vault::EvidenceLocator {
            heading_path: vec!["会议".into()],
            quote: "原型已完成".into(),
            context: String::new(),
            line_hint: Some(3),
            time_start_ms: None,
            time_end_ms: None,
        };
        let sources = [luna_vault::ClaimSource {
            path: source_path,
            title: "会议",
            source_hash: &source_hash,
            locator: locator.clone(),
            complete_input: true,
        }];
        let claims = [luna_vault::NewClaim {
            id: uuid::Uuid::now_v7(),
            text: "原型已完成",
            kind: luna_vault::ClaimKind::Synthesis,
            sources: &sources,
        }];
        let content = vault
            .scoped_topic_content(
                target_path,
                Some("项目"),
                luna_vault::TopicScope::from_tag("项目"),
                &claims,
            )
            .unwrap();
        vault.publish_scoped_topic(target_path, &content).unwrap();
        let plan = vault
            .seal_ingest_plan(luna_vault::IngestPlan {
                operation_id: uuid::Uuid::now_v7(),
                baseline: vault.topic_plan_baseline(target_path).unwrap(),
                evidence: vec![luna_vault::PlanEvidence {
                    source_path: source_path.into(),
                    source_title: "会议".into(),
                    source_hash,
                    locator,
                    complete_input: true,
                }],
                actions: vec![luna_vault::IngestAction::AppendClaim {
                    claim: luna_vault::PlanClaim {
                        id: uuid::Uuid::now_v7(),
                        text: "预算已批准".into(),
                        kind: luna_vault::ClaimKind::Synthesis,
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
            })
            .unwrap();
        vault.persist_sealed_ingest_plan(&plan).unwrap();
        (vault, plan)
    }

    #[tokio::test(start_paused = true)]
    async fn apply_retries_only_transient_failures_within_budget() {
        use luna_vault::PlanRejectionCode as Code;
        // 提交锁一直被占：用满预算后以同一可重试码结束，不无限重试。
        let mut calls = 0;
        let (result, attempts) = retry_within_budget::<()>(
            || {
                calls += 1;
                Err(Some(Code::CommitLockUnavailable))
            },
            || false,
        )
        .await;
        assert_eq!(result, Err(Some(Code::CommitLockUnavailable)));
        assert_eq!(
            (attempts, calls),
            (APPLY_ATTEMPT_BUDGET, APPLY_ATTEMPT_BUDGET)
        );

        // 第二次拿到锁即成功。
        let mut calls = 0;
        let (result, attempts) = retry_within_budget(
            || {
                calls += 1;
                if calls == 1 {
                    Err(Some(Code::CommitLockUnavailable))
                } else {
                    Ok(calls)
                }
            },
            || false,
        )
        .await;
        assert_eq!((result, attempts), (Ok(2), 2));

        // 版本事实、库已切换、已取消：一次即终。
        for (error, cancelled) in [
            (Some(Code::TargetChanged), false),
            (None, false),
            (Some(Code::WriteFailed), true),
        ] {
            let (result, attempts) = retry_within_budget::<()>(|| Err(error), || cancelled).await;
            assert_eq!((result, attempts), (Err(error), 1));
        }
    }

    /// 「近 7 天」（PRD-16-067）：按录于时间数来源、分三类；被主题页链接过的不算「还没进主题」；
    /// 窗口之前录的不算；结论按这段时间整理出、此刻仍在页上的计。
    #[test]
    fn the_week_counts_sources_by_recorded_time_and_lists_the_unfiled_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _plan) = sealed_fixture(dir.path());
        let note = |path: &str, kind: &str, created: &str| {
            let luna = if kind == "note" {
                format!("luna:\n  created: {created}\n")
            } else {
                format!("luna:\n  type: {kind}\n  created: {created}\n")
            };
            vault
                .write(path, &format!("---\n{luna}---\n# 题\n\n正文。\n"))
                .unwrap();
        };
        note("Notes/已归.md", "note", "2026-09-30T09:00:00+08:00");
        note("Notes/未归.md", "note", "2026-09-30T10:00:00+08:00");
        note("Quick/随手.md", "quick-note", "2026-09-29T09:00:00+08:00");
        note("Recordings/会.md", "recording", "2026-09-28T09:00:00+08:00");
        note("Notes/很早.md", "note", "2020-01-01T09:00:00+08:00");
        vault
            .write_topic(
                "Knowledge/旧页.md",
                "# 旧页\n\n见 [已归](../Notes/已归.md)。\n",
                Some("旧"),
            )
            .unwrap();

        // 2026-09-27 00:00 UTC 起。
        let week = knowledge_week_of(&vault, 1_790_467_200_000).unwrap();
        assert_eq!((week.notes, week.quick_notes, week.recordings), (2, 1, 1));
        assert_eq!(week.unfiled_count, 3, "被主题页链接过的那篇不算");
        assert_eq!(week.unfiled[0].path, "Notes/未归.md", "新近在前");
        assert!(week
            .unfiled
            .iter()
            .all(|source| source.path != "Notes/已归.md"));
        // 夹具里那条结论刚刚生成，落在窗口内。
        assert_eq!(week.claims, 1);
        assert_eq!(week.topics[0].path, SEALED_TARGET);
    }

    /// 知识网络预览卡（PRD-16-062）：只摆站得住的结论（出处都是当前、非分歧），来源一改就
    /// 不再摆它但仍如实报声明数；旧格式页只给页名与摘要。
    #[test]
    fn topic_preview_shows_settled_claims_and_degrades_for_legacy_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _plan) = sealed_fixture(dir.path());
        let preview = topic_preview_of(&vault, SEALED_TARGET).unwrap();
        assert_eq!(
            preview.scope.as_ref().unwrap().includes,
            vec!["项目".to_owned()]
        );
        assert_eq!(preview.claim_count, 1);
        assert_eq!(preview.claims.len(), 1);
        assert_eq!(preview.claims[0].text, "原型已完成");
        assert_eq!(preview.claims[0].citations[0].source_title, "会议");
        assert_eq!(preview.summary, "", "摘要就是第一条结论时不重复摆");

        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型改期。\n")
            .unwrap();
        let preview = topic_preview_of(&vault, SEALED_TARGET).unwrap();
        assert_eq!(preview.claim_count, 1);
        assert!(
            preview.claims.is_empty(),
            "来源已变的结论不在预览卡上冒充定论"
        );

        vault
            .write_topic(
                "Knowledge/旧页.md",
                "# 旧页\n\n一段旧式正文。\n",
                Some("旧"),
            )
            .unwrap();
        let legacy = topic_preview_of(&vault, "Knowledge/旧页.md").unwrap();
        assert!(legacy.scope.is_none() && legacy.claims.is_empty());
        assert_eq!(legacy.summary, "一段旧式正文。");
    }

    #[test]
    fn directory_reports_prepared_suggestions_with_an_exact_count() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _plan) = sealed_fixture(dir.path());
        let rows = super::super::knowledge::topic_directory_rows(
            &vault,
            luna_vault::SourcePolicy::FORMAL_ONLY,
        )
        .unwrap()
        .rows;
        let row = rows.iter().find(|row| row.path == SEALED_TARGET).unwrap();
        assert!(matches!(
            row.ready_item_count,
            KnowledgeCountDto::Exact { count: 1 }
        ));
        // 来源变了，计划失效：不再报告一个点下去就被拒的数。
        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型改期。\n")
            .unwrap();
        let rows = super::super::knowledge::topic_directory_rows(
            &vault,
            luna_vault::SourcePolicy::FORMAL_ONLY,
        )
        .unwrap()
        .rows;
        let row = rows.iter().find(|row| row.path == SEALED_TARGET).unwrap();
        assert!(matches!(
            row.ready_item_count,
            KnowledgeCountDto::NotComputed
        ));
    }

    #[test]
    fn claim_filter_lists_changed_sources_and_skips_legacy_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = sealed_fixture(dir.path());
        let fresh = topic_claims_of(&vault, SEALED_TARGET, TopicClaimFilterDto::NeedsCheck)
            .unwrap()
            .unwrap();
        assert!(fresh.claims.is_empty());
        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型改期。\n")
            .unwrap();
        let stale = topic_claims_of(&vault, SEALED_TARGET, TopicClaimFilterDto::NeedsCheck)
            .unwrap()
            .unwrap();
        assert_eq!(stale.claims.len(), 1);
        assert!(!stale.claims[0].conflict);
        let citation = &stale.claims[0].citations[0];
        assert_eq!(citation.source_title.as_deref(), Some("会议"));
        assert_eq!(citation.state, TopicEvidenceStateDto::SourceChanged);
        assert!(
            topic_claims_of(&vault, SEALED_TARGET, TopicClaimFilterDto::Conflicts)
                .unwrap()
                .unwrap()
                .claims
                .is_empty()
        );
        vault
            .write(
                "Knowledge/旧页.md",
                "---\nluna:\n  topic: true\n---\n# 旧页\n",
            )
            .unwrap();
        assert!(
            topic_claims_of(&vault, "Knowledge/旧页.md", TopicClaimFilterDto::Conflicts)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn only_reruns_that_can_differ_offer_a_replan() {
        use PlanRejectionCodeDto as Code;
        for code in [
            Code::TargetChanged,
            Code::SourceChanged,
            Code::ScopeChanged,
            Code::PlanMissing,
            Code::InvalidPlan,
            Code::IncompleteOutput,
        ] {
            assert!(PlanRejectionDto::app(code).can_replan, "{code:?}");
        }
        for code in [
            Code::UserSuppressed,
            Code::TargetNotReady,
            Code::ApplyPending,
            Code::WriteFailed,
            Code::BudgetExceeded,
        ] {
            assert!(!PlanRejectionDto::app(code).can_replan, "{code:?}");
        }
    }

    #[test]
    fn reading_summary_reports_scope_and_sources_needing_attention() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = sealed_fixture(dir.path());
        let fresh = topic_reading_summary_of(&vault, SEALED_TARGET)
            .unwrap()
            .unwrap();
        assert_eq!(fresh.scope.includes, vec!["项目".to_owned()]);
        assert_eq!((fresh.claims, fresh.conflicts), (1, 0));
        assert!(fresh.attention.is_empty());

        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型改期。\n")
            .unwrap();
        let stale = topic_reading_summary_of(&vault, SEALED_TARGET)
            .unwrap()
            .unwrap();
        assert_eq!(stale.attention.len(), 1);
        assert_eq!(stale.attention[0].source_title.as_deref(), Some("会议"));
        assert_eq!(
            stale.attention[0].state,
            TopicEvidenceStateDto::SourceChanged
        );

        vault
            .write(
                "Knowledge/旧页.md",
                "---\nluna:\n  topic: true\n---\n# 旧页\n",
            )
            .unwrap();
        assert!(topic_reading_summary_of(&vault, "Knowledge/旧页.md")
            .unwrap()
            .is_none());
    }

    #[test]
    fn sealed_preview_reuses_one_artifact_and_reports_stale_source_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let (source_path, target_path) = (SEALED_SOURCE, SEALED_TARGET);
        let before = vault.read(target_path).unwrap().content;
        let reopened = luna_vault::Vault::open(dir.path()).unwrap();
        let id = plan.operation_id.to_string();
        for _ in 0..2 {
            match sealed_ingest_preview_of(&reopened, &id) {
                SealedIngestPreviewDto::Ready {
                    operation_id,
                    target_path: path,
                    before_hash,
                    result_hash,
                    markdown,
                    items,
                    ready_item_count: KnowledgeCountDto::Exact { count: 1 },
                    evidence_count: 1,
                } => {
                    // 撤销凭据：写入前指纹就是预览时这一页的整文件指纹。
                    assert_eq!(
                        before_hash,
                        luna_sync_core::hashing::ContentHash::of(before.as_bytes()).to_string()
                    );
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].action, SealedPlanActionDto::Append);
                    assert_eq!(items[0].text, "预算已批准");
                    assert_eq!(
                        items[0].sources,
                        vec![PlanSourceRefDto {
                            path: source_path.into(),
                            title: "会议".into(),
                            evidence_index: 0,
                        }]
                    );
                    assert_eq!(operation_id, id);
                    assert_eq!(path, target_path);
                    assert_eq!(result_hash, plan.result_hash);
                    assert!(markdown.contains("预算已批准"));
                }
                other => panic!("unexpected preview: {other:?}"),
            }
        }
        assert_eq!(vault.read(target_path).unwrap().content, before);

        vault
            .write(source_path, "# 会议\n\n原型已取消。\n")
            .unwrap();
        match sealed_ingest_preview_of(&reopened, &id) {
            SealedIngestPreviewDto::Rejected { rejection } => {
                assert_eq!(rejection.code, PlanRejectionCodeDto::SourceChanged);
                assert_eq!(rejection.source_path.as_deref(), Some(source_path));
            }
            other => panic!("stale source should be rejected: {other:?}"),
        }
        assert_eq!(vault.read(target_path).unwrap().content, before);
    }

    #[test]
    fn confirmed_plan_applies_the_sealed_bytes_after_restart_without_a_model() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let id = plan.operation_id.to_string();
        assert!(matches!(
            check_confirmable_plan(&vault, &id, "another-preview"),
            Err(luna_vault::PlanRejection::InvalidPlan)
        ));
        assert_eq!(
            check_confirmable_plan(&vault, &id, &plan.result_hash).unwrap(),
            plan
        );
        drop(vault);

        // 模拟重启：新句柄只凭 operation ID 取回同一份载荷，写入字节即当初预览的字节。
        let reopened = luna_vault::Vault::open(dir.path()).unwrap();
        let report = apply_sealed_ingest_plan_locked(&reopened, &id).unwrap();
        assert_eq!(report.target_path, SEALED_TARGET);
        assert_eq!(report.source_paths, vec![SEALED_SOURCE.to_owned()]);
        assert!(report.index_updated && report.baseline_recorded);
        assert_eq!(report.applied_items, 1);
        let written = reopened.read(SEALED_TARGET).unwrap().content;
        assert_eq!(
            luna_sync_core::hashing::ContentHash::of(written.as_bytes()).as_str(),
            plan.result_hash
        );
        // 续跑档还没确认清理前的重复执行识别已落盘结果，不追加第二份声明。
        assert_eq!(
            apply_sealed_ingest_plan_locked(&reopened, &id).unwrap(),
            report
        );
        assert_eq!(reopened.read(SEALED_TARGET).unwrap().content, written);
        reopened
            .discard_sealed_ingest_plan(plan.operation_id)
            .unwrap();
        assert_eq!(
            apply_sealed_ingest_plan_locked(&reopened, &id),
            Err(luna_vault::PlanRejection::PlanMissing)
        );
    }

    #[test]
    fn stale_confirmed_plan_is_not_written_and_is_not_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let before = vault.read(SEALED_TARGET).unwrap().content;
        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型已取消。\n")
            .unwrap();
        let rejection =
            apply_sealed_ingest_plan_locked(&vault, &plan.operation_id.to_string()).unwrap_err();
        assert_eq!(
            rejection,
            luna_vault::PlanRejection::SourceChanged(SEALED_SOURCE.into())
        );
        assert!(!plan_rejection_is_retryable(rejection.code()));
        assert!(plan_rejection_is_retryable(
            luna_vault::PlanRejectionCode::CommitLockUnavailable
        ));
        assert_eq!(vault.read(SEALED_TARGET).unwrap().content, before);
        assert!(
            vault.load_sealed_ingest_plan(plan.operation_id).is_ok(),
            "被拒计划保留载荷，预览可重新给出同一拒绝原因"
        );
    }

    #[test]
    fn decision_records_suppression_then_discards_the_payload() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let id = plan.operation_id.to_string();
        let before = vault.read(SEALED_TARGET).unwrap().content;
        assert!(matches!(
            decide_sealed_ingest_plan_locked(
                &vault,
                &id,
                SealedIngestDecisionKindDto::Rejected,
                true
            ),
            SealedIngestDecisionDto::Rejected { rejection }
                if rejection.code == PlanRejectionCodeDto::ApplyPending
        ));
        assert!(vault.load_sealed_ingest_plan(plan.operation_id).is_ok());

        assert!(matches!(
            decide_sealed_ingest_plan_locked(
                &vault,
                &id,
                SealedIngestDecisionKindDto::Rejected,
                false
            ),
            SealedIngestDecisionDto::Recorded
        ));
        assert_eq!(vault.read(SEALED_TARGET).unwrap().content, before);
        assert_eq!(vault.ingest_decisions().unwrap().len(), 1);
        assert_eq!(
            vault.load_sealed_ingest_plan(plan.operation_id),
            Err(luna_vault::PlanRejection::PlanMissing)
        );
        // 同一来源版本再生成的计划仍被随库否决拦下。
        assert_eq!(
            vault.check_ingest_plan(&plan),
            Err(luna_vault::PlanRejection::UserSuppressed)
        );
    }

    #[test]
    fn deciding_an_already_stale_plan_only_discards_it() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        vault
            .write(SEALED_SOURCE, "# 会议\n\n原型已取消。\n")
            .unwrap();
        match decide_sealed_ingest_plan_locked(
            &vault,
            &plan.operation_id.to_string(),
            SealedIngestDecisionKindDto::Ignored,
            false,
        ) {
            SealedIngestDecisionDto::Discarded { rejection } => {
                assert_eq!(rejection.code, PlanRejectionCodeDto::SourceChanged);
            }
            other => panic!("stale plan should only be discarded: {other:?}"),
        }
        assert!(vault.ingest_decisions().unwrap().is_empty());
        assert_eq!(
            vault.load_sealed_ingest_plan(plan.operation_id),
            Err(luna_vault::PlanRejection::PlanMissing)
        );
    }

    #[test]
    fn topic_update_plan_is_the_same_payload_the_user_confirms() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _unused) = sealed_fixture(dir.path());
        let context = vault.topic_update_context(SEALED_TARGET).unwrap();
        let existing_id = context.existing[0].id;
        let batch = luna_vault::TopicEvidenceBatch {
            evidence: vec![luna_vault::TopicEvidenceInput {
                id: "e0000".into(),
                excerpt: "原型已完成".into(),
                kind: luna_vault::TopicEvidenceKind::FormalNote,
                source: luna_vault::PlanEvidence {
                    source_path: SEALED_SOURCE.into(),
                    source_title: "会议".into(),
                    source_hash: vault.body_fingerprint(SEALED_SOURCE).unwrap(),
                    locator: luna_vault::EvidenceLocator {
                        heading_path: vec!["会议".into()],
                        quote: "原型已完成".into(),
                        context: String::new(),
                        line_hint: Some(3),
                        time_start_ms: None,
                        time_end_ms: None,
                    },
                    complete_input: true,
                },
            }],
            input_complete: true,
            omitted_sources: 0,
            coverage: Vec::new(),
        };
        let raw = r#"{"status":"claims","claims":[
            {"text":"原型已完成","kind":"synthesis","evidence_ids":["e0000"]},
            {"text":"会议记录与既有说法不一致","kind":"conflict","evidence_ids":["e0000"],"conflicts_with":"c0000"}
        ]}"#;
        let update = luna_vault::parse_topic_update(
            &context.scope,
            &context.existing,
            &batch.evidence,
            raw,
            true,
        )
        .unwrap();
        let before = vault.read(SEALED_TARGET).unwrap().content;
        let revisions = luna_vault::PlanRevisions {
            model_revision: "local-llm:fixture".into(),
            prompt_revision: "ingest.topic_update:fixture".into(),
            pipeline_revision: "topic-update-v1".into(),
        };
        let dto =
            plan_from_topic_update_locked(&vault, context, &batch, update, revisions, |_| false);
        let TopicIngestPlanDto::Planned {
            preview:
                SealedIngestPreviewDto::Ready {
                    operation_id,
                    result_hash,
                    markdown,
                    items,
                    ready_item_count,
                    evidence_count,
                    ..
                },
            duplicates,
            ..
        } = dto
        else {
            panic!("expected a sealed plan: {dto:?}");
        };
        assert_eq!(duplicates, 1, "逐字重复的既有声明被剔除并计数");
        assert!(matches!(
            ready_item_count,
            KnowledgeCountDto::Exact { count: 1 }
        ));
        assert_eq!(evidence_count, 1);
        assert!(markdown.contains("会议记录与既有说法不一致"));
        assert_eq!(items[0].action, SealedPlanActionDto::Conflict);
        // 确认前能看到分歧与哪一条并列：带上既有声明原文。
        let related = items[0].related_text.as_deref().expect("分歧带上原说法");
        assert!(before.contains(related), "{related}");
        assert_eq!(
            vault.read(SEALED_TARGET).unwrap().content,
            before,
            "生成只封存不写"
        );
        let plan = check_confirmable_plan(&vault, &operation_id, &result_hash).unwrap();
        assert!(matches!(
            &plan.actions[0],
            luna_vault::IngestAction::MarkConflict { related_id, .. } if *related_id == existing_id
        ));
        assert_eq!(plan.prompt_revision, "ingest.topic_update:fixture");

        let context = vault.topic_update_context(SEALED_TARGET).unwrap();
        assert!(matches!(
            plan_from_topic_update_locked(
                &vault,
                context,
                &batch,
                luna_vault::TopicUpdate::AllDuplicates { duplicates: 2 },
                luna_vault::PlanRevisions {
                    model_revision: "m".into(),
                    prompt_revision: "p".into(),
                    pipeline_revision: "r".into(),
                },
                |_| false,
            ),
            TopicIngestPlanDto::NoOp { duplicates: 2, .. }
        ));
    }

    /// 「待你看」（PRD-16-059）：列出仍然有效的计划并标出低风险；排队中的不列；只引用正式笔记的
    /// 新增算低风险，被看到时记一次影子资格。
    #[test]
    fn review_inbox_lists_pending_plans_and_marks_low_risk_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let mut shown = Vec::new();
        let inbox = review_inbox_of(&vault, |_| None, |op, _| shown.push(op.to_owned())).unwrap();
        assert_eq!(inbox.groups.len(), 1);
        let group = &inbox.groups[0];
        assert_eq!(group.topic_path, SEALED_TARGET);
        assert_eq!(group.operation_id, plan.operation_id.to_string());
        assert!(group.low_risk && group.items.iter().all(|item| item.low_risk));
        assert_eq!(shown, vec![plan.operation_id.to_string()]);

        use crate::processing_queue::PlanApplyState;
        let queued = plan.operation_id.to_string();
        let inbox = review_inbox_of(
            &vault,
            |id| (id == queued).then_some(PlanApplyState::Active),
            |_, _| {},
        )
        .unwrap();
        assert!(inbox.groups.is_empty(), "正在排队应用的不再等你看");

        // 依据换成速记：同一份新增不再算低风险（速记记载要人看一眼）。
        let quick = std::collections::HashSet::from([SEALED_SOURCE.to_owned()]);
        let items = sealed_plan_items(&plan, &std::collections::HashMap::new(), &quick);
        assert!(items
            .iter()
            .all(|item| !item.low_risk && item.quick_note_record));
    }

    #[test]
    fn curation_status_restores_the_latest_plan_and_supersedes_older_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, first) = sealed_fixture(dir.path());
        let restored = topic_curation_status_of(&vault, SEALED_TARGET, false, |_| None).unwrap();
        assert_eq!(restored.apply_phase, TopicApplyPhaseDto::Idle);
        match restored.pending {
            Some(TopicIngestPlanDto::Planned {
                preview: SealedIngestPreviewDto::Ready { operation_id, .. },
                ..
            }) => assert_eq!(operation_id, first.operation_id.to_string()),
            other => panic!("expected the sealed plan back: {other:?}"),
        }
        // 正在排队应用的计划不作为「待确认」恢复，只报告已排队。
        use crate::processing_queue::PlanApplyState;
        let queued = first.operation_id.to_string();
        let status = topic_curation_status_of(&vault, SEALED_TARGET, false, |id| {
            (id == queued).then_some(PlanApplyState::Active)
        })
        .unwrap();
        assert_eq!(status.apply_phase, TopicApplyPhaseDto::Queued);
        assert!(status.pending.is_none());
        // 上一次应用失败：计划照常接回，并如实报告失败，可再次确认。
        let status = topic_curation_status_of(&vault, SEALED_TARGET, false, |id| {
            (id == queued).then_some(PlanApplyState::Failed)
        })
        .unwrap();
        assert_eq!(status.apply_phase, TopicApplyPhaseDto::Failed);
        assert_eq!(
            status.pending_operation_id.as_deref(),
            Some(queued.as_str())
        );
        // 刚应用成功、载荷还没清理：既不接回，也不报失败。
        let status = topic_curation_status_of(&vault, SEALED_TARGET, false, |id| {
            (id == queued).then_some(PlanApplyState::Applied)
        })
        .unwrap();
        assert_eq!(status.apply_phase, TopicApplyPhaseDto::Idle);
        assert!(status.pending.is_none());

        // 同一主题生成新计划时，被取代的旧计划载荷被清理。
        let context = vault.topic_update_context(SEALED_TARGET).unwrap();
        let batch = luna_vault::TopicEvidenceBatch {
            evidence: vec![luna_vault::TopicEvidenceInput {
                id: "e0000".into(),
                excerpt: "原型已完成".into(),
                kind: luna_vault::TopicEvidenceKind::FormalNote,
                source: first.evidence[0].clone(),
            }],
            input_complete: true,
            omitted_sources: 0,
            coverage: Vec::new(),
        };
        let update = luna_vault::parse_topic_update(
            &context.scope,
            &context.existing,
            &batch.evidence,
            r#"{"status":"claims","claims":[{"text":"另一种说法","kind":"synthesis","evidence_ids":["e0000"]}]}"#,
            true,
        )
        .unwrap();
        let revisions = luna_vault::PlanRevisions {
            model_revision: "m".into(),
            prompt_revision: "p".into(),
            pipeline_revision: "r".into(),
        };
        let TopicIngestPlanDto::Planned {
            preview: SealedIngestPreviewDto::Ready { operation_id, .. },
            ..
        } = plan_from_topic_update_locked(&vault, context, &batch, update, revisions, |_| false)
        else {
            panic!("expected a new plan");
        };
        assert_eq!(
            vault.sealed_plan_ids_for_target(SEALED_TARGET),
            vec![uuid::Uuid::parse_str(&operation_id).unwrap()]
        );
    }

    #[test]
    fn explicit_topic_upgrade_is_per_page_and_reversible() {
        let dir = tempfile::tempdir().unwrap();
        let vault = luna_vault::Vault::open(dir.path()).unwrap();
        let legacy = "Knowledge/旧主题.md";
        vault
            .write_topic(legacy, "# 旧主题\n\n- 旧要点\n", Some("项目/旧"))
            .unwrap();
        let original = vault.read(legacy).unwrap().content;
        vault.write("Notes/普通.md", "# 普通\n").unwrap();
        let candidates = topic_upgrade_candidates_of(&vault).unwrap();
        assert_eq!(candidates.omitted, 0);
        assert_eq!(candidates.pages.len(), 1);
        assert_eq!(candidates.pages[0].title, "旧主题");
        let previews =
            topic_upgrade_previews(&vault, &[legacy.to_owned(), "Notes/普通.md".to_owned()]);
        let TopicUpgradePreviewDto::Ready {
            before_hash,
            after_hash,
            scope_includes,
            user_edited: false,
            unverified_lines: 1,
            ..
        } = &previews[0]
        else {
            panic!("legacy topic should be upgradable: {:?}", previews[0]);
        };
        assert_eq!(scope_includes, &vec!["项目/旧".to_owned()]);
        assert!(matches!(
            previews[1],
            TopicUpgradePreviewDto::Rejected {
                code: TopicUpgradeRejectionCodeDto::NotTopic,
                ..
            }
        ));
        assert_eq!(vault.read(legacy).unwrap().content, original, "预览不写");

        let results = topic_upgrade_results(
            &vault,
            &[
                TopicUpgradeRequestDto {
                    path: legacy.into(),
                    before_hash: before_hash.clone(),
                    after_hash: after_hash.clone(),
                },
                TopicUpgradeRequestDto {
                    path: "Knowledge/缺失.md".into(),
                    before_hash: before_hash.clone(),
                    after_hash: after_hash.clone(),
                },
            ],
        );
        let TopicUpgradeResultDto::Upgraded {
            backup_id,
            maintenance: TopicMaintenanceStateDto::Ready,
            ..
        } = &results[0]
        else {
            panic!("upgrade should succeed: {:?}", results[0]);
        };
        assert!(matches!(
            results[1],
            TopicUpgradeResultDto::Rejected {
                code: TopicUpgradeRejectionCodeDto::TargetMissing,
                ..
            }
        ));
        // 升级后的页即可进入增量计划（空声明表 + 可验证底版）。
        assert!(vault
            .topic_update_context(legacy)
            .unwrap()
            .existing
            .is_empty());
        assert!(matches!(
            restore_topic_upgrade_locked(&vault, backup_id),
            TopicUpgradeRestoreDto::Restored { .. }
        ));
        assert_eq!(vault.read(legacy).unwrap().content, original);
        assert!(matches!(
            restore_topic_upgrade_locked(&vault, "not-an-id"),
            TopicUpgradeRestoreDto::Rejected {
                code: TopicUpgradeRejectionCodeDto::BackupMissing
            }
        ));
    }

    #[test]
    fn plan_evidence_anchor_resolves_to_the_quoted_line() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, plan) = sealed_fixture(dir.path());
        let anchor = plan_evidence_anchor(&plan.evidence[0]);
        // 来源未变：记录的行号即精确位置（短句也能定位，不退到标题）。
        assert_eq!(
            resolve_versioned_anchor(&vault, SEALED_SOURCE, &anchor, true).unwrap(),
            luna_vault::AnchorHit::Exact(3)
        );
        vault
            .write(SEALED_SOURCE, "# 会议\n\n补充一句。\n\n原型已完成。\n")
            .unwrap();
        // 来源已变：不再信旧行号，退到复合解析（短引文按设计只到小节）。
        assert_eq!(
            resolve_versioned_anchor(&vault, SEALED_SOURCE, &anchor, true).unwrap(),
            luna_vault::AnchorHit::Heading(1),
            "来源改动后不能跳到旧行号冒充精确"
        );
    }

    #[test]
    fn sealed_preview_rejects_invalid_or_missing_operation_id() {
        let dir = tempfile::tempdir().unwrap();
        let vault = luna_vault::Vault::open(dir.path()).unwrap();
        for (id, expected) in [
            ("not-an-id".to_owned(), PlanRejectionCodeDto::InvalidPlan),
            (
                uuid::Uuid::now_v7().to_string(),
                PlanRejectionCodeDto::PlanMissing,
            ),
        ] {
            match sealed_ingest_preview_of(&vault, &id) {
                SealedIngestPreviewDto::Rejected { rejection } => {
                    assert_eq!(rejection.code, expected);
                }
                other => panic!("unknown operation should be rejected: {other:?}"),
            }
        }
    }
}
