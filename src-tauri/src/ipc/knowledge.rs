//! 知识复利域（PRD-16、ADR-026）：
//! 库内检索的上下文合成 + 「相关」面板（未链接提及 / 相关笔记）。
//!
//! 分工与既有域一致：**判定与算法在 `luna_vault::retrieve`（纯函数，可无头单测）**，
//! 本模块只做 Tauri 命令边界（收发/校验/装配）+ DTO + **指令合成**。指令合成留在命令层
//! 是本仓既有惯例（同 `graph::extract_tags_instruction`）——「这次任务要模型干什么」是
//! 调用方的决策，引擎只负责渲染。
//!
//! 脱敏纪律（TDD-12）：用户问题、笔记摘录与转写文本**同级**——日志只出路径与计数，
//! 绝不出问题原文或摘录内容。

use super::*;

/// 出处编号从 1 开始（与回答正文里的 `[1]` 对齐；0 号在人读时天然歧义）。
const FIRST_CITATION_INDEX: u32 = 1;
const MODEL_SUPPLEMENT_SENTINEL: &str = "<<<LUNA_MODEL_SUPPLEMENT>>>";
/// 哨兵的**词干**：识别与剥离都认它，不认两侧的尖括号。
///
/// 真机 2026-09-08：本地小模型把哨兵抄成了 `>>>LUNA_MODEL_SUPPLEMENT>>>`，Markdown 把它
/// 渲染成三层嵌套引用块，界面上冒出一条黄色竖条 + `LUNA_MODEL_SUPPLEMENT>>>`。协议只要求
/// 「独占一行」；尖括号是给人看的装饰，模型抄歪它不该改变判定，更不该让哨兵漏到正文里。
const MODEL_SUPPLEMENT_TOKEN: &str = "LUNA_MODEL_SUPPLEMENT";

/// 这一行是不是哨兵行：去掉两侧空白、尖括号、引用符号、反引号与星号后恰好只剩词干。
/// 词干出现在一句话中间（`结论，但把哨兵塞在句子里`）不算——那是行内泄漏，另行 fail closed。
fn is_sentinel_line(line: &str) -> bool {
    line.trim()
        .trim_matches(|c: char| matches!(c, '<' | '>' | ' ' | '\t' | '`' | '*' | '_'))
        == MODEL_SUPPLEMENT_TOKEN
}

/// 把哨兵的一切形态从正文里剥掉：哨兵行整行删除，行内出现的词干连同两侧尖括号一起删除。
/// 终局正文、归档、复制内容都只能拿到这一份净文本——协议词从不呈现给用户（TDD-06 §3.7.2）。
fn strip_sentinel(text: &str) -> String {
    text.lines()
        .filter(|line| !is_sentinel_line(line))
        .map(remove_inline_sentinel)
        .collect::<Vec<_>>()
        .join("\n")
}

fn remove_inline_sentinel(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find(MODEL_SUPPLEMENT_TOKEN) {
        let bytes = rest.as_bytes();
        let mut start = pos;
        while start > 0 && matches!(bytes[start - 1], b'<' | b'>') {
            start -= 1;
        }
        let mut end = pos + MODEL_SUPPLEMENT_TOKEN.len();
        while end < bytes.len() && matches!(bytes[end], b'<' | b'>') {
            end += 1;
        }
        // 两侧都是空格时只留一个，别在句子中间留下一个双空格的洞。
        if start > 0 && bytes[start - 1] == b' ' && end < bytes.len() && bytes[end] == b' ' {
            end += 1;
        }
        out.push_str(&rest[..start]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

// ── DTO ─────────────────────────────────────────────────────────────────────

/// 复合锚（ADR-026 ⑥）的前端投影。前端只搬运、不解析——解析恒在 Rust。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub(crate) struct CitationAnchorDto {
    /// 标题路径（`["项目 X", "本周进展"]`）。
    pub heading_path: Vec<String>,
    /// 引文。
    pub quote: String,
    /// 引文前的上下文（消歧用）。
    pub prefix: String,
    /// 引文后的上下文（消歧用）。
    pub suffix: String,
    /// 行提示（1-based，相对整篇文件）。
    pub line_hint: u32,
    /// 生成锚点时的源正文指纹；用于归档后识别来源发生过变化。
    pub source_hash: String,
}

impl From<&luna_vault::Anchor> for CitationAnchorDto {
    fn from(a: &luna_vault::Anchor) -> Self {
        Self {
            heading_path: a.heading_path.clone(),
            quote: a.quote.clone(),
            prefix: a.prefix.clone(),
            suffix: a.suffix.clone(),
            line_hint: a.line_hint,
            source_hash: a.source_hash.clone(),
        }
    }
}

impl From<&CitationAnchorDto> for luna_vault::Anchor {
    fn from(a: &CitationAnchorDto) -> Self {
        Self {
            heading_path: a.heading_path.clone(),
            quote: a.quote.clone(),
            prefix: a.prefix.clone(),
            suffix: a.suffix.clone(),
            line_hint: a.line_hint,
            source_hash: a.source_hash.clone(),
        }
    }
}

/// 一条出处（= 回答里的一个 `[n]`）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct RetrievalSourceDto {
    /// 引用编号（1-based，与正文 `[n]` 对齐）。
    pub index: u32,
    /// 笔记相对路径。
    pub path: String,
    /// 标题。
    pub title: String,
    /// 笔记类型 kebab 键（前端按此分「知识 / 资料」呈现，判定在 Rust）。
    pub note_type: String,
    /// 是否资料区（ADR-024；前端不自行判断"哪几种算资料"）。
    pub source_material: bool,
    /// 这条出处**来自记录**吗（ADR-086 / PRD-16-041）：出处芯片与角标 hover 据此标注
    /// 「来自记录」，让用户一眼分清「沉淀过的知识」与「未定稿的草稿」。
    ///
    /// 记录态默认**参与**检索（不进默认排除集），但在各路排序里让位（`DRAFT_WEIGHT`）
    /// ——降权不排除，且**不设绝对分数阈值**（ADR-027 ④）。
    pub draft: bool,
    /// hover 预览摘要。
    pub snippet: String,
    /// 复合锚（点击跳转时回传给 `resolve_citation`）。
    pub anchor: CitationAnchorDto,
}

/// 一次检索的透明信息（PRD-16-006：透明行本身就是范围控件）。仅下行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct RetrievalInfoDto {
    /// 真正读盘打过分的篇数（「已检索 N 篇」——不拿候选数充数）。
    pub scanned: u32,
    /// 送进本轮上下文的出处（按相关度降序，编号即 `[n]`）。
    pub sources: Vec<RetrievalSourceDto>,
    /// 本次是否临时纳入了对话笔记。
    pub include_chat: bool,
    /// 本次是否整组排除了记录态（PRD-16-041）。透明行的开关据此回显——它是**本轮**的
    /// 事实，不是设置项；下一个问题重新按默认口径（记录态参与但降权）检索。
    pub exclude_drafts: bool,
    /// 本次被剔除的笔记路径。
    pub excluded: Vec<String>,
    /// 候选触顶——结果不完整，UI 须如实提示。
    pub truncated: bool,
}

/// 回答中一段内容的依据类别。终局只由 Rust 归类，前端不从 Markdown 猜测。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AnswerSegmentKind {
    KnowledgeEvidence,
    ModelSupplement,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct AnswerSegmentDto {
    pub kind: AnswerSegmentKind,
    pub markdown: String,
}

/// 终局 grounding 事实。`None` 仍有语义：本轮未开启库内检索，不被误读为模型回答。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroundingOutcome {
    GroundedOnly,
    Mixed,
    ModelOnly,
    LegacyUnclassified,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct AnswerMetaDto {
    pub segments: Vec<AnswerSegmentDto>,
    pub sources: Vec<RetrievalSourceDto>,
    pub grounding_outcome: GroundingOutcome,
}

/// 对终局回答做 fail-closed 分段。任何不合格证据段都宁可归模型补充，绝不把无依据文字
/// 标成“来自知识库”。
pub(crate) fn classify_grounded_answer(
    text: &str,
    sources: &[RetrievalSourceDto],
) -> AnswerMetaDto {
    let normalized = normalize_citation_brackets(text);
    let text = normalized.as_str();
    let sentinel_lines = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| is_sentinel_line(line).then_some(index))
        .collect::<Vec<_>>();
    // Sentinel 只能作为一条独占行出现。正文里任何内联、重复形态都 fail closed，
    // 避免 split_once 被正文里的字面量抢先命中。判据认词干而不认尖括号：抄歪的哨兵
    // （`>>>…>>>`、少一个尖括号）仍是哨兵，不是"损坏"。
    if text.matches(MODEL_SUPPLEMENT_TOKEN).count() != sentinel_lines.len()
        || sentinel_lines.len() > 1
    {
        return model_only_meta(text, sources);
    }
    let (evidence, supplement) = if sentinel_lines.len() == 1 {
        let lines = text.lines().collect::<Vec<_>>();
        let split = sentinel_lines[0];
        let left = lines[..split].join("\n");
        let right = lines[split + 1..].join("\n");
        if left.trim().is_empty() || right.trim().is_empty() {
            return model_only_meta(text, sources);
        }
        (left.trim().to_owned(), Some(right.trim().to_owned()))
    } else {
        (text.trim().to_owned(), None)
    };
    let valid: std::collections::HashSet<u32> = sources.iter().map(|source| source.index).collect();
    if valid.is_empty() {
        return model_only_meta(text, sources);
    }
    // 证据段 = **前缀最长的、每一个单元都带合法引用的那一段**，其后的一切归补充。
    //
    // 从前这里是「任何一个单元不合格 → 整条回答降级」。它同样 fail closed，但真机上
    // 让「混合」结构性地不可达：本地小模型总会写一句没编号的引导语、一个小标题、
    // 或者把指令复述一遍，于是整篇带 `[2]`/`[4]` 的合格引用一起被判成「未由知识库支持」，
    // 出处芯片全空、「存为知识笔记」也拒绝写回。契约要的是「未引用的**内容**归补充」
    //（TDD-06 §3.7.2），不是「有一处未引用就整条不算数」。
    //
    // 切分保持**段落粒度**：不把一个列表从中间劈开，也不重排段落顺序，于是归档仍然是
    // 「证据块在前、补充块在后」这一种形态。
    let paragraphs = evidence.split("\n\n").collect::<Vec<_>>();
    let mut grounded_paragraphs = 0usize;
    let mut cited_units = 0usize;
    for paragraph in &paragraphs {
        // 空段与纯结构段（小标题、分隔线）本身不作断言，放行但也不算数。
        let units = evidence_units(paragraph);
        if units.iter().any(|part| !has_valid_citation(part, &valid)) {
            break;
        }
        cited_units += units.len();
        grounded_paragraphs += 1;
    }
    // 结尾若是纯结构段（多半正是那个引出后文的小标题），把它退回补充侧——它属于下面
    // 那段没依据的内容，留在证据块尾巴上只会让两块的边界读起来是错的。
    while grounded_paragraphs > 0 && evidence_units(paragraphs[grounded_paragraphs - 1]).is_empty()
    {
        grounded_paragraphs -= 1;
    }
    let grounded = paragraphs[..grounded_paragraphs].join("\n\n");
    let grounded = grounded.trim();
    // 只有小标题、没有任何带引用的断言，不叫「有依据」。
    if grounded.is_empty() || cited_units == 0 {
        return model_only_meta(text, sources);
    }
    let demoted = paragraphs[grounded_paragraphs..].join("\n\n");
    let supplement = match (demoted.trim(), supplement.as_deref().map(str::trim)) {
        ("", None) | ("", Some("")) => None,
        ("", Some(tail)) => Some(tail.to_owned()),
        (head, None) | (head, Some("")) => Some(head.to_owned()),
        (head, Some(tail)) => Some(format!("{head}\n\n{tail}")),
    };

    let mut evidence_indexes = std::collections::HashSet::new();
    for paragraph in evidence_units(grounded) {
        evidence_indexes.extend(citation_indexes(paragraph));
    }
    let used = sources
        .iter()
        .filter(|source| evidence_indexes.contains(&source.index))
        .cloned()
        .collect::<Vec<_>>();
    let mut segments = vec![AnswerSegmentDto {
        kind: AnswerSegmentKind::KnowledgeEvidence,
        markdown: grounded.to_owned(),
    }];
    let grounding_outcome = if let Some(supplement) = supplement {
        segments.push(AnswerSegmentDto {
            kind: AnswerSegmentKind::ModelSupplement,
            // 补充段里的 `[n]` 是死角标（协议规定哨兵之后不再标编号，出处区也不列它们），
            // 留着只会制造"看着能点、点不开"的过度信任（PRD-16-005）。
            markdown: luna_core::strip_dead_citations(&supplement),
        });
        GroundingOutcome::Mixed
    } else {
        GroundingOutcome::GroundedOnly
    };
    AnswerMetaDto {
        segments,
        sources: used,
        grounding_outcome,
    }
}

/// 把模型写成全角的引用角标（`【1】`、`［2，3］`、`【１】`）归一成协议的 `[n]`。
///
/// 真机（2026-09-27，云端 gpt-oss-20b）：回答通篇用 `【1】` 标注，解析只认半角方括号，
/// 于是一条逐句有出处的回答被整条判成「模型补充 · 未由知识库支持 / 引用 0 篇」。
/// 只在这里归一一次：分段、引用计数、剥死角标与最终落账的正文看到的是同一种写法。
/// 括号里只有数字与分隔符才算角标，`【注】`、`【重要】` 之类照旧是正文。
fn normalize_citation_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['【', '［']) {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let open_len = after.chars().next().map_or(0, char::len_utf8);
        let close = after[open_len..].find(['】', '］']);
        let marker = close.and_then(|close| {
            let inner = &after[open_len..open_len + close];
            let digits: String = inner
                .chars()
                .map(|c| match c {
                    '０'..='９' => char::from_digit(c as u32 - '０' as u32, 10).unwrap_or(c),
                    _ => c,
                })
                .collect();
            let is_marker = !digits.trim().is_empty()
                && digits.chars().any(|c| c.is_ascii_digit())
                && digits
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c == '，' || c == ' ');
            let close_len = after[open_len + close..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
            is_marker.then(|| (digits, open_len + close + close_len))
        });
        match marker {
            Some((digits, consumed)) => {
                out.push('[');
                out.push_str(&digits);
                out.push(']');
                rest = &after[consumed..];
            }
            None => {
                out.push_str(&after[..open_len]);
                rest = &after[open_len..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一行是否只是**结构**（标题 / 分隔线 / 以冒号结尾的引导标签），而不是一条断言。
///
/// 结构行不参与「必须带引用」的校验，因为它**没有可被核查的事实内容**——要求
/// `### 双均线系统的核心原理` 带一个 `[n]` 是范畴错误。真机第七轮实测：不这么分，
/// 模型只要写一个小标题，整篇带合法引用的回答就会被判成「未由知识库支持」。
/// 判据刻意收得很窄：以冒号结尾才算引导标签，`关键逻辑：该系统的核心在于…` 这种
/// **句子**照旧要带引用。
fn is_structural_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with('#') {
        return true;
    }
    if trimmed
        .chars()
        .all(|c| c == '-' || c == '*' || c == '_' || c == '=')
    {
        return true;
    }
    // 去掉列表符号与强调标记后，看它是不是「一句以冒号收尾的标签」。
    let label = trimmed
        .trim_start_matches(['-', '*', '+', '>'])
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', ')'])
        .trim()
        .trim_matches(['*', '_', '`'])
        .trim();
    label.ends_with('：') || label.ends_with(':')
}

fn evidence_units(markdown: &str) -> Vec<&str> {
    markdown
        .split("\n\n")
        .flat_map(|paragraph| {
            let lines = paragraph.lines().collect::<Vec<_>>();
            let has_list = lines.iter().any(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ")
            });
            if !has_list {
                vec![paragraph]
            } else {
                // 列表段落里的每个非空行都是独立证据单元：这既覆盖列表项，也不会把列表
                // 前后的普通断言静默丢掉。比把整段当一单元更严格，但 fail closed 是正确方向。
                lines.into_iter().collect()
            }
        })
        // 结构行不是断言，不参与校验；剩下的每一条都必须带本轮合法引用。
        .filter(|part| !part.lines().all(is_structural_line))
        .collect()
}

fn model_only_meta(text: &str, _sources: &[RetrievalSourceDto]) -> AnswerMetaDto {
    AnswerMetaDto {
        segments: vec![AnswerSegmentDto {
            kind: AnswerSegmentKind::ModelSupplement,
            // 哨兵的任何形态都不进正文；整条都是补充，`[n]` 没有可指向的出处，同样剥掉。
            markdown: luna_core::strip_dead_citations(strip_sentinel(text).trim()),
        }],
        // ModelOnly 没有任何可作为证据的来源；保留检索候选会让归档/保存动作误以为它们
        // 支撑了回答。
        sources: Vec::new(),
        grounding_outcome: GroundingOutcome::ModelOnly,
    }
}

fn citation_indexes(text: &str) -> Vec<u32> {
    cited_indices(text)
}

fn has_valid_citation(text: &str, valid: &std::collections::HashSet<u32>) -> bool {
    let found = citation_indexes(text);
    !found.is_empty() && found.iter().all(|index| valid.contains(index))
}

/// 引用跳转的解析结果（PRD-16-012）。仅下行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct CitationTargetDto {
    /// 目标行（1-based，相对整篇文件）；`None` = 完全锚不上，只打开笔记。
    pub line: Option<u32>,
    /// 是否精确命中引文。`false` **必须**向用户如实说明「原文已改动」——
    /// 静默跳到大概位置比说不知道更糟（ADR-026 ⑥ 反过度信任纪律）。
    pub exact: bool,
}

/// 引用打开的封闭终态。Rust 同时拥有源文件检查、锚解析与工作区导航，前端不能再把
/// “解析成功”误当成“实际打开成功”。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CitationOpenOutcome {
    Exact,
    Moved,
    HeadingOnly,
    Lost,
    SourceMissing,
}

/// 一处未链接提及（PRD-16-013）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct UnlinkedMentionDto {
    /// 行号（1-based，相对整篇文件）。
    pub line: u32,
    /// 命中在该行内的字节起点（DTO 用 `u32`：`usize` 跨 IPC 会被 specta 判为
    /// BigInt 风格类型而拒绝；行内偏移远不到 4 GiB，无精度问题）。
    pub start: u32,
    /// 命中在该行内的字节终点。
    pub end: u32,
    /// 命中的原文（= 目标笔记标题）。
    pub text: String,
    /// 目标笔记相对路径。
    pub target_path: String,
    /// 该行完整文本（面板呈现上下文）。
    pub line_text: String,
}

impl From<&luna_vault::UnlinkedMention> for UnlinkedMentionDto {
    fn from(m: &luna_vault::UnlinkedMention) -> Self {
        Self {
            line: m.line,
            start: m.start as u32,
            end: m.end as u32,
            text: m.text.clone(),
            target_path: m.target_path.clone(),
            line_text: m.line_text.clone(),
        }
    }
}

impl From<&UnlinkedMentionDto> for luna_vault::UnlinkedMention {
    fn from(m: &UnlinkedMentionDto) -> Self {
        Self {
            line: m.line,
            start: m.start as usize,
            end: m.end as usize,
            text: m.text.clone(),
            target_path: m.target_path.clone(),
            line_text: m.line_text.clone(),
        }
    }
}

/// 一页的**所有权**（PRD-16-021）——wire 上的封闭枚举，**全应用只有这一个**。
///
/// **它是 [`luna_vault::TopicOwnership`] 的逐一投影，不是第二套判据**：所有权的事实源
/// 只有 `luna.synth_body` 指纹一个。此前 wire 上有两处各自的裸字符串表示（单页状态查询的
/// `"ai"/"user"/"map_ai"/"map_user"`、归入计划项的 `"ai"/"user"`），主题页目录再加一处就
/// 是第三处——而裸字符串的分叉是静默的：拼错一个取值只会让某个界面永远走另一条分支，
/// typecheck 与全部用例都不会红（ADR-046 不留兼容取值，故这次一次性收干净）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicOwnershipDto {
    /// 既不是主题页也不是知识地图。
    NotTopic,
    /// **Lunarca 维护**：指纹与此刻正文一致，自动档可直接更新。
    AiOwned,
    /// **你已接管**：正文被编辑过（或没有指纹），此后一切更新恒走差异预览。
    UserOwned,
}

impl From<luna_vault::TopicOwnership> for TopicOwnershipDto {
    fn from(value: luna_vault::TopicOwnership) -> Self {
        match value {
            luna_vault::TopicOwnership::NotTopic => Self::NotTopic,
            luna_vault::TopicOwnership::AiOwned => Self::AiOwned,
            luna_vault::TopicOwnership::UserOwned => Self::UserOwned,
        }
    }
}

/// 一条主题页建议（PRD-16-016）。仅下行。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TopicSuggestionDto {
    /// 跨启动概览/图谱稳定的版本化建议身份；度量落盘前还会再经设备密钥 HMAC。
    pub suggestion_token: String,
    /// 出身标签（= 建议的主题名）。
    pub tag: String,
    /// 该标签下的知识笔记（合成初稿的输入；「看看是哪几篇」也用它）。
    pub notes: Vec<String>,
    /// 该标签下**知识笔记**的精确篇数（不含速记、待整理与资料）；`notes` 截在门槛四倍，
    /// 不能拿它的长度当篇数说（PRD-16-051）。
    pub note_count: u32,
    /// 后台已备好、仍可直接发布的初稿有几条结论（PRD-16-060）；`None` = 还没有备好。
    pub draft_items: Option<u32>,
}

fn topic_suggestion_dto(
    vault: &luna_vault::Vault,
    suggestion: luna_vault::TopicSuggestion,
) -> TopicSuggestionDto {
    let suggestion_token = suggestion_token(
        vault.root(),
        "topic",
        &suggestion.tag,
        &suggestion.notes.join("\n"),
    );
    let draft_items = vault
        .ready_topic_draft_for(&suggestion.tag)
        .map(|draft| u32::try_from(draft.claims.len()).unwrap_or(u32::MAX));
    TopicSuggestionDto {
        suggestion_token,
        tag: suggestion.tag,
        notes: suggestion.notes,
        note_count: suggestion.note_count,
        draft_items,
    }
}

/// 知识库脉搏的**一次取数**（PRD-16-027 下一步引擎）。仅下行。
///
/// 三个来源（批量蒸馏预检 / 主题页建议 / 待归入）合成一个命令而不是三次 IPC：脉搏渲染的
/// 是**一个优先级栈的栈顶**，三次往返里任何一次晚到都会让它先渲染一态再跳成另一态。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct KnowledgePulseDto {
    /// 批量蒸馏四桶（判据与单篇徽标同源，不另造第二套盘点）。
    pub distill: super::distill::DistillPreflightDto,
    /// 主题页建议（纯 Rust 零 LLM）。
    pub topics: Vec<TopicSuggestionDto>,
}

/// 图谱维护区的唯一推荐类型。排序与可执行性由 Rust 一次决定。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeNextActionKind {
    TopicOpportunity,
    KnowledgeMapMissing,
    KnowledgeMapConflict,
    KnowledgeMapReview,
    CheckupFinding,
    /// 库内 AI 标签分裂成两种语言（PRD-06-035d ②）。排在 `MissingTags` **之前**：
    /// 缺标签只是这一篇没参与进来，而语言分裂会让**每一个**标签的覆盖面减半，
    /// 「同标签 ≥3 篇」的主题页判据因此整体失效——它坏掉的是机制，不是单篇。
    TagLanguageMismatch,
    MissingTags,
    PendingDistill,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeNextActionCommand {
    CreateTopic,
    OpenCheckup,
    RescanTags,
    /// 重扫并**预选「全部重新生成」档**（PRD-06-035d ①）。与 `RescanTags` 分成两个值
    /// 而不是给 DTO 加一个布尔：前端只派发这个穷尽枚举、不按 kind 猜动作（本 DTO 的
    /// 既有纪律），而「补缺口」与「全部重来」是两件代价差很远的事——语言分裂的笔记
    /// 指纹是最新的，只补缺口一篇都扫不到，等于给一张按下去什么都不发生的卡。
    RescanTagsForce,
    DistillBatch,
    ResolveAiReadiness,
    CreateKnowledgeMap,
    ResolveKnowledgeMapConflict,
    ReviewKnowledgeMap,
}

/// 这一枚推荐要摆在哪一格（`PRD-16-039` 2026-08-30 修订）。
///
/// **判据一句话：这条建议做完之后，结果在图谱那一屏看得见吗？**
/// 把维护区放在图谱的原始依据是「动机产生地（图谱怎么这么稀疏）与结果可见地（图谱变密）
/// 都在同一屏」——补齐标签、收敛标签语言满足它（标签就是图谱的边，补完当场变密）；而
/// 合成主题页 / 归入 / 蒸馏 / 体检**不满足**：它们产出的是笔记，图谱上至多多一个点，
/// 用户看不到自己刚才那一下的结果。
///
/// **分流在 Rust，不在前端**：前端按 kind 自己挑该不该显示，就是把「哪条归哪一格」
/// 这个判据复制到两个渲染面上，而它们必然各自演化（本文件既有纪律：前端只翻译 kind）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeActionScope {
    /// 图谱维护区：只要**做完之后图谱当场变密**的那两档。
    Graph,
    /// 「知识库 → 概览」的下一步（PRD-07-024a ②）：其余全部。
    Overview,
}

impl KnowledgeActionScope {
    /// 这一格要不要这一类推荐。
    /// 目标态已落地（2026-08-30 二阶段）：推荐卡提取成两处共用的组件之后，概览有了渲染
    /// 面，`PRD-16-039` 修订注写的分工可以完整兑现。
    fn accepts(self, kind: KnowledgeNextActionKind) -> bool {
        // 图谱只留「做完之后图谱当场变密」的两档——标签就是图谱的边，补完当场看得见。
        let graph_scoped = matches!(
            kind,
            KnowledgeNextActionKind::MissingTags | KnowledgeNextActionKind::TagLanguageMismatch
        );
        // 主题页机会归概览（2026-08-30 三阶段，`PRD-16-027` 收口）：概览此前同屏摆着
        // 脉搏与推荐卡两套引擎，真机坐实了矛盾——上一行说「有 7 篇都在讲…」、下一行说
        // 「暂时不用整理」。现在概览只剩本命令一个引擎，脉搏退居启动概览弹窗。
        match self {
            Self::Graph => graph_scoped,
            Self::Overview => !graph_scoped,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeActionBlocker {
    LocalModelUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct KnowledgeNextActionDto {
    pub suggestion_token: String,
    pub kind: KnowledgeNextActionKind,
    pub count: u32,
    /// 主动作由 Rust 决定；前端只派发这个穷尽枚举，不再按 kind 猜。
    pub action: KnowledgeNextActionCommand,
    pub blocker: Option<KnowledgeActionBlocker>,
    /// 只有修复就绪性时携带精确深链。
    pub target: Option<String>,
    /// 地图冲突时的候选清单；其它种类恒空。
    pub maps: Vec<NoteRefDto>,
    /// 用户接管的地图需要更新时，携带基于当前磁盘正文生成的唯一差异。
    pub map_review: Option<KnowledgeMapReviewDto>,
    /// 仅主题机会携带；其余候选不让前端反查另一个来源决定动作。
    pub topic: Option<TopicSuggestionDto>,
    /// 仅「标签语言不一致」携带：出现最多的几条另一种语言的标签（C16）；其余种类恒空。
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct KnowledgeMapReviewDto {
    pub path: String,
    pub current_hash: String,
    pub diff: LineDiffDto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReceiptOutcome {
    Saved,
    Queued,
    Updated,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeReceiptKind {
    Distill,
    Merge,
    Topic,
    ChatWriteback,
    KnowledgeMap,
    Ingest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeMaintenanceStep {
    Tag,
    Embed,
    /// 为把这篇当作候选的主题页后台准备整理建议（只封存计划，写入仍待用户确认）。
    Ingest,
    KnowledgeMap,
    /// 文件已写成，全文/派生索引刷新失败；不能误报成文件未写入而重复写盘。
    Index,
    /// 文件已写成，系统底版未能记录；此后自动维护会保守停下。
    Baseline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KnowledgeReceiptAction {
    OpenNote,
}

/// 统一知识回执：只陈述已经落盘或已成功入队的事实。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "knowledge://receipt")]
pub(crate) struct KnowledgeReceiptDto {
    pub receipt_id: String,
    pub operation_id: String,
    pub revision: u32,
    pub kind: KnowledgeReceiptKind,
    pub outcome: ReceiptOutcome,
    pub primary_note: Option<NoteRefDto>,
    pub updated_topics: Vec<NoteRefDto>,
    pub queued_steps: Vec<KnowledgeMaintenanceStep>,
    pub degraded_steps: Vec<KnowledgeMaintenanceStep>,
    pub primary_action: Option<KnowledgeReceiptAction>,
    /// 本次实际写进主题页的条目数；只有逐条确认的计划应用才有。它与目录的「候选来源」、
    /// 预览的「可添加条目」是三个不同的事实，不能互相顶替（TDD-07 §4.1e）。
    pub applied_items: Option<u32>,
}

/// 一条回执要陈述的**已发生事实**。终态由这些事实推导，调用方不再传一个自由字符串——
/// 「已更新」与「等你确认」必须来自真实的写盘结果（PRD-16-038 / TDD-06 §3.7.4）。
#[derive(Default)]
pub(crate) struct KnowledgeReceiptFacts {
    /// 本次真正落盘更新的主题页。
    pub updated_topics: Vec<String>,
    /// 明确失败的封闭步骤码。
    pub degraded_steps: Vec<KnowledgeMaintenanceStep>,
    /// 整个生命周期任务失败（文件已落盘，但后台维护没跑起来）。
    pub failed: bool,
    /// 实际写入的条目数（计划应用专用）。
    pub applied_items: Option<u32>,
}

/// 回执的两个事实版本：`Committed` = 文件已落盘、后台步骤已入队；`Settled` = 后台步骤
/// 已到终态。同一 `operation_id` 只有这两个可见事实版本，故 revision 恒为 1 / 2。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiptStage {
    Committed,
    Settled,
}

fn receipt_kind_of(kind: &str) -> KnowledgeReceiptKind {
    match kind {
        "distill" => KnowledgeReceiptKind::Distill,
        "topic_draft" => KnowledgeReceiptKind::Topic,
        "chat_writeback" => KnowledgeReceiptKind::ChatWriteback,
        "knowledge_map" => KnowledgeReceiptKind::KnowledgeMap,
        "ingest" => KnowledgeReceiptKind::Ingest,
        _ => KnowledgeReceiptKind::Merge,
    }
}

/// 由事实推导终态。**穷尽且互斥**，且绝不把「任务已提交」说成「主题页已更新」。
fn outcome_of(
    stage: ReceiptStage,
    facts: &KnowledgeReceiptFacts,
    queued_steps: &[KnowledgeMaintenanceStep],
) -> ReceiptOutcome {
    match stage {
        ReceiptStage::Committed => {
            if queued_steps.is_empty() {
                // 三项设置都关着：写完就真的完了，没有「正在整理」可言。
                ReceiptOutcome::Saved
            } else {
                ReceiptOutcome::Queued
            }
        }
        ReceiptStage::Settled => {
            if facts.failed {
                ReceiptOutcome::Failed
            } else if !facts.degraded_steps.is_empty() {
                ReceiptOutcome::Degraded
            } else if !facts.updated_topics.is_empty() {
                ReceiptOutcome::Updated
            } else {
                ReceiptOutcome::Saved
            }
        }
    }
}

pub(crate) fn emit_knowledge_receipt(
    app: &tauri::AppHandle,
    operation_id: &str,
    kind: &str,
    note_rel: &str,
    facts: KnowledgeReceiptFacts,
) {
    emit_knowledge_receipt_facts(
        app,
        operation_id,
        kind,
        note_rel,
        ReceiptStage::Settled,
        Vec::new(),
        facts,
    );
}

fn emit_knowledge_receipt_facts(
    app: &tauri::AppHandle,
    operation_id: &str,
    kind: &str,
    note_rel: &str,
    stage: ReceiptStage,
    queued_steps: Vec<KnowledgeMaintenanceStep>,
    facts: KnowledgeReceiptFacts,
) {
    let kind = receipt_kind_of(kind);
    let outcome = outcome_of(stage, &facts, &queued_steps);
    // 终态与提交态说的是同一句话（都没有任何后台步骤）时不再播第二张卡：
    // revision 只在**对用户可见的事实**变化时递增（TDD-06 §3.7.4）。
    if stage == ReceiptStage::Settled
        && outcome == ReceiptOutcome::Saved
        && facts.degraded_steps.is_empty()
    {
        return;
    }
    let revision = match stage {
        ReceiptStage::Committed => 1,
        ReceiptStage::Settled => 2,
    };
    let receipt_id = luna_sync_core::hashing::ContentHash::of(
        format!("knowledge-receipt-v1|{operation_id}|{revision}|{outcome:?}").as_bytes(),
    )
    .to_string();
    let _ = KnowledgeReceiptDto {
        receipt_id,
        operation_id: operation_id.to_owned(),
        revision,
        kind,
        outcome,
        primary_note: Some(NoteRefDto {
            path: note_rel.to_owned(),
        }),
        updated_topics: facts
            .updated_topics
            .iter()
            .map(|path| NoteRefDto { path: path.clone() })
            .collect(),
        queued_steps,
        degraded_steps: facts.degraded_steps.clone(),
        // 一条回执**最多一个**动作（PRD-16-038）：打开这篇知识笔记。待确认的整理建议在
        // 主题页上就地预览，不经回执转达。
        primary_action: Some(KnowledgeReceiptAction::OpenNote),
        applied_items: facts.applied_items,
    }
    .emit(app);
}

/// 已经落入续跑档、但文件尚未写出的知识意图。所有知识生产入口必须按
/// `prepare → write → commit` 的顺序使用它；这不是给界面展示的任务。
pub(crate) struct PreparedKnowledgeWrite {
    task_id: u64,
    operation_id: String,
    origin: String,
    note_rel: String,
    aggregate: bool,
    created_new: bool,
    topic_src: Option<String>,
    rollback: Option<(String, String, String)>,
}

impl PreparedKnowledgeWrite {
    pub(crate) fn with_topic_src(mut self, topic_src: &str) -> Self {
        self.topic_src = Some(topic_src.to_owned());
        self
    }

    pub(crate) fn with_rollback(
        mut self,
        suggestion_token: &str,
        before_fingerprint: &str,
        after_fingerprint: &str,
    ) -> Self {
        self.rollback = Some((
            suggestion_token.to_owned(),
            before_fingerprint.to_owned(),
            after_fingerprint.to_owned(),
        ));
        self
    }
}

/// 在 Markdown 写盘前持久化知识生命周期意图。`body` 必须是最终写入后的正文（不含
/// frontmatter）；worker 也按 Vault 的 `body_fingerprint` 校验同一口径。
///
/// **本函数自己要取 vault 锁**（为了回读判定 `created_new`），故**绝不能在已经持有
/// vault 锁的闭包里调用它**——`std::sync::Mutex` 不可重入，取即死锁。已经拿着锁的调用方
/// 走 [`prepare_knowledge_write_locked`]。
pub(crate) fn prepare_knowledge_write(
    app: &tauri::AppHandle,
    note_rel: &str,
    body: &str,
    origin: &str,
    aggregate: bool,
) -> Result<PreparedKnowledgeWrite, String> {
    let state = app.state::<AppState>();
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let created_new = match vault.read(note_rel) {
        Ok(_) => false,
        Err(luna_vault::VaultError::NotFound(_)) => true,
        Err(error) => return Err(vault_err(error)),
    };
    let prepared =
        prepare_knowledge_write_locked(app, vault, note_rel, body, origin, aggregate, created_new);
    drop(guard);
    prepared
}

/// 纯登记版：**不碰 vault 锁**，供已经持有它的发布路径在同一个临界区里登记 intent。
///
/// **为什么必须有这一版**：汇总的发布路径要在**一次**加锁里做完「复验代际 → 复验源
/// 指纹 → 挑落点 → 原子创建 → 登记 intent → 写内容」。少了它，登记只能挪到锁外，而那正是
/// 修复前的形状——落点是在锁里挑的、intent 是在锁外登记的，两者之间存在一个谁都补不上的
/// 窗口。直接在锁里调 [`prepare_knowledge_write`] 则是**当场死锁**（`std::sync::Mutex`
/// 不可重入）。
///
/// **`created_new` 显式传入，不再靠回读推断**：发布路径的落点是刚由 `link(2)` 创建
/// 出来的，"这是不是一次新建"是**已知事实**而不是需要猜的东西。回读推断在那条路上还必然
/// 出错：任何"先在落点建个占位、再登记"的做法都会让回读看到文件、把新建判成 false，
/// 于是汇总产物永远进不了知识度量的「新建知识笔记」那一类。
///
/// **锁序方向：`vault → processing.inner`，只此一个方向。** 本函数在 `vault` 之内取
/// `processing.inner`（`ProcessingQueue::prepare_knowledge`）；反方向（持 `processing.inner`
/// 再取 `vault`）在全仓不存在，也不许出现。`_vault` 参数不是数据，是"调用方确实拿着那把
/// 锁"的编译期凭据——本 crate 里 `&Vault` 只可能来自 `AppState.vault` 的 guard。
pub(crate) fn prepare_knowledge_write_locked(
    app: &tauri::AppHandle,
    _vault: &luna_vault::Vault,
    note_rel: &str,
    body: &str,
    origin: &str,
    aggregate: bool,
    created_new: bool,
) -> Result<PreparedKnowledgeWrite, String> {
    let operation_id = uuid::Uuid::now_v7().to_string();
    let kind = crate::processing_queue::ProcTaskKind::KnowledgeLifecycle {
        operation_id: operation_id.clone(),
        vault_generation: app.state::<AppState>().vault_generation(),
        note_rel: note_rel.to_owned(),
        // 与 `Vault::knowledge_body_fingerprint` 同一口径（TDD-06 §3.7.1）：写盘前
        // 只有候选正文，落盘后 `body_of` 还含着 frontmatter 之后那个分隔空行。
        body_hash: luna_vault::knowledge_body_hash(body),
        origin: origin.to_owned(),
        aggregate,
    };
    let task_id = app
        .state::<AppState>()
        .processing
        .prepare_knowledge(kind, super::transcribe::note_task_label(note_rel))?;
    Ok(PreparedKnowledgeWrite {
        task_id,
        operation_id,
        origin: origin.to_owned(),
        note_rel: note_rel.to_owned(),
        aggregate,
        created_new,
        topic_src: None,
        rollback: None,
    })
}

/// 文件写盘失败时撤销 Prepared；调用方应在自己的写入错误分支调用。
pub(crate) fn abandon_knowledge_write(app: &tauri::AppHandle, write: PreparedKnowledgeWrite) {
    app.state::<AppState>()
        .processing
        .abandon_prepared_knowledge(write.task_id);
}

/// 文件原子写成功后的唯一提交点：推进续跑档、发“已保存/维护中”回执并拉起 worker。
pub(crate) fn commit_knowledge_write(
    app: &tauri::AppHandle,
    write: PreparedKnowledgeWrite,
) -> Result<(), String> {
    let result = app
        .state::<AppState>()
        .processing
        .mark_knowledge_written(write.task_id);
    // 即使第二次持久化失败，内存项也已经是 Queued，而磁盘仍保留 Prepared；两边都能安全
    // 恢复，所以继续拉 worker，不能让已经写出的用户资产永久少维护一步。
    super::transcribe::emit_processing_queue(app);
    super::transcribe::start_processing_worker_if_idle(app);
    let settings = app.state::<AppState>().settings_snapshot();
    let mut queued_steps = Vec::new();
    if write.origin != "knowledge_map" && settings.get_bool("vault.graph_ai_tags").unwrap_or(true) {
        queued_steps.push(KnowledgeMaintenanceStep::Tag);
    }
    if super::embed::semantic_enabled(&app.state::<AppState>())
        && super::embed::embed_model(&app.state::<AppState>()).is_some()
    {
        queued_steps.push(KnowledgeMaintenanceStep::Embed);
    }
    if write.origin != "knowledge_map" {
        if write.aggregate {
            queued_steps.push(KnowledgeMaintenanceStep::KnowledgeMap);
        } else if settings
            .get_str("knowledge.ingest_mode")
            .is_some_and(|mode| mode != "off")
        {
            queued_steps.push(KnowledgeMaintenanceStep::Ingest);
        }
    }
    emit_knowledge_receipt_facts(
        app,
        &write.operation_id,
        &write.origin,
        &write.note_rel,
        ReceiptStage::Committed,
        queued_steps,
        KnowledgeReceiptFacts::default(),
    );
    let state = app.state::<AppState>();
    let enabled = state
        .settings_snapshot()
        .get_bool("privacy.local_knowledge_metrics")
        .unwrap_or(false);
    if enabled {
        if let Some(root) = state.vault_root() {
            let root = root.display().to_string();
            let now_ms = chrono::Utc::now().timestamp_millis();
            let _ = app
                .try_state::<crate::metrics::LocalKnowledgeMetrics>()
                .and_then(|metrics| {
                    // 授权凭据在这条知识写入的落定处取（TDD-12 §2.1）。
                    let grant = metrics.begin()?;
                    metrics.with_store(grant, |store| {
                        if write.created_new
                            && !write.aggregate
                            && matches!(
                                write.origin.as_str(),
                                "distill" | "merge" | "chat_writeback" | "upgrade"
                            )
                        {
                            store.leaf_created(
                                &root,
                                &write.operation_id,
                                &write.note_rel,
                                now_ms,
                            )?;
                        }
                        if let Some(topic_src) = write.topic_src.as_deref() {
                            store.topic_created(&root, topic_src, &write.operation_id, now_ms)?;
                        }
                        if let Some((suggestion_token, before, after)) = write.rollback.as_ref() {
                            store.suggestion(
                                &root,
                                suggestion_token,
                                luna_metrics::SuggestionEvent::Accepted,
                                None,
                                now_ms,
                            )?;
                            store.register_rollback(
                                &root,
                                luna_metrics::RollbackWindow {
                                    suggestion_token,
                                    operation_id: &write.operation_id,
                                    note_path: &write.note_rel,
                                    before_fingerprint: before,
                                    after_fingerprint: after,
                                    now_ms,
                                },
                            )?;
                        }
                        Ok(())
                    })
                });
        }
    }
    super::schedule_local_knowledge_metrics_reconcile(app);
    result
}

/// `@` 提及的一个候选（PRD-16-009）。仅下行。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct MentionCandidateDto {
    /// 范围形态：`note` / `folder` / `tag`（回传时原样带回，判定在 Rust）。
    pub kind: String,
    /// 范围值：笔记/文件夹相对路径，或标签名。
    pub value: String,
    /// 展示名（笔记与文件夹取末级名，标签即自身）。
    pub label: String,
}

/// 「相关」面板数据（PRD-16-013）。仅下行（Rust → 前端），故不派生 `Deserialize`。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct RelatedNotesDto {
    /// 未链接提及。
    pub mentions: Vec<UnlinkedMentionDto>,
    /// 相关笔记。
    pub related: Vec<NoteRefDto>,
    /// 本篇是否可写入链接（`false` = 资料笔记，只显示发现不给写入动作，PRD-16-014）。
    /// 判定在 Rust——前端隐藏按钮只是不显示，命令仍可达。
    pub can_link: bool,
}

// ── 检索与上下文合成 ─────────────────────────────────────────────────────────

/// 把检索结果投影为出处 DTO（编号即 `[n]`）。
pub(crate) fn sources_of(retrieval: &luna_vault::Retrieval) -> Vec<RetrievalSourceDto> {
    retrieval
        .passages
        .iter()
        .enumerate()
        .map(|(i, passage)| RetrievalSourceDto {
            index: FIRST_CITATION_INDEX + i as u32,
            path: passage.path.clone(),
            title: passage.title.clone(),
            note_type: passage.note_type.clone(),
            source_material: NoteType::from_key(&passage.note_type)
                .is_some_and(NoteType::is_source_material),
            draft: passage.draft,
            snippet: passage.snippet(),
            anchor: CitationAnchorDto::from(&passage.anchor),
        })
        .collect()
}

/// 合成本轮的**瞬态上下文**（ADR-026 ⑤：随本轮提交、不进历史、不落归档）。
///
/// 三条排布纪律：① 指令在前、材料在后（模型对开头的指令遵循最好）；② 每段材料带
/// `[n]` 与标题，模型据此产出可核对的引用编号；③ 明说「不要编造摘录之外的内容」并
/// 允许如实说不知道——检索为空的降级由调用方处理，此处只管有材料时的纪律。
///
/// 语言跟随界面语言：让模型用用户读得懂的语言解释引用，比统一英文更少一次心智转换。
///
/// 参数取的是 **`Passage`（完整摘录）而非 DTO**：DTO 里的 `snippet` 是给 hover 预览用的
/// 短摘要，拿它当模型的依据等于只喂了每篇的头一句——检索做了、上下文却是残缺的。
pub(crate) fn compose_grounding_prompt(passages: &[luna_vault::Passage], zh: bool) -> String {
    let mut out = String::new();
    // 这里写的每一条都是 `classify_grounded_answer` **真的会去判**的规则，两边必须对齐：
    // 判定是 fail closed 的，模型不知道的规则一旦违反，那一段就会被划到「模型补充」。
    // 刻意写得短：真机上把规则写成五条带强调的清单，2B 模型会把规则本身复述进回答里。
    // 前言归注册表 `knowledge.grounding`（ADR-050 修订 2026-09-05），中文是 zh-CN 变体。
    // 补充分隔符由代码注入（`{supplement_marker}`）而不写死在模板里：它同时是
    // `classify_grounded_answer` 的判定锚点，运营改一个字符就会让判定整体失效。
    out.push_str(&luna_core::prompt::registry().render_for_locale(
        luna_core::PromptScope::Knowledge,
        "grounding",
        if zh { Some("zh-CN") } else { None },
        &[("supplement_marker", MODEL_SUPPLEMENT_SENTINEL)],
    ));
    for (i, passage) in passages.iter().enumerate() {
        out.push_str(&format!(
            "[{}] {}\n{}\n\n",
            FIRST_CITATION_INDEX + i as u32,
            passage.title,
            passage.text
        ));
    }
    out.trim_end().to_string()
}

/// 读取「默认是否检索知识库」（设置 `chat.grounding`，PRD-16 D1：它只是默认档）。
pub(crate) fn grounding_default(state: &tauri::State<'_, AppState>) -> bool {
    state
        .settings_snapshot()
        .get_bool("chat.grounding")
        .unwrap_or(true)
}

/// 执行一次库内检索并投影为透明信息。Vault 未打开或无检索词时返回 `None`——
/// **不报错**：检索是增值能力，取不到就走无依据回答（PRD-16-007 诚实降级）。
/// 摘录里给主题页声明加的行内说明（界面语言）。检索加注与回答收尾剔除共用这一份。
pub(crate) fn claim_notices(app: &tauri::AppHandle) -> luna_vault::ClaimNotices {
    luna_vault::ClaimNotices {
        source_changed: backend_text(app, "claim-source-changed", "〔来源已变，待核对〕"),
        missing: backend_text(app, "claim-source-missing", "〔出处已不存在，未核验〕"),
        conflict: backend_text(
            app,
            "claim-conflict",
            "〔本页对此有分歧说法，未判定哪方正确〕",
        ),
    }
}

/// 把模型原样抄进回答的加注剔掉（真机 2026-09-27：云端模型把「〔本页对此有分歧说法…〕」
/// 连同摘录一起抄了出来）。加注是给模型看的系统说明，不是回答内容；提示词已要求不抄，
/// 这里按原文逐字兜底，只删整段说明，不碰回答里别的方括号。
pub(crate) fn strip_claim_notices(text: &str, notices: &luna_vault::ClaimNotices) -> String {
    let mut out = text.to_owned();
    for notice in [&notices.source_changed, &notices.missing, &notices.conflict] {
        // 先删带尾随分隔的原文（英文加注自带一个空格），再删去掉首尾空白的形态。
        for form in [notice.as_str(), notice.trim()] {
            if !form.trim().is_empty() {
                out = out.replace(form, "");
            }
        }
    }
    out
}

pub(crate) async fn retrieve_for_question(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    question: &str,
    overrides: &super::chat::RetrievalOverrides,
) -> Option<(RetrievalInfoDto, String)> {
    let (include_chat, excluded) = (overrides.include_chat(), overrides.excluded());
    // 「排除记录并重答」必须由 Rust 承担（PRD-16-041）：前端只能删掉它看得见的那几条，
    // 库里其余记录态会在下一轮补位——用户要的是**重新检索**，不是把清单擦干净。
    let exclude_drafts = overrides.exclude_drafts();
    let options = luna_vault::RetrieveOptions {
        include_chat,
        exclude_drafts,
        excluded: excluded.to_vec(),
        scope: overrides.scope().to_vec(),
        // 静默升级（PRD-16-029）：缺模型 / 关掉语义 / 库里还没向量时恒为空，
        // 检索退回纯关键词且与批次 1 逐字节一致。
        semantic: super::embed::semantic_hits(state, question).await,
        // 主题页里出处已失效或有分歧说法的声明在摘录里标出来，模型不把它们当作当前已核实、无争议的事实（K4/K5）。
        claim_notices: claim_notices(app),
        ..luna_vault::RetrieveOptions::default()
    };
    let retrieval = {
        let guard = state.vault.lock().ok()?;
        let vault = guard.as_ref()?;
        vault.retrieve(question, &options).ok()?
    };
    let sources = sources_of(&retrieval);
    let prompt = if sources.is_empty() {
        String::new()
    } else {
        compose_grounding_prompt(&retrieval.passages, is_zh(app))
    };
    Some((
        RetrievalInfoDto {
            scanned: retrieval.scanned,
            sources,
            include_chat,
            exclude_drafts,
            excluded: excluded.to_vec(),
            truncated: retrieval.truncated,
        },
        prompt,
    ))
}

/// 界面语言是否中文系（提示词与出处标签跟随它）。
pub(crate) fn is_zh(app: &tauri::AppHandle) -> bool {
    backend_copy_arc(app)
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string())
        .starts_with("zh")
}

/// 从回答正文里解析被实际引用的编号（`[1]` / `[1][2]` / `[1,2]`）。
///
/// 用途是「引用 M 篇」这个数字与出处芯片行的过滤——只显示模型真用到的那几条，
/// 而不是把检索到的全都摆出来（后者会让用户以为每条都被采信了）。判定在 Rust：
/// 前端只渲染，不解析回答文本（ADR-003）。
#[must_use]
pub(crate) fn cited_indices(answer: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let bytes = answer.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(close) = answer[i..].find(']') else {
            break;
        };
        let inner = &answer[i + 1..i + close];
        // 只接受纯数字与分隔符的方括号，`[[wikilink]]` 与 `[文本](链接)` 天然不匹配。
        if !inner.is_empty()
            && inner
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == ' ' || c == '，')
        {
            for part in inner.split([',', '，', ' ']) {
                if let Ok(n) = part.trim().parse::<u32>() {
                    if n >= FIRST_CITATION_INDEX && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        i += close + 1;
    }
    out.sort_unstable();
    out
}

/// 出处的持久化形态（PRD-16-011）：`**出处：** [标题](相对路径) · …`
///
/// **刻意不用 blockquote**（`> [!warning]` 已被 callout 语法占用，争首行）、
/// **刻意不用脚注**（`[^1]` 不在 CommonMark 核心，Lunarca 自己的渲染器会显示成字面量）。
/// 链接恒为标准相对 md 链接（PRD-06-030 可移植性裁决，构造点唯一：
/// `luna_vault::markdown_link`）。
#[must_use]
pub(crate) fn render_sources_line(
    from_note: &str,
    sources: &[RetrievalSourceDto],
    zh: bool,
) -> String {
    if sources.is_empty() {
        return String::new();
    }
    let label = if zh { "出处：" } else { "Sources: " };
    let links: Vec<String> = sources
        .iter()
        .map(|s| luna_vault::markdown_link(from_note, &s.path, &s.title))
        .collect();
    format!("**{label}**{}", links.join(" · "))
}

/// 计数的计算边界。下界不得显示成精确总数，未生成计划不得显示为零条可添加声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum KnowledgeCountDto {
    NotComputed,
    LowerBound { count: u32 },
    Exact { count: u32 },
}

/// 主题页目录的一行（ADR-088 ② / TDD-07 §4.1d）。**全部由既有事实派生**：
/// 不新建索引结构、不新建文件、不新建所有权表示。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TopicDirectoryRowDto {
    /// 库相对路径（行动作的落点；主题页是一篇真实 Markdown）。
    pub path: String,
    /// 页名（= 文件名 stem，与文件树同一口径）。
    pub title: String,
    /// **确定性一句摘要**——与知识地图同一条规则、同一个函数（`luna_vault::topic_summary`）。
    /// 空字符串 = 这一页还没有可读的正文段落（如实留空，不编一句）。
    pub summary: String,
    /// 这一页此刻归谁（与单页状态查询共用同一个枚举）。
    pub ownership: TopicOwnershipDto,
    /// 只统计共享标签且尚未被这一页引用的来源；检索尚未运行，因此仅是可能相关来源的下界。
    pub candidate_source_count: KnowledgeCountDto,
    /// 只有当前有效的封存计划才能给出可添加声明数；目录查询不生成计划。
    pub ready_item_count: KnowledgeCountDto,
    /// 系统最近一次写入这一页的时刻（Unix 毫秒）；没有系统底版为 `None`——与知识地图
    /// 「最近更新」同一个事实（PRD-16-061）。人工改正文不算：那不是 Lunarca 整理过。
    pub updated_ms: Option<f64>,
    /// 相关主题的路径（与本页引用了同一篇来源，按共同来源数降序，至多 3 条）——与知识地图
    /// 同一个函数（`luna_vault::related_topics`）。
    pub related: Vec<String>,
}

/// 0 页时那句空态说什么（PRD-16-016 的三态，**判据在 Rust，前端只渲染**）。
///
/// **③「一个字都不提」是靠结构成立的**，不是靠前端记得别渲染：总开关关着时
/// `checkup_t0` 根本不产出「还没有标签的知识笔记」这条发现（PRD-16-030），于是
/// [`Self::Untagged`] 这一档**不可能**被取到，自然落到教学空态。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum TopicDirectoryEmptyDto {
    /// ① 有合格标签簇 → 建议一行 + [合成主题页]。与另外两个表面**同一份状态与动作**。
    Suggestion { suggestion: TopicSuggestionDto },
    /// ② 无簇，但体检存在「还没有标签的知识笔记」→ 直说这件事 + 通向全库补齐。
    Untagged { count: u32 },
    /// ③ 无话可说 → 纯教学空态（含总开关关闭的情形）。
    Teaching,
}

/// 「知识库 → 主题页」分区的一次取数。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TopicDirectoryDto {
    pub rows: Vec<TopicDirectoryRowDto>,
    /// 按已确认标签层级的分组与顺序——与知识地图同一个函数（`luna_vault::group_topics`）。
    /// 一页可在多组出现，页数按 `rows` 去重计。
    pub groups: Vec<TopicDirectoryGroupDto>,
    /// 只在 `rows` 为空时有值——有页的时候不去跑体检扫描（那是一次全库遍历）。
    pub empty: Option<TopicDirectoryEmptyDto>,
    /// 「最近更新」的那几页（路径，至多 5 页）——与知识地图同一个函数（`luna_vault::recent_topics`）。
    pub recent: Vec<String>,
}

/// 目录里的一组主题（PRD-16-050）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TopicDirectoryGroupDto {
    /// 分组名（已确认标签的顶层段）；`None` = 「其他主题」（没有已确认标签的页）。
    pub name: Option<String>,
    /// 组内主题页路径（组内顺序即显示顺序）。
    pub paths: Vec<String>,
}

/// 新版主题的维护闸门；旧版、未知版本与证据损坏必须各有明确状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicMaintenanceStateDto {
    Ready,
    NotTopic,
    NeedsUpgrade,
    UnknownSchema,
    InvalidEvidence,
    BodyChanged,
    MetadataChanged,
    MissingBaseline,
}

impl From<luna_vault::TopicMaintenanceState> for TopicMaintenanceStateDto {
    fn from(value: luna_vault::TopicMaintenanceState) -> Self {
        use luna_vault::TopicMaintenanceState as State;
        match value {
            State::Ready => Self::Ready,
            State::NotTopic => Self::NotTopic,
            State::NeedsUpgrade => Self::NeedsUpgrade,
            State::UnknownSchema => Self::UnknownSchema,
            State::InvalidEvidence => Self::InvalidEvidence,
            State::BodyChanged => Self::BodyChanged,
            State::MetadataChanged => Self::MetadataChanged,
            State::MissingBaseline => Self::MissingBaseline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TopicEvidenceStateDto {
    Current,
    SourceChanged,
    Missing,
    Ambiguous,
    Partial,
}

impl From<luna_vault::EvidenceState> for TopicEvidenceStateDto {
    fn from(value: luna_vault::EvidenceState) -> Self {
        use luna_vault::EvidenceState as State;
        match value {
            State::Current => Self::Current,
            State::SourceChanged => Self::SourceChanged,
            State::Missing => Self::Missing,
            State::Ambiguous => Self::Ambiguous,
            State::Partial => Self::Partial,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicCitationStateDto {
    pub id: String,
    pub source_path: Option<String>,
    pub state: TopicEvidenceStateDto,
}

/// `None` = 此页尚无可解析的新版证据，不能误显示为“零条来源”。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TopicEvidenceDetailDto {
    pub path: String,
    pub revision: String,
    pub maintenance: TopicMaintenanceStateDto,
    /// 无法解析新版结构时为 `None`，不能用 0 冒充已计算的总数。
    pub total_citations: Option<u32>,
    pub citations: Option<Vec<TopicCitationStateDto>>,
    pub next_offset: Option<u32>,
}

fn topic_evidence_detail_of(
    vault: &luna_vault::Vault,
    path: &str,
    offset: u32,
    limit: u32,
    expected_revision: Option<&str>,
) -> Result<TopicEvidenceDetailDto, String> {
    let topic = vault.read(path).map_err(vault_err)?;
    let revision = luna_sync_core::hashing::ContentHash::of(topic.content.as_bytes()).to_string();
    if expected_revision.is_some_and(|expected| expected != revision) {
        return Err("TOPIC_EVIDENCE_STALE".into());
    }
    let maintenance: TopicMaintenanceStateDto = vault
        .topic_maintenance_state(path)
        .map_err(vault_err)?
        .into();
    let page = if matches!(
        maintenance,
        TopicMaintenanceStateDto::Ready
            | TopicMaintenanceStateDto::BodyChanged
            | TopicMaintenanceStateDto::MetadataChanged
            | TopicMaintenanceStateDto::MissingBaseline
    ) {
        Some(
            vault
                .topic_evidence_page(path, offset, limit, Some(&revision))
                .map_err(vault_err)?,
        )
    } else {
        None
    };
    let (total_citations, citations, next_offset) = match page {
        Some(page) => (
            Some(page.total),
            Some(
                page.items
                    .into_iter()
                    .map(|item| TopicCitationStateDto {
                        id: item.id,
                        source_path: item.source_path,
                        state: item.state.into(),
                    })
                    .collect(),
            ),
            page.next_offset,
        ),
        None => (None, None, None),
    };
    let latest = vault.read(path).map_err(vault_err)?;
    if luna_sync_core::hashing::ContentHash::of(latest.content.as_bytes()).as_str() != revision {
        return Err("TOPIC_EVIDENCE_STALE".into());
    }
    Ok(TopicEvidenceDetailDto {
        path: path.to_owned(),
        revision,
        maintenance,
        citations,
        total_citations,
        next_offset,
    })
}

/// 目录行的构建（纯取数，抽出来是为了让「摘要与地图同源」可被一条测试直接证明）。
/// 「还没有 AI 关系标签」的那条发现——**两个消费面共用的唯一判据**。
///
/// `Untagged` 是**一条聚合发现**，篇数在它自己的 `count` 里；按 `filter(..).count()` 数
/// 发现条数永远只得到 0 或 1。主题页目录空态与「下一步」建议卡都读它，体检卡读的也是同一
/// 个字段，三处因此不可能各说一个数。
fn untagged_finding(findings: &[luna_vault::Finding]) -> Option<&luna_vault::Finding> {
    findings
        .iter()
        .find(|finding| matches!(finding.kind, luna_vault::FindingKind::Untagged))
}

pub(crate) fn topic_directory_rows(
    vault: &luna_vault::Vault,
    policy: luna_vault::SourcePolicy,
) -> Result<TopicDirectoryDto, String> {
    let mut rows = Vec::new();
    let mut contents: Vec<(String, String)> = Vec::new();
    for path in vault.topic_pages().map_err(vault_err)? {
        let doc = vault.read(&path).map_err(vault_err)?;
        contents.push((path.clone(), doc.content.clone()));
        // 用刚读到的那份正文，不再为同一行把文件读第二遍（也保证这一行的四个字段同源）。
        // 与「检查可添加内容」同一个候选判据（剔除当前版本已被否决的来源）。随库决定
        // 读不出时退回未剔除的下界查询：一个坏掉的决定文件不该让整个目录打不开。
        let pending = match vault.topic_candidate_sources_of(&path, &doc.content, policy) {
            Ok(pending) => pending,
            Err(_) => vault
                .topic_ingest_pending_of(&path, &doc.content)
                .map_err(vault_err)?,
        };
        rows.push(TopicDirectoryRowDto {
            title: luna_sync_core::links::title_of(&path).to_owned(),
            // **就是地图那一条规则**：不为目录另写一份摘要、也不落第二份缓存
            //（TDD-07 §4.1d）。两份实现分叉的表现是同一页在两处各有一句自称的摘要。
            summary: luna_vault::topic_summary(&doc.content),
            ownership: luna_vault::ownership_of(&doc.content).into(),
            candidate_source_count: KnowledgeCountDto::LowerBound {
                count: u32::try_from(pending.len()).unwrap_or(u32::MAX),
            },
            // 后台或手动准备好、仍然有效的待确认计划：给出精确条数；没有就是未计算，
            // 不跑检索或模型去估一个数。
            ready_item_count: vault
                .sealed_plan_ids_for_target(&path)
                .into_iter()
                .find_map(|id| {
                    let plan = vault.load_sealed_ingest_plan(id).ok()?;
                    vault.check_ingest_plan(&plan).ok()?;
                    Some(KnowledgeCountDto::Exact {
                        count: u32::try_from(plan.actions.len()).unwrap_or(u32::MAX),
                    })
                })
                .unwrap_or(KnowledgeCountDto::NotComputed),
            #[allow(clippy::cast_precision_loss)]
            updated_ms: vault.topic_updated_ms(&path).map(|at| at as f64),
            related: Vec::new(),
            path,
        });
    }
    // 与地图同一条排序（标题 → 路径，均小写），两处列出来的顺序才一致。
    rows.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    // 分组、最近更新与相关主题读的就是上面这一轮读到的正文，与地图同一组函数、同一种顺序。
    let topics: Vec<luna_vault::KnowledgeMapTopic<'_>> = contents
        .iter()
        .map(|(path, content)| luna_vault::KnowledgeMapTopic {
            path,
            content,
            updated_ms: vault.topic_updated_ms(path),
        })
        .collect();
    let related = luna_vault::related_topics(&topics);
    for row in &mut rows {
        if let Some(others) = related.get(row.path.as_str()) {
            row.related = others.iter().map(|other| (*other).to_owned()).collect();
        }
    }
    let recent = luna_vault::recent_topics(&topics)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let groups = luna_vault::group_topics(&topics)
        .into_iter()
        .map(|group| TopicDirectoryGroupDto {
            name: group.name,
            paths: group
                .topics
                .iter()
                .map(|topic| topic.path.to_owned())
                .collect(),
        })
        .collect();
    Ok(TopicDirectoryDto {
        rows,
        groups,
        empty: None,
        recent,
    })
}

// ── 命令 ────────────────────────────────────────────────────────────────────

/// 「知识库 → 主题页」分区的目录（ADR-088 / TDD-07 §4.1d）。**只读投影，零 LLM**。
///
/// **一次下发四件事**（页名 / 摘要 / 所有权 / 候选与计划计数状态）：前端不读 frontmatter、
/// 不数建议、不推断所有权（ADR-003）。它是一次查询投影，不是第二存储目录——目录里
/// 一个字节都不落盘。
///
/// **行动作不在这里**：打开 / 立即归入 / 交还给 Lunarca / 针对本页体检全部落既有命令，
/// 本命令一条都不新增（"针对本页体检"更是**没有**单页体检命令——体检作用域是全库，
/// 「本页视角」= 打开体检视图并按这一页的路径过滤既有发现）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn topic_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<TopicDirectoryDto, String> {
    let settings = state.settings_snapshot();
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let listing =
        topic_directory_rows(vault, super::knowledge_curation::source_policy(&app, vault))?;
    if !listing.rows.is_empty() {
        return Ok(listing);
    }
    // ① 有合格标签簇：与启动概览、图谱侧栏同一份建议状态与同一条动作命令。
    //
    // **忽略态不参与这里**：`✕` 消掉的只是那两枚主动冒出来的提醒，分区空态恒显
    //（PRD-16-016 / TDD-07 §4.1c）。故这里取的是**未经忽略清单过滤**的建议——把
    // `dismissed` 也接到它的显隐上，等于让这个分区从此答不出「这里为什么是空的」。
    let empty = if let Some(suggestion) = vault
        .topic_suggestions(&[])
        .map_err(vault_err)?
        .into_iter()
        .next()
    {
        TopicDirectoryEmptyDto::Suggestion {
            suggestion: topic_suggestion_dto(vault, suggestion),
        }
    } else {
        // ② 无簇但「还没有标签的知识笔记」：直接说出这件事，而不是让用户对着一句
        // 「还没有主题页」自己去猜为什么簇凑不出来（PRD-16-016 ②）。
        let findings = vault
            .checkup_t0(
                settings.get_bool("vault.graph_ai_tags").unwrap_or(true),
                super::checkup::now_ms(),
            )
            .map_err(vault_err)?;
        match untagged_finding(&findings).map_or(0, |finding| finding.count) {
            0 => TopicDirectoryEmptyDto::Teaching,
            count => TopicDirectoryEmptyDto::Untagged { count },
        }
    };
    Ok(TopicDirectoryDto {
        empty: Some(empty),
        ..listing
    })
}

/// 新证据主题的只读状态；旧/未知格式返回具名维护状态，出处为 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn topic_evidence_detail(
    state: tauri::State<'_, AppState>,
    note_rel: String,
    offset: u32,
    limit: u32,
    expected_revision: Option<String>,
) -> Result<TopicEvidenceDetailDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    topic_evidence_detail_of(
        vault,
        &note_rel,
        offset,
        limit,
        expected_revision.as_deref(),
    )
}

/// 一条标签此刻的**动作资格**（PRD-06-033 的首选/次选分流）。仅下行、纯 Rust 零 LLM。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TagTopicActionsDto {
    /// 已有主题页以它为出身时的路径；`None` = 还没有。
    pub topic_page: Option<String>,
    /// 够不够合成一页：≥3 篇知识笔记 **且** 尚无主题页（PRD-16-016 判据，忽略态不参与）。
    pub can_create_topic: bool,
    /// 合成初稿的输入（与建议同一份候选；不够格时为空）。
    pub notes: Vec<String>,
    /// 用户在别处 `✕` 过这条建议。
    ///
    /// **它只改修辞，不改资格**（PRD-16-016）：首选去掉「建议」二字、降为普通可用态，
    /// 但**不消失**——他关掉的是一条建议，不是一个能力。故本位与 `can_create_topic`
    /// 分开下发，前端不得把它接到显隐上。
    pub dismissed: bool,
    /// 建议身份（与另外两个表面同一个 token）；不够格时为空串。
    pub suggestion_token: String,
}

/// 「知识库 → 标签」分区选中标签后的动作分流（PRD-06-033）。
///
/// **资格判定全在 Rust，前端不按计数自行推导**：「≥3 篇知识笔记」里的「知识笔记」
/// 同时需要来源轴与成熟度轴（ADR-086），前端拼一遍就是第二处会漂的定义——而漂了之后
/// 界面上摆着一枚点下去必然落空的按钮，没有任何东西会红。
#[tauri::command]
#[specta::specta]
pub(crate) async fn tag_topic_actions(
    state: tauri::State<'_, AppState>,
    tag: String,
) -> Result<TagTopicActionsDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let topic_page = vault.topic_page_for_tag(&tag).map_err(vault_err)?;
    let notes = vault.tag_knowledge_notes(&tag).map_err(vault_err)?;
    let can_create_topic =
        topic_page.is_none() && notes.len() >= luna_vault::TOPIC_SUGGESTION_MIN_NOTES;
    let dismissed = vault
        .topic_suggestion_ignores()
        .unwrap_or_default()
        .iter()
        .any(|ignored| ignored.eq_ignore_ascii_case(&tag));
    Ok(TagTopicActionsDto {
        suggestion_token: if can_create_topic {
            suggestion_token(vault.root(), "topic", &tag, &notes.join("\n"))
        } else {
            String::new()
        },
        notes: if can_create_topic { notes } else { Vec::new() },
        topic_page,
        can_create_topic,
        dismissed,
    })
}

/// 知识库脉搏的一次取数（PRD-16-027）。
///
/// 两个来源合成一个命令：脉搏渲染的是**一个优先级栈的栈顶**，分两次 IPC 取会让它先渲染
/// 一态、再跳成另一态——那正是「只渲染栈顶一行」想避免的抖动。两者都是纯 Rust 零 LLM。
#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_pulse(
    state: tauri::State<'_, AppState>,
) -> Result<KnowledgePulseDto, String> {
    let distill = super::distill::distill_preflight(state.clone(), None)?;
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let ignored = vault.topic_suggestion_ignores().unwrap_or_default();
    Ok(KnowledgePulseDto {
        distill,
        topics: vault
            .topic_suggestions(&ignored)
            .map_err(vault_err)?
            .into_iter()
            .map(|suggestion| topic_suggestion_dto(vault, suggestion))
            .collect(),
    })
}

/// 默认维护位的单推荐（TDD-06 §3.7.5）。候选彼此互斥，前端只消费结果、不再对各 hook 的
/// 到达顺序做排序。运行态由各任务自身的进度卡接管。
///
/// `scope` = 这一枚要摆在哪一格（`PRD-16-039` 2026-08-30 修订）。**排序对两格是同一条**，
/// 变的只是各自取其中哪几档：不合本格的候选**跳过并继续往下找**，而不是让这一格空着——
/// 否则图谱那格会因为「此刻排第一的是主题页机会」而显示「暂时不用整理」，而它自己那两档
/// 明明有事可做。
#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_next_action(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: KnowledgeActionScope,
) -> Result<Option<KnowledgeNextActionDto>, String> {
    let settings = state.settings_snapshot();
    // 与设置引导同一判据（`AppState::local_chat_ready`）：对话在服务端跑时不受本机模型约束。
    let llm_ready = super::current_chat_compute_location(&app, &state).is_server()
        || state.local_chat_ready(&app);
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let ignored = vault.topic_suggestion_ignores().unwrap_or_default();
    if let Some(topic) = vault
        .topic_suggestions(&ignored)
        .map_err(vault_err)?
        .into_iter()
        .next()
    {
        let revision = topic.notes.join("\n");
        let stable_token = suggestion_token(vault.root(), "topic", &topic.tag, &revision);
        if scope.accepts(KnowledgeNextActionKind::TopicOpportunity) {
            return Ok(Some(KnowledgeNextActionDto {
                suggestion_token: stable_token.clone(),
                kind: KnowledgeNextActionKind::TopicOpportunity,
                count: topic.note_count,
                action: if llm_ready {
                    KnowledgeNextActionCommand::CreateTopic
                } else {
                    KnowledgeNextActionCommand::ResolveAiReadiness
                },
                blocker: (!llm_ready).then_some(KnowledgeActionBlocker::LocalModelUnavailable),
                target: (!llm_ready).then(|| "settings://local_models/models".to_owned()),
                maps: Vec::new(),
                map_review: None,
                topic: Some(TopicSuggestionDto {
                    suggestion_token: stable_token,
                    draft_items: vault
                        .ready_topic_draft_for(&topic.tag)
                        .map(|draft| u32::try_from(draft.claims.len()).unwrap_or(u32::MAX)),
                    tag: topic.tag,
                    notes: topic.notes,
                    note_count: topic.note_count,
                }),
                tags: Vec::new(),
            }));
        }
    }
    let topic_pages = vault.topic_pages().map_err(vault_err)?;
    let maps = vault.knowledge_maps().map_err(vault_err)?;
    if !topic_pages.is_empty()
        && maps.is_empty()
        && scope.accepts(KnowledgeNextActionKind::KnowledgeMapMissing)
    {
        return Ok(Some(KnowledgeNextActionDto {
            suggestion_token: suggestion_token(
                vault.root(),
                "map",
                "missing",
                &topic_pages.join("\n"),
            ),
            kind: KnowledgeNextActionKind::KnowledgeMapMissing,
            count: u32::try_from(topic_pages.len()).unwrap_or(u32::MAX),
            action: KnowledgeNextActionCommand::CreateKnowledgeMap,
            blocker: None,
            target: None,
            maps: Vec::new(),
            map_review: None,
            topic: None,
            tags: Vec::new(),
        }));
    }
    if maps.len() > 1 && scope.accepts(KnowledgeNextActionKind::KnowledgeMapConflict) {
        return Ok(Some(KnowledgeNextActionDto {
            suggestion_token: suggestion_token(vault.root(), "map", "conflict", &maps.join("\n")),
            kind: KnowledgeNextActionKind::KnowledgeMapConflict,
            count: u32::try_from(maps.len()).unwrap_or(u32::MAX),
            action: KnowledgeNextActionCommand::ResolveKnowledgeMapConflict,
            blocker: None,
            target: None,
            maps: maps.into_iter().map(|path| NoteRefDto { path }).collect(),
            map_review: None,
            topic: None,
            tags: Vec::new(),
        }));
    }
    if let Some(path) = maps.first() {
        if vault.topic_ownership(path) == luna_vault::TopicOwnership::UserOwned {
            let doc = vault.read(path).map_err(vault_err)?;
            // 差异与「有没有差异」都在归一化后的基准上算：拿 `body_of` 原样去比，
            // 首行那个分隔空行会让每一页用户接管的地图恒显示一条假差异。
            let current = luna_vault::body_of(&doc.content).trim_matches('\n');
            let expected_body = vault
                .knowledge_map_body(path, is_zh(&app))
                .map_err(vault_err)?;
            let expected = expected_body.trim_matches('\n');
            if current != expected {
                let current_hash = luna_vault::knowledge_body_hash(current);
                let expected_hash = luna_vault::knowledge_body_hash(expected);
                if scope.accepts(KnowledgeNextActionKind::KnowledgeMapReview) {
                    return Ok(Some(KnowledgeNextActionDto {
                        suggestion_token: suggestion_token(
                            vault.root(),
                            "map",
                            path,
                            &format!("{current_hash}|{expected_hash}"),
                        ),
                        kind: KnowledgeNextActionKind::KnowledgeMapReview,
                        count: u32::try_from(topic_pages.len()).unwrap_or(u32::MAX),
                        action: KnowledgeNextActionCommand::ReviewKnowledgeMap,
                        blocker: None,
                        target: None,
                        maps: Vec::new(),
                        map_review: Some(KnowledgeMapReviewDto {
                            path: path.clone(),
                            current_hash,
                            diff: luna_sync_core::diff::diff_lines(current, expected).into(),
                        }),
                        topic: None,
                        tags: Vec::new(),
                    }));
                }
            }
        }
    }
    // `is_backlog` 同时覆盖 MissingTags/待整理/待归入，故这里严格只取完整性发现。
    let findings = vault
        .checkup_t0(
            settings.get_bool("vault.graph_ai_tags").unwrap_or(true),
            super::checkup::now_ms(),
        )
        .map_err(vault_err)?;
    let integrity = findings
        .iter()
        .filter(|finding| !finding.kind.is_backlog())
        .count();
    if integrity > 0 {
        let revision = findings
            .iter()
            .filter(|finding| !finding.kind.is_backlog())
            .map(|finding| finding.id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if scope.accepts(KnowledgeNextActionKind::CheckupFinding) {
            return Ok(Some(KnowledgeNextActionDto {
                suggestion_token: suggestion_token(vault.root(), "checkup", "integrity", &revision),
                kind: KnowledgeNextActionKind::CheckupFinding,
                count: u32::try_from(integrity).unwrap_or(u32::MAX),
                action: KnowledgeNextActionCommand::OpenCheckup,
                blocker: None,
                target: None,
                maps: Vec::new(),
                map_review: None,
                topic: None,
                tags: Vec::new(),
            }));
        }
    }
    // 标签语言分裂（PRD-06-035d ②）：排在「缺标签」之前——缺标签只是这一篇没参与进来，
    // 而语言分裂让每一个标签的覆盖面减半，主题页判据整体失效。
    //
    // **两个门槛都要满足才提示，拿不准就不说**（同 PRD-16-039「不为凑数硬推操作」）：
    // ① 另一种语言的笔记确实存在；② 本语言的笔记也确实存在。只有 ② 为零时那是「整个库
    // 都是另一种语言」——很可能是用户刚换了界面语言而库本来就是那种语言写的，此时催他把
    // 全库标签重生成一遍是自作主张；只有 ① 为零时则根本没有分裂。
    let tag_language = vault
        .graph_tag_language_scan(super::knowledge::is_zh(&app))
        .map_err(vault_err)?;
    if tag_language.mismatched > 0 && tag_language.matched > 0 {
        let count = u32::try_from(tag_language.mismatched).unwrap_or(u32::MAX);
        if scope.accepts(KnowledgeNextActionKind::TagLanguageMismatch) {
            return Ok(Some(KnowledgeNextActionDto {
                suggestion_token: suggestion_token(
                    vault.root(),
                    "tags",
                    "language_mismatch",
                    &count.to_string(),
                ),
                kind: KnowledgeNextActionKind::TagLanguageMismatch,
                count,
                action: if llm_ready {
                    KnowledgeNextActionCommand::RescanTagsForce
                } else {
                    KnowledgeNextActionCommand::ResolveAiReadiness
                },
                blocker: (!llm_ready).then_some(KnowledgeActionBlocker::LocalModelUnavailable),
                target: (!llm_ready).then(|| "settings://local_models/models".to_owned()),
                maps: Vec::new(),
                map_review: None,
                topic: None,
                tags: tag_language.sample_tags.clone(),
            }));
        }
    }
    if let Some(untagged) = untagged_finding(&findings) {
        let revision = untagged.id.as_str();
        if scope.accepts(KnowledgeNextActionKind::MissingTags) {
            return Ok(Some(KnowledgeNextActionDto {
                suggestion_token: suggestion_token(vault.root(), "tags", "untagged", revision),
                kind: KnowledgeNextActionKind::MissingTags,
                count: untagged.count,
                action: if llm_ready {
                    KnowledgeNextActionCommand::RescanTags
                } else {
                    KnowledgeNextActionCommand::ResolveAiReadiness
                },
                blocker: (!llm_ready).then_some(KnowledgeActionBlocker::LocalModelUnavailable),
                target: (!llm_ready).then(|| "settings://local_models/models".to_owned()),
                maps: Vec::new(),
                map_review: None,
                topic: None,
                tags: Vec::new(),
            }));
        }
    }
    let distill = vault.distill_scan(None).map_err(vault_err)?;
    if !distill.pending.is_empty() {
        let count = u32::try_from(distill.pending.len()).unwrap_or(u32::MAX);
        let revision = distill.pending.join("\n");
        if scope.accepts(KnowledgeNextActionKind::PendingDistill) {
            return Ok(Some(KnowledgeNextActionDto {
                suggestion_token: suggestion_token(vault.root(), "distill", "pending", &revision),
                kind: KnowledgeNextActionKind::PendingDistill,
                count,
                action: if llm_ready {
                    KnowledgeNextActionCommand::DistillBatch
                } else {
                    KnowledgeNextActionCommand::ResolveAiReadiness
                },
                blocker: (!llm_ready).then_some(KnowledgeActionBlocker::LocalModelUnavailable),
                target: (!llm_ready).then(|| "settings://local_models/models".to_owned()),
                maps: Vec::new(),
                map_review: None,
                topic: None,
                tags: Vec::new(),
            }));
        }
    }
    Ok(None)
}

/// 首个新主题页之后自动创建；既有库/用户主动删除则只由推荐卡显式调用本命令。
pub(crate) fn ensure_knowledge_map_after_topic(
    app: &tauri::AppHandle,
) -> Result<Option<String>, String> {
    create_or_refresh_knowledge_map(app, false, true)
}

fn create_or_refresh_knowledge_map(
    app: &tauri::AppHandle,
    explicit: bool,
    allow_create: bool,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let zh = is_zh(app);
    let (path, body) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let topic_pages = vault.topic_pages().map_err(vault_err)?;
        let maps = vault.knowledge_maps().map_err(vault_err)?;
        if topic_pages.is_empty() && maps.is_empty() {
            return Err(coded_error(
                app,
                "KNOWLEDGE_MAP_NO_TOPICS",
                "knowledge-map-no-topics",
            ));
        }
        if maps.len() > 1 {
            return Err(coded_error(
                app,
                "KNOWLEDGE_MAP_CONFLICT",
                "knowledge-map-conflict",
            ));
        }
        let rel = if let Some(path) = maps.first() {
            match vault.topic_ownership(path) {
                luna_vault::TopicOwnership::AiOwned => path.clone(),
                luna_vault::TopicOwnership::UserOwned => return Ok(None),
                luna_vault::TopicOwnership::NotTopic => {
                    return Err(coded_error(
                        app,
                        "KNOWLEDGE_MAP_INVALID",
                        "knowledge-map-invalid",
                    ))
                }
            }
        } else {
            if !allow_create || (!explicit && topic_pages.len() != 1) {
                // 自动出生只允许发生在本次刚创建首个主题页的边界；既有多主题页库必须由用户确认。
                return Ok(None);
            }
            // 新地图落知识产物目录（ADR-096 ②）；既有地图在哪就在哪维护。
            let dir = super::knowledge_product_dir(&state.settings_snapshot());
            let stem = if zh { "知识地图" } else { "Knowledge Map" };
            super::distill::unique_note_path_in(vault, &dir, stem)
        };
        let body = vault.knowledge_map_body(&rel, zh).map_err(vault_err)?;
        // 「已经是这份内容」必须与写盘口径一致，否则它恒不成立：每次去抖刷新都会重写
        // 同样的字节，而写入事件又会排下一次刷新——地图自己把自己转成死循环。
        if vault.knowledge_body_fingerprint(&rel) == Some(luna_vault::knowledge_body_hash(&body)) {
            return Ok(None);
        }
        (rel, body)
    };
    let intent = prepare_knowledge_write(app, &path, &body, "knowledge_map", false)?;
    let write_result = (|| -> Result<(), String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.write_knowledge_map(&path, &body).map_err(vault_err)
    })();
    if let Err(error) = write_result {
        abandon_knowledge_write(app, intent);
        return Err(error);
    }
    emit_vault_changed(app, &path, false);
    commit_knowledge_write(app, intent)?;
    Ok(Some(path))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_map_create(app: tauri::AppHandle) -> Result<Option<String>, String> {
    create_or_refresh_knowledge_map(&app, true, true)
}

/// 知识地图此刻在哪（[PRD-16-023] 的「入口在知识库 → 概览与图谱侧栏」中的前一半）。
///
/// **只回答位置，不承担建议**：不存在（还没有主题页）与存在多份（外部操作造出的
/// 冲突态）都返回 `None`——这两种情形各自有自己的闭环，都在图谱维护区那一条排好序的
/// 建议里（「建立知识地图」/「选择保留的知识地图」）。概览这个入口若也去处理它们，
/// 应用就有了第二个维护推荐面，两处排序迟早不一致。
///
/// **身份由 frontmatter 判、路径按真实 path 回**：地图是普通 Markdown，用户重命名或
/// 移动之后仍是同一页（PRD-16-023），故前端不得按固定文件名去猜它在哪。
/// 「知识库 → 概览」上半部的**现状**（PRD-07-024a ①）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct KnowledgeOverviewDto {
    /// 知识面的篇数（笔记 + 速记）。
    pub knowledge_notes: u32,
    /// 资料区的篇数（录音 + 转写 + 对话）。
    pub materials: u32,
    /// 还没有任何关系（既无链接也无标签）的知识笔记篇数——「图谱稀疏」的用户语言版。
    /// 与体检的孤儿页**同源同数**（判据在 `luna_vault::checkup::classify_connectivity`）。
    pub unlinked_notes: u32,
    /// 近期活跃标签，按窗口内命中篇数降序。
    pub active_tags: Vec<ActiveTagDto>,
    /// 一条值得回看的旧笔记；`None` = 没有够旧的（新库或刚全面整理过）。
    pub revisit: Option<NoteRefLabelDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct NoteRefLabelDto {
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ActiveTagDto {
    pub tag: String,
    pub count: u32,
}

/// 「最近在写什么」的时间窗：两周。
///
/// 取两周而不是「本周」：一周的窗口在真实使用里经常整周为空（出差、休假、手上正忙别的），
/// 而一个**经常空着的现状块**会被用户学会忽略——那比不做更糟。
const ACTIVE_TAG_WINDOW_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// 活跃标签最多列几个。多于这个数就不再是「最近在写什么」，而是另一棵标签树了。
const ACTIVE_TAG_LIMIT: u32 = 5;

/// 「值得回看」的陈化门槛：30 天没动过。
///
/// **必须真的旧**：库里最久没动的那一篇，在一个刚建的库里可能是昨天写的——把它端出来
/// 说「值得回看」是无中生有。取 30 天而不是更短：两周那个窗口是「最近在写什么」用的，
/// 两者若挨着，同一篇笔记会同时出现在「最近」和「很久没看」两行里。
const REVISIT_STALE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 概览的现状事实（PRD-07-024a ①）。
///
/// **「知识 vs 资料」的折叠在这里做，判据取 [`NoteType::is_source_material`]**——索引层
/// 全部知识产物的路径（主题页 + 知识地图，ADR-096）。
///
/// 前端拿它做**路由闸口**：编辑桌、独立窗口与「在全部笔记中显示」据此把产物让路到
/// 知识库空间。它是 Rust 已判定事实的清单（与快速切换器条目里的 `is_source_material` 同一先例），
/// 前端持有的只是缓存、不是第二份判定（ADR-003）；随 `vault://changed` 重取。
#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_product_paths(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.knowledge_product_paths().map_err(vault_err)
}

/// 只回原始分组，刻意不在 SQL 里写第二遍 `type IN (…)`：那条线已经被四五个面共用，
/// 再画一次必然漏掉将来新增的类型。认不出的 type 值一律计入知识面（与 `luna.type` 缺省
/// 即 `note` 的互操作口径一致，不把陌生类型丢掉）。
///
/// **本命令只回「现状」，不回任何建议**：下一步归 `knowledge_next_action` 与
/// `knowledge_pulse`，两处排序不得在这里出现第三份。
#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_overview(
    state: tauri::State<'_, AppState>,
    now_ms: f64,
) -> Result<KnowledgeOverviewDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;

    let mut knowledge_notes = 0u32;
    let mut materials = 0u32;
    for (key, count) in vault.note_counts_by_type().map_err(vault_err)? {
        let is_material = luna_vault::NoteType::from_key(&key)
            .is_some_and(luna_vault::NoteType::is_source_material);
        if is_material {
            materials = materials.saturating_add(count);
        } else {
            knowledge_notes = knowledge_notes.saturating_add(count);
        }
    }

    // 时间基准由调用方给：Rust 侧没有「用户此刻的本地时间」这个事实，而窗口的两端必须
    // 与用户看到的日期同一口径（TDD-11 §1.4b 绝对时间随界面 locale 的同一取向）。
    let since_ms = (now_ms as i64).saturating_sub(ACTIVE_TAG_WINDOW_MS);
    let active_tags = vault
        .active_tags(since_ms, ACTIVE_TAG_LIMIT)
        .map_err(vault_err)?
        .into_iter()
        .map(|(tag, count)| ActiveTagDto { tag, count })
        .collect();

    let revisit = vault
        .revisit_candidate((now_ms as i64).saturating_sub(REVISIT_STALE_MS))
        .map_err(vault_err)?
        .map(|(path, title)| NoteRefLabelDto { path, title });

    Ok(KnowledgeOverviewDto {
        knowledge_notes,
        materials,
        unlinked_notes: vault.unlinked_knowledge_notes().map_err(vault_err)?,
        active_tags,
        revisit,
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_map_entry(
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let mut maps = vault.knowledge_maps().map_err(vault_err)?;
    Ok((maps.len() == 1).then(|| maps.remove(0)))
}

/// 用户确认地图差异后应用。预览后的正文或所有权有变化即拒绝，不能拿旧差异覆盖新编辑。
#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_map_apply_review(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    current_hash: String,
    suggestion_token: String,
) -> Result<(), String> {
    let expected = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if vault.topic_ownership(&path) != luna_vault::TopicOwnership::UserOwned {
            return Err(coded_error(
                &app,
                "KNOWLEDGE_MAP_REVIEW_STALE",
                "knowledge-map-review-stale-ownership",
            ));
        }
        if vault.knowledge_body_fingerprint(&path).as_deref() != Some(current_hash.as_str()) {
            return Err(coded_error(
                &app,
                "KNOWLEDGE_MAP_REVIEW_STALE",
                "knowledge-map-review-stale-body",
            ));
        }
        vault
            .knowledge_map_body(&path, is_zh(&app))
            .map_err(vault_err)?
    };
    let after_hash = luna_vault::knowledge_body_hash(&expected);
    let intent = prepare_knowledge_write(&app, &path, &expected, "knowledge_map", false)?
        .with_rollback(&suggestion_token, &current_hash, &after_hash);
    let write_result = (|| -> Result<(), String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if vault.knowledge_body_fingerprint(&path).as_deref() != Some(current_hash.as_str()) {
            return Err(coded_error(
                &app,
                "KNOWLEDGE_MAP_REVIEW_STALE",
                "knowledge-map-review-stale-body",
            ));
        }
        vault
            .write_knowledge_map(&path, &expected)
            .map_err(vault_err)
    })();
    if let Err(error) = write_result {
        abandon_knowledge_write(&app, intent);
        return Err(error);
    }
    emit_vault_changed(&app, &path, false);
    commit_knowledge_write(&app, intent)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn knowledge_map_resolve_conflict(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    keep_path: String,
) -> Result<(), String> {
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let maps = vault.knowledge_maps().map_err(vault_err)?;
        if maps.len() < 2 || !maps.contains(&keep_path) {
            return Err(coded_error(
                &app,
                "KNOWLEDGE_MAP_CONFLICT_STALE",
                "knowledge-map-conflict-stale",
            ));
        }
        for path in maps.into_iter().filter(|path| path != &keep_path) {
            let doc = vault.read(&path).map_err(vault_err)?;
            let mut meta = doc.meta;
            meta.knowledge_map = false;
            meta.synth_body = None;
            vault
                .write(
                    &path,
                    &luna_vault::update_luna(&doc.content, &meta).map_err(vault_err)?,
                )
                .map_err(vault_err)?;
            emit_vault_changed(&app, &path, false);
        }
    }
    let _ = create_or_refresh_knowledge_map(&app, true, false)?;
    Ok(())
}

static MAP_REFRESH_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 任意 Vault 变更后的单例去抖刷新。缺地图时绝不创建；因此既有库与用户删除都只会回到建议。
pub(crate) fn schedule_knowledge_map_refresh(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    let revision = MAP_REFRESH_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    let generation = app.state::<AppState>().vault_generation();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        if MAP_REFRESH_REVISION.load(Ordering::SeqCst) != revision
            || !app.state::<AppState>().still_current_vault(generation)
        {
            return;
        }
        if let Err(error) = create_or_refresh_knowledge_map(&app, false, false) {
            if !error.starts_with("KNOWLEDGE_MAP_NO_TOPICS") {
                eprintln!(
                    "[knowledge-map] 刷新失败：{}",
                    error.split(':').next().unwrap_or("UNKNOWN")
                );
            }
        }
    });
}

pub(crate) fn suggestion_token(
    vault_root: &std::path::Path,
    kind: &str,
    subject: &str,
    revision: &str,
) -> String {
    let vault_identity =
        std::fs::canonicalize(vault_root).unwrap_or_else(|_| vault_root.to_owned());
    luna_sync_core::hashing::ContentHash::of(
        format!(
            "knowledge-suggestion-v1|{}|{kind}|{subject}|{revision}",
            vault_identity.display()
        )
        .as_bytes(),
    )
    .to_string()
}

/// Frontend may report only actual visibility. Rust owns dedupe and immediately transforms this
/// stable Phase-10 token before it can touch disk.
#[tauri::command]
#[specta::specta]
pub(crate) fn knowledge_suggestion_shown(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    suggestion_token: String,
    surface: SuggestionSurfaceDto,
) -> Result<bool, String> {
    let root = state.vault_root().ok_or_else(|| state.vault_not_open())?;
    Ok(record_suggestion_metric(
        &app,
        &root.display().to_string(),
        &suggestion_token,
        luna_metrics::SuggestionEvent::Shown,
        Some(surface.into()),
    ))
}

/// Stable Phase-10 suggestion identity is HMAC-transformed by the metrics store before disk. This
/// helper intentionally accepts no paths/titles at the metrics boundary.
pub(crate) fn record_suggestion_metric(
    app: &tauri::AppHandle,
    vault_root: &str,
    suggestion_token: &str,
    event: luna_metrics::SuggestionEvent,
    surface: Option<luna_metrics::SuggestionSurface>,
) -> bool {
    app.try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.suggestion(
                    vault_root,
                    suggestion_token,
                    event,
                    surface,
                    chrono::Utc::now().timestamp_millis(),
                )
            })
        })
        .is_some()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SuggestionSurfaceDto {
    /// 启动概览**弹窗**（每次运行只呈现一次）。
    StartupOverview,
    /// 「知识库 → 概览」这一格的常驻下一步。与上一格复用同一个组件，但**不是同一格**
    /// ——合并上报会让这条度量答不出「用户到底在哪儿看见的」。
    KnowledgeOverview,
    Graph,
    Note,
}

impl From<SuggestionSurfaceDto> for luna_metrics::SuggestionSurface {
    fn from(value: SuggestionSurfaceDto) -> Self {
        match value {
            SuggestionSurfaceDto::StartupOverview => Self::StartupOverview,
            SuggestionSurfaceDto::KnowledgeOverview => Self::KnowledgeOverview,
            SuggestionSurfaceDto::Graph => Self::Graph,
            SuggestionSurfaceDto::Note => Self::Note,
        }
    }
}

/// `@` 提及的候选（PRD-16-009）。判定与排序恒在 Rust，前端只渲染并原样回传选中项。
#[tauri::command]
#[specta::specta]
pub(crate) async fn mention_candidates(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<MentionCandidateDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .mention_candidates(&query, MENTION_CANDIDATES_PER_KIND)
        .map_err(vault_err)?
        .into_iter()
        .map(|scope| MentionCandidateDto {
            kind: scope.kind.as_str().to_owned(),
            value: scope.value,
            label: scope.label,
        })
        .collect())
}

/// 每类 `@` 候选一次给几条：三类各 5 条 = 一屏能扫完，超出的靠继续打字收窄。
const MENTION_CANDIDATES_PER_KIND: usize = 5;

/// 「相关」面板数据（PRD-16-013）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn related_notes(
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<RelatedNotesDto, String> {
    // 相关面板的主逻辑是同步读矩阵；异步命令入口先核实运行时身份，fresh launch 不丢语义路。
    let _ = super::embed::prepare_embedding_identity(&state).await;
    // 静默升级（PRD-16-029）：语义近邻**排在标签共现之前**，两者并集去重。
    // 不是"替换"而是"补充 + 提前"：标签共现是用户亲手打的标，它表达的关系比模型算出来的
    // 更权威；语义近邻补的是"他还没来得及打标"的那些。缺模型时这一路为空 → 界面零变化。
    let semantic = super::embed::semantic_neighbours(&state, &note_rel, RELATED_SEMANTIC_LIMIT);
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let found = vault.related_notes(&note_rel).map_err(vault_err)?;
    let can_link = !vault
        .read(&note_rel)
        .map(|doc| doc.meta.note_type.is_source_material())
        .unwrap_or(true);
    let mut related: Vec<NoteRefDto> = Vec::new();
    for path in semantic
        .into_iter()
        .chain(found.related.iter().map(|entry| entry.path.clone()))
    {
        if !related.iter().any(|r| r.path == path) {
            related.push(NoteRefDto { path });
        }
    }
    Ok(RelatedNotesDto {
        mentions: found
            .mentions
            .iter()
            .map(UnlinkedMentionDto::from)
            .collect(),
        related,
        can_link,
    })
}

/// 「相关」页签里语义近邻的条数上限（标签共现补在其后，总条数由前端呈现层收）。
const RELATED_SEMANTIC_LIMIT: usize = 5;

/// 把一处未链接提及改写为正式链接（PRD-16-013）。资料笔记会被 Vault 层拒绝。
#[tauri::command]
#[specta::specta]
pub(crate) async fn link_mention(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    mention: UnlinkedMentionDto,
) -> Result<(), String> {
    let content = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .link_mention(&note_rel, &luna_vault::UnlinkedMention::from(&mention))
            .map_err(vault_err)?;
        vault.read(&note_rel).map_err(vault_err)?.content
    };
    // 这篇八成正开在编辑器里（相关面板就是给活动笔记看的）：不发 `note://written`，
    // 编辑器缓冲不知道链接已落盘，用户下一次保存就把它静默覆盖掉——AI 标签写回的
    // 既有纪律（graph.rs），后端写开着的笔记必须双事件。
    let _ = (NoteWritten {
        path: note_rel.clone(),
        content,
        origin: "main".to_owned(),
    })
    .emit(&app);
    emit_vault_changed(&app, &note_rel, false);
    Ok(())
}

/// 按复合锚定位引用目标（PRD-16-012）：跳转前解析，锚不上如实说明。
#[tauri::command]
#[specta::specta]
pub(crate) async fn resolve_citation(
    state: tauri::State<'_, AppState>,
    note_rel: String,
    anchor: CitationAnchorDto,
) -> Result<CitationTargetDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let hit = vault
        .resolve_citation(&note_rel, &luna_vault::Anchor::from(&anchor))
        .map_err(vault_err)?;
    Ok(CitationTargetDto {
        line: hit.line(),
        exact: hit.is_exact(),
    })
}

/// 一次完成来源检查、复合锚定位与主工作区导航（TDD-06 §3.7.2）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn open_citation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    source: RetrievalSourceDto,
) -> Result<CitationOpenOutcome, String> {
    let anchor = luna_vault::Anchor::from(&source.anchor);
    let (vault_root, outcome) = open_source_at_anchor(&app, &state, &source.path, &anchor, false)?;
    record_citation_metric(&app, &vault_root, outcome);
    Ok(outcome)
}

/// 按复合锚解析来源并导航到主工作区（对话出处与主题计划出处共用这一条路）。
/// 返回库根（供调用方按各自口径记度量）与封闭定位终态；来源不在时不导航。
///
/// `version_first` = 先校验版本（TDD-06 §3.9.1）：来源正文仍等于锚点记录的指纹时，
/// 记录的行号就是精确位置，直接用；变了才退到引文 → 标题 → 行提示的复合解析。
/// 这对短句尤其重要——过短的引文按设计不做模糊匹配，否则只能落到小节标题。
pub(crate) fn open_source_at_anchor(
    app: &tauri::AppHandle,
    state: &AppState,
    path: &str,
    anchor: &luna_vault::Anchor,
    version_first: bool,
) -> Result<(String, CitationOpenOutcome), String> {
    let (vault_root, hit) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let root = vault.root().display().to_string();
        match resolve_versioned_anchor(vault, path, anchor, version_first) {
            Ok(hit) => (root, hit),
            Err(luna_vault::VaultError::NotFound(_)) => {
                return Ok((root, CitationOpenOutcome::SourceMissing));
            }
            Err(error) => return Err(vault_err(error)),
        }
    };
    let outcome = match hit {
        luna_vault::AnchorHit::Exact(_) => CitationOpenOutcome::Exact,
        luna_vault::AnchorHit::Moved(_) => CitationOpenOutcome::Moved,
        luna_vault::AnchorHit::Heading(_) => CitationOpenOutcome::HeadingOnly,
        luna_vault::AnchorHit::Lost => CitationOpenOutcome::Lost,
    };
    let suffix = hit
        .line()
        .map_or_else(String::new, |line| format!("#L{line}"));
    super::open_workspace_route(app.clone(), format!("notes://{path}{suffix}"))?;
    Ok((vault_root, outcome))
}

pub(crate) fn resolve_versioned_anchor(
    vault: &luna_vault::Vault,
    path: &str,
    anchor: &luna_vault::Anchor,
    version_first: bool,
) -> luna_vault::Result<luna_vault::AnchorHit> {
    if version_first
        && anchor.line_hint > 0
        && vault.body_fingerprint(path).as_deref() == Some(anchor.source_hash.as_str())
    {
        return Ok(luna_vault::AnchorHit::Exact(anchor.line_hint));
    }
    vault.resolve_citation(path, anchor)
}

/// 证据定位器 → 复合锚（已写入声明的出处与封存计划的出处共用这一份映射）。
pub(crate) fn locator_anchor(
    locator: &luna_vault::EvidenceLocator,
    source_hash: &str,
) -> luna_vault::Anchor {
    luna_vault::Anchor {
        heading_path: locator.heading_path.clone(),
        quote: locator.quote.clone(),
        prefix: locator.context.clone(),
        suffix: String::new(),
        line_hint: locator.line_hint.unwrap_or(0),
        source_hash: source_hash.to_owned(),
    }
}

/// The attempt is created only after the actual citation command starts. IPC failures intentionally
/// return early above and never masquerade as a broken anchor in the product denominator.
fn record_citation_metric(app: &tauri::AppHandle, vault_root: &str, outcome: CitationOpenOutcome) {
    use luna_metrics::CitationOutcome as M;
    let outcome = match outcome {
        CitationOpenOutcome::Exact => M::Exact,
        CitationOpenOutcome::Moved => M::Moved,
        CitationOpenOutcome::HeadingOnly => M::HeadingOnly,
        CitationOpenOutcome::Lost => M::Lost,
        CitationOpenOutcome::SourceMissing => M::SourceMissing,
    };
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.citation(
                    vault_root,
                    &uuid::Uuid::now_v7().to_string(),
                    outcome,
                    chrono::Utc::now().timestamp_millis(),
                )
            })
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_detail_distinguishes_current_stale_legacy_and_unknown_schema() {
        let dir = tempfile::tempdir().unwrap();
        let vault = luna_vault::Vault::open(dir.path()).unwrap();
        let source = "Notes/会议.md";
        let topic = "Knowledge/项目.md";
        vault.write(source, "# 会议\n\n原型已完成。\n").unwrap();
        let source_hash = vault.body_fingerprint(source).unwrap();
        let sources = [luna_vault::ClaimSource {
            path: source,
            title: "会议",
            source_hash: &source_hash,
            locator: luna_vault::EvidenceLocator {
                heading_path: vec!["会议".into()],
                quote: "原型已完成".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claims = [
            luna_vault::NewClaim {
                id: uuid::Uuid::now_v7(),
                text: "原型已完成",
                kind: luna_vault::ClaimKind::Synthesis,
                sources: &sources,
            },
            luna_vault::NewClaim {
                id: uuid::Uuid::now_v7(),
                text: "原型可以进入评审",
                kind: luna_vault::ClaimKind::Synthesis,
                sources: &sources,
            },
        ];
        let content = vault
            .scoped_topic_content(
                topic,
                Some("项目"),
                luna_vault::TopicScope::from_tag("项目"),
                &claims,
            )
            .unwrap();
        vault.publish_scoped_topic(topic, &content).unwrap();

        let current = topic_evidence_detail_of(&vault, topic, 0, 1, None).unwrap();
        assert_eq!(current.maintenance, TopicMaintenanceStateDto::Ready);
        assert_eq!(current.total_citations, Some(2));
        assert_eq!(current.next_offset, Some(1));
        let citations = current.citations.unwrap();
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source_path.as_deref(), Some(source));
        assert_eq!(citations[0].state, TopicEvidenceStateDto::Current);
        let second =
            topic_evidence_detail_of(&vault, topic, 1, 1, Some(&current.revision)).unwrap();
        assert_eq!(second.citations.unwrap().len(), 1);
        assert_eq!(second.next_offset, None);

        vault.write(source, "# 会议\n\n原型已取消。\n").unwrap();
        let changed = topic_evidence_detail_of(&vault, topic, 0, 1, None).unwrap();
        assert_eq!(changed.maintenance, TopicMaintenanceStateDto::Ready);
        assert_eq!(
            changed.citations.unwrap()[0].state,
            TopicEvidenceStateDto::SourceChanged
        );
        vault.write(topic, &format!("{content}\n")).unwrap();
        assert_eq!(
            topic_evidence_detail_of(&vault, topic, 1, 1, Some(&current.revision)).unwrap_err(),
            "TOPIC_EVIDENCE_STALE"
        );

        let legacy = "Knowledge/旧主题.md";
        vault
            .write_topic(legacy, "# 旧主题\n", Some("旧主题"))
            .unwrap();
        let old = topic_evidence_detail_of(&vault, legacy, 0, 10, None).unwrap();
        assert_eq!(old.maintenance, TopicMaintenanceStateDto::NeedsUpgrade);
        assert_eq!(old.total_citations, None);
        assert!(old.citations.is_none());

        let future = "Knowledge/未来主题.md";
        vault.write(future, "---\nluna:\n  schema: 1\n  topic: true\n  knowledge:\n    schema: 99\n    scope:\n      title: 未来主题\n      description: ''\n      includes: [未来主题]\n      excludes: []\n    claims: {}\n    citations: {}\n---\n# 未来主题\n").unwrap();
        let unknown = topic_evidence_detail_of(&vault, future, 0, 10, None).unwrap();
        assert_eq!(unknown.maintenance, TopicMaintenanceStateDto::UnknownSchema);
        assert!(unknown.citations.is_none());
        assert_eq!(
            topic_evidence_detail_of(&vault, future, 0, 10, Some("stale")).unwrap_err(),
            "TOPIC_EVIDENCE_STALE"
        );
    }

    /// 每一档推荐**恰好归一格**（`PRD-16-039` 2026-08-30 修订、PRD-16-027 收口）。
    ///
    /// 互补是这条分工的全部内容：某一档两格都出 = 同一条建议在同一屏说两遍（真机第四轮
    /// 坐实过的矛盾形态——概览上一行「有 7 篇都在讲…」、下一行「暂时不用整理」）；
    /// 某一档两格都不出 = 这条建议从产品里凭空消失，且没有任何报错会告诉你。
    #[test]
    fn scope_split_is_exhaustive_and_complementary() {
        use KnowledgeNextActionKind as K;
        for kind in [
            K::TopicOpportunity,
            K::KnowledgeMapMissing,
            K::KnowledgeMapConflict,
            K::KnowledgeMapReview,
            K::CheckupFinding,
            K::TagLanguageMismatch,
            K::MissingTags,
            K::PendingDistill,
        ] {
            let graph = KnowledgeActionScope::Graph.accepts(kind);
            let overview = KnowledgeActionScope::Overview.accepts(kind);
            assert_ne!(graph, overview, "{kind:?} 必须恰好归一格");
            // 图谱只留「做完之后图谱当场变密」的两档——标签就是图谱的边。
            assert_eq!(
                graph,
                matches!(kind, K::MissingTags | K::TagLanguageMismatch),
                "{kind:?} 的归属漂了"
            );
        }
    }

    /// **「还没有标签」的篇数只有一个来源**（PRD-16-016 ②）。
    ///
    /// `Untagged` 是一条聚合发现：15 篇无标签笔记产出的是 **1 条** `count == 15` 的发现。
    /// 按发现条数去数会得到 1——主题页空态说「1 篇」、体检卡说「15 篇」，同一件事两个数。
    /// 用户据此判断"才 1 篇，不值得补"，于是主题页建议永远出不来。
    #[test]
    fn the_untagged_count_is_notes_not_findings() {
        let findings = vec![
            luna_vault::Finding {
                id: "broken_link:x".into(),
                kind: luna_vault::FindingKind::BrokenLink,
                paths: vec!["a.md".into()],
                line: None,
                subject: String::new(),
                reason: String::new(),
                count: 2,
            },
            luna_vault::Finding {
                id: "untagged:".into(),
                kind: luna_vault::FindingKind::Untagged,
                paths: vec!["b.md".into(), "c.md".into()],
                line: None,
                subject: String::new(),
                reason: String::new(),
                count: 15,
            },
        ];
        assert_eq!(
            untagged_finding(&findings).map_or(0, |finding| finding.count),
            15,
            "读到的必须是篇数；得到 1 说明又在数发现条数"
        );
        assert_eq!(untagged_finding(&[]).map_or(0, |finding| finding.count), 0);
    }

    /// **主题页目录与知识地图的摘要同源，逐字节相同**（ADR-088 ② / TDD-07 §4.1d）。
    ///
    /// 这条用例在**真实 Vault** 上跑，比"两处都调了同一个函数"的结构断言强一档：它把
    /// 目录那一行的 `summary` 与地图正文里那一行做字节比对。两份实现分叉的表现是同一页
    /// 在地图里一句话、在目录里另一句话，而两处都自称是这一页的摘要——界面上看不出错。
    #[test]
    fn the_topic_directory_and_the_knowledge_map_agree_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = luna_vault::Vault::open(dir.path()).expect("open vault");
        vault
            .write_topic(
                "Notes/交易计划.md",
                "# 交易计划\n\n- **背景**：围绕双均线的一轮复盘。\n\n后面还有别的段落。\n",
                Some("交易"),
            )
            .expect("write topic");

        let TopicDirectoryDto { rows, groups, .. } =
            topic_directory_rows(&vault, luna_vault::SourcePolicy::FORMAL_ONLY).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "交易计划");
        // 分组与地图同源（PRD-16-050）：出身标签「交易」的顶层段就是组名，地图里有同名二级标题。
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name.as_deref(), Some("交易"));
        assert_eq!(groups[0].paths, vec![rows[0].path.clone()]);
        assert!(!rows[0].summary.is_empty(), "摘要不该是空的");
        assert_eq!(
            rows[0].ownership,
            TopicOwnershipDto::AiOwned,
            "刚写的主题页归 Lunarca 维护"
        );

        let map = vault
            .knowledge_map_body("Notes/知识地图.md", true)
            .expect("map body");
        assert!(map.contains("## 交易\n"), "地图分组与目录不同源：\n{map}");
        assert!(
            map.contains(&format!(" — {}\n", rows[0].summary)),
            "目录与地图的摘要不同源：\n{map}\n目录：{}",
            rows[0].summary
        );
    }

    /// 目录即地图（PRD-16-061）：「相关主题」与「最近更新」同样与地图同源——目录给的页与
    /// 地图那几行列出的页一致。
    #[test]
    fn the_directory_lists_the_same_recent_and_related_topics_as_the_map() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = luna_vault::Vault::open(dir.path()).expect("open vault");
        vault
            .write("Notes/周会.md", "# 周会\n\n原型已完成。\n")
            .expect("source");
        for (path, tag) in [("Notes/项目甲.md", "项目甲"), ("Notes/项目乙.md", "项目乙")]
        {
            vault
                .write_topic(path, "# 题\n\n要点见 [周会](周会.md)。\n", Some(tag))
                .expect("write topic");
        }
        vault
            .write_topic(
                "Notes/无关.md",
                "# 无关\n\n与别处都不相干。\n",
                Some("杂项"),
            )
            .expect("write topic");

        let listing =
            topic_directory_rows(&vault, luna_vault::SourcePolicy::FORMAL_ONLY).expect("rows");
        let row = |path: &str| listing.rows.iter().find(|row| row.path == path).unwrap();
        assert_eq!(
            row("Notes/项目甲.md").related,
            vec!["Notes/项目乙.md".to_owned()]
        );
        assert_eq!(
            row("Notes/项目乙.md").related,
            vec!["Notes/项目甲.md".to_owned()]
        );
        assert!(row("Notes/无关.md").related.is_empty());
        // 旧式写入不留系统底版：两处都不列「最近更新」（有底版时的排序由 `recent_topics` 的
        // 单元测试钉住，地图与目录调的是它这一个函数）。
        assert!(listing.recent.is_empty());
        assert!(listing.rows.iter().all(|row| row.updated_ms.is_none()));

        let map = vault
            .knowledge_map_body("Notes/知识地图.md", true)
            .expect("map body");
        assert!(!map.contains("## 最近更新"), "{map}");
        assert!(
            map.contains("相关：[项目乙]"),
            "地图的相关主题与目录不同源：\n{map}"
        );
    }

    /// 目录只计算共享标签下界，不把它伪装成完整候选总数或有效计划条目数。
    #[test]
    fn directory_candidate_lower_bound_counts_uncited_knowledge_without_claiming_ready_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = luna_vault::Vault::open(dir.path()).expect("open vault");
        let tagged = |body: &str| {
            format!("---\ntags:\n  - 交易\nluna:\n  schema: 1\n  type: note\n---\n{body}\n")
        };
        vault
            .write("Notes/已引用.md", &tagged("已经写进页里的那一篇。"))
            .expect("write");
        vault
            .write("Notes/还没归.md", &tagged("还没被收进去的那一篇。"))
            .expect("write");
        // 记录态与资料都不算「知识笔记」（PRD-16-016 / ADR-086）。
        vault
            .write(
                "Notes/草稿.md",
                "---\ntags:\n  - 交易\nluna:\n  schema: 1\n  type: note\n  stage: draft\n---\n草。\n",
            )
            .expect("write");
        vault
            .write(
                "Recordings/录音.md",
                "---\ntags:\n  - 交易\nluna:\n  schema: 1\n  type: recording\n---\n逐字稿。\n",
            )
            .expect("write");
        vault
            .write_topic(
                "Notes/交易计划.md",
                &format!(
                    "# 交易计划\n\n## 进展\n\n- 已经收过了。（{}）\n",
                    luna_vault::markdown_link("Notes/交易计划.md", "Notes/已引用.md", "已引用")
                ),
                Some("交易"),
            )
            .expect("write topic");

        let rows = topic_directory_rows(&vault, luna_vault::SourcePolicy::FORMAL_ONLY)
            .expect("rows")
            .rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].candidate_source_count,
            KnowledgeCountDto::LowerBound { count: 1 },
            "只该剩「还没归」那一篇：已引用的不重复检查，草稿与资料不是知识笔记"
        );
        assert_eq!(rows[0].ready_item_count, KnowledgeCountDto::NotComputed);
    }

    /// **纯登记版绝不能碰 vault 锁**——这是它存在的全部理由。
    ///
    /// `AppState.vault` 是 `std::sync::Mutex`，**不可重入**：在已经持有它的临界区里再取
    /// 一次就是当场死锁，而死锁不会被任何单测发现（那条路要么没人走、要么整个测试进程挂住
    /// 到超时，看起来像"CI 偶尔卡住"）。所以这一条只能由结构断言看着。
    ///
    /// 同一条断言顺带钉住**锁序方向 `vault → processing.inner`**：本函数在 vault 之内取
    /// `processing.inner`；反方向若哪天出现，第一现场就是这里多了一次 `vault.lock()`。
    #[test]
    fn the_locked_variant_never_reaches_for_the_vault_lock() {
        let src = crate::ipc::source_without_tests(include_str!("knowledge.rs"));
        let body = src
            .split("pub(crate) fn prepare_knowledge_write_locked(")
            .nth(1)
            .expect("找不到纯登记版")
            .split("\n/// 文件写盘失败时撤销 Prepared")
            .next()
            .expect("函数边界变了");
        assert!(body.len() > 400, "剥掉注释前扫描面就已经为空");
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for reentrant in ["vault.lock()", "vault_root()", "vault_arc()"] {
            assert!(
                !code.contains(reentrant),
                "纯登记版又去取 vault 锁了（`std::sync::Mutex` 不可重入 = 死锁）：{reentrant}"
            );
        }
        // `created_new` 是**传进来的事实**，不是回读推断出来的。
        assert!(code.contains("created_new,"), "{code}");
        assert!(
            !code.contains(".read(note_rel)"),
            "又开始靠回读推断「是不是新建」了"
        );
    }

    #[test]
    fn suggestion_identity_matches_the_versioned_tdd_contract() {
        let vault = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(vault.path()).unwrap();
        let expected = luna_sync_core::hashing::ContentHash::of(
            format!(
                "knowledge-suggestion-v1|{}|topic|Planning|a.md\nb.md",
                canonical.display()
            )
            .as_bytes(),
        )
        .to_string();
        assert_eq!(
            suggestion_token(vault.path(), "topic", "Planning", "a.md\nb.md"),
            expected
        );
        assert_ne!(
            suggestion_token(vault.path(), "topic", "Planning", "a.md\nb.md"),
            suggestion_token(vault.path(), "topic", "Planning", "a.md\nc.md")
        );
    }

    fn source(index: u32, title: &str, path: &str) -> RetrievalSourceDto {
        RetrievalSourceDto {
            index,
            path: path.to_string(),
            title: title.to_string(),
            note_type: "note".to_string(),
            source_material: false,
            draft: false,
            snippet: format!("{title} 的摘录"),
            anchor: CitationAnchorDto {
                heading_path: vec![],
                quote: String::new(),
                prefix: String::new(),
                suffix: String::new(),
                line_hint: 1,
                source_hash: String::new(),
            },
        }
    }

    fn passage(title: &str, text: &str) -> luna_vault::Passage {
        luna_vault::Passage {
            path: format!("Notes/{title}.md"),
            title: title.to_string(),
            note_type: "note".to_string(),
            draft: false,
            text: text.to_string(),
            anchor: luna_vault::Anchor::default(),
            score: 1.0,
        }
    }

    #[test]
    fn grounding_prompt_puts_instruction_first_and_carries_full_passages() {
        let prompt =
            compose_grounding_prompt(&[passage("甲", "完整的摘录正文，不是短摘要。")], true);
        let instruction_at = prompt.find("写作要求").expect("指令在场");
        let material_at = prompt.find("[1] 甲").expect("材料在场");
        // 指令必须把 `classify_grounded_answer` 真会判的两条规则说出来，否则就是拿模型
        // 没被告知的规则去 fail-closed 降级它（真机第七轮的实测缺陷）。
        assert!(
            prompt.contains("每个段落和每个列表项都要标上它依据的摘录编号"),
            "逐段引用是判据，必须写进指令：{prompt}"
        );
        assert!(
            prompt.contains(MODEL_SUPPLEMENT_SENTINEL) && prompt.contains("单独占一行"),
            "sentinel 必须独占一行的要求要写进指令：{prompt}"
        );
        assert!(
            instruction_at < material_at,
            "指令必须排在材料之前——模型对开头的指令遵循最好"
        );
        // 送给模型的是完整摘录；hover 用的短摘要绝不能顶替它（否则只喂了每篇头一句）。
        assert!(prompt.contains("完整的摘录正文，不是短摘要。"), "{prompt}");
    }

    #[test]
    fn grounding_segments_fail_closed_for_missing_or_invalid_citations() {
        let sources = vec![source(1, "甲", "Notes/甲.md")];
        let grounded = classify_grounded_answer("结论来自这里。[1]", &sources);
        assert_eq!(grounded.grounding_outcome, GroundingOutcome::GroundedOnly);
        assert_eq!(grounded.sources.len(), 1);

        let mixed = classify_grounded_answer(
            "结论来自这里。[1]\n\n<<<LUNA_MODEL_SUPPLEMENT>>>\n\n这一段只是常识补充。",
            &sources,
        );
        assert_eq!(mixed.grounding_outcome, GroundingOutcome::Mixed);
        assert_eq!(mixed.segments.len(), 2);

        // 尾部没带引用的段落只降级**它自己**，前面合格的证据段照常成立——否则本地
        // 小模型随手写一句没编号的小标题，就会让整条带合法引用的回答变成「无依据」
        //（真机第七轮实测：11 篇检索、正文里有 [2]/[4]，界面却报「引用 0 篇」）。
        let tail_demoted = classify_grounded_answer(
            "第一段有依据。[1]\n\n## 补充区：常见误区\n\n这一段没有编号。",
            &sources,
        );
        assert_eq!(tail_demoted.grounding_outcome, GroundingOutcome::Mixed);
        assert_eq!(tail_demoted.sources.len(), 1);
        assert_eq!(tail_demoted.segments[0].markdown, "第一段有依据。[1]");
        assert!(tail_demoted.segments[1].markdown.contains("补充区"));
        assert!(!tail_demoted.segments[0].markdown.contains("没有编号"));

        // 小标题与「以冒号结尾的引导标签」是结构不是断言：要求它们带 `[n]` 是范畴错误，
        // 而真机上模型必然会写小标题——不放行，grounded 分类在本地模型上结构性不可达。
        let with_heading = classify_grounded_answer(
            "## 双均线系统\n\n结论来自这里。[1]\n\n- 要点：\n- 另一条依据。[1]",
            &sources,
        );
        assert_eq!(
            with_heading.grounding_outcome,
            GroundingOutcome::GroundedOnly
        );
        // 但纯结构、零断言不叫「有依据」。
        assert_eq!(
            classify_grounded_answer("## 只有小标题\n\n---", &sources).grounding_outcome,
            GroundingOutcome::ModelOnly
        );
        // 收窄判据的反向验证：以冒号结尾才是标签，成句的断言照旧要带引用。
        assert_eq!(
            classify_grounded_answer("关键逻辑：该系统靠两根均线同向。", &sources)
                .grounding_outcome,
            GroundingOutcome::ModelOnly
        );

        for broken in [
            "没有引用的断言。",
            "越界引用。[2]",
            "结论 [1]，但把 <<<LUNA_MODEL_SUPPLEMENT>>> 塞在句子里。",
            // **首段**没引用即整条无依据：证据段是「前缀」，不能从中间捡合格的段落
            //（那会重排正文顺序，也会把没依据的引导语留在证据区里）。
            "列表前的无引用断言。\n- 有依据的列表项。[1]",
            "A。[1]\n<<<LUNA_MODEL_SUPPLEMENT>>>\nB\n<<<LUNA_MODEL_SUPPLEMENT>>>\nC",
        ] {
            assert_eq!(
                classify_grounded_answer(broken, &sources).grounding_outcome,
                GroundingOutcome::ModelOnly,
                "{broken}"
            );
        }
    }

    /// 真机 2026-09-08：模型把哨兵抄成 `>>>LUNA_MODEL_SUPPLEMENT>>>`，Markdown 把它渲染成
    /// 三层引用块，界面上冒出一条竖条 + 协议词。判定认词干、剥离认所有形态。
    #[test]
    fn sentinel_variants_are_recognised_and_never_leak_into_segments() {
        let sources = vec![source(1, "甲", "Notes/甲.md")];
        for variant in [
            ">>>LUNA_MODEL_SUPPLEMENT>>>",
            "<<LUNA_MODEL_SUPPLEMENT>>",
            "<<< LUNA_MODEL_SUPPLEMENT >>>",
            "> <<<LUNA_MODEL_SUPPLEMENT>>>",
            "`<<<LUNA_MODEL_SUPPLEMENT>>>`",
        ] {
            let meta = classify_grounded_answer(
                &format!("结论来自这里。[1]\n{variant}\n这一段只是常识补充。"),
                &sources,
            );
            assert_eq!(meta.grounding_outcome, GroundingOutcome::Mixed, "{variant}");
            assert_eq!(meta.segments[0].markdown, "结论来自这里。[1]", "{variant}");
            assert_eq!(
                meta.segments[1].markdown, "这一段只是常识补充。",
                "{variant}"
            );
        }
        // 只有补充（哨兵在首行）：整条归模型补充，正文里没有协议词。
        let only = classify_grounded_answer(">>>LUNA_MODEL_SUPPLEMENT>>>\n只有补充。", &sources);
        assert_eq!(only.grounding_outcome, GroundingOutcome::ModelOnly);
        assert_eq!(only.segments[0].markdown, "只有补充。");
        // 行内泄漏：fail closed 归补充，且词干连同尖括号一起从正文里剥掉。
        let inline = classify_grounded_answer(
            "结论 [1]，但把 <<<LUNA_MODEL_SUPPLEMENT>>> 塞在句子里。",
            &sources,
        );
        assert_eq!(inline.grounding_outcome, GroundingOutcome::ModelOnly);
        assert!(!inline.segments[0].markdown.contains(MODEL_SUPPLEMENT_TOKEN));
        assert_eq!(inline.segments[0].markdown, "结论，但把 塞在句子里。");
    }

    /// 补充段里的 `[n]` 没有可指向的出处，是死角标：整条模型补充与混合回答的补充段都剥掉。
    #[test]
    fn dead_citation_markers_are_stripped_from_supplement_segments() {
        let sources = vec![source(1, "甲", "Notes/甲.md")];
        let only = classify_grounded_answer("Hello! [0]", &sources);
        assert_eq!(only.grounding_outcome, GroundingOutcome::ModelOnly);
        assert_eq!(only.segments[0].markdown, "Hello!");
        let mixed = classify_grounded_answer(
            "结论。[1]\n<<<LUNA_MODEL_SUPPLEMENT>>>\n补充抄了一个 [1]。",
            &sources,
        );
        assert_eq!(mixed.grounding_outcome, GroundingOutcome::Mixed);
        assert_eq!(mixed.segments[1].markdown, "补充抄了一个。");
    }

    #[test]
    fn echoed_claim_notices_are_removed_but_other_brackets_stay() {
        let notices = luna_vault::ClaimNotices {
            source_changed: "〔来源已变〕".into(),
            missing: "〔出处已不存在〕".into(),
            conflict: "[Conflicting statement on this page] ".into(),
        };
        assert_eq!(
            strip_claim_notices(
                "- 〔来源已变〕原型已完成 [1]\n- [Conflicting statement on this page] 首版支持移动端 [2]\n〔注〕保留",
                &notices,
            ),
            "- 原型已完成 [1]\n- 首版支持移动端 [2]\n〔注〕保留"
        );
    }

    #[test]
    fn fullwidth_citation_markers_count_as_citations() {
        let sources = vec![
            source(1, "甲", "Notes/甲.md"),
            source(2, "乙", "Notes/乙.md"),
        ];
        let meta = classify_grounded_answer(
            "首版支持移动端。【1】\n\n另一份记录不同。［２，１］",
            &sources,
        );
        assert!(matches!(
            meta.grounding_outcome,
            GroundingOutcome::GroundedOnly
        ));
        assert_eq!(meta.sources.len(), 2);
        assert_eq!(
            meta.segments[0].markdown,
            "首版支持移动端。[1]\n\n另一份记录不同。[2，1]"
        );
        // 括号里不是编号的照旧是正文。
        assert_eq!(
            normalize_citation_brackets("【注】预算【待定】"),
            "【注】预算【待定】"
        );
        assert_eq!(normalize_citation_brackets("未闭合【1"), "未闭合【1");
    }

    #[test]
    fn cited_indices_reads_answer_markers_and_ignores_links() {
        assert_eq!(cited_indices("依据 [1] 与 [3]。"), vec![1, 3]);
        assert_eq!(cited_indices("并列 [1][2] 与 [2,4]"), vec![1, 2, 4]);
        // wikilink 与 md 链接不能被误当引用编号。
        assert_eq!(
            cited_indices("见 [[某笔记]] 与 [文本](路径.md)"),
            Vec::<u32>::new()
        );
        assert_eq!(cited_indices("无引用"), Vec::<u32>::new());
    }

    #[test]
    fn sources_line_uses_portable_relative_links_not_blockquote_or_footnote() {
        let line = render_sources_line(
            "Chats/对话.md",
            &[
                source(1, "甲", "Notes/甲.md"),
                source(2, "乙", "Notes/乙.md"),
            ],
            true,
        );
        // blockquote 会与 callout 语法争首行；脚注在 Lunarca 的渲染器里会显示成字面量。
        assert!(!line.starts_with('>'), "{line}");
        assert!(!line.contains("[^"), "{line}");
        assert!(line.starts_with("**出处：**"), "{line}");
        // 可移植的标准相对 md 链接（PRD-06-030）。
        assert!(
            line.contains(
                "[甲](../Notes/%E7%94%B2.md)"
                    .replace("%E7%94%B2", "甲")
                    .as_str()
            ),
            "{line}"
        );
        assert!(line.contains(" · "), "多条出处以中点分隔：{line}");
        assert!(render_sources_line("a.md", &[], true).is_empty());
    }

    #[test]
    fn receipt_outcome_only_states_facts_that_actually_happened() {
        use super::KnowledgeMaintenanceStep as Step;
        use super::{outcome_of, KnowledgeReceiptFacts, ReceiptOutcome, ReceiptStage};
        let none = KnowledgeReceiptFacts::default();

        // 提交态：有后台步骤才说「正在整理」；三项设置都关着就只是「已保存」。
        assert_eq!(
            outcome_of(ReceiptStage::Committed, &none, &[Step::Tag]),
            ReceiptOutcome::Queued
        );
        assert_eq!(
            outcome_of(ReceiptStage::Committed, &none, &[]),
            ReceiptOutcome::Saved
        );

        // 终态穷尽：五种终态各自可达，且判据互斥。
        let updated = KnowledgeReceiptFacts {
            updated_topics: vec!["Notes/项目 X.md".into()],
            ..Default::default()
        };
        assert_eq!(
            outcome_of(ReceiptStage::Settled, &updated, &[]),
            ReceiptOutcome::Updated
        );
        let degraded = KnowledgeReceiptFacts {
            // 降级压过「已更新」：说「更新好了」而不提有一步没做完就是谎报。
            updated_topics: vec!["Notes/项目 X.md".into()],
            degraded_steps: vec![Step::Embed],
            ..Default::default()
        };
        assert_eq!(
            outcome_of(ReceiptStage::Settled, &degraded, &[]),
            ReceiptOutcome::Degraded
        );
        let failed = KnowledgeReceiptFacts {
            failed: true,
            updated_topics: vec!["Notes/项目 X.md".into()],
            ..Default::default()
        };
        assert_eq!(
            outcome_of(ReceiptStage::Settled, &failed, &[]),
            ReceiptOutcome::Failed
        );
        // 无事可报的终态回落到「已保存」，由发射点负责不再播第二张同义的卡。
        assert_eq!(
            outcome_of(ReceiptStage::Settled, &none, &[]),
            ReceiptOutcome::Saved
        );
    }

    /// 知识地图入口只回答位置，不承担建议（PRD-16-023 / ADR-087）。
    ///
    /// 读源码：命令要真 `AppState` + 真 Vault。要拦的是**职责膨胀**——谁在这里补一条
    /// 「没有就顺手建一个」或「有好几份就挑一个」，应用就有了第二个维护推荐面，两处
    /// 排序迟早不一致（那两种情形的闭环都在图谱维护区那条排好序的建议里）。
    #[test]
    fn knowledge_map_entry_only_answers_where_and_stays_silent_when_ambiguous() {
        let body_src = crate::ipc::source_without_tests(include_str!("knowledge.rs"));
        let body = body_src
            .split("pub(crate) async fn knowledge_map_entry(")
            .nth(1)
            .expect("找不到知识地图入口")
            .split("\n/// ")
            .next()
            .expect("入口边界变了");
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(code.len() > 150, "剥掉注释后扫描面为空");

        // ① 只读一次索引，恰好一份时才给路径——零份与多份都沉默。
        assert!(code.contains("vault.knowledge_maps()"));
        assert!(code.contains("maps.len() == 1"));

        // ② 不写盘、不建议、不挑一个：写入与收口的命令一个都不在这里。
        for escalation in [
            "create_or_refresh_knowledge_map",
            "knowledge_map_resolve_conflict",
            "write_knowledge_map",
            "emit_vault_changed",
            "maps.first()",
            "maps.swap_remove(0)",
        ] {
            assert!(!code.contains(escalation), "入口越权了：{escalation}");
        }
    }
}
