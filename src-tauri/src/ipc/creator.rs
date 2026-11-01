//! 创作空间（ADR-162，PRD-19 §3.1）：视频项目看板、新建项目、拖动卡片改阶段。
//!
//! 阶段与看板的判定在 `luna_vault::creator`；这里只负责落盘目录、界面语言的骨架文字、
//! 重名换名与广播（ADR-003：前端不拼路径、不判阶段）。

use super::settings::{apply_setting_side_effects, set_setting_value, SettingValueDto};
use super::*;
use luna_config::creator_board::{board_stage_tokens, resolve_board_stages};
use luna_vault::{
    display_column, first_visible_stage, next_action, NextAction, ProjectCard, ProjectChild,
    ProjectStage,
};

/// 看板列配置的设置键（PRD-19-003：七个阶段按需显隐、可排序，至少一列）。
const BOARD_STAGES_KEY: &str = "creator.board_stages";
/// 看板视图的设置键（看板 / 列表 / 表格）。
const BOARD_VIEW_KEY: &str = "creator.board_view";

/// 看板的视图：同一份项目数据的三种看法（PRD-19-003）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CreatorBoardViewDto {
    /// 按阶段分列，拖动卡片改阶段。
    Board,
    /// 按阶段分组的紧凑清单，组可折叠。
    List,
    /// 一行一个项目，就地改阶段。
    Table,
    /// 按计划发布日期排的月历，拖到某一天即改日期（PRD-19-040）。
    Calendar,
}

impl CreatorBoardViewDto {
    fn key(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::List => "list",
            Self::Table => "table",
            Self::Calendar => "calendar",
        }
    }

    /// 设置值 → 视图；读不到或不认识（手改坏了的配置）→ 看板。
    fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("list") => Self::List,
            Some("table") => Self::Table,
            Some("calendar") => Self::Calendar,
            _ => Self::Board,
        }
    }
}

/// 视频项目的阶段（看板列，顺序即列序）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStageDto {
    Idea,
    Research,
    Writing,
    Recording,
    Editing,
    Publishing,
    Published,
}

impl From<ProjectStage> for ProjectStageDto {
    fn from(value: ProjectStage) -> Self {
        match value {
            ProjectStage::Idea => Self::Idea,
            ProjectStage::Research => Self::Research,
            ProjectStage::Writing => Self::Writing,
            ProjectStage::Recording => Self::Recording,
            ProjectStage::Editing => Self::Editing,
            ProjectStage::Publishing => Self::Publishing,
            ProjectStage::Published => Self::Published,
        }
    }
}

impl From<ProjectStageDto> for ProjectStage {
    fn from(value: ProjectStageDto) -> Self {
        match value {
            ProjectStageDto::Idea => Self::Idea,
            ProjectStageDto::Research => Self::Research,
            ProjectStageDto::Writing => Self::Writing,
            ProjectStageDto::Recording => Self::Recording,
            ProjectStageDto::Editing => Self::Editing,
            ProjectStageDto::Publishing => Self::Publishing,
            ProjectStageDto::Published => Self::Published,
        }
    }
}

/// 看板上的一张卡片。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectCardDto {
    /// 项目主页的库相对路径。
    pub path: String,
    /// 标题（主页文件名）。
    pub title: String,
    /// 所处阶段（Rust 已判定：无键或不认识的词都落在「选题」）。
    pub stage: ProjectStageDto,
    /// 显示在哪一列：阶段那一列被隐藏时，是它前面最近的显示列（`display_column`）；
    /// 与 `stage` 不同时卡片上要标出真实阶段。
    pub column: ProjectStageDto,
    /// 项目文件夹（主页所在目录，库相对路径；表格视图的「位置」列）。
    pub folder: String,
    /// 主页最后修改时间（epoch ms）。
    pub modified_ms: f64,
    /// 要发的渠道 id（有序）。
    pub channels: Vec<String>,
    /// 计划发布日期（ISO `YYYY-MM-DD`）；没排期为 `None`。
    pub publish_date: Option<String>,
    /// 卡片信号（PRD-19-040）。
    pub signals: ProjectSignalsDto,
}

/// 看板卡片上的信号（PRD-19-040）：一眼看出这一期做到哪了。全由 Rust 从文件判定。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectSignalsDto {
    /// 脚本全篇约念多少秒（与脚本工具栏同一个估算、同一个语速）；没有脚本为 `None`。
    pub script_seconds: Option<u32>,
    /// 研究笔记里的引文条数。
    pub quotes: u32,
    /// 发布清单完成几步 / 共几步（与发布页签同一份判定）。
    pub publish_done: u32,
    pub publish_total: u32,
    /// 这个阶段的下一步（与项目页头同一个映射）；已发布为 `None`。
    pub next_action: Option<NextActionDto>,
}

/// 一个项目的卡片信号：脚本时长、引文数、发布进度、下一步。读不出来的那一项按「没有」算，
/// 不让一个坏文件拖垮整张看板。
fn project_signals(
    vault: &Vault,
    card: &ProjectCard,
    rate: luna_vault::SpeakingRate,
    copy: BackendCopy<'_>,
) -> ProjectSignalsDto {
    let script_rel = project_child_rel(&card.path, copy, "creator-note-script", "脚本");
    let script_seconds = vault
        .project_script(&card.path, &script_rel)
        .ok()
        .flatten()
        .and_then(|path| vault.read(&path).ok())
        .map(|doc| luna_vault::script_outline(&doc.content, rate).total_seconds);
    let checklist = super::creator_publish::publish_checklist_of(vault, &card.path, copy).ok();
    ProjectSignalsDto {
        script_seconds,
        quotes: u32::try_from(super::creator_quote::project_quote_count(
            vault, &card.path, copy,
        ))
        .unwrap_or(u32::MAX),
        publish_done: checklist.as_ref().map_or(0, |list| list.done),
        publish_total: checklist.as_ref().map_or(0, |list| list.total),
        next_action: next_action(card.stage).map(Into::into),
    }
}

fn card_dto(
    card: ProjectCard,
    layout: &[(ProjectStage, bool)],
    signals: ProjectSignalsDto,
) -> ProjectCardDto {
    ProjectCardDto {
        channels: card.channels.clone(),
        publish_date: card
            .publish_date
            .map(|date| date.format(luna_vault::PUBLISH_DATE_FORMAT).to_string()),
        signals,
        column: display_column(card.stage, layout).into(),
        folder: card
            .path
            .rsplit_once('/')
            .map_or_else(String::new, |(dir, _)| dir.to_owned()),
        path: card.path,
        title: card.title,
        stage: card.stage.into(),
        #[allow(clippy::cast_precision_loss)]
        modified_ms: card.modified_ms as f64,
    }
}

/// 看板数据：显示的列（按用户的列序）、隐藏的列、全部卡片。列序由 Rust 给出，前端不自带
/// 一份阶段清单。
#[derive(Debug, Clone, Serialize, Type)]
pub struct CreatorBoardDto {
    /// 当前视图（设置 `creator.board_view`）。
    pub view: CreatorBoardViewDto,
    pub stages: Vec<ProjectStageDto>,
    /// 用户收起的列（按列序）——「看板列」菜单里未勾选的那几项。
    pub hidden_stages: Vec<ProjectStageDto>,
    /// 看板、列表、表格、日历与侧栏项目清单里的项目（不含已归档）。
    pub projects: Vec<ProjectCardDto>,
    /// 已归档的项目（侧栏「已归档」一节；ADR-165 S6）。
    pub archived: Vec<ProjectCardDto>,
}

/// 归一化后的列配置（与设置写入同一个归一化函数，至少一列显示）。
fn board_layout(state: &AppState) -> Vec<(ProjectStage, bool)> {
    let tokens = state
        .settings_snapshot()
        .get(BOARD_STAGES_KEY)
        .and_then(|value| value.as_string_list().map(<[String]>::to_vec))
        .unwrap_or_default();
    resolve_board_stages(&tokens)
        .into_iter()
        .filter_map(|(id, visible)| ProjectStage::from_key(id).map(|stage| (stage, visible)))
        .collect()
}

fn board_view(state: &AppState) -> CreatorBoardViewDto {
    CreatorBoardViewDto::from_setting(state.settings_snapshot().get_str(BOARD_VIEW_KEY).as_deref())
}

/// 看板数据的唯一组装口：列配置与视图都从设置快照读。
fn board_for(state: &AppState, window: &tauri::Window) -> Result<CreatorBoardDto, String> {
    let layout = board_layout(state);
    let view = board_view(state);
    let i18n = backend_copy_arc(window.app_handle());
    state.with_vault(window.label(), |vault| {
        board_of(vault, &layout, view, i18n.as_deref())
    })
}

fn board_of(
    vault: &Vault,
    layout: &[(ProjectStage, bool)],
    view: CreatorBoardViewDto,
    copy: BackendCopy<'_>,
) -> Result<CreatorBoardDto, String> {
    let rate = vault.speaking_rate();
    let pick = |want: bool| -> Vec<ProjectStageDto> {
        layout
            .iter()
            .filter(|(_, visible)| *visible == want)
            .map(|(stage, _)| (*stage).into())
            .collect()
    };
    let (archived, active): (Vec<_>, Vec<_>) = vault
        .video_projects()
        .map_err(vault_err)?
        .into_iter()
        .partition(|card| card.archived);
    let to_dto = |cards: Vec<luna_vault::creator::ProjectCard>| -> Vec<ProjectCardDto> {
        cards
            .into_iter()
            .map(|card| {
                let signals = project_signals(vault, &card, rate, copy);
                card_dto(card, layout, signals)
            })
            .collect()
    };
    Ok(CreatorBoardDto {
        view,
        stages: pick(true),
        hidden_stages: pick(false),
        projects: to_dto(active),
        archived: to_dto(archived),
    })
}

/// 创作空间的看板（全部视频项目及其阶段）。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_board(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<CreatorBoardDto, String> {
    board_for(&state, &window)
}

/// 拖动卡片：把项目移到另一个阶段，返回刷新后的看板（以文件为准重画，不由前端乐观推算）。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_set_project_stage(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    stage: ProjectStageDto,
) -> Result<CreatorBoardDto, String> {
    let written = state.with_vault(window.label(), |vault| {
        vault
            .set_project_stage(&path, stage.into())
            .map_err(vault_err)
    })?;
    if let Some(content) = written {
        // 打开着这篇主页的编辑器据此跟上 `luna:` 块的那一行变化。
        broadcast_note_write(&app, window.label(), &path, content);
    }
    board_for(&state, &window)
}

/// 改项目的计划发布日期（项目页头的日期、日历视图里拖到某一天；`None` = 清掉排期），返回
/// 刷新后的看板。
///
/// # Errors
/// 日期写法不是 `YYYY-MM-DD`、库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_set_publish_date(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    date: Option<String>,
) -> Result<CreatorBoardDto, String> {
    let parsed = match date.as_deref() {
        None => None,
        Some(raw) => Some(
            luna_vault::resolve_publish_date(Some(raw))
                .ok_or_else(|| coded_error(&app, "CREATOR_BAD_DATE", "creator-bad-date"))?,
        ),
    };
    let written = state.with_vault(window.label(), |vault| {
        vault
            .set_project_publish_date(&path, parsed)
            .map_err(vault_err)
    })?;
    if let Some(content) = written {
        broadcast_note_write(&app, window.label(), &path, content);
    }
    board_for(&state, &window)
}

/// 归档 / 取消归档一个项目（项目页头的「更多」菜单；ADR-165 S6）：只改主页的
/// `luna.project_archived`，文件与阶段不动。返回刷新后的看板。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_set_archived(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    archived: bool,
) -> Result<CreatorBoardDto, String> {
    let written = state.with_vault(window.label(), |vault| {
        vault
            .set_project_archived(&home, archived)
            .map_err(vault_err)
    })?;
    if let Some(content) = written {
        broadcast_note_write(&app, window.label(), &home, content);
    }
    board_for(&state, &window)
}

/// 重命名一个项目（项目页头的「更多」菜单；ADR-165 S6）：主页、以「<项目>-」开头的笔记与
/// 同名的项目文件夹一起改名，全库链接随之改写。返回新主页的库相对路径。
///
/// # Errors
/// 名字为空或不能做文件名（`CREATOR_PROJECT_NAME`）、已被占用（`CREATOR_PROJECT_NAME_TAKEN`）、
/// 库未打开或移动失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_rename_project(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    title: String,
) -> Result<String, String> {
    let stem = sanitized_file_stem(&title)
        .ok_or_else(|| coded_error(&app, "CREATOR_PROJECT_NAME", "creator-project-name"))?;
    let renamed = state.with_vault(window.label(), |vault| {
        vault
            .rename_video_project(&home, &stem)
            .map_err(|err| match err {
                luna_vault::VaultError::AlreadyExists(_) => coded_error(
                    &app,
                    "CREATOR_PROJECT_NAME_TAKEN",
                    "creator-project-name-taken",
                ),
                other => vault_err(other),
            })
    })?;
    emit_vault_changed_in_scope(
        &app,
        &renamed,
        false,
        VaultChangeScope::for_window(window.label()),
    );
    Ok(renamed)
}

/// 删除一个项目会删掉什么（ADR-167 ②）：确认框据此写明篇数，确认后前端把 `targets` 交给
/// `vault_delete`（与笔记删除同一条路，按设置里的删除方式移走）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectDeletionDto {
    pub targets: Vec<String>,
    /// 删的是整个项目文件夹（否则只删主页与以「<项目>-」开头的笔记）。
    pub whole_folder: bool,
    pub notes: u32,
    pub other_files: u32,
}

/// 删除项目之前先问清会删掉什么（ADR-167 ②，PRD-19-043）。
///
/// # Errors
/// 创作未启用、库未打开、主页读不出或不是视频项目时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_project_deletion(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<ProjectDeletionDto, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    state.with_vault(window.label(), |vault| {
        let plan = vault.project_deletion(&home).map_err(vault_err)?;
        Ok(ProjectDeletionDto {
            targets: plan.targets,
            whole_folder: plan.whole_folder,
            notes: u32::try_from(plan.notes).unwrap_or(u32::MAX),
            other_files: u32::try_from(plan.other_files).unwrap_or(u32::MAX),
        })
    })
}

/// 看板上就地显隐一列（列头右键「隐藏这一列」、页头「看板列」菜单），返回刷新后的看板。
///
/// 与设置页走同一条 `set_setting_value` 通路（归一化 + 校验 + 落盘 + 广播）：想关掉最后一列
/// 时归一化会把它留下，与设置页的「至少一列」同源。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_set_stage_visible(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    stage: ProjectStageDto,
    visible: bool,
) -> Result<CreatorBoardDto, String> {
    let stage: ProjectStage = stage.into();
    let mut entries: Vec<(&str, bool)> = board_layout(&state)
        .into_iter()
        .map(|(s, v)| (s.key(), v))
        .collect();
    if let Some(entry) = entries.iter_mut().find(|(id, _)| *id == stage.key()) {
        entry.1 = visible;
    }
    persist_setting(
        &app,
        &state,
        BOARD_STAGES_KEY,
        SettingValueDto::StringList(board_stage_tokens(&entries)),
    )?;
    board_for(&state, &window)
}

/// 切换看板视图（页头的「看板 / 列表 / 表格」），返回刷新后的看板。走与设置页同一条写入通路。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_set_board_view(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    view: CreatorBoardViewDto,
) -> Result<CreatorBoardDto, String> {
    persist_setting(
        &app,
        &state,
        BOARD_VIEW_KEY,
        SettingValueDto::Str(view.key().to_owned()),
    )?;
    board_for(&state, &window)
}

/// 创作空间设置的写回出口：落设置 + 副作用 + 广播（列显隐与视图切换都经这里）。
fn persist_setting(
    app: &tauri::AppHandle,
    state: &AppState,
    key: &str,
    value: SettingValueDto,
) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        key,
        value,
    )?;
    apply_setting_side_effects(app, state, key);
    let _ = SettingsChanged {
        key: key.to_owned(),
    }
    .emit(app);
    Ok(())
}

/// 脚本里的一段（PRD-19-010 / 011）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ScriptSectionDto {
    /// 段标题；`null` = 第一个二级标题之前的开场。
    pub heading: Option<String>,
    /// 段标题所在行（1 起，含 frontmatter，与编辑器大纲同一口径）。
    pub line: u32,
    pub cjk_chars: u32,
    pub words: u32,
    /// 本段的 `broll` 代码块数（画面需求）。
    pub broll_blocks: u32,
    /// 约念多少秒。
    pub seconds: u32,
}

/// 脚本里的一个 B-roll 镜头（PRD-19-026）：`broll` 代码块。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BrollShotDto {
    /// 所在段的段名；开场为 `None`。
    pub section: Option<String>,
    /// 开头围栏所在行（含 frontmatter，1 起，与段的 `line` 同一口径）。
    pub line: u32,
    pub text: String,
    /// 已拍 / 已找到（围栏信息串里的 `done`）。
    pub done: bool,
}

/// 一篇脚本的分段与时长（PRD-19-012）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ScriptOutlineDto {
    pub sections: Vec<ScriptSectionDto>,
    /// 全篇的 B-roll 镜头（按出现顺序，PRD-19-026）。
    pub shots: Vec<BrollShotDto>,
    pub total_seconds: u32,
    /// 估算所用语速：每分钟中文字数 / 英文词数；`calibrated = false` 时界面标「约」并说明是默认值。
    pub cjk_per_minute: f32,
    pub words_per_minute: f32,
    pub calibrated: bool,
    /// 校准用到的样本：多少分钟、几条录音（未校准为 0）。
    pub sample_minutes: f32,
    pub recordings: u32,
}

/// 大纲拖动调段序的结果：编辑器把 `change` 作为一次普通编辑应用（进撤销栈），光标落到被搬那段
/// 标题的新行 `line`。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct ScriptSectionMoveDto {
    pub change: TextChangeDto,
    pub line: u32,
}

/// 搬一段（PRD-19-011）：传编辑器里此刻的全文与两个段标题的行号，拿回一次最小替换。不碰文件——
/// 结果回到编辑器，保存仍走编辑器带基线的保存护栏（ADR-138 ①）。`None` = 行号对不上任何一段
/// （大纲过期）或位置不变，编辑器什么都不做。
#[tauri::command]
#[specta::specta]
pub(crate) fn script_move_section(
    content: String,
    from_line: u32,
    before_line: Option<u32>,
) -> Option<ScriptSectionMoveDto> {
    let (moved, line) = luna_vault::move_section(&content, from_line, before_line)?;
    Some(ScriptSectionMoveDto {
        change: luna_vault::frontmatter_edit::text_change(&content, &moved).into(),
        line,
    })
}

/// 勾选 / 取消一个 B-roll 镜头（PRD-19-026）：传编辑器里此刻的全文与镜头的围栏行，拿回一次最小
/// 替换（改那一行的信息串 `broll` ⇄ `broll done`）。与搬段同一分工：不碰文件，结果回到编辑器作为
/// 一次普通编辑（可撤销）。`None` = 那一行不是 `broll` 围栏（大纲过期）或本来就是这个状态。
#[tauri::command]
#[specta::specta]
pub(crate) fn script_set_broll_done(
    content: String,
    line: u32,
    done: bool,
) -> Option<TextChangeDto> {
    let changed = luna_vault::set_broll_done(&content, line, done)?;
    Some(luna_vault::frontmatter_edit::text_change(&content, &changed).into())
}

/// 镜头清单的 CSV（纯函数）：表头一行，每个镜头一行（序号、段落、镜头、完成）；带 UTF-8 BOM，
/// 表格软件直接打开不乱码；字段里有逗号、引号、换行时按 RFC 4180 加引号。
fn broll_csv(shots: &[BrollShotDto], headings: [&str; 4], opening: &str, yes: &str) -> String {
    let field = |value: &str| {
        if value.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_owned()
        }
    };
    let mut out = String::from("\u{feff}");
    out.push_str(&headings.map(field).join(","));
    out.push_str("\r\n");
    for (index, shot) in shots.iter().enumerate() {
        let row = [
            (index + 1).to_string(),
            field(shot.section.as_deref().unwrap_or(opening)),
            field(&shot.text),
            if shot.done {
                yes.to_owned()
            } else {
                String::new()
            },
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

/// 把镜头清单导出成 CSV 交给剪辑（PRD-19-026）：写到项目文件夹里的 `<项目>-镜头清单.csv`（已有就
/// 覆盖），返回它的库相对路径（界面随即在系统文件管理器中显示）。镜头取自编辑器此刻的大纲
/// （与勾选同一份），不再读一遍可能还没保存的文件。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_export_broll(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    shots: Vec<BrollShotDto>,
) -> Result<String, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    let csv = broll_csv(
        &shots,
        [
            &text("creator-broll-col-index", "序号"),
            &text("creator-broll-col-section", "段落"),
            &text("creator-broll-col-shot", "镜头"),
            &text("creator-broll-col-done", "完成"),
        ],
        &text("creator-broll-opening", "开场"),
        &text("creator-broll-yes", "是"),
    );
    let project = note_stem_of(&home);
    let name = format!("{project}-{}.csv", text("creator-broll-file", "镜头清单"));
    let rel = match home.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}"),
        None => name,
    };
    state.with_vault(window.label(), |vault| {
        std::fs::write(vault.root().join(&rel), csv)
            .map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))
    })?;
    Ok(rel)
}

/// 脚本分段与口播时长：传编辑器里此刻的全文（不读正文、不写盘），语速取这个库保存的
/// 本人校准值（PRD-19-012），没校准或库没开时用默认值。
#[tauri::command]
#[specta::specta]
pub(crate) fn script_outline(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    content: String,
) -> ScriptOutlineDto {
    let saved = state
        .with_vault(window.label(), |vault| {
            Ok(vault.saved_speaking_rate().ok().flatten())
        })
        .ok()
        .flatten();
    let rate = saved.as_ref().map_or(
        luna_vault::SpeakingRate::DEFAULT,
        luna_vault::SavedSpeakingRate::rate,
    );
    let outline = luna_vault::script_outline(&content, rate);
    ScriptOutlineDto {
        shots: luna_vault::broll_shots(&content)
            .into_iter()
            .map(|shot| BrollShotDto {
                section: shot.section,
                line: shot.line,
                text: shot.text,
                done: shot.done,
            })
            .collect(),
        sections: outline
            .sections
            .into_iter()
            .map(|section| ScriptSectionDto {
                heading: section.heading,
                line: section.line,
                cjk_chars: section.cjk_chars,
                words: section.words,
                broll_blocks: section.broll_blocks,
                seconds: section.seconds,
            })
            .collect(),
        total_seconds: outline.total_seconds,
        cjk_per_minute: outline.rate.cjk_per_minute,
        words_per_minute: outline.rate.words_per_minute,
        calibrated: outline.rate.calibrated,
        sample_minutes: saved.as_ref().map_or(0.0, |s| s.sample_minutes),
        recordings: saved.as_ref().map_or(0, |s| s.recordings),
    }
}

/// 一次语速校准的结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpeakingRateCalibrationDto {
    /// 已校准并保存。某一类样本不够时那一类为 `null`（沿用默认值）。
    Calibrated {
        cjk_per_minute: Option<f32>,
        words_per_minute: Option<f32>,
        sample_minutes: f32,
        recordings: u32,
    },
    /// 录音里属于用户本人的话还不够（每类至少一分钟），没有保存任何东西。
    NotEnough { minutes: f32 },
}

/// 校准结果所在的库内元数据文件（变更广播用，与 `.luna/graph_scope.json` 同一做法）。
const SPEAKING_RATE_META: &str = ".luna/speaking_rate.json";

/// 用这个库里最近的本机录音校准口播语速（PRD-19-012）。读最多 200 条录音的分段，放在阻塞
/// 线程池里跑，不占界面线程；样本够才保存，只存聚合出的数字。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_calibrate_speaking_rate(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<SpeakingRateCalibrationDto, String> {
    let label = window.label().to_owned();
    let worker = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = worker.state::<AppState>();
        state.with_vault(&label, |vault| {
            let sample = vault.measure_speaking_rate().map_err(vault_err)?;
            match sample.to_saved(chrono::Local::now().to_rfc3339()) {
                Some(saved) => {
                    vault.save_speaking_rate(&saved).map_err(vault_err)?;
                    Ok(SpeakingRateCalibrationDto::Calibrated {
                        cjk_per_minute: saved.cjk_per_minute,
                        words_per_minute: saved.words_per_minute,
                        sample_minutes: saved.sample_minutes,
                        recordings: saved.recordings,
                    })
                }
                None => Ok(SpeakingRateCalibrationDto::NotEnough {
                    minutes: sample.minutes(),
                }),
            }
        })
    })
    .await
    .map_err(|_| "CREATOR_CALIBRATE_FAILED".to_owned())??;
    if matches!(outcome, SpeakingRateCalibrationDto::Calibrated { .. }) {
        emit_vault_changed_in_scope(
            &app,
            SPEAKING_RATE_META,
            false,
            VaultChangeScope::for_window(window.label()),
        );
    }
    Ok(outcome)
}

/// 清掉校准，回到默认语速。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_reset_speaking_rate(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.with_vault(window.label(), |vault| {
        vault.clear_speaking_rate().map_err(vault_err)
    })?;
    emit_vault_changed_in_scope(
        &app,
        SPEAKING_RATE_META,
        false,
        VaultChangeScope::for_window(window.label()),
    );
    Ok(())
}

/// 项目页头的「下一步」（ADR-165 ⑥）：阶段到动作的映射在 `luna_vault::next_action`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NextActionDto {
    WriteIdea,
    ImportMaterial,
    OpenTeleprompter,
    StartFollow,
    ImportFinalCut,
    OpenPublish,
    Distribute,
}

impl From<NextAction> for NextActionDto {
    fn from(value: NextAction) -> Self {
        match value {
            NextAction::WriteIdea => Self::WriteIdea,
            NextAction::ImportMaterial => Self::ImportMaterial,
            NextAction::OpenTeleprompter => Self::OpenTeleprompter,
            NextAction::StartFollow => Self::StartFollow,
            NextAction::ImportFinalCut => Self::ImportFinalCut,
            NextAction::OpenPublish => Self::OpenPublish,
            NextAction::Distribute => Self::Distribute,
        }
    }
}

/// 项目工作区（ADR-165 ⑥ / PRD-19-037）：页头与页签要的一切，一次取齐。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectWorkspaceDto {
    /// 项目主页的库相对路径（概览页签里就地编辑的那一篇）。
    pub home: String,
    /// 项目名（主页文件名）。
    pub title: String,
    /// 所处阶段。
    pub stage: ProjectStageDto,
    /// 阶段步进条（七个阶段，按看板默认列序）。
    pub stages: Vec<ProjectStageDto>,
    /// 研究笔记（不在了为 `null`，研究页签随之不出现）。
    pub research: Option<String>,
    /// 脚本（骨架那篇，或项目文件夹里最近改过的一篇脚本）。
    pub script: Option<String>,
    /// 这个阶段的主动作；已发布暂无。
    pub next_action: Option<NextActionDto>,
    /// 要发的渠道 id（有序；没写过 = 默认渠道）。
    pub channels: Vec<String>,
    /// 计划发布日期（ISO `YYYY-MM-DD`）；没排期为 `None`。
    pub publish_date: Option<String>,
    /// 已归档（页头写明，「更多」菜单给「取消归档」）。
    pub archived: bool,
    /// 阶段推进提示：这个阶段的活看起来做完了时建议进入的下一个阶段（只提示、不替用户改）；
    /// 没有为 `None`。
    pub stage_hint: Option<ProjectStageDto>,
}

/// 判阶段推进提示要的几件事实（[`stage_hint`] 是纯函数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StageFacts {
    /// 主页「选题」一节写了（不算骨架里那句提示）。
    idea_written: bool,
    quotes: usize,
    /// 脚本按语速估的时长（秒）；没有脚本为 `None`。
    script_seconds: Option<u32>,
    /// 项目文件夹里有已转写的录音 / 成片。
    has_takes: bool,
    /// 发布清单每一步都做完、没有待改处。
    checklist_complete: bool,
}

/// 脚本估到这么长才算写出了一稿（再短多半还是骨架和几句笔记）。
const DRAFT_SCRIPT_SECONDS: u32 = 30;

/// 阶段推进提示（ADR-165 ⑥ 评审遗留 S6c）：选题写了 → 研究；有引文 → 撰稿；脚本估出一稿 →
/// 录制；有录音 → 剪辑；有成片 → 发布；发布清单全绿 → 已发布。阶段只由用户改，这里只建议。
fn stage_hint(stage: ProjectStage, facts: StageFacts) -> Option<ProjectStage> {
    let ready = match stage {
        ProjectStage::Idea => facts.idea_written,
        ProjectStage::Research => facts.quotes > 0,
        ProjectStage::Writing => facts
            .script_seconds
            .is_some_and(|secs| secs >= DRAFT_SCRIPT_SECONDS),
        ProjectStage::Recording | ProjectStage::Editing => facts.has_takes,
        ProjectStage::Publishing => facts.checklist_complete,
        ProjectStage::Published => false,
    };
    let next = ProjectStage::ALL
        .iter()
        .position(|candidate| *candidate == stage)
        .and_then(|index| ProjectStage::ALL.get(index + 1).copied());
    next.filter(|_| ready)
}

/// 项目骨架里一篇子笔记的库相对路径：`<项目文件夹>/<项目>-<后缀>.md`（与 [`project_skeleton`]
/// 同一命名，后缀随界面语言）。
pub(super) fn project_child_rel(
    home: &str,
    copy: BackendCopy<'_>,
    key: &str,
    fallback: &str,
) -> String {
    let project = note_stem_of(home);
    let suffix = backend_text_with(copy, key, fallback);
    match home.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{project}-{suffix}.md"),
        None => format!("{project}-{suffix}.md"),
    }
}

/// 打开一个项目（项目工作区的数据）。
///
/// # Errors
/// 库未打开、`home` 不是视频项目主页时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_project(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<ProjectWorkspaceDto, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let research_rel = project_child_rel(&home, copy, "creator-note-research", "研究");
    let script_rel = project_child_rel(&home, copy, "creator-note-script", "脚本");
    state.with_vault(window.label(), |vault| {
        let stage = vault.project_stage(&home).map_err(vault_err)?;
        let channels = vault.project_channels(&home).map_err(vault_err)?;
        let publish_date = vault.project_publish_date(&home).map_err(vault_err)?;
        let script = vault
            .project_script(&home, &script_rel)
            .map_err(vault_err)?;
        let research = vault.note_exists(&research_rel).then_some(research_rel);
        let home_doc = vault.read(&home).map_err(vault_err)?;
        let archived = home_doc.meta.project_archived;
        let idea_heading = backend_text_with(copy, "creator-section-idea", "选题");
        let idea_hint = backend_text_with(
            copy,
            "creator-section-idea-hint",
            "一句话说清这期视频讲什么、讲给谁。",
        );
        let checklist = super::creator_publish::publish_checklist_of(vault, &home, copy).ok();
        let facts = StageFacts {
            idea_written: luna_vault::section_body(&home_doc.content, &idea_heading).is_some_and(
                |body| {
                    let body = body.trim();
                    !body.is_empty() && body != idea_hint
                },
            ),
            quotes: super::creator_quote::project_quote_count(vault, &home, copy),
            script_seconds: script
                .as_ref()
                .and_then(|path| vault.read(path).ok())
                .map(|doc| {
                    luna_vault::script_outline(&doc.content, vault.speaking_rate()).total_seconds
                }),
            has_takes: vault
                .project_publish_sources(&home)
                .is_ok_and(|sources| !sources.is_empty()),
            checklist_complete: checklist.as_ref().is_some_and(|list| {
                list.total > 0 && list.done == list.total && list.checks.is_empty()
            }),
        };
        Ok(ProjectWorkspaceDto {
            title: note_stem_of(&home).to_owned(),
            home: home.clone(),
            stage: stage.into(),
            stages: ProjectStage::ALL.into_iter().map(Into::into).collect(),
            research,
            script,
            next_action: next_action(stage).map(Into::into),
            channels,
            publish_date: publish_date
                .map(|date| date.format(luna_vault::PUBLISH_DATE_FORMAT).to_string()),
            archived,
            stage_hint: stage_hint(stage, facts).map(Into::into),
        })
    })
}

/// 改项目要发的渠道（有序；发布清单随之增减步骤，ADR-166）。至少留一个，空集合不写。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_set_channels(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    channels: Vec<String>,
) -> Result<(), String> {
    let written = state.with_vault(window.label(), |vault| {
        vault
            .set_project_channels(&home, &channels)
            .map_err(vault_err)
    })?;
    if let Some(content) = written {
        broadcast_note_write(&app, window.label(), &home, content);
    }
    Ok(())
}

/// 录制前检查（ADR-165 ⑧ / PRD-19-039）：跟读要的麦克风与实时转写，各一条状态；缺什么
/// 带着与「添加内容」同一套修复动作。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RecordingCheckDto {
    pub microphone: super::create_actions::CreateActionStateDto,
    pub transcriber: super::create_actions::CreateActionStateDto,
}

/// 开录前检查：只读，不弹授权、不装载模型、不联网探活。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_recording_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> RecordingCheckDto {
    let (microphone, transcriber) = super::create_actions::recording_readiness(&app, &state);
    RecordingCheckDto {
        microphone,
        transcriber,
    }
}

/// 脚本里已写回的一处录制标记。
#[derive(Debug, Clone, Serialize, Type)]
pub struct RetakeNoteDto {
    /// 被标的那一行（1 起，含 frontmatter）。
    pub line: u32,
    pub text: String,
    pub note: String,
}

/// 一篇脚本里已写回的录制标记（录制页签列出，点一条回到脚本那一行）。
///
/// # Errors
/// 库未打开或脚本读不出时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_script_retakes(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<RetakeNoteDto>, String> {
    let doc = state.with_vault(window.label(), |vault| vault.read(&path).map_err(vault_err))?;
    Ok(luna_vault::retake_notes(&doc.content)
        .into_iter()
        .map(|note| RetakeNoteDto {
            line: note.line,
            text: note.text,
            note: note.note,
        })
        .collect())
}

/// 一篇笔记所属的项目（主页与项目名）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectRefDto {
    pub home: String,
    pub title: String,
}

/// 这篇笔记属于哪个项目（「属于」与引文、项目概览同一判定：在项目文件夹里，或与主页互有
/// 链接；属于多个时取最近改动的那个）；不属于任何项目为 `null`。供「笔记」空间里那条
/// 「在创作中打开」横幅用（ADR-165 ⑦）。
///
/// # Errors
/// 库未打开、索引查询失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_project_of(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Option<ProjectRefDto>, String> {
    state.with_vault(window.label(), |vault| {
        let projects = vault.projects_of(&path).map_err(vault_err)?;
        Ok(projects.into_iter().next().map(|card| ProjectRefDto {
            title: note_stem_of(&card.path).to_owned(),
            home: card.path,
        }))
    })
}

/// 新建项目时一并给定的几样（新建项目对话框，ADR-165 评审遗留 S6c）：发到哪些渠道（`None` =
/// 设置里的默认渠道）与计划发布日期（ISO `YYYY-MM-DD`，`None` = 不排期）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct NewProjectOptionsDto {
    pub channels: Option<Vec<String>>,
    pub publish_date: Option<String>,
}

/// 新建一个视频项目，返回项目主页。`title` 为空时按「视频项目_时间」命名。
///
/// # Errors
/// 创作未启用、日期写法不对、库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_create_project(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    title: Option<String>,
    options: NewProjectOptionsDto,
) -> Result<NoteDocDto, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let publish_date = match options.publish_date.as_deref() {
        None | Some("") => None,
        Some(raw) => Some(
            luna_vault::resolve_publish_date(Some(raw))
                .ok_or_else(|| coded_error(&app, "CREATOR_BAD_DATE", "creator-bad-date"))?,
        ),
    };
    create_project(
        &app,
        &window,
        &state,
        title.as_deref(),
        ProjectSeed::Blank,
        &ProjectOptions {
            channels: options.channels,
            publish_date,
        },
    )
}

/// 新建一个填好内容的示例项目（启用创作后的入门第三步，ADR-165 ⑥）：每个阶段都已有一点内容——
/// 选题一句话、研究笔记的来源与引文、分好段带录制标记的脚本、发布笔记的章节与标题——停在
/// 「撰稿」，计划一周后发布。文字随界面语言；返回项目主页。
///
/// # Errors
/// 创作未启用、库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_create_example_project(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<NoteDocDto, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let i18n = backend_copy_arc(&app);
    let title = backend_text_with(
        i18n.as_deref(),
        "creator-example-title",
        "示例：笔记本跑大模型",
    );
    create_project(
        &app,
        &window,
        &state,
        Some(&title),
        ProjectSeed::Example,
        &ProjectOptions::default(),
    )
}

/// 新建时一并定下的渠道与日期（[`NewProjectOptionsDto`] 校验之后）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectOptions {
    pub channels: Option<Vec<String>>,
    pub publish_date: Option<chrono::NaiveDate>,
}

/// 新项目从什么开始：空骨架，或填好内容的示例。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSeed {
    Blank,
    Example,
}

/// 新建项目的唯一实现：创作空间的「新建项目」与模板选择器里的「视频项目」都走这里——
/// 视频项目永远是一个文件夹，不会有一篇孤零零的主页（ADR-162 ②）。
pub(crate) fn create_project(
    app: &tauri::AppHandle,
    window: &tauri::Window,
    state: &AppState,
    title: Option<&str>,
    seed: ProjectSeed,
    options: &ProjectOptions,
) -> Result<NoteDocDto, String> {
    let change_scope = VaultChangeScope::for_window(window.label());
    let settings = state.settings_snapshot();
    // 新项目发到哪些渠道：对话框里选的，否则「我的渠道」里标了默认的；与默认渠道相同就不写键
    // （缺省即默认）。对话框给的 id 在下面按目录归一化（认不出的丢掉，一个都不剩时回到默认）。
    let i18n = backend_copy_arc(app);
    let copy = i18n.as_deref();
    let lang = i18n
        .as_ref()
        .map_or_else(|| "zh-CN".to_string(), |i| i.language());
    let stem = title.and_then(sanitized_file_stem).unwrap_or_else(|| {
        empty_note_rel_path(NoteType::VideoProject, &lang)
            .map(|name| name.trim_end_matches(".md").to_owned())
            .unwrap_or_else(|_| "video".to_owned())
    });
    let parent = note_type_dir(&settings, NoteType::VideoProject);
    // 新项目从看板上第一个显示的列开始：用户收起了「选题」时，它不该落进一列看不见的地方。
    // 示例项目停在「撰稿」：选题与研究已填好，脚本是主角。
    let initial = match seed {
        ProjectSeed::Blank => first_visible_stage(&board_layout(state)),
        ProjectSeed::Example => ProjectStage::Writing,
    };

    let (doc, created) = state.with_vault(window.label(), |vault| {
        for index in 0..100 {
            let name = if index == 0 {
                stem.clone()
            } else {
                format!("{stem}-{index}")
            };
            let folder = format!("{parent}/{name}");
            let skeleton = match seed {
                ProjectSeed::Blank => project_skeleton(copy, &name),
                ProjectSeed::Example => example_skeleton(copy, &name),
            };
            match vault.create_video_project(
                &folder,
                &name,
                &skeleton.home_body,
                &skeleton.children,
            ) {
                Ok(mut doc) => {
                    if initial != ProjectStage::Idea {
                        vault
                            .set_project_stage(&doc.path, initial)
                            .map_err(vault_err)?;
                    }
                    let channels = match &options.channels {
                        Some(picked) => luna_vault::channel::resolve_channels(
                            picked,
                            &vault.custom_channels().unwrap_or_default(),
                        ),
                        None => vault.default_project_channels(),
                    };
                    if channels != luna_vault::channel::DEFAULT_CHANNELS {
                        vault
                            .set_project_channels(&doc.path, &channels)
                            .map_err(vault_err)?;
                    }
                    if options.publish_date.is_some() {
                        vault
                            .set_project_publish_date(&doc.path, options.publish_date)
                            .map_err(vault_err)?;
                    }
                    let mut created = vec![doc.path.clone()];
                    if seed == ProjectSeed::Example {
                        let week_later = chrono::Local::now().date_naive() + chrono::Days::new(7);
                        vault
                            .set_project_publish_date(&doc.path, Some(week_later))
                            .map_err(vault_err)?;
                        let publish_rel = super::creator_publish::publish_note_rel(&doc.path, copy);
                        vault
                            .write(&publish_rel, &example_publish_note(copy))
                            .map_err(vault_err)?;
                        created.push(publish_rel);
                    }
                    doc = vault.read(&doc.path).map_err(vault_err)?;
                    created.extend(
                        skeleton
                            .children
                            .iter()
                            .map(|child| format!("{folder}/{}.md", child.stem)),
                    );
                    return Ok((doc, created));
                }
                Err(err) if luna_vault::creator::is_already_exists(&err) => continue,
                Err(err) => return Err(vault_err(err)),
            }
        }
        Err(coded_error_with(
            copy,
            "VAULT_CREATE_EXHAUSTED",
            "vault-create-exhausted",
        ))
    })?;
    emit_vault_changes_in_scope(
        app,
        created.into_iter().map(|path| (path, false)),
        change_scope,
    );
    Ok(doc.into())
}

/// 项目骨架：主页正文与两篇子笔记（研究、脚本）。
///
/// Markdown 结构（标题级别、链接）在这里，**每一句给人读的文字都取自 Fluent**（TDD-07 §2.1）：
/// 英文界面的用户新建项目，得到的是英文骨架。子笔记名带项目名前缀，免得各个项目里的
/// `[[脚本]]` 互相指错（链接按文件名解析）。
struct ProjectSkeleton {
    home_body: String,
    children: Vec<ProjectChild>,
}

fn project_skeleton(copy: BackendCopy<'_>, project: &str) -> ProjectSkeleton {
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    let research = format!("{project}-{}", text("creator-note-research", "研究"));
    let script = format!("{project}-{}", text("creator-note-script", "脚本"));
    let home_body = format!(
        "## {idea}\n\n{idea_hint}\n\n## {research_heading}\n\n- [[{research}]]\n\n## {script_heading}\n\n- [[{script}]]\n",
        idea = text("creator-section-idea", "选题"),
        idea_hint = text(
            "creator-section-idea-hint",
            "一句话说清这期视频讲什么、讲给谁。"
        ),
        research_heading = text("creator-section-research", "资料"),
        script_heading = text("creator-section-script", "脚本"),
    );
    let research_body = format!(
        "## {}\n\n## {}\n",
        text("creator-research-sources", "来源"),
        text("creator-research-quotes", "引文")
    );
    let script_body = format!(
        "## {}\n\n## {}\n\n## {}\n",
        text("creator-script-hook", "开头"),
        copy.and_then(|i18n| i18n.translate_args("creator-script-beat", &[("n", "1".to_owned())]))
            .unwrap_or_else(|| "节拍 1".to_owned()),
        text("creator-script-outro", "结尾"),
    );
    ProjectSkeleton {
        home_body,
        children: vec![
            ProjectChild {
                stem: research,
                note_type: NoteType::Note,
                body: research_body,
            },
            ProjectChild {
                stem: script,
                note_type: NoteType::Script,
                body: script_body,
            },
        ],
    }
}

/// 示例项目的文字（ADR-165 ⑥）：随客户端打包的 Markdown，按界面语言取一份；标题与小节名留
/// `{{…}}` 占位，由这里填进与各功能读取时同一份 Fluent 词（研究的「来源 / 引文」、发布笔记的
/// 「章节 / 标题备选」……），换了界面语言也对得上。
struct ExampleText {
    home: &'static str,
    research: &'static str,
    script: &'static str,
    publish: &'static str,
}

fn example_text(copy: BackendCopy<'_>) -> ExampleText {
    let english = copy.is_some_and(|i18n| i18n.language().starts_with("en"));
    if english {
        ExampleText {
            home: include_str!("../../resources/creator-example/en-US/home.md"),
            research: include_str!("../../resources/creator-example/en-US/research.md"),
            script: include_str!("../../resources/creator-example/en-US/script.md"),
            publish: include_str!("../../resources/creator-example/en-US/publish.md"),
        }
    } else {
        ExampleText {
            home: include_str!("../../resources/creator-example/zh-CN/home.md"),
            research: include_str!("../../resources/creator-example/zh-CN/research.md"),
            script: include_str!("../../resources/creator-example/zh-CN/script.md"),
            publish: include_str!("../../resources/creator-example/zh-CN/publish.md"),
        }
    }
}

/// 把 `{{名字}}` 换成给定的词；没给的占位原样留下（测试据此钉住「没有漏填」）。
fn fill(template: &str, values: &[(&str, String)]) -> String {
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{{{key}}}}}"), value)
        })
}

/// 示例项目的骨架：与空项目同样的三篇（主页、研究、脚本），正文换成填好的示例。
fn example_skeleton(copy: BackendCopy<'_>, project: &str) -> ProjectSkeleton {
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    let blank = project_skeleton(copy, project);
    let example = example_text(copy);
    let values = [
        ("research", blank.children[0].stem.clone()),
        ("script", blank.children[1].stem.clone()),
        ("h_idea", text("creator-section-idea", "选题")),
        ("h_research", text("creator-section-research", "资料")),
        ("h_script", text("creator-section-script", "脚本")),
        ("h_sources", text("creator-research-sources", "来源")),
        ("h_quotes", text("creator-research-quotes", "引文")),
        ("h_hook", text("creator-script-hook", "开头")),
        ("h_outro", text("creator-script-outro", "结尾")),
    ];
    let mut children = blank.children;
    children[0].body = fill(example.research, &values);
    children[1].body = fill(example.script, &values);
    ProjectSkeleton {
        home_body: fill(example.home, &values),
        children,
    }
}

/// 示例项目的发布笔记（章节、链接、YouTube 包装的标题备选与简介；小节名与发布页签读的是同一份词）。
fn example_publish_note(copy: BackendCopy<'_>) -> String {
    let text = |key: &str, fallback: &str| backend_text_with(copy, key, fallback);
    fill(
        example_text(copy).publish,
        &[
            ("h_chapters", text("creator-publish-chapters", "章节")),
            ("h_links", text("creator-publish-links", "链接")),
            (
                "h_package",
                super::creator_publish::builtin_package_heading("youtube", copy),
            ),
            ("h_titles", text("creator-publish-titles", "标题备选")),
            ("h_description", text("creator-publish-description", "简介")),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broll_csv_has_a_bom_headers_and_quotes_fields_that_need_it() {
        let shots = vec![
            BrollShotDto {
                section: None,
                line: 3,
                text: "街景".into(),
                done: true,
            },
            BrollShotDto {
                section: Some("第一段".into()),
                line: 9,
                text: "键盘特写, 慢镜\n屏幕录制".into(),
                done: false,
            },
        ];
        let csv = broll_csv(&shots, ["序号", "段落", "镜头", "完成"], "开场", "是");
        assert_eq!(
            csv,
            "\u{feff}序号,段落,镜头,完成\r\n1,开场,街景,是\r\n2,第一段,\"键盘特写, 慢镜\n屏幕录制\",\r\n"
        );
    }

    #[test]
    fn stage_hints_only_suggest_the_next_stage_once_its_work_is_done() {
        let none = StageFacts::default();
        for stage in ProjectStage::ALL {
            assert_eq!(
                stage_hint(stage, none),
                None,
                "{stage:?} 什么都没做时不提示"
            );
        }
        let idea = StageFacts {
            idea_written: true,
            ..none
        };
        assert_eq!(
            stage_hint(ProjectStage::Idea, idea),
            Some(ProjectStage::Research)
        );
        assert_eq!(
            stage_hint(ProjectStage::Research, StageFacts { quotes: 2, ..none }),
            Some(ProjectStage::Writing)
        );
        assert_eq!(
            stage_hint(
                ProjectStage::Writing,
                StageFacts {
                    script_seconds: Some(DRAFT_SCRIPT_SECONDS - 1),
                    ..none
                }
            ),
            None,
            "脚本还太短"
        );
        assert_eq!(
            stage_hint(
                ProjectStage::Writing,
                StageFacts {
                    script_seconds: Some(DRAFT_SCRIPT_SECONDS),
                    ..none
                }
            ),
            Some(ProjectStage::Recording)
        );
        let takes = StageFacts {
            has_takes: true,
            ..none
        };
        assert_eq!(
            stage_hint(ProjectStage::Recording, takes),
            Some(ProjectStage::Editing)
        );
        assert_eq!(
            stage_hint(ProjectStage::Editing, takes),
            Some(ProjectStage::Publishing)
        );
        let done = StageFacts {
            checklist_complete: true,
            ..none
        };
        assert_eq!(
            stage_hint(ProjectStage::Publishing, done),
            Some(ProjectStage::Published)
        );
        assert_eq!(stage_hint(ProjectStage::Published, done), None);
    }

    #[test]
    fn app_prompt_owners_are_first_party_apps_that_declare_prompts() {
        use luna_config::apps::{AppContribution, FIRST_PARTY_APPS};
        for (app, _, _) in luna_core::prompt::APP_TEMPLATES {
            let manifest = FIRST_PARTY_APPS
                .iter()
                .find(|manifest| manifest.id == *app)
                .unwrap_or_else(|| panic!("APP_TEMPLATES 里的 {app} 不是一方应用"));
            assert!(
                manifest.contributions.contains(&AppContribution::Prompts),
                "{app} 带了自带提示词，详情页却没写「AI 提示词」这一项"
            );
        }
        for manifest in FIRST_PARTY_APPS {
            if manifest.contributions.contains(&AppContribution::Prompts) {
                assert!(
                    luna_core::prompt::APP_TEMPLATES
                        .iter()
                        .any(|(app, _, _)| *app == manifest.id),
                    "{} 声明了提示词贡献却没有自带的模板",
                    manifest.id
                );
            }
        }
    }

    #[test]
    fn example_text_fills_every_placeholder_and_the_quotes_are_real_cards() {
        for language in ["zh-CN", "en-US"] {
            let i18n = crate::i18n::BackendI18n::new(language);
            let copy = Some(&i18n);
            let skeleton = example_skeleton(copy, "P");
            let publish = example_publish_note(copy);
            for text in [
                &skeleton.home_body,
                &skeleton.children[0].body,
                &skeleton.children[1].body,
                &publish,
            ] {
                assert!(!text.contains("{{"), "{language} 漏填占位：{text}");
            }
            assert!(skeleton
                .home_body
                .contains(&format!("[[{}]]", skeleton.children[0].stem)));
            assert_eq!(
                luna_vault::quote::quote_cards(&skeleton.children[0].body).len(),
                3,
                "{language}"
            );
            assert_eq!(
                luna_vault::retake_notes(&skeleton.children[1].body).len(),
                1,
                "{language}"
            );
            let outline = luna_vault::script_outline(
                &skeleton.children[1].body,
                luna_vault::SpeakingRate::DEFAULT,
            );
            assert!(outline.sections.len() >= 4, "{language}");
            let chapters = backend_text_with(copy, "creator-publish-chapters", "章节");
            let body = luna_vault::section_body(&publish, &chapters).unwrap();
            assert!(
                luna_core::chapters::check_chapters(
                    &luna_core::chapters::parse_chapters(&body).0,
                    None
                )
                .is_empty(),
                "{language} 章节要合规"
            );
        }
    }

    #[test]
    fn board_cards_carry_date_channels_and_signals() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let write = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Creator/P/P.md",
            "---\nluna:\n  type: video-project\n  project_stage: writing\n  project_channels:\n  - youtube\n  project_publish_date: 2026-10-24\n---\n正文\n",
        );
        write(
            "Creator/P/P-脚本.md",
            "---\nluna:\n  type: script\n---\n## 开头\n大家好，今天聊统一内存。\n",
        );
        write(
            "Creator/P/P-研究.md",
            "## 引文\n\n> 带宽比算力先撞墙 ^q-abc123\n> 第二条 ^q-def456\n",
        );
        vault.rebuild_index().unwrap();
        let board = board_of(
            &vault,
            &[(ProjectStage::Writing, true)],
            CreatorBoardViewDto::Calendar,
            None,
        )
        .unwrap();
        let card = &board.projects[0];
        assert_eq!(card.publish_date.as_deref(), Some("2026-10-24"));
        assert_eq!(card.channels, ["youtube"]);
        assert_eq!(card.signals.quotes, 2);
        assert!(card.signals.script_seconds.is_some_and(|secs| secs > 0));
        // 只发 YouTube：成片 · 字幕 · 章节 · YouTube · 包装 · 检查。
        assert_eq!(
            (card.signals.publish_done, card.signals.publish_total),
            (0, 5)
        );
        assert_eq!(
            card.signals.next_action,
            Some(NextActionDto::OpenTeleprompter)
        );
    }

    #[test]
    fn stage_dto_mirrors_every_stage_in_board_order() {
        let order: Vec<ProjectStageDto> = ProjectStage::ALL.into_iter().map(Into::into).collect();
        assert_eq!(
            order,
            [
                ProjectStageDto::Idea,
                ProjectStageDto::Research,
                ProjectStageDto::Writing,
                ProjectStageDto::Recording,
                ProjectStageDto::Editing,
                ProjectStageDto::Publishing,
                ProjectStageDto::Published,
            ]
        );
        for stage in ProjectStage::ALL {
            assert_eq!(ProjectStage::from(ProjectStageDto::from(stage)), stage);
        }
    }

    #[test]
    fn board_views_match_the_setting_options_and_bad_values_fall_back_to_the_board() {
        let views = [
            CreatorBoardViewDto::Board,
            CreatorBoardViewDto::List,
            CreatorBoardViewDto::Table,
            CreatorBoardViewDto::Calendar,
        ];
        let keys: Vec<&str> = views.iter().map(|v| v.key()).collect();
        assert_eq!(keys, luna_config::creator_board::CREATOR_BOARD_VIEWS);
        for view in views {
            assert_eq!(CreatorBoardViewDto::from_setting(Some(view.key())), view);
        }
        assert_eq!(
            CreatorBoardViewDto::from_setting(Some("timeline")),
            CreatorBoardViewDto::Board
        );
        assert_eq!(
            CreatorBoardViewDto::from_setting(None),
            CreatorBoardViewDto::Board
        );
    }

    #[test]
    fn the_settings_vocabulary_is_exactly_the_project_stages_in_order() {
        // 两份阶段清单必然要漂：设置注册表用 `luna-config` 那份（它不依赖 `luna-vault`），
        // 判定用 `ProjectStage`。少一个，那一列就永远配不出来，而没有任何东西会变红。
        let keys: Vec<&str> = ProjectStage::ALL
            .into_iter()
            .map(ProjectStage::key)
            .collect();
        assert_eq!(keys, luna_config::creator_board::CREATOR_STAGE_IDS);
    }

    #[test]
    fn the_skeleton_links_the_home_page_to_its_own_notes() {
        let skeleton = project_skeleton(None, "第一期");
        assert!(skeleton.home_body.contains("[[第一期-研究]]"));
        assert!(skeleton.home_body.contains("[[第一期-脚本]]"));
        assert_eq!(skeleton.children[0].stem, "第一期-研究");
        assert_eq!(skeleton.children[1].note_type, NoteType::Script);
        assert!(skeleton.children[1].body.contains("## 开头"));
    }
}
