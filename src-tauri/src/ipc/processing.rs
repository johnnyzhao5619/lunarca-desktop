//! 处理队列**任务编排**（PRD-05 / PRD-04-030/032/055，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：**单 worker 顺序执行一件任务的全过程**——按任务类型派发到 `process_*`，
//! 逐件报告阶段 + 进度、置终态并广播、完成补发通知。五类任务：
//! **链接抓取**（下载 → 转交导入全程，PRD-05-006）/ 导入（解码 → 建笔记 → 媒体落库 →
//! 转写 →（可选）转译）/ 二次转写 / 二次翻译 / 说话人细分。
//!
//! 分层：入队与队列命令在 [`super::transcribe`]，队列数据结构在 [`crate::processing_queue`]，
//! 转写/翻译/细分执行核心在 [`crate::pipeline`]，网络抓取在 [`luna_fetch`]
//! （本模块只做编排，不含引擎与工具细节）。
//!
//! 隐私铁律：进度事件仅计数/阶段/错误码，绝不含转写或译文正文（TDD-12）。

use super::*;
use luna_core::cancel::TaskCancel;

/// 解析离线转写所用的 ASR 模型路径；空槽由模型分配领域继承实时转写槽。
pub(crate) fn resolve_batch_asr_model_path(settings: &Settings, root: &Path) -> Option<PathBuf> {
    whisper_model_path_for_assignment(settings, root, "batch_asr")
}

/// 离线转写语种（`transcribe.lang`）：`auto`/空 = 自动检测（`None`）。
/// 导入首转 / 二次转写 / 说话人细分嵌入选档共用此单一口径。
pub(crate) fn batch_transcribe_lang(settings: &Settings) -> Option<Lang> {
    settings
        .get_str("transcribe.lang")
        .filter(|s| !s.trim().is_empty() && s != "auto")
        .map(|s| Lang::from(s.as_str()))
}

/// 二次翻译（PRD-04-034）目标语言 → 转译配置：显式语种 → `Fixed`（「翻译成 X」，全轨归一到
/// 该语言，源已是该语言的段跳过不改写）；空/`smart`/`auto` → 智能双向（母语↔离线默认外语）。
/// 走默认质量档（`preferred=None`，离线无延迟压力用大模型求准，TDD-04 §6.1）。
pub(crate) fn note_translate_config(
    settings: &Settings,
    detector: Arc<dyn LanguageDetector>,
    target_lang: &str,
    glossary: Arc<[GlossaryEntry]>,
) -> TranslationConfig {
    let target = target_lang.trim();
    let mode = if target.is_empty() || target == "smart" || target == "auto" {
        TargetLangMode::SmartBidirectional
    } else {
        TargetLangMode::Fixed(Lang::from(target))
    };
    TranslationConfig::new(mode, batch_lang_context(settings), detector).with_glossary(glossary)
}

/// 从录音笔记的已登记音频媒体构建重转写分片表（过滤缺失文件）。
pub(crate) fn build_retranscribe_parts(
    vault: &Vault,
    note_rel: &str,
) -> Result<Vec<crate::pipeline::RetranscribePart>, String> {
    let doc = vault.read(note_rel).map_err(vault_err)?;
    Ok(sorted_media_parts(&doc.meta.media)
        .iter()
        .filter_map(|m| {
            let abs = vault.root().join(&m.path);
            abs.is_file().then(|| crate::pipeline::RetranscribePart {
                media_rel: m.path.clone(),
                part_start_ms: m.part_start_ms.unwrap_or(0),
                audio_abs: abs,
                dual_track: m.tracks.as_deref() == Some("mic_system"),
            })
        })
        .collect())
}
/// 文件名净化为安全的笔记/媒体 stem（去路径分隔与不安全字符，空则回退 `import`）。
pub(crate) fn safe_media_stem(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('_').trim();
    if trimmed.is_empty() {
        "import".to_string()
    } else {
        trimmed.to_string()
    }
}
/// 处理队列 worker：单实例顺序处理，按任务类型派发到各 `process_*`，逐件报告阶段 + 进度，
/// 完成置 Done / 取消置 Cancelled / 失败置 Failed 并广播；二次转写/细分完成补发完成通知
/// （PRD-04-032/055、PRD-05-010）。
pub(crate) async fn run_processing_worker(app: tauri::AppHandle) {
    use crate::processing_queue::{ProcPhase, ProcStatus, ProcTaskKind};
    while let Some((id, kind, cancel)) = app.state::<AppState>().processing.next_or_release() {
        emit_processing_queue(&app);
        let tag = kind.tag();
        let lifecycle_receipt = match &kind {
            ProcTaskKind::KnowledgeLifecycle {
                operation_id,
                note_rel,
                origin,
                ..
            } => Some((operation_id.clone(), note_rel.clone(), origin.clone())),
            _ => None,
        };
        // 封存计划应用成功后，续跑档确认不再引用它才清理载荷（见 `discard_applied_sealed_plan`）。
        let mut applied_plan: Option<String> = None;
        // 进度回调：置运行阶段 + 计数并广播（脱敏；总数 0 = 不定量）。
        let progress = {
            let app = app.clone();
            move |step: ProcPhase, done: u32, total: u32| {
                app.state::<AppState>()
                    .processing
                    .set_running(id, step, done, total);
                emit_processing_queue(&app);
            }
        };
        // 执行者回调：与 `progress` 同形，把「谁跑的」记到这一项上并广播。
        // 这一层持有 `id`，而下面各 `process_*` 不该知道队列 id——它们只报告
        // 发生了什么，由这里决定记到哪。
        let on_executor = {
            let app = app.clone();
            move |executor: Option<luna_core::ExecutorInfo>| {
                if executor.is_some() {
                    app.state::<AppState>()
                        .processing
                        .set_executor(id, executor);
                    emit_processing_queue(&app);
                }
            }
        };
        // 排位回调：与 `on_executor` 同形，把服务端排位记到这一项上并广播。
        let on_queue = {
            let app = app.clone();
            move |queue: Option<luna_core::RemoteQueueStatus>| {
                app.state::<AppState>().processing.set_queue(id, queue);
                emit_processing_queue(&app);
            }
        };
        let report = TaskReporter {
            progress: &progress,
            on_executor: &on_executor,
            on_queue: &on_queue,
        };
        // 「导入资料」的观测（PRD-07-070）：本地文件与链接抓取是同一个用户动作的两条路。
        // 在 `match` **之前**取，因为 `kind` 马上就要被移进去；耗时也在这里起表——
        // 排队等待不算在里面（用户感知的是"我发起之后多久能搜到"，而它从执行那一刻起算）。
        let kind_tag = kind.tag();
        let material_import = matches!(
            kind,
            ProcTaskKind::Import { .. } | ProcTaskKind::Fetch { .. }
        );
        let import_started_ms = super::metrics::metric_now_ms();
        // **授权凭据也在这里取**（TDD-12 §2.1「异步观测的授权生命周期契约」）：一次导入
        // 跨越几秒到几分钟，这期间用户完全可能关掉开关或清除历史。**开始时未授权就整条
        // 不记**——到终态再问一次"现在开着吗"，等于把未授权期间发生的事补记进库；而先记
        // 下来、事后再判"当时大概是开着的"更糟：那是拿时间戳当关联键。
        let import_grant = if material_import {
            app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>()
                .and_then(|metrics| metrics.begin())
        } else {
            None
        };
        let result: Result<TaskDone, String> = match kind {
            // 回填的历史项是终态、永远不会被领取（next_or_release 只取 Queued）；这里只为穷尽。
            ProcTaskKind::History { .. } => Err("HISTORY_NOT_RUNNABLE".to_owned()),
            ProcTaskKind::Import {
                path,
                folder,
                link_from,
            } => {
                let done =
                    process_import(&app, ImportSource::local(path, folder), &report, &cancel).await;
                // 「导入素材」（ADR-162 ⑯）：转写成功才在研究笔记里链上它；链不上只记日志。
                if let (Ok(Some(note_rel)), Some(research)) = (&done, &link_from) {
                    super::creator_quote::link_material(&app, research, note_rel);
                }
                done.map(TaskDone::maybe)
            }
            ProcTaskKind::ImportDocument {
                path,
                folder,
                link_from,
            } => {
                let done = super::doc_import::process_document_import(
                    &app,
                    path,
                    folder,
                    report.progress,
                    &cancel,
                )
                .await;
                // 与媒体导入同一时机：成功（含「已导入过」）才链进研究笔记。
                if let (Ok(Some(note_rel)), Some(research)) = (&done, &link_from) {
                    super::creator_quote::link_material(&app, research, note_rel);
                }
                done.map(TaskDone::maybe)
            }
            ProcTaskKind::ClipWeb {
                url,
                folder,
                link_from,
            } => {
                let done =
                    super::web_clip::process_web_clip(&app, url, folder, report.progress, &cancel)
                        .await;
                // 成功（含已剪过）才在研究笔记「来源」一节链上原网页（ADR-164 ⑦）。
                if let (Ok(Some(clip)), Some(research)) = (&done, &link_from) {
                    super::creator_quote::link_source(&app, research, &clip.url, &clip.title);
                }
                done.map(|clip| TaskDone::maybe(clip.map(|clip| clip.note_rel)))
            }
            ProcTaskKind::Fetch {
                url,
                title,
                media,
                into_note,
                folder,
                link_from,
            } => {
                let done = process_fetch(
                    &app,
                    &url,
                    &title,
                    media,
                    FetchTarget {
                        into_note: into_note.as_deref(),
                        folder,
                    },
                    &report,
                    &cancel,
                )
                .await;
                // 创作「添加来源 ▸ 视频链接」（ADR-167 ④）：转写成功才链进研究笔记「素材」一节。
                if let (Ok(Some(note_rel)), Some(research)) = (&done, &link_from) {
                    super::creator_quote::link_material(&app, research, note_rel);
                }
                done.map(TaskDone::maybe)
            }
            ProcTaskKind::Retranscribe {
                note_rel,
                translate_to,
            } => process_retranscribe(&app, note_rel, translate_to, &report, &cancel)
                .await
                .map(TaskDone::maybe),
            ProcTaskKind::Translate {
                note_rel,
                target_lang,
            } => process_translate(&app, note_rel, target_lang, &progress, &cancel)
                .await
                .map(TaskDone::maybe),
            ProcTaskKind::Diarize { note_rel } => {
                process_diarize(&app, note_rel, &progress, &cancel)
                    .await
                    .map(TaskDone::maybe)
            }
            ProcTaskKind::NoteAi {
                note_rel,
                action,
                target_lang,
                summary_style,
            } => {
                // 整篇 AI 动作转入后台（PRD-06-021）：**产物不进正文**，只落进待落地槽，
                // 用户回到这篇笔记时由面板呈现、由他拍板落地。归属指向源笔记——完成通知
                // 与队列项深链都要落回他发起的那一篇。
                let reporter = super::distill::DistillReporter::new(&progress, &cancel);
                super::translate::run_note_ai(
                    &app,
                    &note_rel,
                    &action,
                    target_lang,
                    summary_style,
                    &reporter,
                )
                .await
                .map(|()| TaskDone::at(note_rel))
            }
            ProcTaskKind::Distill {
                note_rel,
                template,
                full_text,
            } => {
                // 整理成知识笔记（PRD-06-051）：进度与取消经 `DistillReporter` 交给执行体，
                // 完成后返回**源笔记**路径——队列项的归属与深链都指向它（用户是从那篇资料
                // 发起的；蒸馏产物由徽标与通知给出入口）。
                let reporter = super::distill::DistillReporter::new(&progress, &cancel);
                super::distill::run_distill(&app, &note_rel, template, full_text, &reporter)
                    .await
                    .map(|_| TaskDone::at(note_rel))
            }
            ProcTaskKind::Merge { notes, origin } => {
                // 汇总成一篇（PRD-06-047）：产出**新**笔记，完成后返回产物路径——队列项
                // 深链指向它（用户点开即见汇总稿）。两种来源共用同一执行体，差别只在候选集政策。
                let reporter = super::distill::DistillReporter::new(&progress, &cancel);
                super::distill::run_merge(&app, &notes, &origin, &reporter)
                    .await
                    .map(TaskDone::at)
            }
            ProcTaskKind::KnowledgeLifecycle {
                operation_id,
                vault_generation,
                note_rel,
                body_hash,
                origin,
                aggregate,
            } => run_knowledge_lifecycle(
                &app,
                &operation_id,
                vault_generation,
                &note_rel,
                &body_hash,
                &origin,
                aggregate,
            )
            .await
            .map(|_| TaskDone::at(note_rel)),
            ProcTaskKind::PrepareTopicPlan {
                topic_rel,
                vault_generation,
            } => super::ingest::run_background_topic_prepare(&app, &topic_rel, vault_generation)
                .await
                .map(TaskDone::maybe),
            ProcTaskKind::PrepareTopicDraft {
                tag,
                vault_generation,
            } => super::ingest::run_background_topic_draft(&app, &tag, vault_generation)
                .await
                .map(|()| TaskDone::maybe(None)),
            ProcTaskKind::ApplyIngestPlan {
                operation_id,
                vault_generation,
                ..
            } => super::knowledge_curation::run_sealed_ingest_apply(
                &app,
                &operation_id,
                vault_generation,
                || cancel.is_cancelled(),
            )
            .await
            .map(|report| {
                applied_plan = Some(operation_id);
                TaskDone::at(report.target_path)
            }),
        };
        // 终态**单一判定点**：取消已请求即归 Cancelled，无论任务是「被 abort 而报错」还是
        // 「恰好赶在取消前跑完」——避免两种竞态各自留下自相矛盾的终态。
        let status = if cancel.is_cancelled() {
            ProcStatus::Cancelled
        } else {
            match &result {
                Ok(done) => ProcStatus::Done {
                    note_rel: done.note_rel.clone(),
                    awaiting_review: done.awaiting_review,
                },
                Err(code) => ProcStatus::Failed {
                    retryable: failure_is_retryable(code),
                    retry_charges_again: luna_core::error::rerun_charges_again(code),
                    code: code.clone(),
                },
            }
        };
        // **失败必须在本机日志里留一行**：队列面板对非抓取类错误一律显示中性的
        // 「任务未能完成」（`taskFailureText` 只给抓取码配了人话），于是失败码在界面上
        // 无处可见；而这里此前一个字都不打，诊断只能靠翻用量库反推 job → 再对服务端日志。
        // 一次真机排障实测：`DISTILL_JOB: PROVIDER_UNAVAILABLE` 花了十几步才还原出来。
        // 只打**码**，不打载荷——路径与正文归脱敏纪律（TDD-12）。
        if let ProcStatus::Failed {
            code, retryable, ..
        } = &status
        {
            // 冒号后那半**只在它本身也是一个稳定码时**留下：错误串的后半可能带路径或
            // 上游原文（`FETCH_IO: /Users/…`），那是日志脱敏明禁的内容（TDD-12）——但
            // `DISTILL_JOB: INVALID_INPUT` 的后半是 `JobError::code()`，是一个不含任何
            // 载荷的枚举投影，把它一起切掉是**过度脱敏**：真机排障时（2026-08-31）日志
            // 只剩一个 `DISTILL_JOB`，而它对所有蒸馏失败都一样，等于没写。
            eprintln!(
                "[queue] 任务失败 kind={kind_tag} retryable={retryable} code={}",
                redacted_failure_code(code)
            );
        }
        let completed = matches!(status, ProcStatus::Done { .. });
        if result.is_err() {
            if let Some((operation_id, note_rel, origin)) = lifecycle_receipt.as_ref() {
                super::knowledge::emit_knowledge_receipt(
                    &app,
                    operation_id,
                    origin,
                    note_rel,
                    super::knowledge::KnowledgeReceiptFacts {
                        failed: true,
                        ..Default::default()
                    },
                );
            }
        }
        if let Some(grant) = import_grant {
            // `arrived` 的含义是**真的搜得到**（TDD-12 §2.1）：`Done` 意味着内容已落盘
            // 且经 `Vault::write` 同步进了全文索引。把"下载完了"记成到达，会让这条指标
            // 回答的问题从"导入好不好使"悄悄变成"网速快不快"。
            record_material_import(&app, grant, &status, import_started_ms);
        }
        let unsuccessful = matches!(status, ProcStatus::Failed { .. } | ProcStatus::Cancelled);
        app.state::<AppState>().processing.set_status(id, status);
        if unsuccessful {
            record_queue_outcome(&app, id);
        }
        emit_processing_queue(&app);
        if let Some(operation_id) = applied_plan.as_deref() {
            // 应用是同步写盘，取消只可能在它之前或之后到达；文件已写成就按成功清理，
            // 与上面「取消已请求即归 Cancelled」的面板口径无关——载荷的去留只看写盘事实。
            super::knowledge_curation::discard_applied_sealed_plan(&app, operation_id);
        }
        if completed {
            notify_processing_done(&app, tag, result.ok().and_then(|d| d.note_rel).as_deref());
            // 一次成功的处理任务可能就是这台设备上的**首个 AI 结果**（PRD-14-050）。引导的
            // 「第一个任务」判的正是它，而判定在 `setup_status` 的用量回填里——不发这一条，
            // 装机状态要等窗口重新聚焦才重读。真机 2026-09-11：点「转写示例」后笔记落了库、
            // 用量记了两条，而那一步一直不打勾，屏幕上偏偏写着「转写完成后这一步会自动打勾」。
            //
            // 发的是「状态可能变了」而不是直接写标志：谁算首个任务的判据只有回填那一处，
            // 在这里再判一次就是第二份名单。
            use tauri_specta::Event as _;
            let _ = super::SetupStatusChanged {}.emit(&app);
        }
    }
    // next_or_release 在无待处理项时已释放 worker 认领。
    emit_processing_queue(&app);
    // 处理忙着时推迟的换账户切库：队列刚静下来，看看能不能接着做。
    crate::vault_lists::retry_pending(&app);
}

#[allow(clippy::too_many_arguments)]
async fn run_knowledge_lifecycle(
    app: &tauri::AppHandle,
    operation_id: &str,
    vault_generation: u64,
    note_rel: &str,
    body_hash: &str,
    origin: &str,
    aggregate: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.still_current_vault(vault_generation) {
        return Err(coded_error(
            app,
            "KNOWLEDGE_LIFECYCLE_VAULT_SWITCHED",
            "knowledge-lifecycle-vault-switched",
        ));
    }
    let current_hash = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.knowledge_body_fingerprint(note_rel)
    };
    if current_hash.as_deref() != Some(body_hash) {
        return Err(coded_error(
            app,
            "KNOWLEDGE_LIFECYCLE_STALE",
            "knowledge-lifecycle-stale",
        ));
    }

    // 地图只负责路由：它需要进入语义索引，命中后由检索器展开真实主题页并移除地图
    // passage；但不打关系标签、不触发归入。写入意图仍经过同一耐久协调器。
    if origin == "knowledge_map" {
        let degraded_steps = if super::embed::embed_if_configured(app, note_rel)
            .await
            .is_err()
        {
            vec![super::knowledge::KnowledgeMaintenanceStep::Embed]
        } else {
            Vec::new()
        };
        super::knowledge::emit_knowledge_receipt(
            app,
            operation_id,
            origin,
            note_rel,
            super::knowledge::KnowledgeReceiptFacts {
                // 地图自身就是本次更新的那一页——回执必须点名它，不能只说「已更新」。
                updated_topics: vec![note_rel.to_owned()],
                degraded_steps,
                ..Default::default()
            },
        );
        return Ok(());
    }

    let settings = state.settings_snapshot();
    let tag_enabled = settings.get_bool("vault.graph_ai_tags").unwrap_or(true);
    // embed 只依赖正文，和 tag 左支并行；join 后 tag 已到终态，ingest 才重新读取最新索引。
    let (embed_result, tag_result) =
        tokio::join!(super::embed::embed_if_configured(app, note_rel), async {
            if tag_enabled {
                super::graph::generate_and_apply(app, note_rel, super::graph::TagRefreshMode::Auto)
                    .await
            } else {
                Ok(Vec::new())
            }
        });
    let mut degraded_steps = Vec::new();
    if embed_result.is_err() {
        degraded_steps.push(super::knowledge::KnowledgeMaintenanceStep::Embed);
    }
    // BUSY 不算降级：撞忙 = 已合并进在跑的那一次、结束后按最新内容补跑
    // （`graph::generate_and_apply` 的合并语义），标签结果一样会来。
    if matches!(&tag_result, Err(code) if !super::graph::is_busy_code(code)) {
        degraded_steps.push(super::knowledge::KnowledgeMaintenanceStep::Tag);
    }

    if aggregate {
        // 聚合页（主题页初稿）不再归入另一页，只刷新地图——地图刷新到位即是本次的
        // 「已更新」，刷不动才是降级。
        let mut updated_topics = Vec::new();
        match super::knowledge::ensure_knowledge_map_after_topic(app) {
            Ok(Some(map_rel)) => updated_topics.push(map_rel),
            Ok(None) => {}
            Err(_) => degraded_steps.push(super::knowledge::KnowledgeMaintenanceStep::KnowledgeMap),
        }
        super::knowledge::emit_knowledge_receipt(
            app,
            operation_id,
            origin,
            note_rel,
            super::knowledge::KnowledgeReceiptFacts {
                updated_topics,
                degraded_steps,
                ..Default::default()
            },
        );
        return Ok(());
    }

    // 标签分支已到终态：为把这篇当候选的那些页后台准备整理建议（ADR-134）；档位与
    // 资格在入队与执行两处各读一次，这里不另判。写入只有用户确认这一个入口。
    super::ingest::enqueue_topic_preparations(app, note_rel);
    super::knowledge::emit_knowledge_receipt(
        app,
        operation_id,
        origin,
        note_rel,
        super::knowledge::KnowledgeReceiptFacts {
            degraded_steps,
            ..Default::default()
        },
    );
    Ok(())
}

/// 一件队列任务**跑完之后的事实**。
///
/// **两格一起给，而不是只给归属笔记**：「完成」在这条队列上有两种意思——东西写好了，
/// 或者建议算好了等你确认（归入的建议档）。只回归属路径的话，两者在界面上是同一个绿勾，
/// 而后者主题页一个字节都没动（真机 2026-08-31，批次 H）。
struct TaskDone {
    /// 归属笔记（前端据此跳转）。
    note_rel: Option<String>,
    /// 还有几处等用户确认；`0` = 完成即落盘。
    awaiting_review: u32,
}

impl TaskDone {
    /// 完成即落盘的那一类——绝大多数任务。
    fn at(note_rel: String) -> Self {
        Self::maybe(Some(note_rel))
    }

    /// 同上，归属可缺席（导入/抓取那几条在失败前不一定有落点）。
    fn maybe(note_rel: Option<String>) -> Self {
        Self {
            note_rel,
            awaiting_review: 0,
        }
    }
}

/// 失败码 → **可进日志的那一截**。
///
/// 后端失败串有两种形态：`FETCH_IO: /Users/…`（后半是载荷）与
/// `DISTILL_JOB: INVALID_INPUT`（后半是 `JobError::code()`，一个不含载荷的枚举投影）。
/// 一律只留冒号前那截会把第二种的**全部信息量**一起丢掉——`DISTILL_JOB` 对每一次蒸馏
/// 失败都一样，日志里写它等于没写（真机排障 2026-08-31 实测）。
///
/// 判据收得很紧：后半必须**整体**是一个稳定码（全大写 + 数字 + 下划线，≥3 字符）
/// 才留下。放宽一格就会把「没找到模型：请先下载」这类人话、或路径的第一段放进日志。
/// 历史封顶（PRD-04-062）：条数 + 天数。
const QUEUE_HISTORY_MAX_RECORDS: usize = 200;
const QUEUE_HISTORY_MAX_AGE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 当前库的用量库句柄（历史落点，随库）。没开库 = 不落盘。
fn queue_history_store(state: &AppState) -> Option<luna_vault::UsageStore> {
    let guard = state.vault.lock().ok()?;
    guard.as_ref().map(|vault| vault.usage_store())
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 非成功终态落盘（PRD-04-062）：只存展示字段与**脱敏后的**失败码，绝不存载荷。
/// 落盘失败只打日志——历史是回看用的，不能拖垮队列本身。
fn record_queue_outcome(app: &tauri::AppHandle, id: u64) {
    let state = app.state::<AppState>();
    let Some(dto) = state.processing.task_dto(id) else {
        return;
    };
    let (outcome, code) = match &dto.status {
        crate::processing_queue::ProcStatus::Failed { code, .. } => {
            ("failed", Some(redacted_failure_code(code)))
        }
        crate::processing_queue::ProcStatus::Cancelled => ("cancelled", None),
        _ => return,
    };
    let Some(store) = queue_history_store(&state) else {
        return;
    };
    let record = luna_vault::QueueOutcome {
        id: None,
        kind: dto.kind,
        label: dto.label,
        note_rel: dto.note_rel,
        outcome: outcome.to_owned(),
        code,
        finished_at_ms: unix_now_ms(),
    };
    match store.insert_queue_outcome(&record) {
        Ok(history_id) => {
            state.processing.attach_history_id(id, history_id);
            let _ = store.prune_queue_outcomes(
                QUEUE_HISTORY_MAX_RECORDS,
                unix_now_ms() - QUEUE_HISTORY_MAX_AGE_MS,
            );
        }
        Err(e) => eprintln!("[queue] 非成功终态落盘失败（仅内存生效）：{e}"),
    }
}

/// 绑库后回填上次会话的非成功终态（PRD-04-062）；切库时随 `rebind` 一起换。
pub(crate) fn restore_queue_history(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let Some(store) = queue_history_store(&state) else {
        state.processing.restore_history(Vec::new());
        return;
    };
    let rows = match store.list_queue_outcomes(QUEUE_HISTORY_MAX_RECORDS) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[queue] 读历史失败（本次不回填）：{e}");
            Vec::new()
        }
    };
    // 库里是倒序（最新在前），面板按入队序展示，故正序回填。
    let records = rows
        .into_iter()
        .rev()
        .filter_map(|row| {
            Some(crate::processing_queue::RestoredOutcome {
                history_id: row.id?,
                tag: row.kind,
                label: row.label,
                note_rel: row.note_rel,
                code: if row.outcome == "failed" {
                    Some(row.code.unwrap_or_else(|| "UNKNOWN".to_owned()))
                } else {
                    None
                },
            })
        })
        .collect();
    state.processing.restore_history(records);
    emit_processing_queue(app);
}

/// 逐条移除 / 清段时连带删历史行（PRD-04-062：「别再给我看这些」）。
pub(crate) fn forget_queue_history(app: &tauri::AppHandle, history_id: Option<i64>) {
    let state = app.state::<AppState>();
    let Some(store) = queue_history_store(&state) else {
        return;
    };
    let result = match history_id {
        Some(id) => store.delete_queue_outcome(id).map(|_| ()),
        None => store.clear_queue_outcomes().map(|_| ()),
    };
    if let Err(e) = result {
        eprintln!("[queue] 删历史失败：{e}");
    }
}

fn redacted_failure_code(code: &str) -> String {
    let mut parts = code.splitn(2, ':');
    let head = parts.next().unwrap_or(code).trim();
    match parts.next().map(str::trim) {
        Some(tail)
            if tail.len() >= 3
                && tail
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') =>
        {
            format!("{head}: {tail}")
        }
        _ => head.to_owned(),
    }
}

/// 失败是否值得给「重试」（**判据单一事实源**，队列面板只渲染结果，ADR-003）。
///
/// 默认可重试——绝大多数失败是环境性的（模型未装、Vault 未开、磁盘满、解码任务 panic），
/// 用户装上模型或腾出空间后原样重跑就成。**唯一的例外是抓取撞上的站点侧硬事实**
/// （已下架 / 区域限制 / 登录墙 / 正在直播 / 站点不支持 / URL 非法）：那些重试只会重复失败，
/// 给按钮等于诱导用户空等（PRD-05-006）。
///
/// 判据整体委托给 `luna_fetch::retryable_for_code`——抓取错误码的含义归定义它的 crate，
/// **这里绝不抄第二份码表**（曾经前端与 Rust 各写一份可重试集合，两边都没接线，
/// 且新增错误码时没有任何测试会红）。
fn failure_is_retryable(code: &str) -> bool {
    // 蒸馏的两个**硬事实**码：原样重跑必然再次失败，给「重试」按钮等于诱导空等
    // （「注定失败的按钮比没给更糟」，PRD-05-006 同一裁决）。
    // - EDITED：知识笔记已被用户编辑，覆盖它的唯一合法路径是差异预览（PRD-06-045），
    //   不是重试；
    // - INELIGIBLE：笔记类型不合资格，类型不会因为再跑一次而改变。
    // 其余蒸馏码保持可重试：EMPTY_SOURCE（转写完成后就有内容了）、VAULT_SWITCHED
    // （回到那个库重试是正当的）、JOB/EMPTY（引擎环境性失败）。
    if code.starts_with("DISTILL_EDITED") || code.starts_with("DISTILL_INELIGIBLE") {
        return false;
    }
    // 归入的同类硬事实（PRD-16-024）：`INELIGIBLE` = 这篇是资料/主题页本身，类型不会
    // 因为再跑一次而改变。其余归入码保持可重试：`VAULT_SWITCHED`（回到那个库重试正当）、
    // `EMPTY`/`NO_RESULT`/`JOB`（引擎环境性失败）、`CANCELLED`（用户意图，环境未变）。
    if code.starts_with("INGEST_INELIGIBLE") {
        return false;
    }
    // 封存计划的来源/目标/范围已变或被用户否决：同一份计划原样重跑只会再被拒，
    // 需要重新生成计划（ADR-134）。环境性故障另走 `KNOWLEDGE_APPLY_UNAVAILABLE`，可重试。
    if code.starts_with("KNOWLEDGE_APPLY_STALE") {
        return false;
    }
    // 文档导入（ADR-164）的硬事实：文件本身是扫描件 / 有密码 / 没字 / 太大 / 坏了 / 格式不认 /
    // 本平台不支持——原样重跑只会再失败（真机 2026-10-19：扫描件也给了「重试」）。读取超时
    // （先下载云盘文件）、读取失败（权限）与系统转换超时保持可重试。
    const DOC_HARD_FACTS: [&str; 10] = [
        "WEB_CLIP_NOT_HTML",
        "WEB_CLIP_TOO_LARGE",
        "WEB_CLIP_NO_ARTICLE",
        "DOC_NO_TEXT_LAYER",
        "DOC_ENCRYPTED",
        "DOC_EMPTY",
        "DOC_TOO_LARGE",
        "DOC_CORRUPT",
        "DOC_UNSUPPORTED",
        "DOC_NEEDS_MACOS",
    ];
    if DOC_HARD_FACTS.iter().any(|hard| code.starts_with(hard)) {
        return false;
    }
    // 非抓取错误码（DECODE_FAILED / MODEL_MISSING / TRANSLATE_NO_TRANSCRIPT …）一律
    // 按可重试处理，维持既有行为不变——它们都是环境性失败。
    luna_fetch::retryable_for_code(code).unwrap_or(true)
}

/// 完成通知的深链路由：按笔记 `luna.type` 落到它真正所属的目的地（PRD-05-030）。
/// 录音产物 → `recording://`，导入媒体的转写产物 → `transcribe://`；读不到类型时按录音兜底。
/// 笔记所在空间的深链（ADR-087 七空间）：**按笔记类型选 scheme**。
///
/// 与 [`note_route`] 分开而不是改它：那一个服务的是录音 / 转写两类任务，它的
/// 「不是转写就是录音」在那个语境下成立；笔记 AI 动作可以跑在**任何**一篇上，
/// 沿用它会把一篇普通笔记的完成通知指到 `recording://`——路由解析器把它落到录音空间，
/// 那里没有这一篇，于是用户点了通知什么也不会发生（真机 2026-09-04）。
fn note_space_route(app: &tauri::AppHandle, note_rel: Option<&str>) -> String {
    let Some(rel) = note_rel else {
        return "notes://".to_owned();
    };
    let note_type = {
        let state = app.state::<AppState>();
        let guard = state.vault.lock().ok();
        guard
            .as_ref()
            .and_then(|guard| guard.as_ref())
            .and_then(|vault| vault.read(rel).ok())
            .map(|doc| doc.meta.note_type)
    };
    let scheme = match note_type {
        Some(NoteType::Transcript) => "transcribe",
        Some(NoteType::Recording) => "recording",
        Some(NoteType::QuickNote) => "quick-notes",
        _ => "notes",
    };
    format!("{scheme}://{rel}")
}

fn note_route(app: &tauri::AppHandle, note_rel: Option<&str>) -> String {
    let is_transcript = note_rel
        .and_then(|rel| {
            let state = app.state::<AppState>();
            let guard = state.vault.lock().ok()?;
            let vault = guard.as_ref()?;
            Some(vault.read(rel).ok()?.meta.note_type)
        })
        .is_some_and(|note_type| note_type == NoteType::Transcript);
    let scheme = if is_transcript {
        "transcribe"
    } else {
        "recording"
    };
    // **带上产物路径**：落点那一格据此顺带选中刚产出的这一篇。此前一律发
    // `…://current`——`current` 从来不是路径（解析器把它整段丢弃），于是「转写完成」
    // 通知点开只能得到一张列表，用户还得自己在里面把刚才那一条找出来，而我们这里
    // **手里就有它的路径**。路径裸传，与 `notes://` 同口径。
    match note_rel {
        Some(rel) => format!("{scheme}://{rel}"),
        None => format!("{scheme}://"),
    }
}

/// 长任务完成 → 完成通知（脱敏，仅笔记路径入私有载荷）。
///
/// **本地导入与二次翻译现在也发**（2026-09-06，随 PRD-15-012 系统通道接线）。此前刻意
/// 不发，理由是「用户刚在文件选择器里操作过、注意力还在应用上」——那个理由只在通知一律
/// 走前台 toast 的年代成立。现在路由按在场态分流：在前台是一条 8 秒自动消散的 toast，
/// 切走或收起窗口时才发系统横幅——而后者正是「导入几个小时的会议录音然后去干别的」这个
/// PRD-05-014 场景最需要的那一条提醒（真机 2026-09-06：收起窗口等转写完，什么都没弹）。
fn notify_processing_done(app: &tauri::AppHandle, tag: &str, note_rel: Option<&str>) {
    let mut kind = match tag {
        "import" => "transcribe_imported",
        "translate" => "transcript_translated",
        "retranscribe" => "recording_retranscribed",
        "diarize" => "recording_diarized",
        "fetch" => "transcribe_fetched",
        // 整篇 AI 动作跑完：深链落回源笔记，打开即由面板呈现待落地结果。
        "note_ai" => "note_ai_ready",
        _ => return,
    };
    // 「完成后提示」档（PRD-06-044）：不新造一套通知机构，只把既有完成通知换成带一句
    // 「可生成知识笔记」的变体——深链照旧落到笔记，页头徽标承接动作。
    if note_rel.is_some_and(|rel| super::distill::distill_hint_for(app, rel)) {
        kind = match kind {
            "recording_retranscribed" => "recording_retranscribed_distill",
            "transcribe_fetched" => "transcribe_fetched_distill",
            "transcribe_imported" => "transcribe_imported_distill",
            other => other,
        };
    }
    if let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() {
        nc.publish(
            crate::notifications::NotificationEvent::completion(
                kind,
                crate::notifications::NotificationTarget::Route(match tag {
                    // 笔记 AI 可以跑在任何一篇上，故按这一篇自己的类型选空间。
                    "note_ai" => note_space_route(app, note_rel),
                    _ => note_route(app, note_rel),
                }),
                crate::notifications::PrivatePayload::new(note_rel.unwrap_or("").to_string()),
            ),
            app,
        );
    }
}

/// 一件导入的**来源描述**：本地文件与网络抓取产物的唯一差别都收敛在这里。
///
/// 抓取（PRD-05-006）不复制导入管线，而是先把媒体变成一个本地文件、再以另一种 `ImportSource`
/// 走同一条路——否则「解码 → 建笔记 → 落库 → 转写 → 原子回滚」这五步就得写两遍，
/// 而其中每一步都已被真机反馈修过好几轮（PRD-05-010 的原子回滚尤其）。
pub(crate) struct ImportSource {
    /// 媒体文件绝对路径（本地导入 = 用户的原文件；抓取 = 任务临时目录里的产物）。
    path: PathBuf,
    /// 笔记与媒体的命名 stem（本地导入 = 文件名；抓取 = 视频标题）。
    ///
    /// 抓取不能沿用"从文件名反推"：站点给的文件名可能是 `BV1xx4y1z7Ab`，而用户认得的是标题。
    stem: String,
    /// 内容来源地址（仅抓取在场 → 落 `luna.source_url`，PRD-05-006 溯源）。
    source_url: Option<String>,
    /// 转写完成后是否把媒体留在 Vault 里。
    ///
    /// `false` = 「仅存链接」（PRD-05-006 省磁盘档 / PRD-05-004② 的网络版）：照常解码转写，
    /// 但**不落媒体、不登记分片**，段落挂在来源地址那个边车槽上（`transcript_slots`）。
    /// 本地文件导入恒 `true`——源文件就在用户磁盘上，"不拷贝"是另一条受阻的路（PRD-05-004②）。
    keep_media: bool,
    /// 笔记建在哪个文件夹（库相对路径）；`None` = 转写笔记的默认文件夹。
    folder: Option<String>,
}

impl ImportSource {
    /// 本地文件导入：stem 取文件名；`folder` 见 [`ProcTaskKind::Import`]。
    pub(crate) fn local(path: PathBuf, folder: Option<String>) -> Self {
        let stem = safe_media_stem(
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("import"),
        );
        Self {
            path,
            stem,
            source_url: None,
            keep_media: true,
            folder,
        }
    }

    /// 抓取产物导入：stem 取视频标题，并携带来源地址与"留不留媒体"。
    fn fetched(
        path: PathBuf,
        title: &str,
        url: &str,
        keep_media: bool,
        folder: Option<String>,
    ) -> Self {
        Self {
            path,
            stem: safe_media_stem(title),
            source_url: Some(url.to_owned()),
            keep_media,
            folder,
        }
    }
}

/// 处理单件**导入**：解码 → 建笔记 + 媒体落库 → 转写 →（可选）转译 → 权威写回。返回归档笔记路径。
///
/// 媒体落库按类型分道（PRD-04-058 / PRD-05-004①「拷贝进 Vault」）：**视频拷原容器**
/// （画面是用户要看的产物，转码或只留音轨都会丢画面）、**音频编 16k WAV**（既是回放媒体，
/// 又让多分片虚拟拼接前提成立）。大 IO 一律在阻塞线程池，不占 worker 的 async 线程。
async fn process_import(
    app: &tauri::AppHandle,
    source: ImportSource,
    report: &TaskReporter<'_>,
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;
    let src = source.path.clone();
    // 同步解析所需输入（含批量档模型按需加载；State 在首个 await 前释放）。
    let (provider_id, queue, vault_arc, transcripts_dir, model_name, lang, glossary) = {
        let state = app.state::<AppState>();
        let provider_id = state.ensure_batch_asr_provider(app)?;
        let settings = state.settings_snapshot();
        let root = state.models_root(app);
        let transcripts_dir = source
            .folder
            .clone()
            .unwrap_or_else(|| note_type_dir(&settings, NoteType::Transcript));
        let model_name =
            resolve_batch_asr_model_path(&settings, &root).map(|p| model_display_name(&p));
        let lang = batch_transcribe_lang(&settings);
        (
            provider_id,
            state.queue_handle(),
            state.vault_arc(),
            transcripts_dir,
            model_name,
            lang,
            // 术语表（PRD-05-003）：整批共用同一份，保证跨文件术语一致。
            load_glossary_entries(&state),
        )
    };

    // 1. 解码媒体 → 16k mono PCM（阻塞线程池）。
    (report.progress)(ProcPhase::Decoding, 0, 0);
    let decode_src = src.clone();
    let decode_flag = cancel.flag();
    let pcm = tokio::task::spawn_blocking(move || {
        luna_audio::decode_media_to_pcm(&decode_src, &decode_flag)
    })
    .await
    .map_err(|e| format!("DECODE_TASK_PANIC: {e}"))?
    .map_err(|e| format!("DECODE_FAILED: {e}"))?;

    // 2. 建转写笔记 + 定媒体落点（持锁，仅元数据；大 IO 不在锁内）。
    let plan = {
        let state = app.state::<AppState>();
        plan_import_note(&state, &transcripts_dir, &src, &source.stem, &pcm)?
    };
    let keep_media = source.keep_media;
    // 溯源紧跟建笔记（PRD-05-006）：来源在下载完成那刻就已确定，不等转写跑完才落盘——
    // 中途取消虽会整体回滚，但"笔记在库里的每一刻都带着来源"是可追溯性的最小承诺。
    if let Some(url) = &source.source_url {
        record_source_url(app, &plan.note_rel, url);
    }
    emit_vault_changed(app, &plan.note_rel, false); // 笔记即时可见

    // 3–6 步一旦开始，笔记与媒体就已经在库里了。**导入必须是原子的**：中途取消或失败却把
    // 半成品留在库里，会让「重试」再建一篇 `-2` 笔记并重拷一份 GB 级媒体（`ProcTaskKind::
    // Import` 只带源文件路径，天然不幂等）。故失败即回滚，重试永远从干净状态开始。
    let outcome = import_after_plan(
        app,
        &plan,
        ImportRun {
            queue: &queue,
            vault_arc: &vault_arc,
            provider_id: &provider_id,
            model_name,
            // 导入**只转写不翻译**：翻译是显式动作，在回放视图按需点「翻译」
            // （PRD-05-031 修订——不由一个全局开关替用户决定整批是否翻译）。
            translate_config: None,
            lang,
            glossary: &glossary,
            keep_media,
        },
        pcm,
        &src,
        report,
        cancel,
    )
    .await;

    match outcome {
        Ok((note_rel, translated)) => {
            emit_vault_changed(app, &note_rel, false);
            maybe_enqueue_auto_diarize(app, &note_rel);
            spawn_distill_if_configured(app, &note_rel);
            // 导入转写已权威写回：本设备授权录音/转写作为来源时，为把它当候选的主题准备建议。
            super::ingest::enqueue_topic_preparations(app, &note_rel);
            // 导入产物（转写）入语义索引：资料在检索里降权不排除（ADR-024），故也要有向量。
            super::embed::spawn_embed_if_configured(app, &note_rel);
            // 转译整批停 → 任务落**失败**终态并带稳定码，但**笔记留着、不回滚**：
            // 转写是这件导入的主产物，它已经完整落盘了（ADR-083 ⑧）。上面那些跟着转写
            // 落定的动作也都已经做完——它们与译文出没出来无关。
            crate::pipeline::translate_outcome_into_result(&translated)?;
            Ok(Some(note_rel))
        }
        Err(err) => {
            rollback_import(app, &plan);
            Err(err)
        }
    }
}

/// 把抓取来源写入笔记 frontmatter（失败只记一行脱敏日志）。
///
/// 溯源是增强而非交付前提：写不进去（笔记刚好被外部改动、磁盘只读）不该让整件抓取失败——
/// 用户要的是转写稿，不是那一行 YAML。日志只出主机名（TDD-12）。
fn record_source_url(app: &tauri::AppHandle, note_rel: &str, url: &str) {
    let state = app.state::<AppState>();
    let outcome = state.vault.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|vault| vault.set_source_url(note_rel, url))
    });
    if let Some(Err(err)) = outcome {
        eprintln!(
            "[fetch] 来源写入失败（{}）：{err}",
            luna_fetch::url::redact(url)
        );
    }
}

/// 处理单件**链接抓取**（PRD-05-006 / ADR-023）：下载到任务临时目录 → 以 [`ImportSource`]
/// 走完整导入管线 → 无论成败清理临时目录。
///
/// 三条与本地导入不同的纪律：
/// ① **严格本地模式硬拦截**——抓取是明确的联网行为（AGENTS.md §7），入队时已校验一次，
///    这里在真正联网前再校验一次（队列可能跨重启续跑，而用户重启期间可能刚打开了严格本地）；
/// ② **临时落点在应用缓存目录而非 Vault**——下到一半的 `.part` 不该出现在用户的知识库里，
///    也不该被文件监听器当成新笔记；
/// ③ **临时目录一律清理**，取消与失败也不例外（媒体动辄数百 MB，留下就是静默漏磁盘）。
/// 抓下来的媒体去哪：补进一篇既有笔记（按需取回），或新建的转写笔记建在哪个文件夹。
struct FetchTarget<'a> {
    into_note: Option<&'a str>,
    folder: Option<String>,
}

async fn process_fetch(
    app: &tauri::AppHandle,
    url: &str,
    title: &str,
    media: luna_fetch::FetchMediaKind,
    target: FetchTarget<'_>,
    report: &TaskReporter<'_>,
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;

    let (tool, work_dir) = {
        let state = app.state::<AppState>();
        // 联网前的最后一道护栏（续跑档跨重启恢复的任务同样经此）。
        if state.strict_local() {
            return Err(fetch_err(&luna_fetch::FetchError::StrictLocalBlocked));
        }
        let settings = state.settings_snapshot();
        let models_root = state.models_root(app);
        (
            resolve_fetch_tool(&settings, &models_root)?,
            fetch_work_dir(app, title),
        )
    };

    (report.progress)(ProcPhase::Fetching, 0, 0);
    // 进度节流（`ProgressThrottle`）：yt-dlp 每次进度更新吐一行（约 10 行/秒），而每次上报都是
    // **整队列 JSON 快照 + IPC 广播**。既有的转写进度是"每分片一次"故从不需要节流，抓取是第一个
    // 高频进度源；不节流就是每秒十几次整队列序列化（ADR-011 的精神：高频流不走 JSON）。
    // 首帧与完成帧仍放行，故进度条不会停在 0 或差最后一格。
    let throttle = std::sync::Mutex::new(luna_fetch::ProgressThrottle::new());
    let request = luna_fetch::FetchRequest {
        url: url.to_owned(),
        media,
        dir: work_dir.clone(),
        // 临时文件名与最终笔记名同源：日志/诊断里看到的就是用户认得的那个名字。
        stem: safe_media_stem(title),
    };
    let outcome = luna_fetch::download(
        &tool,
        &request,
        &|p: luna_fetch::FetchProgress| {
            let due = throttle
                .lock()
                .map(|mut throttle| throttle.should_emit(p))
                // 锁中毒（上游 panic）时宁可照常上报：进度多广播几次不是错误，静默停更才是。
                .unwrap_or(true);
            if due {
                let (done, total) = p.as_kib();
                (report.progress)(ProcPhase::Fetching, done, total);
            }
        },
        &|| cancel.is_cancelled(),
    )
    .await;

    // 抓取产物是**过程性中间物**：无论后续导入成功与否都不留（导入成功时媒体已按落地策略
    // 拷进 Vault，临时副本纯属重复占用；失败/取消时更不该留）。
    let cleanup = |dir: &Path| {
        if dir.exists() {
            if let Err(err) = std::fs::remove_dir_all(dir) {
                eprintln!("[fetch] 临时目录清理失败：{err}");
            }
        }
    };

    let fetched = match outcome {
        Ok(fetched) => fetched,
        Err(err) => {
            cleanup(&work_dir);
            return Err(fetch_err(&err));
        }
    };
    // 两条去向：**按需取回**只把媒体补进既有笔记（绝不重跑转写——文稿可能已被校对过）；
    // 首次抓取走完整导入（建笔记 → 转写）。
    let result = match target.into_note {
        Some(note_rel) => attach_media_to_note(app, note_rel, &fetched.path, report.progress).await,
        None => {
            process_import(
                app,
                ImportSource::fetched(fetched.path, title, url, media.keeps_media(), target.folder),
                report,
                cancel,
            )
            .await
        }
    };
    cleanup(&work_dir);
    result
}

/// 把取回的媒体补进一篇既有的「仅存链接」转写笔记（PRD-05-006）：解码取时长 → 落库 →
/// 登记为分片 → **把已有段落迁到媒体那个挂载槽**。不碰正文、不重跑转写。
///
/// 段落必须迁移：挂载槽从"来源地址"变成了"媒体路径"（`transcript_slots` 的判定随
/// `meta.media` 是否为空翻转），若不迁移，回放会读一个空边车——文稿看起来凭空消失了。
async fn attach_media_to_note(
    app: &tauri::AppHandle,
    note_rel: &str,
    src: &Path,
    progress: &(dyn Fn(crate::processing_queue::ProcPhase, u32, u32) + Sync),
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;

    // 解码取时长与（音频档的）WAV 素材。用一枚**未置位**的令牌：取回的取消发生在下载阶段
    // （占时长的是那一步），这里只剩几秒的本地解码，不值得再穿一条取消通道。
    progress(ProcPhase::Decoding, 0, 0);
    let decode_src = src.to_path_buf();
    let flag = TaskCancel::default().flag();
    let pcm =
        tokio::task::spawn_blocking(move || luna_audio::decode_media_to_pcm(&decode_src, &flag))
            .await
            .map_err(|e| format!("DECODE_TASK_PANIC: {e}"))?
            .map_err(|e| format!("DECODE_FAILED: {e}"))?;

    // 复用导入的落点规划：同一套命名与附件目录策略（DRY），只是笔记已经存在。
    let (plan, segments, source_key) = {
        let state = app.state::<AppState>();
        let settings = state.settings_snapshot();
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        let segments = collect_note_segments(vault.root(), &doc.meta);
        // 迁槽前记下旧槽键（此刻还是来源地址），迁完删它那份死边车。
        let source_key = luna_vault::transcript_slots(&doc.meta)
            .into_iter()
            .next()
            .map(|slot| slot.key);
        let stem = Path::new(note_rel)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("media");
        let asset_dir = media_asset_dir(&settings, note_rel);
        let kind = media_kind(&src.to_string_lossy()).unwrap_or(PlaybackMediaKind::Audio);
        let media_rel =
            luna_vault::join_vault_rel(&asset_dir, &import_media_file_name(stem, src, kind));
        let media_abs = vault.root().join(&media_rel);
        if let Some(parent) = media_abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("MKDIR_FAILED: {e}"))?;
        }
        let duration_ms =
            (pcm.samples.len() as u64) * 1000 / u64::from(luna_audio::TARGET_SAMPLE_RATE).max(1);
        (
            ImportPlan {
                note_rel: note_rel.to_owned(),
                media_rel,
                media_abs,
                kind,
                duration_ms,
            },
            segments,
            source_key,
        )
    };

    progress(ProcPhase::Ingesting, 0, 0);
    let src_owned = src.to_path_buf();
    let write_plan = plan.clone();
    tokio::task::spawn_blocking(move || write_import_media(&write_plan, &src_owned, &pcm))
        .await
        .map_err(|e| format!("IMPORT_MEDIA_TASK_PANIC: {e}"))??;

    // **登记媒体与段落迁槽必须是一次调用**：`finalize_recording` 同时接受分片与段落，
    // 一次 `Vault::write` 落定。若拆成"先登记、再写段落"，中间那一刻挂载槽已经切到媒体路径
    // 而媒体槽的边车还没写——此时崩溃/失败就会让回放读一个空边车，用户看到的是"文稿凭空
    // 消失"。分两步还需要一条回滚路径，而这里根本不需要。
    {
        let state = app.state::<AppState>();
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 书签沿用原笔记：精确层的毫秒 + 可读层的备注（否则导入转写会把备注冲掉）。
        let bookmarks = {
            let doc = vault.read(note_rel).map_err(vault_err)?;
            luna_vault::bookmarks_of(&doc.meta.bookmarks, &doc.content)
        };
        vault
            .finalize_recording(
                note_rel,
                &[luna_vault::RecordingPart {
                    path: plan.media_rel.clone(),
                    start_ms: 0,
                    end_ms: plan.duration_ms,
                    duration: Some(plan.duration_ms as f64 / 1000.0),
                    model: None,
                    lang: None,
                    tracks: None,
                }],
                &segments,
                &bookmarks,
                // 只是把已有段落迁到媒体槽，转写本身没变：权威性沿用原笔记。
                luna_vault::TranscriptAuthority::Unchanged,
            )
            .map_err(vault_err)?;
        // 段落已落到媒体槽，来源槽那份成了死数据——边车与段落一一对应，故一并清掉。
        if let Some(url) = source_key.as_deref() {
            let _ = luna_vault::remove_sidecar_file(
                &vault.root().join(luna_vault::sidecar_path_for(url)),
            );
        }
    }
    emit_vault_changed(app, note_rel, false);
    Ok(Some(note_rel.to_owned()))
}

/// 由设置解析出可用的抓取工具（路径 + 代理）。
pub(crate) fn resolve_fetch_tool(
    settings: &Settings,
    models_root: &Path,
) -> Result<luna_fetch::FetchTool, String> {
    let explicit = settings
        .get_str("transcribe.fetch_tool_path")
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let proxy = settings.get_str("transcribe.fetch_proxy");
    // 受管安装（ADR-023 修订）排在系统各处之前，显式设置仍最优先——顺序在 `luna_fetch::tool`。
    // JS 运行时同理：代装的那份优先于系统里的任何一份（合并安装的前提，TDD-03 §11.1f）。
    let managed = crate::fetch_tool::managed_path(models_root);
    let managed_js = crate::fetch_tool::managed_js_runtime_path(models_root);
    luna_fetch::FetchTool::resolve(
        explicit.as_deref(),
        Some(&managed),
        Some(&managed_js),
        proxy.as_deref(),
    )
    .map_err(|err| fetch_err(&err))
}

/// 抓取错误 → IPC 错误串：**只出稳定 code**，由前端本地化为人话（i18n 单一事实源在前端）。
pub(crate) fn fetch_err(err: &luna_fetch::FetchError) -> String {
    err.code().to_owned()
}

/// 本次抓取的临时工作目录：`<app_cache_dir>/fetch/<净化标题>-<单调序号>/`。
///
/// 序号来自进程内计数器而非时间戳——同一秒内入队的两条链接必须落到不同目录，否则第二件的
/// `.part` 会撞上第一件的产物。落 cache 目录是刻意的：它是系统认可的"可被清理"区域，
/// 应用崩溃后残留的目录由 [`purge_fetch_workspace`] 在下次启动时统一清掉。
fn fetch_work_dir(app: &tauri::AppHandle, title: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    fetch_root(app).join(format!("{}-{seq}", safe_media_stem(title)))
}

/// 抓取临时区根目录。
fn fetch_root(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager as _;
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("fetch")
}

/// 启动时清空抓取临时区（上次会话崩溃/强退留下的半成品）。
///
/// 只清我们自己的 `fetch/` 子目录，且它里面只有过程性中间物——媒体已入 Vault 的任务不在此。
pub(crate) fn purge_fetch_workspace(app: &tauri::AppHandle) {
    let root = fetch_root(app);
    if root.exists() {
        if let Err(err) = std::fs::remove_dir_all(&root) {
            eprintln!("[fetch] 临时区清理失败（不影响本次运行）：{err}");
        }
    }
}

/// 导入「建好笔记之后」各步所需的一组入参（避免 `import_after_plan` 参数表爆长）。
struct ImportRun<'a> {
    queue: &'a Arc<luna_jobqueue::JobQueue>,
    vault_arc: &'a Arc<Mutex<Option<Vault>>>,
    provider_id: &'a luna_core::ProviderId,
    model_name: Option<String>,
    translate_config: Option<TranslationConfig>,
    lang: Option<Lang>,
    glossary: &'a [luna_core::GlossaryEntry],
    /// 见 [`ImportSource::keep_media`]：`false` 时跳过媒体落库与分片登记。
    keep_media: bool,
}

/// 媒体落库 → 登记 → 转写 →（可选）转译 → 权威写回。任一步失败即整体失败，
/// 由调用方回滚（本函数不自行清理，保证「谁建的谁收」的单一职责）。
///
/// **「转译整批停」不算这里的失败**（ADR-083 ⑧）：它随返回值带出去，由调用方在**回滚
/// 之外**折成任务终态。走 `Err` 的话会触发 `rollback_import` ——**永久删除**这篇刚建好、
/// 转写完整的笔记。用一次翻译失败去销毁一段已经跑完的转写，比没有信号贵得多。
async fn import_after_plan(
    app: &tauri::AppHandle,
    plan: &ImportPlan,
    run: ImportRun<'_>,
    pcm: luna_core::PcmBuffer,
    src: &Path,
    report: &TaskReporter<'_>,
    cancel: &TaskCancel,
) -> Result<(String, luna_jobqueue::BatchTranslateOutcome), String> {
    use crate::processing_queue::ProcPhase;

    // 3–4. 媒体落库 + 登记分片。**「仅存链接」整段跳过**（PRD-05-006）：那一档要的正是
    //       "不占磁盘"，落库再删等于白拷一遍；不登记分片则让 `transcript_slots` 自然把段落
    //       挂到来源地址上（读写同一口径，无需在写回处再分支）。
    let pcm = if run.keep_media {
        (report.progress)(ProcPhase::Ingesting, 0, 0);
        let pcm = {
            let plan = plan.clone();
            let src = src.to_path_buf();
            tokio::task::spawn_blocking(move || write_import_media(&plan, &src, &pcm).map(|()| pcm))
                .await
                .map_err(|e| format!("IMPORT_MEDIA_TASK_PANIC: {e}"))??
        };
        {
            let state = app.state::<AppState>();
            attach_import_media(&state, plan, run.model_name.clone())?;
        }
        emit_vault_changed(app, &plan.note_rel, false);
        pcm
    } else {
        pcm
    };

    // 5. 转写（单文件无分片计数 → 不定量）。
    (report.progress)(ProcPhase::Transcribing, 0, 0);
    let (segments, executor) = crate::pipeline::transcribe_pcm(
        run.queue,
        run.provider_id,
        pcm,
        run.lang,
        run.glossary,
        cancel,
        report.on_queue,
    )
    .await?;
    // 如实记下谁跑的（PRD-17-033）。`None` = 本机，保持默认。
    (report.on_executor)(executor);

    // 6. 可选转译 + 权威写回。
    if run.translate_config.is_some() {
        (report.progress)(ProcPhase::Translating, 0, 0);
    }
    let translated = crate::pipeline::translate_and_write(
        run.queue,
        run.vault_arc,
        &plan.note_rel,
        segments,
        run.translate_config,
        run.model_name,
        cancel,
    )
    .await?;
    // 取消若发生在转译阶段，上游会跳过翻译并正常返回——此处显式收口为失败，否则一件被
    // 取消的导入会留下「转写完但没翻译」的半成品，且下方还会给用户再排一件自动细分。
    cancel.bail_if_cancelled()?;
    Ok((plan.note_rel.clone(), translated))
}

/// 回滚半成品导入：删媒体文件 + 永久删除本任务刚建的笔记（PRD-05-010）。
///
/// 只删**本次任务自己建的**东西（`plan` 是本任务的产物，几秒前才落盘、不含用户内容），
/// 故用永久删除而非回收站——半成品不该去打扰用户的回收站。源文件从未被移动，重试即可
/// 从头再来。清理失败只记一行脱敏日志：回滚是尽力而为，不能让它把失败原因盖掉。
fn rollback_import(app: &tauri::AppHandle, plan: &ImportPlan) {
    if plan.media_abs.exists() {
        if let Err(e) = std::fs::remove_file(&plan.media_abs) {
            eprintln!("[import] 回滚删除媒体失败：{e}");
        }
    }
    let state = app.state::<AppState>();
    let deleted = state.vault.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|vault| vault.delete(&plan.note_rel, luna_vault::DeleteMode::Permanent))
    });
    match deleted {
        Some(Ok(())) => emit_vault_changed(app, &plan.note_rel, true),
        Some(Err(e)) => eprintln!("[import] 回滚删除笔记失败：{e}"),
        None => {}
    }
}

/// 导入转写完成后按设置自动入队**说话人细分**（`transcribe.diarize_auto`，PRD-05-032）。
///
/// 只作用于**导入**：导入的媒体天生没有任何轨信息，一段会议录音转出来是一整片没有说话人的
/// 文本；而录音的二次转写对双轨件本就在管线内细分（PRD-04-054），再排一件是重复劳动。
///
/// 走**入队**而非内联：复用同一队列的进度呈现、取消与重试，且不拖长导入任务本身的时长
/// （用户此刻已能读到转写稿）。模型未装 → 静默跳过（诚实：无此能力，不该让导入报错）。
fn maybe_enqueue_auto_diarize(app: &tauri::AppHandle, note_rel: &str) {
    let state = app.state::<AppState>();
    if !state
        .settings_snapshot()
        .get_bool("transcribe.diarize_auto")
        .unwrap_or(false)
    {
        return;
    }
    if !crate::diarize_provider::SherpaDiarizeProvider::models_present(&state.models_root(app)) {
        return;
    }
    state.register_diarize_provider_if_available(app);
    if state.processing.has_active(note_rel, "diarize") {
        return;
    }
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::Diarize {
            note_rel: note_rel.to_owned(),
        },
        note_task_label(note_rel),
    );
    // worker 正在跑本件导入 → 下一轮自然取到，无需再认领。
    emit_processing_queue(app);
}

/// 处理单件**二次转写**：全分片转写（报告分片进度）→（可选）转译 → 权威写回 + AI 标签刷新。
/// 单件任务向队列汇报「发生了什么」的两条回调。
///
/// 打包而不是各传各的：它们**必然成对出现**（每个 `process_*` 都既报进度又报执行者），
/// 且都需要闭包捕获同一个队列项 id。平铺开来只会让每个处理函数的参数表继续变长，
/// 而下一条汇报（比如"用了多少额度"）又要再加一个。
struct TaskReporter<'a> {
    progress: &'a (dyn Fn(crate::processing_queue::ProcPhase, u32, u32) + Sync),
    /// 记下实际执行者（PRD-17-033）。`None` = 本机，不写伪记录。
    on_executor: &'a (dyn Fn(Option<luna_core::ExecutorInfo>) + Sync),
    /// 服务端队列里的排位（ADR-049 ⑧）。`None` = 已经不在队列里（开跑了）。
    ///
    /// 与 `on_executor` 同形而不是合并成一个「远程状态」回调：两者的到达时机与频率
    /// 完全不同（执行者一次，排位反复），合起来只会让每个消费者都去判「这次变的是哪个」。
    on_queue: crate::pipeline::QueueObserver<'a>,
}

async fn process_retranscribe(
    app: &tauri::AppHandle,
    note_rel: String,
    translate_to: Option<String>,
    report: &TaskReporter<'_>,
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;
    let (provider_id, queue, vault_arc, lang, translate_config, model_name, parts, glossary) = {
        let state = app.state::<AppState>();
        let provider_id = state.ensure_batch_asr_provider(app)?;
        let settings = state.settings_snapshot();
        let root = state.models_root(app);
        let lang = batch_transcribe_lang(&settings);
        let glossary = load_glossary_entries(&state);
        // 翻译意图随任务走（`translate_to`）：停录后台重转写带目标语言，手动二次转写为 None。
        let translate_config = translate_to.map(|target| {
            note_translate_config(
                &settings,
                Arc::clone(&state.detector),
                &target,
                Arc::from(glossary.clone()),
            )
        });
        let model_name =
            resolve_batch_asr_model_path(&settings, &root).map(|p| model_display_name(&p));
        let parts = {
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            build_retranscribe_parts(vault, &note_rel)?
        };
        (
            provider_id,
            state.queue_handle(),
            state.vault_arc(),
            lang,
            translate_config,
            model_name,
            parts,
            glossary,
        )
    };
    if parts.is_empty() {
        return Err("该笔记没有已登记的音频媒体".into());
    }
    let translating = translate_config.is_some();
    let (segments, executor) = crate::pipeline::transcribe_all_parts(
        &queue,
        &provider_id,
        parts,
        lang,
        &glossary,
        cancel,
        |done, total| (report.progress)(ProcPhase::Transcribing, done, total),
        report.on_queue,
    )
    .await?;
    (report.on_executor)(executor);
    if translating {
        (report.progress)(ProcPhase::Translating, 0, 0);
    }
    let translated = crate::pipeline::translate_and_write(
        &queue,
        &vault_arc,
        &note_rel,
        segments,
        translate_config,
        model_name,
        cancel,
    )
    .await?;
    emit_vault_changed(app, &note_rel, false);
    // 二次转写正文权威落定后自动刷新 AI 关系标签（PRD-06-035）+ 语义索引（PRD-16-028）
    // ——转写是资料，检索里降权不排除（ADR-024），故它也要有向量。
    spawn_graph_tag_refresh(app, &note_rel);
    super::embed::spawn_embed_if_configured(app, &note_rel);
    // 蒸馏的边界事件之一（PRD-06-044）：文本此刻才是权威的，早于此蒸馏等于整理了草稿。
    // `Auto` 档在此后台生成；`Prompt` 档不动手，由完成通知带一句「可生成知识笔记」。
    spawn_distill_if_configured(app, &note_rel);
    // 停录重转写已权威写回：本设备授权录音/转写作为来源时，为把它当候选的主题准备建议。
    super::ingest::enqueue_topic_preparations(app, &note_rel);
    // 折终态放在最后：上面那些跟着的是**转写**的落定（刷新前端 / 关系标签 / 语义索引 /
    // 蒸馏），与译文出没出来无关；用一个 `?` 顺手跳过它们，表现是笔记正文更新了而界面
    // 不知道、索引也不跟（ADR-083 ⑧）。
    crate::pipeline::translate_outcome_into_result(&translated)?;
    Ok(Some(note_rel))
}

/// 处理单件**二次翻译**：清旧译 → 逐句转译（报告分句进度）→ 权威写回，点亮回放译文/双语。
async fn process_translate(
    app: &tauri::AppHandle,
    note_rel: String,
    target_lang: String,
    progress: &(dyn Fn(crate::processing_queue::ProcPhase, u32, u32) + Sync),
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;
    let (config, mut segments, queue, vault_arc) = {
        let state = app.state::<AppState>();
        let settings = state.settings_snapshot();
        let config = note_translate_config(
            &settings,
            Arc::clone(&state.detector),
            &target_lang,
            Arc::from(load_glossary_entries(&state)),
        );
        let segments = {
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            let doc = vault.read(&note_rel).map_err(vault_err)?;
            collect_note_segments(vault.root(), &doc.meta)
        };
        (config, segments, state.queue_handle(), state.vault_arc())
    };
    if segments.is_empty() {
        return Err(coded_error(
            app,
            "TRANSLATE_NO_TRANSCRIPT",
            "translate-no-transcript",
        ));
    }
    // 重译幂等：先清旧译，绝不在被跳过（源==目标）的段上残留旧译。
    for seg in &mut segments {
        seg.translation = None;
    }
    let sink: Arc<dyn JobSink + Send + Sync> = Arc::clone(&queue) as Arc<_>;
    let outcome = luna_jobqueue::translate_segments_batch(
        &sink,
        &config,
        &mut segments,
        cancel,
        |done, total| progress(ProcPhase::Translating, done as u32, total as u32),
    )
    .await;
    // **整批停也照常写回已译出的部分**（ADR-083 ⑧）：那些句子是真的译出来了，丢掉它们
    // 只是多浪费一次。但这件任务**必须以失败告终**——它的产物就是译文，「成功但是空的」
    // 会写进 Vault（唯一事实源）而错误不会，用户看到「翻译完成」却一句译文都没有，
    // 无从知道该重试。折终态同样放在刷新之后（理由见 `translate_outcome_into_result`）。
    crate::pipeline::write_recording_segments(
        &vault_arc,
        &note_rel,
        segments,
        None,
        luna_vault::TranscriptAuthority::Unchanged,
    )
    .await?;
    emit_vault_changed(app, &note_rel, false);
    crate::pipeline::translate_outcome_into_result(&outcome)?;
    Ok(Some(note_rel))
}

/// 处理单件**说话人细分**：逐分片解码+细分（报告分片进度）→ 归属写回。返回笔记路径（即便未写标签）。
async fn process_diarize(
    app: &tauri::AppHandle,
    note_rel: String,
    progress: &(dyn Fn(crate::processing_queue::ProcPhase, u32, u32) + Sync),
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    use crate::processing_queue::ProcPhase;
    let (parts, mut segments, queue, vault_arc, diar_lang) = {
        let state = app.state::<AppState>();
        let diar_lang = batch_transcribe_lang(&state.settings_snapshot());
        let (parts, segments) = {
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            let doc = vault.read(&note_rel).map_err(vault_err)?;
            let media_parts = sorted_media_parts(&doc.meta.media);
            let segments = collect_note_segments(vault.root(), &doc.meta);
            // (绝对路径, 会话偏移 ms, 是否双轨立体声件)——双轨件解码 R 声道（对方）细分。
            let parts: Vec<(PathBuf, u64, bool)> = media_parts
                .iter()
                .map(|m| {
                    (
                        vault.root().join(&m.path),
                        m.part_start_ms.unwrap_or(0),
                        m.tracks.as_deref() == Some("mic_system"),
                    )
                })
                .collect();
            (parts, segments)
        };
        (
            parts,
            segments,
            state.queue_handle(),
            state.vault_arc(),
            diar_lang,
        )
    };
    if parts.is_empty() {
        return Err(coded_error(
            app,
            "DIARIZE_NOT_APPLICABLE",
            "diarize-not-applicable-audio",
        ));
    }
    if segments.is_empty() {
        return Err(coded_error(
            app,
            "DIARIZE_NO_TRANSCRIPT",
            "diarize-no-transcript",
        ));
    }
    let total = parts.len() as u32;
    let mut diar_all: Vec<luna_core::Segment> = Vec::new();
    for (done, (audio_abs, part_start_ms, dual)) in (1u32..).zip(parts) {
        progress(ProcPhase::Diarizing, done, total);
        // 解码在阻塞线程池（symphonia CPU 密集，ADR-010 纪律）；双轨件取 R 声道（对方）细分。
        let decode_flag = cancel.flag();
        let decoded = tokio::task::spawn_blocking(move || {
            if dual {
                luna_audio::decode_media_to_stereo_tracks(&audio_abs, &decode_flag)
                    .map(|(_, right)| right)
            } else {
                luna_audio::decode_media_to_pcm(&audio_abs, &decode_flag)
            }
        })
        .await
        .map_err(|e| format!("DECODE_TASK_PANIC: {e}"))
        .and_then(|r| r.map_err(|e| format!("DECODE_FAILED: {e}")))?;
        let mut diar =
            crate::pipeline::diarize_pcm(&queue, decoded, diar_lang.clone(), cancel).await?;
        let offset = std::time::Duration::from_millis(part_start_ms);
        for d in &mut diar {
            d.start += offset;
            d.end += offset;
        }
        diar_all.extend(diar);
    }
    let (_speakers, applied) = crate::pipeline::apply_manual_diarization(&mut segments, &diar_all);
    if applied {
        // 权威写回（正文哨兵块 + 边车 + 标记一致；无转译、模型标记不动）。
        crate::pipeline::write_recording_segments(
            &vault_arc,
            &note_rel,
            segments,
            None,
            luna_vault::TranscriptAuthority::Unchanged,
        )
        .await?;
        emit_vault_changed(app, &note_rel, false);
    }
    Ok(Some(note_rel))
}

/// 导入落地计划：建好转写笔记、定好媒体落点，尚未写媒体字节（PRD-04-058 / PRD-05-004①）。
///
/// 拆成「计划（持锁，纯元数据）→ 写媒体（阻塞线程池，可能是 GB 级拷贝）→ 登记（持锁）」
/// 三步，是为了**大 IO 绝不在 vault 锁内、也不在 async worker 线程上**。
#[derive(Clone, Debug)]
pub(crate) struct ImportPlan {
    /// 新建的转写笔记相对路径。
    pub note_rel: String,
    /// 媒体在 Vault 内的相对落点。
    pub media_rel: String,
    /// 媒体绝对落点（父目录已建好）。
    pub media_abs: PathBuf,
    /// 落地方式：视频拷原容器 / 音频编 WAV。
    pub kind: PlaybackMediaKind,
    /// 音轨时长（ms，由解码后的 PCM 推得——视频容器时长以其音轨为准，与 cue 时间轴同源）。
    pub duration_ms: u64,
}

/// 导入媒体的文件名（纯逻辑，可测）：视频保留**原扩展名**（画面必须原容器承载），
/// 音频统一 `.wav`（16k mono 转码产物，同时满足虚拟拼接的 PCM 直拼前提）。
pub(crate) fn import_media_file_name(
    note_stem: &str,
    src: &Path,
    kind: PlaybackMediaKind,
) -> String {
    match kind {
        PlaybackMediaKind::Audio => format!("{note_stem}.wav"),
        PlaybackMediaKind::Video => {
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .filter(|e| VIDEO_MEDIA_EXTS.contains(&e.as_str()))
                .unwrap_or_else(|| "mp4".to_owned());
            format!("{note_stem}.{ext}")
        }
    }
}

/// 建**转写笔记** + 定媒体落点（导入公共同步步骤；不写媒体字节）。
///
/// 落 `vault.transcripts_dir`（`NoteType::Transcript`）而非录音目录——导入的媒体与用户
/// 亲自采集的录音是两类产物，混放会让「录音」列表分不清来源（PRD-05-030 / PRD-07-012）。
pub(crate) fn plan_import_note(
    state: &AppState,
    transcripts_dir: &str,
    src: &Path,
    stem: &str,
    pcm: &luna_core::PcmBuffer,
) -> Result<ImportPlan, String> {
    let duration_ms =
        (pcm.samples.len() as u64) * 1000 / u64::from(luna_audio::TARGET_SAMPLE_RATE).max(1);
    // stem 由调用方按来源决定（本地文件名 / 视频标题），此处只兜住"净化后仍为空"。
    let stem = safe_media_stem(stem);
    let kind = media_kind(&src.to_string_lossy()).unwrap_or(PlaybackMediaKind::Audio);
    // 附件目录策略在锁 vault 前快照设置（避免 vault↔settings 锁嵌套）。
    let settings = state.settings_snapshot();

    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let note_rel_path = format!("{transcripts_dir}/{stem}.md");
    let doc = create_unique_note(
        state.copy(),
        vault,
        &note_rel_path,
        NoteType::Transcript,
        format!("# {stem}\n\n"),
    )?;
    // 媒体与笔记同 stem（笔记已去重 → 媒体名亦唯一），落到笔记附件目录（与录音会话
    // 同源附件策略，PRD-06-012；不再散落在归档根目录）。移动笔记时随附件跟随迁移。
    let note_stem = Path::new(&doc.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&stem)
        .to_string();
    let asset_dir = media_asset_dir(&settings, &doc.path);
    let media_rel =
        luna_vault::join_vault_rel(&asset_dir, &import_media_file_name(&note_stem, src, kind));
    let media_abs = vault.root().join(&media_rel);
    if let Some(parent) = media_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("MKDIR_FAILED: {e}"))?;
    }
    Ok(ImportPlan {
        note_rel: doc.path,
        media_rel,
        media_abs,
        kind,
        duration_ms,
    })
}

/// 把媒体字节落到计划落点（**阻塞**：视频整文件拷贝 / 音频 WAV 编码，均须在阻塞线程池调用）。
pub(crate) fn write_import_media(
    plan: &ImportPlan,
    src: &Path,
    pcm: &luna_core::PcmBuffer,
) -> Result<(), String> {
    use luna_audio::Encoder as _;
    match plan.kind {
        // 视频：拷原容器（画面 + 音轨同源单文件）。绝不转码——转码既慢又有损，且会让
        // 「用户看到的画面」与「他导入的文件」不是同一份东西。
        PlaybackMediaKind::Video => {
            std::fs::copy(src, &plan.media_abs).map_err(|e| format!("MEDIA_COPY_FAILED: {e}"))?;
            Ok(())
        }
        PlaybackMediaKind::Audio => {
            let mut enc =
                luna_audio::WavEncoder::create(&plan.media_abs, luna_audio::TARGET_SAMPLE_RATE)
                    .map_err(|e| format!("WAV_CREATE_FAILED: {e}"))?;
            enc.write_samples(&pcm.samples)
                .map_err(|e| format!("WAV_WRITE_FAILED: {e}"))?;
            Box::new(enc)
                .finalize()
                .map_err(|e| format!("WAV_FINALIZE_FAILED: {e}"))?;
            Ok(())
        }
    }
}

/// 把已落库的媒体登记为笔记的唯一分片（空转写；随后台转写翻正）。
pub(crate) fn attach_import_media(
    state: &AppState,
    plan: &ImportPlan,
    model_name: Option<String>,
) -> Result<(), String> {
    let part = luna_vault::RecordingPart {
        path: plan.media_rel.clone(),
        start_ms: 0,
        end_ms: plan.duration_ms,
        duration: Some(plan.duration_ms as f64 / 1000.0),
        model: model_name,
        lang: None,
        tracks: None,
    };
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .finalize_recording(
            &plan.note_rel,
            &[part],
            &[],
            &[],
            luna_vault::TranscriptAuthority::Unchanged,
        )
        .map_err(vault_err)
}

/// 权重路径 → 模型展示名。
///
/// **不能只取文件名**：LunaManaged 布局是 `<root>/<模型 id>/weights.bin`（文件名恒定，
/// 身份在**目录名**上），于是取 stem 的结果是所有本机模型都叫 `weights`。它出现在算力
/// 卡片的兜底说明（「服务端连不上时改用本机的 weights 继续转写」）、录音归档的
/// `MediaRef.model`，以及批式转写的模型标注上——最后一处最贵：那是将来回答「这段转写
/// 当时用了哪个权重」的唯一依据，写成 `weights` 等于没写。真机坐实（2026-08-08）。
///
/// 判据顺序：先按目录名查目录（拿到「Whisper Small (q5_1)」这样的人话名），查不到就退回
/// 目录名本身（`whisper-small-q5_1-ggml` 至少还是个身份），两者都不成立时才回到文件 stem
/// ——用户自带的 `.bin` 正是那一档，stem 对它就是正确答案。
pub(crate) fn model_display_name(path: &Path) -> String {
    if path
        .file_name()
        .is_some_and(|name| name == crate::model_store::LUNA_MANAGED_WEIGHT_FILE)
    {
        if let Some(id) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
        {
            return crate::model_store::remote_model_spec(id)
                .map_or_else(|| id.to_owned(), |spec| spec.name.to_owned());
        }
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// 记一次「导入资料」的终态（工作区观测，PRD-07-070 / TDD-12 §2.1）。
///
/// **只有终态与耗时桶，没有路径、没有 URL、没有标题**：那三样任意一个都足以指认
/// 用户导入了什么。
///
/// **凭 `grant` 提交，而不是在这里重新问一次"现在还开着吗"**：那张凭据是任务开始时
/// 取的，它一并回答了"这段时间里授权有没有被收回、历史有没有被清过"（TDD-12 §2.1）。
fn record_material_import(
    app: &tauri::AppHandle,
    grant: crate::workspace_metrics::WorkspaceMetricsGrant,
    status: &crate::processing_queue::ProcStatus,
    started_ms: i64,
) {
    use luna_metrics::workspace::{EntryResult, WorkspaceEvent};
    let Some(metrics) = app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>() else {
        return;
    };
    let now = super::metrics::metric_now_ms();
    use crate::processing_queue::ProcStatus;
    let result = match status {
        ProcStatus::Done { .. } => EntryResult::Arrived,
        ProcStatus::Cancelled => EntryResult::Cancelled,
        ProcStatus::Failed { .. } => EntryResult::Failed,
        // 其余（排队中 / 执行中）不是终态——不该走到这里，走到了也不记：进行中的尝试
        // 进库会让分母随时间漂移，同一天的比率每看一次都不一样。
        _ => return,
    };
    let duration_ms = u32::try_from(now.saturating_sub(started_ms)).ok();
    metrics.commit(
        grant,
        WorkspaceEvent::MaterialImportFinished {
            result,
            duration_ms,
        },
        now,
    );
}

#[cfg(test)]
mod tests {
    /// **打开库的两条路径都要回填历史**（PRD-04-062，真机取证 2026-09-07 暴露）。
    ///
    /// 启动装载不经 `bind_vault`（`build_state_with_settings_persistence` 直接按启动指针
    /// 开库），首版只把 `restore_queue_history` 挂在 `bind_vault` 上——于是历史只对
    /// 「这次启动换过库」的人出现，而需求的原句是「关掉应用第二天打开还想知道昨天哪几件
    /// 没成」，那恰恰是不换库的路径。库注册表的 `touch` 与知识体检提醒都原样踩过这个坑。
    #[test]
    fn both_paths_that_open_a_vault_restore_the_history() {
        let startup = crate::ipc::source_without_tests(include_str!("../lib.rs"));
        assert!(
            startup.contains("ipc::restore_queue_history(app.handle())"),
            "启动装载没有回填队列历史——冷启动看不到上次会话的失败项"
        );
        let switch = crate::ipc::source_without_tests(include_str!("vaults.rs"));
        assert!(
            switch.contains("processing::restore_queue_history(app)"),
            "切库没有回填队列历史——历史必须随库换"
        );
    }

    /// **稳定内层码必须进日志，载荷必须不进**（真机排障 2026-08-31）。
    ///
    /// 那次失败在日志里只剩 `DISTILL_JOB`——它对每一次蒸馏失败都一样，于是「是哪一类
    /// 失败」只能靠翻用量库反推。而同一条规则要挡住的另一半是真的：`FETCH_IO` 后面跟的
    /// 是绝对路径，日志脱敏明禁（TDD-12）。
    #[test]
    fn the_log_keeps_a_stable_inner_code_and_drops_everything_else() {
        use super::redacted_failure_code as r;
        assert_eq!(
            r("DISTILL_JOB: INVALID_INPUT"),
            "DISTILL_JOB: INVALID_INPUT"
        );
        assert_eq!(
            r("DISTILL_JOB: EMPTY_AFTER_THINKING"),
            "DISTILL_JOB: EMPTY_AFTER_THINKING"
        );
        assert_eq!(r("INVALID_INPUT"), "INVALID_INPUT");
        // 载荷一律切掉——路径、人话、以及任何不是稳定码的东西。
        assert_eq!(r("FETCH_IO: /Users/someone/Notes/私密.md"), "FETCH_IO");
        assert_eq!(
            r("MERGE_SELECTION_INVALID: 选择里有一条读不出来"),
            "MERGE_SELECTION_INVALID"
        );
        assert_eq!(r("FETCH_TOOL_FAILED: exit 1"), "FETCH_TOOL_FAILED");
        // 小写不是稳定码——服务端自述码（`upstream_unavailable`）不走这条通路，
        // 真出现在这里就是拼错了，不该被当成码放进日志。
        assert_eq!(r("DISTILL_JOB: upstream_unavailable"), "DISTILL_JOB");
    }

    use super::*;

    /// LunaManaged 布局下**所有**模型的权重文件都叫 `weights.bin`，身份在目录名上。
    ///
    /// 取文件 stem 的结果是它们全叫 `weights`——出现在算力卡片的兜底说明、录音归档的
    /// `MediaRef.model`，以及批式转写的模型标注上。最后一处最贵：那是将来回答「这段转写
    /// 当时用了哪个权重」的唯一依据。真机坐实（2026-08-08：卡片写「改用本机的 weights
    /// 继续转写」）。
    #[test]
    fn managed_weights_are_named_by_their_model_not_the_file() {
        let root = Path::new("/models");
        let id = crate::model_store::WHISPER_SMALL_Q5_REMOTE_ID;
        let spec = crate::model_store::remote_model_spec(id).expect("目录里应有这一档");
        assert_eq!(
            model_display_name(&crate::model_store::luna_managed_weight_path(root, id)),
            spec.name,
            "LunaManaged 权重被叫成文件名了"
        );

        // 目录里没登记的 id（用户手放进来的托管目录）：退回**目录名**而不是 `weights`。
        assert_eq!(
            model_display_name(&crate::model_store::luna_managed_weight_path(
                root,
                "custom-ggml"
            )),
            "custom-ggml"
        );

        // 用户自带的散装权重：文件 stem 正是它的身份，保持原行为。
        assert_eq!(
            model_display_name(Path::new("/somewhere/ggml-large-v3-turbo.bin")),
            "ggml-large-v3-turbo"
        );
    }

    /// 蒸馏的硬事实码不可重试：EDITED 的唯一合法出路是差异预览、INELIGIBLE 的类型
    /// 不会因重跑而改变——给「重试」按钮等于诱导空等（PRD-05-006 同一裁决）。
    /// 环境性的蒸馏码保持可重试。
    #[test]
    fn distill_hard_fact_codes_are_not_retryable() {
        assert!(!failure_is_retryable(
            "DISTILL_EDITED: 知识笔记已被编辑，不会自动覆盖。"
        ));
        assert!(!failure_is_retryable(
            "DISTILL_INELIGIBLE: 只有录音 / 转写 / 对话可生成知识笔记。"
        ));
        // 环境性失败：转写完成后就有内容 / 回到那个库重试正当 / 引擎恢复后可成。
        assert!(failure_is_retryable("DISTILL_EMPTY_SOURCE: …"));
        assert!(failure_is_retryable("DISTILL_VAULT_SWITCHED: …"));
        assert!(failure_is_retryable("DISTILL_JOB: PROVIDER_DOWN"));
    }

    /// 文档导入：文件本身的事实不可重试，环境性的读取 / 转换失败可重试。
    #[test]
    fn document_import_hard_facts_are_not_retryable() {
        for code in [
            "DOC_NO_TEXT_LAYER: 这份 PDF 几乎没有可提取的文字",
            "DOC_ENCRYPTED: …",
            "DOC_EMPTY: …",
            "DOC_TOO_LARGE: …",
            "DOC_CORRUPT: …",
            "DOC_UNSUPPORTED: …",
            "DOC_NEEDS_MACOS: …",
        ] {
            assert!(!failure_is_retryable(code), "{code}");
        }
        assert!(failure_is_retryable("DOC_READ_TIMEOUT: …"));
        assert!(failure_is_retryable("DOC_READ_FAILED: …"));
        assert!(failure_is_retryable("DOC_CONVERT_TIMEOUT: …"));
        // 网页剪藏：页面本身的事实不可重试；网络 / 开关 / 严格本地是环境，可重试。
        assert!(!failure_is_retryable("WEB_CLIP_NOT_HTML: …"));
        assert!(!failure_is_retryable("WEB_CLIP_NO_ARTICLE: …"));
        assert!(failure_is_retryable("WEB_CLIP_TIMEOUT: …"));
        assert!(failure_is_retryable("WEB_CLIP_DISABLED: …"));
    }

    /// 导入媒体命名：视频保留原扩展名（画面必须原容器承载），音频统一转 WAV。
    #[test]
    fn import_media_file_name_keeps_video_container_and_transcodes_audio() {
        assert_eq!(
            import_media_file_name(
                "会议记录",
                Path::new("/src/talk.MP4"),
                PlaybackMediaKind::Video
            ),
            "会议记录.mp4"
        );
        assert_eq!(
            import_media_file_name("talk", Path::new("/src/talk.mov"), PlaybackMediaKind::Video),
            "talk.mov"
        );
        assert_eq!(
            import_media_file_name(
                "podcast",
                Path::new("/src/podcast.m4a"),
                PlaybackMediaKind::Audio
            ),
            "podcast.wav"
        );
        // 视频类但扩展名异常（无扩展/非白名单）→ 回落 mp4，绝不产出无扩展媒体。
        assert_eq!(
            import_media_file_name("x", Path::new("/src/x"), PlaybackMediaKind::Video),
            "x.mp4"
        );
    }

    /// 媒体落库：视频**逐字节拷原容器**（画面不能丢），音频写出可播放的 RIFF/WAVE。
    #[test]
    fn write_import_media_copies_video_and_encodes_audio() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("talk.mp4");
        let payload = b"not-a-real-mp4-but-bytes-must-survive-verbatim";
        std::fs::write(&src, payload).expect("write src");

        let video_plan = ImportPlan {
            note_rel: "录音/talk.md".into(),
            media_rel: "录音/assets/talk.mp4".into(),
            media_abs: dir.path().join("out.mp4"),
            kind: PlaybackMediaKind::Video,
            duration_ms: 1_000,
        };
        let pcm = luna_core::PcmBuffer {
            sample_rate: luna_audio::TARGET_SAMPLE_RATE,
            channels: 1,
            samples: vec![0.0_f32; 16_000],
        };
        write_import_media(&video_plan, &src, &pcm).expect("copy video");
        assert_eq!(
            std::fs::read(&video_plan.media_abs).expect("read copy"),
            payload,
            "视频必须原样拷贝（转码/只留音轨都会丢画面）"
        );

        let audio_plan = ImportPlan {
            media_abs: dir.path().join("out.wav"),
            kind: PlaybackMediaKind::Audio,
            ..video_plan
        };
        write_import_media(&audio_plan, &src, &pcm).expect("encode wav");
        let wav = std::fs::read(&audio_plan.media_abs).expect("read wav");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }
}
