//! 知识体检（PRD-16 §3.7
//! `PRD-16-030/031/032`）：T0 即时体检 + T1 深度体检 + 忽略与周期提醒。
//!
//! 分工与既有域一致：**判据全在 `luna_vault::checkup`（纯函数，可无头单测）**，本模块只做
//! 编排——查库、走 AI 主干判一条声明、落派生层、发进度与通知。
//!
//! # 三条本模块特有的纪律
//!
//! 1. **只出建议、绝不自动修**（`PRD-16-032`）：本模块没有任何改笔记的路径。矛盾判定
//!    来自本地小模型，它的误判率远高于"用户自己看一眼"的成本；一个会自动改笔记的体检器，
//!    错一次就够让人再也不敢开它。卡片上的「归入主题页…」走的也是既有确认卡（差异预览 +
//!    版本快照），不是体检自己的写入路径。
//! 2. **周期提醒只提醒、绝不静默自动跑**（`PRD-16-032`）：本地引擎是共享资源，抢占它
//!    去做用户没要求的事是越界。故提醒默认关，且它只发一条通知。
//! 3. **深度体检是十分钟级的**（每条声明一次本地 LLM），故照搬 `graph.rs` / `embed.rs`
//!    的六要素骨架（预检盘点 → 确认卡给数字 → 单例后台逐条 → 进度事件 + 快照 →
//!    连败中止 → 终局通知），而不是一个转圈等的模态。
//!
//! 脱敏（TDD-12）：日志只出路径与计数——声明原文与源笔记摘录**与笔记正文同级**。

use super::graph::strip_think_blocks;
use super::*;
use luna_core::{
    Job, JobEvent, JobKind, JobOrigin, Priority, ResultPayload, SummaryStyle, TextRef,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// 喂给模型的源笔记上限（字符）。一条声明的核对只需要源的主体部分，尾部截断可接受
/// （与归入的材料上限同口径）。
const SOURCE_MAX_CHARS: usize = 6_000;
/// 判定的输出预算。
///
/// **它闸的是 `tokens_out`，不是"一个词 + 一句话理由该有多长"**（ADR-089 ④ / 与蒸馏
/// 同一个坑）：推理模型的思维链**计入** `tokens_out`（真机 7332 里 3540 帧是思考），正文
/// 还没开始写预算就见底了。而新语义下 caller 上限到顶 = **正常完成**——于是 192 这个
/// 数字的表现不是"答得短一点"，是**安静地判不出来**。答案的形状仍靠 prompt 与
/// [`parse_verdict`] 收，不靠一个会把思考也一起掐掉的预算。
const VERDICT_OUTPUT_TOKENS: u32 = 2_048;
// 下限在**编译期**钉住（同 `MERGE_NOTE_MIN` 的先例）：把它调回三位数，遇上推理模型就又是
// 一次「安静地判不出来」——而那种坏法在运行期没有任何用例会红，它表现为体检报告变短。
const _: () = assert!(VERDICT_OUTPUT_TOKENS >= 1_024);
/// 每篇主题页最多核对几条声明。一页三十条全送去判，等于把一次体检变成一小时的引擎占用，
/// 而前几条已足以回答「这一页还站得住吗」（与「立即归入」最多 3 篇同一条判据）。
const MAX_CLAIMS_PER_PAGE: usize = 8;
/// 连续失败即中止（与全库重扫 / 语义建库同一阈值与理由）。
const HALT_AFTER_CONSECUTIVE_FAILURES: u32 = 3;
/// 报告里每类最多摆几张卡。超出的**如实报数**（`truncated`），绝不静默截断——
/// 一个只显示 20 条却说得像全部的报告，会让用户以为库里只有这些问题。
const REPORT_CARDS_PER_KIND: usize = 20;
/// 近重复检测一次最多取几对（阈值 0.93 之上的对数在正常库里是个位数；上限只是防御）。
const NEAR_DUPLICATE_LIMIT: usize = 50;
/// 周期提醒的间隔（天）。默认关；开着时也只在**打开库**这个自然时机看一眼。
const REMINDER_INTERVAL_DAYS: i64 = 14;

// ── DTO ─────────────────────────────────────────────────────────────────────

/// 一张建议卡（`PRD-16-032`）。仅下行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct CheckupFindingDto {
    /// 稳定 id（忽略按它对齐）。
    pub id: String,
    /// 种类 kebab 键——**前端按它查 i18n 取文案**，Rust 不出人读文案。
    pub kind: String,
    /// 涉及页（芯片）。第一项 = 「打开」的落点。
    pub paths: Vec<String>,
    /// 「打开」要落到的行（1-based）；`None` = 只打开笔记。
    pub line: Option<u32>,
    /// 被指认的原文（那条声明 / 断链的链接目标）。
    pub subject: String,
    /// 判定理由（模型产出，仅深度体检有）。
    pub reason: String,
    /// 覆盖的对象数（积压 N 篇 / 断链被 N 页引用）。`0` = 不适用。
    pub count: u32,
    /// 这条能不能被 ✕（积压卡不能——它会自己消失，判定在 Rust）。
    pub ignorable: bool,
    /// 这条是不是**流水线积压**：知识库脉搏已把它放在主位，故启动概览的体检一行
    /// **不数它**（否则同一件事在同一屏说两遍）。判定在 Rust，前端只按结果过滤。
    pub backlog: bool,
    /// 「修复经预览」走哪条既有流程；`None` = 这一类没有确定性修复，只给「打开」。
    pub fix: Option<String>,
}

/// 一次体检报告（T0 + 最近一次深度体检的留存）。仅下行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct CheckupReportDto {
    pub findings: Vec<CheckupFindingDto>,
    /// 因每类上限被折起来的条数（如实报数，不静默截断）。
    pub truncated: u32,
    /// 上一次深度体检的完成时刻（毫秒）；`None` = 从未跑过。
    /// `f64`：`i64` 跨 IPC 会被 specta 判为 BigInt 风格类型而拒绝（与 `NoteRefDto`
    /// 的 `modified_ms` 同一处置；毫秒时间戳在 f64 的 53 位尾数内无精度问题）。
    pub deep_ran_at_ms: Option<f64>,
    /// 是否有一次深度体检正在跑。
    pub deep_running: bool,
}

/// 深度体检预检：确认卡据此在**点开始之前**陈述代价（`PRD-16-031` 六要素之二）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct CheckupPreflightDto {
    /// 待核对的声明条数（= 本次要跑多少次本地模型）。
    pub claims: u32,
    /// 覆盖的主题页数。
    pub topics: u32,
    /// 近重复检测是否可用（语义索引在场）。`false` = 这一项**诚实跳过**，不静默给空结果。
    pub semantic_ready: bool,
    /// 是否已有一次在跑。
    pub running: bool,
}

/// 深度体检进度（`checkup://progress`）。逐条低频（每条一次 LLM），无需节流。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "checkup://progress")]
pub struct CheckupProgress {
    /// 已核对的声明数（含失败）。
    pub done: u32,
    /// 本次总条数。
    pub total: u32,
    /// 已发现的问题数。
    pub found: u32,
    /// 判定失败的条数。
    pub failed: u32,
    /// 终态标记。
    pub finished: bool,
    /// 用户主动取消。
    pub cancelled: bool,
    /// 连续失败提前中止（引擎不可用的典型指纹）。
    pub halted: bool,
}

/// 深度体检运行态（`AppState` 持有）：取消令牌在场 = 有一次在跑（单例）。
#[derive(Default)]
pub(crate) struct CheckupRun {
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    progress: Option<CheckupProgress>,
}

// ── 投影 ────────────────────────────────────────────────────────────────────

/// 「修复经预览」的映射（判定在 Rust，ADR-003）。
///
/// 只有「还没有标签」有**确定性且可预览**的修复（全库补齐的确认卡）。孤儿页此前给
/// 「归入主题页…」，随旧版片段归入退役（ADR-136）一并收回：给它加标签或链接是用户的判断，
/// 加了标签它自然成为主题候选。
///
/// 其余各类**刻意不给修复按钮**：断链要么建那篇笔记要么改链接、空页要么写要么删、
/// 矛盾/过时要改的是一句话、近重复要合并两页——**每一种都是用户的判断**，机器给一个
/// 「修复」按钮只会诱导他不看就点。给「打开」把他带到现场，才是这类工具的诚实形态。
fn fix_for(kind: luna_vault::FindingKind) -> Option<&'static str> {
    match kind {
        // 「还没有标签」的确定性处置就是**跑一次全库补齐**（PRD-06-035b 的既有入口，
        // 而那张确认卡在本轮之后还会如实列出被停用的文件夹并可就地取消）——两条修复
        // 在这里接上：体检说出"为什么建议不出现"，卡片把用户带到能解除它的那一处。
        luna_vault::FindingKind::Untagged => Some("rescan"),
        _ => None,
    }
}

impl From<luna_vault::Finding> for CheckupFindingDto {
    fn from(f: luna_vault::Finding) -> Self {
        Self {
            id: f.id,
            kind: f.kind.key().to_owned(),
            paths: f.paths,
            line: f.line,
            subject: f.subject,
            reason: f.reason,
            count: f.count,
            ignorable: f.kind.ignorable(),
            backlog: f.kind.is_backlog(),
            fix: fix_for(f.kind).map(str::to_owned),
        }
    }
}

fn clamp_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 现在（毫秒）。体检的时间戳用于「上次跑于何时」、周期提醒，以及待沉淀积压的
/// 陈化判据（PRD-06-055——判定内核是纯函数、不读时钟，故由这里喂进去）。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

// ── 命令：报告与忽略 ────────────────────────────────────────────────────────

/// 体检报告（`PRD-16-030` T0 + 最近一次深度体检的留存）。
///
/// **纯 Rust 零 LLM**：T0 是索引上的几条 SQL，深度部分是读回落盘的判定并现场校验它是否
/// 仍成立。故这条命令可以随手调（视图挂载、忽略之后重取），不心疼。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_report(
    state: tauri::State<'_, AppState>,
) -> Result<CheckupReportDto, String> {
    // AI 标签总开关（`vault.graph_ai_tags`）：关着时不报「还没有标签」——那是用户明确
    // 不要这个能力（判定在 Rust；Vault 层读不到设置，故在这里读了传进去）。
    let ai_tags_enabled = state
        .settings_snapshot()
        .get_bool("vault.graph_ai_tags")
        .unwrap_or(true);
    let (findings, ran_at) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let mut all = vault.checkup_deep_findings().map_err(vault_err)?;
        all.extend(
            vault
                .checkup_t0(ai_tags_enabled, now_ms())
                .map_err(vault_err)?,
        );
        (all, vault.checkup_last_run().map_err(vault_err)?)
    };
    // 模型判过的那一对页，顶掉同一对上的失效引用粗判卡（同一件事的两个精度）。
    let mut findings = luna_vault::suppress_superseded(findings);
    luna_vault::sort_findings(&mut findings);

    // 每类封顶 + 如实报出被折起来的条数。
    let mut kept: Vec<luna_vault::Finding> = Vec::new();
    let mut per_kind: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    let mut truncated = 0u32;
    for finding in findings {
        let slot = per_kind.entry(finding.kind.key()).or_default();
        if *slot >= REPORT_CARDS_PER_KIND {
            truncated += 1;
            continue;
        }
        *slot += 1;
        kept.push(finding);
    }

    let deep_running = state
        .checkup_run
        .lock()
        .map_err(|_| "体检状态锁中毒".to_string())?
        .cancel
        .is_some();
    Ok(CheckupReportDto {
        findings: kept.into_iter().map(CheckupFindingDto::from).collect(),
        truncated,
        deep_ran_at_ms: ran_at.map(|ms| ms as f64),
        deep_running,
    })
}

/// ✕：对这条发现永久安静（`PRD-16-032` 忽略项持久化）。
///
/// 积压卡在 Rust 侧就被拒绝——前端不显示 ✕ 只是不显示，而命令永远可达（ADR-003 纪律）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_ignore(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let kind = id
        .split_once(':')
        .map(|(head, _)| head)
        .unwrap_or(id.as_str());
    if luna_vault::FindingKind::from_key(kind).is_some_and(|k| !k.ignorable()) {
        return Err(coded_error(
            &app,
            "CHECKUP_NOT_IGNORABLE",
            "checkup-not-ignorable",
        ));
    }
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.ignore_checkup_finding(&id).map_err(vault_err)
}

// ── 命令：深度体检（六要素骨架）──────────────────────────────────────────────

/// 深度体检预检（确认卡的唯一取数口）。只读、不跑模型。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_preflight(
    state: tauri::State<'_, AppState>,
) -> Result<CheckupPreflightDto, String> {
    // 深度体检会同步消费既有矩阵；先在异步边界核实运行时 digest，避免 fresh launch 把
    // “尚未探测”误报成“语义未就绪”。
    let _ = super::embed::prepare_embedding_identity(&state).await;
    let claims = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.topic_claims(MAX_CLAIMS_PER_PAGE).map_err(vault_err)?
    };
    let topics: std::collections::BTreeSet<&str> =
        claims.iter().map(|(topic, _)| topic.as_str()).collect();
    let running = state
        .checkup_run
        .lock()
        .map_err(|_| "体检状态锁中毒".to_string())?
        .cancel
        .is_some();
    Ok(CheckupPreflightDto {
        claims: clamp_u32(claims.len()),
        topics: clamp_u32(topics.len()),
        semantic_ready: super::embed::semantic_ready(&state),
        running,
    })
}

/// 启动深度体检。返回本次要核对的声明条数；单例——已有一次在跑即拒绝第二次。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_deep_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let _ = super::embed::prepare_embedding_identity(&state).await;
    let claims = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.topic_claims(MAX_CLAIMS_PER_PAGE).map_err(vault_err)?
    };
    // 一条声明都没有 ≠ 无事可做：近重复检测照样值得跑（它不依赖声明）。
    if claims.is_empty() && !super::embed::semantic_ready(&state) {
        return Err(coded_error(
            &app,
            "CHECKUP_NOTHING_TO_DO",
            "checkup-nothing-to-do",
        ));
    }
    let total = clamp_u32(claims.len());
    let cancel = Arc::new(AtomicBool::new(false));
    let initial = CheckupProgress {
        done: 0,
        total,
        found: 0,
        failed: 0,
        finished: false,
        cancelled: false,
        halted: false,
    };
    {
        let mut guard = state
            .checkup_run
            .lock()
            .map_err(|_| "体检状态锁中毒".to_string())?;
        if guard.cancel.is_some() {
            return Err(coded_error(&app, "CHECKUP_BUSY", "checkup-busy"));
        }
        guard.cancel = Some(Arc::clone(&cancel));
        guard.progress = Some(initial.clone());
    }
    let _ = initial.emit(&app);
    let generation = state.vault_generation();
    tauri::async_runtime::spawn(async move {
        run_deep(app, claims, generation, cancel).await;
    });
    Ok(total)
}

/// 请求取消（当前那一条判完即停；幂等）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_deep_cancel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state
        .checkup_run
        .lock()
        .map_err(|_| "体检状态锁中毒".to_string())?;
    if let Some(cancel) = guard.cancel.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// 进度快照（面板重挂载时恢复进度条）；`None` = 无体检在跑。
#[tauri::command]
#[specta::specta]
pub(crate) async fn checkup_deep_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<CheckupProgress>, String> {
    Ok(state
        .checkup_run
        .lock()
        .map_err(|_| "体检状态锁中毒".to_string())?
        .progress
        .clone())
}

// ── 判定：一条声明 vs 它引用的源 ────────────────────────────────────────────

/// 模型对一条声明的判定。四态**互斥且穷尽**，其中两态才成为发现。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// 一致——不报。
    Consistent,
    /// 源与声明冲突。
    Contradicted,
    /// 源已更新，声明只是过时。
    Outdated,
    /// 源里找不到相关内容 → **不报**。体检的误报比漏报贵得多：一条"其实没问题"的
    /// 警告会让用户下次不再相信整张报告。
    Unclear,
}

impl Verdict {
    fn finding_kind(self) -> Option<luna_vault::FindingKind> {
        match self {
            Self::Contradicted => Some(luna_vault::FindingKind::Contradiction),
            Self::Outdated => Some(luna_vault::FindingKind::Outdated),
            Self::Consistent | Self::Unclear => None,
        }
    }
}

/// 解析模型输出：**第一行取那个大写关键词，第二行起是理由**。
///
/// 契约刻意不是 JSON：本地小模型对"只输出一个词"的遵循度远高于"输出合法 JSON"
/// （S17 spike 的同一条结论——让模型做小判断，不做结构化编辑）。关键词用 ASCII 大写词，
/// 于是中英两种指令下的解析口径完全一致。认不出关键词 = 判定失败，**绝不猜**。
fn parse_verdict(raw: &str) -> Option<(Verdict, String)> {
    /// 四个关键词与它们的取值（顺序不影响判定——命中必须唯一，见下）。
    const KEYWORDS: [(&str, Verdict); 4] = [
        ("CONTRADICTED", Verdict::Contradicted),
        ("OUTDATED", Verdict::Outdated),
        ("CONSISTENT", Verdict::Consistent),
        ("UNCLEAR", Verdict::Unclear),
    ];
    let cleaned = strip_think_blocks(raw);
    let mut verdict = None;
    let mut reason = String::new();
    for line in cleaned.lines() {
        if verdict.is_none() {
            let upper = line.to_uppercase();
            let hits: Vec<Verdict> = KEYWORDS
                .iter()
                .filter(|(word, _)| upper.contains(word))
                .map(|(_, v)| *v)
                .collect();
            // **必须唯一命中**：小模型时常把指令里的四个选项原样复述一遍
            //（"CONSISTENT / CONTRADICTED / OUTDATED / UNCLEAR"），而按优先级取第一个
            // 会把这种复述判成「矛盾」——恰恰是最贵的那种误报（用户被指着去改一条
            // 本来没问题的声明）。一行里出现多个关键词 = 它不是判定行，跳过。
            if hits.len() == 1 {
                verdict = hits.first().copied();
            }
            continue;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            reason = trimmed.to_owned();
            break;
        }
    }
    verdict.map(|v| (v, reason))
}

/// 合成判定指令（调用方决策、引擎只渲染——同 `graph::extract_tags_instruction`）。
fn verdict_instruction(zh: bool) -> luna_core::Instruction {
    luna_core::prompt::registry().instruction_for_locale(
        luna_core::PromptScope::Checkup,
        "verdict",
        if zh { Some("zh-CN") } else { None },
        &[],
    )
}

/// 注册表 `checkup` 域的一条模板（ADR-050 修订 2026-09-05）；`zh` 显式选 zh-CN 变体
/// （与 `is_zh` 同源，单测可在英文进程里造中文样本）。
fn checkup_template(key: &str, zh: bool, vars: &[(&str, &str)]) -> String {
    luna_core::prompt::registry().render_for_locale(
        luna_core::PromptScope::Checkup,
        key,
        if zh { Some("zh-CN") } else { None },
        vars,
    )
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max).collect()
}

/// 核对一条声明（走单一 AI 主干，ADR-001；不新增 `JobKind`）。
async fn judge_claim(
    app: &tauri::AppHandle,
    state: &AppState,
    claim: &luna_vault::Claim,
    source_title: &str,
    source_body: &str,
    zh: bool,
) -> Result<(Verdict, String), String> {
    let material = checkup_template(
        "material",
        zh,
        &[
            ("claim", &claim.text),
            ("title", source_title),
            ("body", &truncate_chars(source_body, SOURCE_MAX_CHARS)),
        ],
    );
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(material),
            // Faithful = 指令即完整 system prompt，Provider 不加「Summarize concisely」外套——
            // 否则「只输出一个词」会被外层的「做个摘要」盖过（蒸馏与归入都踩过同一个坑）。
            style: SummaryStyle::Faithful,
            instruction: Some(verdict_instruction(zh)),
        },
        JobOrigin::Checkup,
    );
    // 恒后台优先级：体检是「点了就去做别的」的长任务，不与用户正在等的对话抢队。
    job.priority = Priority::Background;
    job.routing.local_only = state.strict_local();
    job.options.output_budget = Some(VERDICT_OUTPUT_TOKENS);

    let mut handle = state.queue.submit(job);
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                let capped = result.output_truncated.is_some();
                let text = match result.payload {
                    ResultPayload::Text(text) => text,
                    ResultPayload::Chat(message) => message.content,
                    ResultPayload::Segments(_) | ResultPayload::Embeddings(_) => String::new(),
                };
                return parse_verdict(&text).ok_or_else(|| {
                    // **「因触顶而判不出来」不许静默**：新语义下 caller 上限到顶是
                    // `Finished`（ADR-089 ④），于是"预算全花在思考上、判定一个字没来"会
                    // 以一次**成功**的形态走到这里。它与"模型答得驴唇不对马嘴"是两件事，
                    // 用户能做的动作也不同——只说「无法解析」，两种都被说成了模型不听话。
                    if capped {
                        coded_error(app, "CHECKUP_OUTPUT_CAPPED", "checkup-output-capped")
                    } else {
                        coded_error(app, "CHECKUP_PARSE", "checkup-parse")
                    }
                });
            }
            JobEvent::Failed(err) => {
                // **只发码，诊断串进日志**（ADR-108）：`{err}` 是 `JobError` 的
                // `Display`，它的受众是开发者（ADR-101 ②）——拼进 IPC 返回串等于把一句
                // 不随界面语言变的诊断摆到用户面前，而界面本来就有按码取的本地化文案
                // （`common:failure.*`）。这三处是 ADR-101 ② 点名的最后障碍。
                eprintln!("[job] 失败 code={} detail={err}", err.code());
                return Err(err.code().to_owned());
            }
            JobEvent::Cancelled => {
                return Err(coded_error(app, "CHECKUP_CANCELLED", "checkup-cancelled"))
            }
            _ => {}
        }
    }
    Err(coded_error(app, "CHECKUP_NO_RESULT", "checkup-no-result"))
}

// ── 执行体 ──────────────────────────────────────────────────────────────────

/// 深度体检执行体：逐条判定 → 逐条报进度 → 近重复兜底 → 终态收尾。
async fn run_deep(
    app: tauri::AppHandle,
    claims: Vec<(String, luna_vault::Claim)>,
    generation: u64,
    cancel: Arc<AtomicBool>,
) {
    let state = app.state::<AppState>();
    let zh = is_zh(&app);
    let total = clamp_u32(claims.len());
    let mut progress = CheckupProgress {
        done: 0,
        total,
        found: 0,
        failed: 0,
        finished: false,
        cancelled: false,
        halted: false,
    };
    // 新一轮开跑即翻篇：报告的语义是「最近一次深度体检看到的」。
    if let Ok(guard) = state.vault.lock() {
        if let Some(vault) = guard.as_ref() {
            let _ = vault.clear_checkup_findings();
        }
    }

    let mut consecutive_failures: u32 = 0;
    for (topic, claim) in &claims {
        if cancel.load(Ordering::Relaxed) {
            progress.cancelled = true;
            break;
        }
        // 切库即停：判定属于上一个库，继续跑只是白烧算力（落盘点也会再拒一次）。
        if !state.still_current_vault(generation) {
            progress.cancelled = true;
            break;
        }
        let source = {
            let Ok(guard) = state.vault.lock() else {
                break;
            };
            let Some(vault) = guard.as_ref() else { break };
            vault.read(&claim.source).ok().map(|doc| {
                (
                    luna_sync_core::links::title_of(&claim.source).to_owned(),
                    luna_vault::body_of(&doc.content).to_owned(),
                )
            })
        };
        let Some((source_title, source_body)) = source else {
            // 源笔记这会儿不在了 → 它是断链/失效引用的范畴，不是矛盾；不计失败。
            progress.done += 1;
            continue;
        };

        match judge_claim(&app, &state, claim, &source_title, &source_body, zh).await {
            Ok((verdict, reason)) => {
                consecutive_failures = 0;
                if let Some(kind) = verdict.finding_kind() {
                    let finding = luna_vault::Finding {
                        id: luna_vault::Finding::make_id(
                            kind,
                            &[
                                topic,
                                &claim.source,
                                &luna_vault::Finding::claim_fingerprint(&claim.text),
                            ],
                        ),
                        kind,
                        paths: vec![topic.clone(), claim.source.clone()],
                        line: Some(claim.line),
                        subject: claim.text.clone(),
                        reason,
                        count: 0,
                    };
                    // 落盘那一刻校验代际（ADR-025）：判定是分钟级 await，期间可能切过库。
                    if state.still_current_vault(generation) {
                        if let Ok(guard) = state.vault.lock() {
                            if let Some(vault) = guard.as_ref() {
                                if vault.put_checkup_finding(&finding, now_ms()).is_ok() {
                                    progress.found += 1;
                                }
                            }
                        }
                    }
                }
            }
            // 取消不算失败（查询侧的交互任务会抢占在途批次，那是正常运行中的高频事件——
            // 与语义建库把 `EMBED_CANCELLED` 排除在连败之外同一裁决）。
            Err(code) if code.starts_with("CHECKUP_CANCELLED") => {}
            Err(code) => {
                progress.failed += 1;
                consecutive_failures += 1;
                // 脱敏（TDD-12）：只留错误码与路径，绝不含声明原文或源笔记内容。
                eprintln!("[checkup] {topic} 的一条声明判定失败：{code}");
            }
        }
        progress.done += 1;
        if consecutive_failures >= HALT_AFTER_CONSECUTIVE_FAILURES {
            progress.halted = true;
            break;
        }
        if !progress.cancelled && !progress.halted && progress.done < total {
            publish_progress(&app, progress.clone());
        }
    }

    // 近重复页：纯 Rust（语义内核的 LSH 预筛 + 精确复核），故放在最后一步顺手跑完；
    // 语义未就绪时**诚实跳过**（预检里已如实告知，不在这里假装算过）。
    // **连败中止时照样跑**：中止说的是"本地模型跑不起来"，而近重复是纯 Rust
    // （LSH 预筛 + 精确余弦），不依赖那个模型。因它而跳过，等于让没装 LLM 的用户
    // 从深度体检里一无所获——而他本可以拿到近重复那一半。用户主动取消才全停。
    if !progress.cancelled && state.still_current_vault(generation) {
        for finding in near_duplicate_findings(&app) {
            if let Ok(guard) = state.vault.lock() {
                if let Some(vault) = guard.as_ref() {
                    if vault.put_checkup_finding(&finding, now_ms()).is_ok() {
                        progress.found += 1;
                    }
                }
            }
        }
    }

    progress.finished = true;
    // 先清运行态再发终态帧（订阅方收到 finished 后再查 status 必然是"无体检在跑"）。
    {
        if let Ok(mut guard) = state.checkup_run.lock() {
            guard.cancel = None;
            guard.progress = None;
        }
        // 只有**跑完整**的一次才算"体检过"——被取消/中止的那次记上去，周期提醒会
        // 从此安静两周，而库其实没被完整看过。
        if !progress.cancelled && !progress.halted && state.still_current_vault(generation) {
            if let Ok(guard) = state.vault.lock() {
                if let Some(vault) = guard.as_ref() {
                    let _ = vault.set_checkup_last_run(now_ms());
                }
            }
        }
    }
    let _ = progress.clone().emit(&app);
    notify_outcome(&app, &progress);
}

/// 近重复页发现（语义未就绪即空——诚实降级，`PRD-16-029` 同一纪律）。
fn near_duplicate_findings(app: &tauri::AppHandle) -> Vec<luna_vault::Finding> {
    let state = app.state::<AppState>();
    let Some(pairs) = super::embed::near_duplicate_pairs(&state, NEAR_DUPLICATE_LIMIT) else {
        return Vec::new();
    };
    let Ok(guard) = state.vault.lock() else {
        return Vec::new();
    };
    let Some(vault) = guard.as_ref() else {
        return Vec::new();
    };
    vault.near_duplicate_findings(&pairs)
}

fn publish_progress(app: &tauri::AppHandle, progress: CheckupProgress) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.checkup_run.lock() {
        guard.progress = Some(progress.clone());
    }
    let _ = progress.emit(app);
}

/// 终局通知：深度体检是典型的「点了开始就切去做别的」长任务（每条声明一次本地 LLM）。
/// 用户主动取消不发（他知道自己做了什么），与全库重扫 / 语义建库同一裁决。
fn notify_outcome(app: &tauri::AppHandle, progress: &CheckupProgress) {
    if progress.cancelled {
        return;
    }
    let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    let target = crate::notifications::NotificationTarget::Route("checkup://".to_owned());
    let event = if progress.halted {
        // 与关系标签重扫同一条纪律（ADR-059 ③）：文案指向真实落点，不写死本机引擎。
        let location = super::current_chat_compute_location(app, &app.state::<AppState>());
        crate::notifications::NotificationEvent::failure("knowledge_checkup_halted", target)
            .executed_at(location)
    } else {
        crate::notifications::NotificationEvent::completion(
            "knowledge_checkup_done",
            target,
            crate::notifications::PrivatePayload::new(String::new()),
        )
    };
    nc.publish(event, app);
}

// ── 周期提醒（`PRD-16-032`：默认关，只提醒、绝不静默自动跑）────────────────────

/// 一天的毫秒数。
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 打开库之后看一眼要不要提醒做体检。
///
/// **只发通知，绝不代跑**：本地引擎是共享资源，抢占它去做用户没要求的事是越界
/// （`PRD-16-032` 的明文裁决）。默认关，故绝大多数用户永远不会遇到这条路径。
pub(crate) fn maybe_remind(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if !state
        .settings_snapshot()
        .get_bool("knowledge.checkup_reminder")
        .unwrap_or(false)
    {
        return;
    }
    let Ok(guard) = state.vault.lock() else {
        return;
    };
    let Some(vault) = guard.as_ref() else {
        return;
    };
    let Ok(mut checkup_state) = vault.checkup_state() else {
        return;
    };
    let now = now_ms();
    let interval = REMINDER_INTERVAL_DAYS * DAY_MS;
    // 从未跑过（`0`）→ 该提醒（这正是最该提醒的情形）。
    if checkup_state.last_run_ms > 0 && now - checkup_state.last_run_ms < interval {
        return;
    }
    // 提醒本身也按间隔节流：只看"上次体检"的话，到期后**每一次启动**都会响一遍——
    // 用户不跑体检就天天被念叨，那正是提醒变成骚扰的机制。
    if checkup_state.last_reminded_ms > 0 && now - checkup_state.last_reminded_ms < interval {
        return;
    }
    let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    checkup_state.last_reminded_ms = now;
    // 先记下再发：发了却没记上会在下次启动重复打扰，而记了没发只是这一轮安静
    // （下一个间隔照常提醒）——两种失败模式里选安静的那个。
    if vault.set_checkup_state(checkup_state).is_err() {
        return;
    }
    nc.publish(
        crate::notifications::NotificationEvent::completion(
            "knowledge_checkup_due",
            crate::notifications::NotificationTarget::Route("checkup://".to_owned()),
            crate::notifications::PrivatePayload::new(String::new()),
        ),
        app,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_parsing_takes_the_keyword_and_the_reason() {
        let (verdict, reason) =
            parse_verdict("CONTRADICTED\n源里写的是每秒 8 千条，声明说 1.2 万条。")
                .expect("可解析");
        assert_eq!(verdict, Verdict::Contradicted);
        assert!(reason.contains("8 千条"));
        // 小模型常见的包装：think 块、前后杂文、小写。
        let (verdict, _) =
            parse_verdict("<think>hmm</think>\nOutdated\n源已更新。").expect("可解析");
        assert_eq!(verdict, Verdict::Outdated);
        assert_eq!(
            parse_verdict("Verdict: CONSISTENT\n对得上。").map(|(v, _)| v),
            Some(Verdict::Consistent)
        );
    }

    /// 认不出关键词 = 判定失败，**绝不猜**——猜出来的"矛盾"会让用户去改一条本来没问题
    /// 的声明，而那比漏报贵得多。
    #[test]
    fn unparsable_verdict_is_a_failure_not_a_guess() {
        assert!(parse_verdict("我觉得这条大概没什么问题吧。").is_none());
        assert!(parse_verdict("").is_none());
    }

    /// 小模型把四个选项**原样复述**是常见形态；按优先级取第一个会把它判成「矛盾」
    /// ——最贵的那种误报（用户被指着去改一条本来没问题的声明）。一行多命中即跳过。
    #[test]
    fn echoing_the_option_list_is_not_a_verdict() {
        let echoed =
            "请输出 CONSISTENT / CONTRADICTED / OUTDATED / UNCLEAR 之一\nCONSISTENT\n对得上。";
        let (verdict, reason) = parse_verdict(echoed).expect("复述行之后的那一行才是判定");
        assert_eq!(verdict, Verdict::Consistent);
        assert_eq!(reason, "对得上。");
        // 整段只有复述、没有真判定 → 判定失败（绝不从复述里挑一个）。
        assert!(parse_verdict("CONSISTENT / CONTRADICTED / OUTDATED / UNCLEAR").is_none());
    }

    /// 四态里只有两态成为发现：一致与"看不出来"都不报警。
    #[test]
    fn only_contradiction_and_outdated_become_findings() {
        assert_eq!(
            Verdict::Contradicted.finding_kind(),
            Some(luna_vault::FindingKind::Contradiction)
        );
        assert_eq!(
            Verdict::Outdated.finding_kind(),
            Some(luna_vault::FindingKind::Outdated)
        );
        assert_eq!(Verdict::Consistent.finding_kind(), None);
        assert_eq!(Verdict::Unclear.finding_kind(), None);
    }

    /// 每一类发现的「修复去处」**逐条登记**（穷举断言，不是"谁都可以有"）。
    ///
    /// 写成穷举表而不是"只有孤儿有"：新增一类发现、或给某一类加修复按钮时，编译器不会
    /// 提醒你——而这条断言会。修复按钮是**唯一能写用户笔记的路**（PRD-16-032：体检只出建议），
    /// 故它必须是一处显式决定，不能是谁顺手加的。
    #[test]
    fn every_finding_kind_declares_its_fix_route() {
        use luna_vault::FindingKind as K;
        for kind in K::ALL {
            let expected = match kind {
                // 「还没有标签」的正解是跑一次全库补齐 → 那张卡还能顺手解除文件夹停用。
                K::Untagged => Some("rescan"),
                // 其余各类**刻意不给**：断链要么建那篇要么改链接、空页要么写要么删、
                // 矛盾要改的是一句话、近重复要合并两页——每一种都是用户的判断。
                //
                // **待沉淀积压尤其没有「修复」**（PRD-06-053 / PRD-06-055）：把草稿判成
                // 知识只有用户有资格，给这张卡一个一键修复按钮就是给沉淀开了自动档。
                // 它只有主按钮——把用户带到那几篇草稿跟前，沉淀与否由他点。
                // 读不了的文件夹（V1b）：处置是改权限，在访达里做——卡片主按钮把文件夹亮出来，
                // 不替用户改权限，也就没有「修复」。
                K::UnreadableFolder
                | K::Orphan
                | K::Contradiction
                | K::Outdated
                | K::StaleCitation
                | K::NearDuplicate
                | K::BrokenLink
                | K::EmptyNote
                | K::DistillBacklog
                | K::SettleBacklog => None,
            };
            assert_eq!(fix_for(kind), expected, "{kind:?} 的修复去处未按登记");
        }
    }

    /// 指令必须把「输出契约」写死在前面（材料排在后面由调用方保证，ADR-026 ④）。
    #[test]
    fn instruction_pins_the_four_keywords_in_both_languages() {
        for zh in [true, false] {
            let inst = verdict_instruction(zh);
            for keyword in ["CONSISTENT", "CONTRADICTED", "OUTDATED", "UNCLEAR"] {
                assert!(inst.contains(keyword), "{zh} 缺关键词 {keyword}");
            }
        }
    }

    /// **「因触顶而判不出来」不许静默**（R1.6）。
    ///
    /// 新语义下 caller 上限到顶 = 正常完成（ADR-089 ④），而推理模型的思维链计入
    /// `tokens_out`——192 个 token 的判定预算遇上推理模型，表现是**一次成功的空**。
    /// 两条判据：预算不再是按"答案该多长"定的那个数；空结果分得清是不是触顶造成的。
    #[test]
    fn a_verdict_lost_to_the_output_cap_is_never_reported_as_a_plain_parse_failure() {
        // ① 预算按 `tokens_out` 给（思维链也计在里面），不是"一个词 + 一句话"的长度。
        //    下限本身在编译期钉住（见常量旁的 `const _: () = assert!`），这里只钉住
        //    "它确实是按新口径重定过的那个数"。
        assert_eq!(VERDICT_OUTPUT_TOKENS, 2_048);

        // ② 空结果必须先看触顶归属再决定说哪句话。读源码是因为这条链要真 `AppHandle`
        //    与真 LLM；剥注释，否则把实现改坏、只留注释也照样全绿。
        let code: String = crate::ipc::source_without_tests(include_str!("checkup.rs"))
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let body = code
            .split("async fn judge_claim(")
            .nth(1)
            .expect("找不到 judge_claim");
        let body = body.split("\n/// ").next().unwrap_or(body);
        assert!(
            body.contains("result.output_truncated.is_some()"),
            "判定路径完全不读 output_truncated——因触顶而判不出来这件事就此静默"
        );
        assert!(
            body.contains("\"checkup-output-capped\""),
            "触顶造成的空判定没有自己的说明；且它必须走 Fluent，否则英文用户会读到中文"
        );
    }
}
