//! frontmatter 局部精确改写（TDD-06 §4.4，YAML 往返保护）。
//!
//! **硬契约**：Lunarca 只就地改写 frontmatter 中的 `luna:` 子块，对其余第三方键值、注释、
//! 空行、键顺序执行**单字节级逐字保留（round-trip preservation）**。全量 `serde_yaml`
//! 反序列化再序列化会破坏键序/丢注释/重排——绝不这样做。
//!
//! 读取（`read_luna`）走全量解析仅为提取 `luna` 子树，且**读不写盘**，故安全；写入
//! （`update_luna`）走字节级定位拼接，只替换 `luna:` 块字节范围。
//!
//! `tags`（顶层，用户拥有）MVP 只读不写（§2.3），本模块不提供其写入路径。

use crate::error::{Result, VaultError};
use luna_sync_core::frontmatter::{
    find_frontmatter, has_unclosed_frontmatter, line_at, trim_cr, FmSpan,
};
use serde::{Deserialize, Serialize};

/// 当前 `luna:` 块结构版本（§4.5）。旧 `luna:` 块不会被读取或迁移。
pub const LUNA_SCHEMA: u32 = 1;

/// 功能视图归类（§5）。缺省 = [`NoteType::Note`]（互操作命门）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoteType {
    /// 普通笔记（无 `luna.type` 或无 `luna:` 块时的缺省）。
    #[default]
    Note,
    /// 语音速记（语音输入归档）。
    QuickNote,
    /// 录音。
    Recording,
    /// 媒体转写（导入音/视频的转写产物，PRD-05）。
    Transcript,
    /// 对话存档。
    Chat,
    /// 导入的文档（ADR-164，PRD-06-065）：Word、文本、表格等转成的资料笔记，原文件在附件目录。
    Document,
    /// 视频项目主页（创作空间，ADR-162 ②）：一期视频一个，阶段写在 `luna.project_stage`。
    VideoProject,
    /// 视频脚本（ADR-162 ②③）：按二级标题分段的普通 Markdown。
    Script,
    /// 灵感条目（ADR-162 ②，PRD-19-005）：开头、标题、缩略图构思等片段。
    Swipe,
}

impl NoteType {
    /// 全部取值（供枚举遍历，与 serde kebab 口径同源）。
    pub const ALL: [Self; 9] = [
        Self::Note,
        Self::QuickNote,
        Self::Recording,
        Self::Transcript,
        Self::Chat,
        Self::Document,
        Self::VideoProject,
        Self::Script,
        Self::Swipe,
    ];

    /// kebab 键——**唯一事实源**：frontmatter serde、派生索引列、`type:` 搜索过滤器共用。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::QuickNote => "quick-note",
            Self::Recording => "recording",
            Self::Transcript => "transcript",
            Self::Chat => "chat",
            Self::Document => "document",
            Self::VideoProject => "video-project",
            Self::Script => "script",
            Self::Swipe => "swipe",
        }
    }

    /// 由 kebab 键还原（大小写不敏感）；未知键 → `None`（不猜、不缺省）。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.key() == key)
    }

    /// **资料区**（ADR-024）：过程产物，服务溯源与回放而非知识网络——录音 / 媒体转写 /
    /// 对话存档 / 导入的文档（ADR-164 ②）。资料照常落盘、照常可搜（`type:` 精确检索），但在**知识面**默认退场：
    /// 图谱默认隐藏、搜索分组降权、快速切换器降权（口径见 PRD-06-043）。
    ///
    /// 速记**不在**其中：它是用户自己说的话（意图的直接表达），不是机器产物，故照常
    /// 参与知识面；它走的是「收件箱 → 升级为笔记」那条线（PRD-02-055）。
    #[must_use]
    pub fn is_source_material(self) -> bool {
        matches!(
            self,
            Self::Recording | Self::Transcript | Self::Chat | Self::Document
        )
    }
}

/// frontmatter `luna.stage` 的**唯一合法取值**（ADR-086）：记录态标记。
///
/// 全仓只有这一处写下这个字符串——写盘、派生索引列、判定三边共用它，
/// 由 `scripts/check-note-stage-single-source.py` 机器守护。
pub const STAGE_DRAFT: &str = "draft";

/// **记录层三态**（ADR-086 来源 × 成熟度双轴的判定结论）：资料 / 记录 / 知识，**互斥**。
///
/// 两根轴正交：来源轴（[`NoteType::is_source_material`]）答「这是谁写的」，成熟度轴
/// （frontmatter `luna.stage`）答「它定稿了没有」。二者合成的结论只有三种，且**来源轴
/// 优先**——资料笔记上的 `stage` 键被忽略（stage 对过程产物没有意义）。
///
/// **这个枚举是消费面唯一该读的东西**：搜索分组、图谱过滤、AI 打标白名单、问答降权、
/// 主题页建议计数全部只问 [`NoteStage`]，绝不各自去读 frontmatter、也绝不各写一遍缺省
/// 规则（TDD-06 §3/§5）。缺省判定只在 [`NoteStage::resolve`] 一处发生。
///
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoteStage {
    /// **资料**：机器产物（录音 / 媒体转写 / 对话存档），来源轴判定，恒非记录态。
    SourceMaterial,
    /// **记录**：过程性内容（`luna.stage: draft`，或缺省记录态的速记）。
    Draft,
    /// **知识**：已定稿、全量参与知识面。缺省即此档（互操作命门：外来 `.md` 无键即知识）。
    #[default]
    Knowledge,
}

impl NoteStage {
    /// **全仓唯一的成熟度判定点**（与 `type` 缺省同点，TDD-06 §5）。
    ///
    /// `raw` = frontmatter `luna.stage` 的原始取值（[`LunaMeta::stage_raw`]；`None` = 无该键）。
    /// 三条规则，一处写死：
    /// 1. **来源轴优先**：资料类型恒 [`Self::SourceMaterial`]，该键被忽略（三态互斥）；
    /// 2. **无键 = 缺省**：`quick-note` 缺省**记录态**（收件箱语义，PRD-02-055），
    ///    其余类型缺省**知识态**（零迁移的互操作命门，ADR-086 ①）；
    /// 3. **有键即记录态**：合法值目前仅 `draft`；未知取值同样按记录态处理。
    ///
    /// > 假设：未知取值按记录态（理由：TDD-06 §5 已裁决——知识面的误纳成本高于误排，
    /// > 把草稿当知识会污染问答与主题页合成，把知识暂当记录只是少一次自动待遇、
    /// > 树上一眼可见可改；与设置「脏值取最保守档」同一精神）。
    #[must_use]
    pub fn resolve(note_type: NoteType, raw: Option<&str>) -> Self {
        if note_type.is_source_material() {
            return Self::SourceMaterial;
        }
        match raw {
            None => match note_type {
                NoteType::QuickNote => Self::Draft,
                _ => Self::Knowledge,
            },
            // 合法值 `draft` 与任何未知取值同归记录态——见上面的假设。
            Some(_) => Self::Draft,
        }
    }

    /// 由**派生索引行**还原（类型列 + `notes.stage` 列非空与否）。
    ///
    /// 这里**不重跑缺省规则**：列里存的已经是 [`Self::resolve`] 的结论（写索引那一刻算的），
    /// 本函数只把「列 + 来源轴」拼回三态。缺省规则第二次出现的地方，就是它开始漂移的地方。
    #[must_use]
    pub fn from_index_row(note_type: NoteType, draft_column: bool) -> Self {
        if note_type.is_source_material() {
            return Self::SourceMaterial;
        }
        if draft_column {
            Self::Draft
        } else {
            Self::Knowledge
        }
    }

    /// 是不是**记录态**（树内淡显 + 徽标、图谱默认隐藏、搜索独立分组、问答降权的共同判据）。
    #[must_use]
    pub fn is_draft(self) -> bool {
        matches!(self, Self::Draft)
    }

    /// 是不是**知识态**（非资料**且**非记录：主题页建议计数、AI 打标白名单的判据）。
    #[must_use]
    pub fn is_knowledge(self) -> bool {
        matches!(self, Self::Knowledge)
    }

    /// 派生索引 `notes.stage` 列的值：记录态 = `Some("draft")`，资料与知识恒 `None`
    /// （TDD-06 §3——列只编码成熟度轴，资料由 `type` 列表达）。
    #[must_use]
    pub fn column(self) -> Option<&'static str> {
        self.is_draft().then_some(STAGE_DRAFT)
    }

    /// **AI 关系标签自动生成白名单**（ADR-024 的 `{note}` × ADR-086 收窄为「且非记录态」）。
    ///
    /// **为什么判据挂在这里而不是 [`NoteType`] 上**：白名单同时需要来源轴与成熟度轴，
    /// 而 `NoteType` 只知道前者——留在那里就只能让每个调用点自己再补一句「并且不是记录态」，
    /// 那正是「第二处散写」。故判据上移到**同时持有两轴结论**的这一层：调用方先经
    /// [`Self::resolve`]（或 [`Self::from_index_row`]）得到 stage，再把 type 交回来。
    ///
    /// 刻意比 [`NoteType::is_source_material`] 的补集更窄——速记既不是资料，也不该自动打标
    /// （碎片打标噪音大于价值）；视频项目主页是结构页（阶段 + 链接），打标只会把骨架词
    /// 标成主题。脚本与灵感条目是用户写的正文，照普通笔记待遇（灵感库靠标签归类，PRD-19-005）。与文件夹排除（`crate::graph::is_tag_excluded`，PRD-06-035c）
    /// **并集**生效：前者答「哪类内容值得打标」，后者答「这片区域不要打标」。
    /// 单篇手动入口对两者双双绕过（显式动作是明确意图）。
    ///
    #[must_use]
    pub fn allows_auto_ai_tags(self, note_type: NoteType) -> bool {
        self.is_knowledge()
            && matches!(
                note_type,
                NoteType::Note | NoteType::Script | NoteType::Swipe
            )
    }

    /// **「生成知识笔记」入口的白名单**（ADR-024 蒸馏 × ADR-086 ② 推广到记录）。
    ///
    /// 三态里**只有知识态不在其中**：资料经蒸馏、记录经沉淀，两条汇流的终点都是知识，
    /// 而对一篇已经是知识的笔记再蒸馏一次，产出的是它自己的摘要——那不是入库，是复制。
    ///
    /// **判据挂在这里而不是 [`NoteType`] 上**，理由与 [`Self::allows_auto_ai_tags`]
    /// 逐字相同：它同时需要两根轴，而 `NoteType` 只知道一根。蒸馏管线上有四道门
    /// （状态查询 / 入队前置 / worker 门槛 / 输入装配）都问这同一个问题——让每道门自己
    /// 写一遍 `is_source_material() || 记录态`，就是让四道门各自漂移。
    ///
    #[must_use]
    pub fn allows_distill(self) -> bool {
        matches!(self, Self::SourceMaterial | Self::Draft)
    }

    /// **「标记为记录」反向动作的白名单**（PRD-06-058）：只有**知识态**能被降回记录态。
    ///
    /// 资料态一律不给——`stage` 在资料笔记上被 [`Self::resolve`] 忽略（来源轴优先、三态
    /// 互斥），写下去只会得到一个谁都不读的键，而界面却已经答应了用户一件没发生的事。
    /// 记录态自己也不给：它已经在那儿了，重复动作应当是无操作而不是又一次写盘。
    ///
    /// **判据挂在这里而不是调用点**，理由与 [`Self::allows_distill`] 逐字相同：
    /// 「树行右键补一句 && 非资料」与「编辑器右键补一句 && 非资料」是两处会漂的写法，
    /// 而它们必须永远给出同一个答案（同一篇笔记在两个菜单里能做的事不能不一样）。
    ///
    /// 主题页 / 知识地图的排除**不在这里**：它们由 frontmatter 身份标记表达（`luna.topic`
    /// / `luna.knowledge_map`），与成熟度轴正交，判定归 [`crate::Vault::mark_as_draft`]。
    #[must_use]
    pub fn allows_mark_as_draft(self) -> bool {
        matches!(self, Self::Knowledge)
    }

    /// 由**派生索引的两位事实**还原成熟度（建树等只拿得到布尔集合的地方用）。
    ///
    /// 规则与 [`Self::resolve`] 逐字同义——**来源轴优先、三态互斥**：是资料就恒资料态
    /// （`stage` 键在它上面被忽略），否则看记录位，都不是即知识态。
    ///
    /// **它存在是为了让调用点不再自己拼**：建树处此前写着
    /// `is_markdown && !draft && !is_source_material` 当作「可标记为记录」，而同一个判据
    /// 在本类型上已经有一处（[`Self::allows_mark_as_draft`]）；那种手拼一旦与这里分叉，
    /// 同一篇笔记在树行右键与编辑器右键里能做的事就会不一样（本类型注释里已写死的纪律）。
    #[must_use]
    pub fn from_flags(is_source_material: bool, draft: bool) -> Self {
        if is_source_material {
            Self::SourceMaterial
        } else if draft {
            Self::Draft
        } else {
            Self::Knowledge
        }
    }
}

/// 一张贴在桌面上的便签的位置与外观（ADR-119，PRD-06-058 F）。
///
/// 它是速记的第二个呈现面，不是第二个对象。
///
/// **「钉着没有」是 [`Self::pinned`]，不是"这个键在不在"**（2026-09-09 真机修订）：
/// 初版把取消钉住实现成删掉整个键，于是位置、尺寸与颜色**一并没了**——用户换了颜色、
/// 摆好了位置，取消钉住再钉回来，全部回到默认。而"这张纸长什么样、贴在哪儿"是笔记的
/// 属性，与它此刻在不在桌面上是两件事（就像窗口位置不因为你关了窗就该被忘掉）。
/// 键仍然只有一个，笔记本身仍然一个字不动。
///
/// 坐标存 frontmatter 而不是设备侧配置，理由是"这张纸贴在桌面哪儿"与笔记同生共死：
/// 笔记删了坐标就该消失，笔记改名坐标跟着文件走。存设备配置会让它变成一张需要与 Vault
/// 对账的孤儿表。代价是坐标会随 Vault 同步到别的设备——由显示前的 clamp 兜住。
/// 导入文档的原文件（frontmatter `luna.original`，ADR-164）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalFile {
    /// 原文件在库里的相对路径（附件目录里那一份）。
    pub path: String,
    /// 原文件内容的 SHA-256（小写十六进制）：同一文件夹里再导入同一份时据此认出（④）。
    pub sha256: String,
    /// 导入时认定的格式键（`docx` / `csv` / …，`luna_docimport::DocFormat::key`）。
    pub format: String,
}

/// 网页剪藏的元数据（frontmatter `luna.clip`，ADR-164 ⑦）。都是网页自己给的，原样保存。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebClip {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesktopNote {
    /// 屏幕坐标（物理像素，左上角原点）；`None` = 还没摆过，由显示时按光标定位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// 窗口尺寸（逻辑像素）；`None` = 用默认尺寸。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    /// 便签配色键（`yellow` / `blue` / …，词表归呈现层）；`None` = 默认色。
    ///
    /// 存的是**键而不是色值**：主题换了、暗色模式下，同一张便签要跟着变，而写死的
    /// `#ffe9a8` 在暗色里是一块刺眼的白。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 此刻钉在桌面上吗（启动恢复与「速记」行上的钉住态都看它）。
    ///
    /// **缺省是 `true`，且为 `true` 时不写盘**：升级前落下的键没有这个字段，而它们
    /// 当时的语义正是"钉着"——缺省为 `false` 会让所有已钉住的便签在升级后集体消失。
    /// 为 `true` 时跳过序列化，于是老文件回写后仍然逐字节不变（零迁移，与本结构其余
    /// 字段同一纪律）。
    #[serde(default = "pinned_default", skip_serializing_if = "is_pinned")]
    pub pinned: bool,
}

/// `Default` 与 serde 缺省**必须同为 `true`**：`unwrap_or_default()` 在钉住路径上表示
/// "一张全新的便签"，而那一刻它当然是钉着的。两处答案不一样，就会出现"刚钉上就被判为
/// 没钉"。
impl Default for DesktopNote {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            w: None,
            h: None,
            color: None,
            pinned: true,
        }
    }
}

fn pinned_default() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_pinned(pinned: &bool) -> bool {
    *pinned
}

/// 一条媒体映射（§2）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    /// 媒体相对路径（相对笔记或 Vault 根，§1）。
    pub path: String,
    /// 时长（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// 是否已转写。
    #[serde(default, skip_serializing_if = "is_false")]
    pub transcript: bool,
    /// 是否已转译。
    #[serde(default, skip_serializing_if = "is_false")]
    pub translation: bool,
    /// 转写模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 转写语种（设置的 lang_hint；`None` = 自动检测。供回放/二次转写/导出提示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// 耐久边车相对路径（§2.1）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<String>,
    /// 分片在录音会话中的绝对起点（毫秒；仅多 part 录音时写入，§2.2）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_start_ms: Option<u64>,
    /// 分片在录音会话中的绝对止点（毫秒；仅多 part 录音时写入，§2.2）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_end_ms: Option<u64>,
    /// 轨结构标记（PRD-04-052 双轨）：`"mic_system"` = Lunarca 双轨立体声（L=麦克风/我，
    /// R=系统/对方，声道即轨）；缺省 = 普通单时间轴媒体。权威重转写据此按声道归属说话人，
    /// 并与用户导入的普通立体声文件区分（后者绝不按声道拆轨）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// `luna.stage` 的宽容反序列化：任何标量都收下，其余当作无键。
///
/// **绝不因为这一个键失败**：[`read_luna`] 一旦解析失败就整篇回落
/// [`LunaMeta::default`]，于是 `stage: 3`（或 `stage: [a]` 这种手滑）会连带把
/// `type` / `media` / `speakers` 一起丢掉——录音与笔记的关联被静默抹掉，而用户
/// 只是把一个词写错了。收下标量原文交给 [`NoteStage::resolve`] 判成记录态（最保守档），
/// 比"整块作废"诚实得多。
fn de_stage_raw<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let value = Option::<serde_yaml_ng::Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(serde_yaml_ng::Value::Null) => None,
        Some(serde_yaml_ng::Value::String(s)) => Some(s),
        Some(serde_yaml_ng::Value::Bool(b)) => Some(b.to_string()),
        Some(serde_yaml_ng::Value::Number(n)) => Some(n.to_string()),
        // 序列/映射不是取值而是结构错误——按无键处理（回落类型缺省），不臆造一个词。
        Some(_) => None,
    })
}

fn default_schema() -> u32 {
    LUNA_SCHEMA
}

/// `luna:` 子块（Lunarca 私有元数据，§2）。键序与渲染由本模块定义，不受 round-trip 约束。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LunaMeta {
    /// 块结构版本（§4.5）。
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// 功能视图归类（缺省 note）。
    #[serde(rename = "type", default)]
    pub note_type: NoteType,
    /// 成熟度轴的**原始取值**（frontmatter `luna.stage`，ADR-086）；`None` = 无该键。
    ///
    /// **存原始字符串而不是枚举**，两条理由缺一不可：
    /// ① 未知取值不得让整个 `luna:` 块反序列化失败——[`read_luna`] 失败即整篇回落
    ///    [`LunaMeta::default`]，一个手滑写错的 stage 会连带把 `type`/`media`/`speakers`
    ///    全部丢掉，而那是把用户的录音与笔记的关联静默抹掉；
    /// ② round-trip：写回时原样吐出用户写下的那个词，Lunarca 绝不把它「顺手改对」。
    ///
    /// **消费面不读这个字段**——语义判定的唯一入口是 [`LunaMeta::stage`]（→
    /// [`NoteStage::resolve`]）。缺省规则（`quick-note` 缺省记录态、其余缺省知识态）
    /// 只在那一处存在。
    ///
    /// **绝不写回缺省值**：`skip_serializing_if` 保证无键的笔记回写后仍然无键
    /// （ADR-086 ① 的零迁移承诺，与 `type` 的 lazy 注入同一纪律）。
    #[serde(
        rename = "stage",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_stage_raw"
    )]
    pub stage_raw: Option<String>,
    /// 创建时间（RFC3339 字符串，原样保留避免时区漂移）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// 语音速记 AI 提炼的落盘时间（RFC3339，PRD-06-058 C′）；`None` = 没提炼过。
    /// 它同时是幂等标记：有值即不再跑第二次。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refined_at: Option<String>,
    /// 速记采用的产物版本（`raw` / `refined` / `second_pass`，PRD-02-033）；`None` = 没表态。
    /// 只是标记：正文三节不因它改动，词表与解析归 `vault::AdoptedVersion`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted: Option<String>,
    /// 提炼出的日期线索（ISO `YYYY-MM-DD`，PRD-06-058 C′）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dates: Vec<String>,
    /// 视频项目所处阶段的**原始取值**（frontmatter `luna.project_stage`，ADR-162 ②）；
    /// `None` = 无该键。只对 [`NoteType::VideoProject`] 有意义。
    ///
    /// 与 [`Self::stage_raw`] 是两根不相干的轴：那个是成熟度（ADR-086），这个是一期视频
    /// 走到了选题还是剪辑。存原始字符串、宽容反序列化，理由与 `stage_raw` 相同——手改错
    /// 一个词不能让整个 `luna:` 块回落缺省。判定只经 [`crate::ProjectStage::from_key`]。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_stage_raw"
    )]
    pub project_stage: Option<String>,
    /// 视频项目要发到哪些渠道（frontmatter `luna.project_channels`，ADR-166）：有序的渠道 id，
    /// 决定发布清单有哪几步。存原始字符串，判定只经 [`crate::channel::resolve_channels`]；
    /// 空 = 默认渠道（YouTube 与 B 站）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_channels: Vec<String>,
    /// 视频项目的计划发布日期（frontmatter `luna.project_publish_date`，ISO `YYYY-MM-DD`，
    /// PRD-19-040）；`None` = 没排期。存原始字符串，判定只经
    /// [`crate::creator::resolve_publish_date`]——手改成认不出的写法时当作没排期，不丢别的键。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_publish_date: Option<String>,
    /// 视频项目已归档（frontmatter `luna.project_archived`，ADR-165 S6）：不在看板与侧栏的项目
    /// 清单里，侧栏「已归档」一节列出；文件一字不动、阶段不变。没写 = 没归档。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_archived: bool,
    /// 字数目标（笔记库对标轮 W6b）：写这一篇打算写到多少字；`None` = 没设。状态栏按编辑器
    /// 同一个字数显示「已写 / 目标」。随文件走（换设备、换库都在）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_goal: Option<u32>,
    /// 钉在桌面上的那张便签（ADR-119）；`None` = 没钉住。
    ///
    /// 无键的笔记回写后仍然无键（`skip_serializing_if`），与 `stage` 的零迁移同一纪律。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_note: Option<DesktopNote>,
    /// 媒体映射列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaRef>,
    /// 录音书签（会话绝对毫秒，升序；PRD-04-005 的「精确层」，正文书签行为「可读层」，
    /// 与 §2.1 行标记/边车的两层哲学同构）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<u64>,
    /// 说话人显示名映射（PRD-04-054 / TDD-03 §10.3）：speaker id（`self` / `remote` /
    /// `remote:N`）→ 用户改的显示名（如「张三」）。存笔记 frontmatter——笔记即文件，
    /// 改名是对 Vault 内 Markdown 的可追溯编辑；回放徽标与导出前缀共用此映射（DRY）。
    /// BTreeMap 保证 YAML 键序稳定（幂等回写无漂移）。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub speakers: std::collections::BTreeMap<String, String>,
    /// AI 生成的关系标签（PRD-06-035）：LLM 从正文提炼的主题标签，服务关系图谱/标签索引。
    /// 与用户顶层 `tags` 分离存放——顶层 `tags` 用户拥有、只读不写（PRD-06-032），AI 标签
    /// 归 Lunarca 自有 `luna:` 块；索引层把两者合并（`luna_sync_core::tags::extract_tags`），标签面板
    /// 与关系图谱同源受益。重新生成时整体替换本列表，绝不触碰用户标签。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_tags: Vec<String>,
    /// `ai_tags` 生成时的正文指纹（[`luna_sync_core::hashing::ContentHash`] 口径，对 `body_of` 字节）。
    /// 幂等门控：正文未变则不重复调用 LLM（保存/二次转写自动触发的去抖依据）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_tags_src: Option<String>,
    /// 内容来源地址（PRD-05-006「从链接导入」）：这篇转写稿的媒体是从哪个网页抓来的。
    ///
    /// **溯源是产品需求而非调试信息**——三个月后回看一篇「某会议分享」，用户要能一键回到
    /// 原视频核对上下文；只留一个由标题派生的文件名等于把来源丢掉了。故它落**笔记文件本身**
    /// （笔记即文件，与 `speakers` 同一层次），随笔记迁移、可被用户看见与编辑。
    ///
    /// 仅在用户显式从链接导入时写入（本地文件导入无此字段），故它同时是"这篇来自网络"的
    /// 唯一判据。隐私：这是用户自己粘进来的地址，与 Vault 内其他正文同等对待；日志侧仍
    /// 只出脱敏主机名（`luna_fetch::url::redact`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// 导入文档的原文件（ADR-164 ②④）：在库里的位置、内容指纹与格式；只有 `document` 有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<OriginalFile>,
    /// 网页剪藏的元数据（ADR-164 ⑦）：作者、发布时间、站点；来源地址在 [`Self::source_url`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<WebClip>,
    /// **（资料侧）** 本篇资料的蒸馏笔记路径（PRD-06-044 / ADR-024）。
    ///
    /// 与 [`Self::distill_source`] 互为反向指针。指针悬空（用户删除或移动了蒸馏笔记）
    /// 即状态回「未生成」——**绝不复活**旧文件：删除是显式意图。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_note: Option<String>,
    /// **（蒸馏笔记侧）** 出身：源资料笔记的相对路径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_source: Option<String>,
    /// **（蒸馏笔记侧）** 生成时**源**正文的指纹（`ContentHash` 口径，与 `ai_tags_src` 同源）。
    /// 与源笔记此刻的正文指纹不一致 = 「源已更新」，徽标据此提示可重新生成。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_src: Option<String>,
    /// **（蒸馏笔记侧）** 生成时**自身**正文的指纹。
    ///
    /// 这是「不静默覆盖」这条铁律的技术依据（ADR-024）：与此刻自身正文一致 = 仍是原样的
    /// 机器产物，自动档可安全重生成（写前照例留快照）；不一致 = **用户编辑过，它已是用户
    /// 资产**，自动路径一律止步，只亮「源已更新」由用户决定要不要重来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_body: Option<String>,
    /// **（蒸馏笔记侧）** 生成时所用的模板键（`meeting` / `talk` / `interview` / `chat`，
    /// [`luna_core::TemplateId`] 的 serde 契约，PRD-06-051）。
    ///
    /// **与 [`Self::distill_full_text`] 成对存在，且是「用户显式选择」的唯一事实源**：
    /// 不落盘的话，「源已更新」重生成 / 自动档 / 批量整理这三条路径都会按规则重新建议一个
    /// 模板，把用户手选的那个**静默换掉**。存量产物无此字段 → 回退建议规则。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_template: Option<String>,
    /// **（蒸馏笔记侧）** 生成时是否附了「整理稿」区块。
    ///
    /// 与 `distill_template` 同理必须落盘：只记模板不记这一项，重生成会让整理稿按模板
    /// 默认值回落（会议档为关）而**静默消失**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_full_text: Option<bool>,
    /// **（蒸馏笔记侧）** 生成时输出触顶了，且是**哪道闸**先到
    /// （[`luna_core::TruncatedBy`] 的 serde 键：`caller` / `upstream` / `absolute`）。
    /// `None` = 本次没触顶。
    ///
    /// **必须落盘，只在界面上标注是不够的**：这一篇是写进 Vault 的**普通 Markdown**——
    /// 重开笔记、导出、同步到另一台设备之后，它与一篇完整的知识笔记长得一模一样。
    /// 「模型的话没说完」这个事实只活在 DOM 上的话，用户三个月后回来读到的是一份
    /// 缺了结尾却自称完整的纪要，而没有任何地方说得清它缺在哪儿。正文尾注负责让**人**
    /// 看见，本字段负责让**机器**看见（[`crate::Vault::distill_state`] 的「不完整」态、
    /// 批量盘点的待办桶都读它）——两者缺一不可：只有尾注的话，盘点无从识别；只有本字段
    /// 的话，在 Obsidian 里读同一个文件的用户什么都看不到。
    ///
    /// 存归属而不是一个布尔：三种触顶能做的动作不同（见 [`luna_core::TruncatedBy`]），
    /// 用量出口要按归属分类。取值口径与 `stage_raw` / `distill_template` 一致——存字符串
    /// 而非强类型枚举，故用户手改出一个不认识的值时这一篇仍读得出来。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill_output_truncated: Option<String>,
    /// **（主题页侧）** 这是一篇跨源积累的**主题页**（PRD-16-015）。
    ///
    /// 刻意用 frontmatter 标记而**不新增 `NoteType` 变体**：第六个变体会波及资料/知识
    /// 分层、AI 打标白名单、文件树三态分区、搜索分组、图谱过滤全部判定面；主题页就是
    /// `type: note` 的普通知识笔记，于是天然享有知识面的一切既有待遇，无需在五处各写分支。
    #[serde(default, skip_serializing_if = "is_false")]
    pub topic: bool,
    /// **知识地图**身份（PRD-16-023）：普通 Markdown 笔记中的唯一库级路由页。
    /// 文件名不是身份，用户移动/重命名后仍可由该标记定位。
    #[serde(default, skip_serializing_if = "is_false")]
    pub knowledge_map: bool,
    /// **（主题页侧）** 出身标签：这一页是从哪个标签的笔记簇合成的（PRD-16-016）。
    ///
    /// 只服务「这个标签已经有主题页了，别再建议一次」这一个判断——**不是**来源清单
    /// （「哪些源喂养了它」由正文里的链接经 links 表派生，零新增数据结构）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_src: Option<String>,
    /// 声明级证据契约。保留原始 YAML 值，让损坏/未来版本的 knowledge 子树不会导致整篇
    /// `luna:` 反序列化回落缺省、进而让主题身份或媒体关联消失；写入前另行严格解码与校验。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<serde_yaml_ng::Value>,
    /// **（AI 维护页侧）** AI 上次写入时的正文指纹——**所有权判据**（ADR-026 ①）。
    ///
    /// 与此刻正文一致 = **AI 所有**（自动档可直接更新，写前照例留快照）；不一致 =
    /// **用户已接管**，此后一切更新恒走差异预览/建议卡。这是 `distill_body` 同一套判定的
    /// 第四个用户（前三为 `distill_src`/`distill_body`/`ai_tags_src`），**不写第二份**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synth_body: Option<String>,
    /// **（录音/转写侧）** 当前转写是**权威转写**：停录后的批量重转写或导入转写已完整写回
    /// （PRD-16-047）。录音时的实时预览、重转写失败或被跳过时保留的预览文本均为 `false`。
    ///
    /// 只由转写写回路径按调用方声明设置（[`crate::TranscriptAuthority`]）；人工校对、重新翻译、
    /// 说话人分离不改变它。缺省 `false`：旧文件不猜，重新转写一次即成为权威（ADR-046 不迁移）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub transcript_authoritative: bool,
}

impl Default for LunaMeta {
    fn default() -> Self {
        Self {
            schema: LUNA_SCHEMA,
            note_type: NoteType::Note,
            stage_raw: None,
            project_stage: None,
            project_channels: Vec::new(),
            project_publish_date: None,
            project_archived: false,
            created: None,
            refined_at: None,
            adopted: None,
            dates: Vec::new(),
            word_goal: None,
            desktop_note: None,
            media: Vec::new(),
            bookmarks: Vec::new(),
            speakers: std::collections::BTreeMap::new(),
            ai_tags: Vec::new(),
            ai_tags_src: None,
            source_url: None,
            original: None,
            clip: None,
            distill_note: None,
            distill_source: None,
            distill_src: None,
            distill_body: None,
            distill_template: None,
            distill_full_text: None,
            distill_output_truncated: None,
            topic: false,
            knowledge_map: false,
            topic_src: None,
            knowledge: None,
            synth_body: None,
            transcript_authoritative: false,
        }
    }
}

impl LunaMeta {
    /// **记录层三态**（ADR-086）：本篇是资料 / 记录 / 知识。
    ///
    /// 这是消费面读取成熟度事实的**唯一入口**——判定本体在 [`NoteStage::resolve`]，
    /// 本方法只是把这一篇的两轴事实（`type` + `stage`）交给它。
    #[must_use]
    pub fn stage(&self) -> NoteStage {
        NoteStage::resolve(self.note_type, self.stage_raw.as_deref())
    }
}

/// 在 frontmatter 内部 `[inner_start, inner_end)` 定位 `luna:` 块的字节范围。
///
/// 块 = `luna:` 行 + 其后所有缩进行（含中间空行），**尾部空行不并入**（保留用户排版）。
/// 返回 `(block_start, block_end)`；无 `luna:` 键返回 `None`。
fn find_luna_block(content: &str, span: &FmSpan) -> Option<(usize, usize)> {
    let mut cursor = span.inner_start;
    let mut luna_start = None;
    while cursor < span.inner_end {
        let (line, after) = line_at(content, cursor);
        let body = trim_cr(line);
        // 顶层键 `luna:`（列 0、无缩进）。
        if body == "luna:" || body.starts_with("luna:") && !body.starts_with(char::is_whitespace) {
            luna_start = Some(cursor);
            cursor = after;
            break;
        }
        cursor = after;
    }
    let start = luna_start?;

    // 自 luna: 行之后，吞掉缩进行；记录最后一条"缩进非空行"的结束位置作为块尾（排除尾随空行）。
    let mut last_content_end = cursor; // luna: 行的结束（块至少含 luna: 行本身）。
    while cursor < span.inner_end {
        let (line, after) = line_at(content, cursor);
        let body = trim_cr(line);
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if body.trim().is_empty() {
            // 空行：暂吞，不更新块尾（若后续还有缩进行会被下一轮补回）。
            cursor = after;
        } else if indented {
            cursor = after;
            last_content_end = cursor;
        } else {
            // 列 0 非空 = 下一个顶层键，块结束。
            break;
        }
    }
    Some((start, last_content_end))
}

/// 把 [`LunaMeta`] 渲染为带 `luna:` 顶层键的 YAML 块（2 空格缩进，末尾含换行）。
fn render_luna_block(meta: &LunaMeta) -> Result<String> {
    #[derive(Serialize)]
    struct Wrapper<'a> {
        luna: &'a LunaMeta,
    }
    serde_yaml_ng::to_string(&Wrapper { luna: meta }).map_err(|e| VaultError::Decode(e.to_string()))
}

/// 读取 `luna:` 元数据。旧 `luna:` 数据不被读取；缺省 / 无 frontmatter / 解析失败一律返回 [`LunaMeta::default`]
/// （type=note，§5 缺省命门，绝不因解析失败把笔记变成不可读）。**读不写盘**。
#[must_use]
pub fn read_luna(content: &str) -> LunaMeta {
    read_luna_checked(content).unwrap_or_default()
}

/// 写入路径使用的严格读取：坏 YAML 不能退化成“没有元数据”后被新内容覆盖。
/// 普通只读路径继续使用 `read_luna` 的宽容缺省，保证外部笔记可打开。
pub fn read_luna_checked(content: &str) -> Result<LunaMeta> {
    let Some(span) = find_frontmatter(content) else {
        let (first, _) = line_at(content, 0);
        return if trim_cr(first) == "---" {
            Err(VaultError::Decode("unclosed frontmatter".into()))
        } else {
            Ok(LunaMeta::default())
        };
    };
    let inner = &content[span.inner_start..span.inner_end];
    // 全量解析仅为提取 luna 子树；读路径不回写，故不破坏 round-trip。
    let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(inner)
        .map_err(|error| VaultError::Decode(error.to_string()))?;
    if !value.is_mapping() {
        return Err(VaultError::Decode("frontmatter is not a mapping".into()));
    }
    let Some(luna_val) = value.get("luna") else {
        return Ok(LunaMeta::default());
    };
    serde_yaml_ng::from_value::<LunaMeta>(luna_val.clone())
        .map_err(|error| VaultError::Decode(error.to_string()))
}

/// 局部改写：返回**只替换了 `luna:` 块字节**、其余逐字保留的新内容。
///
/// - frontmatter 存在且有 `luna:` 块 → 原位替换块字节。
/// - frontmatter 存在但无 `luna:` 块 → 在闭合 `---` 前插入块（不动既有键）。
/// - 无 frontmatter → 新建 `---\nluna:…\n---\n` 前缀（正文原样跟随）。
/// - 开头是 `---` 却没有闭合行 → 拒绝（[`has_unclosed_frontmatter`]），一个字节不动。
pub fn update_luna(content: &str, meta: &LunaMeta) -> Result<String> {
    let block = render_luna_block(meta)?;
    match find_frontmatter(content) {
        Some(span) => {
            if let Some((start, end)) = find_luna_block(content, &span) {
                let mut out = String::with_capacity(content.len() + block.len());
                out.push_str(&content[..start]);
                out.push_str(&block);
                out.push_str(&content[end..]);
                Ok(out)
            } else {
                // 在闭合 `---` 前插入；inner 结尾必为换行（或空 frontmatter），块自带末换行。
                let mut out = String::with_capacity(content.len() + block.len());
                out.push_str(&content[..span.inner_end]);
                out.push_str(&block);
                out.push_str(&content[span.inner_end..]);
                Ok(out)
            }
        }
        None if has_unclosed_frontmatter(content) => Err(VaultError::Decode(
            "frontmatter 没有闭合行，未做任何改动".into(),
        )),
        None => Ok(format!("---\n{block}---\n{content}")),
    }
}

/// 正文（frontmatter 之后；无 frontmatter 则全文）。供 FTS 入库与字数统计（§3）。
#[must_use]
pub fn body_of(content: &str) -> &str {
    match find_frontmatter(content) {
        Some(span) => &content[span.body_start..],
        None => content,
    }
}

/// 「候选正文」与「落盘后 `body_of`」共用的**唯一**指纹口径（TDD-06 §3.7.1）。
///
/// 写盘之前，生产者手上只有一段候选正文；写盘之后，[`body_of`] 拿到的是
/// frontmatter 之后的**全部**内容——两者差着 frontmatter 与正文之间那个分隔空行。
/// 各算各的，两侧就**永不相等**：知识生命周期的意图会当场被判成「正文已变」，整条
/// tag/embed/归入/地图链路一次都跑不起来；地图的「无变化即跳过」也永远不成立，于是
/// 每次去抖刷新都重写一遍文件、再由写入事件触发下一次刷新，形成自激循环。
///
/// 这个坑仓库里已经踩过两次（`distill_body` 与 `synth_body`，见
/// [`crate::Vault::write_distilled`] 的注释），故这一次收敛成一个函数：**两侧都只能
/// 经它**。归一化**只**去首尾换行——正文内部任何一个字节的改动仍必须让指纹变化，
/// 否则「正文已变即放弃陈旧意图」这条守卫就名存实亡。
#[must_use]
pub fn knowledge_body_hash(body: &str) -> String {
    luna_sync_core::hashing::ContentHash::of(body.trim_matches('\n').as_bytes()).to_string()
}

/// 正文第一行**在整篇文件里**的 0-based 行号（= frontmatter 占掉的行数）。
///
/// 一切"行号"对外都是相对整篇文件的（引用跳转、搜索命中、体检卡片跳行），而算行号的地方
/// 拿到的多半是 [`body_of`] 之后的正文——差着 frontmatter 这段高度。收敛成一个函数是因为
/// 它已有五个调用面（摘录锚点 / 锚点解析 / 未链接提及 / 体检声明…），各写一遍 `filter(=='\n')`
/// 必然有一处忘了 `+1`，而那种偏差表现为「跳过去差几行」，没有任何报错。
#[must_use]
pub fn body_base_line(content: &str) -> usize {
    let body = body_of(content);
    content[..content.len() - body.len()]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_luna_refuses_an_unclosed_frontmatter_but_still_creates_one_when_absent() {
        let meta = LunaMeta {
            ai_tags: vec!["项目".into()],
            ..LunaMeta::default()
        };
        assert!(update_luna("---\ntags: [a]\n---# 标题\n正文\n", &meta).is_err());
        assert!(update_luna("---\n正文里只有一条分隔线\n", &meta).is_err());
        let fresh = update_luna("# 标题\n正文\n", &meta).unwrap();
        assert!(fresh.starts_with("---\nluna:") && fresh.ends_with("---\n# 标题\n正文\n"));
        assert!(!has_unclosed_frontmatter("# 标题\n"));
        assert!(!has_unclosed_frontmatter("---\na: 1\n---\n"));
        assert!(has_unclosed_frontmatter("---\r\na: 1\r\n"));
    }

    #[test]
    fn body_base_line_counts_frontmatter_height() {
        assert_eq!(body_base_line("正文\n第二行\n"), 0);
        let with_fm = "---\nluna:\n  schema: 1\n---\n\n正文\n";
        assert_eq!(body_base_line(with_fm), 4, "正文前有 4 个换行");
        assert_eq!(body_of(with_fm).lines().next(), Some(""));
    }

    #[test]
    fn no_frontmatter_defaults_to_note() {
        let meta = read_luna("# 标题\n正文");
        assert_eq!(meta.note_type, NoteType::Note);
        assert_eq!(meta.schema, LUNA_SCHEMA);
    }

    #[test]
    fn obsidian_note_without_luna_block_is_note() {
        // 既有 Obsidian 笔记：有顶层 tags/自定义键，无 luna: 块 → 缺省 note。
        let content = "---\ntitle: 我的笔记\ntags:\n  - 工作\n  - 重要\n---\n\n正文";
        assert_eq!(read_luna(content).note_type, NoteType::Note);
    }

    #[test]
    fn reads_luna_type() {
        let content =
            "---\nluna:\n  schema: 1\n  type: recording\n  created: 2026-05-28T10:00:00+08:00\n---\n# 会议";
        let meta = read_luna(content);
        assert_eq!(meta.note_type, NoteType::Recording);
        assert_eq!(meta.created.as_deref(), Some("2026-05-28T10:00:00+08:00"));
    }

    #[test]
    fn round_trip_preserves_user_keys_comments_blanks() {
        // 真实样本：注释、自定义键、空行、顶层 tags、键顺序——更新 luna 块后须逐字不变。
        let content = "\
---
# 这是用户的注释，必须保留
title: 周会纪要
tags:
  - 工作
  - 重要

luna:
  schema: 1
  type: note

# 另一段注释
custom_field: 自定义值
nested:
  a: 1
  b: 2
---

# 周会纪要
正文内容";
        // 把 type 从 note 改成 recording。
        let mut meta = read_luna(content);
        meta.note_type = NoteType::Recording;
        let out = update_luna(content, &meta).unwrap();

        // 用户的注释、自定义键、嵌套、tags、正文逐字保留。
        assert!(out.contains("# 这是用户的注释，必须保留"));
        assert!(out.contains("title: 周会纪要"));
        assert!(out.contains("  - 工作\n  - 重要"));
        assert!(out.contains("# 另一段注释"));
        assert!(out.contains("custom_field: 自定义值"));
        assert!(out.contains("nested:\n  a: 1\n  b: 2"));
        assert!(out.contains("# 周会纪要\n正文内容"));
        // luna 块已更新。
        assert_eq!(read_luna(&out).note_type, NoteType::Recording);
        // luna: 之前的 tags 空行排版未被吞。
        assert!(out.contains("  - 重要\n\nluna:"));
    }

    #[test]
    fn insert_luna_block_when_absent_keeps_existing() {
        let content = "---\ntitle: 笔记\ntags: [a, b]\n---\n正文";
        let meta = LunaMeta {
            note_type: NoteType::QuickNote,
            media: vec![MediaRef {
                path: "Recordings/x.wav".into(),
                duration: Some(12.5),
                transcript: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = update_luna(content, &meta).unwrap();
        assert!(out.contains("title: 笔记"));
        assert!(out.contains("tags: [a, b]"));
        let back = read_luna(&out);
        assert_eq!(back.note_type, NoteType::QuickNote);
        assert_eq!(back.media.len(), 1);
        assert_eq!(back.media[0].path, "Recordings/x.wav");
        assert!(back.media[0].transcript);
    }

    #[test]
    fn create_frontmatter_when_none() {
        let content = "# 纯笔记\n没有 frontmatter";
        let meta = LunaMeta {
            note_type: NoteType::Chat,
            ..Default::default()
        };
        let out = update_luna(content, &meta).unwrap();
        assert!(out.starts_with("---\nluna:\n"));
        assert!(out.contains("# 纯笔记\n没有 frontmatter"));
        assert_eq!(read_luna(&out).note_type, NoteType::Chat);
    }

    #[test]
    fn body_of_strips_frontmatter() {
        let content = "---\nluna:\n  type: note\n---\n# 标题\n正文";
        assert_eq!(body_of(content), "# 标题\n正文");
        assert_eq!(body_of("无 frontmatter 正文"), "无 frontmatter 正文");
    }

    #[test]
    fn kebab_case_serialization() {
        let meta = LunaMeta {
            note_type: NoteType::QuickNote,
            ..Default::default()
        };
        let block = render_luna_block(&meta).unwrap();
        assert!(block.contains("type: quick-note"), "块内容：{block}");
    }

    // ── 成熟度轴（ADR-086）──────────────────────────────────────────────────

    fn with_stage(note_type: NoteType, stage: Option<&str>) -> LunaMeta {
        let stage_line = stage.map(|s| format!("\n  stage: {s}")).unwrap_or_default();
        let content = format!(
            "---\nluna:\n  schema: 1\n  type: {}{stage_line}\n---\n正文\n",
            note_type.key()
        );
        read_luna(&content)
    }

    /// 三态判定矩阵：五种类型 × 有键 / 无键 / 未知取值。这张表就是 ADR-086 ① 的全部
    /// 内容，改任何一格之前先去改那条裁决。
    #[test]
    fn stage_matrix_covers_every_type_and_key_state() {
        use NoteStage::{Draft, Knowledge, SourceMaterial};
        // (类型, 无键, 有键 `draft`, 未知取值)
        let matrix = [
            (NoteType::Note, Knowledge, Draft, Draft),
            // 类型缺省的**唯一例外**：速记无键即记录态（收件箱语义，PRD-02-055）。
            (NoteType::QuickNote, Draft, Draft, Draft),
            // 资料四种：来源轴优先，该键被忽略（三态互斥）。
            (
                NoteType::Recording,
                SourceMaterial,
                SourceMaterial,
                SourceMaterial,
            ),
            (
                NoteType::Transcript,
                SourceMaterial,
                SourceMaterial,
                SourceMaterial,
            ),
            (
                NoteType::Chat,
                SourceMaterial,
                SourceMaterial,
                SourceMaterial,
            ),
            (
                NoteType::Document,
                SourceMaterial,
                SourceMaterial,
                SourceMaterial,
            ),
            // 创作空间三种（ADR-162 ②）：用户写的正文，与普通笔记同一规则。
            (NoteType::VideoProject, Knowledge, Draft, Draft),
            (NoteType::Script, Knowledge, Draft, Draft),
            (NoteType::Swipe, Knowledge, Draft, Draft),
        ];
        // 矩阵必须覆盖每一种类型：新增类型而不补一行，这条用例应当变红，而不是照样全绿。
        assert_eq!(
            matrix.len(),
            NoteType::ALL.len(),
            "stage 矩阵漏了某个笔记类型"
        );
        for (note_type, absent, draft, unknown) in matrix {
            assert_eq!(
                with_stage(note_type, None).stage(),
                absent,
                "{note_type:?} 无键"
            );
            assert_eq!(
                with_stage(note_type, Some(STAGE_DRAFT)).stage(),
                draft,
                "{note_type:?} stage: draft"
            );
            assert_eq!(
                with_stage(note_type, Some("wip")).stage(),
                unknown,
                "{note_type:?} 未知取值必须按记录态（TDD-06 §5 假设）"
            );
        }
        // 无 luna: 块的外来 Obsidian 笔记 = 知识态（零迁移的互操作命门）。
        assert_eq!(
            read_luna("---\ntitle: 外来\n---\n正文").stage(),
            NoteStage::Knowledge
        );
        assert_eq!(read_luna("没有 frontmatter").stage(), NoteStage::Knowledge);
    }

    /// 判据 API 的三个消费面口径：记录态、知识态、打标白名单。
    #[test]
    fn stage_predicates_answer_the_three_consumer_questions() {
        assert!(NoteStage::Draft.is_draft());
        assert!(!NoteStage::Knowledge.is_draft() && !NoteStage::SourceMaterial.is_draft());
        assert!(NoteStage::Knowledge.is_knowledge());
        assert!(!NoteStage::Draft.is_knowledge() && !NoteStage::SourceMaterial.is_knowledge());
        // 派生索引列：只有记录态非 NULL。
        assert_eq!(NoteStage::Draft.column(), Some(STAGE_DRAFT));
        assert_eq!(NoteStage::Knowledge.column(), None);
        assert_eq!(NoteStage::SourceMaterial.column(), None);
        // 打标白名单收窄为「note 且非记录态」。
        assert!(NoteStage::Knowledge.allows_auto_ai_tags(NoteType::Note));
        assert!(!NoteStage::Draft.allows_auto_ai_tags(NoteType::Note));
        assert!(!NoteStage::Knowledge.allows_auto_ai_tags(NoteType::QuickNote));
        assert!(!NoteStage::SourceMaterial.allows_auto_ai_tags(NoteType::Chat));
        // 创作空间：脚本与灵感条目照普通笔记打标，项目主页（结构页）不打（ADR-162 ②）。
        assert!(NoteStage::Knowledge.allows_auto_ai_tags(NoteType::Script));
        assert!(NoteStage::Knowledge.allows_auto_ai_tags(NoteType::Swipe));
        assert!(!NoteStage::Knowledge.allows_auto_ai_tags(NoteType::VideoProject));
        assert!(!NoteStage::Draft.allows_auto_ai_tags(NoteType::Swipe));
        // 「生成知识笔记」白名单（PRD-06-054）：资料与记录都给，知识态不给——
        // 对一篇已是知识的笔记再蒸馏，产出的是它自己的摘要，那不是入库。
        assert!(NoteStage::SourceMaterial.allows_distill());
        assert!(NoteStage::Draft.allows_distill());
        assert!(!NoteStage::Knowledge.allows_distill());
        // 「标记为记录」反向动作（PRD-06-058）：只有知识态能降回记录态——它与
        // `allows_distill` 恰好互补，而那不是巧合：一篇笔记要么还没入库（可蒸馏/沉淀），
        // 要么已经入库（可被打回记录）。
        assert!(NoteStage::Knowledge.allows_mark_as_draft());
        assert!(!NoteStage::Draft.allows_mark_as_draft());
        assert!(!NoteStage::SourceMaterial.allows_mark_as_draft());
        // 索引行还原：列非空 = 记录态；资料类型恒非记录态。
        assert_eq!(
            NoteStage::from_index_row(NoteType::Note, true),
            NoteStage::Draft
        );
        assert_eq!(
            NoteStage::from_index_row(NoteType::Note, false),
            NoteStage::Knowledge
        );
        assert_eq!(
            NoteStage::from_index_row(NoteType::Recording, true),
            NoteStage::SourceMaterial
        );
    }

    /// **绝不写回缺省值**（ADR-086 ①：零迁移）：无 `stage` 键的笔记经任何一次
    /// `luna:` 块改写之后，文件里仍然没有这个键。反向验证：去掉
    /// `skip_serializing_if` 即红。
    #[test]
    fn round_trip_never_writes_back_the_default_stage() {
        for note_type in NoteType::ALL {
            let content = format!(
                "---\nluna:\n  schema: 1\n  type: {}\n---\n正文\n",
                note_type.key()
            );
            let mut meta = read_luna(&content);
            meta.ai_tags = vec!["标签".into()]; // 任意一次别的改写都会重渲染整个块。
            let out = update_luna(&content, &meta).unwrap();
            assert!(
                !out.contains("stage:"),
                "{note_type:?} 被补写了 stage 键：\n{out}"
            );
            assert_eq!(read_luna(&out).stage_raw, None);
        }
    }

    /// 显式写下的 `stage` 原样保留（含未知取值——Lunarca 不「顺手改对」用户的文件）。
    #[test]
    fn explicit_stage_survives_round_trip_verbatim() {
        for written in [STAGE_DRAFT, "wip"] {
            let content =
                format!("---\nluna:\n  schema: 1\n  type: note\n  stage: {written}\n---\n正文\n");
            let meta = read_luna(&content);
            assert_eq!(meta.stage_raw.as_deref(), Some(written));
            let out = update_luna(&content, &meta).unwrap();
            assert_eq!(read_luna(&out).stage_raw.as_deref(), Some(written));
            assert_eq!(read_luna(&out).stage(), NoteStage::Draft);
            // 幂等：再回写一次字节相同。
            assert_eq!(update_luna(&out, &read_luna(&out)).unwrap(), out);
        }
    }

    /// 「沉淀为知识」= 去掉这个键（PRD-06-053）：文件 diff 只少 `stage` 一行。
    #[test]
    fn dropping_the_key_is_all_that_promotion_takes() {
        let content = "---\nluna:\n  schema: 1\n  type: note\n  stage: draft\n---\n正文\n";
        let mut meta = read_luna(content);
        assert!(meta.stage().is_draft());
        meta.stage_raw = None;
        let out = update_luna(content, &meta).unwrap();
        assert!(!out.contains("stage:"));
        assert_eq!(read_luna(&out).stage(), NoteStage::Knowledge);
        assert!(out.contains("正文"));
    }

    /// 一个写坏的 `stage` 不得让整块 `luna:` 作废——那会把 `type`/`media` 一起丢掉
    /// （录音与笔记的关联被静默抹掉）。反向验证：把字段改成强类型枚举即红。
    #[test]
    fn a_malformed_stage_never_destroys_the_rest_of_the_block() {
        let content = "---\nluna:\n  schema: 1\n  type: recording\n  stage: 3\n  media:\n    - path: Recordings/x.wav\n---\n正文\n";
        let meta = read_luna(content);
        assert_eq!(meta.note_type, NoteType::Recording, "类型必须活下来");
        assert_eq!(meta.media.len(), 1, "媒体映射必须活下来");
        assert_eq!(meta.stage(), NoteStage::SourceMaterial);
        // 非标量（结构错误）按无键处理，同样不炸整块。
        let listy = "---\nluna:\n  schema: 1\n  type: note\n  stage:\n    - draft\n---\n正文\n";
        assert_eq!(read_luna(listy).note_type, NoteType::Note);
        assert_eq!(read_luna(listy).stage(), NoteStage::Knowledge);
    }

    #[test]
    fn idempotent_update() {
        // 用读出的 meta 原样回写两次，第二次应与第一次字节相同（稳定，无漂移）。
        let content = "---\nluna:\n  schema: 1\n  type: recording\n---\n正文";
        let meta = read_luna(content);
        let once = update_luna(content, &meta).unwrap();
        let twice = update_luna(&once, &read_luna(&once)).unwrap();
        assert_eq!(once, twice);
    }
}
