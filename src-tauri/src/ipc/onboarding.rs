//! 首启引导、装机就绪状态与硬件档位命令域（PRD-14，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! **步骤顺序与三态判定的唯一事实源在这里**（ADR-114 ②）：前端只按下发的 `steps`
//! 渲染与推进，不自己判"这一步算不算完成"。此前"完成"= 用户点过「继续」，于是
//! 跳过与完成不可区分，PRD-14 §5 的「完成设置」提示因此无从实现。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;
use crate::platform::permissions::{PermissionKind, PermissionStatus};
use crate::startup::{StartupDiagnosticsDto, StartupMilestone, StartupTimeline};

/// 引导步骤（顺序即呈现顺序，PRD-14 §3 修订版）。
///
/// Vault 排第二：它是**唯一必选项**，越早解决用户越早能「先进入应用」（Obsidian /
/// Logseq 首屏即是开库）。「AI 在哪跑」仍严格排在「模型准备」之前（PRD-14-005）。
pub(crate) const ONBOARDING_STEP_IDS: [&str; 7] = [
    "welcome",
    "vault",
    "compute",
    "models",
    "permissions",
    "shortcuts",
    "first_task",
];

/// 步骤三态（PRD-14-060 修订）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStepState {
    /// 尚未到访。
    Pending,
    /// 到访过，但它所代表的真实条件仍不成立（模型没装、权限没给、任务没做）。
    Skipped,
    /// 真实条件成立。
    Done,
}

/// 「装机就绪状态可能变了」（`setup://changed`）：引擎就绪核查完清单之后发一次——那是
/// 启动后第一次能回答「本机文本模型到底装没装」的时刻；不发的话横幅要等窗口重新聚焦
/// 才会更新。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "setup://changed")]
pub struct SetupStatusChanged {}

/// 一步的 id 与状态。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OnboardingStepStatusDto {
    pub id: String,
    pub state: OnboardingStepState,
}

/// 装机就绪状态（`setup_status`）：引导页、「完成设置」横幅与就地引导共用的一份事实。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SetupStatusDto {
    /// 引导是否已收场（`onboarding.completed`）——收场后才进工作区；剩余项由横幅承接。
    pub completed: bool,
    /// 算力此刻落在哪。
    pub compute_location: crate::cloud::ComputeLocation,
    /// 转写模型就绪：完全本机形态 = 推荐档已装；服务端形态 = 随包 tiny 兜底即可。
    ///
    /// **这是「模型准备那一步做完了吗」，不是「现在能不能转写」**——两者自随包档落地起
    /// 就分家了：高配机器的推荐档是 Large v3 Turbo，没装即为假，而随包 tiny 一直在、
    /// 转写一直能跑。想问后者的用 [`SetupStatusDto::asr_usable`]。
    pub asr_ready: bool,
    /// 此刻**能不能真的转写**：任何一档可用的转写权重在场即为真（随包档也算）。
    ///
    /// 「第一个任务」问的是这个。用 `asr_ready` 去判的话，一台只有随包档的机器会在
    /// 模型页看到「内置模型已就绪，零下载即可开始转写」，翻到下一屏却被告知「转写模型
    /// 还没准备好」——两屏对同一件事说相反的话，而功能上它明明跑得起来（真机 2026-09-11）。
    pub asr_usable: bool,
    /// 文本模型就绪：服务端形态恒为真；本机形态 = 随包引擎里装着可用的默认对话模型。
    pub llm_ready: bool,
    /// 此刻**能不能翻译**：服务端形态恒为真；本机形态 = 「翻译与实时档」就绪（专用翻译模型
    /// 或继承来的对话模型）。「第一个任务」的弹窗翻译问的是这个，不是 `llm_ready`——只装了
    /// 专用翻译模型时两者不同。
    pub translate_ready: bool,
    pub microphone: PermissionStatus,
    pub accessibility: PermissionStatus,
    pub notification: PermissionStatus,
    pub vault_bound: bool,
    /// 首个 AI 任务是否已在这台设备上成功过一次（语音输入落地或弹窗翻译出结果）。
    pub first_task_done: bool,
    /// 各步骤三态，顺序即呈现顺序。
    pub steps: Vec<OnboardingStepStatusDto>,
    /// 计入「完成设置 (n/N)」的步骤 id（按引导顺序）。哪些算数由这里裁决，前端只按
    /// `steps` 里的三态去数——两处各写一份"谁该计入"，迟早对同一件事给出两个分母。
    ///
    ///  分母 = 步进条的步数（七步），故两处数字对得上；唯一不计入的是**权限全被明确
    ///  拒绝**的情形——应用再问也弹不出系统框，它就没有可补的对象了
    /// （见 `derive_steps`）。
    pub tracked_ids: Vec<String>,
    /// 用户关掉了「完成设置」提醒。横幅与启动概览的「有事才弹」共读这一个事实——
    /// 只关一半等于没关。
    pub reminder_dismissed: bool,
}

/// 首启引导状态（仅读写 `onboarding.*` Device 设置）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OnboardingStateDto {
    /// 是否已完成首启引导。
    pub completed: bool,
    /// 到访过的步骤 id（来自 `onboarding.completed_steps` CSV）。
    pub completed_steps: Vec<String>,
}

/// 解析 `onboarding.completed_steps` CSV 为去空步骤列表。
pub(crate) fn parse_completed_steps(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// 三态判定（纯函数，可单测）：条件成立即 Done；否则到访过为 Skipped、没到访为 Pending。
#[must_use]
pub(crate) fn derive_step_state(visited: bool, condition: bool) -> OnboardingStepState {
    if condition {
        OnboardingStepState::Done
    } else if visited {
        OnboardingStepState::Skipped
    } else {
        OnboardingStepState::Pending
    }
}

/// 判定输入（把「问系统」与「算状态」分开，后者可 headless 单测）。
pub(crate) struct SetupFacts {
    pub visited: Vec<String>,
    pub compute_location: crate::cloud::ComputeLocation,
    pub asr_ready: bool,
    pub llm_ready: bool,
    pub microphone: PermissionStatus,
    pub accessibility: PermissionStatus,
    pub vault_bound: bool,
    pub first_task_done: bool,
}

/// 由事实推导各步三态 + 计入「完成设置」的步骤 id（步数以 `ONBOARDING_STEP_IDS` 为准）。
#[must_use]
pub(crate) fn derive_steps(facts: &SetupFacts) -> (Vec<OnboardingStepStatusDto>, Vec<String>) {
    let visited = |id: &str| facts.visited.iter().any(|v| v == id);
    // `Unsupported` = 这个平台没有这项权限可授（Windows 上两项都是）——没有东西可补，
    // 视为满足；否则 Windows 用户的「完成设置」横幅永远收不掉。
    let satisfied = |status: PermissionStatus| {
        matches!(
            status,
            PermissionStatus::Granted | PermissionStatus::Unsupported
        )
    };
    let permissions_ok = satisfied(facts.microphone) && satisfied(facts.accessibility);
    // 权限这一步什么时候还该计入「完成设置」：**已经满足**（它贡献一个 done，分母里
    // 少了它反而不诚实），**或**还有东西能在应用里补（至少一项没问过）。
    //
    // 两者皆非 = 全被明确拒绝：应用再问也弹不出那个系统框（macOS 只弹一次），只能请
    // 用户去系统设置——那件事归各功能视图的「能力未就绪」空态（PRD-14-061），不是横幅
    // 每次启动都该催的。不这么判的话，明确拒绝过辅助功能的用户会被这条横幅跟一辈子。
    let permissions_askable = [facts.microphone, facts.accessibility]
        .iter()
        .any(|status| matches!(status, PermissionStatus::NotDetermined));
    let permissions_counted = permissions_ok || permissions_askable;
    let steps: Vec<OnboardingStepStatusDto> = ONBOARDING_STEP_IDS
        .iter()
        .map(|&id| {
            let state = match id {
                // 没有"真实条件"的两步：到访即完成。
                "welcome" => derive_step_state(visited(id), visited(id)),
                // 任何一条路都是完整可用的形态；连上服务端等于已经做过这个决定。
                "compute" => derive_step_state(
                    visited(id),
                    visited(id) || facts.compute_location.is_server(),
                ),
                "vault" => derive_step_state(visited(id), facts.vault_bound),
                // 服务端形态下本机档只是断线兜底，**不是这一步的前置**：那条路对用户的承诺
                // 是「零下载、零本地占用、即开即用」（PRD-14-011 ③），而把完成与否挂在一个
                // 本机档上，等于让云用户被横幅一直催去做一件他不需要做的事——引导第三步
                // 刚说完零下载，第四步就把下载列进待办。故与 compute 同构：到访即完成，
                // 本机备用的就绪状态由模型页如实标注，不进待办。
                //
                // 不做成「连上服务端即 Done」（compute 那一步是那样）：那会让用户从没看过
                // 本机备用是哪一档、装没装，而断网那天正是他要靠它的时候。
                "models" => {
                    if facts.compute_location.is_server() {
                        derive_step_state(visited(id), visited(id))
                    } else {
                        derive_step_state(visited(id), facts.asr_ready && facts.llm_ready)
                    }
                }
                "permissions" => derive_step_state(visited(id), permissions_ok),
                // 快捷键开箱即绑好，这一步只是"让你知道有这三个键、顺手改掉不顺手的"——
                // 没有任何"真实条件"可判，故到访即完成（同 welcome）。
                "shortcuts" => derive_step_state(visited(id), visited(id)),
                "first_task" => derive_step_state(visited(id), facts.first_task_done),
                _ => unreachable!("步骤表与判定表同一来源"),
            };
            OnboardingStepStatusDto {
                id: id.to_owned(),
                state,
            }
        })
        .collect();
    // `tracked_ids` = 「完成设置 (n/N)」的**分母**，也是「还差：…」那张名单的取值范围。
    //
    // **步进条几步，横幅就几步**（2026-09-10 真机，经过见 CHANGELOG）：两个数字对不上时，
    // 用户不会去想"哪几步被排除了"，只会觉得其中一个是错的。到访即完成的那几步照常计入
    // ——它们没有"可补的对象"，但按过一次「继续」就是 Done，分子分母同时 +1，n 与「还差」
    // 的条数之和恒等于 N。
    //
    // 唯一的例外是权限——理由在上面 `permissions_counted` 那里，不在这儿说第二遍。
    let tracked_ids = steps
        .iter()
        .filter(|step| step.id != "permissions" || permissions_counted)
        .map(|step| step.id.clone())
        .collect();
    (steps, tracked_ids)
}

/// 转写模型就绪（纯函数）：推荐档已就位，**或**用户自己装了任何一档非内置的转写模型——
/// 装了比推荐档更大的模型不该被判成「没准备」。内置 tiny 不算「自己装了」，否则本机
/// 形态下这一项永远为真、推荐失去意义；服务端形态的推荐本身就是内置档，走前半句。
#[must_use]
pub(crate) fn asr_ready_from(
    entries: &[crate::model_store::ModelEntryDto],
    recommended_id: &str,
) -> bool {
    entries.iter().any(|entry| {
        (entry.id == recommended_id && entry.status != "downloadable")
            || (entry.status == "installed"
                && entry
                    .effective_capabilities
                    .iter()
                    .any(|capability| capability == "transcribe"))
    })
}

fn onboarding_bool(state: &AppState, key: &str) -> bool {
    state.settings_snapshot().get_bool(key).unwrap_or(false)
}

/// 到访记录，**只保留当前步骤表里的 id**：老安装的记录里可能还有已废弃的 `hardware` /
/// `done`，读时丢掉、下次写入即自然清理。
fn visited_steps(state: &AppState) -> Vec<String> {
    parse_completed_steps(
        &state
            .settings_snapshot()
            .get_str("onboarding.completed_steps")
            .unwrap_or_default(),
    )
    .into_iter()
    .filter(|id| ONBOARDING_STEP_IDS.contains(&id.as_str()))
    .collect()
}

/// 读取装机就绪状态。
///
/// 转写模型就绪只看本机目录；文本模型就绪看引擎最近一次编排记下的已装清单——
/// 还没问到过引擎时如实回 `false`，不猜。
#[tauri::command]
#[specta::specta]
pub(crate) fn setup_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> SetupStatusDto {
    // 这台设备上早就产生过 AI 结果的话，「第一个任务」就已经做过了（见下方 helper）。
    backfill_first_task_from_usage(&app, &state);
    let location = current_chat_compute_location(&app, &state);
    let tier = crate::hardware::probe_tier();
    let recommended = crate::model_store::recommended_asr_for(tier, location);
    let store = crate::model_store::ModelStore::load(state.models_root(&app));
    let asr_ready = asr_ready_from(&store.list(), &recommended.id);
    // 「能不能转写」与「推荐档备齐没有」是两个问题，判据也各有一处：这一个问的是
    // 装配真正会拿到哪个权重（`fallback_asr_weight`，随包档优先），不在这里另写一遍。
    let asr_usable = crate::model_store::fallback_asr_weight(&state.models_root(&app)).is_some();
    // 服务端形态下对话在服务端跑，本机装没装与它无关。
    let llm_ready = location.is_server() || state.local_chat_ready(&app);
    let translate_ready = location.is_server() || state.local_translate_ready(&app);
    let microphone = check_permission(PermissionKind::Microphone).status;
    let accessibility = check_permission(PermissionKind::Accessibility).status;
    let notification = check_permission(PermissionKind::Notification).status;
    let facts = SetupFacts {
        visited: visited_steps(&state),
        compute_location: location,
        asr_ready,
        llm_ready,
        microphone,
        accessibility,
        vault_bound: state.vault_root().is_some(),
        first_task_done: onboarding_bool(&state, "onboarding.first_task_done"),
    };
    let (steps, tracked_ids) = derive_steps(&facts);
    SetupStatusDto {
        completed: onboarding_bool(&state, "onboarding.completed"),
        compute_location: location,
        asr_ready,
        asr_usable,
        llm_ready,
        translate_ready,
        microphone,
        accessibility,
        notification,
        vault_bound: facts.vault_bound,
        first_task_done: facts.first_task_done,
        steps,
        tracked_ids,
        reminder_dismissed: onboarding_bool(&state, "onboarding.setup_reminder_dismissed"),
    }
}

/// 读取首启引导状态。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_state(
    state: tauri::State<'_, AppState>,
) -> Result<OnboardingStateDto, String> {
    Ok(OnboardingStateDto {
        completed: onboarding_bool(&state, "onboarding.completed"),
        completed_steps: visited_steps(&state),
    })
}

/// 标记一个引导步骤**到访过**（追加去重到 `onboarding.completed_steps`）。
/// 未知 id 拒绝——步骤表是唯一来源。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_mark_step(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    step: String,
) -> Result<(), String> {
    if !ONBOARDING_STEP_IDS.contains(&step.as_str()) {
        return Err(format!("ONBOARDING_UNKNOWN_STEP: {step}"));
    }
    let mut steps = visited_steps(&state);
    if steps.iter().any(|item| item == &step) {
        return Ok(());
    }
    steps.push(step);
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "onboarding.completed_steps",
        SettingValueDto::Str(steps.join(",")),
    )?;
    let _ = SettingsChanged {
        key: "onboarding.completed_steps".to_owned(),
    }
    .emit(&app);
    Ok(())
}

/// 记一条「进了哪一步」（PRD-14 §1 的本机漏斗记录）。
///
/// 由壳层在切步时调用。**只写步骤 id 与时刻**，不带任何用户内容；未知 id 直接丢弃而不是
/// 报错——它是度量，不该因为前端多报了一个名字就在界面上冒出一条错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_funnel_enter(app: tauri::AppHandle, step: String) {
    if !ONBOARDING_STEP_IDS.contains(&step.as_str()) {
        return;
    }
    record_funnel(&app, &step);
}

/// 读回本机记录（`scripts/onboarding-funnel.py` 之外的第二个读取点；设置界面**不**呈现它
/// ——用户不关心自己在哪一屏停了多久，这份数据服务的是真机作业）。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_funnel(
    app: tauri::AppHandle,
) -> Vec<crate::onboarding_funnel::FunnelMarkDto> {
    app.try_state::<crate::onboarding_funnel::OnboardingFunnel>()
        .map(|funnel| funnel.snapshot())
        .unwrap_or_default()
}

fn record_funnel(app: &tauri::AppHandle, step: &str) {
    let Some(funnel) = app.try_state::<crate::onboarding_funnel::OnboardingFunnel>() else {
        return;
    };
    #[allow(clippy::cast_precision_loss)] // 毫秒时刻远在 f64 的精确整数范围内（见 DTO 注释）
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64);
    funnel.enter(step, at_ms);
}

/// 标记首启引导整体收场（进工作区）。
///
/// 收场前被暂扣的启动通知（本机没装 LLM 之类的「需你决定」类）在此刻补发——它们
/// 说的正是引导刚讲过的事，引导期间弹出来只会抢戏（ADR-114 ③）。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_complete(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // 这一次是不是**首次**收场——决定下面要不要把「完成设置」提醒恢复成开。
    let was_completed = onboarding_bool(&state, "onboarding.completed");
    record_funnel(&app, crate::onboarding_funnel::COMPLETED_MARK);
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "onboarding.completed",
        SettingValueDto::Bool(true),
    )?;
    let _ = SettingsChanged {
        key: "onboarding.completed".to_owned(),
    }
    .emit(&app);
    // 收场即把「完成设置」提醒恢复成开——收场那一屏刚承诺过它。`setup_reminder_dismissed`
    // 是 `Internal` 键：设置页不渲染、搜索也搜不到，关掉之后**没有任何回来的路**（真机
    // 2026-09-10：横幅一直不出，而收场那一屏还指着一个永远不会出现的东西）。不想被提醒的
    // 人在横幅上再点一次即可，那颗键就在眼前。
    //
    // **只有首次收场这么做**：重入时用户多半正是从那条横幅点进来补设置的，收场再替他把
    // 关掉的提醒打开，等于撤销一个他刚做过的决定；"那一屏承诺过横幅"只对第一次成立。
    if !was_completed {
        set_setting_value(
            state.copy(),
            &state.settings,
            &state.settings_persistence,
            "onboarding.setup_reminder_dismissed",
            SettingValueDto::Bool(false),
        )?;
        let _ = SettingsChanged {
            key: "onboarding.setup_reminder_dismissed".to_owned(),
        }
        .emit(&app);
    }
    if let Some(center) = app.try_state::<NotificationCenter>() {
        center.flush_deferred(&app);
    }
    Ok(())
}

/// 开关「完成设置」提醒（PRD-14-064）。
///
/// 关掉的是提醒，不是能力：横幅不再出现，启动概览也不再因为"设置还没做完"而弹（那两件
/// 事是同一个提醒的两个面，只关一半等于没关）；引导本身照常可从设置里重新运行。
/// 一个永远收不掉、又没有关闭办法的横幅，最后被读成的是"这个应用坏了"。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_set_reminder_dismissed(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    dismissed: bool,
) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "onboarding.setup_reminder_dismissed",
        SettingValueDto::Bool(dismissed),
    )?;
    let _ = SettingsChanged {
        key: "onboarding.setup_reminder_dismissed".to_owned(),
    }
    .emit(&app);
    Ok(())
}

/// 「第一个任务」是不是其实早就做过了——由**这台设备上有没有产生过一次成功的 AI 结果**
/// 补判（PRD-14-050 的意图是"它真的能用"，不是"你必须从那两个入口之一进来"）。
///
/// 引导里的两个入口（语音输入落地 / 弹窗翻译出译文）会直接写这个标志，但它们不是产生
/// AI 结果的全部路径：录音转写、媒体转写、AI 对话、笔记优化都不经过它们。真机上因此出现
/// 过一台做了一千多次 AI 任务、听写十一小时的机器，「完成设置」横幅仍在催「第一个任务」
/// ——一个用户已经用行动证明过、却永远清不掉的清单项。
///
/// 补判放在这里而不是散到各条成功路径上：那要在十来处调用点各记得写一次，漏一处就又
/// 回到同一个症状。检出后即写标志，此后短路，不再查用量。
fn backfill_first_task_from_usage(app: &tauri::AppHandle, state: &AppState) {
    if onboarding_bool(state, "onboarding.first_task_done") {
        return;
    }
    let summary = crate::usage::merged_usage_summary(&state.device_usage, &state.vault_arc());
    if summary.records > 0 {
        mark_first_task_done(app, state);
    }
}

/// 首个 AI 任务成功了一次（语音输入落地 / 弹窗翻译出结果）→ 写一次、只写一次。
///
/// 写在**产生结果的那条路径**上而不是由前端上报：前端看到的是自己那扇窗的事件，
/// 而引导页与 HUD 不在同一扇窗里（ADR-003）。
pub(crate) fn mark_first_task_done(app: &tauri::AppHandle, state: &AppState) {
    if onboarding_bool(state, "onboarding.first_task_done") {
        return;
    }
    if let Err(error) = set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        "onboarding.first_task_done",
        SettingValueDto::Bool(true),
    ) {
        eprintln!("[onboarding] 记录首个任务完成失败：{error}");
        return;
    }
    let _ = SettingsChanged {
        key: "onboarding.first_task_done".to_owned(),
    }
    .emit(app);
}

/// 随包示例语音在应用资源目录里的相对路径（`tauri.conf.json` 的 `bundle.resources` 落点）。
pub(crate) const ONBOARDING_SAMPLE_REL: &str = "samples/lunarca-sample-en.wav";

/// 随包示例语音在不在（前端据此决定出不出那张卡）。
///
/// 单独一条命令而不是塞进 `setup_status`：那份状态每次窗口聚焦都重读，而这是一个
/// 全程不变的事实——它只在「第一个任务」那一步挂载时问一次就够了。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_sample_available(app: tauri::AppHandle) -> bool {
    sample_path(&app).is_some_and(|path| path.is_file())
}

fn sample_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager as _;
    app.path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join(ONBOARDING_SAMPLE_REL))
}

/// 「转写一段示例」——PRD-14-051 的替代成功路径。
///
/// 它存在是因为一条真实的死胡同：拒绝了麦克风、又没有本机语言模型的用户，语音输入与
/// 弹窗翻译两张卡同时是灰的，而「第一个任务」是激活漏斗的最后一环。随包示例语音让他
/// 不必交出任何权限、也不必等任何下载，就能真跑一次本机转写。
///
/// 送进的是与「拖入一个音频文件」**同一条**导入路径（`import_media_paths`），不另起调用栈：
/// 转写、落笔记、进处理队列、记用量全部照旧。于是 `onboarding.first_task_done` 由**真实
/// 产生的 AI 结果**经用量回填写成，而不是给这条路另开一个标记——「第一个任务」判的是
/// 「它真的能用」，不是「你从哪个入口进来的」。
#[tauri::command]
#[specta::specta]
pub(crate) fn onboarding_transcribe_sample(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    let path = sample_path(&app)
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("ONBOARDING_SAMPLE_MISSING: {ONBOARDING_SAMPLE_REL}"))?;
    let paths = vec![path.to_string_lossy().into_owned()];
    super::transcribe::import_media_paths(app.clone(), state, paths)
}

/// 设备算力摘要 + 推荐模型档位（PRD-14-010/011）。采样在阻塞线程，避免占用 async worker。
#[tauri::command]
#[specta::specta]
pub(crate) async fn hardware_summary(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<crate::hardware::HardwareSummaryDto, String> {
    let location = current_chat_compute_location(&app, &state);
    tokio::task::spawn_blocking(move || crate::hardware::probe(location))
        .await
        .map_err(|e| e.to_string())
}

/// 启动快照（PRD-14-066）：主窗首帧需要的几份事实**一次**取回——此前外观、引导门控、
/// 概览门控、状态栏各自读一遍全量设置，同一份数据在启动期跨进程走了四趟。
#[derive(Debug, Clone, Serialize, Type)]
pub struct AppBootstrapDto {
    pub onboarding: OnboardingStateDto,
    pub vault: super::vaults::VaultStatusDto,
    pub settings: Vec<SettingSpecDto>,
}

/// 读取启动快照。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_bootstrap(state: tauri::State<'_, AppState>) -> Result<AppBootstrapDto, String> {
    let settings = state
        .settings
        .lock()
        .map(|settings| settings_spec_dtos(&settings))
        .map_err(|_| "Settings 锁中毒".to_string())??;
    Ok(AppBootstrapDto {
        onboarding: OnboardingStateDto {
            completed: onboarding_bool(&state, "onboarding.completed"),
            completed_steps: visited_steps(&state),
        },
        vault: super::vaults::vault_status_of(&state),
        settings,
    })
}

/// 前端打一枚启动里程碑（只允许「首帧」「工作区就绪」，其余由 Rust 自己打）。
///
/// 「首帧」的第一次到达同时是**主窗显示的时刻**（PRD-14-066）：主窗隐藏创建，前端画出
/// 第一面（引导页或工作区）之后才亮，用户看不到空白 WebView。
#[tauri::command]
#[specta::specta]
pub(crate) fn startup_mark(
    app: tauri::AppHandle,
    milestone: StartupMilestone,
) -> Result<(), String> {
    if !milestone.frontend_allowed() {
        return Err(format!("STARTUP_MILESTONE_NOT_FRONTEND: {milestone:?}"));
    }
    let newly = app
        .try_state::<StartupTimeline>()
        .map(|timeline| timeline.mark(milestone))
        .unwrap_or(false);
    if newly && milestone == StartupMilestone::FirstFrame {
        crate::show_main_window(&app);
    }
    Ok(())
}

/// 读回本次与上一次启动时间线（设置 › 系统与诊断）。
#[tauri::command]
#[specta::specta]
pub(crate) fn startup_diagnostics(app: tauri::AppHandle) -> StartupDiagnosticsDto {
    app.try_state::<StartupTimeline>()
        .map(|timeline| timeline.snapshot())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::ComputeLocation;

    fn facts() -> SetupFacts {
        SetupFacts {
            visited: vec![],
            compute_location: ComputeLocation::OnDevice,
            asr_ready: false,
            llm_ready: false,
            microphone: PermissionStatus::NotDetermined,
            accessibility: PermissionStatus::Denied,
            vault_bound: false,
            first_task_done: false,
        }
    }

    #[test]
    fn step_order_puts_vault_second_and_compute_before_models() {
        let ids = ONBOARDING_STEP_IDS;
        assert_eq!(ids[1], "vault");
        let pos = |id: &str| ids.iter().position(|x| *x == id).unwrap();
        assert!(pos("compute") < pos("models"));
        assert!(pos("models") < pos("permissions"));
        assert_eq!(ids.last(), Some(&"first_task"));
    }

    /// 跳过 ≠ 完成：到访过但条件不成立必须是 Skipped，横幅才有东西可提示。
    #[test]
    fn visited_without_condition_is_skipped_not_done() {
        assert_eq!(derive_step_state(true, false), OnboardingStepState::Skipped);
        assert_eq!(
            derive_step_state(false, false),
            OnboardingStepState::Pending
        );
        assert_eq!(derive_step_state(false, true), OnboardingStepState::Done);
    }

    /// 全新安装：七步全为 Pending，「完成设置」的分母就是这七步。
    ///
    /// 分母对齐步进条（2026-09-10 真机）：两个数字对不上时，用户不会去想"哪几步被
    /// 排除了"，只会觉得其中一个是错的。
    #[test]
    fn fresh_install_tracks_every_step() {
        let (steps, tracked) = derive_steps(&facts());
        assert_eq!(steps.len(), ONBOARDING_STEP_IDS.len());
        assert!(steps
            .iter()
            .all(|s| s.state == OnboardingStepState::Pending));
        assert_eq!(tracked, ONBOARDING_STEP_IDS);
    }

    /// 收场必须同时写两个键：`completed` 与「把提醒恢复成开」。
    ///
    /// 只写前者的话，关过提醒的用户重跑一遍引导后横幅照旧不出，而收场那一屏刚
    /// 承诺过它——`setup_reminder_dismissed` 是 `Internal` 键，设置页不渲染、搜索
    /// 搜不到，关掉之后没有任何回来的路（真机 2026-09-10）。判据落在源码上：
    /// 这两次写入必须都在 `onboarding_complete` 体内。
    #[test]
    fn completing_onboarding_restores_the_setup_reminder() {
        let source = include_str!("onboarding.rs");
        let body = source
            .split("pub(crate) fn onboarding_complete(")
            .nth(1)
            .expect("找不到 onboarding_complete")
            .split("\n/// ")
            .next()
            .expect("函数体应在下一条文档注释之前结束");
        assert!(
            body.contains(r#""onboarding.completed""#),
            "收场没写 onboarding.completed"
        );
        assert!(
            body.contains(r#""onboarding.setup_reminder_dismissed""#),
            "收场没把「完成设置」提醒恢复成开——关过提醒的用户从此再也见不到横幅，\
             而收场那一屏刚承诺过它"
        );
        assert!(
            body.contains("if !was_completed"),
            "恢复提醒没挡在「首次收场」这道门后面——重入（覆盖层）时用户多半正是从横幅\
             点进来补设置的，收场再把他关掉的提醒打开，等于替他撤销一个刚做过的决定"
        );
    }

    /// 处理任务成功收尾必须广播「装机状态可能变了」。
    ///
    /// 「第一个任务」的判据是这台设备上有没有产生过一次 AI 结果，而那个判定住在
    /// `setup_status` 的用量回填里——没有这条广播，它只会在窗口重新聚焦时才重跑。
    /// 真机 2026-09-11：引导里点「转写示例」，笔记落了库、用量记了两条，而那一步一直
    /// 不打勾，同一屏上还写着「转写完成后这一步会自动打勾」。
    ///
    /// 判据落在源码上（这条线要真跑起来得有 Tauri 运行时）：成功收尾那一段里必须发它。
    #[test]
    fn a_finished_task_broadcasts_that_setup_may_have_changed() {
        let source = include_str!("processing.rs");
        let body = source
            .split("if completed {")
            .nth(1)
            .expect("找不到处理任务的成功收尾分支");
        let branch = &body[..body.find("\n        }").unwrap_or(body.len())];
        assert!(
            branch.contains("SetupStatusChanged"),
            "成功收尾没广播 setup://changed——「第一个任务」会停在未完成，\
             而界面刚承诺过它会自动打勾"
        );
    }

    /// 权限全被明确拒绝 → 不再计入「完成设置」。
    ///
    /// 应用没法再弹那个系统框（macOS 只弹一次），横幅每次启动催一遍也改变不了任何事；
    /// 去系统设置补授权归各功能视图的「能力未就绪」空态（PRD-14-061）。
    /// 反向验证：把 `permissions_actionable` 恒设为 true，这条当场红。
    #[test]
    fn fully_denied_permissions_stop_being_counted() {
        let mut f = facts();
        f.microphone = PermissionStatus::Denied;
        f.accessibility = PermissionStatus::Denied;
        let (_, tracked) = derive_steps(&f);
        // 分母从七退到六：其余六步照常计入（分母对齐步进条），只有权限掉出来。
        assert_eq!(
            tracked,
            [
                "welcome",
                "vault",
                "compute",
                "models",
                "shortcuts",
                "first_task"
            ]
        );

        // 只要还有一项没问过，它就仍然可补——那一项应用还能自己弹框。
        f.microphone = PermissionStatus::NotDetermined;
        let (_, tracked) = derive_steps(&f);
        assert!(tracked.contains(&"permissions".to_owned()));

        // 已经授予的照常计入：它贡献一个 done，把它从分母里拿掉反而不诚实
        // （用户会看到「完成设置 3/3」而不是 4/4）。
        f.microphone = PermissionStatus::Granted;
        f.accessibility = PermissionStatus::Granted;
        let (_, tracked) = derive_steps(&f);
        assert!(tracked.contains(&"permissions".to_owned()));
    }

    fn state_of(steps: &[OnboardingStepStatusDto], id: &str) -> OnboardingStepState {
        steps.iter().find(|s| s.id == id).unwrap().state
    }

    /// 服务端形态：本机兜底档没装，也不该把「模型准备」挂在待办上——那条路承诺的是
    /// 零下载。同一份输入在完全本机形态下必须仍是 `Skipped`：那里它真是前置。
    #[test]
    fn server_compute_does_not_hang_models_on_the_local_fallback() {
        let mut f = facts();
        f.visited = vec!["models".to_owned()];
        f.llm_ready = true; // 服务端形态恒真
        f.asr_ready = false; // 本机兜底档没装

        for location in [
            ComputeLocation::OfficialCloud,
            ComputeLocation::Organization,
        ] {
            f.compute_location = location;
            let (steps, _) = derive_steps(&f);
            assert_eq!(
                state_of(&steps, "models"),
                OnboardingStepState::Done,
                "{location:?} 下本机兜底档不该成为「模型准备」的前置"
            );
        }

        f.compute_location = ComputeLocation::OnDevice;
        let (steps, _) = derive_steps(&f);
        assert_eq!(state_of(&steps, "models"), OnboardingStepState::Skipped);
    }

    /// 服务端形态但**没到访过**：仍是 `Pending`，不借「连上了服务端」自动打勾——
    /// 用户得知道断网那天接手的是哪一档。
    #[test]
    fn server_compute_still_wants_the_models_step_visited() {
        let mut f = facts();
        f.compute_location = ComputeLocation::OfficialCloud;
        f.llm_ready = true;
        let (steps, _) = derive_steps(&f);
        assert_eq!(state_of(&steps, "models"), OnboardingStepState::Pending);
    }

    #[test]
    fn server_compute_counts_as_decided_even_if_not_visited() {
        let mut f = facts();
        f.compute_location = ComputeLocation::OfficialCloud;
        let (steps, _) = derive_steps(&f);
        let compute = steps.iter().find(|s| s.id == "compute").unwrap();
        assert_eq!(compute.state, OnboardingStepState::Done);
    }

    #[test]
    fn everything_ready_leaves_nothing_remaining() {
        let f = SetupFacts {
            visited: ONBOARDING_STEP_IDS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            compute_location: ComputeLocation::OnDevice,
            asr_ready: true,
            llm_ready: true,
            microphone: PermissionStatus::Granted,
            accessibility: PermissionStatus::Granted,
            vault_bound: true,
            first_task_done: true,
        };
        let (steps, tracked) = derive_steps(&f);
        assert!(steps.iter().all(|s| s.state == OnboardingStepState::Done));
        // 全 Done 时 `tracked` 仍是全部七步——「还剩几项」由前端按三态去数，不是这里的事。
        assert_eq!(tracked, ONBOARDING_STEP_IDS);
    }

    /// Windows 上两项权限都查不到（`Unsupported`）——没有东西可补，必须算满足。
    #[test]
    fn unsupported_permissions_count_as_satisfied() {
        let mut f = facts();
        f.microphone = PermissionStatus::Unsupported;
        f.accessibility = PermissionStatus::Unsupported;
        let (steps, _) = derive_steps(&f);
        let p = steps.iter().find(|s| s.id == "permissions").unwrap();
        assert_eq!(p.state, OnboardingStepState::Done);
    }

    fn entry(id: &str, status: &str, caps: &[&str]) -> crate::model_store::ModelEntryDto {
        crate::model_store::ModelEntryDto {
            id: id.to_owned(),
            name: id.to_owned(),
            declared_capabilities: vec![],
            effective_capabilities: caps.iter().map(|c| (*c).to_owned()).collect(),
            managed_by: String::new(),
            origin: String::new(),
            license_label: String::new(),
            status: status.to_owned(),
            size_bytes: None,
            est_on_disk_bytes: None,
            on_disk_bytes: None,
            runtime_digest: None,
            quantization: None,
            estimated_memory_bytes: None,
            device_fit: String::new(),
            assigned_slots: vec![],
            partial_bytes: None,
            coreml_available: false,
            coreml_installed: false,
            coreml_size_bytes: None,
            coreml_partial_bytes: None,
            update_size_bytes: None,
        }
    }

    /// 装了比推荐档更大的转写模型也算就绪；只有内置 tiny 在场则不算（本机形态）。
    #[test]
    fn a_larger_downloaded_asr_model_counts_as_ready_but_bundled_alone_does_not() {
        let bundled = entry("whisper-tiny-bundled", "bundled", &["transcribe"]);
        let large = entry("whisper-large", "installed", &["transcribe"]);
        let small = entry("whisper-small", "downloadable", &["transcribe"]);
        assert!(!asr_ready_from(
            &[bundled.clone(), small.clone()],
            "whisper-small"
        ));
        assert!(asr_ready_from(
            &[bundled.clone(), small, large],
            "whisper-small"
        ));
        // 服务端形态：推荐就是内置档，在场即就绪。
        assert!(asr_ready_from(&[bundled], "whisper-tiny-bundled"));
    }

    #[test]
    fn permissions_need_both_microphone_and_accessibility() {
        let mut f = facts();
        f.visited = vec!["permissions".to_owned()];
        f.microphone = PermissionStatus::Granted;
        let (steps, _) = derive_steps(&f);
        let p = steps.iter().find(|s| s.id == "permissions").unwrap();
        assert_eq!(p.state, OnboardingStepState::Skipped);
    }
}
