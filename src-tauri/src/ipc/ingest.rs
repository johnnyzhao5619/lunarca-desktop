//! 主题页的生成、增量建议与复核出口（PRD-16 §3.4/§3.12、
//! ADR-134）。
//!
//! **模型只写有出处的声明或「没有新增」，落笔由 Rust 确定性完成**：新建主题
//! （`prepare_topic_draft` → 封存初稿 → 用户确认发布）与既有主题的增量
//! （`prepare_topic_plan_core` → 封存计划 → 用户确认应用，见 `knowledge_curation`）都走
//! 同一纪律——生成与应用分离、预览即落盘、确认绝不重跑模型。
//!
//! 旧版「一条片段 + 小标题锚点」归入（建议条、确认卡、「立即归入」）已随 ADR-134/136 退役：
//! 新格式主题不接受片段写入，旧格式主题经显式升级后进入增量计划（ADR-046：不留第二套算法）。

use super::*;
use luna_core::{
    Job, JobEvent, JobKind, JobOrigin, Priority, ResultPayload, SummaryStyle, TextRef,
};

/// 主题声明是封闭 JSON；预算要覆盖推理模型的思考开销，但触顶时一律拒绝预览。
const TOPIC_GENERATION_OUTPUT_TOKENS: u32 = 2_048;
const TOPIC_PREVIEW_MAX_SOURCES: usize = 256;

// ── 触发档（PRD-16-024）──────────────────────────────────────────────────────

/// 持续整理档（设置 `knowledge.ingest_mode`，PRD-16-048）。
///
/// 与 `distill::DistillMode` 三档结构同构；建议不发通知（PRD-16-004）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IngestMode {
    /// 关闭：不在后台准备建议；主题页上的「检查可添加内容」等显式动作照常可用。
    Off,
    /// 建议：来源变化后在后台准备封存计划，用户在主题页预览、确认后才写入。**不发通知**。
    Suggest,
    /// 自动：与建议同样只准备不写入，直到 PRD-16 §4 的人工评测门槛通过（ADR-136 ③）。
    Auto,
}

impl IngestMode {
    fn parse(raw: &str) -> Self {
        match raw.trim() {
            "auto" => Self::Auto,
            "suggest" => Self::Suggest,
            // 未知取值一律按最保守的档处理——绝不因为读到脏值就替用户跑 LLM 或写库
            // （与 `DistillMode::parse` 同一条纪律）。
            _ => Self::Off,
        }
    }

    fn current(state: &AppState) -> Self {
        Self::parse(
            &state
                .settings_snapshot()
                .get_str("knowledge.ingest_mode")
                .unwrap_or_else(|| "suggest".into()),
        )
    }
}

/// 注册表 `ingest` 域的一条**整句指令**；`zh` 显式选 zh-CN 变体（与 `is_zh` 同源，单测可造样本）。
fn ingest_instruction(key: &str, zh: bool) -> luna_core::Instruction {
    luna_core::prompt::registry().instruction_for_locale(
        luna_core::PromptScope::Ingest,
        key,
        if zh { Some("zh-CN") } else { None },
        &[],
    )
}

/// IPC future 被丢弃时，中止仍在跑的 Provider Job；显式 UI 取消另待接线。
struct CancelTopicPreviewOnDrop(luna_core::CancelToken);

impl Drop for CancelTopicPreviewOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// `Finished` 不等于可用：输出可能已触顶，且 Summarize 只接受 Text 载荷。
/// 保留完整 ResultSchema 到解析点，避免旧 `run_text_job` 的裸字符串路径丢失截断事实。
fn parse_finished_topic_preview(
    result: luna_core::JobResult,
    scope: &luna_vault::TopicScope,
    evidence: &[luna_vault::TopicEvidenceInput],
) -> Result<luna_vault::TopicGeneration, luna_vault::TopicGenerationError> {
    let complete = result.output_truncated.is_none();
    let ResultPayload::Text(raw) = result.payload else {
        return Err(luna_vault::TopicGenerationError::InvalidOutput);
    };
    luna_vault::parse_topic_generation(scope, evidence, &raw, complete)
}

/// 新建证据主题的初稿（K2 / ADR-134）：标签 → Rust 取来源（正式笔记，本设备授权时加上
/// 权威转写，剔除派生重复）→ 证据快照 → JobQueue（后台优先级、严格本地照常）→ ResultSchema
/// 终态 → 封闭解析 → 同锁复核代际与来源 → 封存。只返回预览，不写主题；用户确认时凭
/// operation ID 与预览指纹发布同一份字节（`confirm_topic_draft`）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn prepare_topic_draft(
    app: tauri::AppHandle,
    tag: String,
) -> Result<super::knowledge_curation::TopicDraftPlanDto, String> {
    prepare_topic_draft_core(&app, tag).await
}

/// 新建主题初稿的来源：这个标签下的正式笔记，本设备授权时加上权威转写，剔除派生重复。
/// 对话框准备与后台起草（及其「来源没变就不重跑」的指纹）用同一份清单。
fn draft_source_notes(
    vault: &luna_vault::Vault,
    tag: &str,
    policy: luna_vault::SourcePolicy,
) -> luna_vault::Result<Vec<String>> {
    let mut notes = vault.tag_knowledge_notes(tag)?;
    if policy.transcripts {
        notes.extend(vault.tag_transcript_sources(tag)?);
    }
    Ok(vault.drop_derived_duplicates(notes))
}

/// 后台起草一次最多排几件：每件都是一次分钟级模型调用，一次存量整理不该占满引擎。
const MAX_DRAFT_PREPARATIONS: usize = 2;

/// 后台为已成簇、还没有主题页的标签起草初稿（PRD-16-060）。档位「关」即不排；已有可发布的
/// 初稿、已在排队、或来源自上次准备以来没变的标签都不排。只排、不调模型。
pub(crate) fn enqueue_topic_draft_preparations(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if IngestMode::current(&state) == IngestMode::Off {
        return;
    }
    let tags: Vec<String> = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        let ignored = vault.topic_suggestion_ignores().unwrap_or_default();
        let policy = super::knowledge_curation::source_policy(app, vault);
        let prepared = load_draft_prepared(vault);
        vault
            .topic_suggestions(&ignored)
            .unwrap_or_default()
            .into_iter()
            .map(|suggestion| suggestion.tag)
            .filter(|tag| vault.ready_topic_draft_for(tag).is_none())
            .filter(|tag| {
                draft_source_fingerprint(vault, tag, policy).is_some_and(|fingerprint| {
                    prepared.get(&tag.to_lowercase()) != Some(&fingerprint)
                })
            })
            .collect()
    };
    let mut enqueued = false;
    for tag in tags.into_iter().take(MAX_DRAFT_PREPARATIONS) {
        if state.processing.has_queued_topic_draft(&tag) {
            continue;
        }
        state.processing.enqueue(
            crate::processing_queue::ProcTaskKind::PrepareTopicDraft {
                vault_generation: state.vault_generation(),
                tag: tag.clone(),
            },
            format!("#{tag}"),
        );
        enqueued = true;
    }
    if enqueued {
        super::transcribe::emit_processing_queue(app);
        super::transcribe::start_processing_worker_if_idle(app);
    }
}

/// `PrepareTopicDraft` 的执行体：执行那一刻重读档位与现场；拿到模型结果（含「没有可写」）才记
/// 「这批来源看过」，失败与取消不记、下次触发可重试。只封存初稿，不建页、不发通知。
pub(crate) async fn run_background_topic_draft(
    app: &tauri::AppHandle,
    tag: &str,
    vault_generation: u64,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if IngestMode::current(&state) == IngestMode::Off
        || !state.still_current_vault(vault_generation)
    {
        return Ok(());
    }
    // 候选判定读索引：等开库那一轮扫描做完（V1b），否则按半截索引判簇。
    super::vaults::wait_for_index_ready(&state, vault_generation).await;
    let fingerprint = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if vault.topic_page_for_tag(tag).map_err(vault_err)?.is_some()
            || vault.ready_topic_draft_for(tag).is_some()
        {
            return Ok(());
        }
        let policy = super::knowledge_curation::source_policy(app, vault);
        let Some(fingerprint) = draft_source_fingerprint(vault, tag, policy) else {
            return Ok(());
        };
        if load_draft_prepared(vault).get(&tag.to_lowercase()) == Some(&fingerprint) {
            return Ok(());
        }
        // 走到这里说明这个标签名下已无有效初稿：来源变了的旧封存一并清掉，不留着占盘。
        vault.discard_topic_drafts_for(tag);
        fingerprint
    };
    prepare_topic_draft_core(app, tag.to_owned()).await?;
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    if state.still_current_vault(vault_generation) {
        if let Some(vault) = guard.as_ref() {
            record_draft_prepared(vault, tag, &fingerprint);
        }
    }
    Ok(())
}

/// 一个标签此刻的初稿来源指纹（路径 + 正文版本，排序后取哈希）；没有来源即 `None`。
fn draft_source_fingerprint(
    vault: &luna_vault::Vault,
    tag: &str,
    policy: luna_vault::SourcePolicy,
) -> Option<String> {
    let mut sources: Vec<String> = draft_source_notes(vault, tag, policy)
        .ok()?
        .into_iter()
        .map(|path| {
            let version = vault.body_fingerprint(&path).unwrap_or_default();
            format!("{path}\u{0}{version}")
        })
        .collect();
    if sources.is_empty() {
        return None;
    }
    sources.sort();
    Some(luna_vault::ContentHash::of(sources.join("\n").as_bytes()).to_string())
}

const DRAFT_PREPARED_FILE: &str = ".luna/sidecars/knowledge-draft-prepared.json";

/// 标签（小写）→ 上次后台起草时交给模型的来源指纹。设备本地派生物：丢了只会重跑一次。
fn load_draft_prepared(vault: &luna_vault::Vault) -> std::collections::BTreeMap<String, String> {
    std::fs::read(vault.root().join(DRAFT_PREPARED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn record_draft_prepared(vault: &luna_vault::Vault, tag: &str, fingerprint: &str) {
    let mut prepared = load_draft_prepared(vault);
    prepared.insert(tag.to_lowercase(), fingerprint.to_owned());
    let path = vault.root().join(DRAFT_PREPARED_FILE);
    if let (Some(dir), Ok(bytes)) = (path.parent(), serde_json::to_vec_pretty(&prepared)) {
        if std::fs::create_dir_all(dir).is_ok() {
            let _ = luna_vault::atomic::write_atomic(&path, &bytes);
        }
    }
}

/// 新建主题初稿的准备核心：「创建主题页」对话框与后台起草（PRD-16-060）共用同一条链，
/// 只封存、不建页。
pub(crate) async fn prepare_topic_draft_core(
    app: &tauri::AppHandle,
    tag: String,
) -> Result<super::knowledge_curation::TopicDraftPlanDto, String> {
    let state = app.state::<AppState>();
    use super::knowledge_curation::TopicDraftPlanDto;
    let tag = tag.trim().to_owned();
    if tag.is_empty() {
        return Err(coded_error(app, "TOPIC_NO_SOURCES", "topic-no-sources"));
    }
    let scope = luna_vault::TopicScope::from_tag(&tag);
    let (generation, target, batch) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 同一标签只有一页主题：已有就去那一页补充，不另造。
        if let Some(path) = vault.topic_page_for_tag(&tag).map_err(vault_err)? {
            return Ok(TopicDraftPlanDto::Exists { path });
        }
        let policy = super::knowledge_curation::source_policy(app, vault);
        let mut notes = draft_source_notes(vault, &tag, policy).map_err(vault_err)?;
        if notes.is_empty() {
            return Ok(TopicDraftPlanDto::NoCandidates);
        }
        let beyond = notes.len().saturating_sub(TOPIC_PREVIEW_MAX_SOURCES);
        notes.truncate(TOPIC_PREVIEW_MAX_SOURCES);
        let dir = super::knowledge_product_dir(&state.settings_snapshot());
        let stem = tag.rsplit('/').next().unwrap_or(&tag);
        let target = super::distill::unique_note_path_in(vault, &dir, stem);
        let mut batch = vault.topic_evidence(&notes, policy).map_err(vault_err)?;
        if beyond > 0 {
            batch.input_complete = false;
            batch.omitted_sources = batch
                .omitted_sources
                .saturating_add(u32::try_from(beyond).unwrap_or(u32::MAX));
        }
        (state.vault_generation(), target, batch)
    };
    if batch.evidence.is_empty() {
        return Ok(TopicDraftPlanDto::NoEvidence {
            omitted_sources: batch.omitted_sources,
        });
    }
    let payload =
        match luna_vault::topic_generation_payload(&scope, &batch.evidence, batch.input_complete) {
            Ok(payload) => payload,
            Err(error) => return Ok(topic_draft_rejection(&error)),
        };
    let zh = is_zh(app);
    let prompt_revision = ingest_prompt_revision("topic_generation", zh);
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(payload),
            style: SummaryStyle::Faithful,
            instruction: Some(ingest_instruction("topic_generation", zh)),
        },
        JobOrigin::Ingest,
    );
    job.priority = Priority::Background;
    job.routing.local_only = state.strict_local();
    job.options.output_budget = Some(TOPIC_GENERATION_OUTPUT_TOKENS);
    let mut handle = state.queue.submit(job);
    let _cancel_on_drop = CancelTopicPreviewOnDrop(handle.cancel_token());
    let mut finished = None;
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                finished = Some(result);
                break;
            }
            JobEvent::Failed(error) => return Err(error.code().to_owned()),
            JobEvent::Cancelled => {
                return Err(coded_error(app, "INGEST_CANCELLED", "ingest-cancelled"));
            }
            _ => {}
        }
    }
    let result =
        finished.ok_or_else(|| coded_error(app, "INGEST_NO_RESULT", "ingest-no-result"))?;
    let model_revision = job_model_revision(&result);
    let generated = match parse_finished_topic_preview(result, &scope, &batch.evidence) {
        Ok(generated) => generated,
        Err(error) => return Ok(topic_draft_rejection(&error)),
    };
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    if !state.still_current_vault(generation) {
        return Err(coded_error(
            app,
            "INGEST_VAULT_SWITCHED",
            "ingest-vault-switched-retry",
        ));
    }
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    if !vault
        .topic_evidence_sources_current(&batch.evidence)
        .map_err(vault_err)?
    {
        return Ok(TopicDraftPlanDto::Rejected {
            rejection: super::knowledge_curation::PlanRejectionDto::stale_sources(),
        });
    }
    if matches!(generated, luna_vault::TopicGeneration::NoOp { .. }) {
        return Ok(TopicDraftPlanDto::NoOp {
            input_complete: batch.input_complete,
            omitted_sources: batch.omitted_sources,
        });
    }
    match vault.seal_topic_draft(
        &target,
        &tag,
        scope,
        &batch.evidence,
        &generated,
        batch.input_complete,
        batch.omitted_sources,
        luna_vault::PlanRevisions {
            model_revision,
            prompt_revision,
            pipeline_revision: TOPIC_DRAFT_PIPELINE_REVISION.to_owned(),
        },
    ) {
        Ok(draft) => Ok(super::knowledge_curation::topic_draft_planned(
            vault, &draft,
        )),
        Err(rejection) => Ok(TopicDraftPlanDto::Rejected {
            rejection: rejection.into(),
        }),
    }
}

const TOPIC_DRAFT_PIPELINE_REVISION: &str = "topic-draft-v1";

fn topic_draft_rejection(
    error: &luna_vault::TopicGenerationError,
) -> super::knowledge_curation::TopicDraftPlanDto {
    let rejection = match error {
        luna_vault::TopicGenerationError::IncompleteOutput => {
            luna_vault::PlanRejection::IncompleteOutput
        }
        luna_vault::TopicGenerationError::BudgetExceeded => {
            luna_vault::PlanRejection::BudgetExceeded
        }
        _ => luna_vault::PlanRejection::InvalidPlan,
    };
    super::knowledge_curation::TopicDraftPlanDto::Rejected {
        rejection: rejection.into(),
    }
}

/// 实际跑这次生成的 `provider:model`（计划与初稿都如实记下，不记配置里期望的那个）。
fn job_model_revision(result: &luna_core::JobResult) -> String {
    match result.model_id.as_ref() {
        Some(model) => format!("{}:{}", result.provider_id.as_str(), model.as_str()),
        None => result.provider_id.as_str().to_owned(),
    }
}

/// 增量计划的流水线版本：证据提取规则、封存形状或去重规则变化时递增（TDD-06 §3.9.4）。
// v2（2026-09-27）：增量协议增加 `replaces`（K4 替换），解析与封存随之变化。
const TOPIC_UPDATE_PIPELINE_REVISION: &str = "topic-update-v2";

/// 实际生效模板正文的指纹。服务端目录缺键时回落随包模板，记的就是回落后的那份——只记
/// 目录版本号会让随包模板更新后仍复用旧结果（K0 发现，TDD-06 §3.9.4）。
fn ingest_prompt_revision(key: &str, zh: bool) -> String {
    let template = luna_core::prompt::registry()
        .template_for_locale(
            luna_core::PromptScope::Ingest,
            key,
            if zh { Some("zh-CN") } else { None },
        )
        .unwrap_or_default();
    format!(
        "ingest.{key}:{}",
        luna_vault::ContentHash::of(template.as_bytes())
    )
}

/// 「看过」记录的口径里提示词与流水线那一段：实际生效的增量模板或流水线换了，对旧口径说过的
/// 「没有新增」不沿用，看过的来源会重看一遍（TDD-06 §3.9.4 缓存校验）。
fn topic_update_basis(zh: bool) -> String {
    format!(
        "{}\n{TOPIC_UPDATE_PIPELINE_REVISION}",
        ingest_prompt_revision("topic_update", zh)
    )
}

/// 把 ResultSchema 终态交给封闭增量解析；截断与非文本载荷都不能变成计划。
fn parse_finished_topic_update(
    result: luna_core::JobResult,
    context: &luna_vault::TopicUpdateContext,
    evidence: &[luna_vault::TopicEvidenceInput],
) -> (
    Result<luna_vault::TopicUpdate, luna_vault::TopicGenerationError>,
    String,
) {
    let model_revision = job_model_revision(&result);
    let complete = result.output_truncated.is_none();
    let parsed = match result.payload {
        ResultPayload::Text(raw) => luna_vault::parse_topic_update(
            &context.scope,
            &context.existing,
            evidence,
            &raw,
            complete,
        ),
        _ => Err(luna_vault::TopicGenerationError::InvalidOutput),
    };
    (parsed, model_revision)
}

fn topic_generation_rejection(
    error: &luna_vault::TopicGenerationError,
) -> super::knowledge_curation::TopicIngestPlanDto {
    let rejection = match error {
        luna_vault::TopicGenerationError::IncompleteOutput => {
            luna_vault::PlanRejection::IncompleteOutput
        }
        luna_vault::TopicGenerationError::BudgetExceeded => {
            luna_vault::PlanRejection::BudgetExceeded
        }
        _ => luna_vault::PlanRejection::InvalidPlan,
    };
    super::knowledge_curation::TopicIngestPlanDto::Rejected {
        rejection: rejection.into(),
    }
}

/// 按主题范围为既有新格式主题生成一份增量计划（建议路径，ADR-134）：
/// 候选来源由 Rust 取（与目录「可能相关来源」同一个函数：共享标签且尚未被本页引用），
/// 当前主题与来源快照 → JobQueue（后台优先级、严格本地照常生效）→ ResultSchema 终态
/// → 封闭解析与去重 → 同一把 Vault 锁内复核代际与来源 → 封存并持久化。
///
/// 不写主题：写入只能由用户对同一份预览调用 `confirm_sealed_ingest_plan`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn prepare_topic_ingest_plan(
    app: tauri::AppHandle,
    target_path: String,
) -> Result<super::knowledge_curation::TopicIngestPlanDto, String> {
    prepare_topic_plan_core(&app, &target_path).await
}

/// 这一篇来源变化后，把它作为候选的新格式主题各排一件「准备整理建议」（PRD-16-048「建议」档）。
///
/// 档位为「关」时不排；同一页已有**尚未开始**的准备任务时不重复排（它开跑时自会读到这次变化），
/// 正在执行的那件读的是旧现场，不算数。只入队、不等待：候选判定、候选组指纹与是否已有待确认
/// 计划在任务执行那一刻再核（排队期间来源可能又变了）。
pub(crate) fn enqueue_topic_preparations(app: &tauri::AppHandle, source_rel: &str) {
    let state = app.state::<AppState>();
    if IngestMode::current(&state) == IngestMode::Off {
        return;
    }
    let topics = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        let policy = super::knowledge_curation::source_policy(app, vault);
        vault.topics_with_candidate(source_rel, policy)
    };
    let mut enqueued = false;
    for topic in topics {
        if state.processing.has_queued(&topic, "topic_prepare") {
            continue;
        }
        state.processing.enqueue(
            crate::processing_queue::ProcTaskKind::PrepareTopicPlan {
                vault_generation: state.vault_generation(),
                topic_rel: topic.clone(),
            },
            super::transcribe::note_task_label(&topic),
        );
        enqueued = true;
    }
    if enqueued {
        super::transcribe::emit_processing_queue(app);
        super::transcribe::start_processing_worker_if_idle(app);
    }
    // 同一个边界上，这篇来源也可能让某个标签刚好成簇：为它后台起草新主题（PRD-16-060）。
    enqueue_topic_draft_preparations(app);
}

/// 普通保存/外部编辑后，一篇来源要安静这么久才算「改完了」（TDD-06 §3.9.3 `SourceRevisionChanged`）。
///
/// 编辑器边打边存，每一存都排模型会把「逐键调模型」换成「逐停顿调模型」；安静期内的连续
/// 保存合并成最后一次，只按最新正文准备。
const SOURCE_REVISION_SETTLE: std::time::Duration = std::time::Duration::from_secs(120);

/// 逐来源的去抖槽：同一篇只认最后一次，不同篇互不作废（与知识地图的单例槽不同——
/// 那里刷新的是同一份产物，这里每篇来源牵动的主题各不相同）。
#[derive(Default)]
struct SourceRevisionSlots {
    next: std::sync::atomic::AtomicU64,
    latest: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl SourceRevisionSlots {
    fn bump(&self, source_rel: &str) -> u64 {
        let revision = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if let Ok(mut latest) = self.latest.lock() {
            latest.insert(source_rel.to_owned(), revision);
        }
        revision
    }

    /// 这一拍仍是该来源的最后一次时取走槽位并返回 true；期间又保存过则让给后来那次。
    fn settle(&self, source_rel: &str, revision: u64) -> bool {
        let Ok(mut latest) = self.latest.lock() else {
            return false;
        };
        if latest.get(source_rel) != Some(&revision) {
            return false;
        }
        latest.remove(source_rel);
        true
    }
}

static SOURCE_REVISIONS: std::sync::LazyLock<SourceRevisionSlots> =
    std::sync::LazyLock::new(SourceRevisionSlots::default);

/// 一篇来源的正文经用户保存或外部编辑确认变化后调用：去抖满 [`SOURCE_REVISION_SETTLE`]
/// 才把它牵动的主题交给 [`enqueue_topic_preparations`]。
///
/// 不是 `KnowledgeCreated`：不跑标签/嵌入/地图那条生命周期。主题页、地图等产物不是候选来源，
/// 它们的维护写回走到这里也牵动不到任何主题，不会递归触发综合；只改文件头（如 AI 标签写回）
/// 时正文指纹不变，执行体按候选组指纹安静结束、不调模型。切库后到点的旧拍直接作废。
pub(crate) fn schedule_source_revision(app: &tauri::AppHandle, source_rel: &str) {
    let state = app.state::<AppState>();
    if IngestMode::current(&state) == IngestMode::Off {
        return;
    }
    let revision = SOURCE_REVISIONS.bump(source_rel);
    let generation = state.vault_generation();
    let app = app.clone();
    let source_rel = source_rel.to_owned();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SOURCE_REVISION_SETTLE).await;
        if !SOURCE_REVISIONS.settle(&source_rel, revision)
            || !app.state::<AppState>().still_current_vault(generation)
        {
            return;
        }
        let _ = tauri::async_runtime::spawn_blocking(move || {
            enqueue_topic_preparations(&app, &source_rel);
        })
        .await;
    });
}

/// `PrepareTopicPlan` 的执行体：维护就绪、没有待确认的有效计划、候选里还有没交给模型看过的
/// 版本时，才走与手动检查同一条准备链；否则安静结束。准备出的计划留在盘上，用户打开这一页时
/// 由现场查询接回——不发通知（PRD-16-048）。「看过」由准备链在拿到模型结果后记下；引擎不可用
/// 等失败不记，下次触发可重试。
pub(crate) async fn run_background_topic_prepare(
    app: &tauri::AppHandle,
    topic_rel: &str,
    vault_generation: u64,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    if IngestMode::current(&state) == IngestMode::Off {
        return Ok(Some(topic_rel.to_owned()));
    }
    // 候选与证据读索引：等开库那一轮扫描做完（V1b），否则按半截索引准备建议。
    super::vaults::wait_for_index_ready(&state, vault_generation).await;
    {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        if !state.still_current_vault(vault_generation) {
            return Err(coded_error(
                app,
                "INGEST_VAULT_SWITCHED",
                "ingest-vault-switched-retry",
            ));
        }
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if vault
            .topic_maintenance_state(topic_rel)
            .map_err(vault_err)?
            != luna_vault::TopicMaintenanceState::Ready
        {
            return Ok(Some(topic_rel.to_owned()));
        }
        // 用户还没看过的那份仍然有效：不准备第二份去取代它。
        let pending_ready = vault
            .sealed_plan_ids_for_target(topic_rel)
            .into_iter()
            .any(|id| {
                vault
                    .load_sealed_ingest_plan(id)
                    .and_then(|plan| vault.check_ingest_plan(&plan))
                    .is_ok()
            });
        if pending_ready {
            return Ok(Some(topic_rel.to_owned()));
        }
        let content = vault.read(topic_rel).map_err(vault_err)?.content;
        let policy = super::knowledge_curation::source_policy(app, vault);
        let candidates = vault
            .topic_candidate_sources_of(topic_rel, &content, policy)
            .unwrap_or_default();
        if !vault.has_unconsidered_sources(
            topic_rel,
            &content,
            &topic_update_basis(is_zh(app)),
            &candidates,
        ) {
            return Ok(Some(topic_rel.to_owned()));
        }
    }
    prepare_topic_plan_core(app, topic_rel).await?;
    Ok(Some(topic_rel.to_owned()))
}

/// 手动「检查可添加内容」与后台准备建议共用的同一条准备链（不写主题）。
pub(crate) async fn prepare_topic_plan_core(
    app: &tauri::AppHandle,
    target_path: &str,
) -> Result<super::knowledge_curation::TopicIngestPlanDto, String> {
    let state = app.state::<AppState>();
    let target_path = target_path.to_owned();
    let basis = topic_update_basis(is_zh(app));
    use super::knowledge_curation::TopicIngestPlanDto;
    let (generation, context, batch, target_content, candidates) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let context = match vault.topic_update_context(&target_path) {
            Ok(context) => context,
            Err(rejection) => {
                return Ok(TopicIngestPlanDto::Rejected {
                    rejection: rejection.into(),
                });
            }
        };
        let target_content = vault.read(&target_path).map_err(vault_err)?.content;
        // 来源范围：正式笔记恒在；录音/转写须本设备对本库授权（PRD-16-048）。
        let policy = super::knowledge_curation::source_policy(app, vault);
        match vault.topic_candidate_pool_of(&target_path, &target_content, policy) {
            Ok(pool) if pool.is_empty() => return Ok(TopicIngestPlanDto::NoCandidates),
            Ok(_) => {}
            Err(rejection) => {
                return Ok(TopicIngestPlanDto::Rejected {
                    rejection: rejection.into(),
                });
            }
        }
        // 已被用户忽略/否决的来源版本不再交给模型：否则同一条内容会换个组合重新推荐，
        // 且模型白跑一次必然被拒的生成。与目录候选计数同一个判据。
        let notes = match vault.topic_candidate_sources_of(&target_path, &target_content, policy) {
            Ok(notes) => notes,
            Err(rejection) => {
                return Ok(TopicIngestPlanDto::Rejected {
                    rejection: rejection.into(),
                });
            }
        };
        if notes.is_empty() {
            return Ok(TopicIngestPlanDto::Rejected {
                rejection: luna_vault::PlanRejection::UserSuppressed.into(),
            });
        }
        let candidates = notes.clone();
        // 一次请求只容得下有界的一批：没交给模型看过的来源先轮到，否则前一批「没有新增」的
        // 来源会永远占着队头，后面的来源永远轮不到（TDD-06 §3.9.3）。
        let mut notes =
            vault.unconsidered_sources_first(&target_path, &target_content, &basis, notes);
        // 超出一次请求的来源上限时如实计入遗漏，不静默截成「全部」。
        let beyond = notes.len().saturating_sub(TOPIC_PREVIEW_MAX_SOURCES);
        notes.truncate(TOPIC_PREVIEW_MAX_SOURCES);
        // 上一轮只读了前半的超长来源从断点接着读（TDD-06 §3.9.3）。
        let resume = vault.resume_points(&target_path, &target_content, &basis);
        let mut batch = vault
            .topic_evidence_resuming(&notes, policy, &resume)
            .map_err(vault_err)?;
        if beyond > 0 {
            batch.input_complete = false;
            batch.omitted_sources = batch
                .omitted_sources
                .saturating_add(u32::try_from(beyond).unwrap_or(u32::MAX));
        }
        (
            state.vault_generation(),
            context,
            batch,
            target_content,
            candidates,
        )
    };
    if batch.evidence.is_empty() {
        return Ok(TopicIngestPlanDto::NoEvidence {
            omitted_sources: batch.omitted_sources,
        });
    }
    let payload = match luna_vault::topic_update_payload(
        &context.scope,
        &context.existing,
        &batch.evidence,
        batch.input_complete,
    ) {
        Ok(payload) => payload,
        Err(error) => return Ok(topic_generation_rejection(&error)),
    };
    let zh = is_zh(app);
    let prompt_revision = ingest_prompt_revision("topic_update", zh);
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(payload),
            style: SummaryStyle::Faithful,
            instruction: Some(ingest_instruction("topic_update", zh)),
        },
        JobOrigin::Ingest,
    );
    job.priority = Priority::Background;
    job.routing.local_only = state.strict_local();
    job.options.output_budget = Some(TOPIC_GENERATION_OUTPUT_TOKENS);
    let mut handle = state.queue.submit(job);
    let _cancel_on_drop = CancelTopicPreviewOnDrop(handle.cancel_token());
    let mut finished = None;
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                finished = Some(parse_finished_topic_update(
                    result,
                    &context,
                    &batch.evidence,
                ));
                break;
            }
            JobEvent::Failed(error) => return Err(error.code().to_owned()),
            JobEvent::Cancelled => {
                return Err(coded_error(app, "INGEST_CANCELLED", "ingest-cancelled"));
            }
            _ => {}
        }
    }
    let (parsed, model_revision) =
        finished.ok_or_else(|| coded_error(app, "INGEST_NO_RESULT", "ingest-no-result"))?;
    // 模型看过了这一批（计划或无新增）：下一批让没看过的先轮到，读了前半的下次接着读，
    // 同一批没变就不再调模型。输出不可用时第一次不记看过、下次触发重试，同一版第二次
    // 不可用才记（重试预算 1，TDD-06 §3.9.3）。切库后不记——这一批属于旧库。
    if state.still_current_vault(generation) {
        if let Ok(guard) = state.vault.lock() {
            if let Some(vault) = guard.as_ref() {
                let _ = if parsed.is_ok() {
                    vault.record_considered_sources(
                        &target_path,
                        &target_content,
                        &basis,
                        &batch.coverage,
                        &candidates,
                    )
                } else {
                    vault
                        .record_unusable_output(
                            &target_path,
                            &target_content,
                            &basis,
                            &batch.coverage,
                            &candidates,
                        )
                        .map(|_| ())
                };
            }
        }
    }
    let update = match parsed {
        Ok(update) => update,
        Err(error) => return Ok(topic_generation_rejection(&error)),
    };
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    if !state.still_current_vault(generation) {
        return Err(coded_error(
            app,
            "INGEST_VAULT_SWITCHED",
            "ingest-vault-switched-retry",
        ));
    }
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    if !vault
        .topic_evidence_sources_current(&batch.evidence)
        .map_err(vault_err)?
    {
        return Ok(TopicIngestPlanDto::Rejected {
            rejection: super::knowledge_curation::PlanRejectionDto::stale_sources(),
        });
    }
    Ok(super::knowledge_curation::plan_from_topic_update_locked(
        vault,
        context,
        &batch,
        update,
        luna_vault::PlanRevisions {
            model_revision,
            prompt_revision,
            pipeline_revision: TOPIC_UPDATE_PIPELINE_REVISION.to_owned(),
        },
        |id| state.processing.has_active_plan_apply(id),
    ))
}

/// 忽略一条主题页建议（PRD-16-004：✕ 即对该对象永久安静）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn ignore_topic_suggestion(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    tag: String,
    suggestion_token: String,
) -> Result<(), String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.ignore_topic_suggestion(&tag).map_err(vault_err)?;
    // 不要这条建议，后台备好的初稿也一并丢掉（不留半成品）。
    vault.discard_topic_drafts_for(&tag);
    super::knowledge::record_suggestion_metric(
        &app,
        &vault.root().display().to_string(),
        &suggestion_token,
        luna_metrics::SuggestionEvent::Dismissed,
        None,
    );
    let root = vault.root().display().to_string();
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.topic_censored(
                    &root,
                    &tag,
                    luna_metrics::CohortCensor::Dismissed,
                    chrono::Utc::now().timestamp_millis(),
                )
            })
        });
    Ok(())
}

/// 「转为我的笔记」（ADR-096 ④/⑤）：把产物页当前内容另存为一篇完全归用户的普通笔记。
///
/// 复核态（被人工改动）下它就是「保留我的改动」——另存之后本页随即恢复系统维护
/// （底版快照优先，无底版则以当前内容重登记为新基线；改动已保全，不制造第二份事实）。
/// 系统维护态下就是单纯的转出，本页一个字节不动。
///
/// **前置判定在 Rust**（ADR-003）：前端按态换措辞，但命令永远可达——不是产物页
/// 一律如实拒绝。返回新笔记的库相对路径。
#[tauri::command]
#[specta::specta]
pub(crate) async fn topic_fork_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    topic_rel: String,
) -> Result<String, String> {
    let (fork_rel, restored) = {
        let settings = state.settings_snapshot();
        let dir = super::note_type_dir(&settings, luna_vault::NoteType::Note);
        let zh = super::knowledge::is_zh(&app);
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&topic_rel).map_err(vault_err)?;
        let ownership = luna_vault::ownership_of(&doc.content);
        if ownership == luna_vault::TopicOwnership::NotTopic {
            return Err(coded_error(
                &app,
                "REVIEW_NOT_PENDING",
                "review-not-pending",
            ));
        }
        // ① 把当前内容另存为**用户的**普通笔记——去掉产物身份键，它从此是一篇完全归
        //    用户的笔记（NotebookLM「转为 Source」同构，ADR-096 ④/⑤「转为我的笔记」）。
        //    复核态下这就是「保留我的改动」；系统维护态下就是单纯的转出。
        let stem = luna_vault::title_of(&topic_rel);
        let suffix = backend_text(&app, "review-fork-suffix", "我的笔记");
        let fork_rel =
            super::distill::unique_note_path_in(vault, &dir, &format!("{stem} · {suffix}"));
        let skeleton = format!(
            "---\nluna:\n  schema: {}\n---\n\n{}",
            luna_vault::LUNA_SCHEMA,
            luna_vault::body_of(&doc.content)
        );
        let meta = luna_vault::LunaMeta {
            note_type: luna_vault::NoteType::Note,
            ..luna_vault::LunaMeta::default()
        };
        let content = luna_vault::update_luna(&skeleton, &meta).map_err(vault_err)?;
        vault.publish_new(&fork_rel, &content).map_err(vault_err)?;
        // ② 复核态（被人工改动）随即恢复系统维护：底版快照优先（原样写回）；找不到
        //    底版时以当前内容为新基线——改动已保全在 ①，重登记不制造第二份事实。
        //    地图恒可确定性重建。系统维护态无事可复：页面一个字节不动。
        let restored = if ownership == luna_vault::TopicOwnership::UserOwned {
            Some(restore_product_page(vault, &topic_rel, &doc, zh).map_err(vault_err)?)
        } else {
            None
        };
        (fork_rel, restored)
    };
    if let Some(content) = restored {
        let _ = (NoteWritten {
            path: topic_rel.clone(),
            content,
            origin: "main".to_owned(),
        })
        .emit(&app);
        emit_vault_changed(&app, &topic_rel, false);
    }
    emit_vault_changed(&app, &fork_rel, false);
    Ok(fork_rel)
}

/// 复核态出口②「舍弃改动」（ADR-096 ④）：恢复系统底版。
///
/// 主题页只认**底版快照**（快照链里正文指纹与 `synth_body` 一致的最近一版）——找不到就
/// 如实拒绝并把用户引向「保留我的改动」（那条路恒可走），绝不硬造一份"AI 版本"；
/// 知识地图不受此限（确定性重建，恒有底版）。写盘留快照，舍弃错了可从版本历史找回。
#[tauri::command]
#[specta::specta]
pub(crate) async fn topic_review_discard(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    topic_rel: String,
) -> Result<(), String> {
    let restored = {
        let zh = super::knowledge::is_zh(&app);
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&topic_rel).map_err(vault_err)?;
        if luna_vault::ownership_of(&doc.content) != luna_vault::TopicOwnership::UserOwned {
            return Err(coded_error(
                &app,
                "REVIEW_NOT_PENDING",
                "review-not-pending",
            ));
        }
        if !doc.meta.knowledge_map
            && vault
                .product_baseline_snapshot(&topic_rel)
                .map_err(vault_err)?
                .is_none()
        {
            return Err(coded_error(
                &app,
                "REVIEW_NO_BASELINE",
                "review-no-baseline",
            ));
        }
        restore_product_page(vault, &topic_rel, &doc, zh).map_err(vault_err)?
    };
    let _ = (NoteWritten {
        path: topic_rel.clone(),
        content: restored,
        origin: "main".to_owned(),
    })
    .emit(&app);
    emit_vault_changed(&app, &topic_rel, false);
    Ok(())
}

/// 把一页被人工改动的产物恢复到系统维护（两出口共用的后半步）。
///
/// 地图 = 确定性重建（`knowledge_map_body` 同一输入同一字节）；主题页 = 底版快照原样
/// 写回，找不到底版时以当前内容重登记为新基线（只有「保留」路径会走到这一支——
/// 「舍弃」在调用前已确认底版存在）。
fn restore_product_page(
    vault: &luna_vault::Vault,
    topic_rel: &str,
    doc: &luna_vault::NoteDoc,
    zh: bool,
) -> luna_vault::Result<String> {
    if doc.meta.knowledge_map {
        let body = vault.knowledge_map_body(topic_rel, zh)?;
        vault.write_knowledge_map(topic_rel, &body)?;
        return Ok(vault.read(topic_rel)?.content);
    }
    match vault.product_baseline_snapshot(topic_rel)? {
        Some(baseline) => {
            vault.write(topic_rel, &baseline)?;
            Ok(vault.read(topic_rel)?.content)
        }
        None => vault.reregister_product_baseline(topic_rel),
    }
}

/// 一页笔记的主题页状态（页头徽标的唯一数据源，PRD-16-021）。
///
/// **两个正交的事实各占一位，不再压成一个字符串**：所有权走全应用唯一的
/// [`TopicOwnershipDto`]（主题页目录共用它，TDD-07 §4.1d），「这一页是不是知识地图」
/// 是另一件事。此前 wire 上是 `"map_ai"` 这类**拼出来的**取值——两个维度乘在一个字符串
/// 里，消费面要靠 `startsWith("map_")` 才拆得开，而拆错不会让任何东西变红。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub(crate) struct TopicStateDto {
    pub ownership: TopicOwnershipDto,
    /// 这一页是不是**知识地图**（`luna.knowledge_map`）——它也是一页 AI 维护的资产，
    /// 但不是主题页：不接受归入、也没有「查看喂养来源」。
    pub knowledge_map: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn topic_state(
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<TopicStateDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let ownership = vault.topic_ownership(&note_rel);
    Ok(TopicStateDto {
        ownership: ownership.into(),
        // 不是主题页/地图时这一位没有意义，恒为假——避免出现「不是地图的地图」这种
        // 只可能来自读取顺序的组合。
        knowledge_map: ownership.is_topic()
            && vault
                .read(&note_rel)
                .map(|doc| doc.meta.knowledge_map)
                .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 档位与来源范围、写入互不牵连（K3「关/建议/自动相互独立，已有自动用户不静默扩大」）：
    /// 录音/转写来源只看本设备授权回执，不读档位；后台准备（建议与自动同一条）只封存计划，
    /// 从不自己排应用——证据格式页的写入只有用户确认这一个入口。
    #[test]
    fn modes_never_widen_sources_or_write_evidence_pages_on_their_own() {
        let curation = include_str!("knowledge_curation.rs");
        let policy = curation
            .split("pub(crate) fn source_policy(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap();
        assert!(policy.contains("transcript_source_state("));
        assert!(!policy.contains("IngestMode"), "来源范围不得随档位变化");
        let source = include_str!("ingest.rs");
        let background = source
            .split("pub(crate) async fn run_background_topic_prepare(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap();
        assert!(background.contains("prepare_topic_plan_core("));
        for writer in ["enqueue_plan_apply", "confirm_sealed_ingest_plan"] {
            assert!(
                !background.contains(writer),
                "后台准备不得自己写入：{writer}"
            );
        }
    }

    /// 后台准备接现有基础（K3）：档位为「关」即暂停——触发、入队、执行三处都先看档位，
    /// 已排队的到点安静结束；执行失败（引擎不可用等）不记「看过」，下次触发可重试；
    /// 同一页只合并一件尚未开始的准备，多篇来源同时变化也只排一件，不成洪峰。
    #[test]
    fn background_preparation_pauses_with_off_and_retries_only_on_failure() {
        let source = include_str!("ingest.rs");
        let body = |marker: &str| {
            source
                .split(marker)
                .nth(1)
                .and_then(|rest| rest.split("\n}\n").next())
                .unwrap()
                .to_owned()
        };
        for marker in [
            "pub(crate) fn schedule_source_revision(",
            "pub(crate) fn enqueue_topic_preparations(",
            "pub(crate) async fn run_background_topic_prepare(",
        ] {
            assert!(
                body(marker).contains("IngestMode::current(&state) == IngestMode::Off"),
                "{marker} 没有先看档位"
            );
        }
        // 「看过」只在拿到模型结果之后记：任务失败、取消都在那之前返回，下次触发可重试。
        let core = body("pub(crate) async fn prepare_topic_plan_core(");
        let failed = core.find("JobEvent::Failed(error) => return Err").unwrap();
        let cancelled = core.find("JobEvent::Cancelled =>").unwrap();
        let recorded = core.find("record_considered_sources(").unwrap();
        assert!(
            failed < recorded && cancelled < recorded,
            "失败须在记下「看过」之前返回"
        );
        assert_eq!(
            core.matches("&basis").count(),
            4,
            "排序、续读断点、记录「看过」与记录「输出不可用」须用同一口径（提示词 + 流水线）"
        );
        let unusable = core.find("record_unusable_output(").unwrap();
        assert!(
            failed < unusable && cancelled < unusable,
            "失败须在记下「输出不可用」之前返回"
        );
        let run = body("pub(crate) async fn run_background_topic_prepare(");
        assert!(
            run.contains("topic_update_basis("),
            "后台判定与准备链同一口径"
        );
        assert!(
            run.contains("has_unconsidered_sources("),
            "没看过的版本才调模型"
        );
        assert!(body("pub(crate) fn enqueue_topic_preparations(").contains("has_queued("));
    }

    #[test]
    fn source_revisions_settle_only_the_last_save_per_source() {
        let slots = SourceRevisionSlots::default();
        let first = slots.bump("Notes/A.md");
        let second = slots.bump("Notes/A.md");
        let other = slots.bump("Notes/B.md");
        assert!(
            !slots.settle("Notes/A.md", first),
            "期间又保存过：让给后来那次"
        );
        assert!(slots.settle("Notes/B.md", other), "别的来源不被作废");
        assert!(slots.settle("Notes/A.md", second));
        assert!(!slots.settle("Notes/A.md", second), "同一拍只落地一次");
    }

    /// 每种写入都进同一条派生更新（PRD-16-064）：编辑器保存、外部编辑、捕获框新建、语音速记与
    /// AI 提炼写回都调 `refresh_derived_after_write`，它再喂来源版本槽。
    #[test]
    fn every_write_path_feeds_the_same_derived_refresh() {
        let body_of = |source: &str, marker: &str| -> String {
            crate::ipc::source_without_tests(source)
                .split(marker)
                .nth(1)
                .and_then(|rest| rest.split("\n}\n").next())
                .unwrap_or_else(|| panic!("找不到 {marker}"))
                .to_owned()
        };
        let notes = include_str!("notes.rs");
        let refresh = body_of(notes, "pub(crate) fn refresh_derived_after_write(");
        for step in [
            "spawn_graph_tag_refresh(app, path)",
            "embed::spawn_embed_if_configured(app, path)",
            "schedule_source_revision(app, path)",
        ] {
            assert!(refresh.contains(step), "派生更新少了一步：{step}");
        }
        assert!(body_of(notes, "pub(crate) fn broadcast_note_write(")
            .contains("refresh_derived_after_write(app, path)"));
        assert!(body_of(notes, "pub(crate) fn capture_text_note(")
            .contains("refresh_derived_after_write(app, &doc.path)"));
        assert!(
            body_of(include_str!("mod.rs"), "fn handle_external_vault_change(")
                .contains("refresh_derived_after_write(app, &change.rel_path)")
        );
        assert!(crate::ipc::source_without_tests(include_str!("voice.rs"))
            .contains("refresh_derived_after_write(&app, &doc.path)"));
        assert!(
            crate::ipc::source_without_tests(include_str!("quick_note_refine.rs"))
                .contains("refresh_derived_after_write(&app, &note_rel)")
        );
    }

    #[test]
    fn topic_preview_requires_untruncated_text_result_schema() {
        let scope = luna_vault::TopicScope::from_tag("项目/A");
        let evidence = vec![luna_vault::TopicEvidenceInput {
            id: "e0000".into(),
            excerpt: "本周发布。".into(),
            kind: luna_vault::TopicEvidenceKind::FormalNote,
            source: luna_vault::PlanEvidence {
                source_path: "Notes/A.md".into(),
                source_title: "A".into(),
                source_hash: "sha256:fixture".into(),
                locator: luna_vault::EvidenceLocator {
                    heading_path: vec!["项目 A".into()],
                    quote: "本周发布。".into(),
                    context: String::new(),
                    line_hint: Some(1),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: true,
            },
        }];
        let result = luna_core::JobResult {
            job_id: luna_core::JobId::new(),
            provider_id: luna_core::ProviderId::from("fixture"),
            model_id: None,
            payload: ResultPayload::Text(
                r#"{"status":"claims","claims":[{"text":"项目 A 本周发布。","kind":"synthesis","evidence_ids":["e0000"]}]}"#.into(),
            ),
            usage: luna_core::Usage::default(),
            timings: luna_core::Timings::default(),
            output_truncated: None,
            finished_at: luna_core::Timestamp::ZERO,
        };
        assert!(matches!(
            parse_finished_topic_preview(result.clone(), &scope, &evidence),
            Ok(luna_vault::TopicGeneration::Claims { .. })
        ));
        let mut truncated = result.clone();
        truncated.output_truncated = Some(luna_core::TruncatedBy::Caller);
        assert_eq!(
            parse_finished_topic_preview(truncated, &scope, &evidence),
            Err(luna_vault::TopicGenerationError::IncompleteOutput)
        );
        let mut wrong_payload = result;
        wrong_payload.payload = ResultPayload::Segments(Vec::new());
        assert_eq!(
            parse_finished_topic_preview(wrong_payload, &scope, &evidence),
            Err(luna_vault::TopicGenerationError::InvalidOutput)
        );
    }

    #[test]
    fn topic_update_requires_untruncated_text_and_records_the_model() {
        let context = luna_vault::TopicUpdateContext {
            baseline: luna_vault::PlanBaseline {
                vault_identity: "v".into(),
                target_path: "Knowledge/A.md".into(),
                target_hash: "t".into(),
                scope_hash: "s".into(),
            },
            scope: luna_vault::TopicScope::from_tag("项目/A"),
            existing: Vec::new(),
        };
        let evidence = vec![luna_vault::TopicEvidenceInput {
            id: "e0000".into(),
            excerpt: "本周发布。".into(),
            kind: luna_vault::TopicEvidenceKind::FormalNote,
            source: luna_vault::PlanEvidence {
                source_path: "Notes/A.md".into(),
                source_title: "A".into(),
                source_hash: "sha256:fixture".into(),
                locator: luna_vault::EvidenceLocator {
                    heading_path: vec![],
                    quote: "本周发布。".into(),
                    context: String::new(),
                    line_hint: Some(1),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: true,
            },
        }];
        let result = luna_core::JobResult {
            job_id: luna_core::JobId::new(),
            provider_id: luna_core::ProviderId::from("local-llm"),
            model_id: Some(luna_core::ModelId::from("qwen-test")),
            payload: ResultPayload::Text(
                r#"{"status":"claims","claims":[{"text":"项目 A 本周发布。","kind":"synthesis","evidence_ids":["e0000"]}]}"#.into(),
            ),
            usage: luna_core::Usage::default(),
            timings: luna_core::Timings::default(),
            output_truncated: None,
            finished_at: luna_core::Timestamp::ZERO,
        };
        let (parsed, model) = parse_finished_topic_update(result.clone(), &context, &evidence);
        assert!(matches!(parsed, Ok(luna_vault::TopicUpdate::Claims { .. })));
        assert_eq!(model, "local-llm:qwen-test");
        let mut truncated = result.clone();
        truncated.output_truncated = Some(luna_core::TruncatedBy::Caller);
        assert_eq!(
            parse_finished_topic_update(truncated, &context, &evidence).0,
            Err(luna_vault::TopicGenerationError::IncompleteOutput)
        );
        let mut wrong_payload = result;
        wrong_payload.payload = ResultPayload::Segments(Vec::new());
        assert_eq!(
            parse_finished_topic_update(wrong_payload, &context, &evidence).0,
            Err(luna_vault::TopicGenerationError::InvalidOutput)
        );
        assert!(matches!(
            topic_generation_rejection(&luna_vault::TopicGenerationError::IncompleteOutput),
            super::super::knowledge_curation::TopicIngestPlanDto::Rejected { rejection }
                if rejection.code == super::super::knowledge_curation::PlanRejectionCodeDto::IncompleteOutput
        ));
        assert!(ingest_prompt_revision("topic_update", false).starts_with("ingest.topic_update:"));
    }

    /// 后台起草新主题（PRD-16-060）接在同一条边界上：档位「关」即不排也不跑；拿到模型结果
    /// 之后才记「这批来源看过」（失败、取消都在那之前返回，下次可重试）；只封存不建页。
    #[test]
    fn background_drafts_follow_the_mode_and_record_only_after_a_model_result() {
        let source = crate::ipc::source_without_tests(include_str!("ingest.rs"));
        let body = |marker: &str| -> String {
            source
                .split(marker)
                .nth(1)
                .and_then(|rest| rest.split("\n}\n").next())
                .unwrap_or_else(|| panic!("找不到 {marker}"))
                .to_owned()
        };
        for marker in [
            "pub(crate) fn enqueue_topic_draft_preparations(",
            "pub(crate) async fn run_background_topic_draft(",
        ] {
            assert!(
                body(marker).contains("IngestMode::current(&state) == IngestMode::Off"),
                "{marker} 没有先看档位"
            );
        }
        let run = body("pub(crate) async fn run_background_topic_draft(");
        let prepared = run
            .find("prepare_topic_draft_core(app, tag.to_owned()).await?")
            .unwrap();
        let recorded = run.find("record_draft_prepared(").unwrap();
        assert!(prepared < recorded, "看过只在拿到结果之后记");
        assert!(!run.contains("confirm_topic_draft") && !run.contains("publish_topic_draft"));
        assert!(body("pub(crate) fn enqueue_topic_preparations(")
            .contains("enqueue_topic_draft_preparations(app)"));
    }

    #[test]
    fn draft_source_fingerprint_follows_the_source_versions() {
        let dir = tempfile::tempdir().unwrap();
        let vault = luna_vault::Vault::open(dir.path()).unwrap();
        let policy = luna_vault::SourcePolicy::FORMAL_ONLY;
        assert_eq!(draft_source_fingerprint(&vault, "项目", policy), None);
        for name in ["一", "二", "三"] {
            vault
                .write(
                    &format!("Notes/{name}.md"),
                    &format!("---\ntags: [项目]\n---\n# {name}\n\n进展{name}。\n"),
                )
                .unwrap();
        }
        let first = draft_source_fingerprint(&vault, "项目", policy).unwrap();
        assert_eq!(
            draft_source_fingerprint(&vault, "项目", policy),
            Some(first.clone())
        );
        vault
            .write(
                "Notes/二.md",
                "---\ntags: [项目]\n---\n# 二\n\n进展改了。\n",
            )
            .unwrap();
        assert_ne!(
            draft_source_fingerprint(&vault, "项目", policy),
            Some(first)
        );
    }

    /// 归入档三档解析（PRD-16-024）：脏值一律取最保守档，中档键名刻意不是 `prompt`。
    #[test]
    fn ingest_mode_parses_three_levels_and_defaults_to_off_on_garbage() {
        assert_eq!(IngestMode::parse("auto"), IngestMode::Auto);
        assert_eq!(IngestMode::parse("suggest"), IngestMode::Suggest);
        assert_eq!(IngestMode::parse("off"), IngestMode::Off);
        // 绝不因为读到脏值就替用户跑 LLM 或往知识库里写东西。
        assert_eq!(IngestMode::parse("whatever"), IngestMode::Off);
        // 蒸馏的中档叫 `prompt`，整理的中档叫 `suggest`——两者的呈现刻意不同
        // （前者骑通知、后者只在主题页就地预览），同名会让读代码的人以为它们一样。
        assert_eq!(IngestMode::parse("prompt"), IngestMode::Off);
    }
}
