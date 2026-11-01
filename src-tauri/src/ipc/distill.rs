//! 资料蒸馏为知识笔记（PRD-06-044 / ADR-024）。
//!
//! 录音 / 媒体转写 / AI 对话是**资料**：它们服务溯源与回放，逐字稿与对话记录不是知识。
//! 沉淀进知识库的形态是**蒸馏笔记**——按内置模板整理出的一篇普通笔记（`type: note`），
//! 于是图谱上每场会议、每次对话恰有一个干净的知识节点，资料经链接边挂在它身后。
//!
//! 几条本模块特有的纪律：
//! - **走单一 AI 主干**（ADR-001）：`JobKind::Summarize`，不新增 JobKind——会议纪要
//!   （PRD-04-054）早已是同一条路，蒸馏是它的持久化升级版。
//! - **输入取边车段落而非正文**（录音/转写）：正文里的 `[HH:MM:SS.mmm]` 与 🔖 是给人看的
//!   锚点，喂给 LLM 只会浪费上下文并诱导它把时间码抄进纪要。
//! - **长文两轮 map-reduce**：一小时会议远超单次输入预算，直接截断等于只总结了开头。
//! - **绝不静默覆盖**：写盘前由 `Vault::distill_state` 判定；用户编辑过的蒸馏笔记只有
//!   显式 `force` 才会被重写（且照例先留快照）。
//! - **脱敏**（TDD-12）：日志只出路径与错误码，绝不含正文或 LLM 输出。
//!
//! # 模板与整理稿（PRD-06-051）
//!
//! 产出结构由 [`luna_core::TemplateId`] 决定（四个内置模板按**内容体裁**划分），可选在末尾
//! 附一份「整理稿」。两条本模块必须守住的纪律：
//!
//! - **模板解析只有一处** [`resolve_template`]（显式 > 已记录 > 建议）：单篇手动、
//!   「源已更新」重生成、自动档、批量整理四条路径全部经它。逐处各判一次的话，批量整理
//!   会把用户手选过的模板按规则**静默换回**默认值。
//! - **整理稿绝不走 reduce**：reduce 的本职是压缩，而整理稿要的是保真改写——逐片改写后
//!   由 Rust **按序确定性拼接**，并以 [`SummaryStyle::Faithful`] 提交（否则 Provider 会外套
//!   "Summarize … concisely" 的前缀，与改写指令正面相冲）。

use super::*;
use crate::processing_queue::{MergeOrigin, ProcPhase, ProcTaskKind, TaskCancel};
use luna_core::{KnownFacts, SourceShape, TemplateId, TemplateRef};
use luna_vault::DistillState;

/// 执行期回调：进度上报 + 取消（PRD-06-051 修订，真机反馈「等待时间较长、只能等一篇完
/// 再手动点下一篇」）。
///
/// 蒸馏一篇长录音是**几十次** LLM 调用、可达数分钟。此前它在命令里直接 await：切走视图
/// 就再也看不到跑到哪儿，也没有取消的路。现在执行体住进统一处理队列，本结构是它与队列
/// 之间的唯一接口——`None` 即"无人关心"（预览等内联路径），故同一份执行代码两处共用。
#[derive(Default)]
pub(crate) struct DistillReporter<'a> {
    /// 阶段 + 已完成/总数（队列面板的进度条据此渲染）。
    progress: Option<&'a (dyn Fn(ProcPhase, u32, u32) + Send + Sync)>,
    /// 取消令牌：**逐片检查 + 武装每个在途 Job**，故取消是真 abort 而非丢弃 future。
    cancel: Option<&'a TaskCancel>,
}

impl<'a> DistillReporter<'a> {
    pub(crate) fn new(
        progress: &'a (dyn Fn(ProcPhase, u32, u32) + Send + Sync),
        cancel: &'a TaskCancel,
    ) -> Self {
        Self {
            progress: Some(progress),
            cancel: Some(cancel),
        }
    }

    pub(crate) fn step(&self, phase: ProcPhase, done: u32, total: u32) {
        if let Some(report) = self.progress {
            report(phase, done, total);
        }
    }

    /// 已取消即中止（每片 LLM 调用前检查——一片可达数十秒，不能只在头尾看）。
    pub(crate) fn bail_if_cancelled(&self) -> Result<(), String> {
        self.cancel.map_or(Ok(()), TaskCancel::bail_if_cancelled)
    }

    fn arm(&self, token: luna_core::CancelToken) {
        if let Some(cancel) = self.cancel {
            cancel.arm(token);
        }
    }
}

/// 单次送入 LLM 的正文上限（字符）。超出即走分片摘要 → 二次汇总两轮。
///
/// 与悬浮菜单/标签提炼的 12k 同量级：本地小模型的上下文窗普遍在 4k token 上下，
/// 这个值已在既有路径上被真机验证过。
const DISTILL_CHUNK_CHARS: usize = 12_000;

/// 分片数上限。超出即在末尾诚实标注覆盖边界，绝不假装整篇都被读过（PRD-06-025）。
pub(crate) const DISTILL_MAX_CHUNKS: usize = 12;

/// 二次汇总阶段的输入上限：各分片摘要拼起来仍可能超，故同样受限。
const DISTILL_REDUCE_CHARS: usize = 12_000;

/// 整理稿（保真改写）的分片上限——**比摘要分片小一半**。
///
/// 摘要的输出只有输入的零头，改写的输出**与输入同量级**：同一个上下文窗要同时装下
/// "一片原文 + 一片改写"，故分片必须更小，否则模型写到一半就撞上窗口边界（表现为
/// 整理稿在句子中间断掉）。
const DISTILL_REWRITE_CHUNK_CHARS: usize = 6_000;

/// 蒸馏按全额档设计的上下文窗（token）：16k 覆盖「12k 字符输入 + 摘要输出」与「6k 字符
/// 输入 + 改写输出」两种最坏情形。更小的窗按它等比缩（[`DistillBudget::for_window`]）。
const DISTILL_CTX_TOKENS: u32 = 16_384;

/// 摘要阶段的输出预算（token）。
///
/// **不是"一篇纪要要写多长"，而是"这一次调用允许产出多少 token"**——两者曾被当成
/// 同一件事，于是这里写了 2048（"几千 token 足够"）。两处失真让它不再成立：
/// ① 推理模型的**思维链计入 `tokens_out`**（真机 7332 里 3540 帧是思考），正文还没开始
///    写预算就去了一半；② 服务端此前另有一道按能力的策略上限恰好也是 2048，两道闸叠在
///    一起，长内容片片触顶——而触顶的表现是整篇失败、已生成内容全部丢弃。
/// 策略 cap 退役后 `output_budget` 只是**调用方意图**，不再被任何策略夹取，故这里给足：
/// 与改写阶段同为 8192，标注（[`Generated::truncated`]）才可能是有信息量的少数事件，
/// 而不是恒真的噪声。
const DISTILL_SUMMARY_OUTPUT_TOKENS: u32 = 8_192;

/// 改写阶段的输出预算（token）：整理稿与输入同量级，须给足否则会被腰斩在半句话上。
const DISTILL_REWRITE_OUTPUT_TOKENS: u32 = 8_192;

/// 一次蒸馏的分片与输出预算，按模型这次加载的上下文窗缩放（ADR-143）。
///
/// 上面几个常量是 16k 窗下的**全额**档，服务端与外部端点始终用它（窗够大）。随包引擎的
/// 窗在加载时由内存预检定死、不能逐请求加大，且超窗
/// 是**当场拒绝**而不是截断：低档机或内存退让后窗只有 8k / 4k，原样送 12k 字的分片
/// 每一片都会以 `INPUT_TOO_LONG` 失败、整篇蒸馏起不来。故按窗等比缩小——各量之间的
/// 比例（输入占窗多少、给输出留多少）与全额档一致，那组比例是真机验证过的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DistillBudget {
    /// 摘要分片上限（字符）。
    chunk_chars: usize,
    /// 二次汇总的输入上限（字符）。
    reduce_chars: usize,
    /// 整理稿分片上限（字符）。
    rewrite_chunk_chars: usize,
    /// 摘要一次调用的输出预算（token）。
    summary_output: u32,
    /// 改写一次调用的输出预算（token）。
    rewrite_output: u32,
}

impl DistillBudget {
    /// 全额档（16k 窗）。
    pub(crate) const FULL: Self = Self {
        chunk_chars: DISTILL_CHUNK_CHARS,
        reduce_chars: DISTILL_REDUCE_CHARS,
        rewrite_chunk_chars: DISTILL_REWRITE_CHUNK_CHARS,
        summary_output: DISTILL_SUMMARY_OUTPUT_TOKENS,
        rewrite_output: DISTILL_REWRITE_OUTPUT_TOKENS,
    };

    /// 按窗缩放：不小于全额档的窗用全额，否则各量等比缩小。
    #[must_use]
    pub(crate) fn for_window(window: u32) -> Self {
        if window >= DISTILL_CTX_TOKENS {
            return Self::FULL;
        }
        // 等比：x × window / 16384。窗在内存退让的下限（4096）之下不会出现，但仍给一个
        // 不为 0 的地板，免得 `split_chunks` 拿到 0 按字切。
        let scale = |full: usize| (full * window as usize / DISTILL_CTX_TOKENS as usize).max(500);
        let scale_tokens = |full: u32| {
            u32::try_from(u64::from(full) * u64::from(window) / u64::from(DISTILL_CTX_TOKENS))
                .unwrap_or(full)
                .max(256)
        };
        Self {
            chunk_chars: scale(DISTILL_CHUNK_CHARS),
            reduce_chars: scale(DISTILL_REDUCE_CHARS),
            rewrite_chunk_chars: scale(DISTILL_REWRITE_CHUNK_CHARS),
            summary_output: scale_tokens(DISTILL_SUMMARY_OUTPUT_TOKENS),
            rewrite_output: scale_tokens(DISTILL_REWRITE_OUTPUT_TOKENS),
        }
    }
}

/// 此刻蒸馏该用的预算：默认档指向随包引擎条目时按它这次加载的窗缩放，其余全额。
///
/// 引擎条目没进 preset（因内存被拒）时也按全额——那时每一次调用都会以「未装进引擎」
/// 失败，缩不缩分片都一样，而失败本身才是该给用户看的信息。
pub(crate) async fn distill_budget(state: &AppState) -> DistillBudget {
    let settings = state.settings_snapshot();
    let Some(super::LocalLlm(id)) =
        super::local_llm_for_slot(&settings, &state.engine_models_snapshot(), "default_llm")
    else {
        return DistillBudget::FULL;
    };
    state
        .local_engine_plan()
        .lock()
        .await
        .models
        .iter()
        .find(|planned| planned.id == id)
        .map_or(DistillBudget::FULL, |planned| {
            DistillBudget::for_window(planned.preset.ctx)
        })
}

/// 前置预检比对的用量维度：闸量的是**输出** token（`tokens_out`）。
///
/// 输入侧那一维不是这条特性的对象——蒸馏的输入是用户已有的资料，它有多长是既成事实；
/// 而"跑到第 K 片才发现钱不够"这件事，卡住的恒是输出。
const DISTILL_QUOTA_DIMENSION: &str = "tokens_out";

// ── DTO ─────────────────────────────────────────────────────────────────────

/// 一篇笔记的蒸馏状态（前端徽标四态的唯一数据来源）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillStateDto {
    /// 该笔记是否**能**生成知识笔记（`NoteStage::allows_distill`：资料 ∪ 记录；
    /// 知识态笔记不渲染入口）。PRD-06-054 把这条入口从资料推广到用户记录。
    pub eligible: bool,
    /// 它是不是**资料**（来源轴）——页头「资料」徽标据此渲染。
    ///
    /// 与 `eligible` 刻意分成两个事实：自 PRD-06-054 起「能生成知识笔记」不再等于
    /// 「是资料」，记录态同样能生成。共用一个布尔会让「资料」徽标盖到草稿头上——而它
    /// 说的三条待遇（不入图谱、不打标、搜索折叠成资料组）对记录**一条都不对**。
    pub source_material: bool,
    /// 已生成的蒸馏笔记路径（`null` = 未生成，含指针悬空）。
    pub note_path: Option<String>,
    /// 源正文已变，蒸馏内容不再反映此刻的源。
    pub stale: bool,
    /// 蒸馏笔记被用户编辑过——自动路径一律止步，重生成必须由用户显式发起。
    pub edited: bool,
    /// 已生成的知识笔记不完整时**是哪一道闸到顶**：`caller` / `upstream` / `absolute`
    /// （[`luna_core::TruncatedBy::key`]）；`null` = 正文自然写完。
    ///
    /// **不是一个布尔**：三档各对应一个不同的用户动作（ADR-089 ⑧ / PRD-13 §1.5 E）
    /// ——`caller` 是客户端自己的编译期上限 + 确定性分片，**同一篇再跑必然在同一片撞同一
    /// 道闸**，把它和另外两档一起说成「重新整理」是在请用户白等一遍分钟级的生成；
    /// `upstream` / `absolute` 换一次候选可能就过去了。压成一句「被截断了」，猜错一次就是
    /// 把人引去改一个改了也没用的设置。归属早已落在产物 frontmatter 里，此前只是没人读。
    ///
    /// **不叫 `truncated`**：客户端既有的 `truncated`（`chat_session` / `chat.json:52`）
    /// 指的是**输入上下文被裁剪**——同屏两个相反方向的「截断」共用一个名字，读的人一定
    /// 会串。机器口径（协议帧、frontmatter、Fluent key、i18n key）一律走 `output_truncated`
    /// 系，本字段与它们同名同值。
    pub output_truncated: Option<String>,
    /// 已生成产物所用的模板键（`null` = 未生成或存量产物）。
    ///
    /// 只作**呈现**（徽标 hover 说明「会议纪要式 · 已生成」）。它直接来自已读的目标笔记
    /// frontmatter，零额外 IO——而「建议模板」要读边车数段落，故**刻意不放这里**：本 DTO
    /// 挂在 `vault://changed` 这条高频路径上，见 [`distill_template_options`]。
    pub template: Option<String>,
}

impl DistillStateDto {
    fn absent(eligible: bool, source_material: bool) -> Self {
        Self {
            eligible,
            source_material,
            note_path: None,
            stale: false,
            edited: false,
            output_truncated: None,
            template: None,
        }
    }
}

/// 生成选项（模板选单的数据源，PRD-06-051）。
///
/// **惰性获取**：只在用户展开选项浮层时调用一次。建议模板要读边车段落数说话人，而徽标的
/// [`DistillStateDto`] 每次 `vault://changed` 都会刷新（且不带路径过滤）——把这份计算挂在
/// 那条路径上等于把一次文件 IO 焊死在高频面上。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillOptionsDto {
    /// 可选模板清单（顺序即呈现顺序）。
    ///
    /// **由 Rust 下发而非前端硬编码**（ADR-003）：模板集与「哪个模板默认附整理稿」都是
    /// 业务事实，写死在前端就是第二份事实源——Rust 新增一个模板而前端没跟上时，那个模板
    /// 在浮层里根本不出现，且没有任何编译错误。
    pub templates: Vec<DistillTemplateChoiceDto>,
    /// 按规则建议的模板键（"自动适配"的实现）。
    pub suggested: String,
    /// 建议的判据（前端查 `distill.reason.*` 说明"为什么是它"）。
    pub reason: String,
    /// 此刻**实际会用**的模板键（已记录 > 建议）——浮层据此预选。
    pub template: String,
    /// 此刻实际会用的整理稿开关（已记录 > 模板默认值）。
    pub full_text: bool,
}

/// 一个可选模板（前端按 `distill.template.*` / `distill.templateDesc.*` 查文案）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillTemplateChoiceDto {
    /// 稳定键（内置 `meeting` / `talk` / …；自定义 `user:<id>`；模板包带来的 `pack:<包>/<id>`）。
    pub id: String,
    /// 自定义与模板包模板的展示名（直接呈现）；`null` = 内置，前端查 i18n。
    pub name: Option<String>,
    /// 本模板默认是否附整理稿——用户在浮层里换模板时，勾选随之取新模板的默认值。
    pub full_text_default: bool,
    /// 来自哪个模板包（展示名）；不是包里的为 `null`。
    pub pack_name: Option<String>,
}

// ── 输入装配 ────────────────────────────────────────────────────────────────

/// 蒸馏输入：把一篇笔记装配成适合喂给 LLM 的纯文本。
///
/// - 录音 / 转写：优先取**边车段落**（有说话人则逐行 `显示名: 文本`，供纪要归属行动项）；
///   无段落时退回正文（仅链接档等边界情形），此时时间码由 `clean_for_fts` 同源口径剥掉。
/// - 对话：取正文（`## User` / `## Assistant` 结构本身就是有效的蒸馏输入——谁问的、
///   谁答的决定了哪些是结论、哪些是我的诉求）。
/// - **记录**（PRD-06-054）：取正文。它本来就是用户敲下的散文，没有边车、没有时间码，
///   故既不查段落也不剥时间码——`clean_for_fts` 那一趟在这里只会白改用户的字。
fn distill_input(state: &AppState, app: &tauri::AppHandle, note_rel: &str) -> Option<DistillInput> {
    let i18n = backend_copy_arc(app);
    let guard = state.vault.lock().ok()?;
    let vault = guard.as_ref()?;
    let doc = vault.read(note_rel).ok()?;
    let stage = doc.meta.stage();
    if !stage.allows_distill() {
        return None;
    }

    // 记录（ADR-086）：来源轴之外的那一支。origin 分账在这里定，因为这是唯一同时知道
    // 「这一篇是哪一层」与「这次要喂什么进去」的地方。
    if stage.is_draft() {
        let (text, facts) = draft_distill_input(luna_vault::body_of(&doc.content), &doc.meta);
        return (!text.is_empty()).then_some(DistillInput {
            text,
            has_speakers: false,
            shape: SourceShape::WrittenNotes,
            origin: JobOrigin::Settle,
            facts,
        });
    }

    if matches!(doc.meta.note_type, NoteType::Chat) {
        let body = luna_vault::body_of(&doc.content).trim().to_owned();
        return (!body.is_empty()).then_some(DistillInput {
            text: body,
            has_speakers: false,
            shape: SourceShape::Conversation,
            origin: JobOrigin::Distill,
            facts: KnownFacts::default(),
        });
    }

    // 说话人数是**建议模板**的判据（多人 → 会议式 / 单人 → 讲述式），故与文本一并
    // 由装配阶段如实给出：事后照着 `显示名: 文本` 去反推，会把「注意: 这里有坑」
    // 这样的普通句子误判成说话人行。
    let mut segments = super::playback::collect_note_segments(vault.root(), &doc.meta);
    let mut speakers = 0usize;
    if !segments.is_empty() {
        let labeled = super::playback::resolve_segment_speakers(
            &mut segments,
            &doc.meta.speakers,
            i18n.as_deref(),
        );
        let mut out = String::new();
        let mut distinct: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for seg in &segments {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            if let Some(name) = seg.speaker.as_deref() {
                distinct.insert(name);
                if labeled {
                    out.push_str(name);
                    out.push_str(": ");
                }
            }
            out.push_str(text);
            out.push('\n');
        }
        speakers = distinct.len();
        if !out.trim().is_empty() {
            return Some(DistillInput {
                text: out,
                has_speakers: labeled,
                shape: shape_of(doc.meta.note_type, speakers),
                origin: JobOrigin::Distill,
                facts: KnownFacts::default(),
            });
        }
    }
    // 无段落（如转写尚未完成）：退回正文并剥掉时间码——与 FTS 同一口径（DRY）。
    let body = luna_vault::clean_for_fts(&doc.content).trim().to_owned();
    (!body.is_empty()).then_some(DistillInput {
        text: body,
        has_speakers: false,
        shape: shape_of(doc.meta.note_type, speakers),
        origin: JobOrigin::Distill,
        facts: KnownFacts::default(),
    })
}

/// 记录态笔记（速记 / 待整理）的蒸馏输入（纯函数）：
/// - 语音速记的「原始转录」一节剔掉——它与「优化结果」是同一段话的两个版本，一起送只会
///   让模型把重复当强调；
/// - 跑过 AI 提炼（`refined_at` 有值）的，把它抠出来的待办（正文里的复选框行）与日期
///   （`luna.dates`）作为已确认事实交给模型、要求原样保留（PRD-06-058 C′ ↔ PRD-06-054）。
fn draft_distill_input(body: &str, meta: &luna_vault::LunaMeta) -> (String, KnownFacts) {
    let mut text = String::new();
    let mut skipping = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") {
            let heading = trimmed.trim_start_matches('#').trim();
            skipping = super::voice::VOICE_RAW_HEADINGS.contains(&heading);
            if skipping {
                continue;
            }
        }
        if !skipping {
            text.push_str(line);
            text.push('\n');
        }
    }
    let facts = if meta.refined_at.is_some() {
        KnownFacts {
            action_items: body
                .lines()
                .map(str::trim)
                .filter_map(|l| {
                    l.strip_prefix("- [ ] ")
                        .or_else(|| l.strip_prefix("- [x] "))
                        .map(|t| t.trim().to_owned())
                })
                .filter(|t| !t.is_empty())
                .collect(),
            dates: meta.dates.clone(),
        }
    } else {
        KnownFacts::default()
    };
    (text.trim().to_owned(), facts)
}

/// 资料类型 + 说话人数 → 体裁线索（建议模板的输入，[`luna_core::suggest`]）。
fn shape_of(note_type: NoteType, speakers: usize) -> SourceShape {
    match note_type {
        NoteType::Chat => SourceShape::Conversation,
        NoteType::Recording => SourceShape::OwnRecording { speakers },
        // 其余资料类型（transcript）都是导入得来的媒体文稿。
        _ => SourceShape::ImportedTranscript { speakers },
    }
}

/// 蒸馏输入：文本 + 「行首是否带说话人显名」+ 体裁线索 + 用量归因。
///
/// `has_speakers` 与 `shape` 都由**装配阶段**如实给出，而不是事后去猜文本长什么样。
struct DistillInput {
    text: String,
    has_speakers: bool,
    shape: SourceShape,
    /// 这次调用记在哪本账上：资料 → [`JobOrigin::Distill`]，记录 → [`JobOrigin::Settle`]
    /// （PRD-06-054：只加 origin、不加 `JobKind`）。
    ///
    /// **随输入一路传到 [`summarize_once`]，而不是在提交点自己判一次**：一次蒸馏是
    /// 几十次 Job（分片 / 汇总 / 整理稿），提交点有三处——各自去问"这一篇是记录吗"，
    /// 就是同一个事实读三遍、且其中一处漏掉时用量账悄悄记错本子。
    origin: JobOrigin,
    /// 来源上已确认的事实（目前只有跑过 AI 提炼的速记会带，PRD-06-058 C′）。
    facts: KnownFacts,
}

// ── 模板解析（唯一判定点）──────────────────────────────────────────────────

/// 解析本次该用哪个模板、要不要附整理稿。**四条触发路径的唯一判定点**。
///
/// 优先级：**显式指定（用户这次在浮层里选的） > 已记录（既有蒸馏笔记 frontmatter）>
/// 规则建议**。
///
/// 少了中间那一层，「源已更新」重生成 / 自动档 / 批量整理都会按规则重新建议，把用户
/// 手选过的模板**静默换掉**——而批量整理的待办桶里本就**包含**「已生成但源已更新」这些
/// 重生成件（`distill_scan` 的 pending 桶），所以这不是边角情形，是必然发生。
/// 判定本身是[`luna_core::resolve_distill_template`]（纯函数、可单测）；本函数只把
/// `DistillState` 拆成它要的两个字段——IO 侧不重写判定逻辑（ADR-003 的 Rust 内部同构）。
fn resolve_template(
    explicit: Option<&str>,
    explicit_full_text: Option<bool>,
    existing: Option<&DistillState>,
    shape: SourceShape,
    templates: luna_core::TemplateSet<'_>,
) -> luna_core::Resolution {
    let (recorded, recorded_full_text) = match existing {
        Some(DistillState::Present {
            template,
            full_text,
            ..
        }) => (template.as_deref(), *full_text),
        _ => (None, None),
    };
    luna_core::resolve_distill_template(
        explicit,
        explicit_full_text,
        recorded,
        recorded_full_text,
        shape,
        templates,
    )
}

/// 把解析结果换成可直接用于生成的模板引用。
///
/// `resolve` 返回的键恒可查到（选中的那个本就来自 `find` 或内置建议），故这里的兜底
/// 只是不写 `expect` 的纪律，不是真会走到的分支。
fn resolved_ref<'a>(
    resolution: &luna_core::Resolution,
    templates: &'a DistillTemplates,
) -> TemplateRef<'a> {
    luna_core::find_distill_template(&resolution.key, templates.set())
        .unwrap_or(TemplateRef::Builtin(resolution.suggested))
}

/// 除内置之外可选的整理模板：库里的用户模板与装上的模板包带来的（ADR-169 ⑤）。
struct DistillTemplates {
    user: Vec<luna_core::UserTemplate>,
    packs: Vec<luna_core::PackTemplate>,
    /// 与 `packs` 一一对应的包展示名（选择面标出来源）。
    pack_names: Vec<String>,
}

impl DistillTemplates {
    fn set(&self) -> luna_core::TemplateSet<'_> {
        luna_core::TemplateSet {
            user: &self.user,
            packs: &self.packs,
        }
    }
}

/// 读当前库的用户模板与模板包带来的（读不到即空表——模板文件损坏不该让整理跑不起来）。
fn distill_templates(app: &tauri::AppHandle, state: &AppState) -> DistillTemplates {
    let user = state
        .vault
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|vault| vault.distill_templates().ok())
        })
        .unwrap_or_default();
    let (packs, pack_names) = pack_distill_templates(&installed_template_packs(app))
        .into_iter()
        .unzip();
    DistillTemplates {
        user,
        packs,
        pack_names,
    }
}

// ── 分块摘要（map-reduce）─────────────────────────────────────────────────────
//
// 指令合成全在 `luna_core::distill_template`（纯函数、可单测）：三段正交拼装
// 「输入形态 + 输出区块 + 纪律」，故任何模板配任何源都诚实成立。

/// 按字符预算切片，**优先在段落/行边界切**——从句子中间切开会让分片摘要
/// 把半句话当成完整意思去理解。
pub(crate) fn split_chunks(text: &str, budget: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > budget && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if line.chars().count() > budget {
            // 单行就超预算（无换行的长文）：按字符边界硬切，这一步无从优雅。
            let mut rest = line;
            while rest.chars().count() > budget {
                let cut = rest
                    .char_indices()
                    .nth(budget)
                    .map_or(rest.len(), |(idx, _)| idx);
                chunks.push(rest[..cut].to_owned());
                rest = &rest[cut..];
            }
            current.push_str(rest);
        } else {
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// **模型产出落盘前的确定性归一——唯一咽喉**（PRD-06-057）。
///
/// 蒸馏管线里**每一段**模型产出（分片摘要、二次汇总、整理稿）都经 [`summarize_once`]，
/// 而这里是它取到文本后的第一站。收在一处而非各拼装点：放到 `build_distill_body` 与
/// `merge_notes_into_one` 各写一遍，等于给同一条纪律留两个会漂的入口。
///
/// 两步，都是**幂等的纯函数**，都只删不加：
/// 1. 死引用角标（`[1]` / `[2]`）——理由在 [`luna_core::strip_dead_citations`]：源材料里
///    可能有角标，但它们在这篇新笔记里指向不存在的东西。
/// 2. 单行空嵌套列表链（`* * 文字` / `* * * * * * 文字`）折叠为单个标记——理由在
///    [`luna_vault::tidy_list_levels`]。这是 PRD-06-056 那条**显式动作**的另一半：
///    056 让用户修既有文件（那是他的文件，要他确认），本函数从源头杜绝新增。
///
/// **只对模型产出生效，绝不碰用户手写内容**：本函数只在这一条路径上被调用，而这条
/// 路径上的每一个字都是模型刚刚生成、尚未落盘的。用户自己写的畸形列表由 PRD-06-056 的
/// 显式动作处理——「静默改用户的文件」与「把要不要改判给机器」是同一件事（PRD-06-053
/// 同一纪律），而对一段模型刚吐出来、还没有人看过的文本做确定性清理，不是那件事。
pub(crate) fn normalize_model_output(raw: &str) -> String {
    let text = luna_core::strip_dead_citations(raw.trim());
    luna_vault::tidy_list_levels(&text).content
}

/// 一段模型产出 + **它有没有在输出闸上触顶**。
///
/// **蒸馏链路从此弃裸 `String`**：此前 [`summarize_once`] 只从 `JobEvent::Finished` 里
/// 取走 `ResultPayload::Text`，整个 [`luna_core::JobResult`] 连同 `output_truncated` 一起
/// 丢掉。于是"模型的话没说完"这个事实在管线的**第一站**就已经不存在了，后面无论加多少
/// 标注面都无从谈起。本结构存在的全部意义就是把它带到落盘处。
struct Generated {
    /// 已经过 [`normalize_model_output`] 的正文。
    text: String,
    /// 哪道闸先到（`None` = 没触顶，或执行方给不出这个事实）。
    truncated: Option<luna_core::TruncatedBy>,
}

impl Generated {
    /// 一次蒸馏是几十次调用：**任何一片触顶，整篇产物就是不完整的**。
    ///
    /// 归属取**先遇到的那一个**：同一次蒸馏里每一片走的是同一组闸（同一个调用方预算、
    /// 同一个上游、同一个部署级上限），所以先后两片报出不同归属只会是上游在中途被换掉
    /// 这种边角情形——那时报哪一个都对不出更有用的动作，故不做合并规则。
    fn absorb(&mut self, other: &Self) {
        self.truncated = self.truncated.or(other.truncated);
    }
}

/// 提交一次 Summarize Job 并等结果（单一 AI 主干，ADR-001）。
///
/// `style` 决定 Provider 怎么用 `instruction`：`Faithful` 档它**就是**完整 system prompt
/// （整理稿的保真改写走这一档），其余档由 Provider 外套通用摘要前缀。
#[allow(clippy::too_many_arguments)]
async fn summarize_once(
    state: &AppState,
    i18n: BackendCopy<'_>,
    text: String,
    instruction: luna_core::Instruction,
    style: SummaryStyle,
    output_budget: u32,
    origin: JobOrigin,
    reporter: &DistillReporter<'_>,
) -> Result<Generated, String> {
    reporter.bail_if_cancelled()?;
    let mut job = Job::new(
        JobKind::Summarize {
            text: TextRef::Inline(text),
            style,
            instruction: Some(instruction),
        },
        // **只换 origin、不换 `JobKind`**（PRD-06-054 / ADR-086 ②）：沉淀与蒸馏跑的
        // 是同一条主干，分开的只有那本用量账。
        origin,
    );
    // 恒后台优先级：蒸馏是"点了就去做别的"的长任务，不与用户正在等的交互任务抢队。
    job.priority = Priority::Background;
    job.routing.local_only = state.strict_local();
    // 显式给输出预算：不给的话长改写会被腰斩在半句话上且不报错（见常量注释）。
    job.options.output_budget = Some(output_budget);

    let mut handle = state.queue.submit(job);
    // 武装在途 Job：取消是**真 abort**（一路传到 Provider），不是丢弃 future 后
    // 让模型继续烧着 GPU 把结果算完。
    reporter.arm(handle.cancel_token());
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                // 先把触顶归属取出来再解构 payload：反过来写的话 `result` 已被部分移动，
                // 而"顺手补一句读 `result.output_truncated`"正是上一版丢掉它的方式。
                let truncated = result.output_truncated;
                if let ResultPayload::Text(text) = result.payload {
                    let text = normalize_model_output(&text);
                    if text.trim().is_empty() {
                        return Err(coded_error_with(i18n, "DISTILL_EMPTY", "distill-empty"));
                    }
                    return Ok(Generated { text, truncated });
                }
                return Err(coded_error_with(i18n, "DISTILL_JOB", "distill-job-payload"));
            }
            JobEvent::Failed(err) => return Err(format!("DISTILL_JOB: {}", err.code())),
            JobEvent::Cancelled => {
                return Err(coded_error_with(
                    i18n,
                    "DISTILL_CANCELLED",
                    "distill-cancelled",
                ))
            }
            _ => {}
        }
    }
    Err(coded_error_with(
        i18n,
        "DISTILL_JOB",
        "distill-job-no-result",
    ))
}

/// 本次实际能覆盖到的输入范围（**摘要与整理稿共用同一个边界**）。
///
/// 两条流水各自截断的话，末尾那句「覆盖了前一部分」会对不上其中一条——用户看到的是
/// 一份声称覆盖前 8 成的纪要配一份只到 6 成的整理稿，而没有任何地方说得清。
struct Covered {
    /// 摘要用的分片（已按上限截断）。
    chunks: Vec<String>,
    /// 截断后的完整文本（整理稿按自己的粒度重新切它）。
    text: String,
    /// 是否有内容被丢在覆盖边界之外。
    dropped: bool,
}

fn cover(text: &str, budget: DistillBudget) -> Covered {
    let mut chunks = split_chunks(text, budget.chunk_chars);
    let dropped = chunks.len() > DISTILL_MAX_CHUNKS;
    chunks.truncate(DISTILL_MAX_CHUNKS);
    let joined = chunks.concat();
    Covered {
        chunks,
        text: joined,
        dropped,
    }
}

/// 两轮 map-reduce：单片直接一轮；多片先逐片压缩，再把各片摘要汇总成稿。
#[allow(clippy::too_many_arguments)]
async fn summarize_long(
    state: &AppState,
    i18n: BackendCopy<'_>,
    chunks: &[String],
    chunk_hint: luna_core::Instruction,
    final_instruction: luna_core::Instruction,
    origin: JobOrigin,
    budget: DistillBudget,
    reporter: &DistillReporter<'_>,
) -> Result<Generated, String> {
    let summary = budget.summary_output;
    if chunks.len() <= 1 {
        let text = chunks.first().cloned().unwrap_or_default();
        reporter.step(ProcPhase::Distilling, 0, 1);
        let out = summarize_once(
            state,
            i18n,
            text,
            final_instruction,
            SummaryStyle::Bullets,
            summary,
            origin,
            reporter,
        )
        .await;
        reporter.step(ProcPhase::Distilling, 1, 1);
        return out;
    }

    // 总数含末尾那次汇总（+1）：只数分片会让进度条走到 100% 后还在跑，用户以为卡住了。
    let total = u32::try_from(chunks.len() + 1).unwrap_or(u32::MAX);
    let mut digests = Vec::with_capacity(chunks.len());
    // 分片阶段的触顶要一路带到汇总之后：第 3 片被腰斩、汇总本身没触顶时，成稿仍然缺了
    // 第 3 片的后半段——只看最后那次调用的话，这一篇会被当成完整产物。
    let mut truncated = None;
    for (idx, chunk) in chunks.iter().enumerate() {
        reporter.step(
            ProcPhase::Distilling,
            u32::try_from(idx).unwrap_or(u32::MAX),
            total,
        );
        let digest = summarize_once(
            state,
            i18n,
            chunk.clone(),
            chunk_hint.clone(),
            SummaryStyle::Bullets,
            summary,
            origin,
            reporter,
        )
        .await?;
        truncated = truncated.or(digest.truncated);
        digests.push(format!("--- 第 {} 部分 ---\n{}", idx + 1, digest.text));
    }
    reporter.step(ProcPhase::Distilling, total - 1, total);
    let mut joined = digests.join("\n\n");
    if joined.chars().count() > budget.reduce_chars {
        let cut = joined
            .char_indices()
            .nth(budget.reduce_chars)
            .map_or(joined.len(), |(i, _)| i);
        joined.truncate(cut);
    }
    let mut reduced = summarize_once(
        state,
        i18n,
        joined,
        final_instruction,
        SummaryStyle::Bullets,
        summary,
        origin,
        reporter,
    )
    .await?;
    reduced.truncated = reduced.truncated.or(truncated);
    Ok(reduced)
}

/// 整理稿：把覆盖范围内的正文**逐片保真改写**，再按序拼接。
///
/// **绝不走 reduce**：二次汇总的本职就是压缩，把改写结果再汇总一次等于把整理稿做成
/// 另一份摘要。拼接是确定性动作，交给 Rust——交给 LLM 只会丢内容。
/// **以 `Faithful` 档提交**：否则 Provider 会外套「Summarize … concisely」，与"这是改写
/// 不是总结"的指令正面相冲。
async fn rewrite_full_text(
    state: &AppState,
    i18n: BackendCopy<'_>,
    covered: &str,
    lang: &Lang,
    origin: JobOrigin,
    budget: DistillBudget,
    reporter: &DistillReporter<'_>,
) -> Result<Generated, String> {
    let instruction = luna_core::compose_rewrite_instruction(luna_core::prompt::registry(), lang);
    let chunks = split_chunks(covered, budget.rewrite_chunk_chars);
    let total = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
    let mut parts = Vec::with_capacity(chunks.len());
    let mut truncated = None;
    for (idx, chunk) in chunks.into_iter().enumerate() {
        reporter.step(
            ProcPhase::Rewriting,
            u32::try_from(idx).unwrap_or(u32::MAX),
            total,
        );
        let part = summarize_once(
            state,
            i18n,
            chunk,
            instruction.clone(),
            SummaryStyle::Faithful,
            budget.rewrite_output,
            origin,
            reporter,
        )
        .await?;
        truncated = truncated.or(part.truncated);
        let part = part.text.trim().to_owned();
        if !part.is_empty() {
            parts.push(part);
        }
    }
    Ok(Generated {
        text: parts.join("\n\n"),
        truncated,
    })
}

/// 一次蒸馏的**输出 token 需求估算**：分片数 × 单片预算（PRD-13-040 ②）。
///
/// 一次蒸馏是**几十次** LLM 调用（本模块头自述），且逐片 `await?`——任一片失败即整篇
/// 失败、落盘从未发生。用户那篇「附整理稿」正是这样蒸发的：跑到第 K 片撞上 402，
/// 前 K-1 片的产出与那部分额度一起没了。
///
/// # 宁可估少，绝不估多
///
/// 估多的代价是**拦下一件本来能跑完的任务**——那就把善意提醒变成了新的卡点。故：
/// - 分片按真实切分算（与执行时同一对函数 [`cover`] / [`split_chunks`]，不另写一套口径）；
/// - 但不给思维链、重试、系统提示留任何余量：真实消耗只会比这个数**大**。
///
/// 于是本函数是一个**下界**，只有缺口大到连下界都装不下时才会拒绝。
fn estimate_output_tokens(text: &str, full_text: bool, budget: DistillBudget) -> u64 {
    let covered = cover(text, budget);
    // 单片时只有一次调用；多片时逐片一次 + 末尾汇总一次（与 `summarize_long` 同构）。
    let summary_calls = match covered.chunks.len() {
        0 | 1 => 1,
        n => n + 1,
    };
    let rewrite_calls = if full_text {
        split_chunks(&covered.text, budget.rewrite_chunk_chars).len()
    } else {
        0
    };
    (summary_calls as u64) * u64::from(budget.summary_output)
        + (rewrite_calls as u64) * u64::from(budget.rewrite_output)
}

/// 一段文字约合多少 token（任务前预估用，PRD-13-040 ②）：中日韩字符一字一个，其余四个字符一个。
/// 粗估，只用来回答「约消耗多少」——它不是闸，服务端预留才是。
fn rough_tokens(text: &str) -> u64 {
    let (wide, other) = text.chars().fold((0u64, 0u64), |(wide, other), c| {
        if matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}')
        {
            (wide + 1, other)
        } else {
            (wide, other + 1)
        }
    });
    wide + other.div_ceil(4)
}

/// 一篇蒸馏的**输入 token 估算**：摘要那一遍读一次全文，带整理稿时再读一次。与输出估算
/// （[`estimate_output_tokens`]）同一取向——不给提示词与重试留余量，宁可估少。
fn estimate_input_tokens(text: &str, full_text: bool) -> u64 {
    let once = rough_tokens(text);
    if full_text {
        once * 2
    } else {
        once
    }
}

/// 这几篇蒸馏约要多少（输入，输出）token（任务前预估，PRD-13-040 ②，纵切 3 C4）。输出与入队预检
/// 同一口径（[`estimate_output_tokens`]），整理稿只在显式为真时计（同 [`enqueue_distill`]）。读不到
/// 输入的篇目跳过；一篇都估不出 = `None`（不给预估）。
pub(crate) fn distill_token_need(
    app: &tauri::AppHandle,
    note_rels: &[String],
    full_text: Option<bool>,
) -> Option<(u64, u64)> {
    let state = app.state::<AppState>();
    let full_text = full_text == Some(true);
    let mut any = false;
    let (mut input, mut output) = (0u64, 0u64);
    for rel in note_rels {
        let Some(distill) = distill_input(&state, app, rel) else {
            continue;
        };
        any = true;
        input += estimate_input_tokens(&distill.text, full_text);
        output += estimate_output_tokens(&distill.text, full_text, DistillBudget::FULL);
    }
    any.then_some((input, output))
}

/// 一个范围里待整理的篇目（与 [`distill_batch_start`] 同一份扫描）。库没开 / 扫不动 = `None`。
pub(crate) fn distill_scope_targets(state: &AppState, scope: Option<&str>) -> Option<Vec<String>> {
    let guard = state.vault.lock().ok()?;
    let vault = guard.as_ref()?;
    Some(vault.distill_scan(scope).ok()?.pending)
}

/// 取余量这一趟允许花多久。**超时即当"不知道"，照常起跑**。
///
/// 这道保险不是可有可无的：`CloudClient` 的 HTTP 客户端**没有默认请求超时**，
/// 而本次改动把这一趟塞进了「点整理」这条同步路径的前面。网络被黑洞掉（连上了一个
/// 需要认证的 Wi-Fi、VPN 半死、服务端挂着不回）时，没有超时的表现是**按钮按下去没反应**
/// ——预检当场变成了它最不该变成的东西。3 秒是"用户还感觉得到是在做事"的量级。
const DISTILL_QUOTA_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 起跑线上还剩多少输出额度。**`None` = 读不到，调用方一律放行**。
///
/// 读不到的情形一个都不许变成卡点：未登录、离线、严格本地、私有化部署、跑本机模型、
/// 服务端不按 token 计量、账本写着「不限」、以及**这一趟太慢**——这些用户压根没有额度
/// 这个概念，给他们造一道门只会让「整理」这个按钮在完全正常的环境里失灵。故本函数把
/// **一切**异常都读成"不知道"。
///
/// 走既有的云端用量通路（`/v1/usage` → `fetch_usage`），不新造第二条：那条路已经处理了
/// 严格本地拦截、无凭据本地短路与 401 续期，重写一遍只会漏掉其中一样。
async fn cloud_output_headroom(app: &tauri::AppHandle) -> Option<f64> {
    let client = app
        .try_state::<luna_cloud::commands::CloudState>()
        .map(|state| std::sync::Arc::clone(&state.0))?;
    let observer = app.try_state::<luna_cloud::commands::AccountObserverState>()?;
    // 取数走唯一那条 `fetch_usage`：这一趟取到的 95% 同样要提醒（PRD-13-040 ③）。
    let fetch = luna_cloud::commands::fetch_usage(&client, &observer);
    let usage = tokio::time::timeout(DISTILL_QUOTA_PROBE_TIMEOUT, fetch)
        .await
        .ok()?
        .ok()?;
    luna_cloud::client::remaining_in_dimension(&usage.ledgers, DISTILL_QUOTA_DIMENSION)
}

/// 预检缺口的**三个数**：需要多少、还剩多少、差多少。
///
/// **是三个数而不是一句话**：只说"额度不足"的话，用户既不知道该补多少，也无从判断
/// 是不是该改用本机模型——而那正是这条提示唯一能给出的两个动作。措辞归 Fluent
/// （`distill-quota-short`），本结构只负责把三个数原样带过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuotaShortfall {
    /// 这一篇估算需要的输出 token。
    need: u64,
    /// 起跑线上还剩多少（向下取整）。
    remaining: u64,
    /// 差多少（向上取整）。
    short: u64,
}

impl QuotaShortfall {
    /// 稳定机器前缀 + 本地化说明（同 [`coded_error`]，理由见那里）。
    fn message(self, app: &tauri::AppHandle) -> String {
        coded_error_args(
            app,
            "DISTILL_QUOTA_SHORT",
            "distill-quota-short",
            &[
                ("need", self.need.to_string()),
                ("remaining", self.remaining.to_string()),
                ("short", self.short.to_string()),
            ],
        )
    }
}

/// 起跑线余量的**累减账本**：一批任务共用一份余量，判过一件就扣一件。
///
/// **累减是批量路径的正确口径，逐篇比对不是**。逐篇比对下，余量 100k、每篇需 50k 的
/// 50 篇会**全部通过预检**，而只有前两篇真跑得完——「跑到第 K 片才发现钱不够」这件事被
/// 从单篇里赶出去之后，在批量里原样复活了一遍。累减唯一的坏处（"后面那些静默落空"）
/// 由调用方逐篇交出拒绝理由来抵消，见 [`distill_batch_start`]。
///
/// `None` = 读不到余量（离线 / 未登录 / 本机模型 / 私有化部署 / 不限额），此时账本
/// **恒放行且不记账**——那些用户压根没有额度这个概念。
pub(crate) struct QuotaLedger {
    remaining: Option<f64>,
}

impl QuotaLedger {
    /// 以起跑线余量开账（`None` = 读不到，恒放行）。
    pub(crate) fn new(headroom: Option<f64>) -> Self {
        Self {
            remaining: headroom,
        }
    }

    /// 记一件的估算需求：装得下就扣掉并放行，装不下就交出三个数。
    ///
    /// 扣的是**估算值**而不是实际用量——账本活不过这一次入队循环，它要回答的问题是
    /// 「按这一批的估算，从第几篇起就一定不够了」，而不是记账。
    fn charge(&mut self, need: u64) -> Result<(), QuotaShortfall> {
        let Some(remaining) = self.remaining else {
            return Ok(());
        };
        #[allow(clippy::cast_precision_loss)]
        let need_f = need as f64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let remaining_u = remaining.max(0.0) as u64;
        if need_f > remaining {
            return Err(QuotaShortfall {
                need,
                remaining: remaining_u,
                short: (need_f - remaining).ceil() as u64,
            });
        }
        self.remaining = Some(remaining - need_f);
        Ok(())
    }
}

// ── 命令 ────────────────────────────────────────────────────────────────────

/// 读一篇笔记的蒸馏状态（徽标四态的唯一数据来源）。
///
/// 资料与**记录**都能生成知识笔记（PRD-06-054），判据是 `NoteStage::allows_distill`；
/// 「是不是资料」另作一个事实下发（页头那枚「资料」徽标要它，而它已不等于 `eligible`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn distill_status(
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<DistillStateDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let doc = vault.read(&note_rel).map_err(vault_err)?;
    let stage = doc.meta.stage();
    let eligible = stage.allows_distill();
    let source_material = doc.meta.note_type.is_source_material();
    drop(guard);
    if !eligible {
        return Ok(DistillStateDto::absent(false, source_material));
    }
    current_distill_state(&state, &note_rel, source_material)
}

/// 生成（或重新生成）**这几篇**资料笔记的蒸馏笔记（单篇 = 只交一条路径）。
///
/// `force = false` 时**拒绝覆盖用户编辑过的蒸馏笔记**（返回 `DISTILL_EDITED`），
/// 这是 ADR-024「不静默覆盖」在命令层的守门；自动触发路径恒用 `false`。
///
/// **N=1 不另起一条命令**：多选右键「生成知识笔记」与单篇右键是同一个动作，只是
/// 对象多了几个。此前的 `distill_note` 只收一条路径且**把结果压成 `()`**——被挡下的
/// 理由（已编辑 / 不合资格 / 余量不够）在返回值里就消失了，界面只能报一句泛泛的失败。
/// 现在两条路共用同一个返回：进了几件、哪些没进、为什么。
#[tauri::command]
#[specta::specta]
pub(crate) async fn distill_notes(
    app: tauri::AppHandle,
    note_rels: Vec<String>,
    template: Option<String>,
    full_text: Option<bool>,
) -> Result<DistillBatchStartDto, String> {
    if note_rels.is_empty() {
        return Err(coded_error(
            &app,
            "DISTILL_BATCH_EMPTY",
            "distill-batch-empty",
        ));
    }
    Ok(enqueue_distill_all(&app, note_rels, template, full_text).await)
}

/// 把一件蒸馏排进统一处理队列（PRD-06-051 修订）。
///
/// **同一笔记已在队列里就幂等返回**（与 `enqueue_diarize` 同一先例）——用户连点两下
/// 不该排出两件跑同一篇的任务。
///
/// **刻意不带 `force`**：唯一需要覆盖「用户编辑过」的场景是显式重新整理，而那条路
/// 恒经差异预览（PRD-06-045），不进队列。于是排队中的任务**永远不会**静默覆盖用户资产；
/// 若某篇在排队期间被用户编辑，执行时闸门照常拦下并如实报 `DISTILL_EDITED`。
///
/// `ledger` = 起跑线余量的累减账本（[`QuotaLedger`]）。**读不到余量一律放行**——它的
/// 含义是"读不到"，不是"没有了"。账本由调用方开而不是这里开：批量一次会调本函数几十次，
/// 每次发一趟 `/v1/usage` 既慢又毫无必要，且**一批共用一份余量**这件事只有调用方看得见。
pub(crate) fn enqueue_distill(
    app: &tauri::AppHandle,
    note_rel: String,
    template: Option<String>,
    full_text: Option<bool>,
    ledger: &mut QuotaLedger,
) -> Result<bool, String> {
    let state = app.state::<AppState>();
    if state.processing.has_active(&note_rel, "distill") {
        // 幂等去重，但**如实说没有新入队**——批量的「已加入 N 件」按真入队数报，
        // 用户先手动排过两篇再点批量时，数字不该虚高。
        return Ok(false);
    }
    // 诚实前置：注定失败的不入队（点了「已加入队列」、几秒后面板里冒出一条红的，
    // 比当场说"不行、为什么不行"糟糕得多）。三条硬事实在入队时就能判：
    // ① 类型不合资格；② 知识笔记已被用户编辑（覆盖它的唯一合法路径是差异预览）；
    // ③ 服务端余量装不下这一篇。
    // 「排队期间才被编辑」的竞态仍由 worker 里的同一闸门兜底。
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        // 资料 ∪ 记录（PRD-06-054）。判据是 `NoteStage::allows_distill` 那一处，
        // 三道门（入队前置 / worker 门槛 / 预览）与输入装配问的是同一个函数。
        if !doc.meta.stage().allows_distill() {
            return Err(coded_error(app, "DISTILL_INELIGIBLE", "distill-ineligible"));
        }
        if let Ok(DistillState::Present { edited: true, .. }) = vault.distill_state(&note_rel) {
            return Err(coded_error(app, "DISTILL_EDITED", "distill-edited"));
        }
    }
    // ③ 额度：一次蒸馏是几十次调用，逐片 `await?`，第 K 片被拒即整篇失败、落盘从未发生。
    // 兑现 PRD-13-040 ② 的前半句——「剩余额度不足时前置提示而非跑到一半失败」。
    //
    // **只在读得到余量时才算估算**：账本为"读不到"（离线 / 未登录 / 本机模型 /
    // 私有化部署 / 不限额）时连输入都不装配——那趟装配要读边车段落，批量里是每篇一次
    // 文件 IO，为一个注定放行的判断白花。
    if ledger.remaining.is_some() {
        if let Some(input) = distill_input(&state, app, &note_rel) {
            // 整理稿显式为真时才计它的那几十次调用：`full_text = None` 交由
            // `resolve_template` 在执行时定，这里按不带整理稿估——估少了放行，估多了拦错人。
            // 按全额档估：窗小时分片翻倍、每片输出预算减半，总量与全额档同量级，
            // 本函数「宁可估少」的下界性质不变；而这里是同步入队路径，不去等编排锁。
            let need =
                estimate_output_tokens(&input.text, full_text == Some(true), DistillBudget::FULL);
            ledger.charge(need).map_err(|gap| gap.message(app))?;
        }
    }
    let label = super::transcribe::note_task_label(&note_rel);
    state.processing.enqueue(
        ProcTaskKind::Distill {
            note_rel,
            template,
            full_text,
        },
        label,
    );
    // 入队后**立即**广播队列快照（与 `enqueue_diarize` 等既有入队命令同一纪律）：
    // worker 忙着时（正在跑另一件、或正在等一条取消落定）`start_processing_worker_if_idle`
    // 什么都不做，而 worker 只在自己的循环节点才广播——漏掉这一步，新任务要等上一件
    // 完成才出现在面板里（真机反馈原话："取消之后再添加新任务，直到上一个任务完成
    // 才刷新"）。
    super::transcribe::emit_processing_queue(app);
    super::transcribe::start_processing_worker_if_idle(app);
    Ok(true)
}

/// 执行一件蒸馏（**只由队列 worker 调用**）。
pub(crate) async fn run_distill(
    app: &tauri::AppHandle,
    note_rel: &str,
    explicit_template: Option<String>,
    explicit_full_text: Option<bool>,
    reporter: &DistillReporter<'_>,
) -> Result<DistillStateDto, String> {
    let state = app.state::<AppState>();

    // 发起时的库代际（ADR-025）：LLM 生成是分钟级 await，期间用户可能切库——
    // `note_rel` 与 `target_rel` 都是库相对路径，写进新库就是改错库的文件。
    // 提交点校验见 `distill_job`（批量/自动/手动三条触发路径都汇到那里）。
    let generation = state.vault_generation();

    // 阶段 1：门槛（不跨 await 持锁）。
    let (existing, title) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        // 资料 ∪ 记录（PRD-06-054）。判据是 `NoteStage::allows_distill` 那一处，
        // 三道门（入队前置 / worker 门槛 / 预览）与输入装配问的是同一个函数。
        if !doc.meta.stage().allows_distill() {
            return Err(coded_error(app, "DISTILL_INELIGIBLE", "distill-ineligible"));
        }
        (
            vault.distill_state(note_rel).map_err(vault_err)?,
            luna_vault::title_of(note_rel),
        )
    };

    // 「用户编辑过」是硬闸门：队列路径**一律止步**（ADR-024）。想覆盖只有一条路——
    // 显式重新整理经差异预览（PRD-06-045），那条路不走队列。
    if let DistillState::Present { edited: true, .. } = existing {
        return Err(coded_error(app, "DISTILL_EDITED", "distill-edited"));
    }

    let input = distill_input(&state, app, note_rel)
        .ok_or_else(|| coded_error(app, "DISTILL_EMPTY_SOURCE", "distill-empty-source"))?;

    // 阶段 1b：模板解析（唯一判定点）与落点。**顺序不可颠倒**——落点的文件名后缀
    // 随模板而变，故必须先定模板；而模板的建议判据来自 `input.shape`（说话人数）。
    let templates = distill_templates(app, &state);
    let resolution = resolve_template(
        explicit_template.as_deref(),
        explicit_full_text,
        Some(&existing),
        input.shape,
        templates.set(),
    );
    let template = resolved_ref(&resolution, &templates);
    let full_text = resolution.full_text;
    let target_rel = match &existing {
        DistillState::Present { path, .. } => path.clone(),
        DistillState::Absent => {
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
            unique_distill_path(vault, &dir, &title, template)
        }
    };

    // 阶段 2：单一 in-flight（同一笔记并发只留一个，与 AI 标签同一防重心智）。
    {
        let mut inflight = state
            .distill_inflight
            .lock()
            .map_err(|_| "蒸馏锁中毒".to_string())?;
        if !inflight.insert(note_rel.to_owned()) {
            return Err(coded_error(app, "DISTILL_BUSY", "distill-busy"));
        }
    }
    let outcome = distill_job(
        &state,
        app,
        note_rel,
        &target_rel,
        &title,
        template,
        full_text,
        input,
        generation,
        reporter,
    )
    .await;
    if let Ok(mut inflight) = state.distill_inflight.lock() {
        inflight.remove(note_rel);
    }
    outcome?;

    let source_material = is_source_material(&state, note_rel);
    current_distill_state(&state, note_rel, source_material)
}

/// 读一篇源笔记此刻的蒸馏状态并投影为 DTO（生成 / 应用预览 / 查询三处共用，DRY）。
fn current_distill_state(
    state: &AppState,
    note_rel: &str,
    source_material: bool,
) -> Result<DistillStateDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(match vault.distill_state(note_rel).map_err(vault_err)? {
        DistillState::Absent => DistillStateDto::absent(true, source_material),
        DistillState::Present {
            path,
            stale,
            edited,
            output_truncated,
            template,
            ..
        } => DistillStateDto {
            eligible: true,
            source_material,
            note_path: Some(path),
            stale,
            edited,
            output_truncated: output_truncated.map(|by| by.key().to_owned()),
            template,
        },
    })
}

/// 一篇笔记是不是资料（来源轴）——`current_distill_state` 的三个调用点里有两个只拿得到
/// 路径。读一次 frontmatter 就够，且**读不到时如实报 false**：把未知当成"是资料"会让
/// 「资料」徽标凭空出现在一篇读不出来的笔记上。
fn is_source_material(state: &AppState, note_rel: &str) -> bool {
    let Ok(guard) = state.vault.lock() else {
        return false;
    };
    guard
        .as_ref()
        .and_then(|vault| vault.read(note_rel).ok())
        .is_some_and(|doc| doc.meta.note_type.is_source_material())
}

/// 生成选项（模板选单的数据源，PRD-06-051）。
///
/// **只在浮层展开时调用**：建议模板要读边车段落数说话人，而 [`distill_status`] 挂在
/// `vault://changed` 这条高频路径上（且不带路径过滤）——把这份计算搭在那趟车上，等于
/// 把一次文件 IO 焊死在每一次任意笔记写盘上。
#[tauri::command]
#[specta::specta]
pub(crate) fn distill_template_options(
    app: tauri::AppHandle,
    note_rel: String,
) -> Result<DistillOptionsDto, String> {
    let state = app.state::<AppState>();
    let existing = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.distill_state(&note_rel).map_err(vault_err)?
    };
    let input = distill_input(&state, &app, &note_rel)
        .ok_or_else(|| coded_error(&app, "DISTILL_EMPTY_SOURCE", "distill-empty-source"))?;
    // 与真正生成时**同一个** `resolve_template`：浮层预选的就是不点任何东西直接生成会用的
    // 那一套，两处各算一次必然漂移。
    let templates = distill_templates(&app, &state);
    let resolution = resolve_template(None, None, Some(&existing), input.shape, templates.set());
    Ok(DistillOptionsDto {
        templates: template_choices(&templates),
        suggested: resolution.suggested.key().to_owned(),
        reason: resolution.reason.key().to_owned(),
        template: resolution.key,
        full_text: resolution.full_text,
    })
}

#[allow(clippy::too_many_arguments)]
async fn distill_job(
    state: &AppState,
    app: &tauri::AppHandle,
    note_rel: &str,
    target_rel: &str,
    title: &str,
    template: TemplateRef<'_>,
    full_text: bool,
    input: DistillInput,
    generation: u64,
    reporter: &DistillReporter<'_>,
) -> Result<(), String> {
    let built = build_distill_body(
        state, app, note_rel, target_rel, title, template, full_text, input, reporter,
    )
    .await?;
    let final_body = built.text;
    // 落盘那一刻校验库代际（ADR-025 / TDD-06 §12）：生成期间切过库即放弃——
    // 用户的动作是切库，那才是他要的结果；产物不重跑也不写进新库。
    if !state.still_current_vault(generation) {
        return Err(coded_error(
            app,
            "DISTILL_VAULT_SWITCHED",
            "distill-vault-switched",
        ));
    }
    let intent =
        super::knowledge::prepare_knowledge_write(app, target_rel, &final_body, "distill", false)?;
    let write_result = (|| -> Result<(), String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .write_distilled(
                note_rel,
                target_rel,
                &final_body,
                &template.key(),
                full_text,
                built.truncated.map(luna_core::TruncatedBy::key),
            )
            .map_err(vault_err)
    })();
    if let Err(error) = write_result {
        super::knowledge::abandon_knowledge_write(app, intent);
        return Err(error);
    }
    emit_vault_changed(app, target_rel, false);
    emit_vault_changed(app, note_rel, false);
    super::knowledge::commit_knowledge_write(app, intent)?;
    Ok(())
}

/// 生成蒸馏正文（**只生成、不写盘**）。
///
/// 与写盘分开，是为了让「重生成前的差异预览」（PRD-06-045）复用同一条生成路径：
/// 预览与最终落盘必须是**同一份内容**，各跑一次 LLM 会让预览变成谎报（模型有随机性）。
#[allow(clippy::too_many_arguments)]
async fn build_distill_body(
    state: &AppState,
    app: &tauri::AppHandle,
    note_rel: &str,
    target_rel: &str,
    title: &str,
    template: TemplateRef<'_>,
    full_text: bool,
    input: DistillInput,
    reporter: &DistillReporter<'_>,
) -> Result<Generated, String> {
    let settings = state.settings_snapshot();
    let ctx = lang_context_from_settings(&settings);
    let lang = floating_summary_lang(&settings, &ctx);

    // 覆盖边界只算一次，摘要与整理稿共用（末尾那句「覆盖了前一部分」才对得上两者）。
    let budget = distill_budget(state).await;
    let covered = cover(&input.text, budget);
    // 取一次文案来源往下传：分片摘要那条链只持 `&AppState`（见 `BackendCopy`）。
    let i18n = backend_copy_arc(app);
    let mut body = summarize_long(
        state,
        i18n.as_deref(),
        &covered.chunks,
        luna_core::compose_chunk_instruction(luna_core::prompt::registry(), &lang),
        luna_core::compose_instruction(
            luna_core::prompt::registry(),
            template,
            input.shape,
            input.has_speakers,
            &lang,
            &input.facts,
        ),
        input.origin,
        budget,
        reporter,
    )
    .await?;

    // 整理稿排在摘要之后、来源之前：分层阅读的第三档（概要 → 要点 → 整理稿 → 原始资料）。
    if full_text {
        let rewritten = rewrite_full_text(
            state,
            i18n.as_deref(),
            &covered.text,
            &lang,
            input.origin,
            budget,
            reporter,
        )
        .await?;
        body.absorb(&rewritten);
        if !rewritten.text.trim().is_empty() {
            let heading = if lang.as_str().to_ascii_lowercase().starts_with("zh") {
                "整理稿"
            } else {
                "Cleaned-up transcript"
            };
            body.text
                .push_str(&format!("\n\n## {heading}\n\n{}\n", rewritten.text));
        }
    }

    let truncated = body.truncated;
    let mut final_body = body.text;

    // 溯源区由 Rust 拼接而非交给 LLM：链接必须准确，不能靠模型复述路径。
    // 自动生成的链接用标准相对 md 链接（PRD-06-016 可移植性口径）。
    final_body.push_str("\n\n## 来源\n\n");
    final_body.push_str(&format!(
        "- [{title}]({})\n",
        encode_md_link(&relative_link(target_rel, note_rel))
    ));
    if let Some(url) = source_url_of(state, note_rel) {
        final_body.push_str(&format!("- <{url}>\n"));
    }
    if covered.dropped {
        final_body.push_str("\n> 注：源内容较长，本次整理覆盖了前一部分，末尾未纳入。\n");
    }
    // **截断标注写进正文**，与上面那条覆盖边界注同形（先例就在上一行）：产物是一篇
    // 写进 Vault 的普通 Markdown，用户会在 Obsidian 里读它、导出它、同步到另一台设备。
    // 只在 DOM 上标注的话，那些场景里它与一篇完整笔记**一模一样**。
    //
    // 与覆盖边界注**分成两句**：一句说"源太长、后半没读"，一句说"读了但话没说完"，
    // 两者可以同时成立，合成一句会让用户分不清缺的是输入还是输出。
    //
    // 不按归属分叉措辞：三种触顶对**这一篇笔记的读者**是同一件事（这里缺了一截），
    // 归属的价值在于下一步该找谁，那属于界面上的动作，不属于三个月后重读时的正文。
    //
    // 但正文也**不许开动作**：它此前写着「可在源资料的『生成知识笔记』处重新整理」，
    // 而 `caller` 档的上限是编译期常量、分片确定性——照着做必然在同一片撞同一道闸。
    // 一句不分归属的正文说不出该做什么，那就别说；下一步交给认得归属的徽标。
    if truncated.is_some() {
        final_body.push_str("\n> ");
        final_body.push_str(&backend_text(
            app,
            "distill-output-truncated-note",
            "注：本次整理在生成中途达到了单次输出上限，内容不完整；下一步该做什么，见源资料页头的知识笔记徽标。",
        ));
        final_body.push('\n');
    }
    Ok(Generated {
        text: final_body,
        truncated,
    })
}

/// 蒸馏笔记的落点：`<notes_dir>/<源标题> · <模板后缀>.md`，重名追加序号。
///
/// 后缀随模板（会议 → 「纪要」，其余 → 「笔记」），兑现 PRD-06-044 写下却一直未落地的
/// 两种形态——此前连 AI 对话蒸馏出来的也叫「纪要」。
fn unique_distill_path(vault: &Vault, dir: &str, title: &str, template: TemplateRef<'_>) -> String {
    unique_note_path_in(vault, dir, &format!("{title} · {}", template.name_suffix()))
}

/// 在 `dir` 下找一个没被占用的 `<base>.md`（重名追加序号）。
///
/// 只**找**路径、不建文件——实体由各自的写入点落定（`write_distilled` 还要同时写
/// 指针与指纹，先建一次再写一次等于留下一篇没有指针的空壳）。
///
/// **占用判据是「文件系统里有没有这个条目」，不是「读不读得出来」**（`path_is_free`）。
/// 此前用的是 `vault.read(&candidate).is_err()`：一篇权限不足、或正撞上瞬时 IO 错误的既有
/// 笔记会读失败，于是被当成"这里是空的"，下一步就写在用户已有的资产上。这条判据错得很
/// 安静——正常情况下它与真实存在性完全一致，只在出事那一刻不一致。
pub(crate) fn unique_note_path_in(vault: &Vault, dir: &str, base: &str) -> String {
    for index in 0..100 {
        let name = if index == 0 {
            format!("{base}.md")
        } else {
            format!("{base}-{index}.md")
        };
        let candidate = if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        };
        if vault.path_is_free(&candidate) {
            return candidate;
        }
    }
    format!("{dir}/{base}.md")
}

fn source_url_of(state: &AppState, note_rel: &str) -> Option<String> {
    let guard = state.vault.lock().ok()?;
    let vault = guard.as_ref()?;
    vault.read(note_rel).ok()?.meta.source_url
}

/// 由 `from` 指向 `to` 的 Vault 内相对链接（两者都是 Vault 相对路径）。
fn relative_link(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let to_dirs: Vec<&str> = to.split('/').collect();
    let from_dir = &from_dirs[..from_dirs.len().saturating_sub(1)];
    let to_dir = &to_dirs[..to_dirs.len().saturating_sub(1)];
    let common = from_dir
        .iter()
        .zip(to_dir.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), from_dir.len() - common)
        .chain(to_dir[common..].iter().map(|s| (*s).to_owned()))
        .collect();
    parts.push(to_dirs.last().copied().unwrap_or(to).to_owned());
    parts.join("/")
}

/// Markdown 链接目标里的空格与括号需转义，否则链接在渲染时断掉。
fn encode_md_link(path: &str) -> String {
    path.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

// ── 重生成前的差异预览（PRD-06-045）─────────────────────────────────────────
//
// 「绝不覆盖用户资产」在 P0 只做到**拦住自动路径**：用户显式点重新生成时仍是盲覆盖，
// 他自己补写的那几段会不会丢，只能覆盖之后翻版本历史才知道。预览补齐这最后一环——
// **先看会改掉什么，再决定要不要改**。
//
// 预览与落盘必须是**同一份内容**：故生成结果暂存，接受时直接写，绝不重跑 LLM
// （模型有随机性，各跑一次会让预览变成谎报——那比没有预览更糟）。

/// 预览暂存槽（**单槽**）：最近一次预览的产物。
///
/// 单槽而非按笔记键控的表：预览要跑一次 LLM、是个明确的模态操作，同时预览两篇不是
/// 真实场景；单槽天然自清理（下次预览即顶替），不需要额外的"丢弃"命令与其生命周期。
pub(crate) struct DistillPreviewSlot {
    /// 源资料笔记相对路径（接受时校验，防止槽与当前笔记错配）。
    pub note_rel: String,
    /// 生成时源正文的指纹：重开预览的**槽命中**以此校验——源又被编辑过的话，
    /// 暂存产物已经过时，复用它等于给用户看一份按旧源算出来的差异（谎报）。
    pub source_hash: String,
    /// 蒸馏笔记落点。
    pub target_rel: String,
    /// 已生成、待落盘的正文。
    pub body: String,
    /// 产物所用的模板键。**纳入槽命中判据**：换了模板还复用旧产物算差异是谎报
    /// （与源指纹校验同构）。
    pub template: String,
    /// 产物是否含整理稿。同样纳入命中判据。
    pub full_text: bool,
    /// 本次生成的触顶归属（[`luna_core::TruncatedBy::key`]），`None` = 没触顶。
    ///
    /// **必须随槽一起暂存**：接受预览走的是 `distill_apply`，它**不重跑 LLM**——
    /// 这个事实只在生成那一刻存在过，槽里不带它的话，经预览落盘的产物就永远是
    /// 「完整」的，而经队列落盘的同一份内容却带着标注。同一件事两条路两个答案。
    pub output_truncated: Option<String>,
}

/// 预览结果：差异 + 落点。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillPreviewDto {
    /// 蒸馏笔记路径（尚未写入的新笔记也给——用户要知道会落在哪儿）。
    pub target_path: String,
    /// 当前内容 → 重生成后内容的逐行差异（复用版本对比同一呈现，DRY）。
    pub diff: LineDiffDto,
}

/// 生成一次预览：跑完整条蒸馏管线但**不写盘**，返回与当前内容的差异。
#[tauri::command]
#[specta::specta]
pub(crate) async fn distill_preview(
    app: tauri::AppHandle,
    note_rel: String,
    template: Option<String>,
    full_text: Option<bool>,
) -> Result<DistillPreviewDto, String> {
    let state = app.state::<AppState>();
    let explicit_template = template;
    // 发起时的库代际（ADR-025）：产物**回填暂存槽**同样是异步写回——切库会清空槽，
    // 但迟到的生成结果若不校验就回填，会把旧库的产物塞进新库语境的槽里，
    // `distill_apply`「槽必属当前库」的不变量随之失效。
    let generation = state.vault_generation();

    let (existing, title, source_hash) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        // 资料 ∪ 记录（PRD-06-054）。判据是 `NoteStage::allows_distill` 那一处，
        // 三道门（入队前置 / worker 门槛 / 预览）与输入装配问的是同一个函数。
        if !doc.meta.stage().allows_distill() {
            return Err(coded_error(
                &app,
                "DISTILL_INELIGIBLE",
                "distill-ineligible",
            ));
        }
        let source_hash = luna_vault::ContentHash::of(luna_vault::body_of(&doc.content).as_bytes())
            .as_str()
            .to_owned();
        (
            vault.distill_state(&note_rel).map_err(vault_err)?,
            luna_vault::title_of(&note_rel),
            source_hash,
        )
    };

    // 输入与模板解析先行（都不跑 LLM）：槽命中判据要拿本次**实际会用**的模板与整理稿
    // 开关去比，而它们由 `resolve_template` 定夺——先查槽再解析的话，换了模板的那次预览
    // 会命中旧模板的产物。
    let input = distill_input(&state, &app, &note_rel)
        .ok_or_else(|| coded_error(&app, "DISTILL_EMPTY_SOURCE", "distill-empty-source"))?;
    let templates = distill_templates(&app, &state);
    let resolution = resolve_template(
        explicit_template.as_deref(),
        full_text,
        Some(&existing),
        input.shape,
        templates.set(),
    );
    let template = resolved_ref(&resolution, &templates);
    let want_full_text = resolution.full_text;

    // 槽命中：同一篇的暂存产物还在（用户关掉对话框又重开）、**源没再变**、且**模板与
    // 整理稿开关一致** → 直接用它算差异，不重跑 LLM。任一不符即弃槽重生成：复用等于
    // 给用户看一份按旧参数算出来的差异（谎报）。
    // 差异恒对着**此刻**蒸馏笔记的磁盘内容重算——上次预览后用户可能又编辑过它。
    {
        let slot = state
            .distill_preview
            .lock()
            .map_err(|_| "蒸馏预览锁中毒".to_string())?;
        if let Some(staged) = slot.as_ref().filter(|s| {
            s.note_rel == note_rel
                && s.source_hash == source_hash
                && s.template == resolution.key
                && s.full_text == want_full_text
        }) {
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            let current = vault
                .read(&staged.target_rel)
                .map(|d| luna_vault::body_of(&d.content).to_owned())
                .unwrap_or_default();
            return Ok(DistillPreviewDto {
                target_path: staged.target_rel.clone(),
                diff: luna_vault::diff_lines(&current, &staged.body).into(),
            });
        }
    }

    let (target_rel, current) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        match &existing {
            DistillState::Present { path, .. } => {
                // 对比基准是**此刻磁盘上的内容**（含用户的编辑），不是上次生成时的样子——
                // 用户要回答的问题是"重新生成会改掉我现在看到的什么"（同 PRD-06-041 口径）。
                let body = vault
                    .read(path)
                    .map(|d| luna_vault::body_of(&d.content).to_owned())
                    .unwrap_or_default();
                (path.clone(), body)
            }
            DistillState::Absent => {
                let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
                (
                    unique_distill_path(vault, &dir, &title, template),
                    String::new(),
                )
            }
        }
    };
    {
        let mut inflight = state
            .distill_inflight
            .lock()
            .map_err(|_| "蒸馏锁中毒".to_string())?;
        if !inflight.insert(note_rel.clone()) {
            return Err(coded_error(&app, "DISTILL_BUSY", "distill-busy"));
        }
    }
    let built = build_distill_body(
        &state,
        &app,
        &note_rel,
        &target_rel,
        &title,
        template,
        want_full_text,
        input,
        // 预览是用户正看着的模态动作，进度由对话框自己表达；它也不该被队列取消。
        &DistillReporter::default(),
    )
    .await;
    if let Ok(mut inflight) = state.distill_inflight.lock() {
        inflight.remove(&note_rel);
    }
    let built = built?;
    if !state.still_current_vault(generation) {
        return Err(coded_error(
            &app,
            "DISTILL_VAULT_SWITCHED",
            "distill-vault-switched",
        ));
    }

    let diff = luna_vault::diff_lines(&current, &built.text);
    {
        let mut slot = state
            .distill_preview
            .lock()
            .map_err(|_| "蒸馏预览锁中毒".to_string())?;
        *slot = Some(DistillPreviewSlot {
            note_rel: note_rel.clone(),
            source_hash,
            target_rel: target_rel.clone(),
            body: built.text,
            template: resolution.key.clone(),
            full_text: want_full_text,
            output_truncated: built.truncated.map(|by| by.key().to_owned()),
        });
    }
    Ok(DistillPreviewDto {
        target_path: target_rel,
        diff: diff.into(),
    })
}

/// 接受预览：把暂存的正文落盘。**不重跑 LLM**——用户确认的就是他刚看过的那一份。
#[tauri::command]
#[specta::specta]
pub(crate) fn distill_apply(
    app: tauri::AppHandle,
    note_rel: String,
) -> Result<DistillStateDto, String> {
    let state = app.state::<AppState>();
    let staged = {
        let mut slot = state
            .distill_preview
            .lock()
            .map_err(|_| "蒸馏预览锁中毒".to_string())?;
        // 切库会清空预览槽（TDD-06 §12：槽产物与库绑定），故走到这里的槽必属当前库。
        match slot.take() {
            // 槽与当前笔记不符（预览过别的笔记）：不猜，让用户重新预览一次。
            Some(s) if s.note_rel == note_rel => s,
            other => {
                *slot = other;
                return Err(coded_error(
                    &app,
                    "DISTILL_NO_PREVIEW",
                    "distill-no-preview",
                ));
            }
        }
    };
    let intent = super::knowledge::prepare_knowledge_write(
        &app,
        &staged.target_rel,
        &staged.body,
        "distill",
        false,
    )?;
    let write_result = (|| -> Result<(), String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 写盘经 `Vault::write`，覆盖前照例自动留快照（PRD-06-040）——预览之外的第二重保险。
        // 写失败把产物**放回槽**：那是一次真实的 LLM 产出，磁盘满这类环境问题解决后
        // 用户重试不该被迫重跑一遍生成。
        vault
            .write_distilled(
                &note_rel,
                &staged.target_rel,
                &staged.body,
                &staged.template,
                staged.full_text,
                staged.output_truncated.as_deref(),
            )
            .map_err(vault_err)
    })();
    if let Err(error) = write_result {
        super::knowledge::abandon_knowledge_write(&app, intent);
        if let Ok(mut slot) = state.distill_preview.lock() {
            *slot = Some(staged);
        }
        return Err(error);
    }
    emit_vault_changed(&app, &staged.target_rel, false);
    emit_vault_changed(&app, &note_rel, false);
    super::knowledge::commit_knowledge_write(&app, intent)?;
    let source_material = is_source_material(&state, &note_rel);
    current_distill_state(&state, &note_rel, source_material)
}

// ── 用户自定义模板（PRD-06-051 批次 2）──────────────────────────────────────
//
// 存储随库（`.luna/distill_templates.json`）、校验在 luna-core（纯函数）、id 由 Rust 生成。
// 前端只呈现与收集输入，不认识"什么样的模板算合法"（ADR-003）。

/// 可选模板清单（内置 < 模板包 < 自定义，与 ADR-169 ⑤ 的分层同序——内置是"总在那儿"的稳定锚点，
/// 用户自己加的排在最后，与设置里"内置预设 + 自定义"的既有呈现顺序一致）。
/// 单篇浮层与批量确认卡共用（两处各拼一份必然漂移）。
fn template_choices(templates: &DistillTemplates) -> Vec<DistillTemplateChoiceDto> {
    TemplateId::ALL
        .into_iter()
        .map(|id| DistillTemplateChoiceDto {
            id: id.key().to_owned(),
            name: None,
            full_text_default: id.full_text_default(),
            pack_name: None,
        })
        .chain(
            templates
                .packs
                .iter()
                .zip(&templates.pack_names)
                .map(|(pack, pack_name)| DistillTemplateChoiceDto {
                    id: TemplateRef::Pack(pack).key(),
                    name: Some(pack.template.name.clone()),
                    full_text_default: pack.template.full_text_default,
                    pack_name: Some(pack_name.clone()),
                }),
        )
        .chain(
            templates
                .user
                .iter()
                .map(|template| DistillTemplateChoiceDto {
                    id: TemplateRef::User(template).key(),
                    // 自定义模板的名字是**用户写的**，不进 i18n（那是他的措辞，不是产品文案）。
                    name: Some(template.name.clone()),
                    full_text_default: template.full_text_default,
                    pack_name: None,
                }),
        )
        .collect()
}

/// 模板清单（**无单篇语境**的取数口，批量确认卡用）。
///
/// 与 [`distill_template_options`] 分开：那条要读边车数说话人给建议，而批量没有
/// "这一篇"可言——硬共用会让批量场景多付一次没人消费的边车 IO。
#[tauri::command]
#[specta::specta]
pub(crate) fn distill_template_choices(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DistillTemplateChoiceDto>, String> {
    Ok(template_choices(&distill_templates(&app, &state)))
}

/// 用户模板的 IPC 镜像。
///
/// **不给 `luna_core::UserTemplate` 直接派生 `specta::Type`**：luna-core 是零 IO 的领域层
/// （TDD-00 §3 单向依赖），把绑定层的关注点塞进去
/// 会让领域模型开始为前端形状负责。镜像 + 双向转换是既有惯例（同 `ProcTaskDto` 等）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct UserTemplateDto {
    /// 稳定 id；新建时传空串（由 Rust 生成）。
    pub id: String,
    /// 展示名（用户自写）。
    pub name: String,
    /// 输出区块（顺序即产出顺序）。
    pub sections: Vec<UserSectionDto>,
    /// 本模板默认是否附整理稿。
    pub full_text_default: bool,
}

/// 用户模板的一个区块。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct UserSectionDto {
    /// 小节标题（原样输出）。
    pub heading: String,
    /// 给模型的指导语（可空）。
    pub guidance: String,
}

impl From<luna_core::UserTemplate> for UserTemplateDto {
    fn from(value: luna_core::UserTemplate) -> Self {
        Self {
            id: value.id,
            name: value.name,
            sections: value
                .sections
                .into_iter()
                .map(|section| UserSectionDto {
                    heading: section.heading,
                    guidance: section.guidance,
                })
                .collect(),
            full_text_default: value.full_text_default,
        }
    }
}

impl From<UserTemplateDto> for luna_core::UserTemplate {
    fn from(value: UserTemplateDto) -> Self {
        Self {
            id: value.id,
            name: value.name,
            sections: value
                .sections
                .into_iter()
                .map(|section| luna_core::UserSection {
                    heading: section.heading,
                    guidance: section.guidance,
                })
                .collect(),
            full_text_default: value.full_text_default,
        }
    }
}

/// 列出当前库的用户模板（管理对话框的数据源）。
#[tauri::command]
#[specta::specta]
pub(crate) fn distill_user_templates(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<UserTemplateDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .distill_templates()
        .map_err(vault_err)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 新建或更新一个用户模板；返回更新后的完整清单（前端就地刷新，免二次读取）。
///
/// `id` 为空 = 新建（Rust 生成稳定 id）；否则更新同 id 那条。**id 由 Rust 生成而非前端**：
/// 它会被写进蒸馏笔记的 frontmatter 作长期指针，生成规则必须只有一处。
#[tauri::command]
#[specta::specta]
pub(crate) fn save_distill_template(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    template: UserTemplateDto,
) -> Result<Vec<UserTemplateDto>, String> {
    // 规范化 → 校验：存进去的就是将来喂给模型的那一份（`## 概要` 这类 Markdown 习惯
    // 在此被纠正而非报错）。
    let mut template = luna_core::normalize_template(template.into());
    luna_core::validate_distill_template(&template)
        .map_err(|err| format!("DISTILL_TEMPLATE_INVALID: {}", err.key()))?;

    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let mut templates = vault.distill_templates().map_err(vault_err)?;

    if template.id.trim().is_empty() {
        if templates.len() >= luna_core::MAX_USER_TEMPLATES {
            return Err(coded_error(
                &app,
                "DISTILL_TEMPLATE_LIMIT",
                "distill-template-limit",
            ));
        }
        template.id = new_template_id();
        templates.push(template);
    } else if let Some(slot) = templates.iter_mut().find(|item| item.id == template.id) {
        *slot = template;
    } else {
        // id 指向一条已不存在的模板（另一端删了它）：当作新建收下而不是报错——
        // 用户刚写完的内容不该因为一次时序意外就丢掉。
        template.id = new_template_id();
        templates.push(template);
    }
    vault.set_distill_templates(&templates).map_err(vault_err)?;
    Ok(templates.into_iter().map(Into::into).collect())
}

/// 删除一个用户模板；返回更新后的清单。
///
/// **不去改既有蒸馏笔记的 `distill_template`**：那些笔记记录的是"当时用什么整理的"，
/// 是历史事实；删模板只影响将来（重生成时回退建议规则，见 `luna_core::resolve`）。
#[tauri::command]
#[specta::specta]
pub(crate) fn delete_distill_template(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<UserTemplateDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let mut templates = vault.distill_templates().map_err(vault_err)?;
    templates.retain(|item| item.id != id);
    vault.set_distill_templates(&templates).map_err(vault_err)?;
    Ok(templates.into_iter().map(Into::into).collect())
}

/// 生成一个模板 id（时间无关的随机短串——它只需唯一且稳定，不承载语义）。
fn new_template_id() -> String {
    // 复用 UUIDv7 的实现（仓库已有依赖）：取其去连字符形式的前 12 位足够唯一，
    // 短一点是为了让 frontmatter 里的 `user:xxxx` 仍然可读。
    luna_core::JobId::new()
        .to_string()
        .replace('-', "")
        .chars()
        .take(12)
        .collect()
}

// ── 批量整理（PRD-06-044 批量入口 / PRD-06-051 修订）────────────────────────
//
// 预检盘点（四桶）→ 确认卡 → **批量入队**。执行、进度、取消、重试、断点续跑全部由统一
// 处理队列（PRD-04-055）提供——初版在这里自建的那套「单例 + 进度事件 + 快照命令 + 连败
// 中止 + 终态通知」与队列做的是同一件事，且两套并存会让批量 runner 与队列 worker 同时
// 各跑一件蒸馏、抢同一个本地引擎。

/// 批量预检 DTO：确认卡据此在**点开始之前**陈述代价。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillPreflightDto {
    /// 待整理篇数（从未生成 + 源已更新且未被编辑）。
    pub pending: u32,
    /// 已最新、跳过的篇数。
    pub fresh: u32,
    /// 蒸馏笔记已被编辑、跳过的篇数——**批量绝不覆盖用户资产**，这个数字要让用户看见。
    pub edited: u32,
    /// 正文为空、无从整理的篇数。
    pub empty: u32,
    /// 队列里是否已有蒸馏任务在排/在跑（确认卡据此提示"还有 N 件在队列里"）。
    pub running: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn distill_preflight(
    state: tauri::State<'_, AppState>,
    scope: Option<String>,
) -> Result<DistillPreflightDto, String> {
    let scan = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.distill_scan(scope.as_deref()).map_err(vault_err)?
    };
    // 「在跑」的真值现在只有一处：统一处理队列（本模块不再自持运行态）。
    let running = state.processing.has_active_kind("distill");
    Ok(DistillPreflightDto {
        pending: u32::try_from(scan.pending.len()).unwrap_or(u32::MAX),
        fresh: scan.fresh,
        edited: scan.edited,
        empty: scan.empty,
        running,
    })
}

/// 批量里**被挡下的一篇**：是哪一篇、为什么。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillBatchRejectionDto {
    /// 源笔记相对路径——界面据它渲染一行可点的条目。
    pub note_rel: String,
    /// 拒绝理由：稳定机器前缀 + 本地化说明（与单篇路径同一个串）。
    ///
    /// **理由必须逐篇带出来**：这里此前是 `if enqueue_distill(..) == Ok(true)`，
    /// `Err(理由)` 被整个丢进 `==` 的假分支里——50 篇里挡下 47 篇，用户拿回的是
    /// 一句「已加入 3 件」，另外 47 篇既没入队也没有任何交代。
    pub reason: String,
}

/// 批量入队的结果：进了几件、哪些没进、为什么。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct DistillBatchStartDto {
    /// 真入队篇数（幂等去重不计，「已加入 N 件」不虚报）。
    pub queued: u32,
    /// 被挡下的篇目，顺序即扫描顺序。
    pub rejected: Vec<DistillBatchRejectionDto>,
}

/// 把范围内所有待整理的资料**批量排进统一处理队列**（PRD-06-051 修订）。
///
/// **不再自建 runner**：初版为批量写了一整套「单例 + 进度事件 + 快照命令 + 连败中止 +
/// 终态通知」，与统一处理队列（PRD-04-055）做的是同一件事。两套并存还有实质害处——批量
/// runner 与队列 worker 会**同时**各跑一件蒸馏，两个分钟级 LLM 任务抢同一个本地引擎。
/// 现在批量 = 「一次入队 N 件」，顺序执行 / 逐件进度 / 单件取消 / 失败重试 / 断点续跑
/// 全部由队列提供，用户看到的也是他熟悉的那一个任务面板。
#[tauri::command]
#[specta::specta]
pub(crate) async fn distill_batch_start(
    app: tauri::AppHandle,
    scope: Option<String>,
    template: Option<String>,
    full_text: Option<bool>,
) -> Result<DistillBatchStartDto, String> {
    let state = app.state::<AppState>();
    let targets = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .distill_scan(scope.as_deref())
            .map_err(vault_err)?
            .pending
    };
    if targets.is_empty() {
        return Err(coded_error(
            &app,
            "DISTILL_BATCH_EMPTY",
            "distill-batch-empty",
        ));
    }
    Ok(enqueue_distill_all(&app, targets, template, full_text).await)
}

/// 把一组目标逐篇排进统一处理队列，并如实交代进了几件、哪些没进、为什么。
///
/// **范围扫描与逐条勾选共用这一个循环**：文件夹批量（`distill_batch_start`）与多选
/// 批量（`distill_notes`）的差别只在"这一批是谁"，而余量账本、幂等去重与逐篇拒绝理由
/// 三件事在两条路上必须逐字一致——各写一遍的表现是其中一条悄悄退回"报个总数就完事"。
async fn enqueue_distill_all(
    app: &tauri::AppHandle,
    targets: Vec<String>,
    template: Option<String>,
    full_text: Option<bool>,
) -> DistillBatchStartDto {
    // 整批只取一次余量：几十篇各发一趟 `/v1/usage` 既慢，答案也不会有意义地不同。
    //
    // **一份余量要累减**（[`QuotaLedger`]）：逐篇各自与同一个余量比对时，余量 100k、
    // 每篇需 50k 的 50 篇会全部通过预检，而只有前两篇跑得完——「跑到第 K 片才发现钱不够」
    // 这件事被从单篇里赶走之后，在这里原样复活。此前不做累减的理由是"会让第 K 篇之后的
    // 全部静默落空"，那个理由**只对『不交代理由』的实现成立**，而当时的实现恰恰把
    // `Err` 丢掉了——注释在同一个函数里被自己推翻。现在两件事一起做：累减 + 逐篇交代。
    let mut ledger = QuotaLedger::new(cloud_output_headroom(app).await);
    let mut queued = 0u32;
    let mut rejected = Vec::new();
    for rel in targets {
        // 模板参数（PRD-06-051 修订，真机诉求「批量也要能选模板」）：
        // - `None`（确认卡默认「自动」）：由 `resolve_template` 走「已记录 > 建议」——
        //   待办桶里**包含**重生成件，按规则重判会把用户手选过的模板静默换掉；
        // - `Some(模板)`：**整批统一**按用户这次显式选的来（显式 > 已记录，正是解析
        //   顺序的既有语义——文件夹常是同质内容的聚合，"这个文件夹全是讲座"是文件夹
        //   分类价值的自然延伸）。仍不做**逐篇**覆写与逐篇分布展示（原裁决不变）。
        //
        // 单篇入队失败（如刚被删/刚被编辑/余量装不下）不中断整批：那是这一篇的事，
        // 不是这一批的事——但它必须被**说出来**，而不是消失在一个布尔比较里。
        match enqueue_distill(app, rel.clone(), template.clone(), full_text, &mut ledger) {
            Ok(true) => queued += 1,
            // 幂等去重：这一篇已经在队列里了，既不是新入队也不是被拒。
            Ok(false) => {}
            Err(reason) => rejected.push(DistillBatchRejectionDto {
                note_rel: rel,
                reason,
            }),
        }
    }
    // 整批都进了才发「已加入队列」toast；有被挡下的篇目时那句话不够用——前端按返回值
    // 渲染逐篇理由卡（PRD-06-051：说得出是哪几篇、为什么），不再叠一条 toast。
    if queued > 0 && rejected.is_empty() {
        notify_batch_queued(app, queued);
    }
    DistillBatchStartDto { queued, rejected }
}

/// 「已加入队列 N 件」回执（PRD-06-051 ⑩ 修订，PRD-15-011）：信息档 toast，按分档时长
/// 自动消散，动作直达处理队列面板。此前是工作区底部一条只能手动关的常驻条，与右上角
/// 队列角标讲同一件事的两种说法（真机 2026-09-07）。参数只有计数，不含笔记名。
pub(crate) fn notify_batch_queued(app: &tauri::AppHandle, count: u32) {
    if let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() {
        nc.publish(
            crate::notifications::NotificationEvent::info_at(
                "distill_batch_queued",
                crate::notifications::NotificationTarget::Queue,
            )
            .with_param("count", count.to_string()),
            app,
        );
    }
}

// ── 速记收件箱（PRD-02-055）────────────────────────────────────────────────
//
// 速记是**用户自己说的话**，不是机器产物——故它不属资料区（照常入图谱与搜索），
// 走的是 Drafts 式的「收件箱 → 处理 → 归档」：碎片先落 `QuickNotes/`，经**显式动作**
// 才成为知识库主体的一部分。两个动作对应两种处理方式：一条值得单独留下就「升级」，
// 一堆零碎属于同一主题就「整理入库」合成一篇。

/// 「升级为笔记」（PRD-02-055）：把一条速记移进 `notes_dir` 并改 `luna.type` 为 `note`。
///
/// 返回新路径。**顺序是先移动、后改类型**：移动后它仍是一条速记（还在速记清单里，
/// 只是换了位置）——这是个说得通的中间态；反过来先改类型再移动失败的话，它会从速记
/// 清单里消失、却仍躺在 `QuickNotes/` 目录下，用户找不着。
#[tauri::command]
#[specta::specta]
pub(crate) fn promote_quick_note(
    app: tauri::AppHandle,
    note_rel: String,
) -> Result<String, String> {
    promote_quick_note_impl(&app, &note_rel)
}

/// 「升级为笔记」的**唯一实现**——速记清单的 [`promote_quick_note`] 与文件树/页头的
/// 「沉淀为知识」（[`super::notes::settle_note_as_knowledge`]）共用它。
///
/// **速记的沉淀就是升级，不另造第二条路**（PRD-06-053）：缺省记录态的速记没有
/// `luna.stage` 键可删，删一行对它毫无作用；它的毕业是「换类型 + 搬进 `notes_dir`」，
/// 而这件事这里已经做对了（含移动失败的回滚与知识写入意图）。两处各写一份的代价是
/// 「从速记清单升级会搬文件、从文件树沉淀不会」——同一个动作两种结果。
pub(crate) fn promote_quick_note_impl(
    app: &tauri::AppHandle,
    note_rel: &str,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    // 观测授权在**动作开始处**取得：下面这一串（读、搬、改类型、提交知识写入意图）跨越
    // 多次写盘，这期间用户完全可以关掉开关或清除历史（TDD-12 §2.1）。
    let settle_grant = super::notes::begin_quick_note_settled(app);
    let (target, body) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        if doc.meta.note_type != NoteType::QuickNote {
            return Err(coded_error(
                app,
                "PROMOTE_NOT_QUICK_NOTE",
                "promote-not-quick-note",
            ));
        }
        let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
        let stem = luna_vault::title_of(note_rel);
        let target = unique_note_path_in(vault, &dir, &stem);
        (target, luna_vault::body_of(&doc.content).to_owned())
    };
    let intent = super::knowledge::prepare_knowledge_write(app, &target, &body, "merge", false)?;
    let write_result = (|| -> Result<(std::path::PathBuf, Vec<String>), String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let relinked = vault
            .move_with_report(note_rel, &target)
            .map_err(vault_err)?
            .rewritten;
        // 类型改写顺带去掉显式 `luna.stage`（`Vault::set_note_type` 一次写盘收两根轴，
        // PRD-06-053）——升级完的那一篇必须真的是知识态。
        if let Err(error) = vault.set_note_type(&target, NoteType::Note) {
            let _ = vault.move_(&target, note_rel);
            return Err(vault_err(error));
        }
        Ok((vault.root().to_path_buf(), relinked))
    })();
    let (root, relinked) = match write_result {
        Ok(done) => done,
        Err(error) => {
            super::knowledge::abandon_knowledge_write(app, intent);
            return Err(error);
        }
    };
    emit_vault_changed(app, note_rel, true);
    emit_vault_changed(app, &target, false);
    super::relocation::broadcast_relinked(app, "main", &root, &relinked);
    super::knowledge::commit_knowledge_write(app, intent)?;
    // 速记升级与「标记为待整理」的笔记转正是**同一件事的两条路**（PRD-06-053）——观测
    // 必须两条都记，否则"转正了多少次"只统计到其中一半，而那个数会被当成全部。
    if let Some(grant) = settle_grant {
        super::notes::commit_quick_note_settled(app, grant);
    }
    Ok(target)
}

/// 「撤销转为正式笔记」——[`promote_quick_note_impl`] 的逆操作（PRD-06-058 验收「可撤销反馈」）。
///
/// **它不是「标记为待整理」**：那条路只加 `luna.stage` 一行，产物是一篇待整理的**普通
/// 笔记**，留在「笔记」空间；而用户撤销的是"把速记转正"这个动作，他要的是那条速记**回到
/// 速记流**。故这里与升级严格对称：搬回 `quick_notes_dir` + 换回 `type: quick-note`
/// （缺省记录态，`Vault::set_note_type` 顺带去掉任何显式 `luna.stage`）。
///
/// 只接受普通知识笔记：资料由类型拒绝（它们从来不是速记），主题页与知识地图是聚合层
/// （ADR-096），把它们"降"成一条速记没有任何意义。判定在 Rust，前端只在撤销条上给一颗按钮。
#[tauri::command]
#[specta::specta]
pub(crate) fn demote_to_quick_note(
    app: tauri::AppHandle,
    note_rel: String,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let target = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        if doc.meta.note_type != NoteType::Note || doc.meta.topic || doc.meta.knowledge_map {
            return Err(coded_error(&app, "DEMOTE_NOT_NOTE", "demote-not-note"));
        }
        let dir = note_type_dir(&state.settings_snapshot(), NoteType::QuickNote);
        let stem = luna_vault::title_of(&note_rel);
        unique_note_path_in(vault, &dir, &stem)
    };
    let (root, relinked) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let relinked = vault
            .move_with_report(&note_rel, &target)
            .map_err(vault_err)?
            .rewritten;
        if let Err(error) = vault.set_note_type(&target, NoteType::QuickNote) {
            let _ = vault.move_(&target, &note_rel);
            return Err(vault_err(error));
        }
        (vault.root().to_path_buf(), relinked)
    };
    emit_vault_changed(&app, &note_rel, true);
    emit_vault_changed(&app, &target, false);
    super::relocation::broadcast_relinked(&app, "main", &root, &relinked);
    Ok(target)
}

/// 「把这几条待整理内容整理成一篇知识笔记」（PRD-06-047 / PRD-06-058 C②）。
///
/// **按真实 paths，不按物理目录**（ADR-087 / TDD-07 §2.1）：入口此前是文件树里
/// `QuickNotes/` 那个文件夹的右键，于是"哪些算速记"由**目录**回答——而速记会被移动、
/// 普通笔记也会被「标记为待整理」反向产生，目录从来不等于成熟度。现在入口在「笔记 →
/// 速记」查询投影里：用户勾选若干条，前端把 Rust 已经判定为待整理的**真实 paths** 交回来。
///
/// **资格仍由 Rust 判**：前端交的是选择，不是判断。每一条都必须此刻仍是记录态——
/// 用户勾选之后、点确认之前把其中一篇沉淀掉是完全可能的，那时把它卷进汇总就是拿一篇
/// 已定稿的知识笔记当草稿用。
///
/// **原笔记留在原地不删**——整理不该是隐式销毁（同 AI 标签排除的精神）：产出的是新增的
/// 一份提炼，要不要清掉源由用户自己决定。
#[tauri::command]
#[specta::specta]
pub(crate) async fn merge_notes(app: tauri::AppHandle, paths: Vec<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    if paths.is_empty() {
        return Err(coded_error(
            &app,
            "MERGE_EMPTY_SELECTION",
            "merge-empty-selection",
        ));
    }
    // **诚实前置**（与 `enqueue_distill` 同一纪律）：注定失败的不入队——点了「已加入
    // 队列」、几秒后面板里冒出一条红的，比当场说"不行、为什么不行"糟糕得多。选择、资格与
    // 条数区间在这里判完；执行时 `run_merge` 会在**它自己的临界区**再取一次输入快照
    // ——排队期间用户完全可能改了、移走了或删掉了其中一篇。
    let notes = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let sources = resolve_selected_sources(&app, vault, &paths)?;
        check_selection_size(&app, sources.len())?;
        sources
            .into_iter()
            .map(|source| source.rel)
            .collect::<Vec<_>>()
    };
    // **入队而非内联**（2026-09-02）：候选集放开到录音与转写之后，"速记都很短、不会
    // 跑很久"这个内联的前提没了；而脱离队列跑会与正在跑的蒸馏同时抢一个本地引擎，且在
    // 任务面板里看不见、取消不了（`ProcTaskKind::Merge` 的说明）。
    state.processing.enqueue(
        ProcTaskKind::Merge {
            notes,
            origin: MergeOrigin::Selection,
        },
        backend_text(&app, "merge-stem", "汇总"),
    );
    // 入队后立即广播 + 拉起 worker（与 `enqueue_distill` 同一纪律与理由）。
    super::transcribe::emit_processing_queue(&app);
    super::transcribe::start_processing_worker_if_idle(&app);
    notify_batch_queued(&app, 1);
    Ok(())
}

/// 把某个标签下的笔记汇总成一篇（PRD-06-047）。
///
/// 与「多选汇总」共用同一内核与同一条队列，差别只在候选集口径：这里按标签取（含子孙
/// 标签，与标签视图的过滤语义同源），那里按用户的选择取。候选在**入队时**定死——用户是看着
/// 「这 N 篇」点的汇总，排队期间标签下又多了笔记不该改写他这一件。
///
/// **刻意不做"周报"这样的专有功能**：周报的时间维度用户已能用文件夹 / 标签 /
/// `path:` 表达，而"一周"对不同人意味着不同的东西；专门做一个日期范围选择器 + 周报
/// 模板，是为某一种工作流定制，与 Lunarca「通用工具」的定位不符。泛化的「汇总这一批」
/// 能覆盖同样的诉求，且不引入第二套候选集机构。
#[tauri::command]
#[specta::specta]
pub(crate) async fn distill_tag_notes(app: tauri::AppHandle, tag: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let notes = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .search(&luna_vault::Query {
                tag: Some(tag.clone()),
                limit: MERGE_NOTE_LIMIT,
                ..luna_vault::Query::default()
            })
            .map_err(vault_err)?
            .into_iter()
            .map(|hit| hit.path)
            .collect::<Vec<_>>()
    };
    if notes.is_empty() {
        return Err(coded_error(&app, "DISTILL_MERGE_EMPTY", "merge-empty"));
    }
    let label = merge_tag_stem(&app, &tag);
    state.processing.enqueue(
        ProcTaskKind::Merge {
            notes,
            origin: MergeOrigin::Tag { tag },
        },
        label,
    );
    super::transcribe::emit_processing_queue(&app);
    super::transcribe::start_processing_worker_if_idle(&app);
    notify_batch_queued(&app, 1);
    Ok(())
}

/// 落点标题取标签末级（`项目/luna` → `luna 汇总`）：全路径当文件名既长又要转义斜杠。
fn merge_tag_stem(app: &tauri::AppHandle, tag: &str) -> String {
    format!(
        "{} {}",
        tag.rsplit('/').next().unwrap_or(tag),
        backend_text(app, "merge-tag-stem-suffix", "汇总")
    )
}

/// 逐条选出的候选：**整体拒绝，不静默跳过**——用户逐条选了 5 条，拿回一篇只含 4 条的汇总
/// 而界面上毫无痕迹，是最难被发现的那种错。码与文案在同一个 `match` 里成对给出。
fn resolve_selected_sources(
    app: &tauri::AppHandle,
    vault: &luna_vault::Vault,
    paths: &[String],
) -> Result<Vec<luna_vault::MergeSource>, String> {
    let mut sources: Vec<luna_vault::MergeSource> = Vec::with_capacity(paths.len());
    for rel in paths {
        let source = vault.merge_source(rel).map_err(|error| match error {
            luna_vault::MergeSourceError::Empty => {
                coded_error(app, "MERGE_SOURCE_EMPTY", "merge-source-empty")
            }
            luna_vault::MergeSourceError::PathEscape
            | luna_vault::MergeSourceError::NotMarkdown
            | luna_vault::MergeSourceError::Unreadable => {
                coded_error(app, "MERGE_SELECTION_INVALID", "merge-selection-invalid")
            }
        })?;
        // 资格与界面下发的那两位（`DraftNoteDto.can_merge` / `NoteTreeDto.can_merge`）
        // **同一个函数**（`luna_vault::merge_block`）：下发的是对象资格而不是一次性通行证
        // ——用户选好之后、点确认之前把其中一篇变成主题页是可能的。
        if let Some(block) = luna_vault::merge_block(source.fingerprint.is_aggregate_page()) {
            return Err(match block {
                luna_vault::MergeBlock::AggregatePage => {
                    coded_error(app, "MERGE_SELECTION_INVALID", "merge-aggregate-page")
                }
            });
        }
        sources.push(source);
    }
    // **按同一个对象去重、保留第一次选择的位置**（PRD-06-047）：同一篇被选两次
    // （列表刷新期间的重复提交、库内一条软链别名、大小写不敏感卷上的 `Note.md` /
    // `note.md`）不该在产物里出现两遍，也不该把条数算成两条。判据是**对象身份**而不是
    // 相对路径字符串。**刻意不做大小写折叠**：那是把平台差异搬进业务层。
    Ok(luna_vault::dedup_by_object(sources))
}

/// 条数区间（TDD-07 §2.1）：按**去重后**算——选了 3 条其中两条是同一篇，实际只有 2 条。
/// 条数**用参数插进文案**，不在 ftl 里写死 2 / 200：那就是同一对常量的第三处定义。
fn check_selection_size(app: &tauri::AppHandle, count: usize) -> Result<(), String> {
    if count < MERGE_NOTE_MIN {
        return Err(coded_error_args(
            app,
            "MERGE_SELECTION_TOO_SMALL",
            "merge-selection-too-small",
            &[("min", MERGE_NOTE_MIN.to_string())],
        ));
    }
    if count > MERGE_NOTE_LIMIT {
        return Err(coded_error_args(
            app,
            "MERGE_SELECTION_TOO_LARGE",
            "merge-selection-too-large",
            &[("max", MERGE_NOTE_LIMIT.to_string())],
        ));
    }
    Ok(())
}

/// 队列里「多条 → 一篇」的**执行体**（`ProcTaskKind::Merge`）：两种来源共用，差别只在
/// 候选集政策（[`MergeOrigin`]）。
///
/// **输入快照在这里、在同一次加锁里取**（TDD-07 §2.1）：sources / 库代际 / 落点目录
/// 全在这一个临界区里定死，内核不再事后补读任何一项——入队时校验过的那一份不算数，
/// 排队期间用户完全可能改了、移走了或删掉了其中一篇；落盘前内核还会按同一份快照复验。
pub(crate) async fn run_merge(
    app: &tauri::AppHandle,
    notes: &[String],
    origin: &MergeOrigin,
    reporter: &DistillReporter<'_>,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let (sources, generation, dir) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let generation = state.vault_generation();
        // 汇总产出是普通笔记，落笔记归档目录。
        let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
        let sources = match origin {
            MergeOrigin::Selection => {
                let sources = resolve_selected_sources(app, vault, notes)?;
                check_selection_size(app, sources.len())?;
                sources
            }
            // **候选集由查询派生，故读不到的那一条跳过而不是整体拒绝**——与逐条选择
            // 的入口刚好相反：那里的每一条都是用户亲手选的，少一条就是错；这里的"这一批"
            // 是一个开放集合，其中一篇刚被删掉不该让整个动作失败。**本批不动它。**
            MergeOrigin::Tag { .. } => luna_vault::dedup_by_object(
                notes
                    .iter()
                    .filter_map(|rel| vault.merge_source(rel).ok())
                    .collect::<Vec<_>>(),
            ),
        };
        (sources, generation, dir)
    };
    let (stem, coverage) = match origin {
        // 逐条选择的入口不接受"只覆盖了前一部分"：见 `CoveragePolicy::RejectPartial`。
        MergeOrigin::Selection => (
            backend_text(app, "merge-stem", "汇总"),
            CoveragePolicy::RejectPartial,
        ),
        MergeOrigin::Tag { tag } => (merge_tag_stem(app, tag), CoveragePolicy::AnnotatePartial),
    };
    merge_notes_into_one(
        app,
        MergeRequest {
            sources: &sources,
            stem: &stem,
            coverage,
            generation,
            dir: &dir,
        },
        reporter,
    )
    .await
}

/// 一次汇总的完整请求（PRD-06-047 / TDD-07 §2.1）。
///
/// 收成一个结构体而不是五个平行参数：三个入口共用这一个内核，而它们的差别恰恰只在这
/// 几项上——参数表越长，"哪个入口传错了哪一位"越是只能靠读调用点确认。
pub(crate) struct MergeRequest<'a> {
    /// 已经解析并校验过的源快照（谁进来、进来多少，是**调用方的候选集政策**）。
    pub sources: &'a [luna_vault::MergeSource],
    /// 产物文件名主干。
    pub stem: &'a str,
    /// 输入超出分片覆盖预算时怎么办（见 [`CoveragePolicy`]）。
    pub coverage: CoveragePolicy,
    /// 取 `sources` 那一刻的**库代际**（ADR-025）。
    ///
    /// **必须与 `sources` 在同一个临界区里取，内核绝不自己补读**：晚一步取到的是新库
    /// 的代际，而 sources 属旧库——两者一比恒相等，落盘前那道代际检查从此不再有任何判别
    /// 力，而它看起来仍然在那儿。这是本字段进请求结构而不是留给内核现取的唯一理由。
    pub generation: u64,
    /// 取 `sources` 那一刻的**落点目录**（同一临界区，理由同上：等待之后再读设置，挑的是
    /// 等待之后那份配置里的目录，而用户在等待期间完全可以改它）。
    pub dir: &'a str,
}

/// 输入超出既有分片覆盖预算时的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoveragePolicy {
    /// 照旧生成，并在正文末尾如实标注"本次只覆盖了前一部分"。
    ///
    /// 用于**候选集由查询派生**的入口（标签汇总）：那里的"这一批"本就是一个
    /// 开放集合，用户没有逐条挑过，截断并说明是诚实的。
    AnnotatePartial,
    /// 发 Job **之前**直接拒绝（`MERGE_INPUT_TOO_LARGE`）。
    ///
    /// 用于用户**逐条勾选**的入口：他勾了 40 条，拿回一篇只覆盖前一部分、来源区却列着
    /// 全部 40 条链接的笔记——那是把"部分处理"伪装成"全部已处理"。他要的是知道该分批，
    /// 而不是一篇看起来完整的产物（TDD-07 §2.1）。
    RejectPartial,
}

/// 「把这些笔记汇总成一篇」（PRD-06-047）：任意笔记集合 → 一篇主题知识笔记。
///
/// 速记多选汇总与标签视图的「汇总这个标签」共用**这一个**内核——两者的差别只是
/// **候选集怎么来的**（逐条勾选 / 按标签），产出形态与纪律完全一致：
/// 原笔记不删、来源区由 Rust 拼接、不写蒸馏指针（`distill_source` 装不下多来源）。
///
/// **跨模型等待的两道校验**（TDD-07 §2.1）：等待可能长达分钟级，而这期间用户完全可以
/// 切库、改稿、把某一篇沉淀掉。落盘那一刻校验**库代际**与**每一条源的指纹**；任一项对不
/// 上就整体放弃，不发布一篇与来源对不上的产物。
pub(crate) async fn merge_notes_into_one(
    app: &tauri::AppHandle,
    req: MergeRequest<'_>,
    reporter: &DistillReporter<'_>,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    if req.sources.is_empty() {
        return Err(coded_error(app, "DISTILL_MERGE_EMPTY", "merge-empty"));
    }

    // 逐条带上标题：速记标题含时间，"什么时候写的"是这堆碎片的主要线索。
    let input = req
        .sources
        .iter()
        .map(|src| format!("### {}\n{}", src.title, src.input))
        .collect::<Vec<_>>()
        .join("\n\n");

    let settings = state.settings_snapshot();
    let ctx = lang_context_from_settings(&settings);
    let lang = floating_summary_lang(&settings, &ctx);
    // 汇总（多篇 → 一篇）**不进模板体系**：候选集来自多个来源，没有统一的"输入形态"可言，
    // 而模板回答的是"这一篇素材是什么体裁"（PRD-06-047）。
    let budget = distill_budget(&state).await;
    let covered = cover(&input, budget);
    if covered.dropped && req.coverage == CoveragePolicy::RejectPartial {
        // 发 Job **之前**拒绝：跑完再说"只覆盖了一部分"，模型时间与用户的等待都白花了。
        return Err(coded_error(
            app,
            "MERGE_INPUT_TOO_LARGE",
            "merge-input-too-large",
        ));
    }
    let i18n = backend_copy_arc(app);
    let body = summarize_long(
        &state,
        i18n.as_deref(),
        &covered.chunks,
        luna_core::compose_chunk_instruction(luna_core::prompt::registry(), &lang),
        luna_core::compose_merge_instruction(
            luna_core::prompt::registry(),
            &lang,
            req.sources.len(),
        ),
        // 多篇汇总**留在 `Distill` 这本账上**，不跟着 PRD-06-054 改记 `Settle`：
        // 它的候选集是"一批笔记"（速记收件箱 / 一个标签），
        // 里面既有记录也有知识，没有一个诚实的答案说这次是在沉淀谁的草稿。
        // 分账维度只在**单篇**那条路上分得清（`DistillInput::origin`）。
        JobOrigin::Distill,
        budget,
        // 进度与取消由调用方给：统一队列里的汇总携真实的进度回调与取消令牌。
        reporter,
    )
    .await?;
    // **多篇汇总的触顶标注本批不做，且这是缺口而不是设计**：汇总产物不经
    // `write_distilled`（它没有 `distill_*` 那套指针与指纹），也不在 `distill_scan` 的
    // 任何一个桶里，故给它标注需要另一套「不完整」的落盘与提醒面。本批只收单篇蒸馏
    // 这条路（D9′ 的字面范围），汇总那条留作后续。
    let body = body.text;

    // **最终发布收成一个写锁块**（TDD-07 §2.1）：复验代际 → 逐条复验源指纹 → 挑唯一
    // 落点 → 渲染 → 登记 durable intent → 原子创建落点。修复前这些步骤散在三处：代际检查
    // 在取锁**之前**、落点在第一次加锁里挑、intent 在锁外登记、写盘在第二次加锁里做——
    // 每一道缝里都能挤进另一次写入，而每一步单看都对。
    let (target, intent) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // ① 库代际：用的是与 `sources` **同一临界区**取的那一个（`MergeRequest::generation`）。
        if !state.still_current_vault(req.generation) {
            return Err(coded_error(
                app,
                "DISTILL_VAULT_SWITCHED",
                "merge-vault-switched",
            ));
        }
        publish_merge_locked(
            vault,
            &MergePublish {
                sources: req.sources,
                dir: req.dir,
                stem: req.stem,
            },
            |target| merge_final_body(app, &body, req.sources, target, covered.dropped),
            |target, final_body| {
                // `created_new` **恒为 true 且是事实、不是假设**：下一步的 `link(2)` 要么
                // 把这篇笔记造出来（于是确实是一次新建），要么 `EEXIST` 整体放弃、这条
                // intent 被 abandon 掉而永远不会 commit。所以"汇总发布记的是一次新建"
                // 不靠回读推断，靠"只有造出来了才提交"这条结构保证。
                // 汇总产出的是叶子知识笔记（不是聚合层），生命周期照叶子走（TDD-06 §3.7.1）。
                super::knowledge::prepare_knowledge_write_locked(
                    app, vault, target, final_body, "merge", /* aggregate */ false,
                    /* created_new */ true,
                )
            },
            |intent| super::knowledge::abandon_knowledge_write(app, intent),
        )
        .map_err(|error| match error {
            // 指纹含正文 + 成熟度 + 聚合身份：一篇在等待期间被沉淀掉的草稿，正文可以一个
            // 字节没动，而它已经不该出现在这次汇总里了。
            MergePublishError::SourceChanged => {
                coded_error(app, "MERGE_SOURCE_CHANGED", "merge-source-changed")
            }
            // **码与"源变了"共用一个**（TDD-07 §2.1 的错误清单是封闭的）：对调用方而言
            // 这两件事是同一个契约——"落盘那一刻库已经不是发起时那样了，本次结果放弃，
            // 重试即可"，分支逻辑完全一致，多一个码只是让调用方多写一条同样的分支。
            // 说明文案仍是两条：给人看的那半句必须说准是哪一种，否则"请重新选择"会落在
            // 一个选择本来就没问题的用户头上。
            MergePublishError::TargetTaken => {
                coded_error(app, "MERGE_SOURCE_CHANGED", "merge-target-taken")
            }
            MergePublishError::Failed(message) => message,
        })?
    };
    emit_vault_changed(app, &target, false);
    super::knowledge::commit_knowledge_write(app, intent)?;
    Ok(target)
}

/// 汇总产物的最终正文：模型正文 + Rust 确定性拼接的「来源」区（+ 覆盖说明）。
///
/// 来源区由 Rust 拼接而非交给 LLM（PRD-02-055）：链接必须准确，不能靠模型复述路径。
/// 链接是**相对 `target` 的**，故必须在落点挑定之后才能渲染——这也是发布顺序里"挑落点"
/// 必须排在"渲染"前面的原因。
fn merge_final_body(
    app: &tauri::AppHandle,
    model_body: &str,
    sources: &[luna_vault::MergeSource],
    target: &str,
    partial_coverage: bool,
) -> String {
    let mut final_body = model_body.to_owned();
    final_body.push_str("\n\n## ");
    final_body.push_str(&backend_text(app, "merge-sources-heading", "来源"));
    final_body.push_str("\n\n");
    for src in sources {
        final_body.push_str(&format!(
            "- [{}]({})\n",
            src.title,
            encode_md_link(&relative_link(target, &src.rel))
        ));
    }
    if partial_coverage {
        final_body.push_str("\n> ");
        final_body.push_str(&backend_text(
            app,
            "merge-partial-coverage",
            "注：笔记较多，本次汇总覆盖了前一部分。",
        ));
        final_body.push('\n');
    }
    final_body
}

/// 一次汇总发布的输入（不含 `AppHandle` 才碰得到的那两件事，见 [`publish_merge_locked`]）。
pub(crate) struct MergePublish<'a> {
    pub sources: &'a [luna_vault::MergeSource],
    pub dir: &'a str,
    pub stem: &'a str,
}

/// 发布失败的三种结局。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergePublishError {
    /// 逐条复验发现源在等待期间变了（改稿 / 移走 / 被沉淀 / 改了聚合身份）。
    SourceChanged,
    /// 落点在「挑名字」与「原子创建」之间被别的写入占了（`link(2)` 的 `EEXIST`）。
    TargetTaken,
    /// 登记 intent 或写盘本身失败（错误串原样带回）。
    Failed(String),
}

/// 发布一次汇总产物——**调用方必须已持有 vault 锁，本函数一次都不再取**。
///
/// 抽成自由函数的理由只有一个：它这样能被真库单测。`AppHandle` 在测试里造不出来，而整条
/// 发布路径上只有两处非碰它不可（登记 / 放弃 durable intent），故它们以闭包传入——于是
/// 「复验 → 挑落点 → 渲染 → 登记 → 原子创建」这个顺序本身，连同"占了就整体放弃、绝不换
/// 个名字写下去"这条纪律，可以用真文件、真并发验。
///
/// **登记 intent 排在原子创建之前**（TDD-06 §3.7.1「写盘前先有 durable intent」）：
/// 反过来排的话，被杀在两步之间会留下一篇系统永远不知道的笔记；而按这个顺序，留下的是一条
/// 目标不存在的 intent，崩溃恢复对它有明写的处置（"目标不存在即安全放弃"）。
/// `created_new` 的正确性不因此打折——见调用点注释：只有 `link` 真造出来才会走到 commit。
pub(crate) fn publish_merge_locked<I>(
    vault: &Vault,
    req: &MergePublish<'_>,
    render_body: impl FnOnce(&str) -> String,
    reserve_intent: impl FnOnce(&str, &str) -> Result<I, String>,
    abandon_intent: impl FnOnce(I),
) -> Result<(String, I), MergePublishError> {
    // ② 逐条复验源快照。（① 库代际是 app 层的事实，Vault 不认识它，由调用方在**同一个**
    //    临界区里、在本函数之前复验。）
    if !vault.merge_sources_unchanged(req.sources) {
        return Err(MergePublishError::SourceChanged);
    }
    // ③ 挑唯一落点。它只是**挑名字**，"到底占没占住"由下面 ⑤ 的 `link(2)` 回答。
    let target = unique_note_path_in(vault, req.dir, req.stem);
    // ④ 渲染最终内容。合并整理**不写蒸馏指针**：它的来源是一堆笔记而非某一篇，
    //    `distill_source` 装不下多个来源，硬塞一个进去会让"源已更新"这套判定对着错误的
    //    对象说话。溯源靠 `merge_final_body` 拼的「来源」区。
    let final_body = render_body(&target);
    let skeleton = format!(
        "---\nluna:\n  schema: {}\n---\n\n{final_body}",
        luna_vault::LUNA_SCHEMA
    );
    let meta = luna_vault::LunaMeta {
        note_type: NoteType::Note,
        ..luna_vault::LunaMeta::default()
    };
    let content = luna_vault::update_luna(&skeleton, &meta)
        .map_err(|error| MergePublishError::Failed(vault_err(error)))?;
    // ⑤ 登记 durable intent（写盘之前）。失败即整体放弃，此刻磁盘上什么都还没动。
    let intent = reserve_intent(&target, &final_body).map_err(MergePublishError::Failed)?;
    // ⑥ 原子创建落点：同目录临时文件 + `link(2)`。
    //    **绝不覆盖、也绝不换个名字写下去**：换名会让刚登记的 intent 指着另一篇笔记，
    //    知识生命周期随后作用在错的对象上。
    match vault.publish_new(&target, &content) {
        Ok(()) => Ok((target, intent)),
        Err(luna_vault::VaultError::AlreadyExists(_)) => {
            abandon_intent(intent);
            Err(MergePublishError::TargetTaken)
        }
        Err(error) => {
            abandon_intent(intent);
            Err(MergePublishError::Failed(vault_err(error)))
        }
    }
}

/// 一次汇总最多合并多少篇笔记。
///
/// 上限存在只为挡住"一次把三年碎片全喂进去"这种注定超预算的用法；真正的覆盖边界由
/// 分片摘要的 `DISTILL_MAX_CHUNKS` 决定，超出时正文末尾会如实标注。
///
/// **随 `DraftInboxDto.merge_max` 下发给界面**（PRD-06-047）：前端照着写一份数字就是
/// 同一条规则的第二处定义，改了一边不会有东西变红。
pub(crate) const MERGE_NOTE_LIMIT: usize = 200;

/// 一次汇总最少要选几条。
///
/// **一条不是汇总**：「多条 → 一篇」承诺的是"把散落的碎片收拢成一篇"，一条笔记的
/// 那一篇是**单篇整理**（`distill_notes` 只交一条路径，PRD-06-054）——同一个界面动作在条数
/// 为 1 时悄悄变成另一件事，用户拿到的产物形态与来源区都不是他要的。
pub(crate) const MERGE_NOTE_MIN: usize = 2;

// 区间本身在**编译期**钉住：min 掉到 1 会让「多条 → 一篇」在条数为 1 时悄悄变成单篇
// 整理，而这两件事的产物形态与来源区都不一样（运行期断言拦不住一个常量笔误）。
const _: () = assert!(MERGE_NOTE_MIN >= 2);
const _: () = assert!(MERGE_NOTE_LIMIT > MERGE_NOTE_MIN);

// ── 触发（边界事件）──────────────────────────────────────────────────────────

/// 蒸馏触发档（设置 `recording.distill_mode` / `transcribe.distill_mode` / `chat.distill_mode`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DistillMode {
    /// 关闭：只有用户点徽标/右键才生成。
    Off,
    /// 完成后提示：不动手，只在既有完成通知里带一句「可生成知识笔记」。
    Prompt,
    /// 自动：后台直接生成（仍受「用户编辑过就止步」约束）。
    Auto,
}

impl DistillMode {
    fn parse(raw: &str) -> Self {
        match raw.trim() {
            "auto" => Self::Auto,
            "prompt" => Self::Prompt,
            _ => Self::Off,
        }
    }
}

/// 某类资料的触发档设置键。**按源类型分设**——三条路径的产出频率与价值差别很大
/// （一天可能有几十次对话、但只有一两场会议），共用一个开关会逼用户为其中一条将就。
fn distill_mode_key(note_type: NoteType) -> Option<&'static str> {
    match note_type {
        NoteType::Recording => Some("recording.distill_mode"),
        NoteType::Transcript => Some("transcribe.distill_mode"),
        NoteType::Chat => Some("chat.distill_mode"),
        _ => None,
    }
}

/// 边界事件触发入口（转写完成 / 会话终局）。
///
/// **只挂边界事件，绝不挂 `vault://changed` 之类的高频写盘信号**——持久对话每轮都写盘，
/// 逐写重蒸馏既烧算力又毫无意义（结论还没出现）。
///
/// `Auto` 档在此后台生成；`Prompt` 档不动手——提示由完成通知承载，判定见
/// [`distill_hint_for`]（刻意分成两个函数：一个决定"要不要现在做"，一个决定"要不要
/// 在通知里提一句"，把布尔穿过五层任务返回值只会让每一层都多背一个它不关心的字段）。
pub(crate) fn spawn_distill_if_configured(app: &tauri::AppHandle, note_rel: &str) {
    let state = app.state::<AppState>();
    let Ok(guard) = state.vault.lock() else {
        return;
    };
    let Some(vault) = guard.as_ref() else {
        return;
    };
    let Ok(doc) = vault.read(note_rel) else {
        return;
    };
    let Some(key) = distill_mode_key(doc.meta.note_type) else {
        return;
    };
    // 已被用户编辑过的蒸馏笔记：自动与提示都止步——反复提示"要不要覆盖你的编辑"
    // 是骚扰，用户想重来时徽标一直在那里（PRD-06-044）。
    if let Ok(DistillState::Present { edited: true, .. }) = vault.distill_state(note_rel) {
        return;
    }
    drop(guard);

    let mode = DistillMode::parse(
        &state
            .settings_snapshot()
            .get_str(key)
            .unwrap_or_else(|| "prompt".into()),
    );
    if mode == DistillMode::Auto {
        // 同批量：自动档不做模板决策（沿用已记录的、没有则按规则建议），且**入队而非直接
        // 跑**——转写刚完成时队列里往往还有别的活，插队直接跑会两个 LLM 任务抢引擎。
        // 自动档同样过预检：它花的是同一份额度，而且没有人在等着看结果——跑到一半
        // 炸掉的话，用户只会在队列面板里看到一条来历不明的红。取余量要发网络请求，
        // 故挪进一个 spawn：本函数挂在转写完成 / 会话终局这类边界事件上，是同步的。
        let app = app.clone();
        let note_rel = note_rel.to_owned();
        tauri::async_runtime::spawn(async move {
            let mut ledger = QuotaLedger::new(cloud_output_headroom(&app).await);
            if let Err(code) = enqueue_distill(&app, note_rel, None, None, &mut ledger) {
                // 脱敏：只留错误码，不含正文/输出（TDD-12）。
                eprintln!("[distill] 自动生成入队跳过：{code}");
            }
        });
    }
}

/// 对话终局的「完成后提示」（PRD-03-014）。
///
/// 录音/转写的提示**骑在既有完成通知上**（换 kind），但对话终局本没有任何通知——
/// 逐轮归档是静默的，会话被顶替也是静默的。`Prompt` 档在此**自己发一条** Completion
/// 通知（深链到那篇对话笔记），否则该档对对话就是一句空话；`Auto`/`Off` 档不发
/// （前者结果自己会来，后者用户明说了不要）。
pub(crate) fn notify_chat_distill_hint(app: &tauri::AppHandle, note_rel: &str) {
    if !distill_hint_for(app, note_rel) {
        return;
    }
    let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    nc.publish(
        crate::notifications::NotificationEvent::completion(
            "chat_ended_distill",
            // 路由是导航数据不是展示文本（展示文本走 i18n，不含笔记名）；
            // 深链落到那篇对话笔记，历史行右键与其上的入口都在那儿。
            crate::notifications::NotificationTarget::Route(format!("notes://{note_rel}")),
            crate::notifications::PrivatePayload::new(note_rel.to_owned()),
        ),
        app,
    );
}

/// 完成通知是否该带一句「可生成知识笔记」（`Prompt` 档，PRD-06-044）。
///
/// 只对**还没有**蒸馏笔记、或源已更新且用户没编辑过的情形提示——已经是最新的、
/// 或用户已自己接手编辑的，再提一次就是骚扰。
pub(crate) fn distill_hint_for(app: &tauri::AppHandle, note_rel: &str) -> bool {
    let state = app.state::<AppState>();
    let Ok(guard) = state.vault.lock() else {
        return false;
    };
    let Some(vault) = guard.as_ref() else {
        return false;
    };
    let Ok(doc) = vault.read(note_rel) else {
        return false;
    };
    let Some(key) = distill_mode_key(doc.meta.note_type) else {
        return false;
    };
    let worth_prompting = match vault.distill_state(note_rel) {
        Ok(DistillState::Absent) => true,
        Ok(DistillState::Present { stale, edited, .. }) => stale && !edited,
        Err(_) => false,
    };
    drop(guard);
    worth_prompting
        && DistillMode::parse(
            &state
                .settings_snapshot()
                .get_str(key)
                .unwrap_or_else(|| "prompt".into()),
        ) == DistillMode::Prompt
}

#[cfg(test)]
mod draft_distill_input_tests {
    use super::*;

    #[test]
    fn voice_quick_note_drops_the_raw_section_and_pins_refined_facts() {
        let body = "# 开会改期\n\n## 优化结果\n\n周三开会，通知小李。\n\n## 原始转录\n\n周三 开会 通知小李\n\n## 待办\n- [ ] 通知小李\n- [x] 订会议室\n";
        let meta = luna_vault::LunaMeta {
            refined_at: Some("2026-09-03T10:00:00+08:00".into()),
            dates: vec!["2026-09-09".into()],
            ..luna_vault::LunaMeta::default()
        };
        let (text, facts) = draft_distill_input(body, &meta);
        assert!(!text.contains("原始转录"), "{text}");
        assert!(!text.contains("周三 开会 通知小李"), "{text}");
        assert!(text.contains("周三开会，通知小李。") && text.contains("- [ ] 通知小李"));
        assert_eq!(facts.action_items, vec!["通知小李", "订会议室"]);
        assert_eq!(facts.dates, vec!["2026-09-09"]);
    }

    #[test]
    fn a_hand_written_draft_passes_through_with_no_pinned_facts() {
        let body = "随手记：周三开会\n- [ ] 这是用户自己写的待办\n";
        let (text, facts) = draft_distill_input(body, &luna_vault::LunaMeta::default());
        assert_eq!(text, body.trim());
        assert!(facts.is_empty(), "没跑过提炼的待办不当已确认事实");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distill_mode_parses_three_levels_and_defaults_to_off() {
        assert_eq!(DistillMode::parse("auto"), DistillMode::Auto);
        assert_eq!(DistillMode::parse("prompt"), DistillMode::Prompt);
        assert_eq!(DistillMode::parse("off"), DistillMode::Off);
        // 未知取值一律按最保守的档处理——绝不因为读到脏值就替用户跑 LLM。
        assert_eq!(DistillMode::parse("whatever"), DistillMode::Off);
    }

    #[test]
    fn only_source_material_types_have_a_mode_key() {
        assert!(distill_mode_key(NoteType::Recording).is_some());
        assert!(distill_mode_key(NoteType::Transcript).is_some());
        assert!(distill_mode_key(NoteType::Chat).is_some());
        assert!(distill_mode_key(NoteType::Note).is_none());
        assert!(distill_mode_key(NoteType::QuickNote).is_none());
    }

    // 模板解析的守护测试住在 `luna_core::distill_template`（判定是纯函数，测试跟着判定走，
    // 不在 IO 层再写一份会漂移的副本）：解析优先级 / 批量与自动档沿用已记录 / 换模板取新
    // 默认值 / 未知与已删模板回退建议，共 5 条。

    #[test]
    fn coverage_boundary_is_shared_by_summary_and_full_text() {
        // 摘要与整理稿必须**共用同一个覆盖边界**：各自截断的话，末尾那句「覆盖了前一部分」
        // 会对不上其中一条，而没有任何地方说得清差在哪儿。
        let line = "这是一行足够长的转写内容用来撑满分片预算。\n"; // 22 字符
                                                                   // 每片 12k 字符 ≈ 545 行，故 700 行/片 × 12 片有余，必定超界。
        let text = line.repeat(DISTILL_MAX_CHUNKS * 700);
        let covered = cover(&text, DistillBudget::FULL);
        assert!(covered.dropped, "超界必须如实标记");
        assert_eq!(covered.chunks.len(), DISTILL_MAX_CHUNKS);
        // 整理稿据以重新切分的正是摘要覆盖到的那段文本（拼接零丢失）。
        assert_eq!(covered.text, covered.chunks.concat());
        assert!(text.starts_with(&covered.text));

        // 未超界时一字不丢。
        let short = cover("短文本", DistillBudget::FULL);
        assert!(!short.dropped);
        assert_eq!(short.text, "短文本");
    }

    #[test]
    fn chunks_split_on_line_boundaries_and_cover_everything() {
        let text = (1..=50)
            .map(|i| format!("第{i}行内容"))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = split_chunks(&text, 40);
        assert!(chunks.len() > 1, "应被切成多片");
        // 覆盖完整：拼回去必须与原文一致（一个字都不能在切分中丢掉）。
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn oversized_single_line_is_hard_split_without_loss() {
        let line = "一".repeat(100);
        let chunks = split_chunks(&line, 30);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks.concat(), line);
    }

    /// AI 产物落盘前的确定性归一（PRD-06-057）：两步都做、且**只删不加**。
    #[test]
    fn model_output_is_normalized_before_it_can_reach_disk() {
        // 外部/内部 AI 常见的两种脏：死引用角标 + 单行空嵌套列表链。
        let raw = "  ## 要点[1]\n\n* * 二连\n* * * * * * 六连\n- 正常项\n";
        assert_eq!(
            normalize_model_output(raw),
            // 首尾空白由既有 `trim` 口径去掉（本函数只是接在它后面）。
            "## 要点\n\n* 二连\n* 六连\n- 正常项"
        );
        // 幂等：再跑一次一个字节都不变（归一不是"每次都改一点"）。
        let once = normalize_model_output(raw);
        assert_eq!(normalize_model_output(&once), once);
    }

    /// 归一**绝不动代码块里的内容**：模型产出的 Markdown 里带一段示例代码，
    /// 而那段代码里的 `* * ` 是代码，不是列表。
    #[test]
    fn normalization_keeps_fenced_code_and_ordinary_prose_intact() {
        let raw = "```md\n* * 这是示例，不该被改\n```\n\n正文里的 * * 也不动。\n";
        assert_eq!(normalize_model_output(raw), raw.trim());
        // 干净的产出零改动（除首尾空白的 trim——那是既有口径）。
        let clean = "## 概要\n\n- 一条\n- 两条";
        assert_eq!(normalize_model_output(clean), clean);
    }

    /// **端到端**：一份带畸形列表链的模型产出经归一后落进 Vault，磁盘上那篇知识笔记里
    /// 一条空嵌套链都没有，而正常列表与代码块逐字保留。
    ///
    /// 覆盖的是「归一 → `write_distilled` → 磁盘」这一段；再往前那一段（LLM 真的
    /// 吐出这种链）需要真实模型，**未真机验证**。
    #[test]
    fn normalized_model_output_lands_on_disk_without_empty_nesting_chains() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path()).expect("open vault");
        vault
            .write(
                "Recordings/周会.md",
                "---\nluna:\n  type: recording\n---\n\n会议原文\n",
            )
            .expect("写源笔记");

        // 「模型刚吐出来的那一份」——注意代码块里也有一条链，它必须活下来。
        let model_output = "## 决议\n\n* * 上线时间定在下周三\n* * * * 由甲负责[2]\n\n\
             ```text\n* * 这行是示例\n```\n";
        let body = normalize_model_output(model_output);
        vault
            .write_distilled(
                "Recordings/周会.md",
                "Notes/周会 · 纪要.md",
                &body,
                "meeting",
                false,
                None,
            )
            .expect("落盘");

        let on_disk = vault.read("Notes/周会 · 纪要.md").expect("读回").content;
        assert!(on_disk.contains("* 上线时间定在下周三"), "{on_disk}");
        assert!(on_disk.contains("* 由甲负责"), "{on_disk}");
        // 代码块里的那一条原样保留——归一跳过围栏，落盘之后依然如此。
        assert!(on_disk.contains("```text\n* * 这行是示例"), "{on_disk}");
        // 磁盘上**只剩**围栏里那一条链，正文里一条不留。
        assert_eq!(
            on_disk.matches("* * ").count(),
            1,
            "正文里不该还有空嵌套链：{on_disk}"
        );
        // 死角标一并剥掉（同一个咽喉的另一半）。
        assert!(!on_disk.contains("[2]"), "{on_disk}");
    }

    /// **截断标注必须一路走到落盘处，且必须落在文件里**（D9′ ①）。
    ///
    /// 这条钉住的是**管线的形状**，不是某一句文案：`summarize_once` 从 `JobEvent::Finished`
    /// 里取走的必须是整个 `JobResult` 的两个事实（正文 + 触顶归属），而不是只取 `Text`；
    /// `build_distill_body` 必须把它带出来；`distill_job` 与 `distill_apply` 两条落盘路
    /// 都必须把它交给 `write_distilled`。缺任何一环，"不完整"这个事实就会在那一环蒸发，
    /// 而表现出来只是"某些笔记看着有点短"。
    ///
    /// 读源码是因为整条链是 `async` 且要真 `AppHandle` 与真 LLM；能用真库跑出来的那一半
    /// （标注落盘、盘点桶）在 `luna_vault::vault::tests` 里用真文件验。
    #[test]
    fn the_truncation_fact_survives_every_hop_from_job_result_to_disk() {
        // **先剥注释再扫**（同 `merge_notes_freezes_one_snapshot…` 的手法）：
        // 本文件的注释里逐字写着这些标识符，带注释扫描的话，把实现改坏、只留注释，
        // 这条用例照样全绿——那正是一次变异验证当场抓到的事。
        let code: String = crate::ipc::source_without_tests(include_str!("distill.rs"))
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let code = code.as_str();

        // ① 源头：终态事件里那一格被读出来，而不是只取 payload。
        let once = code
            .split("async fn summarize_once(")
            .nth(1)
            .expect("找不到 summarize_once");
        let once = once
            .split("\n/// ")
            .next()
            .expect("summarize_once 边界变了");
        assert!(
            once.contains("result.output_truncated"),
            "summarize_once 又把 JobResult 拆成裸文本了——触顶事实在第一站就没了"
        );
        assert!(
            once.contains("Ok(Generated {"),
            "summarize_once 的返回类型退回裸 String 了"
        );

        // (1b) 末次汇总同样要并进分片阶段的触顶：只有最后一次调用没触顶不代表整篇完整。
        let long = code
            .split("async fn summarize_long(")
            .nth(1)
            .expect("找不到 summarize_long");
        let long = long.split("\nasync fn ").next().unwrap_or(long);
        assert!(
            long.contains("reduced.truncated.or(truncated)"),
            "二次汇总把分片阶段的触顶丢了"
        );

        // ② 中途：分片与整理稿的触顶必须并进整篇，而不是只看最后一次调用。
        for (name, marker) in [
            // 判据是**逐片那一次读**（`digest` / `part` 是循环里的那一片），不是宽松的
            // `truncated.or(` ——后者被末次汇总那一句顺手满足，删掉逐片累积也不会红。
            ("summarize_long", "truncated.or(digest.truncated)"),
            ("rewrite_full_text", "truncated.or(part.truncated)"),
        ] {
            let body = code
                .split(&format!("async fn {name}("))
                .nth(1)
                .unwrap_or_else(|| panic!("找不到 {name}"));
            let body = body.split("\n/// ").next().unwrap_or(body);
            assert!(
                body.contains(marker),
                "{name} 没有把逐片的触顶并进整篇——第 K 片被腰斩会被当成没发生"
            );
        }

        // ②b 成稿装配处：尾注与返回值取的都必须是**这一次生成**的那个事实。
        // 变异验证抓到过一个很安静的坏法——把这里换成常量 `None`，尾注不再出现、
        // frontmatter 不再写，而落盘处那两行 `.map(TruncatedBy::key)` 原封不动地还在。
        let build = code
            .split("async fn build_distill_body(")
            .nth(1)
            .expect("找不到 build_distill_body");
        let build = build.split("\nfn ").next().unwrap_or(build);
        assert!(
            build.contains("let truncated = body.truncated;"),
            "成稿的触顶事实被换成了别的来源——尾注与 frontmatter 会一起失真"
        );
        assert!(
            build.contains("\"distill-output-truncated-note\""),
            "尾注没走 Fluent key；写错 key 时 backend_text 会静默回落到中文兜底串，\
             英文界面的用户会在自己的笔记里读到一句中文，而一切用例照常全绿"
        );

        // ③ 终点：两条落盘路都把它交给 write_distilled。
        assert_eq!(
            code.matches("write_distilled(").count(),
            2,
            "落盘点数变了——新增一条就必须同样带上截断归属"
        );
        assert!(
            code.contains("built.truncated.map(luna_core::TruncatedBy::key)"),
            "队列落盘路没把归属交给 write_distilled"
        );
        assert!(
            code.contains("staged.output_truncated.as_deref()"),
            "预览落盘路没把归属交给 write_distilled——`distill_apply` 不重跑 LLM，\
             槽里不带这个事实的话经预览落盘的产物永远是「完整」的"
        );
    }

    /// 正文尾注**必须走后端 Fluent**，不是在 IPC 里手写两套文字（TDD-07 §2.1 同一纪律）。
    ///
    /// 它同时兜住一类很安静的错：key 写错时 `backend_text` 会静默回落到中文兜底串，
    /// 于是英文界面的用户在自己的笔记里读到一句中文——而一切测试照常全绿。
    #[test]
    fn the_truncation_note_comes_from_the_fluent_bundle_in_both_languages() {
        for (language, expected_head) in [("zh-CN", "注："), ("en-US", "Note:")] {
            let i18n = crate::i18n::BackendI18n::new(language);
            let text = i18n
                .translate("distill-output-truncated-note")
                .unwrap_or_else(|| panic!("{language} bundle 里没有截断尾注"));
            assert!(
                text.starts_with(expected_head),
                "{language} 取到的是另一种语言的句子：{text}"
            );
            // 与「覆盖边界」那句是两件事：一句说输入没读完，一句说输出没写完。
            let coverage = i18n
                .translate("merge-partial-coverage")
                .expect("覆盖说明历来就在");
            assert_ne!(text, coverage);
        }
    }

    /// 摘要与改写的输出预算**必须同量级**（D9′ ③）。
    ///
    /// 2048 那个值是按"一篇纪要要写多长"定的，而闸量的是 `tokens_out`——推理模型的思维链
    /// 计入它（真机 7332 里 3540 帧是思考）。留着 2048 会让摘要片片触顶，于是刚做出来的
    /// 标注恒真、当场失效。
    #[test]
    fn the_summary_output_budget_is_no_longer_a_page_length_guess() {
        assert_eq!(DISTILL_SUMMARY_OUTPUT_TOKENS, DISTILL_REWRITE_OUTPUT_TOKENS);
        assert_eq!(DISTILL_SUMMARY_OUTPUT_TOKENS, 8_192);
    }

    /// **读不到额度必须放行**（PRD-13-040 ②，也是这条特性能不能上线的前提）。
    ///
    /// 离线、未登录、严格本地、私有化部署、跑本机模型、服务端不按 token 计量、账本写着
    /// 「不限」——这些用户压根没有额度这个概念。把「不知道」当成「不够」，等于在完全正常
    /// 的环境里让「整理」这个按钮失灵，而失灵的理由用户永远看不懂。
    #[test]
    fn an_unknown_headroom_always_lets_the_job_start() {
        let mut ledger = QuotaLedger::new(None);
        assert_eq!(ledger.charge(u64::MAX), Ok(()));
        assert_eq!(ledger.charge(u64::MAX), Ok(()));
        assert_eq!(ledger.charge(0), Ok(()));
    }

    /// 额度不足时**当场拒绝并说清缺口**：需要多少、还剩多少、差多少，三个数一个不少。
    ///
    /// 只说「额度不足」的话，用户既不知道该补多少，也无从判断是不是该改用本机模型——
    /// 而那正是这条提示唯一能给出的两个动作。
    #[test]
    fn a_short_headroom_is_refused_with_the_actual_gap() {
        // 够用（含刚好够）一律放行：边界上宁可放行也不拦。
        assert_eq!(QuotaLedger::new(Some(1_000.0)).charge(1_000), Ok(()));
        assert_eq!(QuotaLedger::new(Some(1_000.5)).charge(1_000), Ok(()));

        let gap = QuotaLedger::new(Some(1_000.0))
            .charge(9_000)
            .expect_err("差 8000 必须拦下");
        assert_eq!(
            gap,
            QuotaShortfall {
                need: 9_000,
                remaining: 1_000,
                short: 8_000,
            }
        );
    }

    /// **缺口那三个数必须作为参数交给 Fluent，绝不在 Rust 里拼成句子。**
    ///
    /// 这条守的是 R1.1 那个缺陷本身：`DISTILL_QUOTA_SHORT` 曾是一句 `format!` 出来的中文，
    /// 英文界面的用户会原样读到它。判据是两条——① 两份 bundle 各自都取得到、且取到的是
    /// 自己那一门语言；② 三个数在两门语言里都真的被代入。
    #[test]
    fn the_quota_shortfall_speaks_the_users_language_with_all_three_numbers() {
        for (language, native, foreign) in [
            ("zh-CN", "服务端余量", "There is not enough"),
            ("en-US", "There is not enough", "服务端余量"),
        ] {
            let i18n = crate::i18n::BackendI18n::new(language);
            let text = i18n
                .translate_args(
                    "distill-quota-short",
                    &[
                        ("need", "9000".to_owned()),
                        ("remaining", "1000".to_owned()),
                        ("short", "8000".to_owned()),
                    ],
                )
                .unwrap_or_else(|| panic!("{language} bundle 里没有额度缺口说明"));
            assert!(text.contains(native), "{language} 取到的不是本语言：{text}");
            assert!(
                !text.contains(foreign),
                "{language} 里混进了另一种语言：{text}"
            );
            for number in ["9000", "1000", "8000"] {
                assert!(
                    text.contains(number),
                    "{language} 缺口少了 {number}——只说「额度不足」是没法行动的：{text}"
                );
            }
        }
        // 那句话不许再出现在 Rust 源码里：`format!` 一次就等于把它钉死在一种语言上。
        let code = crate::ipc::source_without_tests(include_str!("distill.rs"));
        assert!(
            !code.contains("服务端余量不够跑完这一篇"),
            "额度缺口说明又被写回 Rust 源码了——英文用户会读到一段中文"
        );
    }

    /// **一批共用一份余量必须累减**（R1.7）。
    ///
    /// 逐篇各自与同一个余量比对时，余量 100k、每篇需 50k 的 50 篇会全部通过预检，而只有
    /// 前两篇跑得完——「跑到第 K 片才发现钱不够」这件事被从单篇里赶走之后，在批量里原样
    /// 复活。累减之后第三篇起就当场被拒，且拒绝理由带着此刻的真实余量。
    #[test]
    fn a_batch_spends_one_shared_headroom_instead_of_comparing_each_note_to_it() {
        let mut ledger = QuotaLedger::new(Some(100_000.0));
        assert_eq!(ledger.charge(50_000), Ok(()), "第一篇装得下");
        assert_eq!(ledger.charge(50_000), Ok(()), "第二篇正好把余量用完");
        let gap = ledger
            .charge(50_000)
            .expect_err("第三篇必须当场被拒——不做累减的话它会通过预检、跑到一半才炸");
        assert_eq!(
            gap,
            QuotaShortfall {
                need: 50_000,
                remaining: 0,
                short: 50_000,
            }
        );
    }

    /// 估算按**真实分片**走，且是一个**下界**（宁可估少，绝不估多）。
    ///
    /// 估多的代价是拦下一件本来能跑完的任务——那就把善意提醒变成了新的卡点。
    /// 输入粗估（任务前预估）：中日韩一字一个、其余四字一个；带整理稿时全文多读一遍。
    #[test]
    fn rough_input_tokens_count_wide_chars_singly_and_double_for_full_text() {
        assert_eq!(rough_tokens("会议纪要"), 4);
        assert_eq!(rough_tokens("abcdefgh"), 2);
        assert_eq!(rough_tokens("abc"), 1);
        assert_eq!(rough_tokens(""), 0);
        assert_eq!(estimate_input_tokens("会议纪要abcd", false), 5);
        assert_eq!(estimate_input_tokens("会议纪要abcd", true), 10);
    }

    #[test]
    fn the_estimate_scales_with_the_real_shard_count_and_never_overshoots() {
        let short = "很短的一段资料。";
        // 单片 = 一次调用，没有额外的汇总那一次。
        assert_eq!(
            estimate_output_tokens(short, false, DistillBudget::FULL),
            u64::from(DISTILL_SUMMARY_OUTPUT_TOKENS)
        );

        // 多片 = 逐片一次 + 末尾汇总一次（与 `summarize_long` 同构）。
        let line = "这是一行足够长的转写内容用来撑满分片预算。\n";
        let long = line.repeat(DISTILL_MAX_CHUNKS * 700);
        let chunks = cover(&long, DistillBudget::FULL).chunks.len();
        assert_eq!(chunks, DISTILL_MAX_CHUNKS, "这份输入本就该顶到分片上限");
        assert_eq!(
            estimate_output_tokens(&long, false, DistillBudget::FULL),
            (chunks as u64 + 1) * u64::from(DISTILL_SUMMARY_OUTPUT_TOKENS)
        );

        // 整理稿是**成倍**的代价（分片更小、逐片改写），估算必须把它算进去——
        // 漏算它等于对最贵的那条路径不设防，而用户那篇失败的正是「附整理稿」。
        assert!(
            estimate_output_tokens(&long, true, DistillBudget::FULL)
                > estimate_output_tokens(&long, false, DistillBudget::FULL),
            "带整理稿的估算必须更大"
        );

        // 下界性质：估出来的调用数不得超过真实调用数。真实值 = 摘要片数(+1) + 改写片数。
        let rewrite_chunks = split_chunks(
            &cover(&long, DistillBudget::FULL).text,
            DISTILL_REWRITE_CHUNK_CHARS,
        )
        .len();
        let ceiling = (chunks as u64 + 1) * u64::from(DISTILL_SUMMARY_OUTPUT_TOKENS)
            + rewrite_chunks as u64 * u64::from(DISTILL_REWRITE_OUTPUT_TOKENS);
        assert_eq!(
            estimate_output_tokens(&long, true, DistillBudget::FULL),
            ceiling
        );
    }

    /// **预检必须排在入队之前**（D7′ ①）。
    ///
    /// 「入队后再判」与「不判」对用户是同一件事：面板里照样先冒出一件任务、几秒后变红。
    /// 这条特性的全部价值就在这个顺序上，故它得有个会变红的东西守着。
    #[test]
    fn the_quota_precheck_runs_before_anything_is_queued() {
        let code: String = crate::ipc::source_without_tests(include_str!("distill.rs"))
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let body = code
            .split("pub(crate) fn enqueue_distill(")
            .nth(1)
            .expect("找不到 enqueue_distill");
        let body = body.split("\npub(crate) ").next().unwrap_or(body);

        let check = body.find("ledger.charge(").expect("入队路径上没有额度预检");
        let enqueue = body
            .find("state.processing.enqueue(")
            .expect("找不到入队点");
        assert!(
            check < enqueue,
            "预检跑在入队之后——那与不判是同一件事：面板里照样先冒出一件任务、几秒后变红"
        );

        // 读不到余量时连输入装配都不做：那趟装配要读边车段落，批量里是每篇一次文件 IO。
        assert!(
            body.contains("if ledger.remaining.is_some()"),
            "没有余量时仍在白算估算"
        );

        // 批量整批共用**一份**账本：每篇各开一份就退回逐篇比对，累减当场失效。
        let batch = code
            .split("pub(crate) async fn distill_batch_start(")
            .nth(1)
            .expect("找不到批量入口");
        let batch = batch.split("\n#[tauri::command]").next().unwrap_or(batch);
        assert_eq!(
            batch.matches("QuotaLedger::new(").count(),
            1,
            "批量里的账本不止一份——每篇一份等于没有累减"
        );
        // 拒绝理由不许被丢掉：`Err(reason)` 必须被收进结果，而不是落进一个布尔比较。
        assert!(
            batch.contains("Err(reason) => rejected.push("),
            "批量又把每一篇的拒绝理由丢了——用户拿回一句「已加入 N 件」，其余的去哪了没人说得出"
        );
        assert!(
            !batch.contains("== Ok(true)"),
            "批量退回了「只数 queued」的写法：`enqueue_distill(..) == Ok(true)` 会把 Err 整个吃掉"
        );

        // 自动档走的是同一道门：它花的是同一份额度，且没有人在等着看结果。
        let auto = code
            .split("pub(crate) fn spawn_distill_if_configured(")
            .nth(1)
            .expect("找不到自动档入口");
        let auto = auto.split("\n/// ").next().unwrap_or(auto);
        assert!(
            auto.contains("cloud_output_headroom(&app).await"),
            "自动档绕过了预检"
        );

        // 取余量那一趟必须有超时：`CloudClient` 的 HTTP 客户端没有默认请求超时，
        // 而它现在挡在「点整理」前面——网络被黑洞掉时的表现是按钮按下去没反应，
        // 那正是这条特性最不该变成的东西。
        let probe = code
            .split("async fn cloud_output_headroom(")
            .nth(1)
            .expect("找不到余量探测");
        let probe = probe.split("\nfn ").next().unwrap_or(probe);
        assert!(
            probe.contains("tokio::time::timeout(DISTILL_QUOTA_PROBE_TIMEOUT"),
            "余量探测没有超时——一次黑洞网络会把「整理」按钮变成没反应"
        );
    }

    #[test]
    fn relative_link_walks_up_and_down() {
        assert_eq!(
            relative_link("Notes/会议 · 纪要.md", "Recordings/会议.md"),
            "../Recordings/会议.md"
        );
        assert_eq!(relative_link("Notes/a.md", "Notes/b.md"), "b.md");
        assert_eq!(relative_link("a.md", "Recordings/b.md"), "Recordings/b.md");
    }

    #[test]
    fn md_link_escapes_spaces_and_parens() {
        assert_eq!(
            encode_md_link("../Recordings/会议 (1).md"),
            "../Recordings/会议%20%281%29.md"
        );
    }

    /// 汇总入口：整份输入在**同一次加锁**里定死，任何一条不合格即整体拒绝（TDD-07 §2.1）。
    ///
    /// 读源码：它是 `async` 且要真 `AppHandle`。**只保留源码里才看得见的那几条**——命令层
    /// 的政策（整体拒绝 / 条数按去重后算 / 快照的加锁次数 / 文案走本地化）。凡是能用真库
    /// 跑出来的，一律不在这里写字符串断言：
    /// - 去重按对象身份、保序 → `luna_vault::merge::merge_source_tests`
    ///   （`dedup_collapses_aliases_and_keeps_the_order_the_user_saw` / `an_in_vault_alias_counts_once`）；
    /// - 越界 / 软链逃逸 / 非 Markdown / 空正文 / 按类型取输入 / 指纹变动 →
    ///   `luna_vault::open::tests` 与 `merge_source_tests`；
    /// - 复验 / 落点 / 原子创建 / 占了就放弃 → 本文件的 `publish_merge_locked` 真库测试。
    #[test]
    fn merge_notes_freezes_one_snapshot_and_rejects_the_whole_selection() {
        // 入口：只做诚实前置（空选择 / 资格 / 条数）然后**入队**，自己不跑模型。
        let entry = function_body("pub(crate) async fn merge_notes(", "\n/// ");
        assert!(entry.contains("paths: Vec<String>"));
        assert!(entry.contains("MERGE_EMPTY_SELECTION"));
        assert!(entry.contains("resolve_selected_sources("));
        assert!(entry.contains("check_selection_size("));
        assert!(
            entry.contains("ProcTaskKind::Merge") && entry.contains("MergeOrigin::Selection"),
            "多选汇总又绕开处理队列内联跑了——面板里看不见、取消不了、还和蒸馏抢引擎"
        );
        assert!(
            entry.contains("emit_processing_queue(")
                && entry.contains("start_processing_worker_if_idle("),
            "入队后没有立即广播 + 拉起 worker（与 `enqueue_distill` 同一纪律）"
        );
        assert!(
            !entry.contains("merge_notes_into_one("),
            "入口不该直接调内核——那正是「队列里看不到它」的来源"
        );

        // 执行体：输入快照在**一次加锁**里取，逐条勾选的那一臂整体拒绝、绝不静默跳过。
        let code = function_body("pub(crate) async fn run_merge(", "\n/// 一次汇总的完整请求");
        assert!(code.len() > 600, "剥掉注释后扫描面为空");

        // ② **一次加锁**：解析 / 资格 / 正文 / 指纹 / **库代际** / **落点目录**全在同一
        // 个临界区里取。两次读之间切一次库，第二次读到的就是另一个库里的同名文件；而代际
        // 若晚一步取，取到的就是新库的值，与旧库的 sources 一比恒相等——那道检查从此没有
        // 判别力，却看起来仍然在那儿。
        assert_eq!(
            code.matches("state.vault.lock()").count(),
            1,
            "输入快照被拆成了两次加锁"
        );
        let snapshot = code
            .split("state.vault.lock()")
            .nth(1)
            .and_then(|rest| rest.split("merge_notes_into_one(").next())
            .expect("找不到输入快照那一段");
        assert!(
            snapshot.contains("state.vault_generation()"),
            "库代际没和 sources 在同一个临界区里取"
        );
        assert!(
            snapshot.contains("note_type_dir("),
            "落点目录没和 sources 在同一个临界区里取"
        );
        assert_eq!(
            code.matches("state.vault_generation()").count(),
            1,
            "代际被取了不止一次——只该有快照那一次"
        );

        // ③ 逐条勾选那一臂：解析与条数都走入口用的**同一对**共享函数（入队时判过一次的
        // 判据，执行时按新快照原样再判一次——两处各写一份就会各漂各的）。
        let selection_arm = code
            .split("MergeOrigin::Selection =>")
            .nth(1)
            .and_then(|rest| rest.split("MergeOrigin::Tag").next())
            .expect("找不到逐条勾选那一臂");
        assert!(selection_arm.contains("resolve_selected_sources("));
        assert!(selection_arm.contains("check_selection_size("));
        for lenient in ["filter_map", ".ok()", "continue"] {
            assert!(
                !selection_arm.contains(lenient),
                "逐条勾选的入口开始跳过不合格的源了：{lenient}"
            );
        }
        // 共享函数本身：按**对象身份**去重（唯一实现在 `luna_vault::dedup_by_object`）；
        // 条数按**去重后**算；资格走共享谓词且**整体拒绝**——不静默跳过、不取前 N 条。
        // 候选集自 2026-09-02 起只挡聚合页（PRD-06-047 修订），故不再有
        // 「不是待整理内容」那一码——判定仍在同一个共享谓词里。
        let resolve = function_body("fn resolve_selected_sources(", "\n/// ");
        assert!(resolve.contains("luna_vault::dedup_by_object(sources)"));
        assert!(
            !resolve.contains("source.rel.clone()"),
            "又开始按相对路径字符串去重了"
        );
        assert!(resolve.contains("luna_vault::merge_block("));
        assert!(!resolve.contains("MERGE_NOT_DRAFT"));
        assert!(resolve.contains("MERGE_SELECTION_INVALID"));
        assert!(resolve.contains("MERGE_SOURCE_EMPTY"));
        for lenient in ["filter_map", ".ok()", "continue"] {
            assert!(
                !resolve.contains(lenient),
                "逐条勾选的解析开始跳过不合格的源了：{lenient}"
            );
        }
        let size = function_body("fn check_selection_size(", "\n/// ");
        assert!(size.contains("< MERGE_NOTE_MIN"));
        assert!(size.contains("> MERGE_NOTE_LIMIT"));
        assert!(
            !entry.contains("paths.len() <"),
            "条数还在按去重前的提交条数算"
        );

        // ④ 逐条勾选不接受"只覆盖了前一部分"。
        assert!(code.contains("CoveragePolicy::RejectPartial"));

        // ⑤ 执行仍走唯一内核；产物名与全部说明都走后端本地化资源——且不再叫「速记整理」：
        // 候选集放开之后，合三篇录音出来的那一篇标成"速记"就是对用户说谎。
        assert!(code.contains("merge_notes_into_one("));
        assert!(code.contains("backend_text(app, \"merge-stem\""));
        assert!(
            !code.contains("starts_with(\"zh\")"),
            "又在 IPC 里按语言手写两套文字了"
        );
    }

    /// 剥掉注释后的本文件某个函数体（源码结构断言的共用取样口）。
    fn function_body(marker: &str, end: &str) -> String {
        function_body_of(include_str!("distill.rs"), marker, end)
    }

    /// 同上，但取样面由调用方给（跨文件的回归守护要扫 `ingest.rs` / `processing.rs`）。
    fn function_body_of(source: &str, marker: &str, end: &str) -> String {
        let body_src = crate::ipc::source_without_tests(source);
        let body = body_src
            .split(marker)
            .nth(1)
            .unwrap_or_else(|| panic!("找不到 {marker}"))
            .split(end)
            .next()
            .unwrap_or_else(|| panic!("{marker} 的边界变了"));
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(code.len() > 400, "剥掉注释后扫描面为空：{marker}");
        code
    }

    /// 窗不小于全额档时一字不动（服务端与外部端点始终走这里）；更小的窗等比缩，
    /// 输入占窗的比例不变——12k 字 / 16k 窗那组比例是真机验证过的。
    #[test]
    fn distill_budget_scales_with_the_loaded_window() {
        assert_eq!(DistillBudget::for_window(16_384), DistillBudget::FULL);
        assert_eq!(DistillBudget::for_window(32_768), DistillBudget::FULL);

        let half = DistillBudget::for_window(8_192);
        assert_eq!(half.chunk_chars, DISTILL_CHUNK_CHARS / 2);
        assert_eq!(half.reduce_chars, DISTILL_REDUCE_CHARS / 2);
        assert_eq!(half.rewrite_chunk_chars, DISTILL_REWRITE_CHUNK_CHARS / 2);
        assert_eq!(half.summary_output, DISTILL_SUMMARY_OUTPUT_TOKENS / 2);
        assert_eq!(half.rewrite_output, DISTILL_REWRITE_OUTPUT_TOKENS / 2);

        // 中文最坏约 1 token / 字（S16）：缩放后的分片仍须装得进窗，否则引擎当场拒绝。
        for window in [4_096, 6_144, 8_192, 12_288] {
            let budget = DistillBudget::for_window(window);
            assert!(budget.chunk_chars < window as usize, "{window}");
            assert!(budget.reduce_chars < window as usize, "{window}");
            assert!(
                budget.rewrite_chunk_chars + budget.rewrite_chunk_chars < window as usize,
                "{window}"
            );
        }
    }

    /// 窗小时分片更多、覆盖边界照实标注，而不是把超窗的分片原样送去被拒。
    #[test]
    fn a_smaller_window_covers_less_but_says_so() {
        // 17 字 × 6000 = 10.2 万字：全额档（12 片 × 1.2 万）装得下，半窗（12 × 6000）装不下。
        let text = "这是用来测试蒸馏覆盖边界的一句话。".repeat(6_000);
        let full = cover(&text, DistillBudget::FULL);
        let small = cover(&text, DistillBudget::for_window(8_192));
        assert!(!full.dropped);
        assert_eq!(small.chunks.len(), DISTILL_MAX_CHUNKS);
        assert!(small.text.chars().count() < full.text.chars().count());
        assert!(small.dropped);
        assert!(small
            .chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= DistillBudget::for_window(8_192).chunk_chars));
    }

    /// 汇总内核：**发布全在一次写锁里**，且它一件事都不自己补读（TDD-07 §2.1 / ADR-025）。
    ///
    /// 这条只锁**源码里才看得见的那几件事**：加锁次数、代际与落点目录的来源、
    /// `created_new` 是显式传入的、以及"不删原笔记"。真实行为（复验拦不拦得住、落点被占
    /// 会不会换名字写下去、产物长什么样）由下面 `publish_merge_locked` 的真库测试证。
    #[test]
    fn merge_kernel_publishes_inside_exactly_one_write_lock() {
        let code = function_body(
            "pub(crate) async fn merge_notes_into_one(",
            "\n/// 汇总产物的最终正文",
        );

        // ① 覆盖预算超了：`RejectPartial` 的入口在**发 Job 之前**返回，不是跑完再说。
        let before_job = code.split("summarize_long(").next().unwrap_or("");
        assert!(before_job.contains("MERGE_INPUT_TOO_LARGE"));
        assert!(before_job.contains("cover(&input, budget)"));
        // 输入取快照里已按来源类型选好的那一段（口径与真实行为测试同源，见 luna-vault）。
        assert!(before_job.contains("src.input"));

        // ② **发布只取一次 vault 锁**。修复前是两次：落点在第一次里挑、写盘在第二次里
        // 做，中间还夹着一次锁外的 intent 登记——每一道缝里都能挤进另一次写入。
        let after_job = code.split("summarize_long(").nth(1).unwrap_or("");
        assert_eq!(
            after_job.matches("state.vault.lock()").count(),
            1,
            "发布路径又被拆成了不止一次加锁"
        );

        // ③ **代际与落点目录都来自请求，内核一件都不自己补读**。补读代际会取到新库的
        // 值（与旧库的 sources 一比恒相等）；补读目录会挑等待之后那份配置里的目录。
        assert!(after_job.contains("still_current_vault(req.generation)"));
        assert!(
            !code.contains("state.vault_generation()"),
            "内核又自己补读库代际了"
        );
        assert!(!code.contains("note_type_dir("), "内核又自己补读落点目录了");
        assert!(after_job.contains("DISTILL_VAULT_SWITCHED"));
        assert!(after_job.contains("MERGE_SOURCE_CHANGED"));

        // ④ **intent 走纯登记版**（锁内登记；`std::sync::Mutex` 不可重入，调取锁那版
        // 就是当场死锁），且 `created_new` 显式传入而不是靠回读推断。
        assert!(code.contains("prepare_knowledge_write_locked("));
        assert!(
            !code.contains("prepare_knowledge_write(app"),
            "锁里调了会自己取 vault 锁的那一版 = 死锁"
        );
        let flat = after_job.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("/* created_new */ true"),
            "`created_new` 不再是显式传入的 true：{flat}"
        );

        // ⑤ 发布走唯一的那条路，且落点只挑一次。
        assert_eq!(after_job.matches("publish_merge_locked(").count(), 1);
        assert!(
            !code.contains("unique_note_path_in("),
            "落点又在内核里另挑了一次"
        );

        // ⑥ 落点被占与"源变了"共用一个码（§2.1 的错误清单是封闭的），说明文案各说各的。
        // 压平空白再比：`cargo fmt` 会按行宽把同一个调用折成多行，逐字比原文的断言会在
        // 一次纯格式化之后无缘无故变红。
        assert!(
            flat.contains("coded_error(app, \"MERGE_SOURCE_CHANGED\", \"merge-target-taken\")")
                || flat.contains(
                    "coded_error( app, \"MERGE_SOURCE_CHANGED\", \"merge-target-taken\", )"
                ),
            "{flat}"
        );

        // ⑦ 原笔记只被读，一篇都不删。
        for destructive in ["vault.delete", "vault.move_", "remove_file", "DeleteMode"] {
            assert!(
                !code.contains(destructive),
                "汇总内核里出现了破坏性操作：{destructive}"
            );
        }
    }

    /// 发布原语的源码侧不变量：顺序固定、落点只挑一次、不写蒸馏指针。
    #[test]
    fn the_publish_primitive_keeps_the_one_order_it_is_allowed_to_have() {
        let code = function_body("pub(crate) fn publish_merge_locked<I>(", "\n/// 蒸馏触发档");
        // 顺序：复验 → 挑落点 → 渲染 → 登记 intent → 原子创建。位置一错，"占了就整体放弃"
        // 与"记的是一次新建"两条都不再成立。
        let steps = [
            "merge_sources_unchanged(",
            "unique_note_path_in(",
            "render_body(",
            "reserve_intent(",
            "publish_new(",
        ];
        let mut at = 0usize;
        for step in steps {
            let found = code[at..]
                .find(step)
                .unwrap_or_else(|| panic!("发布顺序里少了一步：{step}\n{code}"));
            at += found + step.len();
        }
        assert_eq!(code.matches("unique_note_path_in(").count(), 1);
        // 落点被占**绝不换名字写下去**——换名会让刚登记的 intent 指着另一篇笔记。
        assert!(code.contains("VaultError::AlreadyExists(_)"));
        assert!(code.contains("abandon_intent(intent)"));
        assert!(!code.contains("distill_source"), "多来源装不下蒸馏指针");
        // 落点自己也不许被普通 `write` 覆盖出来：唯一的落地动作是 `publish_new`。
        assert!(!code.contains("vault.write("));
        assert!(!code.contains("write_topic("));
    }

    /// **共享内核改完之后的回归**（TDD-07 §2.1 末条）：标签汇总的**候选集政策**一行不变。
    ///
    /// 它们的"这一批"是查询派生的**开放集合**——其中一篇刚被删掉不该让整个动作失败；
    /// 而逐条勾选那条路恰恰相反（少一条就是错）。两个入口共用一个内核，最容易在"顺手统一
    /// 一下"里被抹平的就是这个差别，而抹平之后没有任何东西会变红：标签汇总仍然产出一篇
    /// 笔记，只是少了几条来源，或者干脆整批失败。
    #[test]
    fn the_query_derived_entries_keep_their_own_candidate_policy() {
        let entries = [(
            "标签汇总",
            // 标签汇总的候选集政策长在执行体 `run_merge` 的 `MergeOrigin::Tag` 那一臂
            // 里（入口 `distill_tag_notes` 只按标签取路径并入队）；逐条勾选那一臂的
            // 严格性由 `merge_notes_freezes_one_snapshot_and_rejects_the_whole_selection`
            // 单独盯着，这里看的是整个执行体：宽松政策在、快照纪律与另一处一致。
            function_body("pub(crate) async fn run_merge(", "\n/// 一次汇总的完整请求"),
            // 汇总产出是普通笔记，落笔记归档目录。
            "note_type_dir(",
        )];
        for (label, code, dir_fn) in entries {
            // ① 读不到的那一条**跳过**，不是整体拒绝。
            assert!(
                code.contains("filter_map(") && code.contains(".ok()"),
                "{label}的候选集变成整体拒绝了"
            );
            // ② 超预算时如实标注"只覆盖了前一部分"，不在发 Job 前拒绝。
            assert!(
                code.contains("CoveragePolicy::AnnotatePartial"),
                "{label}换了覆盖政策"
            );
            // ③ 输入快照的纪律与逐条勾选那条路相同（两处必须一致）。
            assert_eq!(
                code.matches("state.vault.lock()").count(),
                1,
                "{label}的输入快照被拆成了不止一次加锁"
            );
            assert!(code.contains("state.vault_generation()"), "{label}");
            assert!(code.contains(dir_fn), "{label}的落点目录来源变了");
            assert!(code.contains("luna_vault::dedup_by_object("), "{label}");
        }
    }

    /// 来源区由 Rust 确定性拼接（PRD-02-055），链接相对**落点**，故必须在挑完落点之后渲染。
    #[test]
    fn the_sources_section_is_assembled_by_rust_relative_to_the_target() {
        let code = function_body("fn merge_final_body(", "\n/// 一次汇总发布的输入");
        assert!(code.contains("encode_md_link(&relative_link(target, &src.rel))"));
        assert!(code.contains("backend_text(app, \"merge-sources-heading\""));
    }

    // ── 发布路径的真库行为测试 ──────────────────────────────────────────────
    //
    // `merge_notes_into_one` 本身要真 `AppHandle`（本仓没有、也不为此造一套 mock），
    // 但发布这一段与 `AppHandle` 只有两处交集——登记与放弃 durable intent——故它们以闭包
    // 传入，剩下的整条路可以用真文件、真并发验。下面每一条对应一类此前只靠"读源码"看着的
    // 事实。

    fn publish_fixture() -> (tempfile::TempDir, Vault, Vec<luna_vault::MergeSource>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path()).expect("open vault");
        for name in ["甲", "乙"] {
            vault
                .write(
                    &format!("{name}.md"),
                    &format!("---\nluna:\n  type: note\n  stage: draft\n---\n\n{name}的正文\n"),
                )
                .expect("写源笔记");
        }
        let sources = vec![
            vault.merge_source("甲.md").expect("甲"),
            vault.merge_source("乙.md").expect("乙"),
        ];
        (dir, vault, sources)
    }

    /// 记下闭包被调了几次（intent 的登记 / 放弃在测试里没有别的观测口）。
    #[derive(Default)]
    struct IntentLog {
        reserved: std::cell::Cell<usize>,
        abandoned: std::cell::Cell<usize>,
    }

    fn publish(
        vault: &Vault,
        sources: &[luna_vault::MergeSource],
        log: &IntentLog,
        before_link: impl FnOnce(),
    ) -> Result<(String, String), MergePublishError> {
        publish_merge_locked(
            vault,
            &MergePublish {
                sources,
                dir: "Notes",
                stem: "汇总",
            },
            |target| format!("模型正文\n\n## 来源\n\n- [甲]({target})\n"),
            |target, _body| {
                log.reserved.set(log.reserved.get() + 1);
                before_link();
                Ok(target.to_owned())
            },
            |_intent| log.abandoned.set(log.abandoned.get() + 1),
        )
    }

    #[test]
    fn a_normal_publish_creates_exactly_one_note_and_keeps_every_source() {
        let (_dir, vault, sources) = publish_fixture();
        let log = IntentLog::default();
        let (target, intent) = publish(&vault, &sources, &log, || {}).expect("发布");
        assert_eq!(target, "Notes/汇总.md");
        assert_eq!(intent, target, "intent 登记的就是真正落地的那一篇");
        assert_eq!((log.reserved.get(), log.abandoned.get()), (1, 0));
        let on_disk = vault.read(&target).expect("读回").content;
        assert!(on_disk.contains("模型正文"), "{on_disk}");
        assert!(on_disk.contains("schema:"), "frontmatter 走统一渲染");
        // 原笔记一篇都没少、一个字节都没动。
        for name in ["甲", "乙"] {
            assert!(vault
                .read(&format!("{name}.md"))
                .expect("源还在")
                .content
                .contains(&format!("{name}的正文")));
        }
        assert!(vault.merge_sources_unchanged(&sources));
    }

    /// 落点已被占 → **换个名字**，绝不覆盖既有笔记。
    #[test]
    fn an_occupied_name_is_stepped_over_never_overwritten() {
        let (_dir, vault, sources) = publish_fixture();
        vault
            .write("Notes/汇总.md", "---\nluna:\n---\n\n用户已有的一篇\n")
            .expect("先占住");
        let log = IntentLog::default();
        let (target, _) = publish(&vault, &sources, &log, || {}).expect("发布");
        assert_eq!(target, "Notes/汇总-1.md");
        assert!(vault
            .read("Notes/汇总.md")
            .expect("原件还在")
            .content
            .contains("用户已有的一篇"));
    }

    /// **挑名字与原子创建之间那个窗口**：窗口里被别的写入占了，整体放弃——不换名字
    /// 写下去（换名会让刚登记的 intent 指着另一篇笔记，知识生命周期随后作用在错的对象上）。
    ///
    /// 用 `reserve_intent` 闭包制造这次交错：它恰好跑在"渲染完、还没 link"那一刻。
    #[test]
    fn losing_the_race_for_the_target_abandons_the_whole_publish() {
        let (_dir, vault, sources) = publish_fixture();
        let log = IntentLog::default();
        let root = vault.root().to_path_buf();
        let outcome = publish(&vault, &sources, &log, || {
            std::fs::create_dir_all(root.join("Notes")).expect("建目录");
            std::fs::write(root.join("Notes/汇总.md"), "别人抢先落地的一篇").expect("抢占");
        });
        assert_eq!(outcome.unwrap_err(), MergePublishError::TargetTaken);
        assert_eq!(
            (log.reserved.get(), log.abandoned.get()),
            (1, 1),
            "intent 登记了却没被放弃"
        );
        // 抢先落地的那一篇一个字节没动，且**没有**冒出第二篇。
        assert_eq!(
            std::fs::read_to_string(vault.root().join("Notes/汇总.md")).expect("读回"),
            "别人抢先落地的一篇"
        );
        assert!(vault.path_is_free("Notes/汇总-1.md"), "改名写下去了");
        let leftovers = std::fs::read_dir(vault.root().join("Notes"))
            .expect("列目录")
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "临时文件没清干净");
    }

    /// **等待期间源被改 / 被沉淀 → 复验拦下，磁盘一个字节没动、intent 一条没登记。**
    #[test]
    fn a_source_that_changed_during_the_wait_stops_the_publish_before_anything_is_written() {
        for mutate in [
            (|v: &Vault| {
                v.write("甲.md", "---\nluna:\n  type: note\n---\n\n等待期间改的\n")
                    .expect("改稿");
            }) as fn(&Vault),
            |v: &Vault| {
                v.promote_to_knowledge("乙.md").expect("沉淀");
            },
            |v: &Vault| {
                std::fs::remove_file(v.root().join("乙.md")).expect("删掉");
            },
        ] {
            let (_dir, vault, sources) = publish_fixture();
            mutate(&vault);
            let log = IntentLog::default();
            let outcome = publish(&vault, &sources, &log, || {});
            assert_eq!(outcome.unwrap_err(), MergePublishError::SourceChanged);
            assert_eq!(
                (log.reserved.get(), log.abandoned.get()),
                (0, 0),
                "复验之前就不该登记 intent"
            );
            assert!(vault.path_is_free("Notes/汇总.md"), "产物被发布出去了");
        }
    }

    /// intent 登记失败 → 磁盘上什么都不留（写盘前先有 durable intent，TDD-06 §3.7.1）。
    #[test]
    fn a_failed_intent_leaves_nothing_behind() {
        let (_dir, vault, sources) = publish_fixture();
        let outcome = publish_merge_locked(
            &vault,
            &MergePublish {
                sources: &sources,
                dir: "Notes",
                stem: "汇总",
            },
            |_target| "模型正文".to_owned(),
            |_target, _body| Err::<String, _>("KNOWLEDGE_INTENT_PERSIST_FAILED: 磁盘满".to_owned()),
            |_intent| unreachable!("没登记成功就没有要放弃的"),
        );
        assert_eq!(
            outcome.unwrap_err(),
            MergePublishError::Failed("KNOWLEDGE_INTENT_PERSIST_FAILED: 磁盘满".to_owned())
        );
        assert!(vault.path_is_free("Notes/汇总.md"));
    }

    /// 本模块经 `coded_error` 发出的机器码是**封闭清单**（TDD-07 §2.1）。
    ///
    /// 码是**契约**：调用方（前端结果条、日志聚类、将来的重试策略）按它分支。多一个
    /// 码就是多一条所有调用方都要跟着加的分支，而新码往往只是"同一件事的另一种说法"——
    /// `MERGE_TARGET_TAKEN` 就是这么冒出来的：它与"源变了"对调用方是同一个契约（落盘那
    /// 一刻库已经不是发起时那样了，重试即可），故收进 `MERGE_SOURCE_CHANGED`，只把给人
    /// 看的那半句分成两条文案。
    ///
    /// 反过来：说明文案可以按需要细分，**码不行**。这条守护只锁码。
    ///
    /// **判定面从「汇总这一族」扩到了整个模块**（W2 第二批）：它一直是「凡是经
    /// `coded_error` 发出去的码」，而此前只有汇总那一族走这条路，于是读起来像是只管汇总。
    /// 蒸馏其余入口迁过来之后，同一段提取自然覆盖了它们——这不是判据变松，是判据面变全了。
    /// 这一点当场兑现过：第三批把 `summarize_once` 那条链迁进 `coded_error_with` 之后，
    /// 本条立刻变红，把 `DISTILL_EMPTY` / `DISTILL_JOB` / `DISTILL_CANCELLED` 逼进了清单。
    #[test]
    fn distill_error_codes_stay_the_closed_set_the_contract_declares() {
        let src = crate::ipc::source_without_tests(include_str!("distill.rs"));
        let mut found = std::collections::BTreeSet::new();
        // 只认真正发出去的那一处：`coded_error*(...)` 的第二个入参（码）。
        // `_with` 变体必须一并认：漏掉它等于给「换个取用来源就绕过契约」开一扇门，
        // 而本模块的分片摘要那条链正是走它（它只持 `&AppState`，见 `BackendCopy`）。
        for (marker, skip) in [
            ("coded_error(", 1usize),
            ("coded_error_with(", 1usize),
            ("coded_error_args(", 1usize),
        ] {
            for chunk in src.split(marker).skip(skip) {
                let Some(quoted) = chunk.split('"').nth(1) else {
                    continue;
                };
                if quoted
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                    && quoted.len() > 3
                {
                    found.insert(quoted.to_owned());
                }
            }
        }
        let declared: std::collections::BTreeSet<String> = [
            // §2.1 保留的既有前缀（`MERGE_NOT_DRAFT` 随 2026-09-02 的候选集放开退役：
            // 成熟度不再参与汇总资格，那一码没有任何路径会再产生它）。
            "MERGE_EMPTY_SELECTION",
            // §2.1 新定义的其余几条。
            "MERGE_SELECTION_TOO_SMALL",
            "MERGE_SELECTION_TOO_LARGE",
            "MERGE_SELECTION_INVALID",
            "MERGE_SOURCE_EMPTY",
            "MERGE_INPUT_TOO_LARGE",
            "MERGE_SOURCE_CHANGED",
            "DISTILL_VAULT_SWITCHED",
            // 内核在候选集为空时的既有码（三个入口共用，不属于选择态错误）。
            "DISTILL_MERGE_EMPTY",
            // 起跑线预检的额度缺口（PRD-13-040 ②）。它与上面几条同走 `coded_error_args`
            // ——正是"不走它"让这句话曾经是一段 `format!` 出来的中文。
            "DISTILL_QUOTA_SHORT",
            // 「生成知识笔记」与批量整理的其余入口（W2 第二批迁进 `coded_error`）。
            "DISTILL_INELIGIBLE",
            "DISTILL_EDITED",
            "DISTILL_EMPTY_SOURCE",
            "DISTILL_BUSY",
            "DISTILL_NO_PREVIEW",
            "DISTILL_TEMPLATE_LIMIT",
            "DISTILL_BATCH_EMPTY",
            "PROMOTE_NOT_QUICK_NOTE",
            // 升级的逆操作（PRD-06-058 验收「可撤销反馈」）：只有普通笔记能撤回成速记。
            "DEMOTE_NOT_NOTE",
            // 分片摘要那条链（W2 第三批经 `coded_error_with` 迁进来，它只持 `&AppState`）。
            "DISTILL_EMPTY",
            "DISTILL_JOB",
            "DISTILL_CANCELLED",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            found, declared,
            "本模块的错误码偏离了 TDD-07 §2.1 的封闭清单——多出来的那个多半只是同一件事的另一种说法"
        );
    }

    /// 带参数的两族：**参数契约**要真的成立（id 在不在已由全 IPC 层那道门禁机器判定）。
    ///
    /// **手抄 key 列表已退役**：这里此前逐条列着本模块用到的每个 key，而那正是它自己
    /// 头注记着的那个教训的第二种形态——门禁曾写成 `filter(|id| id.starts_with("merge-"))`，
    /// 于是后加的 key 一个都不在判定面上，`DISTILL_QUOTA_SHORT` 被 `format!` 拼成中文一路
    /// 绿灯上线。key 从 3 个模块扩散到全 IPC 层之后，手抄清单必然漏，且漏的方式还是同一个。
    /// 判定面因此改由源码给出：见 `crate::ipc_error_copy` 的
    /// `every_backend_copy_key_used_in_ipc_exists_in_the_bundle`。
    ///
    /// 留在这里的只有那道门禁**判不了**的一半：带 `{ $arg }` 的消息不给参数取不到，
    /// 而「参数名对不对、门槛有没有被写死」是本模块自己的契约。
    #[test]
    fn the_parameterised_messages_this_module_sends_really_take_their_arguments() {
        for language in ["zh-CN", "en-US"] {
            let i18n = crate::i18n::BackendI18n::new(language);
            assert!(
                i18n.translate_args("merge-selection-too-small", &[("min", "2".into())])
                    .is_some(),
                "{language} 缺少 merge-selection-too-small"
            );
            assert!(
                i18n.translate_args("merge-selection-too-large", &[("max", "200".into())])
                    .is_some(),
                "{language} 缺少 merge-selection-too-large"
            );
            assert!(
                i18n.translate_args(
                    "distill-quota-short",
                    &[
                        ("need", "1".into()),
                        ("remaining", "2".into()),
                        ("short", "3".into()),
                    ],
                )
                .is_some(),
                "{language} 缺少 distill-quota-short"
            );
        }
        // 条数门槛只能靠参数插进去：写死 2 / 200 就是同一对常量的第三处定义。
        let zh = include_str!("../../resources/i18n/zh-CN.ftl");
        let en = include_str!("../../resources/i18n/en-US.ftl");
        assert!(zh.contains("merge-selection-too-small = 至少选择 { $min } 条"));
        assert!(en.contains("{ $max } notes per merge"));
    }
}
