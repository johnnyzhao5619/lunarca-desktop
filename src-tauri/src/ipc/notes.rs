//! 笔记读写命令域（PRD-06，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：单篇笔记与库元数据的读写、模板建笔记、标签/反链/出链、树与检索。
//! 「笔记即文件」的事实源是 [`luna_vault`]，本模块只做 IPC 面。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// `VaultError` → IPC 错误串：**稳定码 + 载荷**，措辞归前端（ADR-105）。
///
/// # 它此前是 `e.to_string()`
///
/// 那一句把措辞、载荷、语言三样黏成一条中文，从**一百八十个调用点**流进 IPC，而
/// `VaultError::code()`（14 个稳定码）就在旁边、一次都没被用过。代价有两条，都真实：
/// 英文界面照样读到「笔记不存在：…」；前端拿不到码，于是无从按码取本地化文案，只能把
/// 后端那句原样摆上屏幕。`VaultError` 的模块头注写着「携带稳定错误码供 i18n」——纪律在、
/// 实现不在，这是同一族里的第五次（`ListCursorInvalid` 甚至为此手工把码塞进了自己的
/// Display，那个特例随本批撤销）。
///
/// 形状与本仓既有的机器前缀逐字同源（`DISTILL_INELIGIBLE: 说明…`、`DISTILL_JOB: <码>`），
/// 故 `withoutErrorCode` / `taskFailureText` 这些既有渲染口原样认得它。
pub(crate) fn vault_err(e: luna_vault::VaultError) -> String {
    format!("{}: {}", e.code(), e.detail())
}

/// 读取 Vault 元数据（.luna/{name}.json），返回 JSON 字符串数组。
#[tauri::command]
#[specta::specta]
pub(crate) fn read_vault_meta(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<Vec<String>, String> {
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| state.vault_not_open())?
            .root()
            .to_path_buf()
    };
    let values: Vec<serde_json::Value> = read_meta(&root, &name).map_err(vault_err)?;
    values
        .into_iter()
        .map(|v| serde_json::to_string(&v).map_err(|e| e.to_string()))
        .collect()
}

/// 标签树的一次取数（PRD-06-033 / PRD-16-043）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TagTreeDto {
    /// 层级摘要：中间层、去重笔记数与整支来源均由 Rust 给出，前端只接树。
    pub tags: Vec<TagSummaryDto>,
    /// 这个库此刻生成 AI 关系标签吗（`vault.graph_ai_tags`）。
    ///
    /// **关着时「AI 标签」分组整组不出现**（PRD-07-024 / PRD-16-043）——不是渲染一个
    /// 空分组、也不是渲染一个带说明的占位：那是在向明确关掉该能力的用户反复提起它
    /// （与 PRD-16-030「一个字都不提」同一纪律）。判定随树一次下发，前端不去读设置键，
    /// 也就没有"某个面忘了加这个条件"的可能。
    pub ai_tags_enabled: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn tag_summaries(state: tauri::State<'_, AppState>) -> Result<TagTreeDto, String> {
    let ai_tags_enabled = state
        .settings_snapshot()
        .get_bool("vault.graph_ai_tags")
        .unwrap_or(true);
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(TagTreeDto {
        tags: vault
            .tag_summaries()
            .map(|tags| tags.iter().map(Into::into).collect())
            .map_err(vault_err)?,
        ai_tags_enabled,
    })
}

/// 列出指向 `path` 的反向链接笔记路径。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_backlinks(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<NoteRefDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let links = vault.backlinks(&path).map_err(vault_err)?;
    Ok(links.into_iter().map(|p| NoteRefDto { path: p }).collect())
}

/// 列出 `path` 指向的出链，并逐条如实回答「这一篇此刻在不在」。
///
/// `exists` 不是装饰：一条**断链**在面板上和真链接长得一样时，用户只能靠"点一下看报不报错"
/// 来区分——而那正是「点了才失败」。判定在 Rust（前端零业务），断链在面板上呈现为不可点的
/// 「未创建」，与 wikilink 的未解析态同一语言（PRD-16-013）。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_outgoing_links(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<OutgoingLinkDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let links = vault.outgoing_links(&path).map_err(vault_err)?;
    Ok(links
        .into_iter()
        .map(|p| OutgoingLinkDto {
            exists: vault.note_exists(&p),
            path: p,
        })
        .collect())
}

/// 模板目录与其它 Vault 路径设置同源收敛，前端不拼接或校验文件系统路径。
pub(crate) fn user_templates_dir(settings: &Settings) -> String {
    safe_vault_relative_dir(settings, "vault.templates_dir", ".luna/templates")
}

/// 从用户明确选择的模板（库里的或装上的模板包里的，ADR-169 ⑤）创建普通笔记。模板本身只读；
/// 变量渲染、路径校验及最终命名均归 Rust/Vault 边界，避免前端出现第二套命名和路径规则。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_note_from_template(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    template: TemplateFileDto,
) -> Result<NoteDocDto, String> {
    let settings = state.settings_snapshot();
    let source = read_template_file(&app, &state, &template)?;
    let notes_dir = note_type_dir(&settings, NoteType::Note);
    let i18n = backend_copy_arc(&app);
    let lang = i18n
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string());
    let rel_path = format!(
        "{notes_dir}/{}",
        empty_note_rel_path(NoteType::Note, &lang)?
    );
    let now = chrono::Local::now();
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let doc = create_unique_note_with(state.copy(), vault, &rel_path, NoteType::Note, |title| {
        // 新建时 `{{cursor}}` 只是被删掉：打开新笔记时光标的落点由编辑器自己定。
        luna_vault::render_user_template(
            &source,
            &UserTemplateVars {
                title: title.to_owned(),
                now: now.fixed_offset(),
            },
        )
        .text
    })?;
    emit_vault_changed(&app, &doc.path, false);
    Ok(doc.into())
}

/// 写入 Vault 元数据（.luna/{name}.json），原子写入并触发 vaultChanged 事件。
/// `value` 为 JSON 字符串数组，内部反序列化为 `serde_json::Value` 后写入。
#[tauri::command]
#[specta::specta]
pub(crate) fn write_vault_meta(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
    value: Vec<String>,
) -> Result<(), String> {
    let root = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| state.vault_not_open())?
            .root()
            .to_path_buf()
    };
    let entries: Vec<serde_json::Value> = value
        .iter()
        .map(|s| serde_json::from_str(s).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    write_meta(&root, &name, &entries).map_err(vault_err)?;
    emit_vault_changed(
        &app,
        &format!("{LUNA_VAULT_PRIVATE_DIR}/{name}.json"),
        false,
    );
    Ok(())
}

/// 某个笔记类型的归档目录（Vault 相对路径）。
///
/// 功能视图列表（速记 / 录音 / 转写 / AI 对话）按文件夹分组呈现时要裁掉这层前缀，
/// 否则整棵树只有一个根节点、每次都要多展开一层（`PRD-07-062`）。
/// **判定仍在 Rust**（ADR-003）：目录由设置驱动且非法值要回退注册表默认，
/// 前端不猜、也不去读那几个设置键各拼一遍。
///
/// # Errors
/// 设置快照不可用时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_type_archive_dir(
    state: tauri::State<'_, AppState>,
    note_type: NoteTypeDto,
) -> Result<String, String> {
    Ok(note_type_dir(&state.settings_snapshot(), note_type.into()))
}

/// 文件树「资料区段」的顶层归档目录清单（PRD-06-048①）。
///
/// 一次取齐而不是让前端按一份自带的类型清单逐个问：判据是 `NoteType::is_source_material`，
/// 它属于 Rust（ADR-024/ADR-003）。详见 [`material_archive_dirs`]。
///
/// # Errors
/// 设置快照不可用时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_material_dirs(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    Ok(material_archive_dirs(&state.settings_snapshot()))
}

/// 读取一篇 Vault 笔记。
#[tauri::command]
#[specta::specta]
pub(crate) fn read_note(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<NoteDocDto, String> {
    // 按窗口身份解析落点库（PRD-06-050）：副库窗口读它自己的库。
    state.with_vault(window.label(), |vault| {
        vault.read(&path).map(Into::into).map_err(|error| {
            // 还在 iCloud 上（TDD-06 §4.1 ②）：说清楚的同时就请系统下载，下载回来监听会看到，
            // 打开着的这一篇随之读到正文。
            if matches!(error, luna_vault::VaultError::CloudPlaceholder(_)) {
                let _ = super::conflicts::start_cloud_download(vault.root(), &path);
            }
            vault_err(error)
        })
    })
}

/// 这一篇的**对象级事实**：标签 + 类型 / 成熟度 / 创建 / 修改（PRD-07-031）。
///
/// 右侧上下文面板的两格共用这一次往返——它们问的是同一个对象，拆两条命令只是把一次
/// 往返变成两次，而两次之间那一篇可能已经被改过。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_facts(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<NoteFactsDto, String> {
    // 按窗口身份解析落点库（PRD-06-050），与 `read_note` 同一条规则。
    state.with_vault(window.label(), |vault| {
        vault.note_facts(&path).map(Into::into).map_err(vault_err)
    })
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NoteSaveDto {
    Saved { index_updated: bool },
    Conflict { current_content: Option<String> },
}

/// 核对编辑器基线后保存，冲突不写盘、不广播。
#[tauri::command]
#[specta::specta]
pub(crate) fn write_note(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    content: String,
    expected_content: String,
) -> Result<NoteSaveDto, String> {
    let receipt = state.with_vault(window.label(), |vault| {
        vault
            .save_note(&path, &expected_content, &content)
            .map_err(vault_err)
    })?;
    match receipt {
        luna_vault::NoteSave::Saved { index_updated } => {
            broadcast_note_write(&app, window.label(), &path, content);
            Ok(NoteSaveDto::Saved { index_updated })
        }
        luna_vault::NoteSave::Conflict { current_content } => {
            Ok(NoteSaveDto::Conflict { current_content })
        }
    }
}

/// 笔记写盘后的**同一咽喉尾段**：用户保存（[`write_note`]）与程序化改写
/// （[`tidy_list_levels_apply`]）共用，写盘本体则统一经 `Vault::write`
/// （快照 §4.7 / 原子写 §4.3 / 自写指纹 §4 / 重索引）——不另起写盘栈。
///
/// 跨窗口/多 pane 同文件同步（TDD-07 §4.1）：自写回声抑制会吞掉本应用写盘的
/// `vault://changed`，故经专用事件广播到所有窗口——同一笔记的其它编辑器实例据此
/// 增量跟进（未脏）或提示外部冲突（已脏）。携正文避免各窗重复 read 往返；接收端以
/// `savedContentRef` 为准跳过自写，杜绝把用户后续键入误判为外部变更。
/// 事件携**库上下文**：路径是库相对的，主窗口与副库窗口可能同时开着同名路径的
/// 两篇（不同库的）笔记——不带上下文，副库保存会把内容注入主窗口的同名标签页。
pub(crate) fn broadcast_note_write(
    app: &tauri::AppHandle,
    window_label: &str,
    path: &str,
    content: String,
) {
    let change_scope = VaultChangeScope::for_window(window_label);
    let _ = (NoteWritten {
        path: path.to_owned(),
        content,
        origin: if change_scope.affects_active_vault() {
            "main".to_owned()
        } else {
            window_label.to_owned()
        },
    })
    .emit(app);
    // 派生更新只对主库触发（PRD-06-050），见 `refresh_derived_after_write`。
    if change_scope.affects_active_vault() {
        refresh_derived_after_write(app, path);
    }
}

/// 一篇笔记的正文在主库里落定之后的派生更新——**编辑器保存、捕获框新建、语音速记、AI
/// 提炼写回、外部编辑器与同步**共用这一条（PRD-16-064）：此前只有编辑器保存走全套，其余入口
/// 只发 `vault://changed`，于是它们的 AI 标签、语义索引与主题候选要等下一次在编辑器里保存
/// 才跟上。各步自带门控（资格、设置、正文指纹去抖），重复调用零成本。
///
/// 只对主库调用：AI 写回与向量以 note_rel 为键、以主库为落点（PRD-06-050），副库的写入
/// 若触发它会写进主库的同名笔记。
pub(crate) fn refresh_derived_after_write(app: &tauri::AppHandle, path: &str) {
    // 保存后自动刷新 AI 关系标签（PRD-06-035）：设置门控 + 正文指纹去抖在函数内。
    spawn_graph_tag_refresh(app, path);
    // 增量语义索引（PRD-16-028）：去抖靠正文指纹（已最新即 0 成本返回）。
    embed::spawn_embed_if_configured(app, path);
    schedule_local_knowledge_metrics_reconcile(app);
    // 来源正文变了：去抖后为它牵动的主题准备整理建议（TDD-06 §3.9.3）。
    super::ingest::schedule_source_revision(app, path);
}

/// 「整理列表层级」预览 DTO（PRD-06-056）：改动清单 + 预览基准指纹。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TidyListPreviewDto {
    /// 预览所基于的**全文**指纹；应用命令凭它拒绝陈旧确认（确认卡展示的行必须
    /// 就是将被改写的行，先例同 `GRAPH_TAGS_STALE` / `INGEST_TOPIC_CHANGED`）。
    pub content_hash: String,
    /// 将被归一的行（升序）。空 = 无需整理，前端不该调应用命令。
    pub changes: Vec<TidyListChangeDto>,
}

/// 一行的归一前后对照（确认卡展示单元，PRD-06-056）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TidyListChangeDto {
    /// 1-based 行号，相对整篇文件（与编辑器行号同口径）。
    pub line: u32,
    /// 归一前的行内容（不含行尾）。
    pub before: String,
    /// 归一后的行内容（不含行尾）。
    pub after: String,
}

impl From<luna_vault::ListTidyChange> for TidyListChangeDto {
    fn from(value: luna_vault::ListTidyChange) -> Self {
        Self {
            line: value.line,
            before: value.before,
            after: value.after,
        }
    }
}

/// 「整理列表层级」预览（PRD-06-056）：只读不写。归一规则与刻意不做清单
/// （链长→缩进映射等）单点在 [`luna_vault::tidy_list_levels`]。
#[tauri::command]
#[specta::specta]
pub(crate) fn tidy_list_levels_preview(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<TidyListPreviewDto, String> {
    // 按窗口身份解析落点库（PRD-06-050），与 read_note 同现状。
    state.with_vault(window.label(), |vault| {
        let doc = vault.read(&path).map_err(vault_err)?;
        let tidy = luna_vault::tidy_list_levels(&doc.content);
        Ok(TidyListPreviewDto {
            content_hash: luna_vault::ContentHash::of(doc.content.as_bytes()).to_string(),
            changes: tidy.changes.into_iter().map(Into::into).collect(),
        })
    })
}

/// 「整理列表层级」应用（PRD-06-056）：用户在确认卡上点了确认才会走到这里，
/// **绝无自动档**（与 PRD-06-053 同一纪律）。返回实际归一的行数。
///
/// 读盘、指纹校验、归一、写盘在**同一把 Vault 锁内**完成（先例同 `apply_ingest`），
/// 杜绝「校验通过后、写盘之前」外部写入被静默覆盖；写盘经 `Vault::write`
/// （版本快照 + 原子写 + 重索引），事件尾段复用 [`broadcast_note_write`]。
#[tauri::command]
#[specta::specta]
pub(crate) fn tidy_list_levels_apply(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    expected_hash: String,
) -> Result<u32, String> {
    let (content, lines) = state.with_vault(window.label(), |vault| {
        let doc = vault.read(&path).map_err(vault_err)?;
        if luna_vault::ContentHash::of(doc.content.as_bytes()).as_str() != expected_hash {
            return Err(coded_error(&app, "TIDY_LIST_STALE", "tidy-list-stale"));
        }
        let tidy = luna_vault::tidy_list_levels(&doc.content);
        if tidy.changes.is_empty() {
            // 防御分支：指纹相同则预览已给过空清单，前端不该调到这里；无可归一行不写盘。
            return Ok((None, 0));
        }
        let lines = u32::try_from(tidy.changes.len()).unwrap_or(u32::MAX);
        vault.write(&path, &tidy.content).map_err(vault_err)?;
        Ok((Some(tidy.content), lines))
    })?;
    if let Some(content) = content {
        broadcast_note_write(&app, window.label(), &path, content);
    }
    Ok(lines)
}

/// 「沉淀为知识」（PRD-06-053）：把一篇记录态笔记晋升为知识态，返回它此后的相对路径。
///
/// **两种记录态各有各的毕业方式，路由在 Rust**（ADR-003 前端零业务）：
/// - 显式 `luna.stage` 的笔记 → 去掉这一个键（frontmatter 局部改写，正文与第三方键
///   逐字节保留，TDD-06 §4.4）；路径不变。
/// - **缺省**记录态的速记 → 它根本没有该键可删，毕业动作是既有的「升级为笔记」
///   （PRD-02-055 的 type 改写已含毕业语义）——故此处委派给
///   [`super::distill::promote_quick_note_impl`]，**不另造一条路径**。返回的新路径与
///   速记清单里点「升级为笔记」得到的完全一致（同一实现），前端据此重映射打开中的标签。
///
/// **绝无自动档**：只有用户点了页头徽标或树行右键才会走到这里（与
/// [`tidy_list_levels_apply`] 同一纪律）。已是知识态的笔记原样返回、不写盘。
#[tauri::command]
#[specta::specta]
pub(crate) fn settle_note_as_knowledge(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    // 观测授权在**动作开始处**取得（TDD-12 §2.1）；速记那条分支由它委派的实现自己取。
    let settle_grant = begin_quick_note_settled(&app);
    let quick_note = state.with_vault(window.label(), |vault| {
        Ok(vault.read(&path).map_err(vault_err)?.meta.note_type == NoteType::QuickNote)
    })?;
    if quick_note {
        // 速记走既有升级路径：它自己完成移动、类型改写、知识写入意图与两条 `vault://changed`。
        return super::distill::promote_quick_note_impl(&app, &path);
    }
    // 按窗口身份解析落点库（PRD-06-050），与 `tidy_list_levels_apply` 同现状。
    let written = state.with_vault(window.label(), |vault| {
        vault.promote_to_knowledge(&path).map_err(vault_err)
    })?;
    if let Some(content) = written {
        // 同文件的其它编辑器实例据此增量跟进（frontmatter 少了一行，正文缓冲区必须同步）。
        broadcast_note_write(&app, window.label(), &path, content);
        // 树行的淡显与徽标、图谱与搜索的分组都随派生索引重算——写盘的自写回声被抑制，
        // 故这里显式告诉界面「这一篇变了」（与 `create_note_from_text` 同现状）。
        emit_vault_changed(&app, &path, false);
        // 观测记在**真正提交之后**（TDD-12 §2.1）：写盘失败时 `written` 是 `None`，
        // 这一行就不会执行——记一次"转正了"而文件其实没动，比不记更糟。
        // 只记次数，不带路径、不带标题（那是内容的一半）。
        if let Some(grant) = settle_grant {
            commit_quick_note_settled(&app, grant);
        }
    }
    Ok(path)
}

/// 「转为正式笔记」这个**动作开始**时取得观测授权（TDD-12 §2.1）。
///
/// **转正不是一个瞬间**：它要读一次库、搬一次文件、改一次类型、再提交一次知识写入
/// 意图；这中间用户完全可以在设置里关掉开关或清除历史。开始时未授权就整条不记，绝不在
/// 终点补一个"当时大概是开着的"判断。
pub(crate) fn begin_quick_note_settled(
    app: &tauri::AppHandle,
) -> Option<crate::workspace_metrics::WorkspaceMetricsGrant> {
    app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>()
        .and_then(|metrics| metrics.begin())
}

/// 「转为正式笔记」真的成功了一次（工作区观测，PRD-07-070）。
///
/// **只给次数，不除以本期新建数**：那个分母跨越了本次授权区间之外的历史，算出来的
/// "转化率"没有任何一个诚实的解释（TDD-12 §2.1）。
pub(crate) fn commit_quick_note_settled(
    app: &tauri::AppHandle,
    grant: crate::workspace_metrics::WorkspaceMetricsGrant,
) {
    if let Some(metrics) = app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>() {
        metrics.commit(
            grant,
            luna_metrics::workspace::WorkspaceEvent::QuickNoteSettled,
            super::metrics::metric_now_ms(),
        );
    }
}

/// 一篇笔记此刻的**成熟度动作可用性**（ADR-086 / PRD-06-053 / PRD-06-058）：
/// 页头「记录」徽标与编辑器右键「标记为记录」各据其一显示。
///
/// 下发的是**判定结论**而不是 `luna.stage` 取值——缺省规则（速记无键即记录态）只活在
/// `luna_vault::NoteStage::resolve`，前端一个字都不该有（ADR-003 / PRD-06-052）。
/// 两个布尔一次取齐是因为它们出自**同一次读盘**：分成两条命令，两次读之间被写盘插队就会
/// 出现"既是记录、又能被标记为记录"这种自相矛盾的菜单。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_draft_state(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<NoteDraftStateDto, String> {
    state.with_vault(window.label(), |vault| {
        let meta = vault.read(&path).map_err(vault_err)?.meta;
        let stage = meta.stage();
        Ok(NoteDraftStateDto {
            draft: stage.is_draft(),
            // 与文件树 `NoteTreeDto.markable_as_draft` 逐条同判据（主题页 / 知识地图 /
            // 资料一律不给）——同一篇笔记在两个菜单里能做的事必须一样。
            markable_as_draft: stage.allows_mark_as_draft() && !meta.topic && !meta.knowledge_map,
        })
    })
}

/// 「标记为记录」（PRD-06-058）：把一篇知识笔记降回记录态 = 加 `luna.stage` 一行。
///
/// [`settle_note_as_knowledge`] 的反向动作，走**同一个** frontmatter 改写咽喉
/// （`Vault::rewrite_luna`：round-trip 保护、无变化不写盘、经 `Vault::write` 留快照）。
/// 三类拒绝（资料 / 主题页 / 知识地图）的判定与理由全在 `Vault::mark_as_draft`，
/// 此处不复写——菜单已按 `markable_as_draft` 收敛过，这一层是防菜单陈旧的兜底。
#[tauri::command]
#[specta::specta]
pub(crate) fn mark_note_as_draft(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let written = state.with_vault(window.label(), |vault| {
        vault.mark_as_draft(&path).map_err(vault_err)
    })?;
    if let Some(content) = written {
        // 同文件的其它编辑器实例据此增量跟进（frontmatter 多了一行）；树行的淡显与徽标、
        // 记录收件箱与导航计数则随 `vault://changed` 重算（同 `settle_note_as_knowledge`）。
        broadcast_note_write(&app, window.label(), &path, content);
        emit_vault_changed(&app, &path, false);
    }
    Ok(())
}

/// 速记流（PRD-06-058 A / PRD-07-066）：全库 `type: quick-note` + 权威总数。
///
/// 收录口径是 **type 驱动**（TDD-06 §5「功能视图 = 智能查询视图」）：速记是「先捕获、
/// 后整理」的捕获流，只收文字速记与语音速记的产物；被「标记为待整理」的普通笔记留在
/// 「笔记」空间，由文件树的淡显与徽标表达。时间桶分组所需的 `modified_ms`、时龄警示
/// 所需的 `stale` 都由 Rust 判完下发，前端只按已判定的事实渲染（ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_draft_notes(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    cursor: Option<String>,
    filter: Option<DraftFilterDto>,
    text: Option<String>,
) -> Result<DraftInboxDto, String> {
    // 「现在几点」由命令层取一次喂给整批：逐行各读一次时钟，跨行判据就不再可比。
    let now_ms = chrono::Local::now().timestamp_millis();
    let query = luna_vault::DraftInboxQuery {
        text: text.as_deref(),
        filter: filter.unwrap_or_default().into(),
        now_ms,
    };
    // 此刻钉着的那几篇：运行时事实就是「有没有一扇便签窗开着」（启动时按 frontmatter
    // 恢复过一遍，此后 pin/unpin 两端同步）。取一次给整页用，而不是逐行查表。
    let pinned = state.pinned_desktop_note_paths();
    state.with_vault(window.label(), |vault| {
        // `limit: None` = 按 Rust 的页大小取一页（`luna_vault::LIST_PAGE_SIZE` 单一定义）。
        let (entries, next_cursor, matched, total) = vault
            .draft_inbox(&query, limit.unwrap_or(0) as usize, cursor.as_deref())
            .map_err(vault_err)?;
        Ok(DraftInboxDto {
            total,
            matched,
            next_cursor,
            // 汇总限额随列表一起下发（PRD-06-047）：界面用它决定「整理成一篇」什么时候
            // 可点，而命令用同一对常量重验——前端不得再写一份数字。
            merge_min: super::distill::MERGE_NOTE_MIN as u32,
            merge_max: super::distill::MERGE_NOTE_LIMIT as u32,
            notes: entries
                .into_iter()
                .map(|entry| DraftNoteDto {
                    pinned_to_desktop: pinned.contains(&entry.path),
                    todo_open: entry.todos.open,
                    todo_total: entry.todos.total,
                    // 判定已挪进索引（与「久未整理」那一档筛选同源）：此处只转发，不再
                    // 在 IPC 层拿门槛常量另算一遍，两处各算就会有一头对不上。
                    stale: entry.stale,
                    // 判据是 `luna_vault::merge_block`，与落盘前那次重验同一个函数。
                    can_merge: entry.mergeable,
                    path: entry.path,
                    title: entry.title,
                    preview: entry.preview,
                    modified_ms: entry.modified_ms as f64,
                    quick_note: entry.quick_note,
                    refined: entry.refined,
                    has_media: entry.has_media,
                    duration_ms: entry.duration_ms as f64,
                })
                .collect(),
        })
    })
}

/// 一次落盘的完整形态：落哪儿、算哪一类、正文是什么。
#[derive(Debug)]
pub(crate) struct NotePlan {
    /// Vault 相对路径（含目录与按惯例派生的 `.md` 文件名，尚未做重名消歧）。
    ///
    /// **扩展名不是装饰**：[`create_unique_note`] 用 `file_stem()` 取名，不带扩展名时
    /// 摘要里的最后一个点会被当成扩展名切掉（`速记 … v1.2 发布` → `速记 … v1`）。
    pub rel_path: String,
    pub note_type: NoteType,
    pub body: String,
}

/// 「随手记去记录，动笔写去笔记库」这条心智的**落盘判定唯一处**（PRD-06-058 P0-1）。
///
/// **判定归这里，前端只交事实**：调用方给的是 [`CaptureDestinationDto`]（用户此刻站在
/// 哪个目的地里），不是类型、不是目录、更不是成熟度取值。收敛成一个纯函数是因为这条心智
/// 有**两个入口**（侧栏捕获框 / 空白编辑器画布），而"同屏两个输入口落盘语义相反"正是它被
/// 拆成两处写时的真实代价——用户站在「记录」里，最显眼的那块输入区却创建了知识笔记。
///
/// 「记录」那一支复用速记归档的**全套现成机制**，一条新路都不另起：落 `vault.quick_note`
/// 目录、沿用 [`super::voice::quick_note_stem`] 的时间戳命名惯例（只换前缀）、经
/// [`create_unique_note`] 写盘。于是它天生记录态，天生享有升级毕业 / 整理入库 / 沉淀 /
/// AI 辅助沉淀 / 体检积压——那正是「不新增 NoteType、不新增存储目录」这条纪律换来的东西。
pub(crate) fn destination_note_plan(
    i18n: BackendCopy<'_>,
    settings: &Settings,
    lang: &str,
    destination: CaptureDestinationDto,
    content: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Result<NotePlan, String> {
    match destination {
        CaptureDestinationDto::Drafts => {
            // 捕获的正文是**一段随手记**，两端空白不是内容；而文件名摘要取的也是它。
            let body = content.trim();
            if body.is_empty() {
                return Err(coded_error_with(i18n, "EMPTY_NOTE", "empty-quick-note"));
            }
            let dir = note_type_dir(settings, NoteType::QuickNote);
            let stem =
                super::voice::quick_note_stem(super::voice::text_capture_prefix(lang), body, now);
            Ok(NotePlan {
                rel_path: format!("{dir}/{stem}.md"),
                note_type: NoteType::QuickNote,
                body: body.to_owned(),
            })
        }
        CaptureDestinationDto::Notes => {
            if content.trim().is_empty() {
                return Err(coded_error_with(i18n, "EMPTY_NOTE", "empty-note"));
            }
            // 知识笔记的正文是**用户的文档**，逐字节原样落盘（不 trim）。
            let dir = note_type_dir(settings, NoteType::Note);
            Ok(NotePlan {
                rel_path: format!(
                    "{dir}/{}",
                    draft_note_rel_path(content, NoteType::Note, lang)?
                ),
                note_type: NoteType::Note,
                body: content.to_owned(),
            })
        }
    }
}

/// 把一段文本落成一篇**新**笔记——文字捕获框与空白编辑器画布**共用的唯一命令**。
///
/// **刻意只有一条命令**：前端按目的地在两条命令之间 if/else，与前端自己决定笔记类型
/// 是同一件事（ADR-003）。这里前端交出去的只有正文与 [`CaptureDestinationDto`]，形态全部
/// 由 [`destination_note_plan`] 判定。
///
/// **不问标题、不问分类、不问成熟度**（GTD 的 capture/clarify 分离）：一切决策推迟到
/// 处理阶段。标题即首行——文件名由首行摘要派生。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_note_from_text(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    content: String,
    destination: CaptureDestinationDto,
) -> Result<NoteDocDto, String> {
    capture_text_note(&app, &state, window.label(), &content, destination).map(Into::into)
}

/// [`create_note_from_text`] 的落盘本体：也是 macOS 服务「用 Lunarca 存为速记」的落点
/// （W5b，`crate::services`）——同一段文字，从捕获框进来与从别的应用的「服务」菜单进来，
/// 落成的是同一种笔记、走同一条派生更新。
pub(crate) fn capture_text_note(
    app: &tauri::AppHandle,
    state: &AppState,
    window_label: &str,
    content: &str,
    destination: CaptureDestinationDto,
) -> Result<luna_vault::NoteDoc, String> {
    let lang = backend_copy_arc(app).map_or_else(|| "zh-CN".to_string(), |i| i.language());
    let plan = destination_note_plan(
        state.copy(),
        &state.settings_snapshot(),
        &lang,
        destination,
        content,
        chrono::Local::now(),
    )?;
    let doc = state.with_vault(window_label, |vault| {
        create_unique_note(
            state.copy(),
            vault,
            &plan.rel_path,
            plan.note_type,
            plan.body.clone(),
        )
    })?;
    emit_vault_changed(app, &doc.path, false);
    if VaultChangeScope::for_window(window_label).affects_active_vault() {
        refresh_derived_after_write(app, &doc.path);
    }
    Ok(doc)
}

pub(crate) fn draft_note_rel_path(
    content: &str,
    note_type: NoteType,
    lang: &str,
) -> Result<String, String> {
    let stem = title_line(content)
        .and_then(sanitized_file_stem)
        .unwrap_or_else(|| {
            let now = chrono::Local::now();
            let time_str = now.format("%Y%m%d_%H%M%S").to_string();
            let is_zh = lang.starts_with("zh");
            let prefix = match note_type {
                NoteType::Note => {
                    if is_zh {
                        "笔记"
                    } else {
                        "Note"
                    }
                }
                NoteType::Recording => {
                    if is_zh {
                        "录音"
                    } else {
                        "Recording"
                    }
                }
                NoteType::Chat => {
                    if is_zh {
                        "对话"
                    } else {
                        "Chat"
                    }
                }
                NoteType::QuickNote => {
                    if is_zh {
                        "闪记"
                    } else {
                        "QuickNote"
                    }
                }
                NoteType::Transcript => {
                    if is_zh {
                        "转写"
                    } else {
                        "Transcript"
                    }
                }
                NoteType::Document => {
                    if is_zh {
                        "文档"
                    } else {
                        "Document"
                    }
                }
                NoteType::VideoProject => {
                    if is_zh {
                        "视频项目"
                    } else {
                        "VideoProject"
                    }
                }
                NoteType::Script => {
                    if is_zh {
                        "脚本"
                    } else {
                        "Script"
                    }
                }
                NoteType::Swipe => {
                    if is_zh {
                        "灵感"
                    } else {
                        "Swipe"
                    }
                }
            };
            format!("{prefix}_{time_str}")
        });
    Ok(format!("{stem}.md"))
}

pub(crate) fn empty_note_rel_path(note_type: NoteType, lang: &str) -> Result<String, String> {
    draft_note_rel_path("", note_type, lang)
}

/// 一段正文的**标题行**：第一条有字的行，去掉行首的 Markdown 块标记（标题 `#`、引用 `>`）。
///
/// 文件名从正文首行派生的**唯一**判据（知识笔记 [`draft_note_rel_path`] 与速记
/// [`super::voice::quick_note_stem`] 共用）。此前两处各写一份，只有知识笔记那份认得标题
/// 标记：以 `## 标题` 开头的速记文件名成了「速记 … ##-标题」（真机 2026-10-02，便签「另存为
/// 新速记」）。只剥标记、不剥字：`#标签 正文` 照旧留下「标签 正文」。
pub(crate) fn title_line(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let title = line.trim().trim_start_matches(['#', '>']).trim();
        (!title.is_empty()).then_some(title)
    })
}

pub(crate) fn sanitized_file_stem(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in input.chars() {
        if out.chars().count() >= DRAFT_STEM_MAX_CHARS {
            break;
        }
        if ch == '/' || ch == '\\' || ch == ':' || ch == '\0' || ch.is_control() {
            pending_dash = true;
            continue;
        }
        if ch.is_whitespace() {
            pending_dash = true;
            continue;
        }
        if pending_dash && !out.is_empty() {
            out.push('-');
        }
        pending_dash = false;
        out.push(ch);
    }
    let trimmed = out.trim_matches(['-', '.', ' ']).to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 创建一篇空白 Markdown 笔记。文件名由 Rust 生成，前端只消费返回路径（ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_note(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_type: Option<NoteTypeDto>,
) -> Result<NoteDocDto, String> {
    let n_type = note_type.map(Into::into).unwrap_or(NoteType::Note);
    // 归属一方应用的类型（视频项目 / 脚本 / 灵感）只在那个应用启用时能建（ADR-165 ④）。
    if let Some(owner) = luna_config::apps::note_type_owner(n_type.key()) {
        super::require_app(&app, &state, owner.id)?;
    }
    if n_type == NoteType::VideoProject {
        // 视频项目永远是一个文件夹（ADR-162 ②）：模板选择器里的「视频项目」与创作空间的
        // 「新建项目」走同一条路，不留一篇孤零零的主页。
        return super::creator::create_project(
            &app,
            &window,
            &state,
            None,
            super::creator::ProjectSeed::Blank,
            &super::creator::ProjectOptions::default(),
        );
    }
    let change_scope = VaultChangeScope::for_window(window.label());
    let i18n = backend_copy_arc(&app);
    let lang = i18n
        .as_ref()
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string());
    // 目录取当前设置快照的归档目录：副库窗口沿用同一目录名约定（设置系统单活跃库，
    // 副库自己的 Vault 级设置未加载——目录名是约定式默认，差异可忽略且诚实）。
    let dir = note_type_dir(&state.settings_snapshot(), n_type);
    let rel_path = format!("{dir}/{}", empty_note_rel_path(n_type, &lang)?);
    // 灵感条目带「片段 / 为什么好 / 来源」三节骨架（ADR-162 ⑬）；其余类型是空白笔记。
    let body = if n_type == NoteType::Swipe {
        super::creator_swipe::swipe_skeleton(i18n.as_deref())
    } else {
        String::new()
    };
    let doc = state.with_vault(window.label(), |vault| {
        create_unique_note(state.copy(), vault, &rel_path, n_type, body)
    })?;
    emit_vault_changed_in_scope(&app, &doc.path, false, change_scope);
    Ok(doc.into())
}

pub(crate) fn create_unique_note(
    i18n: BackendCopy<'_>,
    vault: &Vault,
    rel_path: &str,
    note_type: NoteType,
    body: String,
) -> Result<NoteDoc, String> {
    create_unique_note_with(i18n, vault, rel_path, note_type, |_| body.clone())
}

/// 唯一命名循环的单一实现。用户模板在重名时也需拿到最终文件名，以保持 `{{title}}`
/// 与笔记标题事实源一致。
pub(crate) fn create_unique_note_with<F>(
    i18n: BackendCopy<'_>,
    vault: &Vault,
    rel_path: &str,
    note_type: NoteType,
    mut body_for_stem: F,
) -> Result<NoteDoc, String>
where
    F: FnMut(&str) -> String,
{
    let path = Path::new(rel_path);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("note");

    for index in 0..100 {
        let file_name = if index == 0 {
            format!("{stem}.md")
        } else {
            format!("{stem}-{index}.md")
        };
        let candidate = parent
            .map(|parent| parent.join(&file_name))
            .unwrap_or_else(|| PathBuf::from(&file_name))
            .to_string_lossy()
            .into_owned();
        let candidate_stem = Path::new(&candidate)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(stem);
        match vault.create(
            &candidate,
            note_type,
            &TemplateVars {
                body: body_for_stem(candidate_stem),
                ..TemplateVars::default()
            },
        ) {
            Ok(doc) => return Ok(doc),
            Err(VaultError::AlreadyExists(_)) => continue,
            Err(err) => return Err(vault_err(err)),
        }
    }

    Err(coded_error_with(
        i18n,
        "VAULT_CREATE_EXHAUSTED",
        "vault-create-exhausted",
    ))
}

/// 返回 Vault 目录树。
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn vault_tree(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<NoteTreeDto, String> {
    // 「显示隐藏文件」（V4b，设备设置）只作用于文件树这一个呈现面；点文件仍不进索引。
    let show_hidden = state
        .settings_snapshot()
        .get_bool("vault.show_hidden_files")
        .unwrap_or(false);
    // 按窗口身份解析（PRD-06-050）：副库窗口的文件树来自它自己的库。
    state.with_vault(window.label(), |vault| {
        vault
            .tree_showing_hidden(show_hidden)
            .map(Into::into)
            .map_err(vault_err)
    })
}

/// 本地 Vault 搜索（非 Job、不耗 token）。
#[tauri::command]
#[specta::specta]
pub(crate) fn search(
    state: tauri::State<'_, AppState>,
    query: SearchQueryDto,
) -> Result<Vec<SearchHitDto>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .search(&query.into())
        .map(|hits| hits.into_iter().map(Into::into).collect())
        .map_err(vault_err)
}
