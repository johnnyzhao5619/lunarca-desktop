//! 库内检索（PRD-16 §3.2、ADR-026 ⑤⑥）：
//! 把一个**自然语言问题**变成「带出处的上下文」。
//!
//! 与 [`luna_sync_core::search`] 分工明确——那个模块面向**搜索视图**（一行查询语法、AND 语义、
//! 命中行高亮、可替换）；本模块面向**问答**：问题是一整句话，需要的是「哪几篇最相关 +
//! 每篇给一段可读摘录 + 一个能跳回去的锚」。
//!
//! ## 四条口径（都是刻意的）
//!
//! 1. **确定性管线，不是 agentic 工具调用**（ADR-026 ⑤）：词提取 → 索引粗筛 → 读文件精算
//!    → 摘录 → 预算装箱，每一步都是纯函数或确定性 IO，可无头单测、可预算、可解释。
//! 2. **索引只做粗筛、文件读取定权威**（与 [`luna_sync_core::search`] 同一条）：索引是派生数据，
//!    摘录与锚点必须对得上此刻磁盘上的字节，否则跳转会跳到错的地方。
//! 3. **禁区与索引同源**：时间码标记与结构脚手架行（转写哨兵 / 对话角色标题）不参与匹配、
//!    也不进摘录——判定复用 [`luna_sync_core::search::SearchableLine`]，不写第二份。
//! 4. **锚点是复合的、绝不写进源文件**（ADR-026 ⑥）：`{标题路径, 引文±前后文, 行提示}`
//!    按序降级解析；锚不上时如实告知「原文已改动」，**绝不静默跳到错行**。
//!
//! ## 为什么词提取要分「探测词」与「计分词」两套
//!
//! 索引侧 FTS 用的是 trigram 分词器——**少于 3 字符的词命中不了 `MATCH`**，只能退 `LIKE`
//! 全表扫描。故送进 SQL 的**探测词**恒取 ≥3 字符（CJK 取三字窗），把粗筛留在索引路径上；
//! 而读到文件之后的**计分词**可以更细（二字窗 + 整块），因为那时已经是内存里的字符串匹配，
//! 细粒度只提升准确度、不再有扫库代价。两套词表同源于一次切分，不是两套解析。

use crate::frontmatter::{body_of, NoteStage, NoteType};
use luna_sync_core::scaffold::timecode_spans;
use luna_sync_core::search::SearchableLine;

/// 送进 SQL 粗筛的探测词上限（每个词一次索引查询，故这是扫库次数的天花板）。
pub const MAX_PROBE_TERMS: usize = 8;
/// 读文件后用于计分的词上限（内存匹配，可比探测词细）。
pub const MAX_SCORE_TERMS: usize = 32;
/// 单个探测词取回的候选上限。
pub const CANDIDATES_PER_TERM: usize = 40;
/// 全部探测词合并后的候选上限（= 需要读盘的篇数天花板）。
pub const MAX_CANDIDATES: usize = 160;
/// 参与上下文的笔记数上限（默认；一次给模型太多篇会稀释每篇的篇幅）。
pub const DEFAULT_MAX_NOTES: usize = 6;
/// 全部摘录的字符预算（默认）。刻意保守——本地引擎上下文窗小，
/// 且摘录挤掉的是用户自己的对话历史。
pub const DEFAULT_CHAR_BUDGET: usize = 6_000;
/// 摘录窗在命中行上下各取几行。
const PASSAGE_CONTEXT_LINES: usize = 2;
/// 单段摘录的字符上限。
const MAX_PASSAGE_CHARS: usize = 1_200;
/// 锚点前后文各取多少字符（够消歧即可，存长了徒增漂移面）。
const ANCHOR_CONTEXT_CHARS: usize = 48;
/// 出处芯片 hover 预览的字符上限。
const SNIPPET_CHARS: usize = 160;
/// 引文模糊匹配的最短长度——太短的引文在长文里会撞出一堆假命中。
const MIN_QUOTE_CHARS: usize = 8;

/// CJK 功能字。**只用来判定「整个 n-gram 是否全由功能字组成」，绝不用来切断词。**
///
/// 这条纪律是踩过坑立的：按功能字**切断**长句看似能得到实词，实则会毁掉大量真词——
/// 「上」在停用表里（「以上」），于是「上周」被切成孤字丢掉；「中」「关」「个」「一」
/// 同理毁掉「中国」「关系」「个人」「一致」。中文没有词边界，字级停用表只能做**过滤**
/// 不能做**切分**：两个功能字连在一起（「是什」「的了」）必然不是实词，可以安全丢弃；
/// 而只要有一个实字，这个二字窗就可能是真词，必须留下。
const CJK_FUNCTION_CHARS: &str = "的了是在和与及对于我你他她它这那些什么怎样如何为何吗呢啊吧哪个么有没不都很就也还要会能可被把让给从到向并且但而或如因所以一之其此该则由至已将中关下上以及等请帮们";

/// 英文停用词（小写比对）。只收最高频的功能词——停用词表越长越容易误伤专有名词。
const LATIN_STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "of", "to", "in", "on",
    "at", "for", "with", "by", "from", "as", "and", "or", "but", "if", "then", "than", "that",
    "this", "these", "those", "it", "its", "i", "we", "you", "he", "she", "they", "them", "my",
    "our", "your", "their", "do", "does", "did", "done", "have", "has", "had", "what", "which",
    "who", "whom", "how", "why", "when", "where", "can", "could", "would", "should", "will",
    "shall", "may", "might", "must", "about", "into", "over", "under", "please", "me", "so",
];

/// 一次问题解析出的两套检索词（同源于一次切分，见模块文档）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryTerms {
    /// 送进索引粗筛的词（恒 ≥3 字符，走 FTS trigram 路径）。
    pub probe: Vec<String>,
    /// 读到文件后用于计分的词（更细，含二字窗）。
    pub score: Vec<String>,
}

impl QueryTerms {
    /// 无任何可用检索词（不应发起检索）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.score.is_empty()
    }
}

/// 一个「块」被当作 CJK 处理时，短到可以整块作检索词的上限（超过即只取窗）。
const MAX_WHOLE_CHUNK_CHARS: usize = 8;

/// 从自然语言问题提取检索词。
///
/// 切分只按**标点与空白**（非字母数字非下划线即分隔符），得到「块」；块内不再切断——
/// 于是 `项目X` 这类中英混排标识符天然保持完整（切开会同时毁掉「项目」与「X」两个信号）。
/// 含 CJK 的块取整块（≤[`MAX_WHOLE_CHUNK_CHARS`]）+ 滑动二字窗（计分）+ 滑动三字窗
/// （探测）；纯 Latin 块取整词并去停用词。全由功能字组成的窗一律丢弃（见
/// [`CJK_FUNCTION_CHARS`] 的纪律说明）。
#[must_use]
pub fn question_terms(question: &str) -> QueryTerms {
    let mut score: Vec<String> = Vec::new();
    let mut probe: Vec<String> = Vec::new();

    for chunk in split_chunks(question) {
        let chars: Vec<char> = chunk.chars().collect();
        if chars.iter().copied().any(is_cjk) {
            if (2..=MAX_WHOLE_CHUNK_CHARS).contains(&chars.len()) && !is_all_function(&chunk) {
                push_unique(&mut score, chunk.clone());
                if chars.len() >= 3 {
                    push_unique(&mut probe, chunk.clone());
                }
            }
            // 二字窗给计分（细）、三字窗给探测（FTS trigram 可命中）。
            for window in chars.windows(2) {
                let term: String = window.iter().collect();
                if !is_all_function(&term) {
                    push_unique(&mut score, term);
                }
            }
            for window in chars.windows(3) {
                let term: String = window.iter().collect();
                if !is_all_function(&term) {
                    push_unique(&mut probe, term);
                }
            }
        } else {
            let lowered = chunk.to_lowercase();
            if lowered.chars().count() < 2 || LATIN_STOP_WORDS.contains(&lowered.as_str()) {
                continue;
            }
            push_unique(&mut score, lowered.clone());
            if lowered.chars().count() >= 3 {
                push_unique(&mut probe, lowered);
            }
        }
    }

    // 长词更具体 → 优先保留（截断时先丢最泛的）。
    sort_by_specificity(&mut score);
    sort_by_specificity(&mut probe);
    score.truncate(MAX_SCORE_TERMS);
    probe.truncate(MAX_PROBE_TERMS);
    QueryTerms { probe, score }
}

/// 整串都是功能字/数字——这种 n-gram 不可能是实词，丢弃它只减噪不减召回。
fn is_all_function(term: &str) -> bool {
    term.chars()
        .all(|c| CJK_FUNCTION_CHARS.contains(c) || c.is_ascii_digit())
}

fn sort_by_specificity(terms: &mut [String]) {
    terms.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !value.is_empty() && !out.contains(&value) {
        out.push(value);
    }
}

/// 按标点与空白切块；块内一律不切（中英混排标识符如 `项目X` 保持完整）。
fn split_chunks(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// CJK 表意文字（含扩展 A 与兼容区）与日文假名——判定只服务「要不要按字切窗」，
/// 故把同样无词边界的假名一并纳入。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x30FF      // 平假名 / 片假名
        | 0x3400..=0x4DBF    // CJK 扩展 A
        | 0x4E00..=0x9FFF    // CJK 基本区
        | 0xF900..=0xFAFF    // CJK 兼容表意
    )
}

/// 复合锚（ADR-026 ⑥）：**记录**在派生层，**绝不写进源文件**。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Anchor {
    /// 从文首到该位置的标题路径（`["项目 X", "本周进展"]`）。
    pub heading_path: Vec<String>,
    /// 引文（摘录首行的规范化文本）。
    pub quote: String,
    /// 引文之前的上下文（消歧用）。
    pub prefix: String,
    /// 引文之后的上下文（消歧用）。
    pub suffix: String,
    /// 行提示（1-based，相对整篇文件；**只是提示**，解析时最后才用）。
    pub line_hint: u32,
    /// 生成锚点时源笔记正文的指纹。只用于归档后判断来源是否发生过变化，不参与定位。
    pub source_hash: String,
}

/// 锚点解析结果。区分「找到了」与「只能给个大概」——后者必须如实告知用户。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorHit {
    /// 引文在原处（含前后文一致）。
    Exact(u32),
    /// 引文找到了，但位置移动过（编辑导致行号漂移）——跳转照常准确。
    Moved(u32),
    /// 引文没了，只能定位到它所属的标题。
    Heading(u32),
    /// 完全锚不上：原文已改动。
    Lost,
}

impl AnchorHit {
    /// 可跳转的行号（1-based）；[`AnchorHit::Lost`] 无行号。
    #[must_use]
    pub fn line(self) -> Option<u32> {
        match self {
            Self::Exact(line) | Self::Moved(line) | Self::Heading(line) => Some(line),
            Self::Lost => None,
        }
    }

    /// 是否精确命中引文——`false` 即须向用户如实说明「原文已改动」。
    #[must_use]
    pub fn is_exact(self) -> bool {
        matches!(self, Self::Exact(_) | Self::Moved(_))
    }
}

/// 一段摘录（= 一条出处）。
#[derive(Clone, Debug, PartialEq)]
pub struct Passage {
    /// 笔记相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 笔记类型 kebab 键（前端按此分「知识 / 资料」呈现，判定在 Rust）。
    pub note_type: String,
    /// 这条出处**来自记录**吗（ADR-086 / PRD-16-041）：引用标注与透明行分组的事实来源。
    /// 判定在 Rust（[`crate::NoteStage`]），前端只渲染。
    pub draft: bool,
    /// 摘录正文（已剥时间码、已去脚手架行）。
    pub text: String,
    /// 复合锚。
    pub anchor: Anchor,
    /// 相关度（越大越相关）。
    pub score: f64,
}

impl Passage {
    /// hover 预览用的短摘要。
    #[must_use]
    pub fn snippet(&self) -> String {
        truncate_chars(&self.text.replace('\n', " "), SNIPPET_CHARS)
    }
}

/// 一次检索的结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Retrieval {
    /// 入选的摘录（按相关度降序，编号即出处序号）。
    pub passages: Vec<Passage>,
    /// 实际读盘打分的候选篇数（= 界面上的「已检索 N 篇」）。
    pub scanned: u32,
    /// 候选触顶——结果不完整，UI 须如实提示。
    pub truncated: bool,
}

/// 一次 `@` 提及框定的检索范围（PRD-16-009）。
///
/// 三种形态对应用户脑子里已有的三个概念（某一篇 / 某个文件夹 / 某个标签），**不发明
/// 第四种**——`@` 的语义之所以零教学成本，正因为它指的就是库里看得见的那些东西。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetrieveScope {
    /// 某一篇笔记（相对路径）。
    Note(String),
    /// 某个文件夹（相对路径，含子孙）。
    Folder(String),
    /// 某个标签（含子标签，`项目` 覆盖 `项目/luna`）。
    Tag(String),
}

/// 检索选项。**默认值即产品默认口径**（PRD-16-008），覆盖项由命令层按用户此刻的
/// 显式动作传入，不落设置。
#[derive(Clone, Debug)]
pub struct RetrieveOptions {
    /// 参与上下文的笔记数上限。
    pub max_notes: usize,
    /// 全部摘录的字符预算。
    pub char_budget: usize,
    /// 临时纳入对话笔记（透明行内的一次性动作，PRD-16-008）。
    pub include_chat: bool,
    /// 本次**整组排除记录态**（透明行内的一次性动作，PRD-16-041）。
    ///
    /// 与 `include_chat` 同为透明行上的口径翻转，方向相反：一次查询有效、**不改设置**。
    /// 之所以必须由 Rust 承担而不能让前端筛掉已返回的命中——「排除这一组并重答」要的是
    /// **重新检索**，前端只能删掉它看得见的那几条，库里其余记录态照样会在下一轮补位。
    pub exclude_drafts: bool,
    /// 本次剔除的笔记路径（透明行内「不要这篇」，PRD-16-006）。
    pub excluded: Vec<String>,
    /// 本次 `@` 提及框定的范围（空 = 全库，PRD-16-009）。多条取**并集**——
    /// `@工作 @项目 X` 读作"在这两处里找"，取交集会让第二次提及把结果清空。
    pub scope: Vec<RetrieveScope>,
    /// **已算好的**语义命中（按相似度降序，ADR-027 / PRD-16-028）。
    ///
    /// 由调用方（命令层）先跑一次嵌入 + 内存余弦得到——`luna-vault` 不认识 Provider，
    /// 也不做 async（依赖方向：只依赖 `luna-core`，TDD-00 §3）。故这里收的是**结果**
    /// 而不是"去算一下"。
    ///
    /// **空 = 纯关键词检索，行为与批次 1 逐字节一致**（缺模型/关闭语义时的降级路径
    /// 必须是"什么都没变"，而不是"另一套排序"，PRD-16-029）。
    pub semantic: Vec<crate::semantic::SemanticHit>,
    /// 主题页里出处已失效的声明在摘录中的行内说明（K4）；空即不加注。文案随界面语言，
    /// 由命令层给——`luna-vault` 不认识 i18n。
    pub claim_notices: ClaimNotices,
}

/// 主题页声明的行内说明文案（每种情况一句，结尾自带分隔）：出处已失效，或本页对它有
/// 分歧说法——模型据此不把它当作当前已核实、无争议的事实。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaimNotices {
    /// 来源正文已不是引用时的那一版。
    pub source_changed: String,
    /// 来源已不存在。
    pub missing: String,
    /// 本页对这条有分歧说法（分歧本身与它所并列的原说法都标）；不判定哪边对。
    pub conflict: String,
}

impl ClaimNotices {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source_changed.is_empty() && self.missing.is_empty() && self.conflict.is_empty()
    }
}

impl Default for RetrieveOptions {
    fn default() -> Self {
        Self {
            max_notes: DEFAULT_MAX_NOTES,
            char_budget: DEFAULT_CHAR_BUDGET,
            include_chat: false,
            exclude_drafts: false,
            excluded: Vec::new(),
            scope: Vec::new(),
            semantic: Vec::new(),
            claim_notices: ClaimNotices::default(),
        }
    }
}

/// 按**行号**抽一段摘录（语义命中专用，ADR-027）。
///
/// 为什么必须有这一条路径：[`score_note`] 是**词命中驱动**的——一个词都没命中就返回
/// `None`。而语义检索存在的全部理由正是「找到那些字面上一个词都没命中的笔记」（S16 实测
/// 三条跨语言查询字面命中率为 0）。若语义命中仍要过 `score_note` 才能拿到摘录，它们会被
/// 系统性地丢在最后一步——检索做了，结果一条也进不了上下文。
///
/// `line`（1-based，相对整篇文件）来自分片的 `line_start`：那是分块时记下的真实原文行号。
#[must_use]
pub fn passage_at_line(
    path: &str,
    title: &str,
    note_type: NoteType,
    stage: NoteStage,
    content: &str,
    line: u32,
    score: f32,
) -> Option<Passage> {
    let body = body_of(content);
    let base_line = crate::frontmatter::body_base_line(content);
    let lines: Vec<&str> = body.split('\n').map(strip_cr).collect();
    // 文件行 → 正文行下标；越界即钳回正文范围（笔记在建库之后被改短是常态）。
    let idx = (line as usize)
        .saturating_sub(base_line + 1)
        .min(lines.len().saturating_sub(1));
    let text = passage_window(&lines, idx);
    if text.trim().is_empty() {
        return None;
    }
    Some(Passage {
        path: path.to_string(),
        title: title.to_string(),
        note_type: note_type.key().to_string(),
        draft: stage.is_draft(),
        text,
        anchor: capture_anchor(&lines, idx, base_line, body),
        // 余弦（0–1）与关键词的 bm25 式累加分不可比，故这里**只作同路内排序**用；
        // 跨路的次序由 RRF 按排名决定（见 `crate::semantic`），不靠这个数。
        score: f64::from(score) * stage_weight(stage),
    })
}

/// 某篇笔记是否落在 `@` 框定的范围内（PRD-16-009，判定在 Rust——前端只搬运提及）。
///
/// `tags` 是该篇的全部标签（含 AI 标注，与检索同池）。标签比对**大小写不敏感且认子标签**，
/// 与标签面板/图谱同一口径；文件夹认前缀（含子孙）。
#[must_use]
pub fn in_scope(path: &str, tags: &[String], scope: &[RetrieveScope]) -> bool {
    if scope.is_empty() {
        return true;
    }
    scope.iter().any(|entry| match entry {
        RetrieveScope::Note(target) => path == target,
        RetrieveScope::Folder(dir) => {
            let dir = dir.trim_end_matches('/');
            dir.is_empty() || path.starts_with(&format!("{dir}/"))
        }
        RetrieveScope::Tag(tag) => tags.iter().any(|owned| {
            owned.eq_ignore_ascii_case(tag)
                || owned.len() > tag.len()
                    && owned[..tag.len()].eq_ignore_ascii_case(tag)
                    && owned.as_bytes()[tag.len()] == b'/'
        }),
    })
}

/// 这一篇是否参与本轮问答检索——**两根轴上的两道闸门，只有这一处**。
///
/// **来源轴（ADR-026 ②b）**：默认排除对话笔记——它是 AI 产物的镜像，作为事实源即
/// 「AI 引用 AI」自证闭环；而资料层（录音/转写）**必须在**，它们才是原文。
/// `include_chat` 为真时（用户在透明行里显式纳入这一次）对话笔记照常参与。
///
/// **成熟度轴（PRD-16-041）**：记录态默认**参与**——它们是用户之言，不进默认排除集；
/// 只有用户在透明行里显式「排除记录并重答」（`exclude_drafts`）时才整组退场。
/// 平时它只是降权（[`stage_weight`]），**降权不排除**。
#[must_use]
pub fn allows_grounding(note_type: NoteType, stage: NoteStage, options: &RetrieveOptions) -> bool {
    if stage.is_draft() && options.exclude_drafts {
        return false;
    }
    match note_type {
        NoteType::Chat => options.include_chat,
        _ => true,
    }
}

/// 资料区的相关度权重：**降权不排除**（与 ADR-024 搜索分组同一口径）——
/// 转写可查是核心价值，但同等相关时知识笔记该排在前面。
const SOURCE_MATERIAL_WEIGHT: f64 = 0.85;
/// 记录态的相关度权重（PRD-16-041）：同样**降权不排除**——记录是用户之言，不是 `chat`
/// 那种「AI 引用 AI」的自证闭环，故不进默认排除集；但同一事实同时存在于记录与知识时，
/// 回答该优先引用已经沉淀过的那一篇。
///
/// **只是权重，不是阈值**：ADR-027 ④ 明令不设绝对分数阈值，降权的全部作用是让记录态
/// 在各路排序里**让位**，融合名次随之下滑（RRF 是重排而非过滤）。一旦某轮里只有记录态
/// 有话说，它照样进得了上下文——那正是"降权不排除"与"永远搜不到"的分界。
///
/// > 假设：取 0.92（理由：需求只给了「降权」的方向没给数值。ADR-086 ③ 的总原则是
/// > 「记录态介于资料与知识之间」，故权重取在 `SOURCE_MATERIAL_WEIGHT` 与 1.0 之间；
/// > 偏向 1.0 一侧是因为记录终究是用户亲手写的，惩罚该比机器产物轻。真机反馈后再议）。
const DRAFT_WEIGHT: f64 = 0.92;

/// 记录层三态各自的相关度权重——**问答检索里唯一一处**把 stage 换算成分数的地方。
fn stage_weight(stage: NoteStage) -> f64 {
    match stage {
        NoteStage::SourceMaterial => SOURCE_MATERIAL_WEIGHT,
        NoteStage::Draft => DRAFT_WEIGHT,
        NoteStage::Knowledge => 1.0,
    }
}
/// 标题命中的加权（标题是作者亲手写的主题声明，比正文任一处都更有信号）。
const TITLE_HIT_WEIGHT: f64 = 3.0;

/// 给一篇笔记打分并抽取最佳摘录。返回 `None` = 一个计分词都没命中。
///
/// `content` 是**此刻磁盘上的完整文件内容**（含 frontmatter），行号据此计算。
#[must_use]
pub fn score_note(
    path: &str,
    title: &str,
    note_type: NoteType,
    stage: NoteStage,
    content: &str,
    terms: &[String],
) -> Option<Passage> {
    let body = body_of(content);
    let base_line = crate::frontmatter::body_base_line(content);

    let title_lower = title.to_lowercase();
    let mut score = 0.0f64;
    for term in terms {
        if title_lower.contains(&term.to_lowercase()) {
            score += TITLE_HIT_WEIGHT * term_weight(term);
        }
    }

    // 逐行计分：每行的分 = 命中词权重之和；摘录以最高分行为中心。
    let lines: Vec<&str> = body.split('\n').map(strip_cr).collect();
    let mut line_scores = vec![0.0f64; lines.len()];
    let mut any_hit = false;
    for (idx, line) in lines.iter().enumerate() {
        let searchable = SearchableLine::new(line, false);
        let mut line_score = 0.0;
        for term in terms {
            let hits = searchable.find_all(term).count();
            if hits > 0 {
                // 同一行内重复命中收益递减：一行说十遍不比说一遍更相关。
                line_score += term_weight(term) * (1.0 + (hits as f64 - 1.0) * 0.25);
            }
        }
        if line_score > 0.0 {
            any_hit = true;
        }
        line_scores[idx] = line_score;
        score += line_score;
    }
    if !any_hit && score <= 0.0 {
        return None;
    }

    let best = line_scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    let text = passage_window(&lines, best);
    if text.trim().is_empty() {
        return None;
    }
    let anchor = capture_anchor(&lines, best, base_line, body);

    Some(Passage {
        path: path.to_string(),
        title: title.to_string(),
        note_type: note_type.key().to_string(),
        draft: stage.is_draft(),
        text,
        anchor,
        score: score * stage_weight(stage),
    })
}

/// 词权重：长词更具体故更值钱（一个廉价但有效的 IDF 代理——库内词频统计要多扫一遍库，
/// 而检索是交互路径上的操作，这点准确度不值那次扫描）。
fn term_weight(term: &str) -> f64 {
    let len = term.chars().count() as f64;
    1.0 + (len - 1.0) * 0.5
}

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// 以命中行为中心取窗，跳过脚手架行与前导空行；返回可读的摘录文本。
///
/// 摘录**是给人读的**，锚点**是给跳转用的**——两者刻意解耦（见 [`capture_anchor`]）：
/// 摘录可以从上文的标题开始（读起来有语境），锚点却必须钉在命中那一行。
fn passage_window(lines: &[&str], center: usize) -> String {
    let start = center.saturating_sub(PASSAGE_CONTEXT_LINES);
    let end = (center + PASSAGE_CONTEXT_LINES + 1).min(lines.len());
    let mut out = String::new();
    let mut seen = false;
    for line in lines.iter().take(end).skip(start) {
        if luna_sync_core::scaffold::is_scaffold_line(line) {
            continue;
        }
        let cleaned = strip_timecodes(line);
        if cleaned.trim().is_empty() && !seen {
            continue;
        }
        if seen {
            out.push('\n');
        }
        seen = true;
        out.push_str(cleaned.trim_end());
    }
    truncate_chars(out.trim(), MAX_PASSAGE_CHARS)
}

/// 剥掉行内时间码标记（与索引侧 `clean_for_fts` 同源判定，两处一个口径）。
fn strip_timecodes(line: &str) -> String {
    let spans = timecode_spans(line);
    if spans.is_empty() {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        out.push_str(&line[cursor..start]);
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    out
}

/// 捕获复合锚：标题路径 + 引文 ± 前后文 + 行提示。
///
/// **引文取的是命中行，不是摘录首行**。初版取摘录首行，于是命中点上方恰好是标题时，
/// 引文会变成 `# 标题` 这种三五个字符的短串——短到过不了 [`MIN_QUOTE_CHARS`] 的模糊匹配
/// 门槛，锚点当场退化成「只知道在哪一节」。锚点要钉在**最有内容的那一行**才立得住。
fn capture_anchor(lines: &[&str], focus: usize, base_line: usize, body: &str) -> Anchor {
    let quote = strip_timecodes(lines.get(focus).copied().unwrap_or_default())
        .trim()
        .to_string();
    let prefix = lines[..focus]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && !luna_sync_core::scaffold::is_scaffold_line(l))
        .map(|l| tail_chars(strip_timecodes(l).trim(), ANCHOR_CONTEXT_CHARS))
        .unwrap_or_default();
    let suffix = lines
        .get(focus + 1..)
        .unwrap_or_default()
        .iter()
        .find(|l| !l.trim().is_empty() && !luna_sync_core::scaffold::is_scaffold_line(l))
        .map(|l| truncate_chars(strip_timecodes(l).trim(), ANCHOR_CONTEXT_CHARS))
        .unwrap_or_default();
    Anchor {
        heading_path: heading_path_at(lines, focus),
        quote,
        prefix,
        suffix,
        line_hint: (base_line + focus + 1) as u32,
        source_hash: luna_sync_core::hashing::ContentHash::of(body.as_bytes())
            .as_str()
            .to_owned(),
    }
}

/// 该行所处的标题路径（按 `#` 级别维护栈）。
fn heading_path_at(lines: &[&str], target: usize) -> Vec<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    for line in lines.iter().take(target + 1) {
        let Some((level, title)) = parse_heading(line) else {
            continue;
        };
        while stack.last().is_some_and(|(l, _)| *l >= level) {
            stack.pop();
        }
        stack.push((level, title));
    }
    stack.into_iter().map(|(_, title)| title).collect()
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed[hashes..].strip_prefix(' ')?.trim();
    (!rest.is_empty()).then(|| (hashes, rest.to_string()))
}

/// 按锚点在**此刻的文件内容**里定位（ADR-026 ⑥：标题路径 → 引文模糊匹配 → 行提示）。
///
/// 解析顺序是刻意的：引文最能代表「用户当初读到的那句话」，标题次之，行号最不可靠。
/// 但引文若太短则跳过模糊匹配（短串在长文里会撞出假命中，跳错比说不知道更糟）。
#[must_use]
pub fn resolve_anchor(content: &str, anchor: &Anchor) -> AnchorHit {
    let body = body_of(content);
    let base_line = crate::frontmatter::body_base_line(content);
    let lines: Vec<&str> = body.split('\n').map(strip_cr).collect();

    let quote = anchor.quote.trim();
    if quote.chars().count() >= MIN_QUOTE_CHARS {
        let mut candidates: Vec<usize> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            if strip_timecodes(line).contains(quote) {
                candidates.push(idx);
            }
        }
        if !candidates.is_empty() {
            // 多处命中时用前后文消歧，再退行提示就近。
            let best = candidates
                .iter()
                .copied()
                .max_by_key(|idx| context_match_score(&lines, *idx, anchor))
                .unwrap_or(candidates[0]);
            let line = (base_line + best + 1) as u32;
            return if line == anchor.line_hint {
                AnchorHit::Exact(line)
            } else {
                AnchorHit::Moved(line)
            };
        }
    }

    // 引文没了 → 退标题路径（末级标题仍在即可定位到那一节）。
    if let Some(title) = anchor.heading_path.last() {
        for (idx, line) in lines.iter().enumerate() {
            if parse_heading(line).is_some_and(|(_, t)| &t == title) {
                return AnchorHit::Heading((base_line + idx + 1) as u32);
            }
        }
    }
    AnchorHit::Lost
}

/// 前后文吻合度（0..=2）：用于同一引文多处出现时选出最像原处的那一个。
fn context_match_score(lines: &[&str], idx: usize, anchor: &Anchor) -> u8 {
    let mut score = 0;
    if !anchor.prefix.is_empty()
        && idx > 0
        && lines[..idx]
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.contains(anchor.prefix.trim()))
    {
        score += 1;
    }
    if !anchor.suffix.is_empty()
        && lines
            .get(idx + 1..)
            .unwrap_or_default()
            .iter()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.contains(anchor.suffix.trim()))
    {
        score += 1;
    }
    score
}

/// 按预算装箱：相关度降序取，直到篇数或字符预算触顶。
///
/// 一篇笔记只入选一段（最相关的那段）——同一篇给两段会挤掉另一篇的位置，
/// 而「出处覆盖几篇」比「某一篇给几段」更能支撑判断。
#[must_use]
pub fn pack_passages(mut passages: Vec<Passage>, options: &RetrieveOptions) -> Vec<Passage> {
    passages.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut out = Vec::new();
    let mut used = 0usize;
    for passage in passages {
        if out.len() >= options.max_notes {
            break;
        }
        let cost = passage.text.chars().count();
        if used + cost > options.char_budget && !out.is_empty() {
            break;
        }
        used += cost;
        out.push(passage);
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    text.chars().skip(count - max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `@` 范围判定的边界（PRD-16-009）：文件夹**认目录不认前缀**，标签认子标签。
    #[test]
    fn scope_matches_folders_by_path_segment_not_by_prefix() {
        let tags = vec!["项目/luna".to_string()];
        // 空范围 = 全库（既有行为不变）。
        assert!(in_scope("任意/甲.md", &[], &[]));

        let folder = [RetrieveScope::Folder("工作".into())];
        assert!(in_scope("工作/甲.md", &tags, &folder));
        assert!(in_scope("工作/子/乙.md", &tags, &folder), "含子孙");
        // `工作坊/` 不是 `工作/` 的子目录——裸前缀比较会把它错收进来，
        // 而用户 `@工作` 时脑子里指的是那个文件夹，不是所有以这两个字开头的文件夹。
        assert!(!in_scope("工作坊/丙.md", &tags, &folder));
        assert!(
            !in_scope("工作.md", &tags, &folder),
            "同名文件不是那个文件夹"
        );

        let tag = [RetrieveScope::Tag("项目".into())];
        assert!(in_scope("任意/甲.md", &tags, &tag), "父标签覆盖子标签");
        assert!(in_scope("任意/甲.md", &["项目".to_string()], &tag));
        assert!(
            !in_scope("任意/甲.md", &["项目管理".to_string()], &tag),
            "不是子标签"
        );
        assert!(!in_scope("任意/甲.md", &[], &tag));

        let note = [RetrieveScope::Note("工作/甲.md".into())];
        assert!(in_scope("工作/甲.md", &[], &note));
        assert!(!in_scope("工作/乙.md", &[], &note));
    }

    #[test]
    fn splits_cjk_and_latin_and_drops_stop_chars() {
        let terms = question_terms("上周关于项目X的进展和风险是什么？");
        for expected in ["上周", "项目", "进展", "风险"] {
            assert!(
                terms.score.iter().any(|t| t == expected),
                "实词 {expected} 必须在场：{:?}",
                terms.score
            );
        }
        // 「上周」是这条测试的要害：`上` 在功能字表里，若拿功能字去**切断**长句，
        // 它会被切成孤字丢掉。功能字只许用于过滤全功能字窗，不许切词。
        assert!(
            terms.score.iter().any(|t| t == "目X"),
            "中英混排标识符不切开：{:?}",
            terms.score
        );
        // 全功能字的窗（`是什`/`什么`）是噪声，丢掉只减噪不减召回。
        assert!(
            !terms.score.iter().any(|t| t == "什么"),
            "{:?}",
            terms.score
        );
        // 探测词恒 ≥3 字符（FTS trigram 命中不了更短的）。
        assert!(
            terms.probe.iter().all(|t| t.chars().count() >= 3),
            "{:?}",
            terms.probe
        );
    }

    #[test]
    fn drops_latin_stop_words_and_lowercases() {
        let terms = question_terms("What is the Redis max_memory setting?");
        assert!(terms.score.iter().any(|t| t == "redis"));
        assert!(terms.score.iter().any(|t| t == "max_memory"));
        assert!(!terms.score.iter().any(|t| t == "the" || t == "is"));
    }

    #[test]
    fn empty_question_yields_no_terms() {
        assert!(question_terms("   ？！，。  ").is_empty());
        assert!(question_terms("").is_empty());
    }

    #[test]
    fn caps_term_counts_keeping_the_most_specific() {
        let long = "人工智能与机器学习的模型训练推理部署监控评估优化流程".repeat(4);
        let terms = question_terms(&long);
        assert!(terms.probe.len() <= MAX_PROBE_TERMS);
        assert!(terms.score.len() <= MAX_SCORE_TERMS);
        // 截断时先丢最泛的：留下的首项不短于末项。
        let first = terms.score.first().unwrap().chars().count();
        let last = terms.score.last().unwrap().chars().count();
        assert!(first >= last);
    }

    #[test]
    fn scores_title_hits_higher_than_body_hits() {
        let terms = vec!["项目".to_string()];
        let body_only = score_note(
            "a.md",
            "无关标题",
            NoteType::Note,
            NoteStage::Knowledge,
            "---\nluna:\n  schema: 1\n---\n项目进展良好\n",
            &terms,
        )
        .unwrap();
        let with_title = score_note(
            "b.md",
            "项目周报",
            NoteType::Note,
            NoteStage::Knowledge,
            "---\nluna:\n  schema: 1\n---\n项目进展良好\n",
            &terms,
        )
        .unwrap();
        assert!(with_title.score > body_only.score);
    }

    #[test]
    fn source_material_is_down_ranked_not_excluded() {
        let terms = vec!["项目".to_string()];
        let content = "---\nluna:\n  schema: 1\n---\n项目进展良好\n";
        let note = score_note(
            "a.md",
            "标题",
            NoteType::Note,
            NoteStage::Knowledge,
            content,
            &terms,
        )
        .unwrap();
        let transcript = score_note(
            "b.md",
            "标题",
            NoteType::Transcript,
            NoteStage::SourceMaterial,
            content,
            &terms,
        )
        .unwrap();
        assert!(transcript.score < note.score, "资料降权");
        assert!(transcript.score > 0.0, "但绝不排除——转写可查是核心价值");
    }

    /// 记录态同为「降权不排除」（PRD-16-041），且惩罚**轻于**资料
    /// （ADR-086 ③：记录态介于资料与知识之间）。反向验证：把 `DRAFT_WEIGHT` 改回 1.0，
    /// 第一条断言即红；改成排除（返回 `None`），第三条即红。
    #[test]
    fn drafts_are_down_ranked_less_than_material_and_never_excluded() {
        let terms = vec!["项目".to_string()];
        let content = "---\nluna:\n  schema: 1\n---\n项目进展良好\n";
        let knowledge = score_note(
            "a.md",
            "标题",
            NoteType::Note,
            NoteStage::Knowledge,
            content,
            &terms,
        )
        .unwrap();
        let draft = score_note(
            "b.md",
            "标题",
            NoteType::Note,
            NoteStage::Draft,
            content,
            &terms,
        )
        .unwrap();
        let material = score_note(
            "c.md",
            "标题",
            NoteType::Transcript,
            NoteStage::SourceMaterial,
            content,
            &terms,
        )
        .unwrap();
        assert!(draft.score < knowledge.score, "记录态让位于知识态");
        assert!(draft.score > material.score, "但比资料轻");
        assert!(draft.score > 0.0, "降权不排除——记录是用户之言");
        assert!(draft.draft, "命中须随身带上「来自记录」这个事实");
        assert!(!knowledge.draft && !material.draft);
    }

    #[test]
    fn skips_scaffold_lines_and_strips_timecodes_in_passages() {
        let terms = vec!["部署".to_string()];
        let content = "---\nluna:\n  schema: 1\n---\n## Assistant\n[00:01:02] 部署流程如下\n";
        let passage = score_note(
            "a.md",
            "标题",
            NoteType::Note,
            NoteStage::Knowledge,
            content,
            &terms,
        )
        .unwrap();
        assert!(!passage.text.contains("## Assistant"), "{}", passage.text);
        assert!(!passage.text.contains("00:01:02"), "{}", passage.text);
        assert!(passage.text.contains("部署流程如下"));
    }

    #[test]
    fn no_hit_yields_none() {
        let terms = vec!["完全不存在的词".to_string()];
        assert!(score_note(
            "a.md",
            "标题",
            NoteType::Note,
            NoteStage::Knowledge,
            "---\nluna:\n  schema: 1\n---\n正文\n",
            &terms
        )
        .is_none());
    }

    #[test]
    fn line_hint_counts_frontmatter_lines() {
        let terms = vec!["目标".to_string()];
        let content = "---\nluna:\n  schema: 1\n---\n\n第一段\n目标在这里\n";
        let passage = score_note(
            "a.md",
            "t",
            NoteType::Note,
            NoteStage::Knowledge,
            content,
            &terms,
        )
        .unwrap();
        // frontmatter 占 1–4 行、空行第 5、「第一段」第 6，命中行「目标在这里」是第 7。
        assert_eq!(passage.anchor.line_hint, 7, "{:?}", passage.anchor);
        // 摘录仍带上文语境（读得懂），锚却钉在命中行（跳得准）——两者刻意解耦。
        assert!(passage.text.contains("第一段"), "{}", passage.text);
    }

    #[test]
    fn anchor_captures_heading_path() {
        let terms = vec!["细节".to_string()];
        let content = "---\nluna:\n  schema: 1\n---\n# 项目 X\n## 本周进展\n### 交付物\n细节在此\n## 风险\n其它\n";
        let passage = score_note(
            "a.md",
            "t",
            NoteType::Note,
            NoteStage::Knowledge,
            content,
            &terms,
        )
        .unwrap();
        assert_eq!(
            passage.anchor.heading_path,
            vec!["项目 X", "本周进展", "交付物"],
            "标题路径按级别维护栈"
        );
        // 引文钉在命中行本身（而非摘录首行，那里可能是标题）。
        assert_eq!(passage.anchor.quote, "细节在此");
    }

    #[test]
    fn resolve_anchor_survives_line_drift() {
        let terms = vec!["关键结论在这里".to_string()];
        let original = "---\nluna:\n  schema: 1\n---\n# 标题\n关键结论在这里的正文\n";
        let passage = score_note(
            "a.md",
            "t",
            NoteType::Note,
            NoteStage::Knowledge,
            original,
            &terms,
        )
        .unwrap();

        // 用户在上方插入了三行 → 行号漂移，但引文还在。
        assert_eq!(passage.anchor.line_hint, 6, "锚钉在命中行而非摘录首行");
        let edited =
            "---\nluna:\n  schema: 1\n---\n# 标题\n新增一\n新增二\n新增三\n关键结论在这里的正文\n";
        let hit = resolve_anchor(edited, &passage.anchor);
        assert!(matches!(hit, AnchorHit::Moved(_)), "{hit:?}");
        assert!(hit.is_exact(), "找到了引文就是准确跳转");
        assert_eq!(hit.line(), Some(9));
    }

    #[test]
    fn resolve_anchor_degrades_to_heading_then_lost() {
        let terms = vec!["关键结论在这里".to_string()];
        let original = "---\nluna:\n  schema: 1\n---\n## 本周进展\n关键结论在这里的正文\n";
        let passage = score_note(
            "a.md",
            "t",
            NoteType::Note,
            NoteStage::Knowledge,
            original,
            &terms,
        )
        .unwrap();

        // 引文被删，标题还在 → 退到标题，且**必须**标记为不精确。
        let heading_only = "---\nluna:\n  schema: 1\n---\n## 本周进展\n完全换了内容\n";
        let hit = resolve_anchor(heading_only, &passage.anchor);
        assert_eq!(hit, AnchorHit::Heading(5));
        assert!(!hit.is_exact(), "退化命中必须如实告知原文已改动");

        // 标题也没了 → Lost，绝不猜一个行号。
        let gone = "---\nluna:\n  schema: 1\n---\n毫不相干\n";
        assert_eq!(resolve_anchor(gone, &passage.anchor), AnchorHit::Lost);
        assert_eq!(resolve_anchor(gone, &passage.anchor).line(), None);
    }

    #[test]
    fn resolve_anchor_disambiguates_by_context() {
        let anchor = Anchor {
            heading_path: vec![],
            quote: "重复出现的一句话".to_string(),
            prefix: "第二处之前".to_string(),
            suffix: String::new(),
            line_hint: 99,
            source_hash: String::new(),
        };
        let content = "---\nluna:\n  schema: 1\n---\n第一处之前\n重复出现的一句话\n第二处之前\n重复出现的一句话\n";
        // 前后文指向第二处，故不能被行提示或先到先得带偏。
        assert_eq!(resolve_anchor(content, &anchor).line(), Some(8));
    }

    #[test]
    fn short_quote_never_fuzzy_matches() {
        // 短引文在长文里必然撞车，宁可退化也不跳错（跳错比说不知道更糟）。
        let anchor = Anchor {
            heading_path: vec![],
            quote: "的".to_string(),
            prefix: String::new(),
            suffix: String::new(),
            line_hint: 5,
            source_hash: String::new(),
        };
        let content = "---\nluna:\n  schema: 1\n---\n他的书\n";
        assert_eq!(resolve_anchor(content, &anchor), AnchorHit::Lost);
    }

    #[test]
    fn chat_notes_are_excluded_unless_explicitly_included() {
        let default = RetrieveOptions::default();
        let with_chat = RetrieveOptions {
            include_chat: true,
            ..RetrieveOptions::default()
        };
        // 反投毒（ADR-026 ②b）：对话笔记默认不作事实源。
        assert!(!allows_grounding(
            NoteType::Chat,
            NoteStage::SourceMaterial,
            &default
        ));
        // 但排除可翻转——硬排除会把「默认不信」实现成「永远搜不到」。
        assert!(allows_grounding(
            NoteType::Chat,
            NoteStage::SourceMaterial,
            &with_chat
        ));
        // 资料层必须在：它们才是原文。
        for (t, stage) in [
            (NoteType::Note, NoteStage::Knowledge),
            (NoteType::QuickNote, NoteStage::Draft),
            (NoteType::Recording, NoteStage::SourceMaterial),
            (NoteType::Transcript, NoteStage::SourceMaterial),
        ] {
            assert!(allows_grounding(t, stage, &default), "{t:?} 必须参与检索");
        }
    }

    /// 记录态**默认参与**（PRD-16-041：它们是用户之言，不是自证闭环），只有透明行上
    /// 那次显式「排除记录并重答」才整组退场——一次查询有效、不改设置。
    #[test]
    fn drafts_participate_unless_the_user_excludes_the_whole_group() {
        let default = RetrieveOptions::default();
        assert!(allows_grounding(NoteType::Note, NoteStage::Draft, &default));
        let excluded = RetrieveOptions {
            exclude_drafts: true,
            ..RetrieveOptions::default()
        };
        assert!(!allows_grounding(
            NoteType::Note,
            NoteStage::Draft,
            &excluded
        ));
        // 翻转只针对记录态：知识与资料不受影响。
        assert!(allows_grounding(
            NoteType::Note,
            NoteStage::Knowledge,
            &excluded
        ));
        assert!(allows_grounding(
            NoteType::Transcript,
            NoteStage::SourceMaterial,
            &excluded
        ));
    }

    fn passage_of(path: &str, score: f64, chars: usize) -> Passage {
        Passage {
            path: path.to_string(),
            title: path.to_string(),
            note_type: "note".to_string(),
            draft: false,
            text: "内".repeat(chars),
            anchor: Anchor::default(),
            score,
        }
    }

    #[test]
    fn pack_respects_note_and_char_budgets() {
        let options = RetrieveOptions {
            max_notes: 2,
            char_budget: 100,
            ..RetrieveOptions::default()
        };
        let packed = pack_passages(
            vec![
                passage_of("c.md", 1.0, 10),
                passage_of("a.md", 3.0, 10),
                passage_of("b.md", 2.0, 10),
            ],
            &options,
        );
        assert_eq!(packed.len(), 2, "篇数上限");
        assert_eq!(packed[0].path, "a.md", "按相关度降序");
        assert_eq!(packed[1].path, "b.md");

        let budget = RetrieveOptions {
            max_notes: 10,
            char_budget: 25,
            ..RetrieveOptions::default()
        };
        let packed = pack_passages(
            vec![passage_of("a.md", 3.0, 20), passage_of("b.md", 2.0, 20)],
            &budget,
        );
        assert_eq!(packed.len(), 1, "字符预算触顶即停");
    }

    #[test]
    fn pack_always_keeps_at_least_one_passage() {
        // 单段就超预算时也要给一段——给不出上下文的"检索成功"是自欺。
        let options = RetrieveOptions {
            char_budget: 5,
            ..RetrieveOptions::default()
        };
        let packed = pack_passages(vec![passage_of("a.md", 1.0, 500)], &options);
        assert_eq!(packed.len(), 1);
    }
}
