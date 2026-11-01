//! 创作空间的领域规则（ADR-162，PRD-19 §3.1）：视频项目的阶段、看板卡片与项目骨架。
//!
//! 项目 = 一个文件夹 + 一篇项目主页（`luna.type: video-project`）；阶段写在主页的
//! `luna.project_stage`。看板是按类型的查询投影（TDD-06 §5），不建第二份数据：卡片每次都从
//! 派生索引列出主页、再读各自的 frontmatter 得到阶段。

use crate::channel::resolve_channels;
use crate::error::{Result, VaultError};
use crate::frontmatter::NoteType;
use crate::template::TemplateVars;
use crate::vault::{NoteDoc, Vault};

/// 一期视频所处的阶段（PRD-19-003 的七列）。顺序即看板列序。
///
/// 与成熟度轴 `luna.stage`（ADR-086）无关：一篇项目主页可以是知识态，同时停在「剪辑」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectStage {
    /// 选题。
    Idea,
    /// 研究。
    Research,
    /// 撰稿。
    Writing,
    /// 录制。
    Recording,
    /// 剪辑。
    Editing,
    /// 发布。
    Publishing,
    /// 已发布。
    Published,
}

impl ProjectStage {
    /// 全部取值，顺序即看板列序。
    pub const ALL: [Self; 7] = [
        Self::Idea,
        Self::Research,
        Self::Writing,
        Self::Recording,
        Self::Editing,
        Self::Publishing,
        Self::Published,
    ];

    /// frontmatter 里的写法——**唯一事实源**。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Research => "research",
            Self::Writing => "writing",
            Self::Recording => "recording",
            Self::Editing => "editing",
            Self::Publishing => "publishing",
            Self::Published => "published",
        }
    }

    /// 由 frontmatter 写法还原（大小写不敏感）；未知写法 → `None`。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|stage| stage.key() == key)
    }

    /// **唯一的缺省判定点**：无键或写了个不认识的词，都落在第一列「选题」。
    ///
    /// 不认识的词**只在显示时**这样归位，文件里的原文不动——用户手改的那个词由他自己决定
    /// 要不要改回来；拖动卡片时才写入一个合法值。
    #[must_use]
    pub fn resolve(raw: Option<&str>) -> Self {
        raw.and_then(Self::from_key).unwrap_or(Self::Idea)
    }
}

/// 项目页头的「下一步」（ADR-165 ⑥）：每个阶段只有一个主动作。阶段仍只由用户改，这里只回答
/// 「停在这一步时最该做什么」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextAction {
    /// 选题：把项目主页里「选题」一句话写出来。
    WriteIdea,
    /// 研究：把访谈、参考资料导进项目。
    ImportMaterial,
    /// 撰稿：照着脚本念一遍（手动滚动的提词器）。
    OpenTeleprompter,
    /// 录制：提词器跟着说话走。
    StartFollow,
    /// 剪辑：把剪好的成片导进来转写。
    ImportFinalCut,
    /// 发布：做发布包。
    OpenPublish,
    /// 已发布：改写成博客 / 简报 / 推文串，挑出短视频片段（PRD-19-024/025）。
    Distribute,
}

/// 这个阶段的「下一步」：每个阶段恰好一个。
#[must_use]
pub fn next_action(stage: ProjectStage) -> Option<NextAction> {
    match stage {
        ProjectStage::Idea => Some(NextAction::WriteIdea),
        ProjectStage::Research => Some(NextAction::ImportMaterial),
        ProjectStage::Writing => Some(NextAction::OpenTeleprompter),
        ProjectStage::Recording => Some(NextAction::StartFollow),
        ProjectStage::Editing => Some(NextAction::ImportFinalCut),
        ProjectStage::Publishing => Some(NextAction::OpenPublish),
        ProjectStage::Published => Some(NextAction::Distribute),
    }
}

/// 计划发布日期在 frontmatter 里的写法（ISO 日期）。
pub const PUBLISH_DATE_FORMAT: &str = "%Y-%m-%d";

/// frontmatter 里的计划发布日期 → 日期；没写或认不出（手改坏了）为 `None`——当作没排期，
/// 不报错，与阶段词的宽容同理。**唯一判据。**
#[must_use]
pub fn resolve_publish_date(raw: Option<&str>) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(raw?.trim(), PUBLISH_DATE_FORMAT).ok()
}

/// 看板的列配置：用户排好的阶段顺序，各自显示与否（设置 `creator.board_stages` 归一化后
/// 的结果）。至少一列显示由设置归一化保证。
pub type BoardLayout = [(ProjectStage, bool)];

/// 看板上第一个显示的列（新建项目从这里开始）；全隐藏时退回「选题」。
#[must_use]
pub fn first_visible_stage(layout: &BoardLayout) -> ProjectStage {
    layout
        .iter()
        .find(|(_, visible)| *visible)
        .map_or(ProjectStage::Idea, |(stage, _)| *stage)
}

/// 一张卡片**显示**在哪一列。
///
/// 停在显示列上的项目就在它自己那列。停在隐藏列上的项目不能凭空消失：放进用户列序里排在它
/// 前面、最近的那个显示列；前面一个显示列都没有，就放第一个显示列。**只影响显示**——文件里
/// 的阶段不改写，把那一列重新显示出来，卡片就回去了。
#[must_use]
pub fn display_column(stage: ProjectStage, layout: &BoardLayout) -> ProjectStage {
    let Some(at) = layout.iter().position(|(s, _)| *s == stage) else {
        return first_visible_stage(layout);
    };
    if layout[at].1 {
        return stage;
    }
    layout[..at]
        .iter()
        .rev()
        .find(|(_, visible)| *visible)
        .map_or_else(|| first_visible_stage(layout), |(s, _)| *s)
}

/// 看板上的一张卡片：一个视频项目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectCard {
    /// 项目主页的库相对路径。
    pub path: String,
    /// 标题（= 主页文件名 stem）。
    pub title: String,
    /// 所处阶段（已经过 [`ProjectStage::resolve`]）。
    pub stage: ProjectStage,
    /// 主页修改时间（Unix 毫秒）。
    pub modified_ms: i64,
    /// 要发的渠道 id（已经过 [`crate::channel::resolve_channels`]）。
    pub channels: Vec<String>,
    /// 计划发布日期（已经过 [`resolve_publish_date`]）。
    pub publish_date: Option<chrono::NaiveDate>,
    /// 已归档（`luna.project_archived`）。
    pub archived: bool,
}

/// 发布包的一篇成片候选（PRD-19-018，ADR-162 ⑪）：项目文件夹里已转写的录音 / 转写笔记。
#[derive(Clone, Debug, PartialEq)]
pub struct PublishSource {
    /// 笔记的库相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 修改时间（Unix 毫秒）。
    pub modified_ms: i64,
    /// 各段媒体时长之和（秒）；元数据里没有时长为 `None`。
    pub duration_secs: Option<f64>,
    /// 转写语种（第一段媒体的 `lang`；自动检测的为 `None`）。
    pub source_lang: Option<String>,
    /// 是否已有译文。
    pub has_translation: bool,
}

/// 项目概览里的一类（ADR-162 ⑫，PRD-19-004）。顺序即页面上的顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverviewGroup {
    /// 脚本。
    Scripts,
    /// 素材与来源（ADR-167 ③）：「素材」文件夹里的、研究笔记链到的、导入的文档与剪藏。
    Sources,
    /// 成片（不在素材里的转写 / 录音）。
    Footage,
    /// 研究与笔记（其余各类笔记）。
    Research,
    /// 灵感。
    Swipes,
    /// 发布包（发布笔记）。
    Publish,
}

impl OverviewGroup {
    /// 全部取值，顺序即页面上的顺序。
    pub const ALL: [Self; 6] = [
        Self::Scripts,
        Self::Sources,
        Self::Footage,
        Self::Research,
        Self::Swipes,
        Self::Publish,
    ];

    fn of(note_type: &str) -> Self {
        match NoteType::from_key(note_type) {
            Some(NoteType::Script) => Self::Scripts,
            Some(NoteType::Transcript | NoteType::Recording) => Self::Footage,
            Some(NoteType::Swipe) => Self::Swipes,
            _ => Self::Research,
        }
    }
}

/// 一条来源是什么（ADR-167 ③）：界面据此给类型标与图标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// 音视频的转写稿、录音。
    Media,
    /// 导入的文档（Word、PDF、表格……）。
    Document,
    /// 网页剪藏（带来源地址的文档）。
    Web,
    /// 粘贴的文字、手写的资料（普通笔记）。
    Text,
    /// 其它类型的笔记（脚本、灵感……被研究笔记链到）。
    Note,
}

/// 一个项目的几处固定位置（文字随界面语言，由调用方从项目主页推出）。
#[derive(Clone, Copy, Debug)]
pub struct ProjectLayout<'a> {
    /// 发布笔记 `<项目>-发布.md`。
    pub publish_note: &'a str,
    /// 研究笔记 `<项目>-研究.md`。
    pub research_note: &'a str,
    /// 「素材」文件夹。
    pub material_dir: &'a str,
    /// 「发布」导出文件夹。
    pub export_dir: &'a str,
}

/// 项目概览里的一篇笔记：它是什么、跟项目什么关系。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewItem {
    pub path: String,
    pub title: String,
    pub modified_ms: i64,
    pub group: OverviewGroup,
    /// 来源的类型；不在「素材与来源」里为 `None`。
    pub kind: Option<SourceKind>,
    /// 研究笔记链到它（「从库里选」链进来的、导入后自动链上的）。
    pub linked_from_research: bool,
    /// 项目主页链到它。
    pub linked_from_home: bool,
    /// 它链到项目主页。
    pub links_to_home: bool,
    /// 它在项目文件夹（含子文件夹）里。
    pub in_folder: bool,
}

/// 一个项目的概览：与项目有关的笔记（同一类里新的在前）与已导出的发布文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectOverview {
    pub stage: ProjectStage,
    pub items: Vec<OverviewItem>,
    /// 「发布」文件夹里已导出的文件（库相对路径，按文件名排序；笔记不算）。
    pub exports: Vec<String>,
}

/// 项目骨架里除主页以外的一篇笔记（研究、脚本……）。文字由调用方按界面语言给出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectChild {
    /// 文件名 stem（不含 `.md`），落在项目文件夹里。
    pub stem: String,
    /// 笔记类型。
    pub note_type: NoteType,
    /// 初始正文。
    pub body: String,
}

/// 删除一个视频项目会删掉什么（[`Vault::project_deletion`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDeletion {
    /// 交给 [`Vault::delete`] 的库相对路径：整个文件夹一项，或主页与以项目命名的笔记。
    pub targets: Vec<String>,
    /// 是否删的是整个项目文件夹。
    pub whole_folder: bool,
    /// 会删掉几篇笔记（`.md`，含子文件夹）。
    pub notes: usize,
    /// 会一起删掉的其它文件（素材、附件、导出文件；只在删整个文件夹时计）。
    pub other_files: usize,
}

/// 数一个文件夹里（含子文件夹）的笔记与其它文件；隐藏文件不算。
fn count_files(dir: &std::path::Path) -> (usize, usize) {
    let mut notes = 0;
    let mut others = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                Ok(kind) if kind.is_file() => {
                    if entry.path().extension().is_some_and(|ext| ext == "md") {
                        notes += 1;
                    } else {
                        others += 1;
                    }
                }
                _ => {}
            }
        }
    }
    (notes, others)
}

/// 列看板时一页取多少（只决定分几次查索引，不截断结果）。
const PROJECT_PAGE: usize = 200;

impl Vault {
    /// 全部视频项目，按主页修改时间倒序（派生索引的顺序）。
    ///
    /// 读不到的主页（列出之后被删了、正在被外部改写）跳过，不让一张坏卡片拖垮整个看板。
    ///
    /// # Errors
    /// 派生索引查询失败时返回错误。
    pub fn video_projects(&self) -> Result<Vec<ProjectCard>> {
        let custom = self.custom_channels().unwrap_or_default();
        let mut cards = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (entries, next) =
                self.list_notes_of_type(NoteType::VideoProject, PROJECT_PAGE, cursor.as_deref())?;
            for entry in entries {
                let Ok(doc) = self.read(&entry.path) else {
                    continue;
                };
                // 索引与文件之间有一拍延迟：文件已被改成别的类型时，以文件为准。
                if doc.meta.note_type != NoteType::VideoProject {
                    continue;
                }
                cards.push(ProjectCard {
                    stage: ProjectStage::resolve(doc.meta.project_stage.as_deref()),
                    channels: resolve_channels(&doc.meta.project_channels, &custom),
                    publish_date: resolve_publish_date(doc.meta.project_publish_date.as_deref()),
                    archived: doc.meta.project_archived,
                    path: entry.path,
                    title: entry.title,
                    modified_ms: entry.modified_ms,
                });
            }
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(cards)
    }

    /// 一个项目的概览（ADR-162 ⑫，PRD-19-004）：主页链出 ∪ 链入主页 ∪ 位于项目文件夹 ∪ 研究笔记
    /// 链出，取并集后归类；只列出、不复制内容。「素材」文件夹里的、研究笔记链到的、导入的文档
    /// 与剪藏归「素材与来源」并标出类型（ADR-167 ③）；其余转写 / 录音是成片。几处位置的名字随
    /// 界面语言，由调用方经 `layout` 给出。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目、索引查询失败时返回错误。
    pub fn project_overview(
        &self,
        home_rel: &str,
        layout: &ProjectLayout<'_>,
    ) -> Result<ProjectOverview> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        let folder = match home_rel.rsplit_once('/') {
            Some((folder, _)) => format!("{folder}/"),
            None => String::new(),
        };
        let materials = format!("{}/", layout.material_dir);
        let linked_from_home = self.linked_notes(home_rel)?;
        let linked_from_research = if self.note_exists(layout.research_note) {
            self.linked_notes(layout.research_note)?
        } else {
            Vec::new()
        };
        let links_to_home = self.backlinks(home_rel)?;
        let mut items: Vec<OverviewItem> = Vec::new();
        for entry in self.index().notes_by_path()? {
            if entry.path == home_rel {
                continue;
            }
            let in_folder = !folder.is_empty() && entry.path.starts_with(&folder);
            let from_home = linked_from_home.contains(&entry.path);
            let from_research = linked_from_research.contains(&entry.path);
            let to_home = links_to_home.contains(&entry.path);
            if !(in_folder || from_home || to_home || from_research) {
                continue;
            }
            let note_type = NoteType::from_key(&entry.note_type);
            let is_source = entry.path != layout.research_note
                && (entry.path.starts_with(&materials)
                    || from_research
                    || note_type == Some(NoteType::Document));
            let group = if entry.path == layout.publish_note {
                OverviewGroup::Publish
            } else if is_source {
                OverviewGroup::Sources
            } else {
                OverviewGroup::of(&entry.note_type)
            };
            let kind =
                (group == OverviewGroup::Sources).then(|| self.source_kind(&entry.path, note_type));
            items.push(OverviewItem {
                path: entry.path,
                title: entry.title,
                modified_ms: entry.modified_ms,
                group,
                kind,
                linked_from_research: from_research,
                linked_from_home: from_home,
                links_to_home: to_home,
                in_folder,
            });
        }
        items.sort_by(|a, b| {
            let rank = |group: OverviewGroup| OverviewGroup::ALL.iter().position(|g| *g == group);
            rank(a.group)
                .cmp(&rank(b.group))
                .then(b.modified_ms.cmp(&a.modified_ms))
                .then(a.path.cmp(&b.path))
        });
        Ok(ProjectOverview {
            stage: ProjectStage::resolve(home.meta.project_stage.as_deref()),
            items,
            exports: self.publish_exports(layout.export_dir),
        })
    }

    /// 库里全部视频项目主页的路径（只查索引，不读盘）。
    ///
    /// # Errors
    /// 索引查询失败时返回错误。
    pub fn video_project_homes(&self) -> Result<Vec<String>> {
        Ok(self
            .index()
            .notes_by_path()?
            .into_iter()
            .filter(|entry| NoteType::from_key(&entry.note_type) == Some(NoteType::VideoProject))
            .map(|entry| entry.path)
            .collect())
    }

    /// 一篇笔记链到的库内笔记（wikilink 与指向 `.md` 的 Markdown 链接都算，指不到的不算）。
    fn linked_notes(&self, rel: &str) -> Result<Vec<String>> {
        let targets = self.outgoing_links(rel)?;
        Ok(self
            .resolve_wikilinks(rel, &targets)?
            .into_iter()
            .flatten()
            .collect())
    }

    /// 来源的类型：文档带来源地址的是网页剪藏（要读一次 frontmatter，只对文档读）。
    fn source_kind(&self, rel: &str, note_type: Option<NoteType>) -> SourceKind {
        match note_type {
            Some(NoteType::Transcript | NoteType::Recording) => SourceKind::Media,
            Some(NoteType::Document) => {
                if self
                    .read(rel)
                    .is_ok_and(|doc| doc.meta.source_url.is_some())
                {
                    SourceKind::Web
                } else {
                    SourceKind::Document
                }
            }
            Some(NoteType::Note) | None => SourceKind::Text,
            Some(_) => SourceKind::Note,
        }
    }

    /// 「发布」文件夹里已导出的文件（库相对路径，按名排序；笔记与隐藏文件不算）。文件夹
    /// 不存在 = 还没导出过，为空。
    #[must_use]
    pub fn publish_exports(&self, export_dir: &str) -> Vec<String> {
        let mut exports: Vec<String> = std::fs::read_dir(self.root().join(export_dir))
            .map(|dir| {
                dir.filter_map(std::result::Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                    .filter(|name| !name.starts_with('.') && !name.to_lowercase().ends_with(".md"))
                    .map(|name| format!("{export_dir}/{name}"))
                    .collect()
            })
            .unwrap_or_default();
        exports.sort();
        exports
    }

    /// 一个项目的成片候选（PRD-19-018，ADR-162 ⑪）：项目主页所在文件夹（含子文件夹）里
    /// 已转写的录音 / 转写笔记，新的在前。不新增 frontmatter 字段——成片是什么，看它放在哪个
    /// 项目文件夹里，与看板同一「查询投影」原则；多于一篇时由用户选，不猜。
    ///
    /// # Errors
    /// 索引查询失败时返回错误。
    pub fn project_publish_sources(&self, home_rel: &str) -> Result<Vec<PublishSource>> {
        let folder = match home_rel.rsplit_once('/') {
            Some((folder, _)) => format!("{folder}/"),
            None => String::new(),
        };
        let mut sources = Vec::new();
        for note_type in [NoteType::Transcript, NoteType::Recording] {
            let mut cursor: Option<String> = None;
            loop {
                let (entries, next) =
                    self.list_notes_of_type(note_type, PROJECT_PAGE, cursor.as_deref())?;
                for entry in entries {
                    if !entry.path.starts_with(&folder) {
                        continue;
                    }
                    let Ok(doc) = self.read(&entry.path) else {
                        continue;
                    };
                    let media = &doc.meta.media;
                    if !media.iter().any(|m| m.transcript) {
                        continue;
                    }
                    let durations: Vec<f64> = media.iter().filter_map(|m| m.duration).collect();
                    sources.push(PublishSource {
                        path: entry.path,
                        title: entry.title,
                        modified_ms: entry.modified_ms,
                        duration_secs: (!durations.is_empty()).then(|| durations.iter().sum()),
                        source_lang: media.iter().find_map(|m| m.lang.clone()),
                        has_translation: media.iter().any(|m| m.translation),
                    });
                }
                match next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
        }
        sources.sort_by(|a, b| {
            b.modified_ms
                .cmp(&a.modified_ms)
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok(sources)
    }

    /// 把一个视频项目移到另一个阶段（看板拖动卡片）。
    ///
    /// 返回落盘后的全文；`None` = 无需写盘（已在该阶段，或这一篇已不是视频项目——
    /// 后者不写一个谁都不读的键，看板刷新后卡片自然消失）。
    ///
    /// # Errors
    /// 读写失败时返回错误。
    pub fn set_project_stage(&self, rel: &str, stage: ProjectStage) -> Result<Option<String>> {
        self.rewrite_luna(rel, |meta| {
            if meta.note_type != NoteType::VideoProject {
                return false;
            }
            if meta.project_stage.as_deref() == Some(stage.key()) {
                return false;
            }
            meta.project_stage = Some(stage.key().to_owned());
            true
        })
    }

    /// 项目要发的渠道（主页读不出或不是视频项目时报错）。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目时返回错误。
    pub fn project_channels(&self, home_rel: &str) -> Result<Vec<String>> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        Ok(resolve_channels(
            &home.meta.project_channels,
            &self.custom_channels().unwrap_or_default(),
        ))
    }

    /// 改项目要发的渠道（按给定顺序、去重；至少一个，空集合不写）。返回落盘后的全文；`None` =
    /// 无需写盘。不认识的 id 原样写下——判定时由 [`resolve_channels`] 过滤（自定义渠道可能
    /// 在另一台设备上刚建好、还没同步过来）。
    ///
    /// # Errors
    /// 读写失败时返回错误。
    pub fn set_project_channels(&self, rel: &str, channels: &[String]) -> Result<Option<String>> {
        let mut keys: Vec<String> = Vec::new();
        for id in channels {
            let id = id.trim();
            if !id.is_empty() && !keys.iter().any(|key| key == id) {
                keys.push(id.to_owned());
            }
        }
        if keys.is_empty() {
            return Ok(None);
        }
        self.rewrite_luna(rel, |meta| {
            if meta.note_type != NoteType::VideoProject || meta.project_channels == keys {
                return false;
            }
            meta.project_channels = keys.clone();
            true
        })
    }

    /// 项目的计划发布日期（没排期或写法认不出为 `None`）。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目时返回错误。
    pub fn project_publish_date(&self, home_rel: &str) -> Result<Option<chrono::NaiveDate>> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        Ok(resolve_publish_date(
            home.meta.project_publish_date.as_deref(),
        ))
    }

    /// 改项目的计划发布日期（`None` = 清掉排期，键随之消失）。返回落盘后的全文；`None` =
    /// 无需写盘（不是视频项目，或与现值相同）。
    ///
    /// # Errors
    /// 读写失败时返回错误。
    pub fn set_project_publish_date(
        &self,
        rel: &str,
        date: Option<chrono::NaiveDate>,
    ) -> Result<Option<String>> {
        let value = date.map(|date| date.format(PUBLISH_DATE_FORMAT).to_string());
        self.rewrite_luna(rel, |meta| {
            if meta.note_type != NoteType::VideoProject || meta.project_publish_date == value {
                return false;
            }
            meta.project_publish_date.clone_from(&value);
            true
        })
    }

    /// 归档 / 取消归档一个视频项目（只改主页的 `luna.project_archived`；文件与阶段不动）。
    /// 返回新的文件内容；不是视频项目或本来就是这个状态时为 `None`（不写盘）。
    ///
    /// # Errors
    /// 读写失败时返回错误。
    pub fn set_project_archived(&self, rel: &str, archived: bool) -> Result<Option<String>> {
        self.rewrite_luna(rel, |meta| {
            if meta.note_type != NoteType::VideoProject || meta.project_archived == archived {
                return false;
            }
            meta.project_archived = archived;
            true
        })
    }

    /// 重命名视频项目（ADR-165 S6）：主页与项目文件夹里以「<旧名>-」开头的笔记（研究、脚本、
    /// 发布笔记、分发出来的几篇）一起改成新名字；项目文件夹与项目同名时文件夹也改。每一步都走
    /// [`Vault::move_`]，全库指向它们的链接随之改写。返回新主页的库相对路径。
    ///
    /// 动磁盘之前先查完：新名字与旧名字相同时什么都不做；要用的名字已被占用时整次不做
    /// （[`VaultError::AlreadyExists`]）。中途某一步失败时已改的不回滚（与文件树逐个改名同一口径），
    /// 错误如实返回。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目、名字被占用、移动失败时返回错误。
    pub fn rename_video_project(&self, home_rel: &str, new_stem: &str) -> Result<String> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        let (folder, file) = home_rel
            .rsplit_once('/')
            .ok_or_else(|| VaultError::NotFound(home_rel.into()))?;
        let old = file.strip_suffix(".md").unwrap_or(file);
        if old == new_stem {
            return Ok(home_rel.to_owned());
        }
        let mut renames: Vec<(String, String)> = self
            .project_named_notes(folder, old)?
            .into_iter()
            .map(|(name, tail)| {
                let renamed = if tail.is_empty() {
                    format!("{new_stem}.md")
                } else {
                    format!("{new_stem}-{tail}.md")
                };
                (format!("{folder}/{name}"), format!("{folder}/{renamed}"))
            })
            .collect();
        renames.sort();
        let (parent, folder_name) = folder.rsplit_once('/').unwrap_or(("", folder));
        let new_folder = if folder_name == old {
            if parent.is_empty() {
                new_stem.to_owned()
            } else {
                format!("{parent}/{new_stem}")
            }
        } else {
            folder.to_owned()
        };
        let taken = |rel: &str| {
            let abs = self.root().join(rel);
            abs.exists().then_some(abs)
        };
        for (_, to) in &renames {
            if let Some(abs) = taken(to) {
                return Err(VaultError::AlreadyExists(abs));
            }
        }
        if new_folder != folder {
            if let Some(abs) = taken(&new_folder) {
                return Err(VaultError::AlreadyExists(abs));
            }
        }
        for (from, to) in &renames {
            self.move_(from, to)?;
        }
        if new_folder != folder {
            self.move_(folder, &new_folder)?;
        }
        Ok(format!("{new_folder}/{new_stem}.md"))
    }

    /// 项目文件夹里以项目命名的笔记：主页 `<项目>.md` 与 `<项目>-<尾巴>.md`（研究、脚本、发布笔记、
    /// 分发稿……）。返回（文件名，尾巴），主页的尾巴为空串。重命名与删除项目共用这一个判据。
    fn project_named_notes(&self, folder: &str, stem: &str) -> Result<Vec<(String, String)>> {
        let child_prefix = format!("{stem}-");
        Ok(std::fs::read_dir(self.root().join(folder))?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter_map(|name| {
                let note = name.strip_suffix(".md")?;
                let tail = if note == stem {
                    ""
                } else {
                    note.strip_prefix(&child_prefix)?
                };
                let tail = tail.to_owned();
                Some((name, tail))
            })
            .collect())
    }

    /// 删除一个视频项目会删掉什么（ADR-167 ②，PRD-19-043）——先列清单、由用户确认，再交给
    /// [`Vault::delete`] 按设置里的删除方式逐项移走。
    ///
    /// 主页在与它同名的专属文件夹里（新建项目的默认形态）、且文件夹里没有别的视频项目时，删的是
    /// **整个文件夹**（素材、导出文件一起）；否则只删主页与同一文件夹里以「<项目>-」开头的笔记，
    /// 别的一概不动——文件夹不一定是专属的，按文件夹一刀切会误删别人的笔记。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目、文件夹读不出时返回错误。
    pub fn project_deletion(&self, home_rel: &str) -> Result<ProjectDeletion> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        let (folder, file) = home_rel.rsplit_once('/').unwrap_or(("", home_rel));
        let stem = file.strip_suffix(".md").unwrap_or(file);
        let folder_name = folder.rsplit_once('/').map_or(folder, |(_, name)| name);
        let prefix = format!("{folder}/");
        let other_project_inside = self.index().notes_by_path()?.into_iter().any(|entry| {
            entry.path != home_rel
                && entry.path.starts_with(&prefix)
                && NoteType::from_key(&entry.note_type) == Some(NoteType::VideoProject)
        });
        if !folder.is_empty() && folder_name == stem && !other_project_inside {
            let (notes, other_files) = count_files(&self.root().join(folder));
            return Ok(ProjectDeletion {
                targets: vec![folder.to_owned()],
                whole_folder: true,
                notes,
                other_files,
            });
        }
        let mut targets: Vec<String> = self
            .project_named_notes(folder, stem)?
            .into_iter()
            .map(|(name, _)| {
                if folder.is_empty() {
                    name
                } else {
                    format!("{folder}/{name}")
                }
            })
            .collect();
        targets.sort();
        Ok(ProjectDeletion {
            notes: targets.len(),
            targets,
            whole_folder: false,
            other_files: 0,
        })
    }

    /// 项目主页的阶段（主页读不出或不是视频项目时报错）。
    ///
    /// # Errors
    /// 主页读不出或不是视频项目时返回错误。
    pub fn project_stage(&self, home_rel: &str) -> Result<ProjectStage> {
        let home = self.read(home_rel)?;
        if home.meta.note_type != NoteType::VideoProject {
            return Err(VaultError::NotFound(home_rel.into()));
        }
        Ok(ProjectStage::resolve(home.meta.project_stage.as_deref()))
    }

    /// 项目的脚本：骨架起的那一篇（`preferred`，名字随界面语言由调用方给出）还在就用它；
    /// 被改名或删了，就用项目文件夹里最近改过的一篇脚本；都没有为 `None`。
    ///
    /// # Errors
    /// 索引查询失败时返回错误。
    pub fn project_script(&self, home_rel: &str, preferred: &str) -> Result<Option<String>> {
        if self.note_exists(preferred) {
            return Ok(Some(preferred.to_owned()));
        }
        let folder = match home_rel.rsplit_once('/') {
            Some((folder, _)) => format!("{folder}/"),
            None => return Ok(None),
        };
        let mut scripts: Vec<(i64, String)> = self
            .index()
            .notes_by_path()?
            .into_iter()
            .filter(|entry| {
                entry.path.starts_with(&folder)
                    && NoteType::from_key(&entry.note_type) == Some(NoteType::Script)
            })
            .map(|entry| (entry.modified_ms, entry.path))
            .collect();
        scripts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        Ok(scripts.into_iter().next().map(|(_, path)| path))
    }

    /// 新建一个视频项目：建文件夹，在里面写项目主页与骨架笔记，返回主页。
    ///
    /// `folder` 已存在时返回 [`VaultError::AlreadyExists`]，由调用方换一个名字重试——
    /// **绝不往已有文件夹里写**（那可能是用户自己的东西）。骨架中途失败时已写下的文件保留
    /// （都是新建的空骨架，用户可见可删），错误照实返回。
    ///
    /// # Errors
    /// 文件夹已存在、路径越界或写盘失败时返回错误。
    pub fn create_video_project(
        &self,
        folder: &str,
        home_stem: &str,
        home_body: &str,
        children: &[ProjectChild],
    ) -> Result<NoteDoc> {
        self.create_dir(folder)?;
        let home = self.create(
            &format!("{folder}/{home_stem}.md"),
            NoteType::VideoProject,
            &TemplateVars {
                body: home_body.to_owned(),
                ..TemplateVars::default()
            },
        )?;
        for child in children {
            self.create(
                &format!("{folder}/{}.md", child.stem),
                child.note_type,
                &TemplateVars {
                    body: child.body.clone(),
                    ..TemplateVars::default()
                },
            )?;
        }
        Ok(home)
    }
}

/// 项目文件夹已存在（[`Vault::create_video_project`] 的「换个名字再试」信号）。
#[must_use]
pub fn is_already_exists(err: &VaultError) -> bool {
    matches!(err, VaultError::AlreadyExists(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_channels_round_trip_in_written_order() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        assert_eq!(
            vault.project_channels("Creator/P/P.md").unwrap(),
            crate::channel::DEFAULT_CHANNELS
        );
        let ids = [
            "douyin".to_owned(),
            "youtube".to_owned(),
            "douyin".to_owned(),
        ];
        assert!(vault
            .set_project_channels("Creator/P/P.md", &ids)
            .unwrap()
            .is_some());
        assert_eq!(
            vault.project_channels("Creator/P/P.md").unwrap(),
            ["douyin", "youtube"]
        );
        assert_eq!(
            vault.video_projects().unwrap()[0].channels,
            ["douyin", "youtube"]
        );
        assert_eq!(
            vault.set_project_channels("Creator/P/P.md", &[]).unwrap(),
            None
        );
    }

    #[test]
    fn deleting_a_project_in_its_own_folder_takes_the_whole_folder() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let child = |stem: &str| ProjectChild {
            stem: stem.to_owned(),
            note_type: NoteType::Note,
            body: String::new(),
        };
        vault
            .create_video_project("Creator/P", "P", "", &[child("P-研究"), child("P-脚本")])
            .unwrap();
        std::fs::create_dir_all(dir.path().join("Creator/P/素材")).unwrap();
        std::fs::write(dir.path().join("Creator/P/素材/访谈.m4a"), b"x").unwrap();
        std::fs::write(dir.path().join("Creator/P/素材/访谈.md"), "# 访谈\n").unwrap();
        let plan = vault.project_deletion("Creator/P/P.md").unwrap();
        assert_eq!(plan.targets, ["Creator/P"]);
        assert!(plan.whole_folder);
        assert_eq!((plan.notes, plan.other_files), (4, 1));
    }

    #[test]
    fn deleting_a_project_in_a_shared_folder_only_takes_its_named_notes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let child = |stem: &str| ProjectChild {
            stem: stem.to_owned(),
            note_type: NoteType::Note,
            body: String::new(),
        };
        // 文件夹名与项目名不同：不是专属文件夹，别人的笔记不能跟着删。
        vault
            .create_video_project("Creator/合集", "P", "", &[child("P-研究")])
            .unwrap();
        std::fs::write(dir.path().join("Creator/合集/别人的笔记.md"), "# x\n").unwrap();
        std::fs::write(dir.path().join("Creator/合集/PX.md"), "# 不是 P 的子笔记\n").unwrap();
        let plan = vault.project_deletion("Creator/合集/P.md").unwrap();
        assert_eq!(
            plan.targets,
            ["Creator/合集/P-研究.md", "Creator/合集/P.md"]
        );
        assert!(!plan.whole_folder);
        assert_eq!((plan.notes, plan.other_files), (2, 0));
    }

    #[test]
    fn a_folder_holding_another_project_is_never_deleted_whole() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        vault
            .create_video_project("Creator/P/Q", "Q", "", &[])
            .unwrap();
        let plan = vault.project_deletion("Creator/P/P.md").unwrap();
        assert_eq!(plan.targets, ["Creator/P/P.md"]);
        assert!(!plan.whole_folder);
        assert!(
            vault
                .project_deletion("Creator/P/Q/Q.md")
                .unwrap()
                .whole_folder
        );
    }

    #[test]
    fn every_stage_has_exactly_one_next_action() {
        assert_eq!(next_action(ProjectStage::Idea), Some(NextAction::WriteIdea));
        assert_eq!(
            next_action(ProjectStage::Research),
            Some(NextAction::ImportMaterial)
        );
        assert_eq!(
            next_action(ProjectStage::Writing),
            Some(NextAction::OpenTeleprompter)
        );
        assert_eq!(
            next_action(ProjectStage::Recording),
            Some(NextAction::StartFollow)
        );
        assert_eq!(
            next_action(ProjectStage::Editing),
            Some(NextAction::ImportFinalCut)
        );
        assert_eq!(
            next_action(ProjectStage::Publishing),
            Some(NextAction::OpenPublish)
        );
        assert_eq!(
            next_action(ProjectStage::Published),
            Some(NextAction::Distribute)
        );
    }

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn stage_keys_round_trip_and_unknown_falls_into_the_first_column() {
        for stage in ProjectStage::ALL {
            assert_eq!(ProjectStage::from_key(stage.key()), Some(stage));
        }
        assert_eq!(
            ProjectStage::from_key(" Editing "),
            Some(ProjectStage::Editing)
        );
        assert_eq!(ProjectStage::resolve(None), ProjectStage::Idea);
        assert_eq!(ProjectStage::resolve(Some("剪辑中")), ProjectStage::Idea);
        // 不得与成熟度轴的唯一取值撞名（ADR-086 的 `draft` 只属于 `luna.stage`）。
        assert!(ProjectStage::from_key(crate::STAGE_DRAFT).is_none());
    }

    #[test]
    fn cards_in_hidden_columns_fall_back_to_the_nearest_visible_column_before_them() {
        use ProjectStage::{Editing, Idea, Published, Recording, Research, Writing};
        // 用户的列序：选题 研究 撰稿 录制 剪辑 已发布；隐藏了「研究」「录制」「剪辑」。
        let layout = [
            (Idea, true),
            (Research, false),
            (Writing, true),
            (Recording, false),
            (Editing, false),
            (Published, true),
        ];
        assert_eq!(display_column(Writing, &layout), Writing);
        assert_eq!(display_column(Research, &layout), Idea);
        assert_eq!(
            display_column(Editing, &layout),
            Writing,
            "跳过连续的隐藏列"
        );
        // 「发布」不在列序里（缺项不会发生，但判定不该因此丢卡片）：落第一个显示列。
        assert_eq!(display_column(ProjectStage::Publishing, &layout), Idea);
        // 前面没有显示列：落第一个显示列。
        let leading_hidden = [(Idea, false), (Research, true)];
        assert_eq!(display_column(Idea, &leading_hidden), Research);
        assert_eq!(first_visible_stage(&leading_hidden), Research);
        assert_eq!(first_visible_stage(&[]), Idea);
    }

    #[test]
    fn a_new_project_is_a_folder_with_a_home_page_at_the_first_stage() {
        let (_dir, vault) = vault();
        let home = vault
            .create_video_project(
                "Creator/第一期",
                "第一期",
                "## 选题\n",
                &[ProjectChild {
                    stem: "第一期-脚本".into(),
                    note_type: NoteType::Script,
                    body: "## 开头\n".into(),
                }],
            )
            .unwrap();
        assert_eq!(home.path, "Creator/第一期/第一期.md");
        assert_eq!(home.meta.note_type, NoteType::VideoProject);
        assert_eq!(home.meta.project_stage.as_deref(), Some("idea"));
        let script = vault.read("Creator/第一期/第一期-脚本.md").unwrap();
        assert_eq!(script.meta.note_type, NoteType::Script);

        let cards = vault.video_projects().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "第一期");
        assert_eq!(cards[0].stage, ProjectStage::Idea);

        // 同名文件夹已存在：不往里写，交给调用方换名。
        let again = vault
            .create_video_project("Creator/第一期", "第一期", "", &[])
            .unwrap_err();
        assert!(is_already_exists(&again));
    }

    #[test]
    fn publish_sources_are_the_transcribed_media_notes_inside_the_project_folder() {
        let (_dir, vault) = vault();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        let media = |path: &str, extra: &str| {
            format!("---\nluna:\n  type: transcript\n  media:\n    - path: {path}.mp4\n      duration: 61.5\n      transcript: true\n{extra}---\n正文\n")
        };
        vault
            .write(
                "Creator/P/成片.md",
                &media(
                    "Creator/P/成片",
                    "      translation: true\n      lang: zh\n",
                ),
            )
            .unwrap();
        // 子文件夹里的也算；没转写的、项目外的都不算。
        vault
            .write("Creator/P/素材/花絮.md", &media("Creator/P/素材/花絮", ""))
            .unwrap();
        vault
            .write(
                "Creator/P/未转写.md",
                "---\nluna:\n  type: transcript\n  media:\n    - path: x.mp4\n---\n",
            )
            .unwrap();
        vault
            .write("Creator/Q/别的项目.md", &media("Creator/Q/别的项目", ""))
            .unwrap();
        vault.rebuild_index().unwrap();
        let sources = vault.project_publish_sources("Creator/P/P.md").unwrap();
        let mut paths: Vec<&str> = sources.iter().map(|s| s.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, ["Creator/P/成片.md", "Creator/P/素材/花絮.md"]);
        let cut = sources.iter().find(|s| s.title == "成片").unwrap();
        assert!(cut.has_translation);
        assert_eq!(cut.source_lang.as_deref(), Some("zh"));
        assert_eq!(cut.duration_secs, Some(61.5));
    }

    #[test]
    fn moving_a_card_rewrites_only_the_project_stage() {
        let (_dir, vault) = vault();
        vault
            .create_video_project("Creator/P", "P", "正文保持不变\n", &[])
            .unwrap();
        let written = vault
            .set_project_stage("Creator/P/P.md", ProjectStage::Writing)
            .unwrap();
        assert!(written.is_some());
        let doc = vault.read("Creator/P/P.md").unwrap();
        assert_eq!(doc.meta.project_stage.as_deref(), Some("writing"));
        assert!(doc.content.ends_with("正文保持不变\n"));
        assert_eq!(doc.meta.stage_raw, None, "不碰成熟度轴");
        // 已在该阶段：不写盘。
        assert_eq!(
            vault
                .set_project_stage("Creator/P/P.md", ProjectStage::Writing)
                .unwrap(),
            None
        );
        assert_eq!(
            vault.video_projects().unwrap()[0].stage,
            ProjectStage::Writing
        );
    }

    #[test]
    fn publish_date_round_trips_clears_and_tolerates_bad_values() {
        let (_dir, vault) = vault();
        vault
            .create_video_project("Creator/P", "P", "正文\n", &[])
            .unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 24);
        assert!(vault
            .set_project_publish_date("Creator/P/P.md", date)
            .unwrap()
            .is_some());
        let doc = vault.read("Creator/P/P.md").unwrap();
        assert_eq!(doc.meta.project_publish_date.as_deref(), Some("2026-10-24"));
        assert_eq!(vault.project_publish_date("Creator/P/P.md").unwrap(), date);
        assert_eq!(vault.video_projects().unwrap()[0].publish_date, date);
        // 同一天：不写盘。
        assert_eq!(
            vault
                .set_project_publish_date("Creator/P/P.md", date)
                .unwrap(),
            None
        );
        // 清掉：键消失。
        vault
            .set_project_publish_date("Creator/P/P.md", None)
            .unwrap();
        assert!(vault
            .read("Creator/P/P.md")
            .unwrap()
            .meta
            .project_publish_date
            .is_none());
        // 手改坏了：当作没排期。
        assert_eq!(resolve_publish_date(Some("下周五")), None);
        assert_eq!(resolve_publish_date(Some("2026-02-30")), None);
        assert_eq!(resolve_publish_date(Some(" 2026-10-24 ")), date);
    }

    #[test]
    fn a_note_that_is_not_a_project_is_never_given_a_stage() {
        let (_dir, vault) = vault();
        vault
            .create("Notes/a.md", NoteType::Note, &TemplateVars::default())
            .unwrap();
        assert_eq!(
            vault
                .set_project_stage("Notes/a.md", ProjectStage::Editing)
                .unwrap(),
            None
        );
        assert!(vault
            .read("Notes/a.md")
            .unwrap()
            .meta
            .project_stage
            .is_none());
    }

    #[test]
    fn the_overview_gathers_linked_and_folder_notes_by_kind() {
        let (dir, vault) = vault();
        vault
            .create_video_project(
                "Creator/P",
                "P",
                "- [[P-脚本]]\n- [[别处的研究]]\n",
                &[ProjectChild {
                    stem: "P-脚本".into(),
                    note_type: NoteType::Script,
                    body: "## 开头\n".into(),
                }],
            )
            .unwrap();
        // 链出：放在项目外的研究；链入：一条灵感；文件夹里：成片与发布笔记；无关：别的笔记。
        vault.write("Notes/别处的研究.md", "资料\n").unwrap();
        vault
            .write(
                "Creator/Swipe/好开头.md",
                "---\nluna:\n  type: swipe\n---\n给 [[P]] 用\n",
            )
            .unwrap();
        vault
            .write(
                "Creator/P/成片.md",
                "---\nluna:\n  type: transcript\n---\n正文\n",
            )
            .unwrap();
        vault.write("Creator/P/P-发布.md", "## 章节\n").unwrap();
        vault.write("Notes/无关.md", "无关\n").unwrap();
        // 来源：素材文件夹里的转写、项目外被研究笔记链到的一篇、文件夹里的网页剪藏、粘贴的文字。
        vault
            .write(
                "Creator/P/素材/访谈.md",
                "---\nluna:\n  type: transcript\n---\n正文\n",
            )
            .unwrap();
        vault.write("Notes/外部资料.md", "外部\n").unwrap();
        vault
            .write(
                "Creator/P/素材/一篇文章.md",
                "---\nluna:\n  type: document\n  source_url: https://example.com/a\n---\n正文\n",
            )
            .unwrap();
        vault
            .write("Creator/P/素材/粘贴的文字.md", "一段话\n")
            .unwrap();
        vault
            .write(
                "Creator/P/P-研究.md",
                "## 素材\n\n- [外部资料](../../Notes/外部资料.md)\n",
            )
            .unwrap();
        std::fs::create_dir_all(dir.path().join("Creator/P/发布")).unwrap();
        std::fs::write(dir.path().join("Creator/P/发布/P.zh.srt"), "1\n").unwrap();
        vault.rebuild_index().unwrap();

        let layout = ProjectLayout {
            publish_note: "Creator/P/P-发布.md",
            research_note: "Creator/P/P-研究.md",
            material_dir: "Creator/P/素材",
            export_dir: "Creator/P/发布",
        };
        let overview = vault.project_overview("Creator/P/P.md", &layout).unwrap();
        let sources: Vec<(&str, Option<SourceKind>, bool)> = overview
            .items
            .iter()
            .filter(|i| i.group == OverviewGroup::Sources)
            .map(|i| (i.path.as_str(), i.kind, i.linked_from_research))
            .collect();
        let mut sources = sources;
        sources.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            sources,
            [
                ("Creator/P/素材/一篇文章.md", Some(SourceKind::Web), false),
                (
                    "Creator/P/素材/粘贴的文字.md",
                    Some(SourceKind::Text),
                    false
                ),
                ("Creator/P/素材/访谈.md", Some(SourceKind::Media), false),
                ("Notes/外部资料.md", Some(SourceKind::Text), true),
            ]
        );
        let rows: Vec<(&str, OverviewGroup, bool, bool, bool)> = overview
            .items
            .iter()
            .filter(|i| i.group != OverviewGroup::Sources && i.path != "Creator/P/P-研究.md")
            .map(|i| {
                (
                    i.path.as_str(),
                    i.group,
                    i.linked_from_home,
                    i.links_to_home,
                    i.in_folder,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "Creator/P/P-脚本.md",
                    OverviewGroup::Scripts,
                    true,
                    false,
                    true
                ),
                (
                    "Creator/P/成片.md",
                    OverviewGroup::Footage,
                    false,
                    false,
                    true
                ),
                (
                    "Notes/别处的研究.md",
                    OverviewGroup::Research,
                    true,
                    false,
                    false
                ),
                (
                    "Creator/Swipe/好开头.md",
                    OverviewGroup::Swipes,
                    false,
                    true,
                    false
                ),
                (
                    "Creator/P/P-发布.md",
                    OverviewGroup::Publish,
                    false,
                    false,
                    true
                ),
            ]
        );
        assert_eq!(overview.exports, ["Creator/P/发布/P.zh.srt"]);
        assert_eq!(overview.stage, ProjectStage::Idea);
        // 不是项目主页：报错，不猜。
        assert!(vault.project_overview("Notes/无关.md", &layout).is_err());
    }

    #[test]
    fn renaming_a_project_moves_its_folder_home_and_children_and_rewrites_links() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project(
                "Creator/P",
                "P",
                "资料：[[P-研究]]",
                &[ProjectChild {
                    stem: "P-研究".into(),
                    note_type: NoteType::Note,
                    body: "回到 [[P]]".into(),
                }],
            )
            .unwrap();
        vault
            .write("Creator/P/P-X.md", "> 由「[[P]]」改写")
            .unwrap();
        vault
            .write("Creator/P/Pilot.md", "不是这个项目的子笔记")
            .unwrap();
        vault.write("Notes/外部.md", "见 [[P]]").unwrap();

        let home = vault
            .rename_video_project("Creator/P/P.md", "新名")
            .unwrap();
        assert_eq!(home, "Creator/新名/新名.md");
        for rel in [
            "Creator/新名/新名-研究.md",
            "Creator/新名/新名-X.md",
            "Creator/新名/Pilot.md",
        ] {
            assert!(vault.note_exists(rel), "{rel} 应该在");
        }
        assert!(!dir.path().join("Creator/P").exists());
        assert!(vault.read(&home).unwrap().content.contains("[[新名-研究]]"));
        assert!(vault
            .read("Notes/外部.md")
            .unwrap()
            .content
            .contains("[[新名]]"));
        assert!(vault
            .read("Creator/新名/新名-研究.md")
            .unwrap()
            .content
            .contains("[[新名]]"));
    }

    #[test]
    fn renaming_onto_a_taken_name_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        vault
            .create_video_project("Creator/Q", "Q", "", &[])
            .unwrap();
        assert!(matches!(
            vault.rename_video_project("Creator/P/P.md", "Q"),
            Err(VaultError::AlreadyExists(_))
        ));
        assert!(vault.note_exists("Creator/P/P.md"));
        assert_eq!(
            vault.rename_video_project("Creator/P/P.md", "P").unwrap(),
            "Creator/P/P.md"
        );
    }

    #[test]
    fn archiving_round_trips_and_does_not_touch_the_stage() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .create_video_project("Creator/P", "P", "", &[])
            .unwrap();
        vault
            .set_project_stage("Creator/P/P.md", ProjectStage::Writing)
            .unwrap();
        assert!(vault
            .set_project_archived("Creator/P/P.md", true)
            .unwrap()
            .is_some());
        assert!(vault
            .set_project_archived("Creator/P/P.md", true)
            .unwrap()
            .is_none());
        let card = vault.video_projects().unwrap().remove(0);
        assert!(card.archived);
        assert_eq!(card.stage, ProjectStage::Writing);
        vault.set_project_archived("Creator/P/P.md", false).unwrap();
        let doc = vault.read("Creator/P/P.md").unwrap();
        assert!(
            !doc.content.contains("project_archived"),
            "取消归档后不留键"
        );
    }
}
