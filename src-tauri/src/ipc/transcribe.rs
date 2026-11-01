//! 转写任务入队 · 处理队列 命令域（PRD-05 / PRD-04-055，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：**把「要处理什么」变成队列里的一件任务**——导入音/视频文件、对已归档笔记
//! 二次转写 / 二次翻译 / 说话人细分的入队命令，以及队列快照/调序/移除/清历史。
//! 四类任务共用**单一队列 + 单 worker 顺序处理**（天然消并发，保证「先转写后翻译」，
//! PRD-04-055）；队列数据结构在 [`crate::processing_queue`]，任务编排在 [`super::processing`]。
//!
//! 跨模块可见性：命令 `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 入队一件**二次转写**任务（PRD-04-030/055）：进入统一处理队列由单 worker 顺序处理
/// （重跑批量档 whisper →（可选）逐句转译 → 权威写回）。前置校验批量档模型可用 + 笔记有音频；
/// 同一笔记已在队列（排队/运行）则幂等不重复入队。进度/记录经 `processing://queue` 事件。
///
/// # Errors
/// 批量档 Provider 不可用、Vault 未打开、笔记无音频媒体时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn enqueue_retranscribe(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<(), String> {
    if state.processing.has_active(&note_rel, "retranscribe") {
        return Ok(());
    }
    state.ensure_batch_asr_provider(&app)?;
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if build_retranscribe_parts(vault, &note_rel)?.is_empty() {
            return Err("该笔记没有已登记的音频媒体".into());
        }
    }
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::Retranscribe {
            note_rel: note_rel.clone(),
            // 手动二次转写只重跑转写；要译文请点「翻译」——翻译是显式动作，
            // 不由一个全局开关替用户决定（PRD-05-031 修订）。
            translate_to: None,
        },
        note_task_label(&note_rel),
    );
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(())
}

/// 入队一件**二次翻译**任务（PRD-04-034/055）：与二次转写解耦，不重跑 whisper，只对既有
/// 转写段按 `target_lang` 批量转译并权威回写（正文译文引用行 + 边车 `translation` + 标记），
/// 令回放「译文/双语」点亮。改目标语言重跑幂等（worker 先清旧译再翻）。进度/记录经
/// `processing://queue` 事件；同一笔记已有翻译任务在队列则幂等不重复入队。
///
/// `target_lang`：BCP-47 主语种码（如 `zh`/`en`/`ja`）= 全轨翻成该语言；`smart`/`auto`/空 =
/// 智能双向（母语↔默认外语，与实时同源）。
///
/// # Errors
/// Vault 未打开、笔记不存在或尚无转写段时立即返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn enqueue_translate(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    target_lang: String,
) -> Result<(), String> {
    if state.processing.has_active(&note_rel, "translate") {
        return Ok(());
    }
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        if collect_note_segments(vault.root(), &doc.meta).is_empty() {
            return Err(coded_error(
                &app,
                "TRANSLATE_NO_TRANSCRIPT",
                "translate-no-transcript",
            ));
        }
    }
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::Translate {
            note_rel: note_rel.clone(),
            target_lang,
        },
        note_task_label(&note_rel),
    );
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(())
}

/// 入队一件**说话人细分**任务（PRD-04-054/055）：进入统一处理队列由单 worker 顺序处理
/// （decode → 本地 diarization → 权威写回标签）。前置校验细分模型已装 + 笔记有音频与转写段；
/// 同一笔记已有细分任务在队列则幂等不重复入队。进度/记录经 `processing://queue` 事件。
///
/// # Errors
/// 细分模型未装、Vault 未打开、无音频分片或尚无转写段时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn enqueue_diarize(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<(), String> {
    if state.processing.has_active(&note_rel, "diarize") {
        return Ok(());
    }
    // 诚实前置：模型未装 → 引导下载（不入队等失败）。
    let models_root = state.models_root(&app);
    if !crate::diarize_provider::SherpaDiarizeProvider::models_present(&models_root) {
        return Err(coded_error(
            &app,
            "DIARIZE_MODELS_MISSING",
            "diarize-models-missing",
        ));
    }
    // 模型可能是启动后才下载/放置的：确保 Provider 已热注册。
    state.register_diarize_provider_if_available(&app);
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        // 细分要**跑模型读音频**，故它是唯一真正需要本地媒体的处理——仅存链接的转写
        // （PRD-05-006）没有音频可读，如实拒绝并引导先取回媒体，而不是跑一半才失败。
        if sorted_media_parts(&doc.meta.media).is_empty() {
            return Err(coded_error(
                &app,
                "DIARIZE_NOT_APPLICABLE",
                "diarize-not-applicable-media",
            ));
        }
        if collect_note_segments(vault.root(), &doc.meta).is_empty() {
            return Err(coded_error(
                &app,
                "DIARIZE_NO_TRANSCRIPT",
                "diarize-no-transcript",
            ));
        }
    }
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::Diarize {
            note_rel: note_rel.clone(),
        },
        note_task_label(&note_rel),
    );
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(())
}
/// 打开音/视频文件**多选**对话框（Rust 侧，前端零业务）；用户取消返回空 `Vec`。
pub(crate) async fn pick_media_sources(app: &tauri::AppHandle) -> Result<Vec<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    // 过滤集合直接取自可播放媒体扩展名单一来源（PRD-04-058）：能选进来的必然能解码、
    // 能回放，不出现「选得进、放不出」的落差。
    let exts: Vec<&str> = AUDIO_MEDIA_EXTS
        .iter()
        .chain(VIDEO_MEDIA_EXTS.iter())
        .copied()
        .collect();
    app.dialog()
        .file()
        .add_filter("Audio/Video", &exts)
        .pick_files(move |paths| {
            let result: Result<Vec<PathBuf>, String> = paths
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.into_path().map_err(|e| e.to_string()))
                .collect();
            let _ = tx.send(result);
        });
    rx.await
        .map_err(|_| coded_error(app, "DIALOG_CANCELLED", "dialog-cancelled"))?
}

/// 广播处理队列快照（脱敏：仅任务标签/状态阶段/计数）。
pub(crate) fn emit_processing_queue(app: &tauri::AppHandle) {
    use tauri_specta::Event as _;
    let processing = &app.state::<AppState>().processing;
    // 广播是队列每次变化的**唯一出口**，故把续跑档落盘挂在此处——不会漏、也不会被
    // 某个新增的变更路径忘记调用（内部按结构性版本号去重，进度更新不写盘，PRD-05-012）。
    processing.persist_if_changed();
    let items = processing.snapshot();
    let _ = crate::processing_queue::ProcessingQueueEvent { items }.emit(app);
}

/// 笔记任务展示标签 = 笔记文件名 stem（去 `.md`，脱敏）。
pub(crate) fn note_task_label(note_rel: &str) -> String {
    Path::new(note_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(note_rel)
        .to_string()
}

/// 无 worker 在跑则认领并启动（新入队项由现有 worker 顺序消费）。
pub(crate) fn start_processing_worker_if_idle(app: &tauri::AppHandle) {
    if app.state::<AppState>().processing.try_claim_worker() {
        tauri::async_runtime::spawn(run_processing_worker(app.clone()));
    }
}
/// 打开音/视频文件**多选**对话框，交回选中的文件（PRD-04-032）；**不入队**。`[]` = 用户取消或没有
/// 有效文件。
///
/// 选与入队分成两步，是为了在中间插进任务前预估（PRD-13-040 ②，纵切 3 C4）：服务端转写一小时的
/// 录音之前要先说「约消耗多少、够不够」，而对话框在 Rust 里弹、选完就入队的那一版没有这道缝。入队走
/// 与拖入同一条 [`import_media_paths`]（筛选、早验证、顺序处理同一处）。
///
/// # Errors
/// 批量档 Provider 不可用（模型缺失）时返回错误——早验证，不弹选择框。
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_media_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.ensure_batch_asr_provider(&app)?;
    let files = pick_media_sources(&app).await?;
    Ok(files
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// 把**拖入的文件路径**筛出音/视频并入队转写（PRD-05-001）。返回入队数量。
///
/// 前端只转发 `tauri://drag-drop` 给出的绝对路径；**筛选与校验全在此**（ADR-003 前端零业务）：
/// 扩展名白名单取自可播放媒体单一来源（选得进 = 放得出），非文件与非媒体一律丢弃——
/// 用户可能连同文档、图片一起拖进来，我们只处理认识的那些，不报错也不静默假装成功。
///
/// # Errors
/// 批量档 Provider 不可用（模型缺失）时返回错误；无有效媒体返回 `Ok(0)`。
#[tauri::command]
#[specta::specta]
pub(crate) fn import_media_paths(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<u32, String> {
    enqueue_media_imports(&app, &state, paths, None, None)
}

/// 筛出认识的媒体文件并逐个排进导入队列；`folder` 是转写笔记要建进的文件夹（`None` = 默认的
/// 转写文件夹），`link_from` 是转写成功后要链上它的研究笔记（`None` = 不链）。「转写」页、拖入、
/// 创作者发布包页与项目概览共用这一条。
pub(crate) fn enqueue_media_imports(
    app: &tauri::AppHandle,
    state: &AppState,
    paths: Vec<String>,
    folder: Option<String>,
    link_from: Option<String>,
) -> Result<u32, String> {
    let media: Vec<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file() && media_kind(&p.to_string_lossy()).is_some())
        .collect();
    if media.is_empty() {
        return Ok(0);
    }
    // 早验证放在确认有媒体之后：拖入一堆无关文件不该弹「模型缺失」。
    state.ensure_batch_asr_provider(app)?;
    let count = media.len() as u32;
    for path in media {
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("import")
            .to_string();
        state.processing.enqueue(
            crate::processing_queue::ProcTaskKind::Import {
                path,
                folder: folder.clone(),
                link_from: link_from.clone(),
            },
            label,
        );
    }
    emit_processing_queue(app);
    start_processing_worker_if_idle(app);
    Ok(count)
}

// ── 从链接导入（PRD-05-006 / PRD-05-007，ADR-023）──────────────────────────────────

/// 抓取工具可用性快照（前端据此决定给输入框还是给安装引导）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct FetchToolStatusDto {
    /// 可以开始抓取（工具找到且能跑，且未被严格本地拦截）。
    pub ready: bool,
    /// 找到的可执行文件路径（`None` = 未安装；诊断与设置页回显用）。
    pub tool_path: Option<String>,
    /// 工具自报版本（`None` 而 `tool_path` 在场 = 找到了但跑不起来，两种处境的引导不同）。
    pub version: Option<String>,
    /// ffmpeg 是否在场——决定视频档能否取到 1080p（缺失则只能取站点预合并流，通常 ≤720p）。
    pub ffmpeg: bool,
    /// ffmpeg 的路径（`None` = 未装）。工具面板显示"装在哪儿"，也是诊断线索。
    pub ffmpeg_path: Option<String>,
    /// 严格本地模式已开启：抓取被硬拦截（AGENTS.md §7），界面须给"去关闭"而非"去安装"。
    pub strict_local: bool,
    /// 版本探测**超时**了（≠ 工具坏了）：界面该给「重新检测」而不是「重装」。
    pub probe_timed_out: bool,
    /// **Lunarca 受管安装**的版本（`None` = 未由 Lunarca 装过）。有值即表示"应用管着这份工具"，
    /// 界面据此把主按钮从「安装」切成「更新 / 卸载」（ADR-023 修订）。
    pub managed_version: Option<String>,
    /// 当前在用的这份是否就是受管那份（用户自己 brew 装的也可能被选中，见发现顺序）。
    /// 界面如实显示"正在使用 Lunarca 管理的 / 系统安装的"，避免"我 brew upgrade 了怎么没变"的困惑。
    pub using_managed: bool,
    /// 找到的 JS 运行时名（`deno` / `node` / `bun`；`None` = 一个都没装）。
    ///
    /// 缺它**不进 `ready`**：那是降级不是故障（多数站点不需要解 JS 挑战），但 YouTube 这类
    /// 站点的取流会退到不需要挑战的播放器客户端，成功率随站点策略摇摆——故界面要如实标出。
    pub js_runtime: Option<String>,
    /// 该运行时的绝对路径（`None` = 未找到）。与 ffmpeg 同理，面板显示"装在哪儿"。
    pub js_runtime_path: Option<String>,
    /// 在用的这份运行时是不是 Lunarca 代装的（决定要不要给「卸载」——77 MB 得给得掉，
    /// 而用户自己装的那份我们绝不去动，与 yt-dlp 同一条纪律）。
    pub using_managed_js: bool,
    /// Lunarca 代装的那份运行时的版本（`None` = 没代装过，或是本轮之前装的、没记版本）。
    pub managed_js_version: Option<String>,
    /// 第一次用之前确认过权利声明没有（ADR-167 ④）；没有就先给确认卡。
    pub rights_confirmed: bool,
}

/// 一次「让 Lunarca 安装」的结果（PRD-05-007 修订：一次点击装齐 yt-dlp + 必要的 JS 运行时）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct InstallToolchainDto {
    /// 装好的 yt-dlp 版本。
    pub tool_version: String,
    /// 顺带装上的 JS 运行时版本（`None` = 本机已有一份，没必要再装）。
    pub js_runtime_version: Option<String>,
    /// 运行时那一半的失败原因（稳定错误码；`None` = 没失败）。
    ///
    /// 它是**返回值而不是错误**：yt-dlp 已经装好且可用，把整次安装判死是撒谎。
    /// 但也不能沉默——用户点了一颗按钮，得知道拿到的是一件还是两件。
    pub js_runtime_error: Option<String>,
}

/// 安装计划里的一件组件（`luna_fetch::install::PlannedComponent` 的投影）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct FetchInstallComponentDto {
    /// 稳定 id（`yt_dlp` / `js_runtime`，与 `ExternalToolGuideDto::tool` 同一取值空间）。
    pub tool: String,
    /// 本机已有一份 Lunarca 管理的这件组件，这次安装会覆盖它（界面说「重新安装」而不是「安装」）。
    pub replaces_managed: bool,
    /// 上游来源（仓库地址，不带协议）。
    pub source: String,
    /// 典型下载体积（字节，**约数**；精确值在下载开始后由进度给出）。
    pub approx_download_bytes: f64,
    /// 典型磁盘占用（字节，约数）。
    pub approx_disk_bytes: f64,
}

/// 「让 Lunarca 安装」在**点击之前**的完整计划（PRD-05-007 2026-10-08 修订）。
///
/// 不联网：组件取舍只看本机文件，体积是约数。两个安装入口（「从链接导入」对话框与
/// 「外部工具」面板）都渲染它，并把看过的组件 id 原样交回 `install_fetch_tool` 核对。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct FetchInstallPlanDto {
    /// 将要下载的组件（按安装顺序）。
    pub components: Vec<FetchInstallComponentDto>,
    /// 安装目录（Lunarca 的私有工具目录；不需要管理员权限、不改系统 PATH）。
    pub install_dir: String,
}

/// **安装前的计划**（不联网：只看本机文件）。
#[tauri::command]
#[specta::specta]
pub(crate) fn fetch_install_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> FetchInstallPlanDto {
    let models_root = state.models_root(&app);
    let components = crate::fetch_tool::toolchain_plan(&models_root)
        .into_iter()
        .map(|component| {
            let replaces_managed = match component {
                luna_fetch::install::PlannedComponent::YtDlp => {
                    crate::fetch_tool::managed_path(&models_root).exists()
                }
                luna_fetch::install::PlannedComponent::JsRuntime => {
                    crate::fetch_tool::managed_js_runtime_path(&models_root).exists()
                }
            };
            FetchInstallComponentDto {
                tool: component.as_str().to_owned(),
                replaces_managed,
                source: component.source().to_owned(),
                approx_download_bytes: component.typical_download_bytes() as f64,
                approx_disk_bytes: component.typical_disk_bytes() as f64,
            }
        })
        .collect();
    let install_dir = crate::fetch_tool::managed_path(&models_root)
        .parent()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    FetchInstallPlanDto {
        components,
        install_dir,
    }
}

/// 一件外部工具在**本机平台**上的安装引导（`luna_fetch::tool::install_guide` 的投影）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct ExternalToolGuideDto {
    /// 稳定工具 id（`yt_dlp` / `ffmpeg` / `js_runtime`）。前端按它取文案与图标。
    ///
    /// 与 `transcribe.fetch_media` 同一条跨界口径：`luna-fetch` 保持零新增依赖（不引 specta），
    /// 故枚举以**稳定字符串**过界，前端一侧由 `EXTERNAL_TOOLS` 常量 + 守护测试锁住取值集合。
    pub tool: String,
    /// 包管理器展示名（`None` = 本平台不给一行命令，只给官方说明页）。
    pub manager: Option<String>,
    /// 可复制的一行安装命令（`None` 同上）。
    pub command: Option<String>,
    /// 官方安装说明页（**恒有**）。
    pub docs_url: String,
}

/// **四件外部工具在本机平台上该怎么装**（纯平台常量，不联网、不读设置）。
///
/// 这条命令的存在是一次缺陷修复：安装命令此前**写死在 i18n 里**（`brew install …`），
/// 于是 Windows 用户复制到的是一条必然失败的命令，而面板里没有第二条路。命令是**环境事实**
/// ——随平台变、不随语言变，放 i18n 就必然分裂成"语言 × 平台"份，且没有任何门禁能发现哪一份
/// 是错的。现在它与工具发现同住 `luna_fetch::tool`（ADR-003 前端零业务），并恒附官方说明页
/// 作为"包管理器不在场"时的出路。
#[tauri::command]
#[specta::specta]
pub(crate) fn external_tool_guides() -> Vec<ExternalToolGuideDto> {
    luna_fetch::tool::ExternalTool::ALL
        .iter()
        .map(|tool| {
            let guide = luna_fetch::tool::install_guide(*tool);
            ExternalToolGuideDto {
                tool: tool.as_str().to_owned(),
                manager: guide.manager,
                command: guide.command,
                docs_url: guide.docs_url,
            }
        })
        .collect()
}

/// 受管工具的上游更新查询结果（用户点「检查更新」时才联网，见 `check_fetch_tool_update`）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct FetchToolUpdateDto {
    /// 上游 latest 的版本 tag（如 `2026.08.19`）。
    pub latest_version: String,
    /// 已装的那份是否落后于它（`false` = 已是最新）。
    pub update_available: bool,
    /// 受管 JS 运行时（Deno）的检查结果；不是 Lunarca 装的就不查，为 `None`（ADR-167 ⑤）。
    pub js_runtime: Option<ComponentUpdateDto>,
}

/// 一件组件的更新检查结果。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct ComponentUpdateDto {
    pub latest_version: String,
    pub update_available: bool,
}

/// 探测结果里的一条可抓取媒体。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct UrlProbeEntryDto {
    /// 站点侧 id（列表内稳定 key）。
    pub id: String,
    /// 标题（→ 队列标签与笔记名）。
    pub title: String,
    /// 该条目的抓取地址。
    pub url: String,
    /// 时长（毫秒；`None` = 站点未给）。
    pub duration_ms: Option<f64>,
    /// 上传者 / 频道。
    pub uploader: Option<String>,
    /// 预估体积（字节；`None` = 未知）。
    pub filesize: Option<f64>,
    /// 正在直播 / 待首播 → 不可抓取（界面禁选并说明原因）。
    pub is_live: bool,
}

/// URL 探测结果（下载前的确认卡数据）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct UrlProbeDto {
    /// 站点主机名（脱敏展示"来自 youtube.com"）。
    pub site: String,
    /// 播放列表标题（`None` = 单条媒体）。
    pub playlist_title: Option<String>,
    /// 条目清单（单条媒体即长度 1）。
    pub entries: Vec<UrlProbeEntryDto>,
    /// 是否为播放列表（界面据此给"单卡"还是"可勾选清单"）。
    ///
    /// **判据在 Rust**（`FetchProbe::is_playlist`，ADR-003）：单条目的播放列表
    /// （YouTube 的 `?list=` 单集）同样算列表——用户得看清自己抓的是这一条还是整个列表，
    /// 而"条目数 == 1"这个前端曾自己写的近似判据恰恰把它误判成单视频。
    pub is_playlist: bool,
    /// 因超过上限而未列出的条数（>0 时界面须如实告知）。
    pub truncated: u32,
    /// 清单上限（界面解释"为什么只有这些"）。
    pub limit: u32,
}

/// 待入队的一条抓取任务（前端从探测清单里勾选后回传）。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct UrlFetchItemDto {
    /// 抓取地址。
    pub url: String,
    /// 标题（笔记名与队列标签来源；空则由 Rust 侧回落地址主机名）。
    pub title: String,
    /// 探测得到的预估体积（字节；`None` = 站点未给）。供**入队前磁盘预检**求和。
    pub filesize: Option<f64>,
}

/// 抓取落盘需要的余量系数（相对站点自报体积）。
///
/// 初版取 2.2 并在注释里写"音频转 WAV 后通常比源更小"——那个推理是**反的**，故常数偏小。
///
/// 峰值占用 = **临时区那一份** + **Vault 里那一份**，而后者的大小取决于落地策略：
/// - 视频档逐字节拷原容器 → 恰好两份；
/// - 音频档转 **16k mono 16bit WAV**（≈31 KB/s）→ 产物**比源更大**：128 kbps AAC 源约
///   16 KB/s，故 WAV 约为源的 2 倍，一小时音频从 56 MB 变成 109 MB。
///
/// > 假设：3.0 倍是够用的启发式下界（理由：覆盖"源 1 份 + WAV 2 份"这个常见最坏情况；
/// > 更低码率的源（64 kbps 播客）理论上需要 5 倍，但把常数抬那么高会误拒大量本来放得下的
/// > 抓取。预检的职责是**挡住明显放不下的**，真到写盘失败时仍有 `no space left` 的分类兜底）。
const FETCH_DISK_HEADROOM: f64 = 3.0;

/// 入队前磁盘预检（PRD-05 §5「磁盘空间不足：预检与提示」）。
///
/// **必须在下载之前**：`FETCH_INSUFFICIENT_DISK` 若只能由 yt-dlp 的 stderr 事后触发，用户已经
/// 花掉了整份带宽才被告知放不下。站点未给体积（`filesize` 全为 `None`）时**不阻断**——
/// 宁可让它跑到失败，也不因"猜不出多大"就拒绝一次合理的抓取。
///
/// # Errors
/// 剩余空间不足时返回 `FETCH_INSUFFICIENT_DISK`。
fn ensure_disk_headroom(
    vault_root: Option<&Path>,
    items: &[UrlFetchItemDto],
) -> Result<(), String> {
    // 查询是 IO、判定是纯逻辑 → 分开，后者才测得动**行为**（而不是去断言一个常量的值）。
    let free = vault_root.and_then(|root| fs4::available_space(root).ok());
    disk_headroom_verdict(free, items)
}

/// 预检判定（**纯函数**）：`free = None` 表示查不到剩余空间（未开库 / 网络卷 / 权限）→ 不阻断。
fn disk_headroom_verdict(free: Option<u64>, items: &[UrlFetchItemDto]) -> Result<(), String> {
    let total: f64 = items.iter().filter_map(|item| item.filesize).sum();
    if total <= 0.0 {
        return Ok(()); // 体积未知：不猜、不阻断
    }
    let Some(free) = free else {
        return Ok(());
    };
    if (free as f64) < total * FETCH_DISK_HEADROOM {
        return Err(luna_fetch::FetchError::InsufficientDisk.code().to_owned());
    }
    Ok(())
}

/// 抓取工具状态（打开「从链接导入」对话框与设置页时各拉一次）。
///
/// 不联网：只在候选路径里找文件并跑一次 `--version`（PATH 陷阱与候选顺序见
/// `luna_fetch::tool`）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn fetch_tool_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    refresh: bool,
) -> Result<FetchToolStatusDto, String> {
    let (explicit, strict_local, models_root) = {
        let settings = state.settings_snapshot();
        (
            settings
                .get_str("transcribe.fetch_tool_path")
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            settings.strict_local(),
            state.models_root(&app),
        )
    };
    let managed = crate::fetch_tool::managed_path(&models_root);
    // 带凭证与缓存（受管那份零等待；见 `fetch_tool::cached_status`）；`refresh` = 用户点「重新检测」。
    let status = crate::fetch_tool::cached_status(explicit.as_deref(), &models_root, refresh).await;
    let using_managed = status.tool_path.as_deref() == Some(managed.as_path());
    Ok(FetchToolStatusDto {
        ready: status.ready() && !strict_local,
        tool_path: status
            .tool_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        version: status.version.clone(),
        ffmpeg: status.ffmpeg,
        ffmpeg_path: status
            .ffmpeg_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        strict_local,
        probe_timed_out: status.probe_timed_out,
        managed_version: crate::fetch_tool::installed_version(&models_root),
        managed_js_version: crate::fetch_tool::installed_js_runtime_version(&models_root),
        rights_confirmed: state
            .settings_snapshot()
            .get_bool(LINK_IMPORT_RIGHTS_KEY)
            .unwrap_or(false),
        using_managed,
        js_runtime: status.js_runtime.as_ref().map(|rt| rt.name.clone()),
        js_runtime_path: status
            .js_runtime
            .as_ref()
            .map(|rt| rt.path.to_string_lossy().into_owned()),
        using_managed_js: status
            .js_runtime
            .as_ref()
            .is_some_and(|rt| rt.path == crate::fetch_tool::managed_js_runtime_path(&models_root)),
    })
}

/// **查上游有没有新版 yt-dlp**（用户显式点击才联网，ADR-023 修订）。
///
/// 这条命令的存在本身就是一次缺陷修复：受管安装把版本冻结在安装那一刻，而 yt-dlp 的
/// 站点提取器跟着站点反爬策略每周变。2026-08-23 的真机缺陷即此——一份 7 周前装好、当时
/// 完全可用的工具，在站点封掉某个播放器客户端之后只会以 `HTTP 403` 收场，而应用里
/// **没有任何地方能让用户发现"我该更新了"**。判据（`install::update_available`）早已写好，
/// 缺的正是这条把它接到界面上的通道。
///
/// 不做后台轮询：默认离线是铁律（AGENTS.md §7），联网只发生在用户点这个按钮的那一刻。
///
/// # Errors
/// 严格本地拦截、联网失败、上游发布物不符预期时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn check_fetch_tool_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<FetchToolUpdateDto, String> {
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), &app);
        }
        return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
    }
    let models_root = state.models_root(&app);
    let (latest_version, update_available) = crate::fetch_tool::check_update(&models_root).await?;
    let js_runtime = crate::fetch_tool::check_js_runtime_update(&models_root)
        .await?
        .map(|(latest_version, update_available)| ComponentUpdateDto {
            latest_version,
            update_available,
        });
    // 与「应用」页的「检查更新」记进同一份结论（ADR-168 ⑧）：组件面板查出来的，应用页的「可更新」
    // 也认。只记 Lunarca 下载的那份——系统里自己装的那份由用户自己升级。
    if crate::fetch_tool::managed_path(&models_root).is_file() {
        crate::fetch_tool::record_update(
            luna_fetch::install::PlannedComponent::YtDlp,
            update_available,
        );
    }
    if let Some(js) = &js_runtime {
        crate::fetch_tool::record_update(
            luna_fetch::install::PlannedComponent::JsRuntime,
            js.update_available,
        );
    }
    Ok(FetchToolUpdateDto {
        latest_version,
        update_available,
        js_runtime,
    })
}

/// **一键安装 / 更新受管抓取工具**（PRD-05-007 修订，ADR-023）：从上游取 latest 单文件档、
/// 用同一 release 的 `SHA2-256SUMS` 校验、落应用私有目录并置可执行位。返回装好的版本号。
///
/// 为什么应用可以代劳（而"打包随应用分发"仍被否）：按需取 **latest** 没有版本冻结问题，
/// 甚至比 `brew`（用户得记得 upgrade）更不易过期；且**下载**不构成我们对 GPL 打包物的分发。
/// 详见 ADR-023 修订段。
///
/// `confirmed` 是用户看过的那份计划（`fetch_install_plan` 的组件 id）；与此刻的计划不一致时
/// 返回 `FETCH_TOOL_PLAN_CHANGED`，界面重新取计划再让用户确认。
///
/// # Errors
/// 严格本地拦截、计划已变、联网失败、上游发布物不符预期、**校验和不匹配**、落盘失败时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn install_fetch_tool(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    confirmed: Vec<String>,
) -> Result<InstallToolchainDto, String> {
    // 联网前的硬护栏：这是显式动作，但严格本地模式一律优先（AGENTS.md §7）。
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), &app);
        }
        return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
    }
    let models_root = state.models_root(&app);
    let outcome = crate::fetch_tool::install_toolchain(&app, &models_root, &confirmed).await?;
    Ok(InstallToolchainDto {
        tool_version: outcome.tool_version,
        js_runtime_version: outcome.js_runtime_version,
        js_runtime_error: outcome.js_runtime_error,
    })
}

/// **单独安装受管 JS 运行时**（不重装 yt-dlp）。
///
/// 首次安装走的是合并那条（`install_fetch_tool` 一次装齐），故这条只服务一种处境：
/// yt-dlp 已经装好、运行时那一份却不在了——用户自己卸载过、或合并安装时它那一半失败过。
/// 没有它的话，用户要拿回运行时就只能在 yt-dlp 那一行点「重新安装」，白白再下 38 MB
/// ——那是让界面的形状去迁就实现，而不是迁就用户。
///
/// # Errors
/// 严格本地拦截、联网失败、上游发布物不符预期、校验失败、解压或落盘失败时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn install_fetch_js_runtime(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), &app);
        }
        return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
    }
    let models_root = state.models_root(&app);
    managed_component(luna_fetch::tool::ExternalTool::JsRuntime)?
        .install(&app, &models_root)
        .await
}

/// 组件的那一份实现（ADR-169 ⑧：组件怎么装、怎么卸只在 `crate::components` 里写一次）。
fn managed_component(
    tool: luna_fetch::tool::ExternalTool,
) -> Result<&'static dyn crate::components::Component, String> {
    crate::components::component(tool.as_str())
        .ok_or_else(|| format!("COMPONENT_UNKNOWN: {}", tool.as_str()))
}

/// 卸载受管 JS 运行时（解压后约 77 MB；装得上就必须删得掉）。
///
/// 与卸载 yt-dlp 分开：用户可能只想留其中一件。只删我们自己放的那一份。
///
/// # Errors
/// 删除失败（权限等）时返回稳定错误码。
#[tauri::command]
#[specta::specta]
pub(crate) fn uninstall_fetch_js_runtime(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    managed_component(luna_fetch::tool::ExternalTool::JsRuntime)?
        .uninstall(&state.models_root(&app))
}

/// 卸载受管抓取工具（工具管理面板用；幂等）。
///
/// 系统里自己装的那份不受影响——我们只删自己放的那一份，绝不碰用户的 Homebrew。
///
/// # Errors
/// 删除失败（权限等）时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn uninstall_fetch_tool(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    managed_component(luna_fetch::tool::ExternalTool::YtDlp)?.uninstall(&state.models_root(&app))
}

/// 探测一条网络地址（**不下载**）：拿标题/时长/上传者，并识别播放列表与直播（PRD-05-006）。
///
/// 为什么必须先探测：粘错链接、3 小时的直播、200 集的播放列表，用户都该在花掉带宽与磁盘
/// **之前**看见。这也是唯一能在下载前拦住直播（无终点流）的时机。
///
/// # Errors
/// 严格本地拦截、工具缺失、URL 非法、站点侧失败（登录/区域/下架）时返回稳定错误码
/// （`FETCH_*`，前端本地化为人话）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn probe_media_url(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<UrlProbeDto, String> {
    super::require_app(&app, &state, luna_config::apps::LINK_IMPORT_APP.id)?;
    let settings = {
        if state.strict_local() {
            if let Some(center) = app.try_state::<NotificationCenter>() {
                center.publish(NotificationEvent::strict_local_blocked(), &app);
            }
            return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
        }
        state.settings_snapshot()
    };
    let tool = super::processing::resolve_fetch_tool(&settings, &state.models_root(&app))?;
    let probe = luna_fetch::probe(&tool, &url)
        .await
        .map_err(|err| super::processing::fetch_err(&err))?;
    let is_playlist = probe.is_playlist();
    Ok(UrlProbeDto {
        site: probe.site,
        playlist_title: probe.playlist_title,
        is_playlist,
        entries: probe
            .entries
            .into_iter()
            .map(|e| UrlProbeEntryDto {
                id: e.id,
                title: e.title,
                url: e.url,
                // u64 经 IPC 走 f64：与既有 `duration_ms` / 体积字段同口径（媒体时长与体积
                // 远在 2^53 之内，不损精度）。
                duration_ms: e.duration_ms.map(|ms| ms as f64),
                uploader: e.uploader,
                filesize: e.filesize.map(|b| b as f64),
                is_live: e.is_live,
            })
            .collect(),
        truncated: probe.truncated,
        limit: luna_fetch::PLAYLIST_LIMIT as u32,
    })
}

use luna_config::apps::LINK_IMPORT_RIGHTS_KEY;

/// 「从链接导入」第一次使用前的权利声明：用户在确认卡上点「我确认」后记下（ADR-167 ④）。
///
/// # Errors
/// 写设置失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn confirm_link_import_rights(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    super::settings::set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        LINK_IMPORT_RIGHTS_KEY,
        super::settings::SettingValueDto::Bool(true),
    )?;
    let _ = SettingsChanged {
        key: LINK_IMPORT_RIGHTS_KEY.to_owned(),
    }
    .emit(&app);
    Ok(())
}

/// 把探测清单里用户勾选的条目入队抓取（PRD-05-006）。返回入队件数。
///
/// 每条 = 一件 `Fetch` 任务，走**同一条队列**（单 worker 顺序处理）：抓取只是导入多出来的
/// 前置阶段，故进度/取消/重试/续跑全部复用既有机制，不另起一套。
///
/// `media`：`audio`（仅音轨，省磁盘）/ `video`（连画面，可分屏校对）。**随任务走**——
/// 同一批链接里"这条只要音频、那条要画面"是正常诉求，不由全局开关代劳（PRD-05-031 教训）。
///
/// # Errors
/// 严格本地拦截、工具不可用、批量档 ASR 模型缺失、无有效条目时返回错误（不入队）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn enqueue_media_url(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    items: Vec<UrlFetchItemDto>,
    media: String,
    home: Option<String>,
) -> Result<u32, String> {
    super::require_app(&app, &state, luna_config::apps::LINK_IMPORT_APP.id)?;
    // 第一次用之前要确认过一次权利声明（ADR-167 ④）：界面先问，这里兜底。
    if !state
        .settings_snapshot()
        .get_bool(LINK_IMPORT_RIGHTS_KEY)
        .unwrap_or(false)
    {
        return Err(coded_error(
            &app,
            "LINK_IMPORT_RIGHTS_UNCONFIRMED",
            "link-import-rights-unconfirmed",
        ));
    }
    // 从创作项目「添加来源」来的：建进项目的素材文件夹并链进研究笔记（ADR-167 ④）。
    let (folder, link_from) = match &home {
        Some(home) => {
            super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
            let (folder, research) =
                super::creator_quote::material_target(&app, &window, &state, home)?;
            (Some(folder), Some(research))
        }
        None => (None, None),
    };
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), &app);
        }
        return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
    }
    // 早验证：抓下来却没有转写能力等于白下载一场（与本地导入同一纪律）。
    state.ensure_batch_asr_provider(&app)?;
    let settings = state.settings_snapshot();
    super::processing::resolve_fetch_tool(&settings, &state.models_root(&app))?;
    // 磁盘预检（PRD-05 §5）：放不下就不要开始下——下完才发现等于白花一份带宽。
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let root = guard.as_ref().map(|vault| vault.root().to_path_buf());
        ensure_disk_headroom(root.as_deref(), &items)?;
    }

    let media = luna_fetch::FetchMediaKind::from_setting(&media);
    // URL 校验在 Rust（ADR-003）：非 http(s) / 选项形状一律拒绝，见 `luna_fetch::url`。
    let mut queued = 0u32;
    for item in items {
        let Ok(url) = luna_fetch::url::normalize(&item.url) else {
            continue;
        };
        // 标题缺失时回落主机名——绝不产出一篇无名笔记。
        let title = {
            let trimmed = item.title.trim();
            if trimmed.is_empty() {
                luna_fetch::url::host_of(&url)
            } else {
                trimmed.to_owned()
            }
        };
        state.processing.enqueue(
            crate::processing_queue::ProcTaskKind::Fetch {
                url,
                title: title.clone(),
                media,
                into_note: None,
                folder: folder.clone(),
                link_from: link_from.clone(),
            },
            title,
        );
        queued += 1;
    }
    if queued == 0 {
        return Err(luna_fetch::FetchError::UnsupportedUrl.code().to_owned());
    }
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(queued)
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    fn item(size: Option<f64>) -> UrlFetchItemDto {
        UrlFetchItemDto {
            url: "https://a.com/v".into(),
            title: "v".into(),
            filesize: size,
        }
    }

    const MIB: f64 = 1024.0 * 1024.0;

    /// 体积未知时**不阻断**：宁可让它跑到失败，也不因"猜不出多大"就拒绝一次合理的抓取
    /// （站点常不给 filesize，尤其是 DASH 分片流）。
    #[test]
    fn disk_precheck_does_not_block_when_size_unknown() {
        assert!(disk_headroom_verdict(Some(0), &[item(None), item(None)]).is_ok());
        // 查不到剩余空间（未开库 / 网络卷 / 权限）时同样不阻断——那是另一条路径该报的错。
        assert!(disk_headroom_verdict(None, &[item(Some(1e12))]).is_ok());
    }

    /// 需求量超过剩余空间即拒绝，且给的是可本地化的稳定码（PRD-05 §5 预检）。
    #[test]
    fn disk_precheck_rejects_when_over_free_space() {
        let err = disk_headroom_verdict(Some(10 * 1024 * 1024), &[item(Some(100.0 * MIB))])
            .expect_err("应拒绝");
        assert_eq!(err, "FETCH_INSUFFICIENT_DISK");
    }

    /// 预检求**整批之和**（一次勾 30 集课程时，单集放得下不等于整批放得下）。
    #[test]
    fn disk_precheck_sums_the_whole_batch() {
        let free = Some(1024 * 1024 * 1024); // 1 GiB
        assert!(
            disk_headroom_verdict(free, &[item(Some(100.0 * MIB))]).is_ok(),
            "单件 100 MiB 放得下"
        );
        let batch: Vec<UrlFetchItemDto> = (0..5).map(|_| item(Some(100.0 * MIB))).collect();
        assert!(
            disk_headroom_verdict(free, &batch).is_err(),
            "5 × 100 MiB 在 3 倍余量下需 1.5 GiB，放不下"
        );
    }

    /// **音频档的 WAV 产物比源更大**（16k mono 16bit ≈31 KB/s vs 128 kbps AAC ≈16 KB/s），
    /// 故峰值是"源 1 份 + WAV 2 份"。这条锁住那个曾经写反的推理：一份 100 MiB 的音频源在
    /// 只剩 250 MiB 的卷上**必须被拒**——初版按 2.2 倍算只需 220 MiB，会错误放行。
    #[test]
    fn disk_precheck_accounts_for_wav_being_larger_than_the_source() {
        assert!(
            disk_headroom_verdict(Some(250 * 1024 * 1024), &[item(Some(100.0 * MIB))]).is_err(),
            "源 100 MiB → 临时区 100 + WAV 约 200 = 300 MiB，250 MiB 放不下"
        );
        // 同样一份源，卷上有 400 MiB 时应放行（预检不该过度保守到误拒）。
        assert!(disk_headroom_verdict(Some(400 * 1024 * 1024), &[item(Some(100.0 * MIB))]).is_ok());
    }
}

/// **按需取回媒体**（PRD-05-006「仅存链接」的回补路径）：对一篇只有文稿与来源地址的
/// 转写笔记，重新抓取媒体并登记为它的分片，从而点亮回放与说话人细分。
///
/// 为什么是"入队一件抓取"而不是新写一条下载路径：取回媒体与首次抓取是**同一件事**——
/// 下载 → 落库 → 登记；差别只在"笔记已经存在、段落已经在库里"。故走同一队列、同一进度、
/// 同一取消与重试（PRD-05-010/012），只是任务载荷带上目标笔记。
///
/// # Errors
/// 严格本地拦截、工具不可用、笔记不存在、没有来源地址、或它已经有本地媒体时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn refetch_note_media(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    media: String,
) -> Result<(), String> {
    if state.processing.has_active(&note_rel, "fetch") {
        return Ok(()); // 同一笔记已在取回，幂等不重复入队
    }
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), &app);
        }
        return Err(luna_fetch::FetchError::StrictLocalBlocked.code().to_owned());
    }
    let settings = state.settings_snapshot();
    super::processing::resolve_fetch_tool(&settings, &state.models_root(&app))?;

    let (url, title) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        if !sorted_media_parts(&doc.meta.media).is_empty() {
            return Err(coded_error(
                &app,
                "FETCH_MEDIA_ALREADY_PRESENT",
                "fetch-media-already-present",
            ));
        }
        let url = doc
            .meta
            .source_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| coded_error(&app, "FETCH_NO_SOURCE_URL", "fetch-no-source-url"))?
            .to_owned();
        (url, note_task_label(&note_rel))
    };
    // 取回**只补媒体**，不重跑转写——文稿已经在库里且可能已被用户校对过，重转会把它覆盖掉。
    // `media` 只接受「仅音频 / 视频」：`link_only` 在这里自相矛盾（取回的目的正是留下媒体），
    // 故由 `from_setting` 的回落把它归入音频档——那是"取回最小可用的一份"。
    state.processing.enqueue(
        crate::processing_queue::ProcTaskKind::Fetch {
            url,
            title: title.clone(),
            // 取回的形态只在「音频 / 视频」之间选；`link_only` 在此自相矛盾（下完就删
            // 等于没取回），故显式归一到音频档而不是靠调用方自觉。
            media: match luna_fetch::FetchMediaKind::from_setting(&media) {
                luna_fetch::FetchMediaKind::Video => luna_fetch::FetchMediaKind::Video,
                _ => luna_fetch::FetchMediaKind::Audio,
            },
            into_note: Some(note_rel),
            folder: None,
            link_from: None,
        },
        // 展示标签用笔记名（`title` 即由它派生）——曾误写成 `note_task_label("")`，
        // 那恒为空串，取回任务在队列面板上是一行空白。
        title,
    );
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(())
}
