//! 知识体检的**纯判定内核**（PRD-16 §3.7
//! `PRD-16-030/031/032`）。零 IO、可无头单测。
//!
//! 分工与 `semantic` / `retrieve` 一致：判据全在这里，`Vault` 只负责把索引里的行喂进来、
//! 把结果端出去，IPC 层只做编排与呈现投影。
//!
//! ## 两档的分界不是"深浅"，是**要不要烧模型**
//!
//! - **T0**（[`t0_findings`]）：纯 Rust、零 LLM。判据全部落在派生索引的几行 SQL 上，
//!   故可以随手跑、有发现才出现（`PRD-16-030`）。
//! - **T1**（[`FindingKind::is_deep`] 的三类）：矛盾/过时各要烧一次本地模型，近重复要有
//!   语义向量。故显式触发、走六要素骨架、结果落派生层（`PRD-16-031`）。
//!
//! ## 体检只出建议、绝不自动修（`PRD-16-032`）
//!
//! 本模块**没有任何写入函数**——它只会指出"这里可能有问题"，怎么处置永远是用户点的。
//! 这不是保守，是这一类功能的正确形态：矛盾检测靠的是本地小模型的判断，而它的误判率
//! 远高于"用户自己看一眼"的成本；一个会自动改笔记的体检器，错一次就够让人再也不敢开它。

use crate::frontmatter::{NoteStage, NoteType};
use crate::graph::{build_graph, GraphEdgeKind, GraphNodeKind, GraphRows};
use luna_sync_core::hashing::ContentHash;
use luna_sync_core::links::LinkIndex;

/// 发现的种类。**卡片文案由前端按种类查 i18n**——本模块只出种类与涉及对象，绝不出人读文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// 主题页的一条声明与它引用的源**互相矛盾**（T1，`PRD-16-031` 第一优先级）。
    Contradiction,
    /// 主题页的一条声明**已被源推翻或超越**（T1；与矛盾同一次判定的两种取值）。
    Outdated,
    /// 失效引用：主题页引用的源在这条声明写下**之后**又改过（T0）。
    StaleCitation,
    /// 两页内容近重复（T1，需要语义向量；`PRD-16-020` 防膨胀的同一判据）。
    NearDuplicate,
    /// 断链：链接指向的笔记不存在（T0，= 图谱的幽灵节点）。
    BrokenLink,
    /// 有文件夹读不了（T0，笔记库对标轮 V1b）：全库遍历时没有权限进入的子目录，其下的笔记
    /// 不进索引——搜不到、链接解析不到、知识库看不见，而此前没有任何地方说出来。
    UnreadableFolder,
    /// **还没有标签的知识笔记**（T0）：主题页建议要靠标签成簇（`PRD-16-016`：同标签
    /// ≥3 篇），没有标签就**永远不会有建议**——而在此之前，界面上没有任何地方说出这件事，
    /// 用户只能得出"主题页入口不明确"的结论（2026-07-30 真机第二/三轮连撞三次）。
    Untagged,
    /// 孤儿页：与库里任何东西都没有连接的知识笔记（T0）。
    Orphan,
    /// 空页：有文件、没正文的知识笔记（T0）。
    EmptyNote,
    /// 待整理积压（T0）。
    DistillBacklog,
    /// **待沉淀积压**（T0，PRD-06-055 / ADR-086 ②）：记录态笔记攒下了、且最旧的一篇
    /// 已经搁置了一段时间。
    ///
    /// 与「待整理资料」是对称的两道闸门（资料经蒸馏、记录经沉淀，同一条流水线的两条支流），
    /// 故同属积压类：不给 ✕、不计入启动概览、随处理自然消失。
    ///
    SettleBacklog,
}

impl FindingKind {
    /// 全部取值（与 [`Self::key`] 同源，供遍历与守护测试）。
    pub const ALL: [Self; 11] = [
        Self::Contradiction,
        Self::Outdated,
        Self::StaleCitation,
        Self::NearDuplicate,
        Self::BrokenLink,
        Self::UnreadableFolder,
        Self::Untagged,
        Self::Orphan,
        Self::EmptyNote,
        Self::DistillBacklog,
        Self::SettleBacklog,
    ];

    /// kebab 键——**唯一事实源**：id 前缀、派生表的 `kind` 列、前端 i18n 键共用。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Contradiction => "contradiction",
            Self::Outdated => "outdated",
            Self::StaleCitation => "stale-citation",
            Self::NearDuplicate => "near-duplicate",
            Self::BrokenLink => "broken-link",
            Self::UnreadableFolder => "unreadable-folder",
            Self::Untagged => "untagged",
            Self::Orphan => "orphan",
            Self::EmptyNote => "empty-note",
            Self::DistillBacklog => "distill-backlog",
            Self::SettleBacklog => "settle-backlog",
        }
    }

    /// 由 kebab 键还原；未知键 → `None`（不猜、不缺省——读到脏行如实丢弃那一行）。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// 是不是深度体检（T1）才会产出的种类：要烧本地模型或要有语义向量。
    #[must_use]
    pub fn is_deep(self) -> bool {
        matches!(
            self,
            Self::Contradiction | Self::Outdated | Self::NearDuplicate
        )
    }

    /// 是不是**流水线积压**（待整理资料 / 待整理内容 / 还没有标签）。
    ///
    /// 它与其余七类的性质不同：积压不是"库里有个地方坏了"，而是"流水线上游还有货"——
    /// 知识库脉搏（`PRD-16-027`）已经把它放在主位，故体检里它只是补一眼，且**不参与
    /// 「有发现才出现」的计数**（否则启动概览会在脉搏那一行下面把同一件事再说一遍）。
    #[must_use]
    pub fn is_backlog(self) -> bool {
        // 「还没有标签」同属这一类：它会随一次全库补齐自然消失，永久静音一个会自己消失的
        // 东西，只会让下一次真的缺标签时看不见；它也不该计入启动概览那一行的数字
        // （那一行报的是"库本身该收拾的地方"）。
        matches!(
            self,
            Self::DistillBacklog | Self::SettleBacklog | Self::Untagged
        )
    }

    /// 这条发现能不能被 ✕ 永久忽略。
    ///
    /// **积压卡不给 ✕**：它会随用户处理完自然消失，而"永久静音一个会自己消失的东西"
    /// 的真实后果是——下次真堆积起来时他看不见了。忽略只对**指名道姓的那一条**有意义。
    #[must_use]
    pub fn ignorable(self) -> bool {
        !self.is_backlog()
    }

    /// 报告里的排序权重（小者在前）。
    ///
    /// 顺序即「先看哪个」：**内容真伪**（矛盾 → 过时 → 失效引用）排最前，因为它们动摇的是
    /// 知识库的可信度；**网络完整性**（近重复 → 断链 → 孤儿 → 空页）次之；**流水线积压**
    /// 垫底——脉搏里它已经有主位（`PRD-16-027`），体检里只是补一眼。
    #[must_use]
    fn rank(self) -> u8 {
        match self {
            Self::Contradiction => 0,
            Self::Outdated => 1,
            Self::StaleCitation => 2,
            Self::NearDuplicate => 3,
            Self::BrokenLink => 4,
            // 与断链同属「网络完整性」：断链是一条链接落空，读不了的文件夹是一整片笔记
            // 从索引里缺席——排在断链之后、没标签之前。
            Self::UnreadableFolder => 5,
            // 排在孤儿之前：孤儿是"这一篇没连上"，而没标签是**整条链路的入口不亮**
            // （建议永不出现 → 主题页永不诞生 → 归入/交还/深度体检全部够不着）。
            Self::Untagged => 6,
            Self::Orphan => 7,
            Self::EmptyNote => 8,
            Self::DistillBacklog => 9,
            // 垫底：两道入库闸门里，"资料还没整理"是机器欠的活，"记录还没沉淀"是
            // 只有用户有资格做的判断（PRD-06-053），催得最轻。
            Self::SettleBacklog => 10,
        }
    }
}

/// 一条发现（= 报告里的一张建议卡）。
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// 稳定 id：`<kind>:<主体…>`。忽略清单按它对齐，故同一条发现在两次体检之间认得出来。
    pub id: String,
    pub kind: FindingKind,
    /// 涉及页（芯片）。**第一项是卡片「打开」的落点**。
    pub paths: Vec<String>,
    /// 「打开」时要落到的行（1-based）；`None` = 只打开笔记。
    pub line: Option<u32>,
    /// 被指认的原文（T1 的那条声明 / 断链的链接目标原文）。空 = 不适用。
    pub subject: String,
    /// 判定理由（**仅 T1，模型产出**——不是 UI 文案）。空 = 不适用。
    pub reason: String,
    /// 这条发现覆盖的对象数（积压卡的 N、断链卡的引用页数）。`0` = 不适用。
    pub count: u32,
}

impl Finding {
    /// 由种类与主体串构造 id（主体以 `|` 相连，与 [`Self::id`] 的契约同源）。
    #[must_use]
    pub fn make_id(kind: FindingKind, subjects: &[&str]) -> String {
        if subjects.is_empty() {
            return kind.key().to_owned();
        }
        format!("{}:{}", kind.key(), subjects.join("|"))
    }

    /// 一条声明的短指纹（T1 发现 id 的一段）。
    ///
    /// 用**声明原文**而不是行号：行号会随上下文增删漂移，于是同一条声明在下一次体检里
    /// 变成"另一条发现"，用户按过的 ✕ 随之失效；反过来，声明被改写就该重新判一次——
    /// 指纹变了正是这个意思。
    #[must_use]
    pub fn claim_fingerprint(claim: &str) -> String {
        ContentHash::of(claim.trim().as_bytes())
            .as_str()
            .chars()
            .take(CLAIM_FINGERPRINT_CHARS)
            .collect()
    }
}

/// 声明指纹取多少位：16 位十六进制 = 64 bit，撞车概率在一个库的量级上可忽略，
/// 而完整指纹只会让 id 变长、日志更难读。
const CLAIM_FINGERPRINT_CHARS: usize = 16;

/// 体检的**权威状态**（随库存 `.luna/checkup_state.json`）。
///
/// 两个时刻分开记，因为它们回答两个不同的问题：`last_run_ms` = 「库上一次被完整看过是
/// 什么时候」（报告里如实呈现），`last_reminded_ms` = 「上一次为此打扰过用户是什么时候」。
/// 只记前者的话，周期提醒会在到期后的**每一次启动**都响一遍——用户不跑体检就天天被
/// 念叨，而那正是"提醒"变成"骚扰"的机制。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckupState {
    /// 上一次**完整跑完**的深度体检时刻（毫秒）；`0` = 从未跑过。
    #[serde(default)]
    pub last_run_ms: i64,
    /// 上一次发出周期提醒的时刻（毫秒）；`0` = 从未提醒过。
    #[serde(default)]
    pub last_reminded_ms: i64,
}

/// 一篇笔记在体检眼里的样子（全部来自派生索引，无一项需要读文件）。
#[derive(Clone, Debug)]
pub struct CheckupNote {
    pub path: String,
    pub title: String,
    /// 类型 kebab 键（[`NoteType::key`]）。
    pub note_type: String,
    /// 是不是主题页（`luna.topic`）。
    pub is_topic: bool,
    /// 是不是知识地图（`luna.knowledge_map`）。
    pub is_knowledge_map: bool,
    /// 正文字符数（trim 后）。
    pub body_chars: u32,
    /// 文件 mtime（毫秒）。
    pub modified_ms: i64,
    /// 是不是**记录态**（`notes.stage`，ADR-086）。
    pub draft: bool,
}

impl CheckupNote {
    /// 是不是**知识笔记**：资料区（录音/转写/对话）之外的都算。
    ///
    /// 体检的五类 T0 判据一律只看知识面——一篇还没转写的录音"没有出入链、正文为空"
    /// 是它的正常状态，报给用户只是噪声（ADR-024 的分层在这里同样成立）。
    #[must_use]
    pub fn is_knowledge(&self) -> bool {
        NoteType::from_key(&self.note_type).is_some_and(|t| !t.is_source_material())
    }

    /// 这一篇**本该被自动打标**吗（[`NoteStage::allows_auto_ai_tags`]：`note` 且非记录态）。
    ///
    /// 判据与打标那条链路同源（ADR-024 白名单 × ADR-086 收窄）——速记、资料区与记录态
    /// 都不打标，把它们数进「还没有标签」只会制造一堆用户无须处置的待办。
    /// 这里**不重写规则**，只是把索引行的两轴事实交给同一个判据函数。
    /// **知识地图同样豁免**（PRD-16-023 修订）：它是**导航页**不是内容——落盘路径
    /// 本就跳过打标（`origin == "knowledge_map"`），全库重扫与本盘点若不同步豁免，
    /// 就是「重扫给它打上、缺标签把它数进去」两头各自出错（真机第五轮：地图挂着
    /// `#cutover-迁移` 混进标签页的「共 6 篇」）。
    #[must_use]
    pub fn allows_auto_tags(&self) -> bool {
        !self.is_knowledge_map
            && NoteType::from_key(&self.note_type).is_some_and(|note_type| {
                NoteStage::from_index_row(note_type, self.draft).allows_auto_ai_tags(note_type)
            })
    }

    /// 这一篇是不是**记录态**（PRD-06-055 待沉淀积压的判据）。
    ///
    /// 走 [`NoteStage::from_index_row`] 而不是直接读 `self.draft`：来源轴优先——
    /// 资料类型上的 stage 列恒 NULL，但读脏行时那个布尔仍可能是 true，而"一篇录音待沉淀"
    /// 是说不通的（三态互斥）。判据只有一处，这里不重写它。
    #[must_use]
    pub fn is_draft(&self) -> bool {
        NoteType::from_key(&self.note_type)
            .is_some_and(|note_type| NoteStage::from_index_row(note_type, self.draft).is_draft())
    }
}

/// T0 的全部输入（`Vault` 一次从索引里查齐）。
#[derive(Clone, Debug, Default)]
pub struct CheckupRows {
    pub notes: Vec<CheckupNote>,
    /// 链接边 (源笔记路径, 链接目标原文)。
    pub links: Vec<(String, String)>,
    /// 标签边 (笔记路径, 标签)。
    pub tags: Vec<(String, String)>,
    /// (笔记路径, 别名)：按别名写的链接不算断链（对标方案 C2）。
    pub aliases: Vec<(String, String)>,
    /// 待整理的资料篇数（有正文、还没有整理笔记）。
    pub distill_backlog: u32,
    /// AI 关系标签的自动生成总开关（设置 `vault.graph_ai_tags`）是否开着。
    ///
    /// 关着时**不报「还没有标签」**：那是用户明确不要这个能力，报它就是唠叨。
    /// 设置读不到 Vault 层，故由调用方显式传进来——比在这里假设一个默认值诚实。
    pub ai_tags_enabled: bool,
    /// 现在（毫秒）。**待沉淀积压的陈化判据要它**（PRD-06-055：最旧一篇 ≥14 天没动过）。
    ///
    /// 由调用方传入而不是在这里读时钟：本模块是**纯判定内核**（零 IO、可无头单测），
    /// 而"现在几点"是环境。缺省 `0` 让所有笔记看起来来自未来，于是不报——
    /// 忘了传的后果是**少报一条**，不是凭空催用户去沉淀。
    pub now_ms: i64,
}

/// 积压卡上摆几个芯片（其余只报数字——摆十几个路径芯片会把卡片撑成一张清单）。
const BACKLOG_CHIPS: usize = 3;

/// 待沉淀积压的**数量**门槛（PRD-06-055 假设：≥5 篇）。
///
/// 常量起步、不入设置：真机反馈之前，一个没人知道该调成几的旋钮只是多一处要解释的东西。
const SETTLE_BACKLOG_MIN_NOTES: usize = 5;

/// 待沉淀积压的**陈化**门槛（PRD-06-055 假设：最旧一篇 ≥14 天没动过）。
///
/// **数量与陈化是「且」**：记录的价值恰在低摩擦采集，刚记完一批就被催沉淀会把这条
/// 路的成本抬回去。提醒只该对着确实搁置的草稿说话。
pub const SETTLE_BACKLOG_STALE_DAYS: i64 = 14;

/// 陈化门槛换算成毫秒。
///
/// **公开**是因为记录收件箱的「时龄警示色」用的必须是**同一个**门槛（PRD-06-058）：
/// 软硬两级提醒（行上的轻警示 / 体检里的积压行）说的是同一件事，两个数字各写一遍，
/// 用户就会看到一篇行上已泛黄、体检却一个字不提的笔记，而那只会让两处都失去可信度。
pub const SETTLE_BACKLOG_STALE_MS: i64 = SETTLE_BACKLOG_STALE_DAYS * 24 * 60 * 60 * 1000;

/// 一篇笔记在「连通度」这一维上的处境。
///
/// **判据的唯一实现**：体检的孤儿页/空页那一趟与概览的「多少篇还没有任何关系」
/// （PRD-07-024a ①）共用它。规格要求那两个数字
/// **同源同数**——各写一遍必然在某次修订后对不上，而两个都自称是同一件事的数字对不上，
/// 比只有一个数字更糟。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteConnectivity {
    /// 有正文，却既没有链接也没有标签——它不在知识网络里。
    Orphan,
    /// 正文为空。**优先于孤儿**：空页当然也没有关系，但该对用户说的是「它是空的」
    /// ——让他去补内容，而不是让他去建链接。两者互斥不是排版取舍，是下一步做什么不同。
    Empty,
    /// 至少有一条边。
    Connected,
    /// 不参与这一维：资料区笔记本就不进知识网络（ADR-024）。
    Skipped,
}

/// 连通度判定（纯函数，零 IO）。`degree` 由 [`connectivity_graph`] 给出。
#[must_use]
pub fn classify_connectivity(note: &CheckupNote, degree: u32) -> NoteConnectivity {
    if !note.is_knowledge() {
        return NoteConnectivity::Skipped;
    }
    if note.body_chars == 0 {
        NoteConnectivity::Empty
    } else if degree == 0 {
        NoteConnectivity::Orphan
    } else {
        NoteConnectivity::Connected
    }
}

/// 从体检行构图——连通度的两个消费者（体检发现、概览计数）共用同一张图，
/// 免得一处按全图度数、另一处按别的口径。
fn connectivity_graph(rows: &CheckupRows) -> crate::graph::VaultGraph {
    build_graph(&GraphRows {
        notes: rows
            .notes
            .iter()
            .map(|n| crate::graph::GraphNoteRow {
                path: n.path.clone(),
                title: n.title.clone(),
                note_type: n.note_type.clone(),
                draft: n.draft,
            })
            .collect(),
        links: rows.links.clone(),
        tags: rows.tags.clone(),
        aliases: rows.aliases.clone(),
    })
}

/// 「多少篇知识笔记还没有任何关系」（PRD-07-024a ①）。
///
/// 与体检的孤儿页**同源同数**——两者走同一个 [`classify_connectivity`]、同一张
/// [`connectivity_graph`]。空页不计入：它有自己的那一档，且用户的下一步不同。
#[must_use]
pub fn unlinked_count(rows: &CheckupRows) -> u32 {
    let graph = connectivity_graph(rows);
    let by_path: std::collections::HashMap<&str, &CheckupNote> =
        rows.notes.iter().map(|n| (n.path.as_str(), n)).collect();
    let n = graph
        .nodes
        .iter()
        .filter(|node| node.kind == GraphNodeKind::Note)
        .filter_map(|node| {
            by_path
                .get(node.id.as_str())
                .map(|note| (note, node.degree))
        })
        .filter(|(note, degree)| classify_connectivity(note, *degree) == NoteConnectivity::Orphan)
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 「有文件夹读不了」（V1b）：每个读不了的子目录一张卡，芯片就是那个文件夹（卡上的动作是
/// 在访达里显示它——改权限是用户的事）。按路径成 id，用户 ✕ 过的那一个下次认得出来。
#[must_use]
pub fn unreadable_folder_findings(dirs: &[String]) -> Vec<Finding> {
    dirs.iter()
        .map(|dir| Finding {
            id: Finding::make_id(FindingKind::UnreadableFolder, &[dir]),
            kind: FindingKind::UnreadableFolder,
            paths: vec![dir.clone()],
            line: None,
            subject: String::new(),
            reason: String::new(),
            count: 0,
        })
        .collect()
}

/// T0 即时体检（`PRD-16-030`）：纯函数，输入是派生索引的几张表，输出是建议卡列表。
///
/// 五类判据：
/// - **孤儿页**：知识笔记，且在图谱里**度数为 0**。度数含**标签边**——判据与图谱视图
///   同源（用的就是同一个 [`build_graph`]），否则会出现"体检说这篇是孤儿、图谱上它却
///   连着两根线"这种两套事实。有标签的笔记本就能被标签树、主题页建议、`@` 定范围找到，
///   报成孤儿是噪声。
/// - **断链**：图谱的幽灵节点（链接指向的笔记不存在）。一个目标一张卡，卡上列出引用它的页。
/// - **空页**：知识笔记且正文为空。**与孤儿互斥**（空页恒无链接，两张卡说同一件事只是重复）。
/// - **失效引用**：**主题页**引用的源笔记，其 mtime 晚于主题页自身的 mtime——即"这条
///   声明写下之后，源又改过"。只看主题页：普通笔记之间的链接是用户自己维护的关系，
///   "我链了 B 然后又编辑了 B"是日常，不是问题；而主题页上的每一条声明都是 **AI 代写、
///   带源链接的断言**（ADR-026 ②a），源变了就值得回头看一眼。这也正是 T1 矛盾检测的
///   候选集——两档用的是同一个判据，不各挑一套。
/// - **积压**：待整理 / 待归入，两张卡（动作完全不同：一个去批量确认卡，一个去那篇笔记）。
#[must_use]
pub fn t0_findings(rows: &CheckupRows) -> Vec<Finding> {
    let graph = connectivity_graph(rows);
    let by_path: std::collections::HashMap<&str, &CheckupNote> =
        rows.notes.iter().map(|n| (n.path.as_str(), n)).collect();
    let mut out: Vec<Finding> = Vec::new();

    // ── 孤儿页 / 空页（同一趟节点遍历，互斥）────────────────────────────────
    for node in graph.nodes.iter().filter(|n| n.kind == GraphNodeKind::Note) {
        let Some(note) = by_path.get(node.id.as_str()) else {
            continue;
        };
        let make = |kind: FindingKind| Finding {
            id: Finding::make_id(kind, &[&note.path]),
            kind,
            paths: vec![note.path.clone()],
            line: None,
            subject: String::new(),
            reason: String::new(),
            count: 0,
        };
        match classify_connectivity(note, node.degree) {
            NoteConnectivity::Empty => out.push(make(FindingKind::EmptyNote)),
            NoteConnectivity::Orphan => out.push(make(FindingKind::Orphan)),
            NoteConnectivity::Connected | NoteConnectivity::Skipped => {}
        }
    }

    // ── 还没有标签的知识笔记（D14：整条链路的入口不亮）────────────────────
    if rows.ai_tags_enabled {
        let tagged: std::collections::HashSet<&str> =
            rows.tags.iter().map(|(note, _)| note.as_str()).collect();
        let untagged: Vec<String> = rows
            .notes
            .iter()
            .filter(|n| n.allows_auto_tags() && n.body_chars > 0)
            .filter(|n| !tagged.contains(n.path.as_str()))
            .map(|n| n.path.clone())
            .collect();
        if !untagged.is_empty() {
            out.push(Finding {
                id: Finding::make_id(FindingKind::Untagged, &[]),
                kind: FindingKind::Untagged,
                count: untagged.len() as u32,
                paths: untagged.into_iter().take(BACKLOG_CHIPS).collect(),
                line: None,
                subject: String::new(),
                reason: String::new(),
            });
        }
    }

    // ── 断链（幽灵节点，一个目标一张卡）───────────────────────────────────
    for ghost in graph
        .nodes
        .iter()
        .filter(|n| n.kind == GraphNodeKind::Ghost)
    {
        let citing: Vec<String> = graph
            .edges
            .iter()
            .filter(|e| e.kind == GraphEdgeKind::Link && e.target == ghost.id)
            .map(|e| e.source.clone())
            .collect();
        if citing.is_empty() {
            continue;
        }
        out.push(Finding {
            id: Finding::make_id(FindingKind::BrokenLink, &[&ghost.id]),
            kind: FindingKind::BrokenLink,
            count: citing.len() as u32,
            paths: citing,
            line: None,
            subject: ghost.label.clone(),
            reason: String::new(),
        });
    }

    // ── 失效引用（主题页 → 源，源更晚被改过）──────────────────────────────
    for edge in graph.edges.iter().filter(|e| e.kind == GraphEdgeKind::Link) {
        let (Some(from), Some(to)) = (
            by_path.get(edge.source.as_str()),
            by_path.get(edge.target.as_str()),
        ) else {
            continue; // 目标是幽灵 → 已由断链卡覆盖
        };
        if !from.is_topic || to.modified_ms <= from.modified_ms {
            continue;
        }
        out.push(Finding {
            id: Finding::make_id(FindingKind::StaleCitation, &[&from.path, &to.path]),
            kind: FindingKind::StaleCitation,
            paths: vec![from.path.clone(), to.path.clone()],
            line: None,
            subject: String::new(),
            reason: String::new(),
            count: 0,
        });
    }

    // ── 积压卡 ────────────────────────────────────────────────────────
    if rows.distill_backlog > 0 {
        out.push(Finding {
            id: Finding::make_id(FindingKind::DistillBacklog, &[]),
            kind: FindingKind::DistillBacklog,
            count: rows.distill_backlog,
            paths: Vec::new(),
            line: None,
            subject: String::new(),
            reason: String::new(),
        });
    }
    if let Some(finding) = settle_backlog(rows) {
        out.push(finding);
    }

    sort_findings(&mut out);
    out
}

/// 待沉淀积压（`PRD-06-055`）：**纯 Rust、零 LLM**——判据全在已经查出来的那张 notes 表上。
///
/// 双条件：记录态 ≥ [`SETTLE_BACKLOG_MIN_NOTES`] 篇，**且**其中最旧一篇已 ≥
/// [`SETTLE_BACKLOG_STALE_DAYS`] 天没动过。芯片按**最旧在前**取前几篇——用户要处置的
/// 正是那几篇，把最近刚记的摆在卡上等于指着一批他还在用的东西说"该收拾了"。
///
/// **全库没有记录态时一个字都不提**（PRD-06-055 验收）：`0 >= 5` 为假，天然不出卡；
/// 这条不靠调用方记得判断。
fn settle_backlog(rows: &CheckupRows) -> Option<Finding> {
    let mut drafts: Vec<&CheckupNote> = rows.notes.iter().filter(|n| n.is_draft()).collect();
    if drafts.len() < SETTLE_BACKLOG_MIN_NOTES {
        return None;
    }
    drafts.sort_by_key(|n| n.modified_ms);
    // 排序后第一篇就是最旧的那一篇；它没搁置到期，整批就都不算积压。
    let oldest = drafts.first()?.modified_ms;
    if rows.now_ms - oldest < SETTLE_BACKLOG_STALE_MS {
        return None;
    }
    Some(Finding {
        id: Finding::make_id(FindingKind::SettleBacklog, &[]),
        kind: FindingKind::SettleBacklog,
        count: drafts.len() as u32,
        paths: drafts
            .into_iter()
            .take(BACKLOG_CHIPS)
            .map(|n| n.path.clone())
            .collect(),
        line: None,
        subject: String::new(),
        reason: String::new(),
    })
}

/// 报告排序：种类权重优先，同种按涉及路径（确定性——同一个库两次体检的顺序必须一样）。
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        a.kind
            .rank()
            .cmp(&b.kind.rank())
            .then_with(|| a.paths.cmp(&b.paths))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// 深度判定**顶掉**同一对页上的失效引用卡（T0 + T1 合并呈现时的去重）。
///
/// 两者说的是同一件事的两个精度：失效引用只知道"源在这之后改过"（mtime），矛盾/过时
/// 是模型**读过内容**给的判断。同一对（主题页, 源）上两张卡并列，用户要读两遍才发现它们
/// 讲的是一处——而**报告里的重复是这类工具最快的失信方式**（"它是不是在凑数"）。
/// 深度判定更有信息量，故它在场时收起那张粗判卡。
#[must_use]
pub fn suppress_superseded(findings: Vec<Finding>) -> Vec<Finding> {
    let judged: std::collections::HashSet<(&str, &str)> = findings
        .iter()
        .filter(|f| matches!(f.kind, FindingKind::Contradiction | FindingKind::Outdated))
        .filter_map(|f| match (f.paths.first(), f.paths.get(1)) {
            (Some(topic), Some(source)) => Some((topic.as_str(), source.as_str())),
            _ => None,
        })
        .collect();
    if judged.is_empty() {
        return findings;
    }
    let superseded: std::collections::HashSet<String> = findings
        .iter()
        .filter(|f| f.kind == FindingKind::StaleCitation)
        .filter(|f| match (f.paths.first(), f.paths.get(1)) {
            (Some(topic), Some(source)) => judged.contains(&(topic.as_str(), source.as_str())),
            _ => false,
        })
        .map(|f| f.id.clone())
        .collect();
    findings
        .into_iter()
        .filter(|f| !superseded.contains(&f.id))
        .collect()
}

/// 剔掉用户按过 ✕ 的发现（`PRD-16-032` 忽略项持久化）。
#[must_use]
pub fn without_ignored(findings: Vec<Finding>, ignored: &[String]) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| !ignored.iter().any(|i| i == &f.id))
        .collect()
}

// ── T1 的候选：主题页上「带源链接的声明」 ────────────────────────────────────

/// 一条待核对的声明（主题页正文里的一行 + 它引用的源）。
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    /// 行号（1-based，相对**整篇文件**——与引用跳转、搜索命中同一口径）。
    pub line: u32,
    /// 声明原文（整行，去掉列表符号与首尾空白）。
    pub text: String,
    /// 被引用的源笔记路径。
    pub source: String,
}

/// 一行里的链接目标（`extract_links` 的逐行版；两者共用同一套语法识别）。
fn line_link_targets(line: &str) -> Vec<String> {
    crate::parse::extract_links(line)
}

/// 从一篇主题页里抽出「带源链接的声明」（纯函数）。
///
/// 判据刻意窄：**必须是列表项**（`- ` / `* ` / `1. `）**且**引用了库内另一篇笔记。
/// 归入写进去的每一条都是这个形状（ADR-026 ④ 的落笔形态），而正文段落、小标题、
/// 「来源」区的裸链接不是声明——把它们一起送去让模型判矛盾，只会烧算力换一堆
/// "无法判断"。
///
/// `line_offset` = 正文第一行在整篇文件里的行号（frontmatter 之后），由调用方给出：
/// 卡片要能跳到那一行，而 `body_of` 之后行号会差着 frontmatter 的高度。
#[must_use]
pub fn claims_of(
    topic_rel: &str,
    body: &str,
    line_offset: u32,
    index: &LinkIndex<'_>,
) -> Vec<Claim> {
    let mut out = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let trimmed = raw.trim_start();
        let text = match trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            Some(rest) => rest,
            None => match trimmed
                .split_once(". ")
                .filter(|(head, _)| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()))
            {
                Some((_, rest)) => rest,
                None => continue,
            },
        };
        let Some(source) = line_link_targets(raw)
            .iter()
            .filter_map(|dst| index.resolve(topic_rel, dst))
            .find(|path| *path != topic_rel)
            .map(str::to_owned)
        else {
            continue;
        };
        out.push(Claim {
            line: line_offset + i as u32,
            text: text.trim().to_owned(),
            source,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, kind: &str, chars: u32, modified: i64) -> CheckupNote {
        CheckupNote {
            path: path.to_owned(),
            title: luna_sync_core::links::title_of(path),
            note_type: kind.to_owned(),
            is_topic: false,
            is_knowledge_map: false,
            body_chars: chars,
            modified_ms: modified,
            draft: false,
        }
    }

    /// 打标白名单在 ADR-086 后是**两轴**判据：`note` 且非记录态。
    /// 反向验证：把 `allows_auto_tags` 改回只看类型，第二条断言即红。
    #[test]
    fn auto_tag_whitelist_excludes_drafts_and_source_material() {
        let mut knowledge = note("Notes/知识.md", "note", 100, 0);
        assert!(knowledge.allows_auto_tags(), "知识态普通笔记该打标");
        knowledge.draft = true;
        assert!(
            !knowledge.allows_auto_tags(),
            "记录态不打标（ADR-086 收窄）"
        );

        assert!(!note("Quick/速记.md", "quick-note", 100, 0).allows_auto_tags());
        assert!(!note("Rec/录音.md", "recording", 100, 0).allows_auto_tags());
        // 资料类型上的 stage 列恒 NULL；即便有人硬塞一个 true 也不改变结论（来源轴优先）。
        let mut material = note("Rec/录音.md", "recording", 100, 0);
        material.draft = true;
        assert!(!material.allows_auto_tags());

        // 知识地图是导航页不是内容（PRD-16-023 修订）：不打标，也不进「缺标签」——
        // 否则豁免打标之后它就成了一篇永远补不上的"缺标签笔记"。
        let mut map = note("Notes/知识地图.md", "note", 100, 0);
        map.is_knowledge_map = true;
        assert!(!map.allows_auto_tags());
    }

    #[test]
    fn finding_kinds_have_unique_stable_keys() {
        let keys: std::collections::BTreeSet<&str> =
            FindingKind::ALL.iter().map(|k| k.key()).collect();
        assert_eq!(keys.len(), FindingKind::ALL.len(), "kebab 键必须互不相同");
        for kind in FindingKind::ALL {
            assert_eq!(FindingKind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(FindingKind::from_key("nonsense"), None);
    }

    /// 孤儿判据必须与**图谱看到的度数**同源：有标签的笔记在图谱上连着标签节点，
    /// 报成"孤儿"就是体检与图谱各说一套。
    #[test]
    fn tagged_note_is_not_an_orphan_because_the_graph_draws_that_edge() {
        let rows = CheckupRows {
            notes: vec![
                note("Notes/孤零零.md", "note", 100, 0),
                note("Notes/有标签.md", "note", 100, 0),
                note("Recordings/录音.md", "recording", 0, 0),
            ],
            tags: vec![("Notes/有标签.md".into(), "工作".into())],
            ..CheckupRows::default()
        };
        let ids: Vec<String> = t0_findings(&rows)
            .into_iter()
            .filter(|f| f.kind == FindingKind::Orphan)
            .map(|f| f.paths[0].clone())
            .collect();
        assert_eq!(ids, vec!["Notes/孤零零.md"]);
    }

    /// 空页与孤儿互斥（空页必然无链接，两张卡说的是同一件事）；资料区一律不参与。
    #[test]
    fn empty_note_wins_over_orphan_and_source_material_is_out_of_scope() {
        let rows = CheckupRows {
            notes: vec![
                note("Notes/空的.md", "note", 0, 0),
                note("Recordings/还没转写.md", "recording", 0, 0),
                note("Chats/对话.md", "chat", 0, 0),
            ],
            ..CheckupRows::default()
        };
        let findings = t0_findings(&rows);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::EmptyNote);
        assert_eq!(findings[0].paths, vec!["Notes/空的.md"]);
    }

    /// **「还没有标签」必须说出来**（2026-07-30 真机连撞三轮的那件事）：主题页建议要靠
    /// 标签成簇，没有标签就永远没有建议——而在此之前界面上没有任何地方说出这件事，用户
    /// 只能得出"主题页入口不明确"的结论。
    #[test]
    fn untagged_knowledge_notes_explain_why_suggestions_never_appear() {
        let mut rows = CheckupRows {
            notes: vec![
                note("Notes/有标签.md", "note", 100, 0),
                note("Notes/没标签甲.md", "note", 100, 0),
                note("Notes/没标签乙.md", "note", 100, 0),
                // 速记不自动打标（ADR-024 白名单），资料区同理——数进来只是制造待办。
                note("QuickNotes/速记.md", "quick-note", 100, 0),
                note("Transcripts/转写.md", "transcript", 100, 0),
                // 空页无从提炼标签，它自有一张卡。
                note("Notes/空的.md", "note", 0, 0),
            ],
            tags: vec![("Notes/有标签.md".into(), "工作".into())],
            ai_tags_enabled: true,
            ..CheckupRows::default()
        };
        let untagged = t0_findings(&rows)
            .into_iter()
            .find(|f| f.kind == FindingKind::Untagged)
            .expect("该报「还没有标签」");
        assert_eq!(untagged.count, 2, "只数本该打标、且有正文的知识笔记");

        // 用户明确关掉了 AI 标签 → 一个字都不提（报它就是唠叨）。
        rows.ai_tags_enabled = false;
        assert!(
            t0_findings(&rows)
                .iter()
                .all(|f| f.kind != FindingKind::Untagged),
            "总开关关着时不该出现这条"
        );

        // 它与积压同性质：会随一次补齐自然消失 → 不给 ✕、也不计入启动概览那一行。
        assert!(!FindingKind::Untagged.ignorable());
        assert!(FindingKind::Untagged.is_backlog());
    }

    #[test]
    fn broken_link_groups_citing_pages_under_one_card() {
        let rows = CheckupRows {
            notes: vec![note("a.md", "note", 10, 0), note("b.md", "note", 10, 0)],
            links: vec![
                ("a.md".into(), "尚未创建".into()),
                ("b.md".into(), "尚未创建".into()),
                ("a.md".into(), "b".into()),
            ],
            ..CheckupRows::default()
        };
        let broken: Vec<Finding> = t0_findings(&rows)
            .into_iter()
            .filter(|f| f.kind == FindingKind::BrokenLink)
            .collect();
        assert_eq!(broken.len(), 1, "一个缺失目标一张卡：{broken:?}");
        assert_eq!(broken[0].subject, "尚未创建");
        assert_eq!(broken[0].count, 2);
        assert_eq!(broken[0].paths, vec!["a.md", "b.md"]);
    }

    /// 失效引用只认**主题页**的引用：普通笔记之间"我链了 B 然后又编辑了 B"是日常。
    #[test]
    fn stale_citation_only_fires_for_topic_pages() {
        let mut topic = note("Notes/主题.md", "note", 200, 1_000);
        topic.is_topic = true;
        let plain = note("Notes/普通.md", "note", 200, 1_000);
        let source = note("Notes/源.md", "note", 200, 2_000); // 之后又改过
        let rows = CheckupRows {
            notes: vec![topic, plain, source],
            links: vec![
                ("Notes/主题.md".into(), "源".into()),
                ("Notes/普通.md".into(), "源".into()),
            ],
            ..CheckupRows::default()
        };
        let stale: Vec<Vec<String>> = t0_findings(&rows)
            .into_iter()
            .filter(|f| f.kind == FindingKind::StaleCitation)
            .map(|f| f.paths)
            .collect();
        assert_eq!(stale, vec![vec!["Notes/主题.md", "Notes/源.md"]]);
    }

    /// 源比主题页**旧**（归入之后没人动过源）→ 什么都不该报。
    #[test]
    fn citation_is_fine_when_the_source_predates_the_claim() {
        let mut topic = note("Notes/主题.md", "note", 200, 5_000);
        topic.is_topic = true;
        let rows = CheckupRows {
            notes: vec![topic, note("Notes/源.md", "note", 200, 1_000)],
            links: vec![("Notes/主题.md".into(), "源".into())],
            ..CheckupRows::default()
        };
        assert!(t0_findings(&rows)
            .iter()
            .all(|f| f.kind != FindingKind::StaleCitation));
    }

    #[test]
    fn backlog_cards_carry_numbers_and_are_not_ignorable() {
        let rows = CheckupRows {
            notes: vec![note("a.md", "note", 10, 0)],
            links: vec![("a.md".into(), "a".into())], // 自环剔除后 a 仍是孤儿
            distill_backlog: 7,
            ..CheckupRows::default()
        };
        let findings = t0_findings(&rows);
        let distill = findings
            .iter()
            .find(|f| f.kind == FindingKind::DistillBacklog)
            .expect("待整理卡");
        assert_eq!(distill.count, 7);
        // 积压卡不给 ✕：它会自己消失，永久静音只会让下次真堆积时看不见。
        assert!(!FindingKind::DistillBacklog.ignorable());
        assert!(FindingKind::Orphan.ignorable());
    }

    /// 待沉淀积压（PRD-06-055）：**数量 × 陈化两条都满足**才出卡。
    ///
    /// 反向验证：把 `&&` 改成 `||`，第二与第三条断言各会红一条。
    #[test]
    fn settle_backlog_needs_both_a_pile_and_an_aged_oldest_note() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let now = 100 * DAY;
        let draft = |path: &str, modified: i64| {
            let mut n = note(path, "note", 100, modified);
            n.draft = true;
            n
        };
        // 5 篇、最旧一篇 20 天没动 → 出卡，数字给全、芯片最旧在前。
        let rows = CheckupRows {
            notes: (0..5)
                .map(|i| draft(&format!("Notes/草稿{i}.md"), now - (20 - i) * DAY))
                .collect(),
            now_ms: now,
            ..CheckupRows::default()
        };
        let card = t0_findings(&rows)
            .into_iter()
            .find(|f| f.kind == FindingKind::SettleBacklog)
            .expect("该报待沉淀积压");
        assert_eq!(card.count, 5);
        assert_eq!(
            card.paths,
            vec!["Notes/草稿0.md", "Notes/草稿1.md", "Notes/草稿2.md"],
            "芯片按最旧在前——用户要处置的正是那几篇"
        );
        // 与「待整理资料」同性质：不给 ✕、不计入启动概览。
        assert!(!FindingKind::SettleBacklog.ignorable());
        assert!(FindingKind::SettleBacklog.is_backlog());

        // 数量不够（4 篇，同样很旧）→ 不提。
        let few = CheckupRows {
            notes: (0..4)
                .map(|i| draft(&format!("Notes/草稿{i}.md"), now - 40 * DAY))
                .collect(),
            now_ms: now,
            ..CheckupRows::default()
        };
        assert!(t0_findings(&few)
            .iter()
            .all(|f| f.kind != FindingKind::SettleBacklog));

        // 够多但都是这两天记的 → 不提（低摩擦采集才是记录的价值所在）。
        let fresh = CheckupRows {
            notes: (0..8)
                .map(|i| draft(&format!("Notes/草稿{i}.md"), now - i * DAY))
                .collect(),
            now_ms: now,
            ..CheckupRows::default()
        };
        assert!(t0_findings(&fresh)
            .iter()
            .all(|f| f.kind != FindingKind::SettleBacklog));
    }

    /// **全库无记录态时一个字都不提**（PRD-06-055 验收第一条），且资料笔记绝不被
    /// 数进来——来源轴优先，一篇"待沉淀的录音"是说不通的。
    #[test]
    fn settle_backlog_is_silent_without_drafts_and_never_counts_source_material() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let now = 100 * DAY;
        let mut rows = CheckupRows {
            notes: (0..9)
                .map(|i| note(&format!("Notes/知识{i}.md"), "note", 100, 0))
                .collect(),
            now_ms: now,
            ..CheckupRows::default()
        };
        assert!(t0_findings(&rows)
            .iter()
            .all(|f| f.kind != FindingKind::SettleBacklog));

        // 资料行上硬塞 draft=true（写入侧不会产生，读到脏行才会）：仍不计入。
        rows.notes = (0..9)
            .map(|i| {
                let mut n = note(&format!("Recordings/录音{i}.md"), "recording", 100, 0);
                n.draft = true;
                n
            })
            .collect();
        assert!(t0_findings(&rows)
            .iter()
            .all(|f| f.kind != FindingKind::SettleBacklog));
    }

    #[test]
    fn report_order_puts_truth_before_hygiene_before_backlog() {
        let mut topic = note("Notes/主题.md", "note", 200, 1_000);
        topic.is_topic = true;
        let rows = CheckupRows {
            notes: vec![
                topic,
                note("Notes/源.md", "note", 200, 2_000),
                note("Notes/孤儿.md", "note", 200, 0),
                note("Notes/空.md", "note", 0, 0),
            ],
            links: vec![
                ("Notes/主题.md".into(), "源".into()),
                ("Notes/主题.md".into(), "没这一篇".into()),
            ],
            distill_backlog: 2,
            ..CheckupRows::default()
        };
        let kinds: Vec<FindingKind> = t0_findings(&rows).iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            vec![
                FindingKind::StaleCitation,
                FindingKind::BrokenLink,
                FindingKind::Orphan,
                FindingKind::EmptyNote,
                FindingKind::DistillBacklog,
            ]
        );
    }

    #[test]
    fn ignored_findings_are_filtered_by_stable_id() {
        let rows = CheckupRows {
            notes: vec![note("Notes/孤儿.md", "note", 10, 0)],
            ..CheckupRows::default()
        };
        let findings = t0_findings(&rows);
        let id = findings[0].id.clone();
        assert_eq!(id, "orphan:Notes/孤儿.md");
        assert!(without_ignored(findings, &[id]).is_empty());
    }

    /// 同一对页上「模型判过的」顶掉「按 mtime 粗判的」：报告里的重复是这类工具最快的
    /// 失信方式（"它是不是在凑数"）。
    #[test]
    fn a_judged_pair_supersedes_its_stale_citation_card() {
        let stale = Finding {
            id: Finding::make_id(FindingKind::StaleCitation, &["主题.md", "源.md"]),
            kind: FindingKind::StaleCitation,
            paths: vec!["主题.md".into(), "源.md".into()],
            line: None,
            subject: String::new(),
            reason: String::new(),
            count: 0,
        };
        let other = Finding {
            id: Finding::make_id(FindingKind::StaleCitation, &["主题.md", "别的源.md"]),
            paths: vec!["主题.md".into(), "别的源.md".into()],
            ..stale.clone()
        };
        let judged = Finding {
            id: Finding::make_id(FindingKind::Contradiction, &["主题.md", "源.md", "abc"]),
            kind: FindingKind::Contradiction,
            paths: vec!["主题.md".into(), "源.md".into()],
            line: Some(12),
            subject: "限流策略已确认。".into(),
            reason: "源里写的是尚未确认。".into(),
            count: 0,
        };
        let kept: Vec<String> = suppress_superseded(vec![judged, stale, other])
            .into_iter()
            .map(|f| f.id)
            .collect();
        assert!(kept.iter().any(|id| id.starts_with("contradiction:")));
        assert!(
            !kept.iter().any(|id| id == "stale-citation:主题.md|源.md"),
            "模型判过的那一对，粗判卡收起：{kept:?}"
        );
        assert!(
            kept.iter()
                .any(|id| id == "stale-citation:主题.md|别的源.md"),
            "没判过的那一对照常留着：{kept:?}"
        );
    }

    #[test]
    fn claims_need_a_list_marker_and_a_resolvable_source_link() {
        let notes = [("Notes/源.md", "源"), ("Notes/主题.md", "主题")];
        let index = LinkIndex::build(notes);
        let body = "# 主题\n\n## 进展\n\n- 限流策略已确认。（[源](%E6%BA%90.md)）\n- 没有出处的一条。\n段落里提到 [源](源.md) 但不是列表项。\n";
        let claims = claims_of("Notes/主题.md", body, 5, &index);
        assert_eq!(claims.len(), 1, "{claims:?}");
        assert_eq!(claims[0].source, "Notes/源.md");
        assert!(claims[0].text.starts_with("限流策略已确认。"));
        // 行号 = 正文偏移 + 行内序号（正文第 5 行是那条声明，偏移 5 → 第 9 行）。
        assert_eq!(claims[0].line, 5 + 4);
    }

    /// 声明指纹认的是**原文**而不是行号：上面插一行不该让用户按过的 ✕ 失效。
    #[test]
    fn claim_fingerprint_follows_the_text_not_the_line() {
        let a = Finding::claim_fingerprint("限流策略已确认。");
        let b = Finding::claim_fingerprint("  限流策略已确认。  ");
        assert_eq!(a, b, "首尾空白不改变身份");
        assert_ne!(a, Finding::claim_fingerprint("限流策略尚未确认。"));
        assert_eq!(a.len(), CLAIM_FINGERPRINT_CHARS);
    }
}
