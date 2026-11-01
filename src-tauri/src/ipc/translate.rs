//! 翻译与浮动菜单命令域（PRD-01/PRD-03，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：把选中文本变成一个 `Job` 交给主干（ADR-001），并把事件流镜像回前端。
//! 语言上下文、模板与实时转译配置的解析也在这里。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 经 `ipc::Channel` 发送若干字节帧（Phase 0 高频通道骨架）。
///
/// ⚠ S8：Phase 0 用 `Vec<u8>`（默认 JSON 数字数组）；真·二进制（前端 `ArrayBuffer`）
/// 须改 `InvokeResponseBody::Raw` 并在 GUI 运行时实测（[TDD-07 §2]/[TDD-00 §8]）。
#[tauri::command]
#[specta::specta]
pub(crate) fn start_byte_stream(channel: Channel<Vec<u8>>) -> Result<(), String> {
    for i in 0u8..3 {
        channel
            .send(vec![i, i.wrapping_add(1), i.wrapping_add(2)])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 笔记 AI 悬浮菜单单次输入上限（字符）的**上界**。本地 LLM 上下文有限（随包引擎按档位
/// 8k–16k token，CJK 约 1 字符/token），超窗请求会被引擎拒收、外部端点则可能静默丢弃——
/// 违反 PRD-06-025「不静默截断」，故在提交前显式截断，并经 `TranslateEvent::Truncated`
/// 告知用户质量边界。
///
/// **有效上限由 [`floating_input_limit`] 按当前上下文窗算**，本常量只是它的封顶。
pub(crate) const FLOATING_MENU_INPUT_MAX_CHARS: usize = 12_000;

/// 悬浮菜单**有效**输入上限（字符）：按已探测的上下文窗收窄，未探测时取封顶值。
///
/// 口径刻意保守（1 字符 ≈ 1 token，即 CJK 的最坏情形）：宁可对英文多提示一次「只处理
/// 了前 N 字」，也不让中文用户在没有任何提示的情况下丢掉后半段。留 1/4 给输出与
/// 系统提示——不留的话，输入刚好填满窗时模型一个字都写不出来。
#[must_use]
pub(crate) fn floating_input_limit(ctx_window: u32) -> usize {
    if ctx_window == 0 {
        return FLOATING_MENU_INPUT_MAX_CHARS;
    }
    let usable = (ctx_window as usize / 4) * 3;
    usable.clamp(1, FLOATING_MENU_INPUT_MAX_CHARS)
}

/// 按字符边界截断悬浮菜单输入；返回（截断后文本, 保留字符数——未截断为 `None`）。
pub(crate) fn truncate_floating_input(text: String, limit: usize) -> (String, Option<u32>) {
    if text.chars().count() <= limit {
        return (text, None);
    }
    let kept: String = text.chars().take(limit).collect();
    #[allow(clippy::cast_possible_truncation)]
    (kept, Some(limit as u32))
}

/// 把**本次输入命中**的术语约束注入 `Translate` / `Refine` Job 的 `instruction`
/// （PRD-01-017）；其余 Job 类型原样穿过。
///
/// **弹窗翻译与笔记悬浮菜单共用这一处**，两者再各自经
/// [`luna_core::with_glossary_constraint`] 与实时逐句转译、批量转译走同一个注入函数
/// （TDD-03 §3.3「同一注入函数」）。此前它只住在弹窗模块里，悬浮菜单从头到尾没接过
/// 词典——TDD-03 与 `glossary.rs` 都写着「四条路共用」，而代码只有三条。
///
/// 只注入命中项：全表会让 prompt 膨胀、拖慢本地 LLM 并挤占上下文窗，而没出现的术语
/// 对这段文本毫无约束力。`Refine` 也要注入——润色不译词，但 LLM 会把专有名词「顺手改回」
/// 错误形式，那与译错是同一种损失。
///
/// **结构保真约束（PRD-01-021）不在这里**：`Translate` 与 `Refine` 的 system 模板都带
/// `{structure}` 占位符，由 `luna_provider::llm_http::build_chat_messages` 统一渲染。
///
/// 无命中时 `instruction` 原样返回（空指令仍是 `None`，让 Provider 用通用译文 prompt）。
pub(crate) fn with_job_glossary(
    kind: JobKind,
    source: &str,
    glossary: &[GlossaryEntry],
) -> JobKind {
    let inject = |instruction: Option<Instruction>| {
        luna_core::with_glossary_constraint(
            luna_core::prompt::registry(),
            instruction,
            source,
            glossary,
        )
        .filter(|i| !i.is_empty())
    };
    match kind {
        JobKind::Translate {
            text,
            source: src,
            target,
            instruction,
            template,
        } => JobKind::Translate {
            text,
            source: src,
            target,
            instruction: inject(instruction),
            template,
        },
        JobKind::Refine { text, instruction } => JobKind::Refine {
            text,
            instruction: inject(instruction),
        },
        other => other,
    }
}

/// 构造「优化/润色」指令：**语言无关地锁定输入语言与字形变体**，强制同语改写、禁翻译。
///
/// 根治「点『优化』却把选区改写成译文」：小模型（如 `qwen3:0.6b`）对全英文 system
/// prompt + 非英文输入极易把「优化」退化成「翻译」，只说 "same language" 不足以约束。
///
/// 通用适配（非只中英，避免硬编码）：
/// - **语言点名**——复用已检测源语言，可靠命中即点名其英文全称（`display_name` 覆盖检测器
///   编译进的全部 17 种语言：英/法/日/韩/德/西/俄/意/葡/阿/印地/泰/越/印尼/荷/土/中），
///   与翻译 prompt 同一显式语言策略（小模型对 "into French" 远比 "into fr" 敏感，[TDD-03 §3.3]）；
///   检测不可靠时回退「与输入同语言」。
/// - **字形/书写变体保持**——检测器只给 ISO 639-1（繁简中文同为 `zh`、西里尔/拉丁塞尔维亚语
///   同为 `sr`），故「保持与输入完全相同的书写系统与字形变体」交由**能看见输入字形的 LLM**，
///   而非在 Rust 端引入繁简字符表/转换依赖（那才是硬编码 + License 负担，ADR-005）。此约束
///   对所有语言通用，繁简中文只是最典型的一例。
///
/// 措辞全在注册表的 `editor` 域（ADR-050 修订 2026-09-05）：本模块只准备变量、拼消息。
/// 此前这些整句是 `format!` 字面量，运营改不动、评测覆盖不到。
pub(crate) fn optimize_instruction(
    detection: &Detection,
    context: &FloatingMenuContext,
) -> Instruction {
    editor_instruction(
        "optimize",
        &[
            ("lang_clause", &lang_lock_clause(detection)),
            ("context", &context_clause(context)),
        ],
    )
}

/// 注册表 `editor` 域的一条**片段**模板（语言子句、语气名、上下文条款……），作为变量喂给
/// 最外层那条模板；悬浮菜单指令零字面量的唯一取用口。
fn editor(key: &str, vars: &[(&str, &str)]) -> String {
    luna_core::prompt::registry().render(luna_core::PromptScope::Editor, key, vars)
}

/// 注册表 `editor` 域的一条**整句指令**（能进 `JobKind` 的那一层；类型即约束，ADR-050 ②）。
fn editor_instruction(key: &str, vars: &[(&str, &str)]) -> Instruction {
    luna_core::prompt::registry().instruction(luna_core::PromptScope::Editor, key, vars)
}

/// 「必须留在 X 语」子句：可靠命中即点名全称，否则「与输入同语言」。
fn lang_lock_clause(detection: &Detection) -> String {
    match detection.reliable_lang() {
        Some(lang) => editor("lang_lock.named", &[("lang", lang.display_name())]),
        None => editor("lang_lock.same", &[]),
    }
}

/// 语言 / 字形锁定条款（优化、校对、改语气共用）：点名检测到的语言全称，保持书写变体。
fn language_lock_clause(detection: &Detection) -> String {
    editor(
        "language_lock",
        &[("lang_clause", &lang_lock_clause(detection))],
    )
}

/// 校对指令（PRD-06-021 逐条差异的输入）：**只改错、不改写**，输出改正稿全文；前端拿
/// 改正稿与原文算字符级差异。2026-09-03 用 qwen3.5:4b 跑 20 段中英样本：纯文本改正稿
/// 20/20 干净、25 处错误修对 24 处、过度改写中位数 0；JSON 结构化反而 2/20 解析失败且
/// 慢 4 倍——故这里不要结构化输出。
pub(crate) fn proofread_instruction(detection: &Detection) -> Instruction {
    editor_instruction(
        "proofread",
        &[("language_lock", &language_lock_clause(detection))],
    )
}

/// 改语气指令：保持内容与事实，只换语气；语言 / 字形同样锁定。`tone_key` 是
/// `editor.tone.*` 里的一档（正式 / 轻松 / 简洁），语气措辞本身也是模板。
pub(crate) fn tone_instruction(
    tone_key: &str,
    detection: &Detection,
    context: &FloatingMenuContext,
) -> Instruction {
    let tone = editor(tone_key, &[]);
    editor_instruction(
        "tone",
        &[
            ("tone", &tone),
            ("language_lock", &language_lock_clause(detection)),
            ("context", &context_clause(context)),
        ],
    )
}

/// 视频脚本改稿指令（PRD-19-013/014）：模板只要语言锁与前后文两个子句，同 [`tone_instruction`]。
pub(crate) fn script_instruction(
    key: &str,
    detection: &Detection,
    context: &FloatingMenuContext,
) -> Instruction {
    editor_instruction(
        key,
        &[
            ("language_lock", &language_lock_clause(detection)),
            ("context", &context_clause(context)),
        ],
    )
}

/// 总结指令按风格生成。
///
/// 三档都**显式写出形态**，且语言用全称：此前要点档只写 `Write the summary in zh.`
/// ——Provider 外套的通用前缀只说「concisely」，一个字没提要点，于是「要点」这一档实际
/// 从未被要求过；而 `zh` 这种代码对小模型几乎不起作用（TDD-03 §3.3，同翻译模板策略）。
fn summary_instruction(style: FloatingSummaryStyle, lang: &Lang) -> Instruction {
    let key = match style {
        FloatingSummaryStyle::Bullets => "summary.bullets",
        FloatingSummaryStyle::Paragraph => "summary.paragraph",
        FloatingSummaryStyle::OneLine => "summary.one_line",
    };
    editor_instruction(key, &[("lang", lang.display_name())])
}

/// 会议纪要总结指令（PRD-04-054：录音笔记 + 说话人显名转写输入时替代普通总结指令）。
/// 输入行形如 `显示名: 文本`，要求产出「决议 / 谁负责什么 / 待决问题」结构
/// （对标 Granola / 飞书妙记纪要形态），归属一律用给定说话人名。
fn meeting_minutes_instruction(lang: &Lang) -> Instruction {
    editor_instruction("meeting_minutes", &[("lang", lang.display_name())])
}

/// 选区前后上下文**每侧**上限（字符）。只改选区、前后文仅供参考（PRD-06-020 上下文
/// 策略）：它决定的是「短句润色不脱离语境」，不是「把整篇喂给模型」——本地小模型的窗口
/// 有限，云路径下它还是出域字数，故封顶且用户可关（`editor.floating_context`）。
pub(crate) const FLOATING_MENU_CONTEXT_MAX_CHARS: usize = 300;

/// 续写输入上限（字符）：取光标**前文的尾部**——续写只需要最近的语境，整篇前文只会把
/// 小模型的窗口挤满。
pub(crate) const FLOATING_MENU_CONTINUE_MAX_CHARS: usize = 1_500;

/// 选区前后段落。前端**如实切片**（纯文本，不判定取多少），Rust 在这里裁剪与拼装。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FloatingMenuContext {
    pub before: String,
    pub after: String,
}

impl FloatingMenuContext {
    /// 按每侧上限裁剪：前文取**尾部**（离选区最近），后文取**头部**。
    pub(crate) fn clamped(before: Option<&str>, after: Option<&str>) -> Self {
        Self {
            before: tail_chars(
                before.unwrap_or_default().trim(),
                FLOATING_MENU_CONTEXT_MAX_CHARS,
            ),
            after: head_chars(
                after.unwrap_or_default().trim(),
                FLOATING_MENU_CONTEXT_MAX_CHARS,
            ),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }

    /// 上下文字符数（回执给前端标注出域字数）。
    pub(crate) fn chars(&self) -> usize {
        self.before.chars().count() + self.after.chars().count()
    }
}

fn head_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn tail_chars(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_owned();
    }
    text.chars().skip(total - max).collect()
}

/// 上下文条款：拼在 Refine 指令尾部。明确「仅供参考、不得改写/重复/输出」——否则小模型
/// 会把前后文一起改了再吐出来，替换时正文就重复了。
fn context_clause(context: &FloatingMenuContext) -> String {
    if context.is_empty() {
        return String::new();
    }
    let before = if context.before.is_empty() {
        String::new()
    } else {
        editor("context.before", &[("before", &context.before)])
    };
    let after = if context.after.is_empty() {
        String::new()
    } else {
        editor("context.after", &[("after", &context.after)])
    };
    editor("context.intro", &[("before", &before), ("after", &after)])
}

/// 续写指令（光标处作用域，PRD-06-020）：输入是光标前文，只输出接续内容。语言锁定与
/// 优化指令同一策略（点名全称；检测不可靠时回退「与输入同语言」）。
pub(crate) fn continue_instruction(detection: &Detection) -> Instruction {
    let lang_clause = match detection.reliable_lang() {
        Some(lang) => editor("continue_lang.named", &[("lang", lang.display_name())]),
        None => editor("continue_lang.same", &[]),
    };
    editor_instruction("continue", &[("lang_clause", &lang_clause)])
}

/// 光标处生成的定稿整理：剥掉输出开头与**输入尾部重复**的行。
///
/// 小模型对「不要重复给定文本」并不可靠——续写时常先把光标所在的标题 / 上一行再吐一遍，
/// 插入后正文里就多出一份。判据是确定性的：输出开头若与输入的最后几行（去空白后）逐行
/// 相同，就去掉这几行；最多比对 3 行，比不上即原样返回。
pub(crate) fn strip_leading_overlap(input: &str, output: &str) -> String {
    let tail: Vec<&str> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(3)
        .collect();
    let mut remaining = output;
    // 从离光标最近的那一行开始匹配：输出可能只重复了最后一行，也可能重复了最后两三行
    // （顺序与输入一致）。先找输出开头命中的最远一行，再逐行往回吃。
    for start in (0..tail.len()).rev() {
        let mut probe = output;
        let mut matched = true;
        for line in tail[..=start].iter().rev() {
            if let Some(rest) = probe.trim_start().strip_prefix(line) {
                probe = rest;
            } else {
                matched = false;
                break;
            }
        }
        if matched {
            remaining = probe;
            break;
        }
    }
    remaining.trim_start_matches(['\n', '\r']).to_owned()
}

/// 长文策略（PRD-06-025）：按字符数 × 作用域 × 动作定，提交前与提交时用同一判据。
/// `limit` = 本次的有效输入上限（[`floating_input_limit`]），预检与提交必须传同一个数。
pub(crate) fn floating_menu_strategy(
    chars: usize,
    scope: FloatingMenuScope,
    action: Option<FloatingMenuAction>,
    limit: usize,
) -> (FloatingMenuStrategy, u32) {
    if chars <= limit {
        return (FloatingMenuStrategy::Direct, 0);
    }
    match (scope, action) {
        (FloatingMenuScope::Cursor, _) | (_, Some(FloatingMenuAction::Continue)) => {
            (FloatingMenuStrategy::Tail, 0)
        }
        // 整篇：预检时还不知道会点哪个动作，按主动作「总结」答；其它整篇动作提交时再定。
        (FloatingMenuScope::Doc, None | Some(FloatingMenuAction::Summarize)) => {
            let chunks = chars
                .div_ceil(limit.max(1))
                .min(super::distill::DISTILL_MAX_CHUNKS);
            #[allow(clippy::cast_possible_truncation)]
            (FloatingMenuStrategy::Chunked, chunks as u32)
        }
        _ => (FloatingMenuStrategy::Truncated, 0),
    }
}

/// 提交前预检（PRD-06-025）：告诉指令框「这么长会怎么处理」，用户在点动作之前就看到。
/// 上限按当前上下文窗算（[`floating_input_limit`]），与提交时同一个数；还没探测过就
/// 先补探一次，否则预检按封顶答、提交按真实窗算，两处说的不是同一件事。
#[tauri::command]
#[specta::specta]
pub(crate) async fn floating_menu_preflight(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    chars: u32,
    scope: FloatingMenuScope,
) -> Result<FloatingMenuPreflightDto, String> {
    super::chat::ensure_chat_context_window(&app).await;
    let limit = floating_input_limit(state.chat_ctx_window());
    let (strategy, chunks) = floating_menu_strategy(chars as usize, scope, None, limit);
    #[allow(clippy::cast_possible_truncation)]
    Ok(FloatingMenuPreflightDto {
        limit: limit as u32,
        strategy,
        chunks,
    })
}

/// 建议落地方式（PRD-06-021）：按「动作 × 作用域」定，前端把它摆成主按钮。
///
/// - 选区：优化 / 自定义改写 → **替换**（改的就是这段）；翻译 / 总结 / 续写 → **插入下方**
///   （原文要留着，Notion / Word 同口径）。
/// - 光标处：一律 **插入此处**。
/// - 整篇：总结 → **插入顶部**（摘要放在正文前面才读得到）；其余 → **插入文末**。
pub(crate) fn suggested_landing(
    action: FloatingMenuAction,
    scope: FloatingMenuScope,
) -> FloatingMenuLanding {
    match scope {
        FloatingMenuScope::Cursor => FloatingMenuLanding::InsertCursor,
        FloatingMenuScope::Doc => match action {
            FloatingMenuAction::Summarize => FloatingMenuLanding::InsertTop,
            _ => FloatingMenuLanding::InsertEnd,
        },
        FloatingMenuScope::Selection => {
            if action.rewrites_in_place() {
                FloatingMenuLanding::Replace
            } else {
                FloatingMenuLanding::InsertBelow
            }
        }
    }
}

/// 一次悬浮菜单请求（Job 构造的全部输入；`text` 已完成会议纪要换装与截断）。
pub(crate) struct FloatingMenuRequest<'a> {
    pub text: String,
    pub action: FloatingMenuAction,
    pub scope: FloatingMenuScope,
    /// 仅 `Ask` 使用（自定义指令 / 追问）。
    pub instruction: Option<&'a str>,
    /// 已裁剪的选区上下文；空 = 不带。
    pub context: FloatingMenuContext,
    /// 子菜单选择（翻译目标语言 / 总结风格）；缺省走设置。
    pub options: FloatingMenuOptionsDto,
    pub strict_local: bool,
    pub meeting_minutes: bool,
    /// 术语表（PRD-01-017）：只把**本次输入命中**的词条注入翻译 / 润色指令。
    pub glossary: &'a [GlossaryEntry],
}

pub(crate) fn floating_menu_job(
    i18n: BackendCopy<'_>,
    request: FloatingMenuRequest<'_>,
    settings: &Settings,
    detector: &dyn LanguageDetector,
) -> Result<Job, String> {
    let FloatingMenuRequest {
        text,
        action,
        scope,
        instruction,
        context,
        options,
        strict_local,
        meeting_minutes,
        glossary,
    } = request;
    let ctx = lang_context_from_settings(settings);
    let detection = detector.detect(&text);
    // 术语约束按**原文**命中（截断后的那份正是要送出去的）；`text` 随后移进 Job。
    let source_for_glossary = text.clone();
    let kind = match action {
        FloatingMenuAction::Summarize => {
            let lang = floating_summary_lang(settings, &ctx);
            let style = options.summary_style.unwrap_or_else(|| {
                FloatingSummaryStyle::from_setting(
                    &settings
                        .get_str("editor.floating_summary_style")
                        .unwrap_or_else(|| "bullets".into()),
                )
            });
            JobKind::Summarize {
                text: TextRef::Inline(text),
                // Job 形状如实记录三档（账本与服务端读的是它，不是指令文本）；
                // 会议纪要是结构化要点，归要点档。
                style: if meeting_minutes {
                    SummaryStyle::Bullets
                } else {
                    style.as_summary_style()
                },
                instruction: Some(if meeting_minutes {
                    meeting_minutes_instruction(&lang)
                } else {
                    summary_instruction(style, &lang)
                }),
            }
        }
        FloatingMenuAction::Optimize => JobKind::Refine {
            instruction: Some(optimize_instruction(&detection, &context)),
            text: TextRef::Inline(text),
        },
        FloatingMenuAction::Proofread => JobKind::Refine {
            instruction: Some(proofread_instruction(&detection)),
            text: TextRef::Inline(text),
        },
        FloatingMenuAction::ToneFormal
        | FloatingMenuAction::ToneCasual
        | FloatingMenuAction::ToneConcise => {
            let tone = match action {
                FloatingMenuAction::ToneCasual => "tone.casual",
                FloatingMenuAction::ToneConcise => "tone.concise",
                _ => "tone.formal",
            };
            JobKind::Refine {
                instruction: Some(tone_instruction(tone, &detection, &context)),
                text: TextRef::Inline(text),
            }
        }
        FloatingMenuAction::Continue => JobKind::Refine {
            instruction: Some(continue_instruction(&detection)),
            text: TextRef::Inline(text),
        },
        // 视频脚本的四个改稿动作（PRD-19-013/014）：与改语气同一形状——指令取自 editor 域
        // 的随包 / 目录模板，语言锁与前后文由同一组子句提供。
        FloatingMenuAction::ScriptTighten
        | FloatingMenuAction::ScriptHook
        | FloatingMenuAction::ScriptSpoken
        | FloatingMenuAction::ScriptDictation => {
            let key = match action {
                FloatingMenuAction::ScriptHook => "creator.script.hook",
                FloatingMenuAction::ScriptSpoken => "creator.script.spoken",
                FloatingMenuAction::ScriptDictation => "creator.script.dictation",
                _ => "creator.script.tighten",
            };
            // 脚本改稿也带上设置里的「我的频道风格」（ADR-166 ⑤）：改成口语、重写开头都该像作者本人。
            let mut instruction = script_instruction(key, &detection, &context);
            super::creator_publish::CreatorNotes::new(settings, None).apply(&mut instruction);
            JobKind::Refine {
                instruction: Some(instruction),
                text: TextRef::Inline(text),
            }
        }
        FloatingMenuAction::Translate => {
            // 子菜单点名了目标语言 → 固定目标；否则智能双向（设置）。
            let mode = options
                .target_lang
                .as_deref()
                .filter(|code| !code.is_empty())
                .map_or(TargetLangMode::SmartBidirectional, |code| {
                    TargetLangMode::Fixed(Lang::from(code))
                });
            let branch = decide_branch(&detection, &mode, &ctx);
            compose_job_kind(luna_core::prompt::registry(), &branch, None, text)
        }
        FloatingMenuAction::Ask => {
            let instruction = instruction
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| coded_error_with(i18n, "EMPTY_INSTRUCTION", "empty-instruction"))?;
            // 光标处的自定义指令是**生成**而不是改写：给定文本只是光标前文（语境），
            // 产物是要插进光标处的新文字，绝不能把前文改一遍再吐回来。
            // 单趟渲染（`luna_core::prompt::render`）：用户指令里的花括号不会被二次展开。
            let rule = if scope == FloatingMenuScope::Cursor {
                editor_instruction("ask.cursor", &[("instruction", instruction)])
            } else {
                editor_instruction(
                    "ask.selection",
                    &[
                        ("instruction", instruction),
                        ("context", &context_clause(&context)),
                    ],
                )
            };
            JobKind::Refine {
                text: TextRef::Inline(text),
                instruction: Some(rule),
            }
        }
        // 表格 AI（对标方案 T6）：整理与解释各一趟；结果是否是一张合法的表由调用方校验。
        FloatingMenuAction::TableFromText => JobKind::Refine {
            instruction: Some(editor_instruction("table.from_text", &[])),
            text: TextRef::Inline(text),
        },
        FloatingMenuAction::TableExplain => JobKind::Refine {
            instruction: Some(editor_instruction("table.explain", &[])),
            text: TextRef::Inline(text),
        },
        // 按列填充是逐行多趟（`run_table_fill`），不是一个 Job。
        FloatingMenuAction::TableFillColumn => {
            return Err(coded_error_with(
                i18n,
                "EMPTY_INSTRUCTION",
                "empty-instruction",
            ));
        }
    };
    // 术语表注入（PRD-01-017）：翻译与全部改写类动作；总结原样穿过。
    let kind = with_job_glossary(kind, &source_for_glossary, glossary);

    let mut job = Job::new(kind, JobOrigin::FloatingMenu);
    job.priority = Priority::Interactive;
    job.routing.local_only = strict_local;
    Ok(job)
}

pub(crate) fn lang_context_from_settings(settings: &Settings) -> LangContext {
    LangContext {
        native: Lang::from(
            settings
                .get_str("popup_translate.native_lang")
                .unwrap_or_else(|| "zh".into())
                .as_str(),
        ),
        default_foreign: Lang::from(
            settings
                .get_str("popup_translate.default_foreign")
                .unwrap_or_else(|| "en".into())
                .as_str(),
        ),
    }
}

/// 转译语言基准：母语与弹窗翻译**同源**（`popup_translate.native_lang`，用户母语是单一概念），
/// 默认外语按调用域取不同键——实时与离线是两套配置（PRD-05-031），不共用一个开关。
fn lang_context_with_foreign(settings: &Settings, foreign_key: &str) -> LangContext {
    LangContext {
        native: Lang::from(
            settings
                .get_str("popup_translate.native_lang")
                .unwrap_or_else(|| "zh".into())
                .as_str(),
        ),
        default_foreign: Lang::from(
            settings
                .get_str(foreign_key)
                .unwrap_or_else(|| "en".into())
                .as_str(),
        ),
    }
}

/// **实时**域语言基准：默认外语取 `realtime.translate_target_lang`（录音/字幕面板可现场切）。
fn realtime_lang_context(settings: &Settings) -> LangContext {
    lang_context_with_foreign(settings, "realtime.translate_target_lang")
}

/// **离线**域语言基准：默认外语取 `popup_translate.default_foreign`。
///
/// 离线翻译是**显式动作**（用户在回放视图选目标语言并点触发），只有选「智能双向」时才需要
/// 一个默认外语兜底——那正是用户在翻译域已经设过的那一个，不为离线域再造一份重复设置。
/// （`realtime.translate_target_lang` 之所以独立，是因为录音/字幕面板要**现场热切**。）
pub(crate) fn batch_lang_context(settings: &Settings) -> LangContext {
    lang_context_with_foreign(settings, "popup_translate.default_foreign")
}

/// 构造**实时**域转译配置（智能双向 + 注入检测器，DRY 复用弹窗翻译决策核心）。
///
/// **不再有 `preferred_llm` 参数**（ADR-083 ⑥）：档位由 `Job::service_class()` 按 Job
/// 的形状推导，由路由层按「泳道 → 模型槽位」总映射填 `RoutingHint.preferred_provider`。
/// 此前它由本函数的调用点传 `realtime_llm_provider_id` 下来，而那个 id 只可能是**本机
/// 模型**的——算力一交给服务端，「实时档」这个区分当场消失，设置页却仍显示它生效。
/// 本函数与 `note_translate_config` 的差别从此只剩语言基准域（实时域
/// `realtime.translate_target_lang` vs 离线域 `popup_translate.default_foreign`）。
pub(crate) fn realtime_translate_config(
    settings: &Settings,
    detector: Arc<dyn LanguageDetector>,
    glossary: Arc<[GlossaryEntry]>,
) -> TranslationConfig {
    TranslationConfig::new(
        TargetLangMode::SmartBidirectional,
        realtime_lang_context(settings),
        detector,
    )
    .with_glossary(glossary)
}

pub(crate) fn floating_summary_lang(settings: &Settings, ctx: &LangContext) -> Lang {
    match settings
        .get_str("editor.floating_summary_lang")
        .unwrap_or_else(|| "native".into())
        .trim()
    {
        "" | "native" => ctx.native.clone(),
        "default_foreign" => ctx.default_foreign.clone(),
        lang => Lang::from(lang),
    }
}

/// 提交笔记内 AI 悬浮菜单任务，并复用翻译流事件投影给前端。
///
/// `scope` 是前端如实上报的召唤位置（选区 / 光标 / 整篇）；`instruction` 仅 `Ask` 动作
/// 使用（自定义指令/追问，PRD-06-021）；`context_before` / `context_after` 是选区前后
/// 段落的**原样切片**——取不取、取多少、怎么拼全在这里定（`editor.floating_context`、
/// 每侧上限，ADR-003）。流首先发 `FloatingMenu` 事实（建议落地、执行位置、发送字数），
/// 超长输入再按 `FLOATING_MENU_INPUT_MAX_CHARS` 截断并发 `Truncated`（PRD-06-025）。
/// 进行中任务登记取消令牌（单槽）：新任务顶替旧任务，`floating_menu_stop` 可随时停止。
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)] // 命令签名逐项对应前端如实上报的事实，见 ADR-003
pub(crate) async fn submit_floating_menu(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
    action: FloatingMenuAction,
    scope: FloatingMenuScope,
    instruction: Option<String>,
    note_rel: Option<String>,
    context: Option<FloatingMenuContextDto>,
    options: FloatingMenuOptionsDto,
    on_event: Channel<TranslateEvent>,
) -> Result<(), String> {
    let settings = state.settings_snapshot();
    if action == FloatingMenuAction::TableFillColumn {
        return run_table_fill(
            &app,
            &state,
            &settings,
            &text,
            instruction.as_deref(),
            &options,
            &on_event,
        )
        .await;
    }
    // 会议纪要装配（PRD-04-054）：**整篇总结**目标是录音笔记且转写段带说话人时，输入
    // 换成「显示名: 文本」逐行转写（正文时间戳行没有说话人这一纪要核心结构信号），
    // 指令转「决议/谁负责什么」纪要式。`note_rel` 仅整篇模式由前端携带；普通笔记/
    // 无说话人 → 原文原指令，行为零改变。仍走同一 Summarize Job 主干（ADR-001）。
    let (text, meeting_minutes) = if matches!(action, FloatingMenuAction::Summarize) {
        let i18n_state = backend_copy_arc(&app);
        match note_rel
            .as_deref()
            .and_then(|rel| speaker_labeled_transcript(&state, i18n_state.as_deref(), rel))
        {
            Some(labeled) => (labeled, true),
            None => (text, false),
        }
    } else {
        (text, false)
    };
    // 有效上限与上下文窗提示取同一个探测值：上限按它收窄，Job 按它下发 `num_ctx`——
    // 两者分家的表现是「面板说没截断、模型只读了前一半」。还没探测过就先补探一次。
    super::chat::ensure_chat_context_window(&app).await;
    let limit = floating_input_limit(state.chat_ctx_window());
    let (strategy, planned_chunks) =
        floating_menu_strategy(text.chars().count(), scope, Some(action), limit);
    // 续写只要最近的语境：前文取尾部，不走截断提示（那是「只处理了开头」的语义，
    // 与「只看结尾」相反）。整篇总结超上限走分段摘要（下方），这里不截。
    let (text, truncated) = match strategy {
        FloatingMenuStrategy::Tail => (tail_chars(&text, FLOATING_MENU_CONTINUE_MAX_CHARS), None),
        FloatingMenuStrategy::Chunked => (text, None),
        FloatingMenuStrategy::Direct | FloatingMenuStrategy::Truncated => {
            truncate_floating_input(text, limit)
        }
    };
    // 上下文只给「改这一段」的动作（优化 / 自定义改写）且只在选区作用域；翻译不带
    // （译文要与原文一一对应，前后文只会诱使模型多译）；用户关掉开关即不带。
    let context_enabled = settings.get_bool("editor.floating_context").unwrap_or(true);
    // 校对刻意不带：改正稿要与原文逐字对齐算差异，前后文只会诱使模型多改。
    let context = if context_enabled
        && scope == FloatingMenuScope::Selection
        && action.rewrites_in_place()
        && action != FloatingMenuAction::Proofread
    {
        FloatingMenuContext::clamped(
            context.as_ref().map(|c| c.before.as_str()),
            context.as_ref().map(|c| c.after.as_str()),
        )
    } else {
        FloatingMenuContext::default()
    };
    let availability = crate::cloud::evaluate_for(&app);
    let endpoint = compute_endpoint_source(availability, &state.cloud_server());
    let on_server = !settings.strict_local()
        && chat_runs_on_server(&settings, &endpoint)
        && state.server_serves(action.server_capability());
    #[allow(clippy::cast_possible_truncation)]
    let meta = FloatingMenuMeta {
        suggested_landing: suggested_landing(action, scope),
        on_server,
        sent_chars: (text.chars().count() + context.chars()) as u32,
        context_chars: context.chars() as u32,
        chunks: planned_chunks,
    };
    // 分段摘要（PRD-06-025，整篇总结超上限）：先逐段压缩成要点，再把各段要点作为最终
    // 总结的输入走下方同一条流式主干。段内摘要不流式（用户看的是最终稿），但每段的取消
    // 令牌都登记进单槽，「停止」随时生效。
    let text = if strategy == FloatingMenuStrategy::Chunked {
        on_event
            .send(TranslateEvent::FloatingMenu(meta.clone()))
            .map_err(|e| e.to_string())?;
        on_event
            .send(TranslateEvent::Started)
            .map_err(|e| e.to_string())?;
        // 进度经流事件回面板；发不出去（面板已关）不算错——那正是「转入后台」的常态。
        let progress = |done: u32, total: u32| {
            let _ = on_event.send(TranslateEvent::FloatingChunk { done, total });
        };
        match reduce_long_input(&state, &settings, text, limit, &progress).await? {
            ReduceOutcome::Text { text, dropped } => {
                if dropped {
                    #[allow(clippy::cast_possible_truncation)]
                    on_event
                        .send(TranslateEvent::Truncated(
                            (super::distill::DISTILL_MAX_CHUNKS * limit) as u32,
                        ))
                        .map_err(|e| e.to_string())?;
                }
                text
            }
            // 用户按了「停止」——那是**终态**，不是失败。经 `?` 抛成命令错误的话，
            // 面板上写的是「错误: CANCELLED」，把一次正常的中止说成了故障。
            ReduceOutcome::Cancelled => {
                on_event
                    .send(TranslateEvent::Cancelled)
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    } else {
        text
    };
    let sent_text = text.clone();
    let glossary = load_glossary_entries(&state);
    let job = floating_menu_job(
        state.copy(),
        FloatingMenuRequest {
            text,
            action,
            scope,
            instruction: instruction.as_deref(),
            context,
            options,
            strict_local: settings.strict_local(),
            meeting_minutes,
            glossary: &glossary,
        },
        &settings,
        state.detector.as_ref(),
    )?;
    if strategy != FloatingMenuStrategy::Chunked {
        on_event
            .send(TranslateEvent::FloatingMenu(meta))
            .map_err(|e| e.to_string())?;
    }
    if let Some(kept_chars) = truncated {
        on_event
            .send(TranslateEvent::Truncated(kept_chars))
            .map_err(|e| e.to_string())?;
    }
    // 整理为表格（对标方案 T6）：跑到终态再从输出里抽出一张合法的 GFM 表；抽不出就不给落地
    // （`FloatingRejected`）——宁可不给，也不把一段说明文字当表格替换进正文。
    if action == FloatingMenuAction::TableFromText {
        on_event
            .send(TranslateEvent::Started)
            .map_err(|e| e.to_string())?;
        let event = match run_floating_chunk_job(&state, job).await? {
            ChunkOutcome::Text(output) => match luna_vault::gfm_table::extract_table(&output) {
                Some(table) => TranslateEvent::Done(table),
                None => TranslateEvent::FloatingRejected(FloatingRejectReason::NotATable),
            },
            ChunkOutcome::Cancelled => TranslateEvent::Cancelled,
        };
        return on_event.send(event).map_err(|e| e.to_string());
    }
    let handle = state.queue.submit(job);
    let my_generation = register_floating_run(&state, handle.cancel_token())?;
    // 光标处生成（续写 / 起草）：定稿前剥掉与光标前文尾部重复的开头行。
    let finalize_input = if scope == FloatingMenuScope::Cursor {
        Some(sent_text.clone())
    } else {
        None
    };
    let finalize = finalize_input
        .as_deref()
        .map(|input| move |output: String| strip_leading_overlap(input, &output));
    let outcome = stream_translate_events_with(
        handle,
        &on_event,
        Some(&app),
        finalize
            .as_ref()
            .map(|f| f as &(dyn Fn(String) -> String + Send + Sync)),
    )
    .await;
    finish_floating_run(&state, my_generation);
    outcome.map(|_| ())
}

/// 登记一次运行的取消令牌到单槽（顶替上一次），返回代数。
fn register_floating_run(state: &AppState, cancel: luna_core::CancelToken) -> Result<u64, String> {
    let mut run = state
        .floating_menu
        .lock()
        .map_err(|_| "悬浮菜单锁中毒".to_string())?;
    if let Some(previous) = run.cancel.take() {
        previous.cancel();
    }
    run.generation += 1;
    run.cancel = Some(cancel);
    Ok(run.generation)
}

/// 运行结束：只清自己那一代的令牌（新任务已顶替时不动）。
fn finish_floating_run(state: &AppState, generation: u64) {
    if let Ok(mut run) = state.floating_menu.lock() {
        if run.generation == generation {
            run.cancel = None;
        }
    }
}

/// 分段归约的结果：归约后的输入（`dropped` = 段数触顶、尾部没读到），或被用户停止。
pub(crate) enum ReduceOutcome {
    Text { text: String, dropped: bool },
    Cancelled,
}

/// 「按这一列让 AI 填写」（对标方案 T6）：表格源码里每一行正文跑一趟，模型只看这一行
/// （「列名: 值」逐行），按用户的一句指令写出这一格；全部写完拼回表格——**只改这一列**
/// （`gfm_table::replace_column`），再以替换为建议落地交给面板预览。逐行之间没有 token 流，
/// 进度经 `FloatingChunk` 报到第几行；每趟的取消令牌都登记进单槽，「停止」随时生效。
#[allow(clippy::too_many_arguments)]
async fn run_table_fill(
    app: &tauri::AppHandle,
    state: &AppState,
    settings: &Settings,
    source: &str,
    instruction: Option<&str>,
    options: &FloatingMenuOptionsDto,
    on_event: &Channel<TranslateEvent>,
) -> Result<(), String> {
    let instruction = instruction
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| coded_error_with(state.copy(), "EMPTY_INSTRUCTION", "empty-instruction"))?;
    let column = options.table_column.map_or(usize::MAX, |c| c as usize);
    let rows = luna_vault::gfm_table::table_cells(source)
        .filter(|rows| column < rows[0].len() && rows.len() > 1);
    let Some(rows) = rows else {
        return on_event
            .send(TranslateEvent::FloatingRejected(
                FloatingRejectReason::NotATableSource,
            ))
            .map_err(|e| e.to_string());
    };
    let header = &rows[0];
    let availability = crate::cloud::evaluate_for(app);
    let endpoint = compute_endpoint_source(availability, &state.cloud_server());
    #[allow(clippy::cast_possible_truncation)]
    let total = (rows.len() - 1) as u32;
    #[allow(clippy::cast_possible_truncation)]
    let meta = FloatingMenuMeta {
        suggested_landing: FloatingMenuLanding::Replace,
        on_server: !settings.strict_local()
            && chat_runs_on_server(settings, &endpoint)
            && state.server_serves(FloatingMenuAction::TableFillColumn.server_capability()),
        sent_chars: source.chars().count() as u32,
        context_chars: 0,
        chunks: total,
    };
    on_event
        .send(TranslateEvent::FloatingMenu(meta))
        .map_err(|e| e.to_string())?;
    on_event
        .send(TranslateEvent::Started)
        .map_err(|e| e.to_string())?;
    let rule = editor_instruction(
        "table.fill_cell",
        &[("column", &header[column]), ("instruction", instruction)],
    );
    let glossary = load_glossary_entries(state);
    let mut values = Vec::with_capacity(rows.len() - 1);
    for (index, row) in rows[1..].iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let _ = on_event.send(TranslateEvent::FloatingChunk {
            done: index as u32,
            total,
        });
        let row_text = header
            .iter()
            .enumerate()
            .map(|(c, name)| format!("{name}: {}", row.get(c).map_or("", String::as_str)))
            .collect::<Vec<_>>()
            .join("\n");
        let kind = with_job_glossary(
            JobKind::Refine {
                text: TextRef::Inline(row_text.clone()),
                instruction: Some(rule.clone()),
            },
            &row_text,
            &glossary,
        );
        match run_floating_chunk(state, kind).await? {
            ChunkOutcome::Text(output) => values.push(fill_cell_value(&output, &header[column])),
            ChunkOutcome::Cancelled => {
                return on_event
                    .send(TranslateEvent::Cancelled)
                    .map_err(|e| e.to_string());
            }
        }
    }
    let _ = on_event.send(TranslateEvent::FloatingChunk { done: total, total });
    let event = match luna_vault::gfm_table::replace_column(source, column, &values) {
        Some(table) => TranslateEvent::Done(table),
        None => TranslateEvent::FloatingRejected(FloatingRejectReason::NotATableSource),
    };
    on_event.send(event).map_err(|e| e.to_string())
}

/// 模型给一格的回答 → 放进格子的那一行：取第一行非空文字，剥掉它复述的「列名:」前缀。
fn fill_cell_value(output: &str, column: &str) -> String {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let line = line
        .strip_prefix(column)
        .and_then(|rest| rest.trim_start().strip_prefix([':', '：']))
        .map_or(line, str::trim);
    line.trim_matches(['"', '“', '”']).trim().to_owned()
}

/// 长文分段归约（PRD-06-025）：超上限的整篇输入先逐段压缩成要点，再把各段要点拼成
/// 最终输入。**前台与后台共用**——两处各写一遍，迟早在分段提示词或段数上限上分叉，
/// 而那种分叉的表现是「面板里总结得好好的，后台跑出来的却是另一种粒度」。
///
/// `progress(done, total)` 报到第几段：分段之间没有 token 流，前台据此更新状态行，
/// 后台据此更新队列进度。
async fn reduce_long_input(
    state: &AppState,
    settings: &Settings,
    text: String,
    limit: usize,
    progress: &(dyn Fn(u32, u32) + Send + Sync),
) -> Result<ReduceOutcome, String> {
    let mut chunks = super::distill::split_chunks(&text, limit);
    let dropped = chunks.len() > super::distill::DISTILL_MAX_CHUNKS;
    chunks.truncate(super::distill::DISTILL_MAX_CHUNKS);
    let ctx = lang_context_from_settings(settings);
    let lang = floating_summary_lang(settings, &ctx);
    let hint = editor_instruction("chunk_digest", &[("lang", lang.display_name())]);
    #[allow(clippy::cast_possible_truncation)]
    let total = chunks.len() as u32;
    let mut digests = Vec::with_capacity(chunks.len());
    for (idx, chunk) in chunks.into_iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        progress(idx as u32, total);
        match summarize_chunk(state, chunk, hint.clone()).await? {
            ChunkOutcome::Text(digest) => {
                digests.push(format!("--- Part {} ---\n{digest}", idx + 1));
            }
            ChunkOutcome::Cancelled => return Ok(ReduceOutcome::Cancelled),
        }
    }
    progress(total, total);
    let (joined, _) = truncate_floating_input(digests.join("\n\n"), limit);
    Ok(ReduceOutcome::Text {
        text: joined,
        dropped,
    })
}

/// 一段分片摘要的结果：产出，或被用户停止。**取消不是错误**，故它是一个正常返回值而
/// 不是 `Err`——用 `Err` 表达会让调用处的 `?` 把中止一路抛成命令失败。
pub(crate) enum ChunkOutcome {
    Text(String),
    Cancelled,
}

/// 分段摘要的一段：非流式，取终态文本；取消令牌登记进单槽以便「停止」。
async fn summarize_chunk(
    state: &AppState,
    chunk: String,
    instruction: Instruction,
) -> Result<ChunkOutcome, String> {
    run_floating_chunk(
        state,
        JobKind::Summarize {
            text: TextRef::Inline(chunk),
            style: SummaryStyle::Bullets,
            instruction: Some(instruction),
        },
    )
    .await
}

/// 跑一个悬浮菜单 Job 到终态、只取文本；取消一律视为失败（调用方不区分中止与故障）。
async fn run_floating_to_text(state: &AppState, kind: JobKind) -> Result<String, String> {
    match run_floating_chunk(state, kind).await? {
        ChunkOutcome::Text(text) => Ok(text),
        ChunkOutcome::Cancelled => Err("CANCELLED".to_owned()),
    }
}

/// 跑一个悬浮菜单 Job 到终态（非流式路径共用：分段摘要、变更解释）。
async fn run_floating_chunk(state: &AppState, kind: JobKind) -> Result<ChunkOutcome, String> {
    let mut job = Job::new(kind, JobOrigin::FloatingMenu);
    job.priority = Priority::Interactive;
    job.routing.local_only = state.settings_snapshot().strict_local();
    run_floating_chunk_job(state, job).await
}

/// 同上，但接收**已构造好**的 Job（后台执行体要的是与前台完全同一个 Job）。
async fn run_floating_chunk_job(state: &AppState, job: Job) -> Result<ChunkOutcome, String> {
    let mut handle = state.queue.submit(job);
    let generation = register_floating_run(state, handle.cancel_token())?;
    let outcome = loop {
        match handle.recv().await {
            Some(JobEvent::Finished(result)) => match result.payload {
                ResultPayload::Text(text) => {
                    break Ok(ChunkOutcome::Text(super::distill::normalize_model_output(
                        &text,
                    )))
                }
                _ => break Err("FLOATING_CHUNK: empty payload".to_owned()),
            },
            Some(JobEvent::Failed(err)) => break Err(format!("FLOATING_CHUNK: {}", err.code())),
            Some(JobEvent::Cancelled) | None => break Ok(ChunkOutcome::Cancelled),
            Some(_) => {}
        }
    };
    finish_floating_run(state, generation);
    outcome
}

/// 校对变更的解释指令：一句话、用文本自身的语言、只解释这一处。
pub(crate) fn explain_change_instruction(
    original: &str,
    replacement: &str,
    detection: &Detection,
) -> Instruction {
    let lang_clause = match detection.reliable_lang() {
        Some(lang) => editor("explain_lang.named", &[("lang", lang.display_name())]),
        None => editor("explain_lang.same", &[]),
    };
    let what = if original.is_empty() {
        editor("explain_what.insert", &[("replacement", replacement)])
    } else if replacement.is_empty() {
        editor("explain_what.remove", &[("original", original)])
    } else {
        editor(
            "explain_what.change",
            &[("original", original), ("replacement", replacement)],
        )
    };
    editor_instruction(
        "explain_change",
        &[("lang_clause", &lang_clause), ("what", &what)],
    )
}

/// 校对差异里某一处变更的「为什么」（PRD-06-021，Apple 校对对标）：按需、单次、非流式。
/// 输入只有含这一处的那句话与前后片段，不发整段——解释一个改动不需要整段出域。
#[tauri::command]
#[specta::specta]
pub(crate) async fn floating_menu_explain(
    state: tauri::State<'_, AppState>,
    sentence: String,
    original: String,
    replacement: String,
) -> Result<String, String> {
    let sentence = sentence.trim().to_owned();
    if sentence.is_empty() {
        return Err(coded_error_with(
            state.copy(),
            "EMPTY_INSTRUCTION",
            "empty-instruction",
        ));
    }
    let detection = state.detector.detect(&sentence);
    let instruction = explain_change_instruction(original.trim(), replacement.trim(), &detection);
    run_floating_to_text(
        &state,
        JobKind::Refine {
            text: TextRef::Inline(sentence),
            instruction: Some(instruction),
        },
    )
    .await
}

// ── 整篇动作转入后台（PRD-06-021）：进处理队列 + 完成通知 ─────────────────────

/// 把一次**整篇**AI 动作交给处理队列（与蒸馏同一条队列、同一套进度 / 取消 / 重试）。
///
/// **只收整篇**：队列载荷只带笔记路径，正文执行时重读；选区那份输入是一段范围，
/// 面板一关就随编辑漂移，而选区的落地含「替换」——替换必须对准打开面板那一刻的那段
/// 文字。整篇的落地方式全是插入，隔一次后台往返仍然成立。约束在 `ProcTaskKind::NoteAi`
/// 的文档注释里给了完整理由。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_menu_background(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    action: FloatingMenuAction,
    options: FloatingMenuOptionsDto,
) -> Result<(), String> {
    let note_rel = note_rel.trim().to_owned();
    if note_rel.is_empty() {
        return Err(coded_error_with(
            state.copy(),
            "INVALID_INPUT",
            "note-ai-invalid",
        ));
    }
    // 转入后台即放弃前台这一次：同一份活儿没有理由跑两遍。
    if let Ok(run) = state.floating_menu.lock() {
        if let Some(cancel) = &run.cancel {
            cancel.cancel();
        }
    }
    let label = super::transcribe::note_task_label(&note_rel);
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::NoteAi {
            note_rel,
            action: action.as_str().to_owned(),
            target_lang: options.target_lang,
            summary_style: options
                .summary_style
                .map(FloatingSummaryStyle::as_str)
                .map(str::to_owned),
        },
        label,
    );
    // 入队后立即广播快照：worker 忙着时只在自己的循环节点才广播，漏掉这一步新任务要等
    // 上一件跑完才出现在面板里（与 `enqueue_distill` 同一纪律）。
    super::transcribe::emit_processing_queue(&app);
    super::transcribe::start_processing_worker_if_idle(&app);
    Ok(())
}

/// 取这一篇待落地的后台结果（打开笔记时问一次；`None` = 没有）。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_menu_pending(
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Option<FloatingMenuPendingDto> {
    state
        .floating_pending
        .lock()
        .ok()
        .and_then(|pending| pending.get(note_rel.trim()).cloned())
}

/// 用户已经处置（落地或丢弃）：清掉这一篇的待落地结果。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_menu_pending_clear(state: tauri::State<'_, AppState>, note_rel: String) {
    if let Ok(mut pending) = state.floating_pending.lock() {
        pending.remove(note_rel.trim());
    }
}

/// 队列执行体：跑一次整篇 AI 动作，产物存进待落地槽（**不动正文**——落地仍由用户在
/// 面板里拍板，PRD-06-021 的「绝不在无预览下改正文」在后台同样成立）。
pub(crate) async fn run_note_ai(
    app: &tauri::AppHandle,
    note_rel: &str,
    action: &str,
    target_lang: Option<String>,
    summary_style: Option<String>,
    reporter: &super::distill::DistillReporter<'_>,
) -> Result<(), String> {
    use tauri::Manager as _;
    let state = app.state::<AppState>();
    let action = FloatingMenuAction::from_tag(action)
        .ok_or_else(|| coded_error_with(state.copy(), "INVALID_INPUT", "note-ai-invalid"))?;
    let settings = state.settings_snapshot();
    let i18n = backend_copy_arc(app);

    // 正文执行时重读——载荷不带正文（脱敏 + 载荷不会与笔记脱节）。
    let body = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        luna_vault::body_of(&doc.content).to_owned()
    };
    if body.trim().is_empty() {
        return Err(coded_error_with(
            state.copy(),
            "INVALID_INPUT",
            "note-ai-invalid",
        ));
    }

    // 与前台同一条主干：会议纪要换装 → 长文策略 → 分段归约 → 同一个 Job。
    let (text, meeting_minutes) = if matches!(action, FloatingMenuAction::Summarize) {
        match speaker_labeled_transcript(&state, i18n.as_deref(), note_rel) {
            Some(labeled) => (labeled, true),
            None => (body, false),
        }
    } else {
        (body, false)
    };
    super::chat::ensure_chat_context_window(app).await;
    let limit = floating_input_limit(state.chat_ctx_window());
    let (strategy, chunks) = floating_menu_strategy(
        text.chars().count(),
        FloatingMenuScope::Doc,
        Some(action),
        limit,
    );
    #[allow(clippy::cast_possible_truncation)]
    let sent_chars = text.chars().count() as u32;
    let endpoint = compute_endpoint_source(crate::cloud::evaluate_for(app), &state.cloud_server());
    let on_server = !settings.strict_local()
        && chat_runs_on_server(&settings, &endpoint)
        && state.server_serves(action.server_capability());
    reporter.bail_if_cancelled()?;
    let text = match strategy {
        FloatingMenuStrategy::Chunked => {
            let progress = |done: u32, total: u32| {
                reporter.step(crate::processing_queue::ProcPhase::Distilling, done, total);
            };
            match reduce_long_input(&state, &settings, text, limit, &progress).await? {
                ReduceOutcome::Text { text, .. } => text,
                ReduceOutcome::Cancelled => return Err("CANCELLED".to_owned()),
            }
        }
        _ => truncate_floating_input(text, limit).0,
    };

    let options = FloatingMenuOptionsDto {
        target_lang,
        summary_style: summary_style
            .as_deref()
            .map(FloatingSummaryStyle::from_setting),
        // 后台只跑整篇动作，没有「哪一列」。
        table_column: None,
    };
    let glossary = load_glossary_entries(&state);
    let job = floating_menu_job(
        state.copy(),
        FloatingMenuRequest {
            text,
            action,
            scope: FloatingMenuScope::Doc,
            instruction: None,
            context: FloatingMenuContext::default(),
            options,
            strict_local: settings.strict_local(),
            meeting_minutes,
            glossary: &glossary,
        },
        &settings,
        state.detector.as_ref(),
    )?;
    let output = match run_floating_chunk_job(&state, job).await? {
        ChunkOutcome::Text(text) => text,
        ChunkOutcome::Cancelled => return Err("CANCELLED".to_owned()),
    };
    if output.trim().is_empty() {
        return Err("FLOATING_EMPTY".to_owned());
    }
    if let Ok(mut pending) = state.floating_pending.lock() {
        pending.insert(
            note_rel.to_owned(),
            FloatingMenuPendingDto {
                note_rel: note_rel.to_owned(),
                action,
                output,
                meta: FloatingMenuMeta {
                    suggested_landing: suggested_landing(action, FloatingMenuScope::Doc),
                    on_server,
                    sent_chars,
                    context_chars: 0,
                    chunks,
                },
            },
        );
    }
    Ok(())
}

// ── 自定义指令预设（PRD-06-024）：随库存于 `.luna/ai_presets.json` ─────────────

/// 预设数量上限：预设是「常用的几条」，不是第二个笔记库。
const FLOATING_PRESETS_MAX: usize = 50;

/// 浮动菜单的预设清单：库里的与装上的模板包带来的（同名时库里的优先，ADR-169 ⑤）。
fn presets_dto(
    app: &tauri::AppHandle,
    presets: Vec<luna_vault::AiPreset>,
) -> Vec<FloatingPresetDto> {
    ai_preset_registry(&installed_template_packs(app), presets)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn floating_presets_list(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<FloatingPresetDto>, String> {
    let presets = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.ai_presets().map_err(vault_err)?
    };
    Ok(presets_dto(&app, presets))
}

/// 存一条预设（名字与指令都去首尾空白；空则拒绝；超上限拒绝）。返回更新后的完整清单（含模板包带来的）。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_preset_save(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
    instruction: String,
) -> Result<Vec<FloatingPresetDto>, String> {
    let name = name.trim().to_owned();
    let instruction = instruction.trim().to_owned();
    if name.is_empty() || instruction.is_empty() {
        return Err(coded_error_with(
            state.copy(),
            "EMPTY_INSTRUCTION",
            "empty-instruction",
        ));
    }
    let list = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let mut presets = vault.ai_presets().map_err(vault_err)?;
        if presets.len() >= FLOATING_PRESETS_MAX {
            return Err("PRESETS_FULL".to_owned());
        }
        presets.push(luna_vault::AiPreset {
            id: luna_core::JobId::new().to_string(),
            name,
            instruction,
        });
        vault.set_ai_presets(&presets).map_err(vault_err)?;
        presets
    };
    emit_vault_changed(&app, ".luna/ai_presets.json", false);
    Ok(presets_dto(&app, list))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn floating_preset_delete(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<FloatingPresetDto>, String> {
    let list = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let mut presets = vault.ai_presets().map_err(vault_err)?;
        presets.retain(|preset| preset.id != id);
        vault.set_ai_presets(&presets).map_err(vault_err)?;
        presets
    };
    emit_vault_changed(&app, ".luna/ai_presets.json", false);
    Ok(presets_dto(&app, list))
}

/// 停止当前笔记 AI 悬浮菜单生成（保留已产 token，前端呈「已取消」态，PRD-06-021）。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_menu_stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let run = state
        .floating_menu
        .lock()
        .map_err(|_| "悬浮菜单锁中毒".to_string())?;
    if let Some(cancel) = &run.cancel {
        cancel.cancel();
    }
    Ok(())
}

/// AI 结果的落地回执（PRD-06-021）：用户对这一份结果最终做了什么——替换 / 插入 / 丢弃。
/// 只记动作名与落地方式，**绝不含正文**；落 Vault 级用量库（与 `FloatingMenu` 来源的
/// 用量记录同库），接受率 = 已落地 / (已落地 + 丢弃)。
#[tauri::command]
#[specta::specta]
pub(crate) fn floating_menu_landed(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    action: FloatingMenuAction,
    landing: FloatingMenuLanding,
) -> Result<(), String> {
    state.with_vault(window.label(), |vault| {
        vault
            .usage_store()
            .insert_outcome(&luna_vault::AiOutcome {
                origin: JobOrigin::FloatingMenu,
                action: action.as_str().to_owned(),
                landing: landing.as_str().to_owned(),
                recorded_at_ms: super::checkup::now_ms(),
            })
            .map_err(|e| e.to_string())
    })
}

/// 把一个 Job 的流式 `JobEvent` 投影为 `TranslateEvent` 推给前端（`popup_generate` 与
/// `submit_floating_menu` 共用，DRY）。终态事件后即停。
///
/// 返回**成功完成时的最终译文**（`Some`，供弹窗会话缓存回填，PRD-01-016）；失败/取消为 `None`。
pub(crate) async fn stream_translate_events(
    handle: JobHandle,
    on_event: &Channel<TranslateEvent>,
    app: Option<&tauri::AppHandle>,
) -> Result<Option<String>, String> {
    stream_translate_events_with(handle, on_event, app, None).await
}

/// 终态整理器（须 `Send + Sync`：命令 future 跨线程）。
pub(crate) type Finalizer<'a> = &'a (dyn Fn(String) -> String + Send + Sync);

/// 同上，终态文本先经 `finalize` 整理再下发（流式 token 原样透传，`Done` 才是定稿——
/// 前端收到 `Done` 即整体替换预览）。`None` = 不整理。
pub(crate) async fn stream_translate_events_with(
    mut handle: JobHandle,
    on_event: &Channel<TranslateEvent>,
    app: Option<&tauri::AppHandle>,
    finalize: Option<Finalizer<'_>>,
) -> Result<Option<String>, String> {
    while let Some(event) = handle.recv().await {
        let (dto, final_text) = match event {
            JobEvent::Started => (TranslateEvent::Started, None),
            JobEvent::Partial(PartialPayload::Token(t)) => (TranslateEvent::Token(t), None),
            JobEvent::Finished(result) => {
                let text = match result.payload {
                    ResultPayload::Text(t) => t,
                    _ => String::new(),
                };
                let text = finalize.map_or(text.clone(), |f| f(text));
                (TranslateEvent::Done(text.clone()), Some(Some(text)))
            }
            JobEvent::Failed(err) => {
                publish_job_failure_notification(app, &err);
                (
                    TranslateEvent::Failed(TranslateFailure::from(&err)),
                    Some(None),
                )
            }
            JobEvent::Cancelled => (TranslateEvent::Cancelled, Some(None)),
            // 其它事件（Queued/Progress/Usage/Asr partial）翻译场景下不投影。
            _ => continue,
        };
        on_event.send(dto).map_err(|e| e.to_string())?;
        // 终态（`final_text` 为 `Some(_)`）后即停并返回最终译文（或 None）。
        if let Some(result) = final_text {
            return Ok(result);
        }
    }
    Ok(None)
}

pub(crate) fn publish_job_failure_notification(
    app: Option<&tauri::AppHandle>,
    err: &luna_core::JobError,
) {
    let Some(app) = app else {
        return;
    };
    if !matches!(err, luna_core::JobError::StrictLocalBlocked { .. }) {
        return;
    }
    use tauri::Manager as _;
    if let Some(center) = app.try_state::<NotificationCenter>() {
        center.publish(NotificationEvent::strict_local_blocked(), app);
    }
}

#[cfg(test)]
mod table_fill_tests {
    use super::fill_cell_value;

    #[test]
    fn cell_value_takes_one_line_and_drops_an_echoed_column_name() {
        assert_eq!(
            fill_cell_value("\n卖点：很亮的台灯\n解释……", "卖点"),
            "很亮的台灯"
        );
        assert_eq!(fill_cell_value("\"Bright lamp\"", "Pitch"), "Bright lamp");
        assert_eq!(fill_cell_value("Pitch: bright", "Pitch"), "bright");
        assert_eq!(fill_cell_value("   ", "x"), "");
    }
}
