//! 创建动作的**可用性快照**（PRD-07-067/068/069 / TDD-07 §2.2 / ADR-003）。
//!
//! 「添加内容」始终列出五类已支持动作；暂时执行不了的**照样在场**，但要说清为什么、以及
//! 怎么解决。判定全在这里，前端只渲染——它不该按文案猜权限，也不该按目录存在与否猜 Vault
//! 就绪（那两种猜法在真机上都会给出与执行结果相反的答案）。
//!
//! **只读，且真的只读**：本查询不弹权限窗口、不读笔记内容、不启动录音、不下载、不做
//! 联网探活。只有用户点了修复动作才会去申请权限或打开设置。一个"看一眼就弹系统授权框"
//! 的查询会在每次打开 Popover 时骚扰用户。
//!
//! **快照不是授权凭证**：执行命令自己会再权威校验一次。快照回答的是"现在点下去大概
//! 会怎样"，而两次之间世界可以变。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_specta::Event;

use super::AppState;
#[cfg(target_os = "macos")]
use crate::platform::permissions::PermissionKind;
use crate::platform::permissions::PermissionStatus;

/// 五类创建动作的稳定 id（与 `luna_config::CREATE_ACTIONS` 穷尽映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CreateActionIdDto {
    /// 文字速记 → 「速记」。
    TextQuickNote,
    /// 语音速记 → 「速记」。
    VoiceQuickNote,
    /// 新建笔记 → 「笔记」。
    NewNote,
    /// 开始录音 → 「录音」。
    StartRecording,
    /// 导入资料：音视频 → 「转写」，文档 → 文档文件夹（ADR-164 ⑩）。
    ImportMaterial,
}

impl CreateActionIdDto {
    /// 由 `luna-config` 词表的稳定 id 还原（未知 id → `None`）。
    ///
    /// **只有这一个方向是手写的**：反方向就是 serde 的 `snake_case` 序列化本身，那才是
    /// 真正的 wire 契约。再手写一个 `as_str` 会造出第二份"id 长什么样"的定义，而它与
    /// wire 值不一致时，前端收到的和 Rust 以为发出的是两回事——round-trip 测试因此比
    /// 对的是 serde 的输出，不是我自己写的那张表。
    #[must_use]
    pub fn from_str(id: &str) -> Option<Self> {
        match id {
            "text_quick_note" => Some(Self::TextQuickNote),
            "voice_quick_note" => Some(Self::VoiceQuickNote),
            "new_note" => Some(Self::NewNote),
            "start_recording" => Some(Self::StartRecording),
            "import_material" => Some(Self::ImportMaterial),
            _ => None,
        }
    }
}

/// 暂时执行不了的**封闭原因**（TDD-07 §2.2）。
///
/// **封闭而不是自由文本**：前端要按原因决定说哪句话、给哪个修复入口，而自由文本只能
/// 被原样渲染——那等于把本地化和交互决策一起丢给了一个字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CreateActionReasonDto {
    /// 还没有打开的笔记库（含切库过渡期）。
    VaultUnavailable,
    /// 麦克风权限尚未询问过——点一下就能申请。
    MicrophonePermissionRequired,
    /// 麦克风权限已被拒绝/撤销——只能去系统设置里改。
    MicrophonePermissionDenied,
    /// 麦克风被另一路采集占着（实时字幕 / 语音输入 / 弹窗语音，硬互斥）。
    AudioBusy,
    /// 已有一个语音输入会话在进行中。
    VoiceSessionBusy,
    /// 已有一场**录音**在进行中（B2）：入口不是禁用，而是「回到录音」。
    ///
    /// 此前它被并进 `audio_busy`——「麦克风正被另一处采集占用」。可那"另一处"就是
    /// 用户自己刚关掉面板的那场录音：麦克风在录、文件在长，而界面上既看不出来、也回不去，
    /// 只剩热键把它停掉。
    RecordingInProgress,
    /// 没有可用的转写能力（既没有本地权重，也没有服务端通道）。
    AsrUnavailable,
    /// 转写要走服务端，而用户还没同意把麦克风内容上云。
    ServiceConsentRequired,
    /// 本平台没有采集类入口依赖的 HUD 窗口（见 `CAPTURE_PLATFORM_SUPPORTED`）。
    UnsupportedPlatform,
}

/// 修复动作（TDD-07 §2.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateActionRepairDto {
    /// 打开设置的某个目标。
    OpenSettings {
        /// 目标深链。
        route: CreateActionSettingsRouteDto,
    },
    /// 申请麦克风权限（复用既有 `request_permission`）。
    RequestMicrophonePermission,
    /// 把已在进行的语音会话 HUD 显示出来（复用 `voice_input_show_hud` 的保留会话行为）。
    ShowVoiceSession,
    /// 回到进行中的录音（`recording_show_hud`：只显示常驻面板，不碰会话）。
    ShowRecordingSession,
}

/// 修复用的设置深链——**wire 值直接就是深链本身**（TDD-07 §2.2）。
///
/// 前端**原样**交给 `open_workspace_route`，不拼 URL：拼接意味着前端要知道页名与
/// 键名，而那是 Rust 注册表的事；拼错了只会在点下去的那一刻变成一次静默的 no-op。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum CreateActionSettingsRouteDto {
    /// 笔记库路径那一行。
    #[serde(rename = "settings://notes/storage.vault_root")]
    VaultRoot,
    /// 存储与隐私页。
    #[serde(rename = "settings://notes")]
    Storage,
    /// AI 算力与模型页。
    #[serde(rename = "settings://ai_services")]
    ProviderModel,
}

/// 一个动作此刻的状态。
///
/// **判别联合，不是"三态枚举 + 两个可空字段"**：后者能拼出 `available` 却带着一个
/// reason、或者 `needs_attention` 却没有 repair 这类非法组合，而每个消费者都要各自防一遍。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateActionStateDto {
    /// 现在就能用。
    Available,
    /// 暂时不能用，且**没有**可一键执行的修复（只能说明原因）。
    Unavailable {
        /// 原因。
        reason: CreateActionReasonDto,
    },
    /// 暂时不能用，但有明确的下一步。
    NeedsAttention {
        /// 原因。
        reason: CreateActionReasonDto,
        /// 修复动作。
        repair: CreateActionRepairDto,
    },
}

/// 一条动作状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateActionEntryDto {
    /// 动作 id。
    pub id: CreateActionIdDto,
    /// 此刻的状态。
    pub state: CreateActionStateDto,
}

/// 一次完整快照：**每个已支持 id 恰好一条**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CreateActionSnapshotDto {
    /// 进程内单调递增的版本号——前端只接受不旧于当前的快照。
    ///
    /// 没有它就没法处理竞态：订阅与查询是两条路，慢的那次查询回来时可能已经过时，
    /// 而"过时的可用性"会让一个刚被授权的动作重新变灰。
    pub revision: u32,
    /// 五条状态（顺序即「添加内容」的呈现顺序）。
    pub actions: Vec<CreateActionEntryDto>,
}

/// 快照变化事件（`create-actions://states-changed`，携同形快照）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "create-actions://states-changed")]
pub struct CreateActionStatesChanged(pub CreateActionSnapshotDto);

/// 进程内的快照版本号。
///
/// **不进 `AppState`**：它没有任何跨状态的不变量要维护——只是一个单调计数器，重启即
/// 归零正是它想要的语义（前端每次连上都会重新订阅并查询）。塞进 AppState 只会让构造函数
/// 多一个参数，而每个读它的地方都要先拿到 state。
static REVISION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 让快照版本号前进一格并返回新值（状态可能变了就调它）。
fn bump_revision() -> u32 {
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// 当前版本号（不推进）。
fn current_revision() -> u32 {
    REVISION.load(std::sync::atomic::Ordering::Relaxed)
}

/// 本平台有没有采集类入口依赖的 HUD 窗口（录音 HUD / 语音 HUD）：macOS 是非激活 NSPanel，
/// Windows 是 `WS_EX_NOACTIVATE` 浮层（ADR-163 ①）。与「麦克风权限查不查得了」是两件事——
/// Windows 查不了逐应用授权（`Unsupported`），但照样能采集。
const CAPTURE_PLATFORM_SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// 这台机器此刻的麦克风权限（只查，不申请）。
fn microphone_permission() -> PermissionStatus {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::permissions::PermissionManager as _;
        crate::platform::macos::MacPermissionManager.check(PermissionKind::Microphone)
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionStatus::Unsupported
    }
}

/// 一次采集类动作（语音速记 / 开始录音）共同的阻断判定。
///
/// **顺序就是 TDD-07 §2.2 声明的那一条**：平台支持 → Vault → 既有会话/音频占用 →
/// 系统权限 → 算力/同意。多因并存时只报**第一个**阻断原因——同时甩出三条理由，用户也
/// 只能一条条试，而第一条往往让后面几条自动消失。
///
/// **纯函数**（只收判定所需的事实，不收 `AppState`）：顺序是这里唯一的产品语义，
/// 而"顺序对不对"必须能不启动 Tauri 就验。此前语音速记那一支把会话占用**排在平台支持
/// 与 Vault 判定之前**，于是没打开库时它说的是"已有一个语音会话在进行中"——一条答非
/// 所问、且用户照着做也解决不了的建议。
///
/// `session_block` 是调用方在「既有会话」这一档要额外插进来的那条（语音速记的
/// `voice_session_block`）；它排在音频占用之前：会话忙比"麦克风被占"更具体，且带得出
/// 「显示那个会话」这个真正可执行的下一步。
fn capture_block(
    platform_ok: bool,
    vault_ok: bool,
    permission: PermissionStatus,
    mic_idle: bool,
    session_block: Option<CreateActionStateDto>,
) -> Option<CreateActionStateDto> {
    // ① 平台支持
    if !platform_ok {
        return Some(CreateActionStateDto::Unavailable {
            reason: CreateActionReasonDto::UnsupportedPlatform,
        });
    }
    // ② Vault
    if !vault_ok {
        return Some(vault_unavailable());
    }
    // ③ 既有会话 / 音频占用
    if session_block.is_some() {
        return session_block;
    }
    if !mic_idle {
        // 修复动作刻意留空：麦克风被谁占着，用户自己看得见（录音条 / 语音 HUD 都在），
        // 而"替他停掉另一路采集"是我们无权替他做的决定。
        return Some(CreateActionStateDto::Unavailable {
            reason: CreateActionReasonDto::AudioBusy,
        });
    }
    // ④ 系统权限
    match permission {
        PermissionStatus::NotDetermined => Some(CreateActionStateDto::NeedsAttention {
            reason: CreateActionReasonDto::MicrophonePermissionRequired,
            repair: CreateActionRepairDto::RequestMicrophonePermission,
        }),
        PermissionStatus::Denied => Some(CreateActionStateDto::NeedsAttention {
            reason: CreateActionReasonDto::MicrophonePermissionDenied,
            // 被拒之后系统不会再弹框，只能去系统设置——这里把用户送到我们能送到的最近处。
            repair: CreateActionRepairDto::OpenSettings {
                route: CreateActionSettingsRouteDto::Storage,
            },
        }),
        // ⑤ 算力/同意由调用方接在本函数之后（只有语音速记要它，录音降级为纯录音照样可用）。
        // `Unsupported` = 本平台查不了逐应用授权（Windows）：不拦，被系统隐私开关挡住时由
        // 采集本身如实报错。
        PermissionStatus::Granted | PermissionStatus::Unsupported => None,
    }
}

/// 有没有一个可写的笔记库（切库过渡期算没有）。
fn vault_ready(state: &AppState) -> bool {
    !state
        .vault_switching_flag()
        .load(std::sync::atomic::Ordering::Acquire)
        && state.vault.lock().is_ok_and(|guard| guard.is_some())
}

fn vault_unavailable() -> CreateActionStateDto {
    CreateActionStateDto::NeedsAttention {
        reason: CreateActionReasonDto::VaultUnavailable,
        repair: CreateActionRepairDto::OpenSettings {
            route: CreateActionSettingsRouteDto::VaultRoot,
        },
    }
}

/// 语音速记额外的一条：已有会话在跑时，给「显示那个会话」而不是让他再开一个。
fn voice_session_block(state: &AppState) -> Option<CreateActionStateDto> {
    use crate::voice_input::VoiceInputStatus as S;
    let status = state.voice_input.state().status;
    matches!(status, S::Recording | S::Recognizing | S::Landing).then_some(
        CreateActionStateDto::NeedsAttention {
            reason: CreateActionReasonDto::VoiceSessionBusy,
            repair: CreateActionRepairDto::ShowVoiceSession,
        },
    )
}

/// 「开始录音」额外的一条：这场录音本来就在跑，给「回到录音」而不是一个灰按钮（B2）。
///
/// 只认**录音**形态：实时字幕也占着麦克风，但字幕面板不是「录音」，回去也没有归档在
/// 长——它照旧走 `audio_busy`。纯函数，`AppState` 只负责喂快照。
fn recording_session_block_for(
    state: &crate::recording::RecordingState,
) -> Option<CreateActionStateDto> {
    use crate::recording::{RecordingStatus as S, SessionPurpose};
    let in_progress = state.purpose == SessionPurpose::Record
        && matches!(state.status, S::Recording | S::Paused | S::Stopping);
    in_progress.then_some(CreateActionStateDto::NeedsAttention {
        reason: CreateActionReasonDto::RecordingInProgress,
        repair: CreateActionRepairDto::ShowRecordingSession,
    })
}

fn recording_session_block(state: &AppState) -> Option<CreateActionStateDto> {
    recording_session_block_for(&state.recording.state())
}

/// 转写能力的阻断原因（**纯判定**，与 `install_stream_asr` 的失败模式一一对应）。
///
/// **不建立第二套 readiness 逻辑**（TDD-07 §2.2）：装配那一侧只会因为两件事失败，
/// 这里就只答这两件事——
/// - 槽位**指向服务端**：唯一的门槛是「服务端转写」开关（= 把麦克风内容交给服务端的
///   那一次同意）。没开就是 `service_consent_required`，去打开一个开关。
/// - 槽位**不指向服务端**：那就该用本机权重；没有本机权重就是"没装模型"，
///   `asr_unavailable`，去装一个。
///
/// **此前这里把两者搞反了**：判据写成"同意开关没开 → `service_consent_required`"，
/// 而那个开关在槽位指向本地模型时**根本不参与装配**。于是最常见的一种情形——槽位指向
/// 本地模型、权重还没下载——被报成「去同意上云」，用户照着做也解决不了；真实原因是
/// 本机没装模型。指向错误动作的报错比不报错更伤。
const fn asr_reason(
    to_service: bool,
    has_local_weight: bool,
    service_consented: bool,
) -> Option<CreateActionReasonDto> {
    if to_service {
        if service_consented {
            None
        } else {
            Some(CreateActionReasonDto::ServiceConsentRequired)
        }
    } else if has_local_weight {
        None
    } else {
        Some(CreateActionReasonDto::AsrUnavailable)
    }
}

/// 转写能力（读设置与模型目录，**不加载权重、不联网探活**）。
fn asr_block(state: &AppState, app: &tauri::AppHandle) -> Option<CreateActionStateDto> {
    let settings = state.settings_snapshot();
    let shape = state.desired_stream_asr_shape(&settings, &state.models_root(app));
    asr_reason(
        shape.to_service,
        shape.local.is_some(),
        crate::ipc::remote_asr_available(&settings),
    )
    .map(|reason| CreateActionStateDto::NeedsAttention {
        reason,
        // 两种原因的修复入口都是「AI 引擎 / 模型」：一个去装模型，一个去开开关，
        // 而它们就是同一块面板上的两行。
        repair: CreateActionRepairDto::OpenSettings {
            route: CreateActionSettingsRouteDto::ProviderModel,
        },
    })
}

/// 录制前检查（ADR-165 ⑧ / PRD-19-039）：跟读要麦克风与实时转写，判定与修复和「添加内容」
/// 的语音速记同一套（不另起 readiness 逻辑）——返回（麦克风，转写）两条状态。纯读。
pub(crate) fn recording_readiness(
    app: &tauri::AppHandle,
    state: &AppState,
) -> (CreateActionStateDto, CreateActionStateDto) {
    let microphone = capture_block(
        CAPTURE_PLATFORM_SUPPORTED,
        vault_ready(state),
        microphone_permission(),
        state.mic.lock().is_ok_and(|arbiter| arbiter.is_idle()),
        recording_session_block(state),
    );
    (
        microphone.unwrap_or(CreateActionStateDto::Available),
        asr_block(state, app).unwrap_or(CreateActionStateDto::Available),
    )
}

/// 算一份完整快照（纯读，见模块说明）。
fn snapshot(app: &tauri::AppHandle, state: &AppState, revision: u32) -> CreateActionSnapshotDto {
    let permission = microphone_permission();
    let mic_idle = state.mic.lock().is_ok_and(|arbiter| arbiter.is_idle());
    let vault_ok = vault_ready(state);
    let write_block = (!vault_ok).then(vault_unavailable);

    // 排列跟随「快速开始」设置里的顺序（PRD-07-068）：已选项按用户顺序在前，其余按目录顺序
    // 在后。菜单与概览消费的是同一份快照，用户不会在两处看到两种先后；设置写入会重发快照。
    let preferred = state
        .settings_snapshot()
        .get("workspace.quick_start_actions")
        .and_then(|value| value.as_string_list().map(<[String]>::to_vec))
        .unwrap_or_default();
    let actions = luna_config::ordered_create_action_ids(&preferred)
        .into_iter()
        .filter_map(CreateActionIdDto::from_str)
        .map(|id| {
            let state_dto = match id {
                // 三个写作入口只要有库就能用——它们不碰麦克风也不碰模型。
                CreateActionIdDto::TextQuickNote | CreateActionIdDto::NewNote => write_block,
                // 语音速记：采集判定（会话占用作为「既有会话」那一档插进去）+ 转写能力。
                CreateActionIdDto::VoiceQuickNote => capture_block(
                    CAPTURE_PLATFORM_SUPPORTED,
                    vault_ok,
                    permission,
                    mic_idle,
                    voice_session_block(state),
                )
                .or_else(|| asr_block(state, app)),
                // 录音**不要求转写就绪**：没有模型也照样录得下来（降级为纯录音），
                // 而把它判成不可用会让用户丢掉一段本来录得到的会议。
                CreateActionIdDto::StartRecording => capture_block(
                    CAPTURE_PLATFORM_SUPPORTED,
                    vault_ok,
                    permission,
                    mic_idle,
                    recording_session_block(state),
                ),
                // 导入资料只需要一个能写的库：选文件、落盘、排队转写，麦克风与它无关。
                CreateActionIdDto::ImportMaterial => write_block,
            };
            CreateActionEntryDto {
                id,
                state: state_dto.unwrap_or(CreateActionStateDto::Available),
            }
        })
        .collect();

    CreateActionSnapshotDto { revision, actions }
}

/// 五类创建动作此刻的可用性（唯一只读查询，TDD-07 §2.2）。
#[tauri::command]
#[specta::specta]
pub(crate) fn create_action_states(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<CreateActionSnapshotDto, String> {
    Ok(snapshot(&app, &state, current_revision()))
}

/// 状态可能变了：重算并广播。
///
/// **由 Rust 失效并发布，而不是让前端定时轮询**：轮询要么太慢（授权完还灰着）、要么
/// 太费（每秒问一遍权限）。发布的是**同形快照**，前端不必再查一次。
///
/// 调用面按 TDD-07 §2.2 的四类状态源分工，一类都不能少：
/// - **Vault**：切库 / 打开 / 关闭（`ipc/vaults.rs`）；
/// - **设置**：写入与恢复默认（`ipc/settings.rs`）——它同时覆盖「算力/同意」那一档，
///   因为模型槽位与「服务端转写」开关都是设置项；
/// - **音频**：录音与实时字幕会话的起止（`ipc/recording.rs` / `ipc/caption.rs`）；
/// - **既有语音会话**：语音输入会话的起止与落地（`ipc/voice.rs`）。
///
/// **后两类此前一条都没接**：于是用户开始录音之后，已经打开的「添加内容」与常驻的
/// 「快速开始」会一直停在录音前那份快照上；录完之后又一直停在 `audio_busy` 上——两次都是
/// 界面在说一件此刻不成立的事。
///
/// 残留缺口（如实记下）：语音会话在**没有用户动作**的情况下自行收尾时（ASR 失败、
/// 采集句柄异常释放）不经过任何 IPC 边界，故那一刻不发快照。控制器一侧没有 `AppHandle`，
/// 把它穿进去是另一个批次的事。此刻的兜底是 §2.2 本来就要求的两次重查：Popover 打开时、
/// 窗口重新激活时——用户下一次去点那个入口之前必然经过其中之一。
pub(crate) fn publish_create_action_states(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let snapshot = snapshot(app, &state, bump_revision());
    let _ = CreateActionStatesChanged(snapshot).emit(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_and_the_config_catalog_are_an_exhaustive_round_trip() {
        // 两份 id 清单是必然要漂的东西：设置校验用 `luna-config` 那份，IPC 用这份。
        // 少一个，那个动作就永远不出现在「添加内容」里，而没有任何东西会变红。
        for id in luna_config::CREATE_ACTION_IDS {
            let dto =
                CreateActionIdDto::from_str(id).unwrap_or_else(|| panic!("IPC 侧缺少 id：{id}"));
            // 比对的是**serde 的输出**（真正的 wire 值），不是另写一张 id 表。
            let wire = serde_json::to_string(&dto).unwrap();
            assert_eq!(wire.trim_matches('"'), *id, "round-trip 不闭合：{id}");
        }
        assert_eq!(CreateActionIdDto::from_str("nope"), None);
    }

    #[test]
    fn the_settings_routes_serialize_to_the_deep_links_themselves() {
        // wire 值就是深链本体：前端原样交给 `open_workspace_route`，不拼 URL。
        let route = |r: CreateActionSettingsRouteDto| serde_json::to_string(&r).unwrap();
        assert_eq!(
            route(CreateActionSettingsRouteDto::VaultRoot),
            r#""settings://notes/storage.vault_root""#
        );
        assert_eq!(
            route(CreateActionSettingsRouteDto::Storage),
            r#""settings://notes""#
        );
        assert_eq!(
            route(CreateActionSettingsRouteDto::ProviderModel),
            r#""settings://ai_services""#
        );
    }

    #[test]
    fn every_route_is_reachable_through_the_cross_window_whitelist() {
        // 修复动作最终经 `open_workspace_route` 派发，而那条命令有白名单——不在名单里
        // 的深链会被拒绝，表现为"点了修复什么都没发生"。
        for route in [
            CreateActionSettingsRouteDto::VaultRoot,
            CreateActionSettingsRouteDto::Storage,
            CreateActionSettingsRouteDto::ProviderModel,
        ] {
            let wire = serde_json::to_string(&route).unwrap();
            let url = wire.trim_matches('"');
            assert!(
                crate::ipc::workspace::workspace_route_allowed(url),
                "{url} 不在跨窗口白名单里"
            );
        }
    }

    #[test]
    fn a_state_cannot_be_available_and_carry_a_reason_at_the_same_time() {
        // 判别联合的全部价值：非法组合根本表达不出来。这条断言锁的是 wire 形态——
        // 换成"三态 + 两个可空字段"之后它会立刻变红。
        let available = serde_json::to_string(&CreateActionStateDto::Available).unwrap();
        assert_eq!(available, r#"{"kind":"available"}"#);
        let needs = serde_json::to_string(&CreateActionStateDto::NeedsAttention {
            reason: CreateActionReasonDto::VaultUnavailable,
            repair: CreateActionRepairDto::OpenSettings {
                route: CreateActionSettingsRouteDto::VaultRoot,
            },
        })
        .unwrap();
        assert!(needs.contains(r#""kind":"needs_attention""#), "{needs}");
        assert!(needs.contains(r#""reason":"vault_unavailable""#), "{needs}");
        assert!(
            needs.contains(r#""kind":"open_settings""#),
            "修复动作也必须是判别联合：{needs}"
        );
    }

    /// 会话忙时的那一条（语音速记专有）。
    fn session_busy() -> CreateActionStateDto {
        CreateActionStateDto::NeedsAttention {
            reason: CreateActionReasonDto::VoiceSessionBusy,
            repair: CreateActionRepairDto::ShowVoiceSession,
        }
    }

    fn reason_of(state: Option<CreateActionStateDto>) -> Option<CreateActionReasonDto> {
        match state {
            Some(CreateActionStateDto::Unavailable { reason })
            | Some(CreateActionStateDto::NeedsAttention { reason, .. }) => Some(reason),
            _ => None,
        }
    }

    #[test]
    fn multiple_blockers_report_the_first_one_in_the_declared_order() {
        use CreateActionReasonDto as R;
        // 顺序就是 TDD-07 §2.2 那一条：平台支持 → Vault → 既有会话/音频占用 → 系统权限。
        // 每一格都让**它之后**的所有因素同时成立，于是"报的是哪一个"就是顺序本身。
        let all_bad = |platform_ok, permission| {
            capture_block(platform_ok, false, permission, false, Some(session_busy()))
        };
        assert_eq!(
            reason_of(all_bad(false, PermissionStatus::Denied)),
            Some(R::UnsupportedPlatform),
            "平台不支持排第一"
        );
        assert_eq!(
            reason_of(all_bad(true, PermissionStatus::Denied)),
            Some(R::VaultUnavailable),
            "没有库时不该先说会话忙——这条建议用户照着做也解决不了"
        );
        assert_eq!(
            reason_of(capture_block(
                true,
                true,
                PermissionStatus::Denied,
                false,
                Some(session_busy())
            )),
            Some(R::VoiceSessionBusy),
            "已有会话在跑时，「去授权」是答非所问"
        );
        assert_eq!(
            reason_of(capture_block(
                true,
                true,
                PermissionStatus::Denied,
                false,
                None
            )),
            Some(R::AudioBusy),
            "没有语音会话但麦克风被占（录音/字幕）"
        );
        assert_eq!(
            reason_of(capture_block(
                true,
                true,
                PermissionStatus::Denied,
                true,
                None
            )),
            Some(R::MicrophonePermissionDenied),
        );
        assert_eq!(
            capture_block(true, true, PermissionStatus::Granted, true, None),
            None,
            "四关都过就是可用"
        );
    }

    /// 权限查不了（Windows 无逐应用麦克风授权可查）≠ 平台不支持：平台有采集 HUD 就可用，
    /// 此前两者混为一谈，Windows 上「语音速记 / 开始录音」恒为「本平台暂不支持」。
    #[test]
    fn an_unqueryable_permission_is_not_an_unsupported_platform() {
        assert_eq!(
            capture_block(true, true, PermissionStatus::Unsupported, true, None),
            None
        );
    }

    /// B2：自己的录音在跑 → 「回到录音」；字幕占着麦克风 → 仍是 `audio_busy`（那不是录音）。
    #[test]
    fn an_own_recording_session_offers_a_way_back_instead_of_a_grey_button() {
        use crate::recording::{RecordingState, RecordingStatus as S, SessionPurpose};
        let snapshot = |status, purpose| RecordingState {
            status,
            purpose,
            session_id: Some("s1".into()),
            note_rel: None,
            elapsed_ms: 0.0,
            message_key: None,
            message_detail: None,
        };
        for status in [S::Recording, S::Paused, S::Stopping] {
            assert_eq!(
                recording_session_block_for(&snapshot(status, SessionPurpose::Record)),
                Some(CreateActionStateDto::NeedsAttention {
                    reason: CreateActionReasonDto::RecordingInProgress,
                    repair: CreateActionRepairDto::ShowRecordingSession,
                }),
                "{status:?} 应给出「回到录音」"
            );
        }
        assert_eq!(
            recording_session_block_for(&snapshot(S::Idle, SessionPurpose::Record)),
            None
        );
        assert_eq!(
            recording_session_block_for(&snapshot(S::Recording, SessionPurpose::Caption)),
            None,
            "字幕会话不是录音，回不去也没有归档在长"
        );
        // 接进 capture_block 后，它排在「麦克风被占」之前——那条会把自己的录音说成"另一处"。
        assert_eq!(
            reason_of(capture_block(
                true,
                true,
                PermissionStatus::Granted,
                false,
                recording_session_block_for(&snapshot(S::Recording, SessionPurpose::Record)),
            )),
            Some(CreateActionReasonDto::RecordingInProgress)
        );
    }

    #[test]
    fn a_missing_local_weight_is_never_reported_as_a_consent_problem() {
        use CreateActionReasonDto as R;
        // 最常见的一种情形：槽位指向本地模型、权重还没装。此前它被报成「去同意上云」，
        // 而那个开关在槽位指向本地时根本不参与装配——用户照着做也解决不了。
        assert_eq!(
            asr_reason(false, false, false),
            Some(R::AsrUnavailable),
            "本地槽位 + 没装权重 = 没装模型"
        );
        assert_eq!(
            asr_reason(false, false, true),
            Some(R::AsrUnavailable),
            "同意开关开着也一样：它与本地槽位无关"
        );
        assert_eq!(asr_reason(false, true, false), None, "装了权重就能转");
        // 服务端槽位：唯一门槛是那次同意。
        assert_eq!(
            asr_reason(true, false, false),
            Some(R::ServiceConsentRequired)
        );
        assert_eq!(
            asr_reason(true, true, false),
            Some(R::ServiceConsentRequired),
            "指向服务端时，本地兜底腿也救不回来——装配那一侧就是这么判的"
        );
        assert_eq!(asr_reason(true, false, true), None);
    }

    #[test]
    fn the_revision_only_ever_moves_forward() {
        let before = current_revision();
        let a = bump_revision();
        let b = bump_revision();
        assert!(a > before && b > a, "版本号必须单调递增：{before} {a} {b}");
    }
}
