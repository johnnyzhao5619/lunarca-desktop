//! 录音编排（Phase 4 C4 / PRD-04 / TDD-05 §3/§5.3/§6.1）。
//!
//! 状态机：Idle → Recording ⇄ Paused → Stopping → Idle（归档完成）。
//! 麦克风硬互斥：经 [`MicArbiter`]（[`MicHolder::Recording`]），拒绝语音输入/弹窗语音。
//! 隐私铁律：日志与系统通知不携带转写正文；[`RecordingEvent`] 是发往用户自己 HUD 的
//! 类型化显示通道，实时文本仅在该通道内传递，终稿经 `luna_vault::finalize_recording`
//! 落盘（ADR-011 / AGENTS §隐私铁律）。

use luna_audio::{
    start_capture, CaptureConfig, CaptureDiagnostic, CaptureEvent, CaptureMode, CaptureSource,
    DegradationState, PartInterval, RecordingFormat, SaveSpec, SpeechEvent, VadDecision,
    TARGET_SAMPLE_RATE,
};
use luna_core::{
    AudioRef, CancelToken, Job, JobEvent, JobKind, JobOrigin, Lang, PartialPayload, Priority,
    ResultPayload, Segment, SegmentId, StreamId, WindowInput,
};
use luna_jobqueue::{
    JobHandle, JobQueue, JobSink, RealtimeSession, RealtimeTranslationPipeline, TranslationConfig,
};
use luna_vault::RecordingPart;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use crate::arbiter::{MicArbiter, MicBusy, MicHolder};
use crate::notifications::{NotificationEvent, NotificationTarget, PrivatePayload};
use crate::voice_input::{apply_glossary_to_segments, glossary_initial_prompt, GlossaryEntry};
use crate::voice_input::{voice_level_payload, RawBytes};

// ─── 公开状态 / 事件 ──────────────────────────────────────────────────────────

const TRACK_SELF: &str = "self";
const TRACK_REMOTE: &str = "remote";

/// 卸载负载跳窗的上报节流：跳窗在过载时是**每窗都发生**的，逐条打会把 stdout 淹掉，
/// 而运维要的只是「此刻在跳、跳了多少、哪个档位」。
///
/// 取 5s 的推导：跳窗的产生速率与快照窗同阶（连续语音下约每秒数窗），故节流窗必须
/// 覆盖多个窗才算"聚合"；而它又必须短于用户察觉「字幕停更」的时间（数秒），否则日志
/// 里的第一条痕迹晚于用户的抱怨。同时与 `luna_provider` 的上行统计、`ConsumerPipeline`
/// 的电平分布同为 5s——三条节流诊断在 stdout 上按同一节奏出现，肉眼可对齐时间。
const SKIP_REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// 「声明 ≠ 交付」的分界（ppm）。判据与理由的唯一事实源在 `luna_audio::clock`——本处只
/// 取用，不复制（DRY；判定本身已随 `DriftReport::rate_mismatch` 一并下放到测量所在处）。
use luna_audio::clock::RATE_MISMATCH_PPM;

/// 实时音频会话的**形态**（一等公民，非兼容层）：同一条采集→ASR→转译引擎，两种目的。
///
/// - [`SessionPurpose::Record`]：录音归档——落盘音频 + `finalize_recording` 写入 Vault 笔记。
/// - [`SessionPurpose::Caption`]：实时字幕——**不落盘、不归档**，纯转写/转译供实时字幕面板显示
///   （跨语言面对面交流 / 实时翻译辅助，PRD-04-040）。收尾按形态分治（是否归档、隐藏哪个窗口）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionPurpose {
    /// 录音归档（默认）。
    Record,
    /// 实时字幕（无持久化）。
    Caption,
    /// 提词器跟读（ADR-162 ⑤ C4b）：只听麦克风、无持久化、不转译；转写只喂给对齐器。
    Teleprompter,
}

/// 归档回执在 HUD 上停留多久再收起（ADR-137 H5）：够读完「已保存 · 打开」并点一下。
const ARCHIVED_RECEIPT_LINGER: std::time::Duration = std::time::Duration::from_secs(3);

/// 录音控制器状态枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// 空闲，无活动会话。
    Idle,
    /// 录音进行中（含转写流水线）。
    Recording,
    /// 已暂停（麦克风已释放，等待继续或停止）。
    Paused,
    /// 停止中（最后一批转写任务收尾中）。
    Stopping,
    /// 麦克风被其他功能占用。
    MicBusy,
    /// 发生错误。
    Error,
}

/// 发给前端的录音会话状态快照（脱敏）。
///
/// `Deserialize` 只为它能作为跨窗广播事件 [`crate::ipc::RecordingStateChanged`] 的载荷
/// （tauri-specta `Event` 的类型约束）；Rust 侧从不反序列化它。
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct RecordingState {
    /// 状态枚举。
    pub status: RecordingStatus,
    /// 会话形态（录音归档 / 实时字幕）：前端据此渲染对应外壳与收尾行为。
    pub purpose: SessionPurpose,
    /// 客户端会话 ID（前端生成，round-trip 对账）。
    pub session_id: Option<String>,
    /// 归档笔记相对路径（已知时）。
    pub note_rel: Option<String>,
    /// 本次会话已录音时长（仅计实采样本，不含暂停间隙，毫秒）。
    /// TS `number`（f64）可无损表示 2^53 ms ≈ 28.5 万年。
    pub elapsed_ms: f64,
    /// i18n key（前端 t(key) 渲染）。
    pub message_key: Option<String>,
    /// 补充细节（路径、原始错误串等**与语言无关**的具体值）。
    ///
    /// 它此前叫 `guidance_zh` 并直接装中文句子，而前端把它原样渲染——英文界面的用户
    /// 会读到中文。可本地化的那一半一律走 `message_key`（前端 `t(key)`），这里只留
    /// "翻译不了也不该翻译"的那一半。
    pub message_detail: Option<String>,
}

impl RecordingState {
    /// 会话仍占用控制权（活动态或已接受停止、尚未收尾）。与前端 `isRecordingSessionOpen` 同义。
    pub(crate) fn session_open(&self) -> bool {
        matches!(
            self.status,
            RecordingStatus::Recording | RecordingStatus::Paused | RecordingStatus::Stopping
        )
    }

    fn idle() -> Self {
        Self {
            status: RecordingStatus::Idle,
            purpose: SessionPurpose::Record,
            session_id: None,
            note_rel: None,
            elapsed_ms: 0.0,
            message_key: Some("recording:status.idle".into()),
            message_detail: None,
        }
    }
}

/// 录音事件。
///
/// **脱敏边界（AGENTS §隐私铁律）**：铁律约束的是**日志与系统通知**绝不含转写正文——
/// 本通道是发往用户自己 HUD 的类型化事件流，`Partial`/`Hypothesis` 携带转写文本正是
/// PRD-04-010 [P1]"实时转录流式滚动展示"的产品功能（同 voice-input `FieldInput{text}` 先例）。
/// utterance 级低频文本不属 ADR-011 的高频数字流（那类才强制二进制 Channel）。
/// 终稿仍经 `luna_vault::finalize_recording` 落盘为事实源。
#[derive(Clone, Debug, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum RecordingEvent {
    /// 状态快照（脱敏）。
    State(RecordingState),
    /// 已定稿转写段（committed，永不回退；HUD 实时滚动显示，PRD-04-010）。
    Partial {
        /// 会话内绝对起点（ms）。TS `number`（f64）。
        start_ms: f64,
        /// 会话内绝对止点（ms）。TS `number`（f64）。
        end_ms: f64,
        /// 段文本（已定稿）。
        text: String,
        /// 轨级说话人标签（PRD-04-053 L0）："self"=我（麦克风轨）/ "remote"=对方（系统轨）；
        /// `None` = 单麦克风源（无需轨级标签）。
        track: Option<String>,
    },
    /// 轨级暂定转写尾部（hypothesis，可被改写；HUD 灰显，空串 = 清除该轨）。
    Hypothesis {
        /// 暂定文本。
        text: String,
        /// 暂定文本所属轨道：`self` / `remote`；单麦克风源为 `None`。
        /// Both 必须按轨独立投影，禁止以一个会话级字符串互相覆盖。
        track: Option<String>,
    },
    /// 已译完的句界单元（实时转译，TDD-08；HUD 在对应 committed 行下双语显示）。
    /// 与 `Partial` 同属发往用户自己 HUD 的显示载体，绝不入日志/通知。
    Translation {
        /// 单元起点（会话绝对 ms；与首个 committed 段 `start_ms` 对齐，前端据此配对）。
        start_ms: f64,
        /// 单元止点（会话绝对 ms）。
        end_ms: f64,
        /// 译文。
        text: String,
        /// 译文所属轨道：`self` / `remote`；单麦克风源为 `None`。
        track: Option<String>,
    },
    /// 降级档位变化（仅档位名与原因码，无内容）。
    Degraded {
        /// "green" / "yellow" / "orange" / "red"
        tier: String,
        /// 原因码（log-safe）："high_rtf" = RTF 过高；"window_drop" = ASR 窗通道满丢窗。
        reason: String,
    },
    /// 归档完成，前端可刷新笔记列表。
    Archived {
        /// 归档笔记路径（Vault 相对）。
        note_rel: String,
    },
    /// 失败错误码。
    Failed {
        /// 错误码（无正文）。
        code: String,
    },
    /// 这次转写由谁执行、音频会不会离开设备（ADR-038 ④ 的「联网指示」）。
    ///
    /// **必须与录音指示同时在场**——麦克风内容是常开的那条路径，用户有权在开录的那一刻
    /// 就知道它去了哪儿，而不是等第一段文字回来才知道。故本事件发两次：
    /// ① 会话建立时按 Provider 的 `location` 立刻发（`display_name` 尚未知），
    /// ② 服务端回报执行者后再发一次（补上名字）。
    /// 只等 ② 的话，握手到首帧之间会有一段"在录、但没有任何联网提示"的时间。
    Executor {
        /// 执行者展示名（如「本机 whisper.cpp」）；`None` = 会话刚建立，尚未回报。
        display_name: Option<String>,
        /// 音频会离开这台设备吗。UI 据此点亮联网指示。
        leaves_device: bool,
    },
    /// **实时转写停了，录音继续**（不是致命错误，不改录音状态）。
    ///
    /// 此前这条根本不存在：主循环只看 `Partial`，`JobEvent::Failed` 被直接丢掉。
    /// 本机 whisper 极少中途失败，所以一直没露馅；而云通道的额度耗尽 / 未登录 /
    /// 断网 / 用户切到「仅本地」**都是常态**。后果有两层：
    /// ① 用户看到「字幕突然不出了」，没有任何提示——而他刚刚可能已经被扣了额度；
    /// ② Job 终态后事件通道关闭，`recv()` **立即返回 `None`** 而不是挂起，于是那条
    ///    `select!` 分支空转烧满一个核，直到用户停录（录音本就是吃资源的场景）。
    ///
    /// 与 [`RecordingEvent::Failed`] 分开：那条会把整个录音置为 error，而这里录音
    /// 完全正常——音频照常落盘，停录后的二次转写照常补齐权威结果（PRD-04-030）。
    /// 与 [`RecordingEvent::Degraded`] 也分开：那条是负载降级、会自愈，而额度耗尽
    /// 不会自愈，共用一条会让 HUD 说出「负载回落将自动恢复」这种不成立的话。
    TranscribeStopped {
        /// 稳定错误码（`QUOTA_EXHAUSTED` / `UNAUTHORIZED` / `STRICT_LOCAL_BLOCKED` /
        /// `PROVIDER_UNAVAILABLE` …；无正文，log-safe）。UI 据此给可执行提示。
        code: String,
        /// 出问题的轨道：`self` / `remote`；单麦克风源为 `None`。
        track: Option<String>,
    },
    /// **云腿断了，正在静默重连窗内重试**（ADR-038 ⑧）。`active: false` = 接上了。
    ///
    /// 与 [`RecordingEvent::TranscribeStopped`] 分开，因为用户能做的事完全不同：那条说
    /// 「这条轨的转写到此为止」（去加额度 / 去登录 / 改隐私档），这条说「等几秒」——
    /// 而绝大多数瞬断会在几百毫秒内自己好。共用一条的话，一次 Wi-Fi 漫游就会让 HUD
    /// 弹出「转写已停止」，用户去点了一圈设置，而字幕早就回来了。
    ///
    /// 与 [`RecordingEvent::Degraded`] 也分开：那条是本机负载降级、会自愈到「绿」，
    /// 文案里带着「负载回落将自动恢复」——用在断网上就是一句不成立的话。
    TranscribeReconnecting {
        /// 窗开着（`true`）还是已经接上（`false`）。
        active: bool,
        /// 出问题的轨道：`self` / `remote`；单麦克风源为 `None`。
        track: Option<String>,
    },
    /// 转写源语言自动检测结果（PRD-04-046，Chrome Live Caption 对标）：auto 模式下引擎
    /// 首个有实质文本的窗口锁定语言后回报（BCP-47 码，无正文，log-safe）；`None` = 尚未
    /// 检测到 / 用户切回 auto 待重新检测 / 用户显式指定（无需回显）。面板据此在
    /// 「自动检测」选项上标注检测到的语言（如「自动检测（中文）」）。
    TranscribeLangDetected {
        /// 检测锁定的语言码（如 "zh"）。
        lang: Option<String>,
    },
    /// **实时转译停了，转写与录音继续**（ADR-083 ⑧）。
    ///
    /// 撞上「重试与换腿都改变不了」的结构性错误（模型没装 / 未登录 / 额度用尽 /
    /// 严格本地挡住 …，判据见 `luna_core::JobError::is_structural`）时发**一次**，
    /// 此后本会话不再提交任何句子、`translation` 一律留 `None`。
    ///
    /// 此前的处置是每句一行 `eprintln!` + 留 `None`：一场会议里用户看到的是「译文一直
    /// 不出来」而界面零信号，stdout 里刷着一百条一模一样的失败——那既不是可见信号，
    /// 也不是可归因的日志，而且那一百次请求全都注定失败。
    ///
    /// 与 [`RecordingEvent::TranscribeStopped`] 同形但分开：那条说的是**转写**那条腿停了
    /// （字幕不出字），这条说的是**译文**那条腿停了（原文照常上屏）——两句话对用户的含义
    /// 与下一步动作都不同。完整双语仍由停录后的二次转写补齐（PRD-04-030）。
    ///
    /// **用户自己按的停止不会走到这里**：`Cancelled` 不是结构性错误（见
    /// `JobError::is_structural`），否则每一次正常停录都会弹一条「翻译已停止」。
    TranslateStopped {
        /// 稳定错误码（`MODEL_MISSING` / `UNAUTHORIZED` / `QUOTA_EXHAUSTED` /
        /// `STRICT_LOCAL_BLOCKED` …；无正文，log-safe）。UI 据此给可执行提示。
        code: String,
    },
}

/// 把任一 ASR 轨的同一份 Partial 投影为 HUD 事件与会话绝对时间段。
///
/// 主轨与 Both 系统副轨必须共用这一个出口：`committed_delta` 和 `hypothesis` 要么一并
/// 投影，要么一并失败，禁止再次出现某条分支只转发 committed、静默丢掉低延迟暂定尾。
fn project_asr_partial(
    committed_delta: Vec<Segment>,
    hypothesis: String,
    base_elapsed_ms: u64,
    track: Option<&str>,
) -> (Vec<RecordingEvent>, Vec<Segment>) {
    let track = track.map(str::to_string);
    let mut events = Vec::with_capacity(committed_delta.len() + 1);
    let mut absolute_segments = Vec::with_capacity(committed_delta.len());

    for mut segment in committed_delta {
        events.push(RecordingEvent::Partial {
            start_ms: (base_elapsed_ms + segment.start.as_millis() as u64) as f64,
            end_ms: (base_elapsed_ms + segment.end.as_millis() as u64) as f64,
            text: segment.text.clone(),
            track: track.clone(),
        });
        segment.start += Duration::from_millis(base_elapsed_ms);
        segment.end += Duration::from_millis(base_elapsed_ms);
        segment.speaker.clone_from(&track);
        absolute_segments.push(segment);
    }
    events.push(RecordingEvent::Hypothesis {
        text: hypothesis,
        track,
    });

    (events, absolute_segments)
}

// ─── 内部控制 ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum RecordingControl {
    Stop,
    Pause,
    Resume,
    Bookmark {
        /// 会话绝对时刻（ms）。
        abs_ms: u64,
        /// 可选短备注（PRD-04-005）。
        note: Option<String>,
    },
    /// 给已打的书签补 / 改备注（按 `abs_ms` 对上；对不上就当一条新书签）。
    BookmarkNote {
        abs_ms: u64,
        note: String,
    },
    /// 麦克风静音开关（PRD-04-003）：只作用于麦克风轨；系统音频必须持续采集。
    SetMicrophoneMuted {
        muted: bool,
    },
    /// 实时转写开关（PRD-04-012）：关 = 停喂 ASR 窗（录音/落盘不受影响）。
    SetTranscribe {
        enabled: bool,
    },
    /// 实时转译开关（PRD-04-011/012，TDD-08）：关 = 停喂翻译管线（转写不受影响）。
    SetTranslate {
        enabled: bool,
    },
    /// 实时转译**目标外语现场切换**（录音/字幕面板下拉，PRD-04-041）：热更新翻译配置的
    /// 默认外语（`TranslationConfig.ctx`），下一句界单元即按新目标解析（智能双向）。
    SetTranslateTarget {
        lang: String,
    },
    /// **转写源语言现场切换**（录音/字幕面板下拉，PRD-04-046，Chrome Live Caption 对标）：
    /// `Some` = 显式指定（whisper 恒用该语言）；`None` = 回自动检测（清锁，下一个有实质
    /// 文本的窗口重新检测锁定）。热更新共享语言锁（当前窗后即生效）+ 更新 `lang_hint`
    /// 保证暂停/恢复的新 part 一致。
    SetTranscribeLang {
        lang: Option<String>,
    },
}

// ─── 内部会话状态 ─────────────────────────────────────────────────────────────

/// 活动录音会话（存在于 Mutex 内，有活动会话时非 None）。
struct ActiveRecording {
    session_id: String,
    control_tx: mpsc::UnboundedSender<RecordingControl>,
    mic_holder: MicHolder,
    /// 本会话的状态事件通道。停止命令在控制事件入队后立即广播 `Stopping`，让所有表面
    /// 进入确定的收尾态；最终 `Idle`/`Error` 仍由会话任务拥有。
    on_event: Option<Channel<RecordingEvent>>,
}

pub(crate) struct RecordingControllerInner {
    state: RecordingState,
    active: Option<ActiveRecording>,
    /// 早到的控制事件（session 还未起来时暂存）。
    pending_control: Option<(String, RecordingControl)>,
}

/// 录音控制器：状态机 + 麦克风互斥 + 会话编排。
///
/// 生命周期随 App 同生，`Arc<RecordingController>` 注入 `AppState`。
pub struct RecordingController {
    pub(crate) inner: Mutex<RecordingControllerInner>,
    /// 波形电平二进制通道（**按窗口 label 键控的活订阅表**，ADR-011）。
    ///
    /// 多个录音表面**常驻并存**（浮层 recording-hud 窗、live-caption 字幕窗、主窗录音页），
    /// 各自挂载时 `subscribe_levels` 登记自己的 `Channel`——**每窗一槽**（同窗重挂载替换，
    /// 不泄漏），会话每帧 Level 事件**广播到全部活订阅**。历史缺陷链（真机两轮反馈）：
    /// ① 会话启动时快照一次 → 晚挂载的 HUD 收不到；② 单槽「后订阅者胜」→ 启动序
    /// recording-hud 先建、live-caption 后建，字幕窗把 HUD 的订阅覆盖 → HUD 波形恒平、
    /// 计时却在走（计时走 always-live 的 `on_event`）。专用锁避免与控制流争 `inner`。
    level_channels: Mutex<HashMap<String, Channel<RawBytes>>>,
    /// 状态**转移**观察者（B2 / H2）：`status` 或 `session_id` 一变就被叫一次，载荷是新快照。
    ///
    /// 会话的 `on_event` Channel 只通向**发起会话的那一扇窗**：录音从 HUD 开始，主窗
    /// 状态栏永远不知道正在录（此前主窗听的 `luna:recording-started` 自定义事件根本没有
    /// 任何地方派发过——它在 HUD 那个 webview 里派发也到不了主窗）；HUD 隐藏/锁屏期间
    /// 终态那一条 Channel 消息丢了，面板就永远停在「正在停止…」。控制器是唯一事实源，
    /// 于是转移在这里广播（App 装配时挂的是 Tauri 全窗事件），任何表面都能以它为准。
    /// 只在转移时叫，不在每帧计时更新时叫：计时走会话 Channel 的节流 `State`。
    state_observer: Mutex<Option<StateObserver>>,
}

/// 状态转移观察者（见 [`RecordingController::state_observer`]）。
type StateObserver = Box<dyn Fn(&RecordingState) + Send + Sync>;

/// 两份快照之间是否发生了**转移**（值得广播），而非仅计时推进。
fn is_state_transition(previous: &RecordingState, next: &RecordingState) -> bool {
    previous.status != next.status || previous.session_id != next.session_id
}

impl Default for RecordingController {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RecordingControllerInner {
                state: RecordingState::idle(),
                active: None,
                pending_control: None,
            }),
            level_channels: Mutex::new(HashMap::new()),
            state_observer: Mutex::new(None),
        }
    }
}

// ─── 公开 API ─────────────────────────────────────────────────────────────────

impl RecordingController {
    /// 挂状态转移观察者（App 装配时挂一次；见字段文档）。
    pub fn set_state_observer(&self, observer: StateObserver) {
        if let Ok(mut slot) = self.state_observer.lock() {
            *slot = Some(observer);
        }
    }

    /// 通知观察者（**锁外**调用：观察者会发 IPC，不能在持 `inner` 时叫它）。
    fn notify_transition(&self, state: &RecordingState) {
        if let Ok(slot) = self.state_observer.lock() {
            if let Some(observer) = slot.as_ref() {
                observer(state);
            }
        }
    }

    /// 返回当前状态快照（脱敏）。
    ///
    /// **异常结束 → 复位**（H2）：`active` 已经没了、状态却还停在会话态（`Recording` /
    /// `Paused` / `Stopping`），说明会话任务没走到终态写入就没了（任务被 abort / panic）。
    /// 这份快照是所有表面「以谁为准」的那一份，它不能把一个不存在的会话报成还在收尾——
    /// 那正是「HUD 永远停在正在停止…、开始录音持续禁用、只能重启应用」的形状。
    #[must_use]
    pub fn state(&self) -> RecordingState {
        self.inner
            .lock()
            .map(|inner| {
                if inner.active.is_none() && inner.state.session_open() {
                    RecordingState::idle()
                } else {
                    inner.state.clone()
                }
            })
            .unwrap_or_else(|_| RecordingState {
                status: RecordingStatus::Error,
                purpose: SessionPurpose::Record,
                session_id: None,
                note_rel: None,
                elapsed_ms: 0.0,
                message_key: Some("recording:error.state_lock".into()),
                message_detail: None,
            })
    }

    /// 订阅波形电平二进制通道（ADR-011）。任一录音表面挂载即调用，按**窗口 label** 登记
    /// 一槽（同窗重挂载替换旧通道，不泄漏）；会话每帧广播到全部活订阅——多常驻窗口
    /// （recording-hud / live-caption / 主窗）互不覆盖（根治「后订阅者胜致 HUD 波形恒平」）。
    pub fn subscribe_levels(
        &self,
        window_label: String,
        channel: Channel<RawBytes>,
    ) -> Result<(), String> {
        let mut guard = self
            .level_channels
            .lock()
            .map_err(|_| "波形通道锁中毒".to_string())?;
        guard.insert(window_label, channel);
        Ok(())
    }

    /// 发送停止控制到活动会话，并立即进入 `Stopping`。
    ///
    /// `Stopping` 由控制器在接受停止请求的同一临界区写入，避免麦克风数字静默等场景下
    /// 用户点击后界面仍长期显示「聆听中」。会话任务完成资源释放后再落到 `Idle`/`Error`。
    pub fn stop(&self, session_id: Option<String>) -> Result<RecordingState, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "录音状态锁中毒".to_string())?;
        let Some(active) = &inner.active else {
            if recording_should_queue_pending(false, session_id.as_deref()) {
                if let Some(id) = session_id {
                    inner.pending_control = Some((id, RecordingControl::Stop));
                }
                eprintln!("[F4] 停止事件早于活动会话到达，已按 session 暂存。");
            }
            return Ok(inner.state.clone());
        };
        if !recording_control_targets_active(active.session_id.as_str(), session_id.as_deref()) {
            return Err("录音会话已变化，请重试停止操作。".to_string());
        }

        let active_session_id = active.session_id.clone();
        let control_tx = active.control_tx.clone();
        let on_event = active.on_event.clone();
        control_tx
            .send(RecordingControl::Stop)
            .map_err(|_| "录音会话已结束".to_string())?;

        let state = recording_state(
            RecordingStatus::Stopping,
            inner.state.purpose,
            &active_session_id,
            inner.state.note_rel.as_deref().unwrap_or_default(),
            inner.state.elapsed_ms.max(0.0) as u64,
            "recording:status.stopping",
        );
        inner.state = state.clone();
        drop(inner);
        self.notify_transition(&state);
        if let Some(channel) = on_event {
            let _ = channel.send(RecordingEvent::State(state.clone()));
        }
        Ok(state)
    }

    /// 发送暂停控制到活动会话，返回当前状态。
    pub fn pause(&self, session_id: Option<String>) -> Result<RecordingState, String> {
        self.send_control(RecordingControl::Pause, session_id)?;
        Ok(self.state())
    }

    /// 发送继续控制到活动会话，返回当前状态。
    pub fn resume(&self, session_id: Option<String>) -> Result<RecordingState, String> {
        self.send_control(RecordingControl::Resume, session_id)?;
        Ok(self.state())
    }

    /// 在当前位置插入书签（abs_ms 由调用方根据 state.elapsed_ms 传入），可带短备注。
    pub fn bookmark(
        &self,
        session_id: Option<String>,
        abs_ms: u64,
        note: Option<String>,
    ) -> Result<(), String> {
        self.send_control(RecordingControl::Bookmark { abs_ms, note }, session_id)
    }

    /// 给已打的书签补 / 改备注（HUD 打点后追加输入，PRD-04-005）。
    pub fn bookmark_note(
        &self,
        session_id: Option<String>,
        abs_ms: u64,
        note: String,
    ) -> Result<(), String> {
        self.send_control(RecordingControl::BookmarkNote { abs_ms, note }, session_id)
    }

    /// 切换静音（录音中可调，PRD-04-003）。
    pub fn set_microphone_muted(
        &self,
        session_id: Option<String>,
        muted: bool,
    ) -> Result<(), String> {
        self.send_control(RecordingControl::SetMicrophoneMuted { muted }, session_id)
    }

    /// 切换实时转写喂窗闸门（录音中可调，PRD-04-012）。
    pub fn set_transcribe(&self, session_id: Option<String>, enabled: bool) -> Result<(), String> {
        self.send_control(RecordingControl::SetTranscribe { enabled }, session_id)
    }

    /// 切换实时转译喂句闸门（录音中可调，PRD-04-011/012）。
    pub fn set_translate(&self, session_id: Option<String>, enabled: bool) -> Result<(), String> {
        self.send_control(RecordingControl::SetTranslate { enabled }, session_id)
    }

    /// 现场切换实时转译目标外语（录音/字幕面板下拉，PRD-04-041）：热更新翻译配置，下一句界即生效。
    pub fn set_translate_target(
        &self,
        session_id: Option<String>,
        lang: String,
    ) -> Result<(), String> {
        self.send_control(RecordingControl::SetTranslateTarget { lang }, session_id)
    }

    /// 现场切换转写源语言（录音/字幕面板下拉，PRD-04-046）：`None` = 自动检测。
    pub fn set_transcribe_lang(
        &self,
        session_id: Option<String>,
        lang: Option<String>,
    ) -> Result<(), String> {
        self.send_control(RecordingControl::SetTranscribeLang { lang }, session_id)
    }

    /// 关闭 HUD 时复位状态：**仅在无活动会话时**把状态清回 idle（清除 error/失败残留），
    /// 录音中调用为 no-op（防止误关中断会话）。返回是否已复位。
    pub fn dismiss_idle(&self) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.active.is_some() {
            return false; // 录音中不复位
        }
        let idle = RecordingState::idle();
        let transition = is_state_transition(&inner.state, &idle);
        inner.state = idle.clone();
        inner.pending_control = None;
        drop(inner);
        if transition {
            self.notify_transition(&idle);
        }
        true
    }

    /// 取消持有指定 MicHolder 的录音会话（麦克风仲裁层调用）。
    pub fn cancel_holder(&self, holder: MicHolder) -> Result<(), String> {
        let session_id = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "录音状态锁中毒".to_string())?;
            let Some(active) = &inner.active else {
                return Ok(());
            };
            if active.mic_holder != holder {
                return Ok(());
            }
            active.session_id.clone()
        };
        self.stop(Some(session_id)).map(|_| ())
    }

    // ── 私有辅助 ──────────────────────────────────────────────────────────────

    fn send_control(
        &self,
        control: RecordingControl,
        session_id: Option<String>,
    ) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "录音状态锁中毒".to_string())?;
        let Some(active) = &inner.active else {
            if recording_should_queue_pending(false, session_id.as_deref()) {
                if let Some(id) = session_id {
                    inner.pending_control = Some((id, control));
                }
                eprintln!("[F4] 控制事件早于活动会话到达，已按 session 暂存。");
            }
            return Ok(());
        };
        if !recording_control_targets_active(active.session_id.as_str(), session_id.as_deref()) {
            return Err("录音会话已变化，请重试操作。".to_string());
        }
        active
            .control_tx
            .send(control)
            .map_err(|_| "录音会话已结束".to_string())
    }

    fn replace_state(&self, state: RecordingState) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        // 停止请求已被接受后，采集通道里可能仍排着最后几个 Level 状态。它们不得把
        // `Stopping` 回写成 `Recording`/`Paused`；只有会话终态能结束收尾状态。
        if inner.state.status == RecordingStatus::Stopping
            && inner.state.session_id == state.session_id
            && matches!(
                state.status,
                RecordingStatus::Recording | RecordingStatus::Paused
            )
        {
            return;
        }
        let transition = is_state_transition(&inner.state, &state);
        inner.state = state.clone();
        drop(inner);
        if transition {
            self.notify_transition(&state);
        }
    }

    fn finish_active(&self, state: RecordingState) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.active = None;
        inner.state = state.clone();
        drop(inner);
        // 终态恒广播：它就是「异常结束 → 复位」要送达每一扇窗的那一条。
        self.notify_transition(&state);
    }

    fn forget_pending_control(&self, session_id: Option<&str>) {
        let Some(id) = session_id else {
            return;
        };
        if let Ok(mut inner) = self.inner.lock() {
            if inner
                .pending_control
                .as_ref()
                .is_some_and(|(pending_id, _)| pending_id == id)
            {
                inner.pending_control = None;
            }
        }
    }
}

// ─── start() ─────────────────────────────────────────────────────────────────

/// 启动录音会话的选项。
/// 采集源选项（`recording.capture_source` / `caption.capture_source`，PRD-04-052 / ADR-019）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RecordingCaptureSource {
    /// 麦克风（默认）。
    #[default]
    Mic,
    /// 系统音频（会议对端声音；macOS CATap / Windows WASAPI loopback）。
    SystemAudio,
    /// 双轨（麦克风=我 + 系统=对方）：两条独立采集会话与 ASR 流并行，part 收尾离线混流为
    /// **立体声单文件**（L=mic / R=system，回放与权威重转写零改动，`luna_audio::muxer`）。
    Both,
}

impl RecordingCaptureSource {
    /// 设置字符串 → 采集源。调用方显式给出自身任务的默认：录音=Mic，实时字幕=Both。
    #[must_use]
    pub fn from_setting(value: Option<&str>, default: Self) -> Self {
        match value {
            Some("input_device") => Self::Mic,
            Some("system_loopback") => Self::SystemAudio,
            Some("both") => Self::Both,
            _ => default,
        }
    }

    /// 是否需要麦克风 TCC。单系统声音字幕不应被无关的麦克风授权阻断。
    #[must_use]
    pub const fn uses_microphone(self) -> bool {
        matches!(self, Self::Mic | Self::Both)
    }

    /// 全部采集源（`max_track_count` 的定义域；加变体必须同步加进来，由单测钉住）。
    pub const ALL: [Self; 3] = [Self::Mic, Self::SystemAudio, Self::Both];

    /// 本采集源并行跑几条实时 ASR 流。
    ///
    /// Both = 两条：麦克风轨与系统回环轨各有独立的 `StreamId` 与 `Transcribe` Job
    /// （见 `run_session` 里的双轨装配），并发解码。
    #[must_use]
    pub const fn track_count(self) -> usize {
        match self {
            Self::Mic | Self::SystemAudio => 1,
            Self::Both => 2,
        }
    }

    /// 任一采集源的轨数上限——**会话池额度的派生基数**（ADR-078 ①）。
    ///
    /// 从枚举**算**出来而不是写一个 2：加一种采集源（比如「麦克风 + 系统 + 第二只
    /// 麦克风」）时，会话池会自己跟着变大。写死的那个 2 不会，而它错了的表现是新采集源
    /// 的最后一条轨永远拿不到额度——一个只在特定档位下出现、且没有任何东西会变红的缺陷。
    #[must_use]
    pub fn max_track_count() -> usize {
        Self::ALL
            .iter()
            .map(|source| source.track_count())
            .max()
            .unwrap_or(1)
    }

    /// 双源中的两条 ASR 流必须各自检测语言；一个全局显式提示无法同时描述两种声源。
    /// 单源会话才应用用户选择的语言提示。
    #[must_use]
    fn effective_lang_hint(self, configured: Option<Lang>) -> Option<Lang> {
        match self {
            Self::Both => None,
            Self::Mic | Self::SystemAudio => configured,
        }
    }
}

pub struct RecordingStartOptions {
    /// 客户端生成的会话 ID（前端 round-trip 对账）。
    pub session_id: String,
    /// 会话形态（录音归档 / 实时字幕）：决定收尾是否归档、隐藏哪个窗口。
    pub purpose: SessionPurpose,
    /// 归档笔记路径（Vault 相对；`Caption` 形态为空串，不归档）。
    pub note_rel: String,
    /// 音频文件存放目录（绝对路径；`None` = 不落盘，仅内存转写。`Caption` 形态恒 `None`）。
    /// = `vault_root/<录音笔记附件目录>`（附件位置策略，PRD-06-012），不再是 vault_root 直接子目录。
    pub audio_dir: Option<PathBuf>,
    /// Vault 根绝对路径——writer 落盘绝对路径→frontmatter 用的 Vault 相对路径的换算锚点。
    /// 音频目录已随附件策略下沉（如 `录音/assets/`），不能再由 `audio_dir.parent()` 反推。
    /// `None` = 不落盘（`Caption`）。
    pub vault_root: Option<PathBuf>,
    /// 录音容器格式（当前仅 WAV；FLAC 编码器待 spike，TDD-02 §3.5）。
    pub format: RecordingFormat,
    /// 采集调参（波形帧率 / 磁盘预警阈值 / 滚动切分上限，来自设置，PRD-04 §5 / PRD-09-040）。
    pub tuning: luna_audio::CaptureTuning,
    /// VAD 实现（`None` = 能量 VAD；Silero 在场且设置选了它时由装配点给，ADR-012）。
    pub vad: Option<luna_audio::VadFactory>,
    /// 转写语种提示（`None` = 自动检测，PRD-04-014）。
    pub lang_hint: Option<Lang>,
    /// 指定输入设备名（`recording.input_device` 设置；`None`/未找到 → 系统默认；仅麦克风源生效）。
    pub input_device: Option<String>,
    /// 采集源（`recording.capture_source` 设置，PRD-04-052）。
    pub capture_source: RecordingCaptureSource,
    /// 实时转写初始闸门（`realtime.transcribe_enabled` 设置；会话中可经控制切换）。
    pub transcribe_enabled: bool,
    /// 实时转译初始闸门（`realtime.translate_enabled` 设置；会话中可经控制切换，TDD-08）。
    pub translate_enabled: bool,
    /// 实时转译配置（智能双向 + 注入检测器；语言决策与弹窗翻译同源，DRY）。
    pub translate_config: TranslationConfig,
    /// Vault 个人词典（PRD-02-040~042）：ASR committed 段显示/翻译/归档前统一 canonical 化。
    pub glossary: Vec<GlossaryEntry>,
    /// 归档写进 `MediaRef.model` 的转写模型名。
    ///
    /// 是个**共享格子**而不是一个值：会话开始时装配层只知道本机权重叫什么，而转写
    /// 真正由谁执行要等服务端回报 `executed_by`（可能还中途降级换过腿）。归档发生在
    /// 会话结束时，那时格子里是最后一个如实的答案。
    /// 传一个开始时的快照，就等于把「我们以为会用哪个模型」写成了「实际用了哪个」。
    pub asr_model_name: Arc<Mutex<Option<String>>>,
    /// 流式 ASR 会话句柄来源（[`luna_core::StreamingAsr`] 接缝）；`None` = 无可用转写
    /// 通道 → 降级纯录音。
    pub asr_provider: Option<Arc<dyn luna_core::StreamingAsr>>,
    /// 归档回调：收到所有 parts + segments + 书签后调用（返回 note_rel 或错误）。
    pub finalize_fn: FinalizeFn,
    /// 主轨实时转写的旁听者（提词器跟读，ADR-162 ⑤ C4b）：每次 Partial 到达时收到这次新确定
    /// 的文字与此刻的暂定尾。`None` = 没人旁听。只读，不影响转写与归档。
    pub partial_observer: Option<PartialObserver>,
}

/// 实时转写旁听者：（这次新确定的文字，此刻的暂定尾）。
pub type PartialObserver = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// 归档回调类型（Send + 'static 以跨 tokio 任务边界）。
/// 参数：分片表、转写段、书签（会话绝对 ms，PRD-04-005）。
pub type FinalizeFn = Box<
    dyn FnOnce(Vec<RecordingPart>, Vec<Segment>, Vec<RecordingBookmark>) -> Result<String, String>
        + Send,
>;

/// 会话内收集的一条书签（PRD-04-005）：绝对毫秒 + 可选备注。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingBookmark {
    pub abs_ms: u64,
    pub note: Option<String>,
}

/// 给已收集的书签补备注：按 `abs_ms` 对上就改，对不上（用户改了时刻、或先输备注后打点）
/// 就当一条新书签——两种情况下用户写的字都不能丢。
fn attach_bookmark_note(bookmarks: &mut Vec<RecordingBookmark>, abs_ms: u64, note: String) {
    match bookmarks.iter_mut().rev().find(|b| b.abs_ms == abs_ms) {
        Some(existing) => existing.note = Some(note),
        None => bookmarks.push(RecordingBookmark {
            abs_ms,
            note: Some(note),
        }),
    }
}

impl RecordingController {
    /// 请求麦克风互斥、构造会话、派发异步 `run_session` 任务。
    ///
    /// 镜像 `VoiceInputController::start()`（ADR-003）。
    pub fn start(
        self: &Arc<Self>,
        queue: Arc<JobQueue>,
        mic: Arc<Mutex<MicArbiter>>,
        options: RecordingStartOptions,
        app_handle: tauri::AppHandle,
        on_event: Channel<RecordingEvent>,
    ) -> Result<RecordingState, String> {
        // 1. 麦克风硬互斥：尝试占用 MicHolder::Recording。
        {
            let mut arbiter = mic.lock().map_err(|_| "麦克风仲裁锁中毒".to_string())?;
            if let Err(err) = arbiter.try_acquire(MicHolder::Recording) {
                self.forget_pending_control(Some(&options.session_id));
                let state = mic_busy_state(&options, &err);
                self.replace_state(state.clone());
                let _ = on_event.send(RecordingEvent::State(state.clone()));
                return Ok(state);
            }
        }

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "录音状态锁中毒".to_string())?;

        // 2. 已有活动会话：拒绝重入（同一麦克风）。
        if inner.active.is_some() {
            // 麦克风已由活动会话合法持有（try_acquire 对同一持有者幂等）；
            // 不释放，避免把租约从活动会话脚下抽走（同 VoiceInputController）。
            let state = RecordingState {
                status: RecordingStatus::MicBusy,
                purpose: options.purpose,
                session_id: Some(options.session_id.clone()),
                note_rel: Some(options.note_rel.clone()),
                elapsed_ms: 0.0,
                message_key: Some("mic.busy.capture_active".into()),
                message_detail: None,
            };
            drop(inner);
            self.replace_state(state.clone());
            let _ = on_event.send(RecordingEvent::State(state.clone()));
            return Ok(state);
        }

        // 3. 取早到控制事件（若有）。波形电平通道不在此快照——会话运行中每帧读
        //    `self.level_channel` 的当前活订阅（见 `subscribe_levels` 文档），跨会话/晚挂载
        //    的 HUD 均即时生效。
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let pending_control = inner
            .pending_control
            .take()
            .and_then(|(pending_id, control)| {
                (pending_id == options.session_id).then_some(control)
            });

        // 4. 写入 active + 初始 Recording 状态。
        inner.active = Some(ActiveRecording {
            session_id: options.session_id.clone(),
            control_tx: control_tx.clone(),
            mic_holder: MicHolder::Recording,
            on_event: Some(on_event.clone()),
        });
        let state = recording_state(
            RecordingStatus::Recording,
            options.purpose,
            &options.session_id,
            &options.note_rel,
            0,
            "recording:status.recording",
        );
        inner.state = state.clone();
        drop(inner);
        self.notify_transition(&state);

        let _ = on_event.send(RecordingEvent::State(state.clone()));

        // 5. 派发异步会话任务（必须用 tauri::async_runtime::spawn，同 VoiceInputController）。
        let controller = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            controller
                .run_session(queue, mic, options, app_handle, on_event, control_rx)
                .await;
        });

        // 6. 消费早到控制事件。
        if let Some(control) = pending_control {
            eprintln!("[F4] start 消费早到控制事件，立即下发。");
            let _ = control_tx.send(control);
        }

        Ok(state)
    }
}

// ─── 实时转写腿（ADR-080：腿的存在性 ≡ 转写闸门）──────────────────────────────

/// 一条 part 上「实时转写」的采集侧全部句柄（主轨 + 可选副轨）。
///
/// 只在 [`open_asr_legs`] → [`install_asr_legs`] 这一小段里以整体存在：`run_session`
/// 内部必须把它**拆成独立变量**放（`tokio::select!` 的两条 Job 臂各要一个 `&mut`，
/// 同一个结构体给不出两份），故这里只做「一次建好、一次装入」的载体。
struct AsrLegs {
    window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    asr_handle: Option<JobHandle>,
    stream_lang: Option<Arc<dyn luna_core::StreamLangControl>>,
    sys_window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    sys_asr_handle: Option<JobHandle>,
    sys_stream_lang: Option<Arc<dyn luna_core::StreamLangControl>>,
    /// RTF 探针所在流的每窗耗时回读端（Both = 副轨，单源 = 主轨）。
    infer_rx: Option<std::sync::mpsc::Receiver<u64>>,
    /// 两条轨各自**登记**的回收句柄（ADR-082）。与 `window_tx` 分开一份：关流只丢发送端，
    /// 而窗缓冲是随**接收端**走的，那一半住在 Provider 的登记表里。
    release: Option<luna_core::StreamRelease>,
    sys_release: Option<luna_core::StreamRelease>,
}

impl AsrLegs {
    /// 一条腿都没有（转写闸门关着）。
    fn none() -> Self {
        Self {
            window_tx: None,
            asr_handle: None,
            stream_lang: None,
            sys_window_tx: None,
            sys_asr_handle: None,
            sys_stream_lang: None,
            infer_rx: None,
            release: None,
            sys_release: None,
        }
    }
}

/// 建腿：`register_stream` + 提交流式 `Transcribe`（每轨一条独立会话）。
///
/// **`transcribe_gate` 关着就一条腿都不建**（ADR-080 ②）——判据只在这一个函数里，
/// 两处入口（part 起步 / 会话中途重新打开）共用它。实时腿的 `execute` 是**建连即连**、
/// 不等第一个窗，而它按 WebSocket 连接时长计费：为一个没人要字幕的 part 建腿，就是为
/// 一条从不上行的连接付整场的钱。在设置里把实时转写关掉的用户，此前每一次录音、每一次
/// pause→resume 的新 part 都在付这笔。
fn open_asr_legs(
    provider: &Arc<dyn luna_core::StreamingAsr>,
    queue: &JobQueue,
    options: &RecordingStartOptions,
    with_system_track: bool,
    transcribe_gate: bool,
) -> AsrLegs {
    if !transcribe_gate {
        return AsrLegs::none();
    }
    let stream_id = StreamId::new();
    let effective_lang_hint = options
        .capture_source
        .effective_lang_hint(options.lang_hint.clone());
    let reg = provider.register_stream(stream_id);
    let mut job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Stream(stream_id),
            lang_hint: effective_lang_hint.clone(),
            initial_prompt: glossary_initial_prompt(&options.glossary),
        },
        JobOrigin::Recording,
    );
    // 会话绑定在**这一个** Provider 实例上——见 `Router::resolve` 里对流式 Job 的钉选论证。
    job.routing.preferred_provider = Some(provider.id());
    job.priority = Priority::Normal;
    let mut legs = AsrLegs {
        asr_handle: Some(queue.submit(job)),
        // RTF 探针挂哪条流：Both = 副轨（会议负载大头=对方语音；主轨窗共享其档位门控但
        // 不聚合 RTF——RTF 的 infer/audio 必须同流配对）；单源 = 主轨。论证见 `run_session`
        // 里构造 `RealtimeSession` 那一段（ADR-078 ②）。
        infer_rx: Some(reg.infer_rx),
        window_tx: Some(reg.window_tx),
        stream_lang: Some(reg.lang),
        release: Some(reg.release),
        sys_window_tx: None,
        sys_asr_handle: None,
        sys_stream_lang: None,
        sys_release: None,
    };
    if with_system_track {
        let sys_stream_id = StreamId::new();
        let sys_reg = provider.register_stream(sys_stream_id);
        let mut sys_job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(sys_stream_id),
                lang_hint: effective_lang_hint,
                initial_prompt: glossary_initial_prompt(&options.glossary),
            },
            JobOrigin::Recording,
        );
        sys_job.routing.preferred_provider = Some(provider.id());
        sys_job.priority = Priority::Normal;
        legs.sys_asr_handle = Some(queue.submit(sys_job));
        legs.sys_window_tx = Some(sys_reg.window_tx);
        legs.sys_stream_lang = Some(sys_reg.lang);
        legs.sys_release = Some(sys_reg.release);
        legs.infer_rx = Some(sys_reg.infer_rx);
    }
    legs
}

/// 把建好的腿装进 `run_session` 的那组独立变量，并把新探针交给 [`RealtimeSession`]。
///
/// 参数多是因为那组变量必须分开放（见 [`AsrLegs`]）；写成一个函数是为了**建腿的两处
/// 入口**（part 起步 / 会话中途重新打开转写）装的是同一份东西——两处各抄一遍的话，
/// 下次多一个句柄就会只有一处跟上。
#[allow(clippy::too_many_arguments)]
fn install_asr_legs(
    legs: AsrLegs,
    window_tx: &mut Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    asr_handle: &mut Option<JobHandle>,
    stream_lang: &mut Option<Arc<dyn luna_core::StreamLangControl>>,
    release: &mut Option<luna_core::StreamRelease>,
    sys_window_tx: &mut Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    sys_asr_handle: &mut Option<JobHandle>,
    sys_stream_lang: &mut Option<Arc<dyn luna_core::StreamLangControl>>,
    sys_release: &mut Option<luna_core::StreamRelease>,
    rt_session: Option<&mut RealtimeSession>,
) {
    *window_tx = legs.window_tx;
    *asr_handle = legs.asr_handle;
    *stream_lang = legs.stream_lang;
    *release = legs.release;
    *sys_window_tx = legs.sys_window_tx;
    *sys_asr_handle = legs.sys_asr_handle;
    *sys_stream_lang = legs.sys_stream_lang;
    *sys_release = legs.sys_release;
    if let Some(session) = rt_session {
        session.attach_infer(legs.infer_rx);
    }
}

/// 收腿：丢掉采集侧发送端（两条轨一起）+ 丢句柄。
///
/// **关流就是丢掉 `window_tx`**（ADR-079 ②：采集侧从不投 `None`，走的一直是丢发送端
/// 那一种），腿据此发 `END`、关连接、结算——「转写闸门关掉」与「停录」在这一步上做的
/// 事逐字相同，故共用这一个函数，不新造第二种收尾。
///
/// **刻意不 `cancel()`**（ADR-080 ③）：取消是「断」，关流是「说完了」。用户拨下开关
/// 时最后那一窗已经在解码，走关流才拿得到它的 committed 定稿；取消会把它连同连接一起
/// 掐掉，而那句话用户真的说过。停录路径另行 `cancel` 的是**采集侧**的令牌，不是这里。
///
/// **连登记一起注销**（ADR-082）：判据是「还要不要这条 Job 的结果」，不是「关没关流」
/// ——关流恒早于终态，光看关流就注销会把 `execute` 手上还没解完的窗连同登记一起撤掉
/// （语音输入松手即关流、识别还在跑，正是那个形状）。而**这里**同一步就把句柄丢了：
/// 从此没有人在等这条 Job 的任何事件，于是登记也该当场还回去。不还的话，走不到
/// `execute` 的那条 Job（准入被拒 / 路由失败）会把窗缓冲一直攥到 part 收尾。
fn close_asr_legs(
    window_tx: &mut Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    asr_handle: &mut Option<JobHandle>,
    release: &mut Option<luna_core::StreamRelease>,
    sys_window_tx: &mut Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
    sys_asr_handle: &mut Option<JobHandle>,
    sys_release: &mut Option<luna_core::StreamRelease>,
) {
    drop(window_tx.take());
    drop(sys_window_tx.take());
    // 句柄一丢，`JobQueue` 的 worker 在下一个事件推不出去时即返回并丢掉 Provider 流
    // （ADR-079「已知边界」那一段依赖的正是这一条）。
    let _ = asr_handle.take();
    let _ = sys_asr_handle.take();
    drop(release.take());
    drop(sys_release.take());
}

// ─── run_session ─────────────────────────────────────────────────────────────

impl RecordingController {
    /// 主录音循环（在 tokio 任务中运行）—— 流式实时转录路径（TDD-02 §3.3-B / TDD-01 §5.9）。
    ///
    /// 架构（ADR-001/003/009/010、TDD-05 §3/§5.3/§6.1）：
    /// - 采集 `CaptureMode::RealtimeStream`：消费线程产 `Window`（**会话绝对时钟**）+ 落盘（writer）。
    /// - `register_stream` + `Transcribe{AudioRef::Stream}` 走单一 AI 主干；LocalAgreement-2 在 Provider。
    /// - `RealtimeSession` 聚合 RTF（`infer_rx`）驱动降级；窗喂 whisper、`Partial` 回 HUD。
    /// - 段绝对时间与**落盘文件逐样本对齐**（writer 与 segmenter 同源归一化帧）→ 回放 seek 不漂移。
    /// - 无 ASR 模型 → 降级纯录音（音频仍落盘，可事后二次转写，PRD-04-013/030）。
    /// - 降级 / 磁盘水位经 `NotificationCenter::publish`（脱敏，不含转写内容，PRD-15）。
    #[allow(clippy::too_many_arguments)]
    async fn run_session(
        &self,
        queue: Arc<JobQueue>,
        mic: Arc<Mutex<MicArbiter>>,
        mut options: RecordingStartOptions,
        app_handle: tauri::AppHandle,
        on_event: Channel<RecordingEvent>,
        mut control_rx: mpsc::UnboundedReceiver<RecordingControl>,
    ) {
        let mut committed_parts: Vec<RecordingPart> = Vec::new();
        let mut all_segments: Vec<Segment> = Vec::new();
        // 书签（会话绝对 ms + 可选备注；finalize 时写 frontmatter 精确层 + 正文可读锚点行，
        // PRD-04-005）。备注是用户文本：只进归档，绝不进日志。
        let mut bookmarks_ms: Vec<RecordingBookmark> = Vec::new();
        // 静音与实时转写/转译闸门（PRD-04-003/011/012）：会话级状态，跨 pause-part 保持。
        let mut microphone_muted = false;
        let mut transcribe_gate = options.transcribe_enabled;
        let mut uplink = UplinkIndicator::default();
        let mut translate_gate = options.translate_enabled;
        // 转写语言自动检测回显（PRD-04-046）：上次已回报给面板的检测语言，变化才发事件。
        let mut last_detected_lang: Option<String> = None;
        // 已译句界单元（首段 id → 译文；停录收尾回填 all_segments，TDD-08 §4）。
        let mut translations: HashMap<SegmentId, String> = HashMap::new();
        // ASR 丢窗计数（PRD-04-013：过载丢窗属设计内降级，但绝不静默——
        // 首次丢窗发 Degraded(window_drop) + 系统健康通知，会话内只告警一次防刷屏）。
        let mut window_drops: u64 = 0;
        // **卸载负载跳掉的窗也要有数**（TDD-02 §5.2）。它此前是一句静默的 `continue`：
        // Yellow/Orange 档会跳掉**全部快照窗**——而快照窗正是实时字幕连续出字的来源。
        // 于是「档位掉下去」在界面上只有一个降级角标，在 stdout 上一个字都没有，表现是
        // 「字幕忽然不更新了」，而没有任何东西说明它被跳了多少窗、在哪个档位跳的。
        // （2026-08-18 现场：说了 2 分钟只出两句，上行不足 60 秒，而窗通道一次都没满过。）
        let mut window_skips: u64 = 0;
        let mut last_skip_report = std::time::Instant::now();
        // ASR 链路脱敏诊断开关（与 LUNA_DEBUG_SHORTCUTS 同模式；默认关，排查准确度/丢窗时开）。
        let asr_debug = std::env::var_os("LUNA_DEBUG_ASR").is_some();
        // base_elapsed_ms = 已经归档 part 的累计录音 ms（不含当前 part；跨 pause 连续）。
        let mut base_elapsed_ms: u64 = 0;
        let mut part_idx: u32 = 0;
        let terminal: SessionTerminal;

        // Vault 根（把 writer 绝对落盘路径换算成 frontmatter 用的 Vault 相对路径）。
        // 音频目录随附件策略下沉（如 `录音/assets/`），故由编排层显式传入真实 vault root，
        // 不再由 `audio_dir.parent()` 反推（那样只会得到附件目录的父级而非 vault 根）。
        let vault_root = options.vault_root.clone();

        'outer: loop {
            // ── 每个采集会话（pause 之间一段）──────────────────────────────────

            // 本采集会话落盘规格（writer 滚动切分会再派生 partN；按 SaveSpec.path 命名）。
            let is_both = options.capture_source == RecordingCaptureSource::Both;
            // Both 双轨共享主机时钟原点（§3.1-I 任务④）：令 mic/sys 两会话 first_nanos 跨会话
            // 可比，part 收尾据锚点差对晚启动轨头部补零对齐（启动偏差校正）。单/非 Both = None。
            let shared_epoch = is_both.then(std::time::Instant::now);
            let mic_clock = shared_epoch.map(luna_audio::CaptureClock::new);
            let sys_clock = shared_epoch.map(luna_audio::CaptureClock::new);
            let save = options.audio_dir.as_ref().map(|dir| SaveSpec {
                format: options.format,
                // Both：主轨（mic）带 -mic 后缀，part 收尾混流为无后缀立体声文件。
                path: part_file(
                    dir,
                    &options.session_id,
                    part_idx,
                    if is_both { "-mic" } else { "" },
                    options.format,
                ),
                sample_rate: TARGET_SAMPLE_RATE,
            });
            let cfg = CaptureConfig {
                mode: CaptureMode::RealtimeStream,
                emit_level: true,
                tuning: options.tuning,
                vad: options.vad.clone(),
                save,
                // 采集源（PRD-04-052）：麦克风为具名设备（`recording.input_device`，未找到由
                // capture 回退默认并发诊断）；系统音频为 SystemLoopback（平台路径见
                // start_system_capture）。
                source: match options.capture_source {
                    RecordingCaptureSource::Mic | RecordingCaptureSource::Both => {
                        CaptureSource::InputDevice(options.input_device.clone())
                    }
                    RecordingCaptureSource::SystemAudio => CaptureSource::SystemLoopback,
                },
                ..CaptureConfig::default()
            };

            let (capture_sink, capture_rx) = std::sync::mpsc::channel();
            // 系统音频平台源守卫（macOS = CATap 运行句柄；其余平台/麦克风 = 空）。
            // 停止顺序契约（TDD-02 §3.1-I）：part 收尾时**先 release 守卫停平台源，再停 CaptureHandle**。
            let mut system_guard = SystemSourceGuard::none();
            // Both mic 侧 VPIO 守卫（ADR-021 主力）+ 回退 AEC3 的 far-end 发送端。
            let mut mic_vpio_guard = MicVpioGuard::none();
            let mut aec_render_for_sys: Option<luna_audio::FarEndFeeder> = None;
            let capture = match options.capture_source {
                RecordingCaptureSource::Mic | RecordingCaptureSource::Both => {
                    // Both 外放回声分层（ADR-021）：macOS 主力 = 系统级 VPIO（回声在系统层
                    // 消除、16k 直出零重采样，S14 实测泄漏消除 ~24dB）；不可用 → cpal +
                    // 应用内 AEC3 回退。两层**绝不叠加**（双重非线性抑制伤语音）。
                    // 用户显式指定输入设备时尊重其选择走 cpal（VPIO 恒用系统默认输入）；
                    // Mic 单源恒 cpal 原始采集（无回声问题，保留保真不套语音处理）。
                    let mut vpio_handle: Option<luna_audio::CaptureHandle> = None;
                    if is_both && options.input_device.is_none() {
                        match start_vpio_mic_capture(&cfg, capture_sink.clone(), mic_clock.clone())
                        {
                            Ok((handle, guard)) => {
                                mic_vpio_guard = guard;
                                vpio_handle = Some(handle);
                                if part_idx == 0 {
                                    eprintln!(
                                        "[F4] Both mic 采集经 VPIO（系统级回声消除，ADR-021）。"
                                    );
                                }
                            }
                            Err(code) => {
                                eprintln!("[F4] VPIO 不可用（{code}），回退 cpal + AEC3。");
                            }
                        }
                    }
                    match vpio_handle {
                        Some(handle) => handle,
                        None => {
                            // 回退/单源：cpal。Both 回退时构造应用内 AEC3（TDD-02 §3.1-I
                            // 层③回退线）：system 轨归一化输出为参考、mic 轨写盘/波形/VAD
                            // 前消回声；构造失败 → 直通 + 层②转写去重兜底。
                            let aec_capture = if is_both {
                                match luna_audio::create_echo_cancel_pair() {
                                    Some((canceller, feeder)) => {
                                        aec_render_for_sys = Some(feeder);
                                        Some(canceller)
                                    }
                                    None => {
                                        eprintln!("[F4] AEC 不可用（平台/构造失败），Both 直通继续（层②转写去重兜底）。");
                                        None
                                    }
                                }
                            } else {
                                None
                            };
                            match start_capture(
                                &cfg,
                                capture_sink.clone(),
                                mic_clock.clone(),
                                aec_capture.map(|c| luna_audio::AecRole::Capture(Box::new(c))),
                            ) {
                                Ok(handle) => handle,
                                Err(e) => {
                                    terminal = SessionTerminal::CaptureError(format!(
                                        "CAPTURE_FAILED: {e}"
                                    ));
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                RecordingCaptureSource::SystemAudio => {
                    match start_system_capture(&cfg, capture_sink.clone(), None, None) {
                        Ok((handle, guard)) => {
                            system_guard = guard;
                            if part_idx == 0 {
                                maybe_publish_speaker_playback_hint(&app_handle);
                            }
                            handle
                        }
                        Err(code) => {
                            terminal = SessionTerminal::CaptureError(code);
                            break 'outer;
                        }
                    }
                }
            };
            // `capture_sink` 只用于把同一个事件出口交给「VPIO 尝试 → cpal 回退」的候选路径。
            // 采集建立后必须释放编排层根副本：CaptureHandle 停止并丢掉管线副本后，桥接线程
            // 才能观察到 std channel EOF。此前 Both+VPIO 成功路径遗留此副本，停止会永久等在
            // `audio_rx.recv()`，正是实时字幕无法结束的根因。
            drop(capture_sink);

            // 桥接线程：将 std::sync::mpsc 事件转发到 tokio mpsc（同 VoiceInputController）。
            let (audio_tx, mut audio_rx) = mpsc::unbounded_channel();
            std::thread::Builder::new()
                .name(format!("luna-recording-events-p{part_idx}"))
                .spawn(move || {
                    while let Ok(event) = capture_rx.recv() {
                        if audio_tx.send(event).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn recording capture bridge");

            // ── Both 副轨：系统音频独立采集会话（PRD-04-052 双轨）────────────
            // 失败**降级仅麦克风**（非致命：会议仍录到自己 + 健康通知引导授权/版本）。
            let mut sys_capture_opt: Option<luna_audio::CaptureHandle> = None;
            let mut sys_audio_rx: Option<mpsc::UnboundedReceiver<CaptureEvent>> = None;
            if is_both {
                let sys_save = options.audio_dir.as_ref().map(|dir| SaveSpec {
                    format: options.format,
                    path: part_file(dir, &options.session_id, part_idx, "-sys", options.format),
                    sample_rate: TARGET_SAMPLE_RATE,
                });
                let sys_cfg = CaptureConfig {
                    tuning: options.tuning,
                    vad: options.vad.clone(),
                    mode: CaptureMode::RealtimeStream,
                    // 副轨独立电平（TDD-02 §6 / PRD-04-052「两者」双电平条）：HUD 双轨条
                    // 数据源；计时仍由主轨 Level 驱动（副轨 Level 只广播不驱动状态）。
                    emit_level: true,
                    save: sys_save,
                    source: CaptureSource::SystemLoopback,
                    ..CaptureConfig::default()
                };
                let (sys_sink, sys_rx_std) = std::sync::mpsc::channel();
                match start_system_capture(
                    &sys_cfg,
                    sys_sink,
                    sys_clock.clone(),
                    aec_render_for_sys
                        .take()
                        .map(|f| luna_audio::AecRole::Render(Box::new(f))),
                ) {
                    Ok((handle, guard)) => {
                        system_guard = guard;
                        // VPIO 在场时外放回声已在系统层消除——「建议佩戴耳机」提示不再适用。
                        if part_idx == 0 && !mic_vpio_guard.is_active() {
                            maybe_publish_speaker_playback_hint(&app_handle);
                        }
                        sys_capture_opt = Some(handle);
                        let (tx, rx) = mpsc::unbounded_channel();
                        std::thread::Builder::new()
                            .name(format!("luna-recording-sys-events-p{part_idx}"))
                            .spawn(move || {
                                while let Ok(event) = sys_rx_std.recv() {
                                    if tx.send(event).is_err() {
                                        break;
                                    }
                                }
                            })
                            .expect("spawn system capture bridge");
                        sys_audio_rx = Some(rx);
                    }
                    Err(code) => {
                        eprintln!("[F4] 系统音频副轨启动失败（{code}），降级仅麦克风继续。");
                        publish_recording_health(
                            &app_handle,
                            "recording_system_audio_no_data",
                            "system_track_failed".into(),
                        );
                    }
                }
            }

            // 流式 ASR 会话（有模型才转写；无模型纯录音）。窗口发送端 / Job 句柄 / RTF 会话拆为
            // 独立变量，使 tokio::select 各臂借用不相交（喂窗 vs 并发轮询 Partial）。
            let cancel = CancelToken::new();
            let mut window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>> = None;
            let mut asr_handle: Option<JobHandle> = None;
            // 会话**登记**的回收句柄，与关流是两件事（`StreamRelease` 的文档逐条写了
            // 为什么）：登记由 `execute` 摘走，而 Job 有走不到 `execute` 的出口——会话池
            // 准入到期即拒（ADR-078 ③）、路由解析失败。那时不丢掉它，Provider 的登记表里
            // 就留下一条攥着满通道窗缓冲的会话，而流式 Provider 实例是复用的，一场一场叠加。
            let mut stream_release: Option<luna_core::StreamRelease> = None;
            let mut rt_session: Option<RealtimeSession> = None;
            let mut last_tier: Option<String> = None;
            // 各轨是否报过「还在等本机会话额度」（ADR-078 ③）。额度到手后必须**撤掉**那条
            // 横幅：一条挂着不走的「还在等」，在字幕已经开始滚动之后就是在说假话。
            let mut admission_waiting = [false; 2];
            // 本 part 的共享语言锁句柄（现场切换转写语言 / 回读 auto 检测，PRD-04-046）。
            // 语言控制面按**接缝类型**持有：会话不该知道自己控制的是本机 whisper 还是远程转写。
            let mut stream_lang: Option<std::sync::Arc<dyn luna_core::StreamLangControl>> = None;
            // 实时转译管线（TDD-08）：仅在有 ASR（有 committed 段可译）时随 part 构建；
            // `translate_gate` 控制喂句（会话中可切换），降级门控经 RealtimeSession watch。
            let mut translate_pipeline: Option<RealtimeTranslationPipeline> = None;
            // 副轨 ASR 流（Both 且副轨采集在跑）：独立 StreamId/Job 并发解码（每窗独立
            // whisper state，ctx 只读共享；JobQueue 并发度 4 容纳双流）。
            let mut sys_window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>> = None;
            let mut sys_asr_handle: Option<JobHandle> = None;
            let mut sys_stream_release: Option<luna_core::StreamRelease> = None;
            let mut sys_stream_lang: Option<std::sync::Arc<dyn luna_core::StreamLangControl>> =
                None;
            if let Some(provider) = &options.asr_provider {
                // 联网指示与录音指示同时在场（ADR-038 ④）：这一条在**任何音频离开
                // 设备之前**发出，判据是 Provider 自报的位置，不依赖服务端回话。
                //
                // 但必须**同时**看转写开关：关掉实时转写时一个字节都不会外发
                // （`transcribe_gate` 挡住投窗），此时仍然点亮指示就是误报。隐私指示的
                // 全部价值在于准确——一旦用户发现关掉转写它还亮，真正外发时他也不会看。
                // 会话建立时两条轨用**同一个** Provider，故位置相同；之后各自可能独立降级。
                let leaves = provider.descriptor().location.is_external();
                uplink.per_track = [leaves, leaves && sys_audio_rx.is_some()];
                let _ = on_event.send(uplink.event(transcribe_gate));
                // RTF 探针挂哪条流（`open_asr_legs` 据此选 `infer_rx`）：Both = 副轨（会议
                // 负载大头=对方语音；主轨窗共享其档位门控但不聚合 RTF——RTF 的 infer/audio
                // 必须同流配对）；单源 = 主轨。
                //
                // > 假设（**部分被证伪，2026-08-18 / ADR-078 ②**）：单探针足够（TDD-02
                // > §3.1-I ASR 算力预算：两轨 VAD 互补，负载同机同池，单探针档位对双流同时
                // > 生效；理由：双探针会产生竞争性档位事件）。
                //
                // 它当初为什么成立：那时 whisper 池上**只有这一场会话的两条轨**在跑，
                // 两轨互补（一个人说话时另一个多半静默），所以任意一条轨测出的 RTF 就是
                // 整个池的负载，一个探针确实描述得了两条流。
                //
                // 被什么打破：**批式二次转写**。它跑在同一台机器的同一个 whisper 池里，
                // 却不产任何窗、不挂任何探针——于是探针测到的 RTF 里混进了一份它看不见也
                // 说不出来源的负载。「负载同机同池」这半句仍然为真，而且恰恰是它成了问题：
                // 同池意味着批式的开销会原样计入实时腿的 RTF，而当时控制器唯一的动作是
                // 跳实时快照窗 ⇒ 用户点了「二次转写」，代价是同一时刻的实时字幕停更。
                //
                // 现在的形状：探针仍然只有一个（竞争性档位事件那条理由没变），但控制器的
                // 输入从「这条流的 RTF」升级成了 `AsrLoad{realtime_rtf, batch_inflight}`
                // ——第二个字段正是这个探针测不到的那半份负载，由 `JobQueue` 如实提供。
                //
                // 负载探针取自同一个 `JobQueue`：会话据它判断「还有没有批式可卸」，
                // 并在该卸时扳上批式的准入闸（ADR-078 ②）。腿是后面按闸门建的，故
                // `infer_rx` 此刻留空，由 `install_asr_legs` 交给它（ADR-080）。
                let (session, _deg_rx) =
                    RealtimeSession::new(None, cancel.clone(), None, queue.asr_load());
                // 转译独立于 ASR 降级（引擎分治，TDD-08 §4）：不再接 ASR 挂起门控，管线自身就近优先卸载。
                translate_pipeline = Some(RealtimeTranslationPipeline::spawn(
                    Arc::clone(&queue) as Arc<dyn JobSink + Send + Sync>,
                    options.translate_config.clone(),
                ));
                rt_session = Some(session);
                // 腿按闸门建：闸关时 `open_asr_legs` 一条都不建（ADR-080 ②，判据在那里），
                // 之后由 `SetTranscribe` 臂补建。
                install_asr_legs(
                    open_asr_legs(
                        provider,
                        &queue,
                        &options,
                        sys_audio_rx.is_some(),
                        transcribe_gate,
                    ),
                    &mut window_tx,
                    &mut asr_handle,
                    &mut stream_lang,
                    &mut stream_release,
                    &mut sys_window_tx,
                    &mut sys_asr_handle,
                    &mut sys_stream_lang,
                    &mut sys_stream_release,
                    rt_session.as_mut(),
                );
            }

            // 轨级标签（PRD-04-053 L0）：主轨在 Both=self、单系统=remote、单麦克风=None。
            let primary_track: Option<&'static str> = match options.capture_source {
                RecordingCaptureSource::Mic => None,
                RecordingCaptureSource::SystemAudio => Some(TRACK_REMOTE),
                RecordingCaptureSource::Both => Some(TRACK_SELF),
            };
            // 跨 pause-part 保持静音状态（resume 新建采集会话，须重放当前静音位）。
            if microphone_muted && options.capture_source.uses_microphone() {
                capture.set_muted(true);
            }
            let mut capture_opt = Some(capture);
            let inner_exit: InnerExit;
            // 本采集会话墙钟起点：HUD 计时由**连续的 Level 事件**（采集存活即每帧触发，静音亦触发）
            // 驱动 `base + part_start.elapsed()`。采集活 → 平滑走；采集死（设备丢失等）→ 计时停，
            // 如实反映而非由「仅语音 Window」更新导致静音期假性冻结（真机：二次录音计时不动）。
            let part_start = std::time::Instant::now();
            // State 事件发往前端节流（~4/s）：波形/电平走 Level 通道逐帧驱动，State（计时/档位）
            // 不必每 Level 帧发一次——高频 State IPC 会与解码争抢前端主线程致波形卡顿（真机问题1）。
            let mut last_state_emit = std::time::Instant::now()
                .checked_sub(STATE_EMIT_INTERVAL)
                .unwrap_or_else(std::time::Instant::now);
            // 自适应卸载负载探测配额（Red 档每 PROBE_INTERVAL 放行一个探测窗重测 RTF）：
            // **每轨一个**，理由见 [`ProbeBudget`]。
            let mut probe = ProbeBudget::ready(std::time::Instant::now());
            let mut sys_probe = ProbeBudget::ready(std::time::Instant::now());

            // ── 内层采集循环（三臂 select：控制 / 采集事件 / ASR Partial）──────
            'inner: loop {
                tokio::select! {
                    Some(ctrl) = control_rx.recv() => {
                        match ctrl {
                            RecordingControl::Stop => { inner_exit = InnerExit::Stop; break 'inner; }
                            RecordingControl::Pause => { inner_exit = InnerExit::Pause; break 'inner; }
                            RecordingControl::Resume => { /* 未暂停时忽略 */ }
                            RecordingControl::Bookmark { abs_ms, note } => {
                                bookmarks_ms.push(RecordingBookmark { abs_ms, note });
                            }
                            RecordingControl::BookmarkNote { abs_ms, note } => {
                                attach_bookmark_note(&mut bookmarks_ms, abs_ms, note);
                            }
                            RecordingControl::SetMicrophoneMuted { muted: m } => {
                                microphone_muted = m;
                                if options.capture_source.uses_microphone() {
                                    if let Some(c) = &capture_opt {
                                        c.set_muted(m);
                                    }
                                }
                                // 静音后麦克风不会再产新 Window，也就没有下一条 Partial 自然清除
                                // 已显示的暂定尾；立即清本轨，系统轨 hypothesis 保持独立继续更新。
                                if m {
                                    let _ = on_event.send(RecordingEvent::Hypothesis {
                                        text: String::new(),
                                        track: primary_track.map(str::to_string),
                                    });
                                }
                            }
                            RecordingControl::SetTranscribe { enabled } if enabled == transcribe_gate => {
                                // 幂等：同一个值再来一次不重建腿（重建 = 新会话、新计费、
                                // 还要重过会话池准入，见 ADR-080 ④）。
                            }
                            RecordingControl::SetTranscribe { enabled } => {
                                transcribe_gate = enabled;
                                let had_system_leg = sys_asr_handle.is_some();
                                if enabled {
                                    // **重新打开 = 重新建一条会话**（ADR-080 ④）：新
                                    // `StreamId`、新 Job，`Started`/`ExecutedBy` 会再报一次，且
                                    // 要重过会话池准入——满了就是 `LocalSessionBusy`，走既有的
                                    // 「实时字幕已停，录音继续」通路如实报出（PRD-04-061），
                                    // 而不是静默。指示按 Provider 位置重新起算：上一条腿可能
                                    // 已经降级到本机，而新腿是从主腿重新开始的。
                                    if let Some(provider) = &options.asr_provider {
                                        uplink.name = None;
                                        let leaves = provider.descriptor().location.is_external();
                                        uplink.per_track =
                                            [leaves, leaves && sys_audio_rx.is_some()];
                                        admission_waiting = [false; 2];
                                        install_asr_legs(
                                            open_asr_legs(
                                                provider,
                                                &queue,
                                                &options,
                                                sys_audio_rx.is_some(),
                                                enabled,
                                            ),
                                            &mut window_tx,
                                            &mut asr_handle,
                                            &mut stream_lang,
                                            &mut stream_release,
                                            &mut sys_window_tx,
                                            &mut sys_asr_handle,
                                            &mut sys_stream_lang,
                                            &mut sys_stream_release,
                                            rt_session.as_mut(),
                                        );
                                    }
                                } else {
                                    // **关掉转写 = 把腿收掉，不是「不发这一窗」**（ADR-080 ①）。
                                    // 只翻布尔的话字节确实停了、指示也灭了，而那条 WebSocket
                                    // 会一直开着空转到停录——它按**连接时长**计费，还占着上游
                                    // 并发席位与那笔支出预留。关流走的是与停录逐字相同的一步。
                                    close_asr_legs(
                                        &mut window_tx,
                                        &mut asr_handle,
                                        &mut stream_release,
                                        &mut sys_window_tx,
                                        &mut sys_asr_handle,
                                        &mut sys_stream_release,
                                    );
                                    stream_lang = None;
                                    sys_stream_lang = None;
                                }
                                // 关掉转写 = 当场不再外发，指示必须跟着灭；重新打开则重新亮。
                                let _ = on_event.send(uplink.event(transcribe_gate));
                                if !enabled {
                                    // 清除各轨灰显暂定文本；已 committed 段保留（不回退）。
                                    let _ = on_event.send(RecordingEvent::Hypothesis {
                                        text: String::new(),
                                        track: primary_track.map(str::to_string),
                                    });
                                    if had_system_leg {
                                        let _ = on_event.send(RecordingEvent::Hypothesis {
                                            text: String::new(),
                                            track: Some(TRACK_REMOTE.into()),
                                        });
                                    }
                                }
                            }
                            RecordingControl::SetTranslate { enabled } => {
                                // 关 = 停喂翻译管线（在途句仍会译完并显示，不丢已请求内容）。
                                translate_gate = enabled;
                            }
                            RecordingControl::SetTranslateTarget { lang } => {
                                // 现场切目标外语（PRD-04-041）：热更新共享翻译配置（管线持同一 Arc），
                                // 下一句界单元即按新目标解析——无需重启会话。
                                options.translate_config.set_foreign_lang(Lang::from(lang.as_str()));
                            }
                            RecordingControl::SetTranscribeLang { lang } => {
                                // 现场切转写源语言（PRD-04-046）：热更共享语言锁（当前窗后生效）；
                                // 同步 lang_hint 使暂停/恢复的新 part 一致（新 part 重建 ASR 流）。
                                let configured = lang.as_deref().map(Lang::from);
                                options.lang_hint = options
                                    .capture_source
                                    .effective_lang_hint(configured);
                                let effective = options
                                    .lang_hint
                                    .as_ref()
                                    .map(ToString::to_string);
                                if let Some(sl) = &stream_lang {
                                    sl.set_configured(effective.clone());
                                }
                                if let Some(sl) = &sys_stream_lang {
                                    sl.set_configured(effective);
                                }
                                // 切换即清面板回显（显式指定无需回显；回 auto 待重新检测再回报）。
                                if last_detected_lang.take().is_some() {
                                    let _ = on_event.send(RecordingEvent::TranscribeLangDetected { lang: None });
                                }
                            }
                        }
                    }
                    event_opt = audio_rx.recv() => {
                        let Some(event) = event_opt else {
                            inner_exit = InnerExit::StreamEnded;
                            break 'inner;
                        };
                        match event {
                            CaptureEvent::Level(bytes) => {
                                // 波形/电平：逐帧走二进制 Level 通道（ADR-011），不节流——实时流畅的命门。
                                // **广播到全部活订阅**（每窗一槽，非快照/非单槽）：多常驻窗口互不覆盖，
                                // 晚挂载/重挂载的表面下一帧即收到波形。消息带轨前缀（TDD-02 §6 双轨电平）。
                                let tagged = luna_audio::tag_track_levels(
                                    luna_audio::LEVEL_TRACK_PRIMARY, &bytes,
                                );
                                if let Ok(guard) = self.level_channels.lock() {
                                    for ch in guard.values() {
                                        let _ = ch.send(voice_level_payload(tagged.clone()));
                                    }
                                }
                                // 连续墙钟计时（采集存活的唯一信号；静音亦触发 → 计时不假性冻结）。
                                let elapsed = base_elapsed_ms + part_start.elapsed().as_millis() as u64;
                                let state = recording_state(
                                    RecordingStatus::Recording, options.purpose, &options.session_id, &options.note_rel,
                                    elapsed, "recording:status.recording",
                                );
                                self.replace_state(state.clone());
                                // State（计时/状态）发往前端节流 ~4/s：高频 State IPC 会与解码争抢前端主
                                // 线程，是长会议波形冻结的加剧因素（问题1）；计时 4fps 视觉上足够平滑。
                                if last_state_emit.elapsed() >= STATE_EMIT_INTERVAL {
                                    last_state_emit = std::time::Instant::now();
                                    let _ = on_event.send(RecordingEvent::State(state));
                                }
                            }
                            CaptureEvent::Window(SpeechEvent::Window { samples, abs_start, speech_len, elapsed_samples, is_snapshot }) => {
                                // 喂窗给流式 whisper；RTF 的分母 = 自上一窗以来流过的**会话时长**
                                // （elapsed_samples，含静音），不是窗里的语音时长——底噪切出的短窗
                                // 每个都付整窗解码，按语音算会把一成多的负载算成 RTF 1–2.6（TDD-02 §5.1）。
                                // 计时不在此更新（由 Level 连续驱动），本臂仅喂 whisper + 聚合降级。
                                let elapsed_ms = (elapsed_samples as u64 * 1000) / TARGET_SAMPLE_RATE as u64;
                                // Both 时探针挂副轨，主轨不聚合（infer/elapsed 必须同流配对）。
                                let rtf_probe_here = sys_asr_handle.is_none();
                                // 转写闸门（PRD-04-012）：关 = 不喂窗、不聚合 RTF；录音/落盘照常。
                                // 有 ASR（window_tx 与 rt_session 同生），据当前档位自适应卸载负载（问题1根治）。
                                if transcribe_gate {
                                    if let (Some(tx), Some(session)) = (&window_tx, &mut rt_session) {
                                        // 卸载负载决策（TDD-02 §5.2）：高负载跳过快照、Red 仅探测窗放行。
                                        // 跳过 = 不解码本窗（音频已落盘，停录二次转写补齐权威结果）——释放
                                        // CPU 使波形/界面不被饥饿冻结，并让 RTF 有机会回落自动恢复实时。
                                        // 配额随本轨走（Yellow/Orange 跳快照不占探测配额）。
                                        //
                                        // 用 `realtime_decode_tier()` 而**不是** `current_tier()`
                                        // （ADR-078 ②）：档位语义一个字没改，改的是「什么时候轮到实时
                                        // 被跳」——只要还有批式转写可卸，这一位就是 Green。拿实时窗去
                                        // 补一份晚点解也一样的活，方向是反的。
                                        let tier = session.realtime_decode_tier();
                                        if !probe.admit(tier, is_snapshot, std::time::Instant::now())
                                        {
                                            // 跳过的窗时间照样流过：记进下一个喂解码窗的分母，
                                            // 否则降档后 RTF 只剩终窗/探测窗自己那一小段，永远降不回来。
                                            if rtf_probe_here {
                                                session.on_skipped_window(elapsed_ms as f32);
                                            }
                                            window_skips += 1;
                                            if last_skip_report.elapsed() >= SKIP_REPORT_INTERVAL {
                                                last_skip_report = std::time::Instant::now();
                                                eprintln!(
                                                    "[F4] 卸载负载：已跳过 {window_skips} 窗（当前档位 {tier:?}；\
                                                     Yellow/Orange 会跳掉全部快照窗 = 实时字幕停更）"
                                                );
                                            }
                                            continue;
                                        }
                                        // 实时路径 try_send（不阻塞事件循环）；满 = 解码积压 128 窗的极端
                                        // 过载，丢窗属设计内降级，但**绝不静默**（脱敏：仅计数无内容）。
                                        // 音频已全量落盘，可二次转写兜底（PRD-04-013/030）。
                                        if tx
                                            .try_send(Some(WindowInput {
                                                samples,
                                                abs_start,
                                                speech_len,
                                            }))
                                            .is_err()
                                        {
                                            if rtf_probe_here {
                                                session.on_skipped_window(elapsed_ms as f32);
                                            }
                                            window_drops += 1;
                                            eprintln!("[F4] ASR 窗通道满，丢弃 1 窗（abs_start={abs_start}；音频已落盘可二次转写）。");
                                            if window_drops == 1 {
                                                let _ = on_event.send(RecordingEvent::Degraded {
                                                    tier: "yellow".into(),
                                                    reason: "window_drop".into(),
                                                });
                                                publish_recording_health(
                                                    &app_handle,
                                                    "recording_window_dropped",
                                                    "window_drop".into(),
                                                );
                                            }
                                            continue;
                                        } else if asr_debug {
                                            // 脱敏诊断（仅计数/采样位移，无正文）：确认实时窗确有产出并喂入 whisper。
                                            eprintln!("[F4] 喂入 ASR 窗 abs_start={abs_start} speech_len={speech_len}（距上一窗 ≈{elapsed_ms}ms）。");
                                        }
                                        // 真正喂入解码的窗驱动 RTF（跳过窗的时长已由 on_skipped_window 记下）。
                                        if rtf_probe_here {
                                            if let Some(deg) = session.on_audio_window(elapsed_ms as f32) {
                                                let tier = format!("{:?}", deg.tier).to_lowercase();
                                                if last_tier.as_deref() != Some(tier.as_str()) {
                                                    last_tier = Some(tier.clone());
                                                    let _ = on_event.send(RecordingEvent::Degraded {
                                                        tier: tier.clone(),
                                                        reason: "high_rtf".into(),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            CaptureEvent::Diagnostic(diag) => {
                                // 设备丢失/磁盘满 → 优雅收尾（保住已采音频 + 通知），绝不静默挂死。
                                let fatal = matches!(diag,
                                    CaptureDiagnostic::DiskCritical { .. } | CaptureDiagnostic::DeviceLost);
                                // 主轨是否麦克风（Mic/Both；单系统源主轨的零=系统静音属正常）。
                                let mic_track = matches!(
                                    options.capture_source,
                                    RecordingCaptureSource::Mic | RecordingCaptureSource::Both
                                );
                                handle_capture_diagnostic(&app_handle, &on_event, &diag, mic_track);
                                if fatal {
                                    // 磁盘满 / 设备丢失：保住已录音频并停采 → 走收尾归档（不丢已采内容）。
                                    inner_exit = InnerExit::StreamEnded;
                                    break 'inner;
                                }
                            }
                            // RealtimeStream 模式不产 SpeechStarted/端点 Utterance（既有路径事件）。
                            CaptureEvent::SpeechStarted
                            | CaptureEvent::Utterance { .. }
                            | CaptureEvent::Window(SpeechEvent::Utterance { .. }) => {}
                        }
                    }
                    // **副轨**采集事件（Both）：窗喂副轨流（探针在副轨：档位门控 + RTF 聚合
                    // 都在此臂）；副轨死亡降级仅麦克风，不拆整个会话。
                    sys_event_opt = async {
                        match &mut sys_audio_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending::<Option<CaptureEvent>>().await,
                        }
                    } => {
                        let Some(event) = sys_event_opt else {
                            sys_audio_rx = None; // 副轨桥接 EOF：降级仅麦克风继续
                            let _ = on_event.send(RecordingEvent::Hypothesis {
                                text: String::new(),
                                track: Some(TRACK_REMOTE.into()),
                            });
                            continue;
                        };
                        match event {
                            CaptureEvent::Level(bytes) => {
                                // 副轨电平（TDD-02 §6 / PRD-04-052 双电平条）：同一通道带轨前缀
                                // 广播（HUD 订阅面不变）；计时/状态恒由主轨 Level 驱动，此处不碰。
                                let tagged = luna_audio::tag_track_levels(
                                    luna_audio::LEVEL_TRACK_SYSTEM, &bytes,
                                );
                                if let Ok(guard) = self.level_channels.lock() {
                                    for ch in guard.values() {
                                        let _ = ch.send(voice_level_payload(tagged.clone()));
                                    }
                                }
                            }
                            CaptureEvent::Window(SpeechEvent::Window { samples, abs_start, speech_len, elapsed_samples, is_snapshot }) => {
                                let elapsed_ms = (elapsed_samples as u64 * 1000) / TARGET_SAMPLE_RATE as u64;
                                if transcribe_gate {
                                    if let (Some(tx), Some(session)) = (&sys_window_tx, &mut rt_session) {
                                        // 副轨自持配额：本轨是 RTF 探针所在轨，其探测窗是 Red 档
                                        // 唯一的恢复采样来源，绝不能被主轨吃掉（见 [`ProbeBudget`]）。
                                        // 档位同主轨取 `realtime_decode_tier()`（ADR-078 ②）。
                                        let tier = session.realtime_decode_tier();
                                        if !sys_probe.admit(
                                            tier,
                                            is_snapshot,
                                            std::time::Instant::now(),
                                        ) {
                                            session.on_skipped_window(elapsed_ms as f32);
                                            window_skips += 1;
                                            if last_skip_report.elapsed() >= SKIP_REPORT_INTERVAL {
                                                last_skip_report = std::time::Instant::now();
                                                eprintln!(
                                                    "[F4] 卸载负载（副轨）：已跳过 {window_skips} 窗（当前档位 {tier:?}）"
                                                );
                                            }
                                            continue;
                                        }
                                        if tx
                                            .try_send(Some(WindowInput {
                                                samples,
                                                abs_start,
                                                speech_len,
                                            }))
                                            .is_err()
                                        {
                                            session.on_skipped_window(elapsed_ms as f32);
                                            window_drops += 1;
                                            eprintln!("[F4] 副轨 ASR 窗通道满，丢弃 1 窗（abs_start={abs_start}；音频已落盘可二次转写）。");
                                            if window_drops == 1 {
                                                let _ = on_event.send(RecordingEvent::Degraded {
                                                    tier: "yellow".into(),
                                                    reason: "window_drop".into(),
                                                });
                                                publish_recording_health(
                                                    &app_handle,
                                                    "recording_window_dropped",
                                                    "window_drop".into(),
                                                );
                                            }
                                            continue;
                                        }
                                        // RTF 探针在副轨：聚合本流流过的会话时长（与其 infer 同流配对）。
                                        if let Some(deg) = session.on_audio_window(elapsed_ms as f32) {
                                            let tier = format!("{:?}", deg.tier).to_lowercase();
                                            if last_tier.as_deref() != Some(tier.as_str()) {
                                                last_tier = Some(tier.clone());
                                                let _ = on_event.send(RecordingEvent::Degraded {
                                                    tier: tier.clone(),
                                                    reason: "high_rtf".into(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            CaptureEvent::Diagnostic(diag) => {
                                let fatal = matches!(diag,
                                    CaptureDiagnostic::DiskCritical { .. } | CaptureDiagnostic::DeviceLost);
                                // 副轨=系统回环：零样本=系统静音（正常），mic_track 恒 false。
                                handle_capture_diagnostic(&app_handle, &on_event, &diag, false);
                                if fatal {
                                    // 副轨死亡（聚合设备失效/副轨磁盘写失败）：释放平台源与副轨
                                    // 句柄，**降级仅麦克风**继续；主轨落盘/转写不受影响。
                                    system_guard.release();
                                    drop(sys_capture_opt.take());
                                    drop(sys_window_tx.take());
                                    sys_audio_rx = None;
                                    let _ = on_event.send(RecordingEvent::Hypothesis {
                                        text: String::new(),
                                        track: Some(TRACK_REMOTE.into()),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    // 并发轮询 ASR Partial：committed 段 + hypothesis → HUD 实时滚动显示
                    // （PRD-04-010；脱敏边界=日志/通知，见 RecordingEvent 文档）。
                    // 最终段在收尾 drain Finished 时取，避免与此处双计。
                    job_ev = async {
                        match &mut asr_handle {
                            Some(h) => h.recv().await,
                            None => std::future::pending::<Option<JobEvent>>().await,
                        }
                    } => {
                        if let Some(JobEvent::ExecutedBy(info)) = &job_ev {
                            // 服务端回报了执行者：补上名字，并按它的性质校正指示
                            // （降级切回本机时这一条会把指示灭掉）。
                            uplink.name = Some(info.display_name.clone());
                            // 用 `leaves_device` 而**不是** `kind.leaves_deployment()`：
                            // 后者答的是「在服务端那一侧有没有再转给第三方」，服务端把
                            // 自己的 WhisperLive 报成 `local_runtime` 是对的——但拿它当
                            // 「离开这台设备了吗」，指示会在音频正上传时熄灭。
                            // （2026-08-08 真机打出来的。）
                            uplink.per_track[UPLINK_PRIMARY] = info.leaves_device;
                            let _ = on_event.send(uplink.event(transcribe_gate));
                            if let Ok(mut slot) = options.asr_model_name.lock() {
                                *slot = Some(info.model.clone());
                            }
                        }
                        // 重连窗开合：录音与转写都没停，只是云腿在重试（ADR-038 ⑧）。
                        if let Some(active) = transcribe_reconnecting(&job_ev) {
                            let _ = on_event.send(RecordingEvent::TranscribeReconnecting {
                                active, track: primary_track.map(str::to_string),
                            });
                        }
                        // 还在等本机会话额度（ADR-078 ③）：录音与落盘照常，但字幕一个字都
                        // 不会出——这一刻必须说出口，否则用户只看得到「录着音，没有字」。
                        if let Some(waited_ms) = transcribe_admission_delay(&job_ev) {
                            admission_waiting[UPLINK_PRIMARY] = true;
                            eprintln!(
                                "[F4] 实时转写腿已等本机会话额度 {waited_ms}ms 仍未开始\
                                 （录音与落盘不受影响；停录后二次转写补齐）。"
                            );
                            let _ = on_event.send(RecordingEvent::Degraded {
                                tier: "yellow".into(),
                                reason: "admission_wait".into(),
                            });
                            publish_recording_health(
                                &app_handle,
                                "recording_transcribe_waiting",
                                "admission_wait".into(),
                            );
                        }
                        // 额度到手 → 撤掉「还在等」。
                        if admission_waiting[UPLINK_PRIMARY]
                            && matches!(job_ev, Some(JobEvent::Started))
                        {
                            admission_waiting[UPLINK_PRIMARY] = false;
                            last_tier = None;
                            let _ = on_event.send(RecordingEvent::Degraded {
                                tier: "green".into(),
                                reason: "admitted".into(),
                            });
                        }
                        // 转写终态 = 这条轨的转写到此为止，**但录音继续**。
                        //
                        // 必须把句柄置 `None`：Job 终态后事件通道关闭，`recv()` 立即返回
                        // `None` 而不是挂起——不置空的话这条 select! 分支会空转烧满一个核。
                        if let Some(stopped) = transcribe_stopped_code(&job_ev) {
                            if let Some(code) = stopped {
                                let _ = on_event.send(RecordingEvent::TranscribeStopped {
                                    code, track: primary_track.map(str::to_string),
                                });
                            }
                            asr_handle = None;
                            // 停止投窗：往一个没人读的通道里塞只是白占内存。
                            drop(window_tx.take());
                            // **同时注销登记**，别只丢发送端：`std::sync::mpsc` 的缓冲随
                            // **接收端**被丢才释放，而接收端正是滞留在 Provider 登记表里的
                            // 那一半。Job 终态之后 `execute` 再也不会来摘它——只丢发送端的话，
                            // 已经排在通道里的那些窗会一直攥到进程退出（录音还剩几十分钟，
                            // 那份内存一分钟都不会回来）。
                            drop(stream_release.take());
                        }
                        if let Some(JobEvent::Partial(PartialPayload::Asr { committed_delta, hypothesis, .. })) = job_ev {
                            let committed_delta = apply_glossary_to_segments(
                                committed_delta,
                                &options.glossary,
                            );
                            // 脱敏诊断（仅计数/字符长度，无正文）：确认 Partial 回流且产出非空。
                            if asr_debug {
                                eprintln!(
                                    "[F4] 收到 ASR Partial：committed={} 段，hypothesis={} 字符。",
                                    committed_delta.len(),
                                    hypothesis.chars().count()
                                );
                            }
                            // 实时转译（TDD-08）：只喂 committed 段（永不回退 → 译文不抖动）。
                            if translate_gate && !committed_delta.is_empty() {
                                if let Some(p) = &translate_pipeline {
                                    let mut lane_segments = committed_delta.clone();
                                    for segment in &mut lane_segments {
                                        segment.speaker = primary_track.map(str::to_string);
                                    }
                                    p.feed(lane_segments);
                                }
                            }
                            // 旁听者（提词器跟读）只看主轨：committed 与暂定尾一起给。
                            if let Some(observer) = &options.partial_observer {
                                let committed: String = committed_delta
                                    .iter()
                                    .map(|segment| segment.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                observer(&committed, &hypothesis);
                            }
                            // 主、副轨共用同一 Partial 投影出口：committed 与 hypothesis 不可拆开。
                            let (events, absolute_segments) = project_asr_partial(
                                committed_delta,
                                hypothesis,
                                base_elapsed_ms,
                                primary_track,
                            );
                            for event in events {
                                let _ = on_event.send(event);
                            }
                            // 增量累积到会话绝对时钟：停录即时归档用（committed 永不回退 →
                            // 无需在停止时同步 drain 解码积压；未决尾段与权威转写由停录后台
                            // 重转写补齐，PRD-04-030）。轨级 speaker 同样由共享投影写入。
                            all_segments.extend(absolute_segments);
                            // auto 检测回显（PRD-04-046）：Partial 到达时引擎已 observe 锁定，
                            // 读共享句柄比对上次回报，变化才发（显式指定时恒 None 不发）。
                            // auto 检测回显：Both 时以副轨（对方语音）为准，主轨不报。
                            if sys_asr_handle.is_none() {
                                let detected = stream_lang.as_ref().and_then(|l| l.auto_detected());
                                if detected != last_detected_lang {
                                    last_detected_lang = detected.clone();
                                    let _ = on_event.send(RecordingEvent::TranscribeLangDetected { lang: detected });
                                }
                            }
                        }
                    }
                    // 并发轮询**副轨** ASR Partial（Both）：committed 与 hypothesis 都按 remote
                    // 轨投影。低延迟字幕主要来自 hypothesis；丢弃它会造成麦克风静音后系统轨
                    // 虽持续解码、界面却完全无字幕。
                    sys_job_ev = async {
                        match &mut sys_asr_handle {
                            Some(h) => h.recv().await,
                            None => std::future::pending::<Option<JobEvent>>().await,
                        }
                    } => {
                        // 副轨的执行者回报同样要进指示——它是**独立会话**，可以自己降级，
                        // 也可以在主轨已经切回本机之后仍然在上云。
                        if let Some(JobEvent::ExecutedBy(info)) = &sys_job_ev {
                            uplink.per_track[UPLINK_SYSTEM] = info.leaves_device;
                            let _ = on_event.send(uplink.event(transcribe_gate));
                        }
                        if let Some(active) = transcribe_reconnecting(&sys_job_ev) {
                            let _ = on_event.send(RecordingEvent::TranscribeReconnecting {
                                active, track: Some(TRACK_REMOTE.into()),
                            });
                        }
                        if let Some(waited_ms) = transcribe_admission_delay(&sys_job_ev) {
                            admission_waiting[UPLINK_SYSTEM] = true;
                            eprintln!(
                                "[F4] 实时转写腿（副轨）已等本机会话额度 {waited_ms}ms 仍未开始。"
                            );
                            let _ = on_event.send(RecordingEvent::Degraded {
                                tier: "yellow".into(),
                                reason: "admission_wait".into(),
                            });
                            publish_recording_health(
                                &app_handle,
                                "recording_transcribe_waiting",
                                "admission_wait".into(),
                            );
                        }
                        if admission_waiting[UPLINK_SYSTEM]
                            && matches!(sys_job_ev, Some(JobEvent::Started))
                        {
                            admission_waiting[UPLINK_SYSTEM] = false;
                            last_tier = None;
                            let _ = on_event.send(RecordingEvent::Degraded {
                                tier: "green".into(),
                                reason: "admitted".into(),
                            });
                        }
                        if let Some(stopped) = transcribe_stopped_code(&sys_job_ev) {
                            // 这条轨不再外发。
                            uplink.per_track[UPLINK_SYSTEM] = false;
                            let _ = on_event.send(uplink.event(transcribe_gate));
                            if let Some(code) = stopped {
                                let _ = on_event.send(RecordingEvent::TranscribeStopped {
                                    code, track: Some(TRACK_REMOTE.into()),
                                });
                            }
                            sys_asr_handle = None;
                            drop(sys_window_tx.take());
                            // 同主轨：关流之外还要注销登记（缓冲随接收端走）。
                            drop(sys_stream_release.take());
                        }
                        if let Some(JobEvent::Partial(PartialPayload::Asr { committed_delta, hypothesis, .. })) = sys_job_ev {
                            let committed_delta = apply_glossary_to_segments(committed_delta, &options.glossary);
                            if translate_gate && !committed_delta.is_empty() {
                                if let Some(p) = &translate_pipeline {
                                    let mut lane_segments = committed_delta.clone();
                                    for segment in &mut lane_segments {
                                        segment.speaker = Some(TRACK_REMOTE.into());
                                    }
                                    p.feed(lane_segments);
                                }
                            }
                            let (events, absolute_segments) = project_asr_partial(
                                committed_delta,
                                hypothesis,
                                base_elapsed_ms,
                                Some(TRACK_REMOTE),
                            );
                            for event in events {
                                let _ = on_event.send(event);
                            }
                            all_segments.extend(absolute_segments);
                            // auto 检测回显（Both 以副轨为准）。
                            let detected = sys_stream_lang.as_ref().and_then(|l| l.auto_detected());
                            if detected != last_detected_lang {
                                last_detected_lang = detected.clone();
                                let _ = on_event.send(RecordingEvent::TranscribeLangDetected { lang: detected });
                            }
                        }
                    }
                    // 并发接收已译句界单元 → HUD 双语显示 + 暂存待两阶段汇合回填。
                    unit = async {
                        match &mut translate_pipeline {
                            Some(p) => p.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        match unit {
                            Some(luna_jobqueue::TranslationEvent::Unit(unit)) => {
                                forward_translated_unit(&on_event, base_elapsed_ms, &unit);
                                store_translated_unit(&mut translations, unit);
                            }
                            // 结构性失败：一次会话级横幅，之后管线不再提交任何句子
                            // （ADR-083 ⑧）。录音与转写完全正常，故不动会话状态。
                            Some(luna_jobqueue::TranslationEvent::Stopped { code }) => {
                                let _ = on_event.send(RecordingEvent::TranslateStopped {
                                    code: code.to_string(),
                                });
                            }
                            // worker 已收尾（不应在 part 进行中发生）：置 None 防 select 忙轮询。
                            None => translate_pipeline = None,
                        }
                    }
                }
            }
            // ── 内层采集循环结束 ─────────────────────────────────────────────

            // ── 收尾本采集会话（PRD-04-030 瞬时停止）──────────────────────────
            // 1. 停采集：**先停平台源**（tap 不再灌帧，TDD-02 §3.1-I 停止顺序契约），
            //    再停 CaptureHandle（flush writer → 真实分片区间表，保住已录音频）。
            system_guard.release();
            mic_vpio_guard.release(); // VPIO 先停（不再灌帧），再停 CaptureHandle（同平台源契约）
                                      // 启动偏差校正（§3.1-I 任务④）：停轨**前**读两轨采集主机锚点，对晚启动轨算头部
                                      // 补零量（16k 样本），令混流两轨落到同一主机时间轴。
            let dual_lead_in = both_track_lead_in(capture_opt.as_ref(), sys_capture_opt.as_ref());
            let intervals = capture_opt.take().map(|h| h.stop()).unwrap_or_default();
            // 2. **即时收尾**：cancel ASR worker 中止在途解码，关流、丢句柄——committed 段已在
            //    Partial 臂增量累积进 all_segments；未决尾段与权威转写交由停录后台重转写补齐
            //    （不再同步 drain 解码积压 → 停止秒退，根治真机「停止后卡很久」）。
            cancel.cancel();
            // 关流与「转写闸门关掉」是同一步（ADR-080 ③）——两条轨一起收，共用一个函数。
            // 副轨的采集侧收尾另有平台源要停，故那一段仍在下面，此处只收腿。
            close_asr_legs(
                &mut window_tx,
                &mut asr_handle,
                &mut stream_release,
                &mut sys_window_tx,
                &mut sys_asr_handle,
                &mut sys_stream_release,
            );
            // drain 桥接通道让桥接线程干净退出。即使未来某个平台源错误持有发送端，控制面
            // 也只做有界等待，绝不再次把停止变成永久悬挂。
            if !drain_capture_events(&mut audio_rx, CAPTURE_EVENT_DRAIN_TIMEOUT).await {
                eprintln!("[F4] 主轨事件桥未在停止时限内关闭，已放弃等待以完成会话收尾。");
            }
            // 副轨收尾（Both）：平台源已 release；停副轨采集、关流、排空桥接。
            let sys_intervals = sys_capture_opt.take().map(|h| h.stop()).unwrap_or_default();
            if let Some(mut rx) = sys_audio_rx.take() {
                if !drain_capture_events(&mut rx, CAPTURE_EVENT_DRAIN_TIMEOUT).await {
                    eprintln!("[F4] 副轨事件桥未在停止时限内关闭，已放弃等待以完成会话收尾。");
                }
            }
            // 3. 关翻译输入并**收割在途译文**（短有界超时，收尾尾句仍能上屏/入保存；停录后语速
            //    积压已停、就近卸载不再触发，短暂等待即可译完保留单元。完整权威双语仍由重转写补齐）。
            if let Some(mut pipeline) = translate_pipeline.take() {
                pipeline.close_input();
                while let Ok(Some(event)) =
                    tokio::time::timeout(STOP_DRAIN_TIMEOUT, pipeline.recv()).await
                {
                    match event {
                        luna_jobqueue::TranslationEvent::Unit(unit) => {
                            forward_translated_unit(&on_event, base_elapsed_ms, &unit);
                            store_translated_unit(&mut translations, unit);
                        }
                        luna_jobqueue::TranslationEvent::Stopped { code } => {
                            let _ = on_event.send(RecordingEvent::TranslateStopped {
                                code: code.to_string(),
                            });
                        }
                    }
                }
            }
            // 4. Both 落盘收口：双轨混流为立体声单文件（L=mic/R=sys；失败回退注册系统轨）。
            let intervals = merge_dual_track_intervals(intervals, sys_intervals, dual_lead_in);
            // 5. 用 writer 真实分片区间注册 media[]（每物理文件一条；时间轴与落盘逐样本对齐）。
            let session_dur_ms = intervals
                .last()
                .map_or(0, |(iv, _)| iv.abs_end * 1000 / TARGET_SAMPLE_RATE as u64);
            for (iv, dual) in &intervals {
                committed_parts.push(part_from_interval(
                    iv,
                    base_elapsed_ms,
                    vault_root.as_deref(),
                    *dual,
                ));
            }
            base_elapsed_ms += session_dur_ms;

            match inner_exit {
                InnerExit::Stop | InnerExit::StreamEnded => {
                    terminal = SessionTerminal::Completed;
                    break 'outer;
                }
                InnerExit::Pause => {
                    // 暂停：通知前端 + 等待 Resume 或 Stop。
                    let state = recording_state(
                        RecordingStatus::Paused,
                        options.purpose,
                        &options.session_id,
                        &options.note_rel,
                        base_elapsed_ms,
                        "recording:status.paused",
                    );
                    self.replace_state(state.clone());
                    let _ = on_event.send(RecordingEvent::State(state));

                    loop {
                        match control_rx.recv().await {
                            Some(RecordingControl::Resume) => {
                                part_idx += 1;
                                let state = recording_state(
                                    RecordingStatus::Recording,
                                    options.purpose,
                                    &options.session_id,
                                    &options.note_rel,
                                    base_elapsed_ms,
                                    "recording:status.recording",
                                );
                                self.replace_state(state.clone());
                                let _ = on_event.send(RecordingEvent::State(state));
                                continue 'outer; // 新 part
                            }
                            Some(RecordingControl::Stop) | None => {
                                terminal = SessionTerminal::Completed;
                                break 'outer;
                            }
                            Some(RecordingControl::Bookmark { abs_ms, note }) => {
                                // 暂停期书签同样收集（时间轴冻结，abs_ms 即暂停时刻）。
                                bookmarks_ms.push(RecordingBookmark { abs_ms, note });
                            }
                            Some(RecordingControl::BookmarkNote { abs_ms, note }) => {
                                attach_bookmark_note(&mut bookmarks_ms, abs_ms, note);
                            }
                            Some(RecordingControl::SetMicrophoneMuted { muted: m }) => {
                                // 暂停期麦克风已释放：仅更新会话状态，resume 重放。
                                microphone_muted = m;
                            }
                            Some(RecordingControl::SetTranscribe { enabled }) => {
                                // 暂停期没有腿可收：内层循环退出时已经收过了（`close_asr_legs`），
                                // resume 的新 part 按此刻这个闸门重新建（ADR-080 ②）。故这里
                                // 只记状态 + 更新指示。
                                transcribe_gate = enabled;
                                let _ = on_event.send(uplink.event(transcribe_gate));
                            }
                            Some(RecordingControl::SetTranslate { enabled }) => {
                                translate_gate = enabled;
                            }
                            Some(RecordingControl::SetTranslateTarget { lang }) => {
                                // 暂停期也允许改目标：热更新配置，resume 后新句即按新目标译。
                                options
                                    .translate_config
                                    .set_foreign_lang(Lang::from(lang.as_str()));
                            }
                            Some(RecordingControl::SetTranscribeLang { lang }) => {
                                // 暂停期切转写语言：更新 lang_hint，resume 的新 part 按新语言
                                //（新 part 重建 ASR 流，共享句柄随流重建，无需热更）。
                                options.lang_hint = options
                                    .capture_source
                                    .effective_lang_hint(lang.as_deref().map(Lang::from));
                                if last_detected_lang.take().is_some() {
                                    let _ = on_event.send(RecordingEvent::TranscribeLangDetected {
                                        lang: None,
                                    });
                                }
                            }
                            Some(RecordingControl::Pause) => {
                                // 已暂停再次暂停 → 忽略
                            }
                        }
                    }
                }
            }
        }
        // ── outer loop 结束 ──────────────────────────────────────────────────

        // 释放麦克风。
        let _ = mic
            .lock()
            .map(|mut arbiter| arbiter.release(MicHolder::Recording));

        // 归档或报告错误。
        match terminal {
            SessionTerminal::CaptureError(code) => {
                let state = RecordingState {
                    status: RecordingStatus::Error,
                    purpose: options.purpose,
                    session_id: Some(options.session_id),
                    note_rel: Some(options.note_rel),
                    elapsed_ms: base_elapsed_ms as f64,
                    message_key: Some("recording:error.capture_failed".into()),
                    message_detail: None,
                };
                self.finish_active(state.clone());
                let _ = on_event.send(RecordingEvent::State(state));
                let _ = on_event.send(RecordingEvent::Failed { code });
            }
            SessionTerminal::Completed
                if matches!(
                    options.purpose,
                    SessionPurpose::Caption | SessionPurpose::Teleprompter
                ) =>
            {
                // 实时字幕与提词器跟读：**不归档、不落盘**（PRD-04-040、ADR-162 ⑤）。收尾仅复位 idle，让字幕面板可再次开始；
                // 不隐藏面板（用户显式打开的常驻字幕窗，由 ✕ 自行关闭）。
                let state = RecordingState::idle();
                self.finish_active(state.clone());
                let _ = on_event.send(RecordingEvent::State(state));
            }
            SessionTerminal::Completed => {
                // 回填已收割的实时译文（按段 id；未译段留 None，由停录后台重转写补齐双语）。
                for s in &mut all_segments {
                    if let Some(translation) = translations.remove(&s.id) {
                        s.translation = Some(translation);
                    }
                }
                // 调用归档回调（TDD-06 §2.1/§2.2 finalize_recording）。
                // 双流 committed 交错到达（Both）：按会话绝对起点排序，归档呈现单一时间线。
                all_segments.sort_by_key(|s| s.start);
                // 外放回声去重（TDD-02 §3.1-I 层②，与权威重转写共用纯函数）：mic 轨拾到
                // 扬声器里对方声音的段（时间重叠 + 文本相似）移除；无 system 轨时 no-op。
                let echo_dropped = luna_core::drop_cross_track_echo(&mut all_segments);
                if echo_dropped > 0 {
                    eprintln!(
                        "[F4] 跨轨回声去重：移除 {echo_dropped} 个 mic 轨回声段（实时归档）。"
                    );
                }
                match (options.finalize_fn)(committed_parts, all_segments, bookmarks_ms) {
                    Ok(archived_rel) => {
                        let state = RecordingState::idle();
                        self.finish_active(state.clone());
                        let _ = on_event.send(RecordingEvent::Archived {
                            note_rel: archived_rel,
                        });
                        let _ = on_event.send(RecordingEvent::State(state));
                        // 停录归档完成 → 回执「已保存 · 打开」停留一会儿再自隐藏常驻 HUD（ADR-137 H5：
                        // 此前归档即收，用户看不到存成了没有）。Rust 侧 hide（前端 JS hide 受 ACL 限制
                        // 无效）；收起前再看一眼——这几秒里用户又开了一场就不收。录音开着「译文独立窗」
                        // 时镜像窗一并收起（避免孤窗，PRD-04-044 录音扩展）。
                        let hide_handle = app_handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(ARCHIVED_RECEIPT_LINGER);
                            use tauri::Manager as _;
                            let still_idle = hide_handle
                                .try_state::<crate::ipc::AppState>()
                                .is_none_or(|state| {
                                    matches!(
                                        state.recording().state().status,
                                        RecordingStatus::Idle
                                    )
                                });
                            if still_idle {
                                crate::hide_recording_hud(&hide_handle);
                                crate::hide_recording_translation_mirror(&hide_handle);
                            }
                        });
                    }
                    Err(e) => {
                        let state = RecordingState {
                            status: RecordingStatus::Error,
                            purpose: options.purpose,
                            session_id: Some(options.session_id),
                            note_rel: Some(options.note_rel),
                            elapsed_ms: base_elapsed_ms as f64,
                            message_key: Some("recording:error.archive_failed".into()),
                            message_detail: None,
                        };
                        self.finish_active(state.clone());
                        let _ = on_event.send(RecordingEvent::State(state));
                        let _ = on_event.send(RecordingEvent::Failed {
                            code: format!("ARCHIVE_FAILED: {e}"),
                        });
                    }
                }
            }
        }
    }
}

// ─── 内部辅助枚举 ─────────────────────────────────────────────────────────────

/// 系统音频平台源守卫（RAII）：macOS 持 CATap 运行句柄（release/drop 即停 tap + 拆聚合
/// 设备）；Windows 的 WASAPI loopback 由 `CaptureHandle` 全权持有，守卫恒为空。
struct SystemSourceGuard {
    /// macOS CATap 运行句柄（麦克风源 / 其余平台恒 `None`）。
    #[cfg(target_os = "macos")]
    tap: Option<crate::platform::macos::system_audio_tap::RunningSystemAudioTap>,
}

impl SystemSourceGuard {
    /// 空守卫（麦克风源 / Windows loopback）。
    fn none() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            tap: None,
        }
    }

    /// 显式停平台源（**先于** `CaptureHandle::stop`，TDD-02 §3.1-I 停止顺序契约）。
    fn release(&mut self) {
        #[cfg(target_os = "macos")]
        drop(self.tap.take());
    }
}

/// Both mic 侧 VPIO 守卫（RAII，ADR-021）：macOS 持运行中的 Voice Processing I/O 句柄
/// （release/drop 即停 AudioUnit）；非 macOS / cpal 回退路径恒为空。
/// 停止顺序契约与平台源同款：part 收尾**先 release 本守卫**（不再灌帧），再停 `CaptureHandle`。
struct MicVpioGuard {
    #[cfg(target_os = "macos")]
    vpio: Option<crate::platform::macos::voice_process_input::RunningVoiceProcessInput>,
}

impl MicVpioGuard {
    fn none() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            vpio: None,
        }
    }

    /// VPIO 是否在场（外放提示抑制等 UX 分支用）。
    fn is_active(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.vpio.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// 显式停 VPIO（先于 `CaptureHandle::stop`）。
    fn release(&mut self) {
        #[cfg(target_os = "macos")]
        drop(self.vpio.take());
    }
}

/// 启动 Both 模式 mic 侧的 **VPIO 采集**（ADR-021 macOS 主力）：16k mono 直出经
/// [`luna_audio::start_external_capture`] 注入管线（与 CATap 同模式，下游零分叉）。
///
/// 非 macOS / 组件失败返回 `Err`（调用方回退 cpal + AEC3）。错误码脱敏（仅 OSStatus）。
fn start_vpio_mic_capture(
    cfg: &CaptureConfig,
    sink: std::sync::mpsc::Sender<CaptureEvent>,
    clock: Option<std::sync::Arc<luna_audio::CaptureClock>>,
) -> Result<(luna_audio::CaptureHandle, MicVpioGuard), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::macos::voice_process_input as vpio;
        let (handle, injector) = luna_audio::start_external_capture(
            cfg,
            sink,
            luna_audio::ExternalSourceSpec {
                sample_rate: vpio::VPIO_SAMPLE_RATE,
                channels: vpio::VPIO_CHANNELS,
            },
            clock,
            None, // VPIO 在场绝不叠加 AEC3（ADR-021）
        )
        .map_err(|e| format!("VPIO_PIPELINE_FAILED: {e}"))?;
        // 启动失败时 handle 随 `?` 丢弃 → CaptureHandle::drop 自行收尾消费线程。
        let running = vpio::start_voice_process_input(injector)
            .map_err(|e| format!("VPIO_START_FAILED: {e}"))?;
        Ok((
            handle,
            MicVpioGuard {
                vpio: Some(running),
            },
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cfg, sink, clock);
        Err("VPIO_UNSUPPORTED".into())
    }
}

/// 启动**系统音频**采集（PRD-04-052 / ADR-019 / TDD-02 §3.1-I）。
///
/// - macOS：CATap 平台源（`system_audio_tap`）→ [`luna_audio::start_external_capture`]
///   外部注入管线；正式签名构建下首次调用触发「系统音频录制」授权弹框。
/// - Windows：cpal WASAPI loopback（`CaptureSource::SystemLoopback` 内建路径）。
/// - 其余平台 / macOS <14.4：`SYSTEM_AUDIO_UNSUPPORTED`。
///
/// 错误返回会话终端码（脱敏，仅错误类别/OSStatus）。
fn start_system_capture(
    cfg: &CaptureConfig,
    sink: std::sync::mpsc::Sender<CaptureEvent>,
    clock: Option<std::sync::Arc<luna_audio::CaptureClock>>,
    aec: Option<luna_audio::AecRole>,
) -> Result<(luna_audio::CaptureHandle, SystemSourceGuard), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::macos::system_audio_tap::{SystemAudioTap, TapError};
        let tap = SystemAudioTap::create().map_err(|e| match e {
            TapError::Unsupported => "SYSTEM_AUDIO_UNSUPPORTED".to_string(),
            other => format!("SYSTEM_AUDIO_FAILED: {other}"),
        })?;
        let fmt = tap.format();
        let (handle, injector) = luna_audio::start_external_capture(
            cfg,
            sink,
            luna_audio::ExternalSourceSpec {
                sample_rate: fmt.sample_rate,
                channels: fmt.channels,
            },
            clock,
            aec,
        )
        .map_err(|e| format!("SYSTEM_AUDIO_FAILED: {e}"))?;
        // tap.start 失败时 handle 随 `?` 丢弃 → CaptureHandle::drop 自行收尾消费线程。
        let running = tap
            .start(injector)
            .map_err(|e| format!("SYSTEM_AUDIO_FAILED: {e}"))?;
        Ok((handle, SystemSourceGuard { tap: Some(running) }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        start_capture(cfg, sink, clock, aec)
            .map(|h| (h, SystemSourceGuard::none()))
            .map_err(|e| match e {
                luna_audio::CaptureError::UnsupportedSource => {
                    "SYSTEM_AUDIO_UNSUPPORTED".to_string()
                }
                other => format!("SYSTEM_AUDIO_FAILED: {other}"),
            })
    }
}

/// 组装 part 落盘文件路径（Both 双轨以后缀区分轨："-mic"/"-sys"；混流产物无后缀）。
fn part_file(
    dir: &Path,
    session_id: &str,
    part_idx: u32,
    suffix: &str,
    format: RecordingFormat,
) -> String {
    dir.join(format!(
        "{session_id}-p{part_idx:02}{suffix}.{}",
        format.extension()
    ))
    .to_string_lossy()
    .into_owned()
}

/// 启动偏差补零量上限（16k 样本 = 10s）：正常启动偏差为毫秒级，此上限仅防病态锚点差
/// （如一轨授权延迟极久）导致混流文件异常膨胀，超限即封顶并记日志（§3.1-I 任务④）。
const MAX_LEAD_IN_SAMPLES: u64 = 10 * TARGET_SAMPLE_RATE as u64;

/// 计算 Both 双轨启动偏差补零量（16k 样本；§3.1-I 任务④）。
///
/// 读两轨采集主机锚点（共享 epoch，`first_nanos` 跨会话可比），对**晚启动轨**头部补零；
/// 任一快照缺失（某轨无真实数据）→ 不补零。返回 `(lead_in_mic, lead_in_sys)`。
fn both_track_lead_in(
    mic: Option<&luna_audio::CaptureHandle>,
    sys: Option<&luna_audio::CaptureHandle>,
) -> (u64, u64) {
    let (Some(m), Some(s)) = (mic, sys) else {
        return (0, 0);
    };
    let (Some(ms), Some(ss)) = (m.clock_snapshot(), s.clock_snapshot()) else {
        return (0, 0);
    };
    let diff_nanos = ms.first_nanos.abs_diff(ss.first_nanos);
    // 纳秒 → 16k 样本（编排层落盘恒 TARGET_SAMPLE_RATE）。
    let lead = (diff_nanos.saturating_mul(u64::from(TARGET_SAMPLE_RATE)) / 1_000_000_000)
        .min(MAX_LEAD_IN_SAMPLES);
    if lead >= MAX_LEAD_IN_SAMPLES {
        eprintln!("[F4] 双轨启动偏差 {diff_nanos}ns 超上限，补零封顶 {MAX_LEAD_IN_SAMPLES} 样本。");
    }
    if ms.first_nanos <= ss.first_nanos {
        (0, lead) // sys 晚启动 → 补 sys（右轨）头部
    } else {
        (lead, 0) // mic 晚启动 → 补 mic（左轨）头部
    }
}

/// Both 双轨 part 落盘收口（TDD-02 §3.1-I 容器选择 = 立体声单文件，`luna_audio::muxer`）。
///
/// `lead_in` = `(mic, sys)` 头部补零量（启动偏差校正，§3.1-I 任务④）。
///
/// - 副轨为空（单源/副轨失败）→ 主轨区间原样返回；
/// - 两轨各恰一分片 → 混流 L=mic/R=sys 为无后缀立体声文件（晚启动轨头部补零对齐），删除两
///   mono，注册混流区间（起点取两轨较早者，帧数取混流实际帧数）；
/// - 分片数不对称（滚动切分，16k mono ≈ 37 小时才触发）或混流失败 → **回退注册系统轨**
///   （会议主体=对方），麦克风轨文件保留在目录不注册（诚实降级，仅结构化日志）。
fn merge_dual_track_intervals(
    mic: Vec<PartInterval>,
    sys: Vec<PartInterval>,
    lead_in: (u64, u64),
) -> Vec<(PartInterval, bool)> {
    if sys.is_empty() {
        return mic.into_iter().map(|iv| (iv, false)).collect();
    }
    if mic.len() == 1 && sys.len() == 1 {
        let m = &mic[0];
        let s = &sys[0];
        let out = std::path::PathBuf::from(m.path.to_string_lossy().replace("-mic.", "."));
        match luna_audio::mux_mono_to_stereo(&m.path, &s.path, &out, lead_in.0, lead_in.1) {
            Ok(frames) => {
                let _ = std::fs::remove_file(&m.path);
                let _ = std::fs::remove_file(&s.path);
                let abs_start = m.abs_start.min(s.abs_start);
                return vec![(
                    PartInterval {
                        part: m.part,
                        abs_start,
                        abs_end: abs_start + frames,
                        path: out,
                    },
                    true, // 双轨立体声（声道即轨），注册时打 tracks 标记
                )];
            }
            Err(e) => {
                eprintln!("[F4] 双轨混流失败（{e}），回退注册系统轨（麦克风轨文件保留不注册）。");
                return sys.into_iter().map(|iv| (iv, false)).collect();
            }
        }
    }
    eprintln!(
        "[F4] 双轨分片数不对称（mic={}, sys={}），回退注册系统轨。",
        mic.len(),
        sys.len()
    );
    sys.into_iter().map(|iv| (iv, false)).collect()
}

#[derive(Debug)]
enum InnerExit {
    Stop,
    Pause,
    StreamEnded,
}

#[derive(Debug)]
enum SessionTerminal {
    Completed,
    CaptureError(String),
}

// ─── 实时转译辅助（TDD-08）───────────────────────────────────────────────────

/// 已译句界单元 → HUD 事件（时间加 part 基准偏移，与 Partial 同一会话绝对时钟）。
fn forward_translated_unit(
    on_event: &Channel<RecordingEvent>,
    base_elapsed_ms: u64,
    unit: &luna_jobqueue::TranslatedUnit,
) {
    let _ = on_event.send(RecordingEvent::Translation {
        start_ms: (base_elapsed_ms + unit.start.as_millis() as u64) as f64,
        end_ms: (base_elapsed_ms + unit.end.as_millis() as u64) as f64,
        text: unit.translation.clone(),
        track: unit.track.clone(),
    });
}

/// 暂存已译单元：整句译文**按各段原文占比摊回到单元内每一段**（PRD-04-034 / TDD-08 §3）。
///
/// 翻译粒度是句界（半句送 LLM 只会得到语无伦次的译文），而回放列表与双语导出的粒度是段
/// ——此前把整句译文只挂首段、其余段留空，用户看到的就是"译文与原文对不上"。
fn store_translated_unit(
    translations: &mut HashMap<SegmentId, String>,
    unit: luna_jobqueue::TranslatedUnit,
) {
    let sources: Vec<&str> = unit.parts.iter().map(|p| p.text.as_str()).collect();
    for (part, piece) in unit.parts.iter().zip(luna_core::distribute_translation(
        &unit.translation,
        &sources,
    )) {
        if let Some(piece) = piece {
            translations.insert(part.id, piece);
        }
    }
}

// ─── 流式收尾辅助 ─────────────────────────────────────────────────────────────

/// drain 流式 Transcribe Job 到终态，返回最终 committed 段（会话 0-based 绝对时间）。
/// drain Job 事件直至终态（流式/批量转写共用）：
/// `Finished(Segments)` → `Ok(segs)`；`Failed`/`Cancelled`/通道早闭 → `Err(code)`。
pub(crate) async fn drain_job_segments(handle: JobHandle) -> Result<Vec<Segment>, String> {
    // 本机路径不会有服务端排位（那是远程转写独有的），故给一个什么都不做的观察者。
    drain_job_segments_with_executor(handle, &|_| {})
        .await
        .map(|(s, _)| s)
}

/// 同上，并带回**实际执行者**（PRD-17-033）。
///
/// 分成两个函数而不是让所有调用点都改签名：绝大多数调用点不关心执行者
/// （它们跑的是本机 Provider，执行者就是 Provider 自己）。只有会被路由到服务端的
/// 那条路需要它——把它做成可选的第二个入口，比让十个调用点各写一个 `_` 更诚实。
pub(crate) async fn drain_job_segments_with_executor(
    mut handle: JobHandle,
    on_queue: crate::pipeline::QueueObserver<'_>,
) -> Result<(Vec<Segment>, Option<luna_core::ExecutorInfo>), String> {
    let mut executor = None;
    let mut queued = false;
    while let Some(ev) = handle.recv().await {
        match ev {
            // 谁真正在跑这次转写。本机 Provider 不发这条，故 `None` = 本机。
            JobEvent::ExecutedBy(info) => {
                // 收到执行者 = 服务端已经选好上游并开跑（`running` 首帧才带它），
                // 那一刻排位就不再成立——留着会让面板一直写「排队第 1 位 · 转写中」。
                if queued {
                    queued = false;
                    on_queue(None);
                }
                executor = Some(info);
            }
            // 服务端排位要**边收边报**（ADR-049 ⑧），不能攒到终态：它的全部价值
            // 就是即时——攒着发等于让用户在排队那两小时里只看到一个转圈。
            JobEvent::RemoteQueued(status) => {
                queued = true;
                on_queue(Some(status));
            }
            // 服务端开跑了（首帧执行者随 `running` 到达）→ 清掉排位。
            // 只在**报过排位之后**清，避免给本机路径无谓地打一次回调。
            JobEvent::Started if queued => {
                queued = false;
                on_queue(None);
            }
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Segments(segs) => Ok((segs, executor)),
                    _ => Ok((Vec::new(), executor)),
                };
            }
            // 出**稳定码**，不出 `Display`（本函数的文档一直是这么写的，实现却不是）。
            //
            // 这条 `Err` 的去向是 `ProcStatus::Failed { code }` → 队列面板/回放内联的
            // `taskFailureText(code)`，那里按码查 i18n 文案。给它一句 Rust 里写死的中文
            // （`JobError` 的 `Display`）有两个后果：英文界面上冒出中文，且**任何码都
            // 匹配不上**，于是每一种转写失败都被显示成同一句中性的「任务未能完成」——
            // 恰好抹掉这些错误本可给出的下一步动作。`RESULT_UNAVAILABLE`（结果取不回来，
            // 该重试一次二次转写）与 `QUEUE_EXPIRED`（没排上，未计费）都是这样被吞掉的。
            //
            // 同一进程里另外两条路（`transcribe_stopped_code`、`fetch_err`）本来就出码，
            // 这里是唯一的例外。
            JobEvent::Failed(e) => return Err(e.code().to_owned()),
            JobEvent::Cancelled => return Err("CANCELLED".into()),
            _ => {} // Started / Partial / Progress 忽略
        }
    }
    Err("JOB_CHANNEL_CLOSED".into())
}

/// 由 writer 分片区间构造 [`RecordingPart`]（绝对路径→Vault 相对；样本→ms + base 偏移）。
fn part_from_interval(
    iv: &PartInterval,
    base_ms: u64,
    vault_root: Option<&Path>,
    dual_track: bool,
) -> RecordingPart {
    let sr = u64::from(TARGET_SAMPLE_RATE);
    let start_ms = base_ms + iv.abs_start * 1000 / sr;
    let end_ms = base_ms + iv.abs_end * 1000 / sr;
    let rel = vault_root
        .and_then(|root| iv.path.strip_prefix(root).ok())
        .unwrap_or(iv.path.as_path());
    RecordingPart {
        // 写进 frontmatter、随库跨平台同步：一律 `/`。Windows 上 `to_string_lossy` 给的是 `\`，
        // 另一台 Mac 会把整串当成一个文件名，录音就断了链（Windows 真机 2026-10-14）。
        path: rel.to_string_lossy().replace('\\', "/"),
        start_ms,
        end_ms,
        duration: Some(
            iv.abs_end.saturating_sub(iv.abs_start) as f64 / f64::from(TARGET_SAMPLE_RATE),
        ),
        model: None,
        lang: None,
        tracks: dual_track.then(|| "mic_system".to_string()),
    }
}

/// 处理采集诊断：磁盘水位类升级为系统健康通知（脱敏）；其余内部诊断不打扰用户。
///
/// `mic_track`：本诊断来自**麦克风轨**（Mic/Both 主轨）——数字零输入（`SilentInput`）
/// 仅此时升级提示（系统回环轨的零 = 系统静音，属正常，绝不误报）。
fn handle_capture_diagnostic(
    app: &tauri::AppHandle,
    on_event: &Channel<RecordingEvent>,
    diag: &CaptureDiagnostic,
    mic_track: bool,
) {
    match diag {
        CaptureDiagnostic::DiskLow { .. } => {
            publish_recording_health(app, "recording_disk_low", "disk_low".into());
        }
        CaptureDiagnostic::DiskCritical { .. } => {
            let _ = on_event.send(RecordingEvent::Failed {
                code: "DISK_FULL_STOPPED".into(),
            });
            publish_recording_health(app, "recording_disk_critical", "disk_critical".into());
        }
        // 落盘初始化失败：**非致命**——会话继续仅实时转写，但需告知用户「本次不落盘」（脱敏，仅档位名）。
        CaptureDiagnostic::SaveUnavailable { .. } => {
            publish_recording_health(app, "recording_save_unavailable", "save_unavailable".into());
        }
        // 设备丢失（拔出/被占/重配失败）：上报健康通知；会话由 run_session 据此优雅收尾保住已采音频。
        // 绝不静默忽略（此前落入 `_` → 会话冻结无反馈、计时/波形停、只存已采部分）。
        CaptureDiagnostic::DeviceLost => {
            publish_recording_health(app, "recording_device_lost", "device_lost".into());
        }
        // 指定输入设备未找到 → 已回退默认设备（非致命，仅提示用户检查设置）。
        CaptureDiagnostic::InputDeviceFallback { .. } => {
            publish_recording_health(
                app,
                "recording_input_device_fallback",
                "input_device_fallback".into(),
            );
        }
        // 系统音频零回调（TCC「系统音频录制」未授权/被撤销的唯一表现形态，TDD-02 §3.1-I
        // S11 口径）：升级健康通知引导授权。**非致命**——会话保持（音频轨静默但计时/落盘
        // 结构完好），用户授权后重新开录即恢复；绝不静默让用户录了个寂寞。
        CaptureDiagnostic::NoCaptureData { .. } => {
            publish_recording_health(
                app,
                "recording_system_audio_no_data",
                "system_audio_no_data".into(),
            );
        }
        // 麦克风数字零输入（有回调、内容恒为精确零 ≥3s）：真实麦克风必有底噪，全零是
        // 「麦克风权限被拒（macOS 给静音帧不报错，dev 重编译后 TCC 失效是典型触发）/
        // 系统输入音量为 0 / 虚拟设备无信号」的指纹——升级健康通知引导检查，绝不让用户
        // 「录了 N 分钟寂寞」（与系统音频 NoCaptureData 同一诚实纪律）。
        // 系统回环轨的零 = 系统静音，属正常，不提示。
        CaptureDiagnostic::SilentInput { .. } if mic_track => {
            // 横幅的主动作直达「录音输入设备」那一行（2026-10-08 C51）：三种成因里能在应用内
            // 就地改的只有设备；暂停 / 结束仍在录音面板上，不在通知里另造一套录音控制。
            publish_recording_health_to(
                app,
                "recording_mic_silent_input",
                "mic_silent".into(),
                NotificationTarget::Settings("recording/recording.input_device".into()),
            );
        }
        // 双轨时钟漂移指标（§6）：现场诊断长会时间线错位/AEC 失效用；纯数值脱敏结构化日志，
        // **不升级用户通知**（校正自动进行，无需打扰）。
        //
        // 轨标识必不可少（理由见 `track_label`：上一轮现场诊断正卡在"判不出坏的是哪条轨"）。
        //
        // **校正上限之外的漂移要吵**：`DRIFT_MAX_CORRECTION_PPM` 是 500ppm，而实测出现过
        // 2,002,000ppm（= 3.00×）——那不是"漂移"，是**声明与设备实际交付对不上**，校正永远
        // 追不上。这一档必须与常规漂移分开说：它不会自愈，且它让这条轨的时间线以 3 倍速前进
        // （实测 90 秒内领先 90 秒），下游按时间线合并的一切都会错位。
        //
        // ⚠ **别顺着"采样率"查**：那个 3.00× 与 48k/16k 数值相同纯属巧合，真实根因是 6ch/2ch
        // ——VPIO 装在默认输出设备上的回声参考流被当成了 tap（ADR-082，已在 `system_audio_tap`
        // 启动前拦掉）。这行注释此前写的"正好是 48k/16k"把上一轮现场诊断带偏了整整一程。
        CaptureDiagnostic::ClockDrift {
            drift_ppm,
            offset_ms,
            corrections,
        } => {
            let track = track_label(mic_track);
            let verdict = if drift_ppm.abs() > RATE_MISMATCH_PPM {
                "  采样率声明与设备交付不符（校正追不上，非漂移）"
            } else {
                ""
            };
            eprintln!(
                "[clock/{track}] drift={drift_ppm:.1}ppm offset={offset_ms:.2}ms \
                 corrections={corrections}{verdict}"
            );
        }
        // **声明 ≠ 交付：不许只打日志就继续**（ADR-082 的第二笔账）。这一档意味着这条轨
        // 的时间线在倍速前进——落盘混流错位、云端按解出的 PCM 秒计费一起倍增，而用户对此
        // 一无所知（真机上它安静地跑了整整 90 秒）。已知根因（VPIO 的 6 声道回声参考流被
        // 当成 tap）已由 `system_audio_tap` 在启动前拦掉；这条守的是**还没被认识的下一个**，
        // 故一次性升级为用户可见的健康通知（脱敏：只有轨标识与两个采样率数值）。
        CaptureDiagnostic::SourceRateMismatch {
            declared_hz,
            measured_hz,
        } => {
            eprintln!(
                "[clock/{}] 声明 {declared_hz}Hz vs 实测交付 {measured_hz:.0}Hz\
                 ——该轨时间线与计量均已倍速，非漂移、校正追不上",
                track_label(mic_track)
            );
            publish_recording_health(
                app,
                "recording_source_rate_mismatch",
                "source_rate_mismatch".into(),
            );
        }
        // 电平类诊断（判不出语音 / 一直判语音 / 帧 RMS 分布）：不升级用户通知，只留 stderr 痕迹。
        // 打印口径收在 `trace_input_level` 一处——三条采集入口共用，见该函数文档。
        CaptureDiagnostic::SuspectedSilence { .. }
        | CaptureDiagnostic::VadSaturated { .. }
        | CaptureDiagnostic::InputLevels { .. } => {
            trace_input_level(track_label(mic_track), diag);
        }
        // 以下两条是内部诊断，**已在别处各自处理完毕**，此处刻意无动作：
        // `RingOverflow` = 环形缓冲丢了转写音频，容量低估的信号，由 luna-audio 侧计数；
        // `RolledToPart` = 落盘滚动切分，属正常归档行为，分片表已记录。
        // 不写 `_ => {}`：新增变体必须让编译器在这里报错，而不是被兜底静默吞掉。
        // `SuspectedSilence` 正是这么丢的——它一直在产出，却整条落进 `_`，于是现场
        // 「麦克风一直判静音」这件事在日志里从来没出现过（本批连同兜底一起拆掉）。
        CaptureDiagnostic::RingOverflow { .. } | CaptureDiagnostic::RolledToPart { .. } => {}
        // `SilentInput` 的非麦克风轨（系统回环的零 = 系统静音，属正常）落在这里。
        // 上面那条带 `if mic_track` 卫式的分支不构成穷尽性覆盖，故必须显式收尾。
        CaptureDiagnostic::SilentInput { .. } => {}
    }
}

/// 轨标识（`mic` / `sys`）——诊断行的第一现场信息。
///
/// Both 双轨同时上报，两条数值交错打进同一份 stdout；不带轨标识就只能靠数值大小猜，
/// 而两条轨坏掉后的下一步动作完全不同（麦克风轨查 VPIO 的格式协商，系统轨查**聚合设备的
/// 交付布局**——不是 tap 的原生格式，那个一直是对的，ADR-082）。上一轮现场诊断就卡在这里。
fn track_label(mic_track: bool) -> &'static str {
    if mic_track {
        "mic"
    } else {
        "sys"
    }
}

/// 采集**电平类**诊断的唯一打印出口（`SuspectedSilence` / `InputLevels`）。
///
/// # 为什么必须收成一处
///
/// 三条采集入口——录音（`ipc::recording`）、实时字幕（`ipc::caption`）、语音输入
/// （`voice_input`）——跑的是同一条 [`luna_audio::ConsumerPipeline`]，但各有各的事件
/// 循环：前两条经 [`RecordingController`] 共用 [`handle_capture_diagnostic`]，语音输入
/// 那条则单独把 `Diagnostic` 翻译成 HUD 文案、**一行 stderr 都不留**。于是「语音输入
/// 听不见」这类现场，唯一能定位的电平数据恰好是缺的那份。
///
/// 复制三份的下场比缺更糟：三处格式各写各的，现场要靠肉眼对齐数值——而肉眼对齐数值
/// 正是本批要消灭的东西。
///
/// 走 `eprintln!` 而非 `tracing`：客户端没有装 `tracing_subscriber`（`RUST_LOG` 不生效），
/// 既有的 `[clock]` / `[F4]` 诊断就是这个形状。
///
/// **零内容**：只有轨标识与 dBFS/毫秒数值，绝不含音频样本或转写文本。
pub(crate) fn trace_input_level(track: &str, diag: &CaptureDiagnostic) {
    match diag {
        // 峰值把两种成因分开：≈ −120dBFS = 这条轨根本没有信号；−60 上下 = 正常底噪
        // （真的没人说话）；明显高于 −50 却仍判静音 = 信号在，但被判成了非语音。
        CaptureDiagnostic::SuspectedSilence {
            elapsed_ms,
            peak_dbfs,
            decision,
        } => {
            // 门限自适应之后，光有峰值答不上「为什么判静音」——底与门限是运行时的量，
            // 从代码里读不出来。三个数一起打：底 = 这条轨此刻的噪声底估计，门限 = 底 + 档位
            // 余量，末帧 = 触发这条诊断的那一帧的 RMS。
            match decision {
                VadDecision::Energy {
                    rms_dbfs,
                    floor_dbfs,
                    threshold_dbfs,
                } => eprintln!(
                    "[F4] {track} 轨连续 {elapsed_ms}ms 判不出语音（峰值 {peak_dbfs:.1}；\
                     底 {floor_dbfs:.1} 门限 {threshold_dbfs:.1} 末帧 {rms_dbfs:.1} dBFS）"
                ),
                VadDecision::Neural { rms_dbfs, .. } => eprintln!(
                    "[F4] {track} 轨连续 {elapsed_ms}ms 判不出语音（Silero；峰值 {peak_dbfs:.1} \
                     末帧 {rms_dbfs:.1} dBFS）"
                ),
            }
        }
        // 反方向的失效：一直判语音 = 噪声底跟踪被锁死，门限已经不起作用。
        // 与上一条**必须分开说**：两者的下一步动作相反（一个查信号被谁消掉了，
        // 一个查是什么持续大信号把底顶到了上夹取）。
        CaptureDiagnostic::VadSaturated { speech_run_ms } => {
            eprintln!(
                "[F4] {track} 轨连续 {speech_run_ms}ms 一路判语音——噪声底跟踪疑似锁死，门限已失效"
            );
        }
        // 判读口径见 `CaptureDiagnostic::InputLevels` 的文档（p10≈噪声底、p90≈语音、
        // 二者之差 = 这条轨的可用 SNR）——此处只负责把数值原样说出来，不做任何判定。
        CaptureDiagnostic::InputLevels {
            window_ms,
            p10_dbfs,
            p50_dbfs,
            p90_dbfs,
            min_dbfs,
        } => {
            eprintln!(
                "[F4] {track} 轨电平分布 p10={p10_dbfs:.1} p50={p50_dbfs:.1} \
                 p90={p90_dbfs:.1} min={min_dbfs:.1} dBFS（窗 {}s）",
                window_ms / 1000
            );
        }
        // 其余诊断各有归属（健康通知 / 落盘 / 时钟），不从这个出口走；此处不兜底静默，
        // 新增变体若属电平语义，编译器不会提醒——故显式列出全部余项。
        CaptureDiagnostic::SilentInput { .. }
        | CaptureDiagnostic::DeviceLost
        | CaptureDiagnostic::RingOverflow { .. }
        | CaptureDiagnostic::DiskLow { .. }
        | CaptureDiagnostic::DiskCritical { .. }
        | CaptureDiagnostic::RolledToPart { .. }
        | CaptureDiagnostic::SaveUnavailable { .. }
        | CaptureDiagnostic::InputDeviceFallback { .. }
        | CaptureDiagnostic::NoCaptureData { .. }
        | CaptureDiagnostic::ClockDrift { .. }
        | CaptureDiagnostic::SourceRateMismatch { .. } => {}
    }
}

/// 外放提示（PRD-04-052 / TDD-02 §3.1-I 外放对策层①）：系统音频采集启动时，默认输出
/// 若是**内建扬声器**（外放）→ 建议佩戴耳机——外放声音会进入麦克风污染「我」轨
/// （层② 转写去重已兜底归属，但耳机才是最佳分轨）。仅会话首个 part 调用（30s dedupe
/// 兜底）；Info 类 → 前台瞬时 toast，绝不打断录音。
fn maybe_publish_speaker_playback_hint(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager as _;
        if crate::platform::macos::system_audio_tap::default_output_is_builtin_speaker() {
            if let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() {
                nc.publish(NotificationEvent::info("recording_speaker_playback"), app);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows：cpal loopback 无输出设备类型口径，暂无外放检测（诚实按平台能力）。
        let _ = app;
    }
}

/// 发系统健康通知（脱敏：仅 i18n key + 档位/水位名，无转写正文，PRD-15）。
fn publish_recording_health(app: &tauri::AppHandle, key: &'static str, payload: String) {
    publish_recording_health_to(app, key, payload, NotificationTarget::Workspace);
}

/// 同 [`publish_recording_health`]，但点击回跳到给定落点。
fn publish_recording_health_to(
    app: &tauri::AppHandle,
    key: &'static str,
    payload: String,
    target: NotificationTarget,
) {
    use tauri::Manager as _;
    if let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() {
        nc.publish(
            NotificationEvent::system_health(key, target, PrivatePayload::new(payload)),
            app,
        );
    }
}

// ─── 纯逻辑辅助（可单元测试，不依赖 IO）──────────────────────────────────────

/// 探测窗间隔（Red 档停喂时每隔此时长放行一个探测窗重测 RTF，供自动恢复；TDD-02 §5.2）。
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_millis(1500);

/// 录音 State 事件发往前端的最小间隔（节流：计时/档位 ~4/s 足够，避免高频 IPC 与解码争抢
/// 前端主线程致波形卡顿；电平/波形另走 Level 二进制通道逐帧驱动，不受此节流）。
pub(crate) const STATE_EMIT_INTERVAL: Duration = Duration::from_millis(250);

/// 停录时收割在途实时译文的每条等待上限（收尾尾句上屏/入保存；完整双语仍由二次转写补齐）。
pub(crate) const STOP_DRAIN_TIMEOUT: Duration = Duration::from_millis(800);
/// 采集已经停止后，事件桥只应排空少量尾事件并立即 EOF；上限防止发送端所有权回归再次
/// 卡死停止控制面。正常路径不会等待满此时限。
pub(crate) const CAPTURE_EVENT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// 有界排空采集事件桥；`true` = 观察到发送端全部释放，`false` = 超时并由调用方继续收尾。
async fn drain_capture_events(
    rx: &mut mpsc::UnboundedReceiver<CaptureEvent>,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, async { while rx.recv().await.is_some() {} })
        .await
        .is_ok()
}

/// 自适应卸载负载决策（TDD-02 §5.2 单一事实源，纯逻辑可单测）：按降级档位决定本窗是否喂入
/// whisper 解码。释放 CPU 使界面/波形不被饥饿冻结，同时保持**可恢复**（Red 靠探测窗重测 RTF）。
///
/// - **Green**：全量解码（快照 + 终窗），完整实时滚动。
/// - **Yellow / Orange**：**跳过增长快照**、只解终窗/强切窗——去掉连续语音 ~3× 快照重解码开销
///   （`degradation.rs` 记载的"便宜杠杆"）；文本仍在 utterance 边界定稿，仅牺牲中途滚动粒度。
/// - **Red**：停喂，但每 [`PROBE_INTERVAL`] 放行一个探测窗——喂养 RTF 采样，负载回落即自动降档恢复
///   （非停到会话结束）。跳过窗对应音频已全量落盘，停录后二次转写补齐权威结果。
#[must_use]
pub fn should_decode_window(
    tier: DegradationState,
    is_snapshot: bool,
    since_last_probe: Duration,
) -> bool {
    match tier {
        DegradationState::Green => true,
        DegradationState::Yellow | DegradationState::Orange => !is_snapshot,
        DegradationState::Red => since_last_probe >= PROBE_INTERVAL,
    }
}

/// 单条 ASR 轨的 Red 档探测配额（**每轨一个，绝不共享**）。
///
/// Both 的 RTF 探针只挂副轨——只有副轨的探测窗会调 `RealtimeSession::on_audio_window`
/// 重测 RTF（infer/audio 必须同流配对）。两轨若共享一个探测计时，先到的主轨窗会吃掉配额、
/// 副轨恒被跳过，RTF 从此拿不到采样，档位永久钉死在 Red，两轨一起失声。配额随轨走即根除
/// 这条跨轨饥饿；判据仍单一收敛在 [`should_decode_window`]（TDD-02 §5.2）。
#[derive(Debug)]
pub(crate) struct ProbeBudget {
    last_probe: std::time::Instant,
}

impl ProbeBudget {
    /// 以「配额已就绪」构造：会话首个窗即便处在 Red 档也能立刻探测。
    pub(crate) fn ready(now: std::time::Instant) -> Self {
        Self {
            last_probe: now.checked_sub(PROBE_INTERVAL).unwrap_or(now),
        }
    }

    /// 本轨是否解码该窗；Red 档放行时**就地消费本轨配额**（其余档位不占配额）。
    pub(crate) fn admit(
        &mut self,
        tier: DegradationState,
        is_snapshot: bool,
        now: std::time::Instant,
    ) -> bool {
        if !should_decode_window(
            tier,
            is_snapshot,
            now.saturating_duration_since(self.last_probe),
        ) {
            return false;
        }
        if tier == DegradationState::Red {
            self.last_probe = now;
        }
        true
    }
}

/// 控制事件是否定向当前活动会话（`None` = 广播 = 总命中；`Some(id)` 与活动 id 比对）。
#[must_use]
pub fn recording_control_targets_active(active_session_id: &str, requested: Option<&str>) -> bool {
    requested.is_none_or(|id| id == active_session_id)
}

/// 是否应将早到的控制事件暂存（无活动会话 + 有 session_id）。
#[must_use]
pub fn recording_should_queue_pending(active_exists: bool, session_id: Option<&str>) -> bool {
    !active_exists && session_id.is_some()
}

/// 构造 MicBusy 状态（区分"录音中拒语音"与"语音中拒录音"两种场景，见 `MicBusy::i18n_key`）。
#[must_use]
pub fn mic_busy_state(options: &RecordingStartOptions, err: &MicBusy) -> RecordingState {
    RecordingState {
        status: RecordingStatus::MicBusy,
        purpose: options.purpose,
        session_id: Some(options.session_id.clone()),
        note_rel: Some(options.note_rel.clone()),
        elapsed_ms: 0.0,
        message_key: Some(err.i18n_key().to_string()),
        message_detail: None,
    }
}

/// 构造带 session/note_rel/elapsed 的录音状态快照（去样板）。
#[must_use]
pub fn recording_state(
    status: RecordingStatus,
    purpose: SessionPurpose,
    session_id: &str,
    note_rel: &str,
    elapsed_ms: u64,
    message_key: &str,
) -> RecordingState {
    RecordingState {
        status,
        purpose,
        session_id: Some(session_id.to_string()),
        note_rel: Some(note_rel.to_string()),
        elapsed_ms: elapsed_ms as f64,
        message_key: Some(message_key.to_string()),
        message_detail: None,
    }
}

/// 联网指示的当前依据（ADR-038 ④ / `PRD-04-060`）。
///
/// 判据是**两个条件的合取**，且两个都会在会话中途变：
/// - `leaves_device`：这条转写链路会不会把音频发出设备。会话开始时取 Provider 自报的
///   位置（不等服务端回话——否则握手到首帧之间会有一段「在录、但没有任何联网提示」）；
///   服务端回报 `executed_by` 后按它校正（降级切回本机时它变 false）。
/// - 转写开关：关掉实时转写时一个字节都不会外发。此时仍点亮指示就是**误报**，而隐私
///   指示的全部价值在于准确——一旦用户发现关掉转写它还亮，真正外发时他也不会看。
///
/// 收成一个类型而不是在四处各写一遍 `&&`：这个判据要在「会话建立」「收到执行者」
/// 「录音中开关转写」「暂停中开关转写」四个地方发出，散着写必然有一处漏掉新条件。
#[derive(Default)]
struct UplinkIndicator {
    /// 执行者展示名；`None` = 服务端还没回报（或本机执行）。
    name: Option<String>,
    /// **每条轨**是否在外发（主轨 / 副轨）。
    ///
    /// 分开记、取**或**，而不是记一个布尔：Both 模式（麦克风 + 系统音频）下两条轨是
    /// **独立的转写会话**，各自可能独立降级。只记「最后一条回报」的话，主轨切回本机
    /// 那一刻指示就灭了——**而副轨仍在把会议对方的语音往外发**。
    per_track: [bool; 2],
}

/// [`UplinkIndicator::per_track`] 的下标。用具名常量而不是裸 0/1——两条轨调换会让指示
/// 反映错的那一条，而那种错不会有任何报错。
const UPLINK_PRIMARY: usize = 0;
const UPLINK_SYSTEM: usize = 1;

impl UplinkIndicator {
    /// 此刻**任一条轨**在外发吗。
    fn leaves_device(&self) -> bool {
        self.per_track.iter().any(|leaves| *leaves)
    }

    fn event(&self, transcribe_gate: bool) -> RecordingEvent {
        RecordingEvent::Executor {
            display_name: self.name.clone(),
            leaves_device: self.leaves_device() && transcribe_gate,
        }
    }
}

/// 这一帧是不是「这条轨的转写到此为止」，以及要不要告诉用户。
///
/// 返回 `None` = 还没结束（`Partial` / `Started` / …，照常走后续处理）。
/// 返回 `Some(None)` = 结束了但**不必打扰用户**：
///   - `Finished`：正常收尾（停录时采集侧关流，本来就该结束）；
///   - `Cancelled`：停录/切轨是**我们自己**取消的，弹一句「转写被取消了」只会让人困惑。
///
/// 返回 `Some(Some(code))` = 结束了且**用户需要知道**——他刚刚可能已经被扣了额度，
/// 或者他按下的隐私开关正在生效，而字幕突然不出了这件事本身解释不了原因。
///
/// `None` 那一支（通道关闭）也算结束：Job 终态后 `recv()` 立即返回 `None`，
/// 不当结束处理的话，那条 `select!` 分支会空转烧满一个核。
/// 这条事件说的是「重连窗开了还是关了」吗（ADR-038 ⑧）。
///
/// 单独一个函数而不是在两条轨的分支里各写一次 `match`：主轨与系统副轨是**两条独立
/// 会话**，可以各自断线、各自恢复。判据写两遍必然分叉成「主轨会显示重连中、副轨不会」。
fn transcribe_reconnecting(event: &Option<JobEvent>) -> Option<bool> {
    match event {
        Some(JobEvent::Reconnecting(phase)) => match phase {
            // 真断连 → 弹「网络不稳」。
            luna_core::ReconnectPhase::Retrying => Some(true),
            // 窗内接上 → 撤横幅。
            luna_core::ReconnectPhase::Resumed => Some(false),
            // 上游正常关流后的**静默重连**：不打扰用户（缺陷 A / ADR-065）——不发事件，
            // 「网络不稳」横幅在链路完全正常时就不会因每 ~2 秒一次的正常关流而几乎常驻。
            luna_core::ReconnectPhase::Resyncing => None,
        },
        _ => None,
    }
}

/// 这条事件说的是「这条流式腿还在等本机准入额度」吗（ADR-078 ③）。
///
/// **等待必须与失败分开说**：`JobEvent::Queued` 此前是哑的——事件臂根本没有它的分支，
/// 于是「实时转写在等一个永远不会来的额度」与「一切正常」在界面上、在 stdout 上、在测试里
/// 都完全无法区分。这是 TDD-16「失败必须说出口」的同族缺口，漏掉的那一半是「等待」。
///
/// 与 `transcribe_reconnecting` 同样单独成函数、同样两条轨共用：主副轨是两条独立会话，
/// 判据写两遍必然分叉。
fn transcribe_admission_delay(event: &Option<JobEvent>) -> Option<u64> {
    match event {
        Some(JobEvent::AdmissionDelayed { waited_ms }) => Some(*waited_ms),
        _ => None,
    }
}

fn transcribe_stopped_code(event: &Option<JobEvent>) -> Option<Option<String>> {
    match event {
        Some(JobEvent::Failed(e)) => Some(Some(e.code().to_owned())),
        Some(JobEvent::Finished(_) | JobEvent::Cancelled) | None => Some(None),
        Some(_) => None,
    }
}

/// 录音事件的脱敏边界断言辅助（隐私铁律验证，见测试）。
///
/// 铁律（AGENTS §安全隐私）约束**日志与系统通知**不含转写正文：
/// - `State`/`Degraded`/`Archived`/`Failed` 不携带正文（仅状态/档位/路径/错误码）——
///   这些变体可能被透传到通知/诊断场景，必须无正文；
/// - `Partial`/`Hypothesis`/`Translation` 携带文本是 PRD-04-010/011 实时显示的产品功能，
///   仅发往用户自己的 HUD 事件通道，**绝不写日志/通知**（`publish_recording_health`
///   只发档位名）。
#[must_use]
pub fn recording_event_is_log_safe(event: &RecordingEvent) -> bool {
    match event {
        RecordingEvent::State(_)
        | RecordingEvent::Degraded { .. }
        | RecordingEvent::Archived { .. }
        | RecordingEvent::Failed { .. }
        // 仅语言码（如 "zh"），无正文。
        | RecordingEvent::TranscribeLangDetected { .. }
        // 仅执行者展示名与"是否离开设备"，无正文。
        | RecordingEvent::Executor { .. }
        // 仅稳定错误码与轨道名，无正文。
        | RecordingEvent::TranscribeStopped { .. }
        // 仅稳定错误码，无正文。
        | RecordingEvent::TranslateStopped { .. }
        // 仅一个布尔与轨道名，无正文。
        | RecordingEvent::TranscribeReconnecting { .. } => true,
        // 含正文变体：不得进入日志/通知路径。
        RecordingEvent::Partial { .. }
        | RecordingEvent::Hypothesis { .. }
        | RecordingEvent::Translation { .. } => false,
    }
}

// ─── 测试 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::MicHolder;
    use luna_core::{LangContext, TargetLangMode};
    use luna_provider::LinguaDetector;

    // ── 自适应卸载负载决策（TDD-02 §5.2）─────────────────────────────────────

    #[test]
    fn green_decodes_all_windows() {
        assert!(should_decode_window(
            DegradationState::Green,
            true,
            Duration::ZERO
        ));
        assert!(should_decode_window(
            DegradationState::Green,
            false,
            Duration::ZERO
        ));
    }

    #[test]
    fn yellow_orange_skip_snapshots_keep_final() {
        for tier in [DegradationState::Yellow, DegradationState::Orange] {
            assert!(
                !should_decode_window(tier, true, Duration::from_secs(10)),
                "{tier:?} 应跳过增长快照卸载负载"
            );
            assert!(
                should_decode_window(tier, false, Duration::ZERO),
                "{tier:?} 仍解终窗（保证 utterance 边界定稿）"
            );
        }
    }

    #[test]
    fn red_only_probes_on_interval() {
        // 未到探测间隔：停喂（无论快照/终窗）。
        assert!(!should_decode_window(
            DegradationState::Red,
            false,
            Duration::ZERO
        ));
        assert!(!should_decode_window(
            DegradationState::Red,
            true,
            PROBE_INTERVAL / 2
        ));
        // 到达探测间隔：放行一个探测窗重测 RTF（自动恢复的唯一采样来源）。
        assert!(should_decode_window(
            DegradationState::Red,
            true,
            PROBE_INTERVAL
        ));
        assert!(should_decode_window(
            DegradationState::Red,
            false,
            PROBE_INTERVAL + Duration::from_millis(1)
        ));
    }

    #[test]
    fn probe_budget_admits_first_window_then_throttles_within_interval() {
        let t0 = std::time::Instant::now();
        let mut budget = ProbeBudget::ready(t0);
        // 首窗即可探测（配额以「已就绪」构造）。
        assert!(budget.admit(DegradationState::Red, true, t0));
        // 同一配额在间隔内不再放行。
        assert!(!budget.admit(DegradationState::Red, true, t0 + PROBE_INTERVAL / 2));
        // 间隔到达再放行。
        assert!(budget.admit(DegradationState::Red, true, t0 + PROBE_INTERVAL));
    }

    #[test]
    fn probe_budget_is_not_consumed_outside_red() {
        let t0 = std::time::Instant::now();
        let mut budget = ProbeBudget::ready(t0);
        // Green/Yellow 放行的终窗不占探测配额（Yellow 跳快照亦然）。
        assert!(budget.admit(DegradationState::Green, true, t0));
        assert!(!budget.admit(DegradationState::Yellow, true, t0));
        assert!(budget.admit(DegradationState::Yellow, false, t0));
        // 配额仍完好：紧接着转入 Red 的首窗依然能探测。
        assert!(budget.admit(DegradationState::Red, true, t0));
    }

    /// Both 双轨：主轨窗**绝不能**吃掉副轨的探测配额——副轨是 RTF 探针所在轨，
    /// 其探测窗是 Red 档唯一的恢复采样来源，被饿死即档位永久钉在 Red、两轨一起失声。
    #[test]
    fn dual_lane_probe_budgets_do_not_starve_each_other() {
        let t0 = std::time::Instant::now();
        let mut primary = ProbeBudget::ready(t0);
        let mut system = ProbeBudget::ready(t0);

        // 主轨窗先到并消费了自己的配额。
        assert!(primary.admit(DegradationState::Red, true, t0));
        // 同一时刻到达的副轨窗仍必须放行（配额随轨走）。
        assert!(
            system.admit(DegradationState::Red, true, t0),
            "副轨探测配额不得被主轨消费"
        );
        // 各自独立节流：两轨都在间隔内被挡下。
        let t1 = t0 + PROBE_INTERVAL / 2;
        assert!(!primary.admit(DegradationState::Red, true, t1));
        assert!(!system.admit(DegradationState::Red, true, t1));
    }

    // ── 辅助 ────────────────────────────────────────────────────────────────

    fn dummy_translate_config() -> TranslationConfig {
        TranslationConfig::new(
            TargetLangMode::SmartBidirectional,
            LangContext {
                native: Lang::from("zh"),
                default_foreign: Lang::from("en"),
            },
            Arc::new(LinguaDetector::new()),
        )
    }

    fn dummy_options(session_id: &str) -> RecordingStartOptions {
        RecordingStartOptions {
            tuning: luna_audio::CaptureTuning::default(),
            vad: None,
            session_id: session_id.to_string(),
            purpose: SessionPurpose::Record,
            note_rel: "Recordings/test.md".to_string(),
            audio_dir: None,
            vault_root: None,
            format: RecordingFormat::Wav,
            lang_hint: None,
            input_device: None,
            capture_source: RecordingCaptureSource::default(),
            transcribe_enabled: true,
            translate_enabled: false,
            translate_config: dummy_translate_config(),
            glossary: Vec::new(),
            asr_provider: None,
            asr_model_name: Arc::new(Mutex::new(None)),
            finalize_fn: Box::new(|_, _, _| Ok("Recordings/test.md".to_string())),
            partial_observer: None,
        }
    }

    // ─── 转写闸门 = 腿的存在性（ADR-080）────────────────────────────────────

    /// 一条**可观测**的假 ASR 腿：记账每次 `register_stream`，并把「采集侧关流」与
    /// 「收到了窗」两件事如实上报。
    ///
    /// 断言的对象是**腿那一侧看到的事实**（关流到没到、窗喂没喂进来），而不是采集侧
    /// 某个布尔——后者恰恰是本缺陷的形状：闸翻了、字节停了，而腿一无所知地连着。
    struct ObservedAsr {
        location: luna_core::ProviderLocation,
        registered: Arc<Mutex<Vec<StreamId>>>,
        closed: mpsc::UnboundedSender<StreamId>,
        windows: mpsc::UnboundedSender<StreamId>,
        entered: mpsc::UnboundedSender<StreamId>,
        /// 真的那一张登记表（ADR-082）。本假腿的 `execute` **不摘**会话——它模拟的是
        /// 「腿建好了、连着，但结果没人要了」，于是表里剩不剩条目，答的正好是
        /// 「回收路径依不依赖 `execute`」。
        registry: luna_core::StreamRegistry<()>,
    }

    struct ObservedAsrEyes {
        registered: Arc<Mutex<Vec<StreamId>>>,
        closed: mpsc::UnboundedReceiver<StreamId>,
        windows: mpsc::UnboundedReceiver<StreamId>,
        entered: mpsc::UnboundedReceiver<StreamId>,
        /// 腿那一侧**此刻还挂着几条登记**（每条都攥着一份窗缓冲）。
        live_registrations: luna_core::StreamRegistry<()>,
    }

    impl ObservedAsrEyes {
        fn registered(&self) -> Vec<StreamId> {
            self.registered.lock().unwrap().clone()
        }
    }

    struct NoLang;

    impl luna_core::StreamLangControl for NoLang {
        fn set_configured(&self, _configured: Option<String>) {}
        fn auto_detected(&self) -> Option<String> {
            None
        }
    }

    impl luna_core::StreamingAsr for ObservedAsr {
        fn register_stream(&self, id: StreamId) -> luna_core::StreamRegistration {
            self.registered.lock().unwrap().push(id);
            let (window_tx, window_rx) = std::sync::mpsc::sync_channel(8);
            let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(8);
            let closed = self.closed.clone();
            let windows = self.windows.clone();
            std::thread::spawn(move || {
                // 关流的两种写法都收在这里（ADR-079 ②）：投 `None` 即 `Ok(None)`，
                // 丢掉发送端即 `Err`——采集侧走的一直是后一种。
                while let Ok(Some(_)) = window_rx.recv() {
                    let _ = windows.send(id);
                    let _ = infer_tx.try_send(1);
                }
                let _ = closed.send(id);
            });
            luna_core::StreamRegistration {
                window_tx,
                infer_rx,
                lang: Arc::new(NoLang),
                release: self.registry.register(id, ()),
            }
        }
    }

    #[async_trait::async_trait]
    impl luna_core::AIProvider for ObservedAsr {
        fn id(&self) -> luna_core::ProviderId {
            luna_core::ProviderId::from("observed-asr")
        }

        fn descriptor(&self) -> luna_core::ProviderDescriptor {
            luna_core::ProviderDescriptor {
                id: self.id(),
                display_name: "Observed ASR".into(),
                model: None,
                location: self.location,
                license_summary: None,
            }
        }

        fn capabilities(&self) -> luna_core::CapabilitySet {
            luna_core::CapabilitySet {
                capabilities: [luna_core::Capability::Transcribe].into_iter().collect(),
                streaming: true,
                location: self.location,
                explicit_only: false,
            }
        }

        async fn health(&self) -> luna_core::ProviderHealth {
            luna_core::ProviderHealth::ready()
        }

        fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
            None
        }

        async fn execute(&self, job: Job, _cancel: CancelToken) -> luna_core::JobEventStream {
            let entered = self.entered.clone();
            let stream_id = match job.kind {
                JobKind::Transcribe {
                    audio: AudioRef::Stream(id),
                    ..
                } => id,
                _ => unreachable!("本假腿只接流式转写"),
            };
            Box::pin(async_stream::stream! {
                yield JobEvent::Started;
                let _ = entered.send(stream_id);
                // 占住会话额度直到被丢弃——真腿在这里跑着 WebSocket。
                std::future::pending::<()>().await;
            })
        }
    }

    /// 装配：一个假 ASR + 一个真 `JobQueue`（准入额度可指定）。
    fn observed_asr_stack(
        capacity: luna_jobqueue::AdmissionCapacity,
    ) -> (
        Arc<dyn luna_core::StreamingAsr>,
        Arc<JobQueue>,
        ObservedAsrEyes,
    ) {
        let (closed_tx, closed) = mpsc::unbounded_channel();
        let (windows_tx, windows) = mpsc::unbounded_channel();
        let (entered_tx, entered) = mpsc::unbounded_channel();
        let registered: Arc<Mutex<Vec<StreamId>>> = Arc::default();
        let provider = Arc::new(ObservedAsr {
            location: luna_core::ProviderLocation::Cloud,
            registered: Arc::clone(&registered),
            closed: closed_tx,
            windows: windows_tx,
            entered: entered_tx,
            registry: luna_core::StreamRegistry::new(),
        });
        let registry = luna_provider::ProviderRegistry::new();
        registry.register(Arc::clone(&provider) as Arc<dyn luna_core::AIProvider>);
        let live_registrations = provider.registry.clone();
        (
            provider as Arc<dyn luna_core::StreamingAsr>,
            Arc::new(JobQueue::new(Arc::new(registry), capacity)),
            ObservedAsrEyes {
                registered,
                closed,
                windows,
                entered,
                live_registrations,
            },
        )
    }

    /// `run_session` 里那组必须分开放的变量（`tokio::select!` 的两条 Job 臂各要一个
    /// `&mut`）。用例按同样的形状持有它们，装/收走的是同一对函数。
    #[derive(Default)]
    struct LegSlots {
        window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
        asr_handle: Option<JobHandle>,
        stream_lang: Option<Arc<dyn luna_core::StreamLangControl>>,
        release: Option<luna_core::StreamRelease>,
        sys_window_tx: Option<std::sync::mpsc::SyncSender<Option<WindowInput>>>,
        sys_asr_handle: Option<JobHandle>,
        sys_stream_lang: Option<Arc<dyn luna_core::StreamLangControl>>,
        sys_release: Option<luna_core::StreamRelease>,
    }

    impl LegSlots {
        fn install(&mut self, legs: AsrLegs, session: Option<&mut RealtimeSession>) {
            install_asr_legs(
                legs,
                &mut self.window_tx,
                &mut self.asr_handle,
                &mut self.stream_lang,
                &mut self.release,
                &mut self.sys_window_tx,
                &mut self.sys_asr_handle,
                &mut self.sys_stream_lang,
                &mut self.sys_release,
                session,
            );
        }

        fn close(&mut self) {
            close_asr_legs(
                &mut self.window_tx,
                &mut self.asr_handle,
                &mut self.release,
                &mut self.sys_window_tx,
                &mut self.sys_asr_handle,
                &mut self.sys_release,
            );
        }
    }

    fn a_window() -> WindowInput {
        WindowInput {
            samples: vec![0.0; 16],
            abs_start: 0,
            speech_len: 16,
        }
    }

    async fn next_soon<T>(rx: &mut mpsc::UnboundedReceiver<T>, what: &str) -> T {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("等了 5 秒也没等到{what}"))
            .expect("通道提前关闭")
    }

    /// ADR-082：收腿的同一步必须把**登记**也还回去。
    ///
    /// 关流只丢发送端，而窗缓冲是随**接收端**走的——那一半住在 Provider 的登记表里，
    /// 由 `execute` 摘走。可这条 Job 有走不到 `execute` 的出口（会话池准入到期即拒、
    /// 路由解析失败），而 `close_asr_legs` 同一步就把句柄丢了：从此没有人在等它的任何
    /// 事件，`execute` 也不会再来摘。不还的话，一条登记攥着满通道的窗一直到 part 收尾，
    /// 而流式 Provider 实例是复用的。
    ///
    /// 判据刻意是「还要不要这条 Job 的结果」而不是「关没关流」：关流恒早于终态，只看
    /// 关流就注销会把 `execute` 手上还没解完的窗连同登记一起撤掉（语音输入松手即关流、
    /// 识别还在跑，正是那个形状）。
    ///
    /// 反向验证（已实跑）：把 `close_asr_legs` 里那两句 `drop(release.take())` 去掉，
    /// 末尾那条断言即红（两条登记原样挂着）。
    #[tokio::test]
    async fn closing_the_gate_gives_the_registrations_back() {
        let (provider, queue, mut eyes) =
            observed_asr_stack(luna_jobqueue::AdmissionCapacity::for_capture_tracks(2));
        let options = dummy_options("gate-close-registry");
        let mut slots = LegSlots::default();

        slots.install(open_asr_legs(&provider, &queue, &options, true, true), None);
        assert_eq!(
            eyes.live_registrations.len(),
            2,
            "双轨该在腿那一侧留下两条登记"
        );
        next_soon(&mut eyes.entered, "主轨腿开跑").await;
        next_soon(&mut eyes.entered, "副轨腿开跑").await;

        slots.close();

        assert!(
            eyes.live_registrations.is_empty(),
            "登记留在腿的表里——它攥着的那份窗缓冲要等到 part 收尾才回来，而 execute 不会再来摘"
        );
    }

    /// ADR-080 ①：关掉「实时转写」= **腿真的收掉**，不是「这一窗不发」。
    ///
    /// 反向验证：把 `close_asr_legs` 改回只翻布尔（留着 `window_tx` 不丢），腿那一侧
    /// 永远等得到下一窗，两条关流都不会到达，本例在 5 秒死线上超时——那正是缺陷形态：
    /// 字节停了、指示灭了，而按连接时长计费的 WebSocket 一直开到停录。
    #[tokio::test]
    async fn closing_the_gate_ends_the_asr_legs() {
        let (provider, queue, mut eyes) =
            observed_asr_stack(luna_jobqueue::AdmissionCapacity::for_capture_tracks(2));
        let options = dummy_options("gate-close");
        let mut slots = LegSlots::default();

        // 双轨（Both）：两条独立会话，闸门一次管两条。
        slots.install(open_asr_legs(&provider, &queue, &options, true, true), None);
        assert_eq!(eyes.registered().len(), 2, "双轨该建两条腿");
        assert!(slots.window_tx.is_some() && slots.sys_window_tx.is_some());
        next_soon(&mut eyes.entered, "主轨腿开跑").await;
        next_soon(&mut eyes.entered, "副轨腿开跑").await;

        slots.close();

        let mut closed = vec![
            next_soon(&mut eyes.closed, "第一条腿的关流").await,
            next_soon(&mut eyes.closed, "第二条腿的关流").await,
        ];
        closed.sort_by_key(luna_core::StreamId::to_string);
        let mut expected = eyes.registered();
        expected.sort_by_key(luna_core::StreamId::to_string);
        assert_eq!(closed, expected, "两条轨的关流都必须真的到达 Provider");
        assert!(
            slots.window_tx.is_none()
                && slots.asr_handle.is_none()
                && slots.sys_window_tx.is_none()
                && slots.sys_asr_handle.is_none(),
            "收腿之后采集侧不得再留任何一端——留着就还会有人往里投窗"
        );
    }

    /// ADR-080 ④ 的「能恢复」那一半：重新打开 = 新建会话，且窗真的喂得进新腿。
    ///
    /// 反向验证：让 `SetTranscribe{true}` 只翻布尔而不建腿（即闸关时留着旧腿的老做法
    /// 反过来），重开后 `window_tx` 仍是 `None`，投窗那一步当场 panic。
    #[tokio::test]
    async fn reopening_the_gate_starts_a_fresh_session_on_both_tracks() {
        let (provider, queue, mut eyes) =
            observed_asr_stack(luna_jobqueue::AdmissionCapacity::for_capture_tracks(2));
        let options = dummy_options("gate-reopen");
        let mut slots = LegSlots::default();

        slots.install(open_asr_legs(&provider, &queue, &options, true, true), None);
        let first = eyes.registered();
        slots.close();
        next_soon(&mut eyes.closed, "关流").await;
        next_soon(&mut eyes.closed, "关流").await;

        slots.install(open_asr_legs(&provider, &queue, &options, true, true), None);
        let all = eyes.registered();
        assert_eq!(
            all.len(),
            4,
            "重开必须建**新**会话，而不是复用已经关掉的那条"
        );
        assert!(
            all[2..].iter().all(|id| !first.contains(id)),
            "新腿必须是新的 StreamId：旧 id 在 Provider 那边已经注销"
        );

        // 字幕能恢复 = 窗真的喂得进新腿（两条轨都要）。
        slots
            .window_tx
            .as_ref()
            .expect("重开后主轨必须有投窗端")
            .try_send(Some(a_window()))
            .expect("新腿的窗通道必须可投");
        slots
            .sys_window_tx
            .as_ref()
            .expect("重开后副轨必须有投窗端")
            .try_send(Some(a_window()))
            .expect("新腿的窗通道必须可投");
        let fed = [
            next_soon(&mut eyes.windows, "主轨新腿收到窗").await,
            next_soon(&mut eyes.windows, "副轨新腿收到窗").await,
        ];
        assert!(
            fed.iter().all(|id| all[2..].contains(id)),
            "窗必须喂到**新**腿上，喂回旧腿等于关流没生效"
        );
    }

    /// ADR-080 ④ 的「如实报错」那一半：会话池被占满时重开**打不开**，而这件事必须走
    /// 既有通路说出口（黄色横幅 → 可行动错误码），不是静默。
    ///
    /// 反向验证：把重开那一步改回「只翻布尔、不建腿」，`expect("闸开必须建腿")` 当场
    /// 红——而那正是「拨回来却什么也没发生」。等待与拒绝这两个信号本身另有守护
    /// （`admission_lanes` 那一组，ADR-078 ③），此处钉的是**重开走的就是那条通路**。
    #[tokio::test(start_paused = true)]
    async fn reopening_into_a_full_session_lane_says_so_instead_of_going_silent() {
        let (provider, queue, mut eyes) = observed_asr_stack(luna_jobqueue::AdmissionCapacity {
            compute: 2,
            session: 1,
        });
        let options = dummy_options("gate-reopen-busy");

        // 唯一的会话额度被另一条腿占着（上一场没收干净 / 另一个采集会话）。
        let _holder = open_asr_legs(&provider, &queue, &options, false, true);
        next_soon(&mut eyes.entered, "占位腿开跑").await;

        // 用户把「实时转写」拨回来 → 新建一条腿，进不去。
        let mut reopened = open_asr_legs(&provider, &queue, &options, false, true);
        let mut handle = reopened.asr_handle.take().expect("闸开必须建腿");
        assert_eq!(
            handle.recv().await,
            Some(JobEvent::Queued),
            "提交回执恒发一次"
        );

        let waiting =
            tokio::time::timeout(luna_jobqueue::STREAM_ADMISSION_LIMIT * 60, handle.recv())
                .await
                .expect("等待信号整条没接上");
        assert!(
            transcribe_admission_delay(&waiting).is_some(),
            "重开进不去的第一步必须是「还在等本机会话额度」，实际是 {waiting:?}"
        );

        let terminal =
            tokio::time::timeout(luna_jobqueue::STREAM_ADMISSION_LIMIT * 60, handle.recv())
                .await
                .expect("放弃信号整条没接上");
        assert_eq!(
            transcribe_stopped_code(&terminal),
            Some(Some("LOCAL_SESSION_BUSY".into())),
            "打不开就要给可行动的稳定错误码（PRD-04-061），而不是一条没有字幕的静默录音"
        );
    }

    /// ADR-080 ②：在设置里关掉实时转写的用户，录音期间**一条腿都不该建**。
    ///
    /// 反向验证：把 `open_asr_legs` 里那个提前返回删掉（回到「只看有没有 Provider」的
    /// 老做法），`register_stream` 当场发生，本例红——而真机上它的形态是一条从不上行、
    /// 却按连接时长计费到停录的 WebSocket。pause→resume 的每个新 part 各来一条。
    #[tokio::test]
    async fn a_closed_gate_builds_no_leg_at_all() {
        let (provider, queue, eyes) =
            observed_asr_stack(luna_jobqueue::AdmissionCapacity::for_capture_tracks(2));
        let options = dummy_options("gate-closed-at-start");

        let legs = open_asr_legs(&provider, &queue, &options, true, false);

        assert!(
            eyes.registered().is_empty(),
            "闸关时一次 register_stream 都不该发生——实时腿是建连即连，不等第一个窗"
        );
        assert!(
            legs.window_tx.is_none()
                && legs.asr_handle.is_none()
                && legs.sys_window_tx.is_none()
                && legs.sys_asr_handle.is_none()
                && legs.infer_rx.is_none(),
            "闸关 = 一条腿都没有"
        );
    }

    #[test]
    fn recording_segments_apply_personal_glossary_before_archive() {
        let seg = Segment {
            id: SegmentId::new(),
            start: Duration::ZERO,
            end: Duration::from_millis(500),
            text: "hello luna".into(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        };
        let corrected = apply_glossary_to_segments(
            vec![seg],
            &[GlossaryEntry {
                term: "luna".into(),
                canonical: "Lunarca".into(),
            }],
        );
        assert_eq!(corrected[0].text, "hello Lunarca");
    }

    // ── 分片区间 → RecordingPart（M1：落盘时间轴对齐的命门，纯逻辑可测）──────

    #[test]
    fn part_from_interval_maps_samples_to_ms_and_relativizes_path() {
        let iv = PartInterval {
            part: 0,
            abs_start: 16_000, // 1s @16k
            abs_end: 48_000,   // 3s
            path: PathBuf::from("/vault/Recordings/sess-p00.wav"),
        };
        // base_elapsed 5s（前一 pause-part 累计）。
        let part = part_from_interval(&iv, 5_000, Some(Path::new("/vault")), false);
        assert_eq!(
            part.path, "Recordings/sess-p00.wav",
            "应换算为 Vault 相对路径"
        );
        assert_eq!(part.start_ms, 6_000, "base 5s + 区间起 1s");
        assert_eq!(part.end_ms, 8_000, "base 5s + 区间止 3s");
        assert_eq!(part.duration, Some(2.0), "(48000-16000)/16000 = 2s");
    }

    /// 平台拼出来的路径（Windows 上是 `\`）写进 frontmatter 时一律 `/`：库会同步到另一台机器。
    #[test]
    fn part_from_interval_writes_forward_slashes_on_every_platform() {
        let root = Path::new("/vault");
        let iv = PartInterval {
            part: 0,
            abs_start: 0,
            abs_end: 16_000,
            path: root.join("Recordings").join("assets").join("sess-p00.wav"),
        };
        let part = part_from_interval(&iv, 0, Some(root), false);
        assert_eq!(part.path, "Recordings/assets/sess-p00.wav");
    }

    #[test]
    fn part_from_interval_without_vault_root_keeps_path() {
        let iv = PartInterval {
            part: 1,
            abs_start: 0,
            abs_end: 16_000,
            path: PathBuf::from("/x/a.part1.wav"),
        };
        let part = part_from_interval(&iv, 0, None, true);
        assert_eq!(part.path, "/x/a.part1.wav");
        assert_eq!(part.end_ms, 1_000);
    }

    // ── 采集源设置映射（PRD-04-052）────────────────────────────────────────────

    #[test]
    fn capture_source_setting_maps_with_task_specific_defaults() {
        assert_eq!(
            RecordingCaptureSource::from_setting(
                Some("system_loopback"),
                RecordingCaptureSource::Mic,
            ),
            RecordingCaptureSource::SystemAudio
        );
        assert_eq!(
            RecordingCaptureSource::from_setting(
                Some("input_device"),
                RecordingCaptureSource::Both,
            ),
            RecordingCaptureSource::Mic
        );
        assert_eq!(
            RecordingCaptureSource::from_setting(Some("garbage"), RecordingCaptureSource::Mic,),
            RecordingCaptureSource::Mic
        );
        assert_eq!(
            RecordingCaptureSource::from_setting(None, RecordingCaptureSource::Both),
            RecordingCaptureSource::Both,
            "实时字幕缺省值应保持 Both"
        );
        assert_eq!(
            RecordingCaptureSource::from_setting(Some("both"), RecordingCaptureSource::Mic),
            RecordingCaptureSource::Both
        );
        assert!(RecordingCaptureSource::Mic.uses_microphone());
        assert!(RecordingCaptureSource::Both.uses_microphone());
        assert!(!RecordingCaptureSource::SystemAudio.uses_microphone());
    }

    /// **会话池的额度必须跟着采集源枚举走，不能是一个写死的数字**（ADR-078 ①）。
    ///
    /// 这条同时钉住三件事：`ALL` 覆盖了全部变体（漏一个 → 轨数上限算小 → 新采集源的
    /// 最后一条轨永远拿不到额度）、Both 确实是两条流、以及生产注入的会话池容量恰好
    /// 装得下「轨数上限 + 一条实时转译」。缺任何一条，缺陷都会以「某个档位下字幕不出」
    /// 的形式回来，而不会有任何东西变红。
    #[test]
    fn the_session_lane_is_sized_from_the_capture_source_enum_not_a_hardcoded_number() {
        // 穷尽（无 `_` 臂）：加变体即编译失败，逼着人把它加进 `ALL`。
        for source in RecordingCaptureSource::ALL {
            let expected = match source {
                RecordingCaptureSource::Mic | RecordingCaptureSource::SystemAudio => 1,
                // Both = 麦克风 + 系统回环两条独立流式 ASR Job。
                RecordingCaptureSource::Both => 2,
            };
            assert_eq!(source.track_count(), expected, "{source:?} 的轨数不对");
        }
        assert_eq!(
            RecordingCaptureSource::ALL.len(),
            3,
            "新增采集源必须同步进 ALL，否则轨数上限会算小"
        );

        let max_tracks = RecordingCaptureSource::max_track_count();
        assert_eq!(max_tracks, 2);
        let capacity = luna_jobqueue::AdmissionCapacity::for_capture_tracks(max_tracks);
        assert_eq!(
            capacity.session,
            max_tracks + 1,
            "会话池 = 轨数上限 + 1 条实时转译（转译 worker 串行，任一时刻最多一条在飞）"
        );
    }

    #[test]
    fn dual_source_never_reuses_one_language_hint_for_both_tracks() {
        let configured = Some(Lang::from("zh"));
        assert_eq!(
            RecordingCaptureSource::Both.effective_lang_hint(configured.clone()),
            None
        );
        assert_eq!(
            RecordingCaptureSource::Mic.effective_lang_hint(configured.clone()),
            configured
        );
        assert_eq!(
            RecordingCaptureSource::SystemAudio.effective_lang_hint(Some(Lang::from("en"))),
            Some(Lang::from("en"))
        );
    }

    // ── Both 双轨落盘收口（纯逻辑分支；混流本体由 luna-audio muxer 单测覆盖）──────

    #[test]
    fn merge_dual_track_returns_mic_when_no_sys() {
        let mic = vec![PartInterval {
            part: 0,
            abs_start: 0,
            abs_end: 16_000,
            path: PathBuf::from("/v/a-p00.wav"),
        }];
        let merged = merge_dual_track_intervals(mic.clone(), Vec::new(), (0, 0));
        assert_eq!(merged, vec![(mic[0].clone(), false)]);
    }

    #[test]
    fn merge_dual_track_falls_back_to_sys_on_asymmetric_parts() {
        let mic = vec![
            PartInterval {
                part: 0,
                abs_start: 0,
                abs_end: 1,
                path: PathBuf::from("/v/a-p00-mic.wav"),
            },
            PartInterval {
                part: 1,
                abs_start: 1,
                abs_end: 2,
                path: PathBuf::from("/v/a-p00-mic-2.wav"),
            },
        ];
        let sys = vec![PartInterval {
            part: 0,
            abs_start: 0,
            abs_end: 2,
            path: PathBuf::from("/v/a-p00-sys.wav"),
        }];
        let merged = merge_dual_track_intervals(mic, sys.clone(), (0, 0));
        assert_eq!(
            merged,
            vec![(sys[0].clone(), false)],
            "分片不对称应回退注册系统轨"
        );
    }

    #[test]
    fn merge_dual_track_falls_back_to_sys_when_mux_fails() {
        // 文件不存在 → 混流必失败 → 回退系统轨。
        let mic = vec![PartInterval {
            part: 0,
            abs_start: 0,
            abs_end: 1,
            path: PathBuf::from("/nonexistent/a-p00-mic.wav"),
        }];
        let sys = vec![PartInterval {
            part: 0,
            abs_start: 0,
            abs_end: 1,
            path: PathBuf::from("/nonexistent/a-p00-sys.wav"),
        }];
        let merged = merge_dual_track_intervals(mic, sys.clone(), (0, 0));
        assert_eq!(merged, vec![(sys[0].clone(), false)]);
    }

    // ── 纯逻辑：控制定向 ───────────────────────────────────────────────────

    #[test]
    fn control_targets_active_when_session_id_matches() {
        assert!(recording_control_targets_active("sess-1", Some("sess-1")));
    }

    #[test]
    fn control_targets_active_when_no_session_id_given() {
        // None = 广播，总命中。
        assert!(recording_control_targets_active("sess-1", None));
    }

    #[test]
    fn control_does_not_target_active_when_session_id_mismatches() {
        assert!(!recording_control_targets_active("sess-1", Some("sess-2")));
    }

    // ── 纯逻辑：早到控制事件 ──────────────────────────────────────────────

    #[test]
    fn pending_control_queued_when_no_active_and_session_id_present() {
        assert!(recording_should_queue_pending(false, Some("sess-1")));
    }

    #[test]
    fn pending_control_not_queued_when_active_exists() {
        assert!(!recording_should_queue_pending(true, Some("sess-1")));
    }

    #[test]
    fn pending_control_not_queued_without_session_id() {
        assert!(!recording_should_queue_pending(false, None));
    }

    // ── 纯逻辑：麦克风互斥决策 ────────────────────────────────────────────

    #[test]
    fn mic_busy_state_carries_i18n_key_not_user_content() {
        use crate::arbiter::MicBusy;
        let err = MicBusy {
            held_by: MicHolder::Recording,
            requested_by: MicHolder::VoiceInput,
        };
        let opts = dummy_options("s1");
        let state = mic_busy_state(&opts, &err);
        assert_eq!(state.status, RecordingStatus::MicBusy);
        let key = state.message_key.unwrap();
        // i18n key 不含用户正文（key 为点分路径，不含中文文案）。
        assert!(!key.is_empty(), "message_key 不应为空");
        assert!(!key.contains('，'), "i18n key 不应含中文标点（非句子）");
    }

    #[test]
    fn recording_busy_rejects_voice_input_with_correct_key() {
        use crate::arbiter::MicBusy;
        // 录音持有 → 语音输入请求 → "录音进行中"引导。
        let err = MicBusy {
            held_by: MicHolder::Recording,
            requested_by: MicHolder::VoiceInput,
        };
        let key = err.i18n_key();
        assert_eq!(key, "mic.busy.recording_active");
    }

    #[test]
    fn voice_input_busy_rejects_recording_with_correct_key() {
        use crate::arbiter::MicBusy;
        // 语音输入持有 → 录音请求 → "capture_active" i18n key。
        let err = MicBusy {
            held_by: MicHolder::VoiceInput,
            requested_by: MicHolder::Recording,
        };
        let key = err.i18n_key();
        // 非 Recording 持有者一律走 "capture_active"（见 MicBusy::i18n_key）。
        assert_eq!(key, "mic.busy.capture_active");
    }

    // ── 脱敏边界：日志/通知可达变体不含正文；显示载体变体明确隔离 ─────────

    #[test]
    fn text_bearing_events_are_marked_not_log_safe() {
        // Partial/Hypothesis/Translation 携带正文（PRD-04-010/011 实时显示），
        // 必须被标记为不可入日志/通知。
        assert!(!recording_event_is_log_safe(&RecordingEvent::Partial {
            start_ms: 0.0,
            end_ms: 1000.0,
            text: "示例转写".into(),
            track: None,
        }));
        assert!(!recording_event_is_log_safe(&RecordingEvent::Hypothesis {
            text: "暂定文本".into(),
            track: Some(TRACK_REMOTE.into()),
        }));
        assert!(!recording_event_is_log_safe(&RecordingEvent::Translation {
            start_ms: 0.0,
            end_ms: 1000.0,
            text: "translated text".into(),
            track: Some(TRACK_REMOTE.into()),
        }));
    }

    #[test]
    fn hypothesis_ipc_contract_keeps_the_system_track_identity() {
        let value = serde_json::to_value(RecordingEvent::Hypothesis {
            text: "系统声音暂定".into(),
            track: Some(TRACK_REMOTE.into()),
        })
        .expect("RecordingEvent 应可序列化");
        assert_eq!(value["kind"], "hypothesis");
        assert_eq!(value["data"]["track"], "remote");
        assert_eq!(value["data"]["text"], "系统声音暂定");
    }

    #[test]
    fn system_lane_first_window_projects_hypothesis_before_any_commit() {
        let (events, absolute_segments) =
            project_asr_partial(Vec::new(), "系统声音首窗暂定".into(), 0, Some(TRACK_REMOTE));
        assert!(absolute_segments.is_empty());
        assert!(matches!(
            events.as_slice(),
            [RecordingEvent::Hypothesis { text, track }]
                if text == "系统声音首窗暂定" && track.as_deref() == Some(TRACK_REMOTE)
        ));
    }

    #[test]
    fn system_lane_commit_and_clear_keep_one_track_identity() {
        let segment = Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(250),
            end: Duration::from_millis(750),
            text: "系统声音定稿".into(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        };
        let (events, absolute_segments) =
            project_asr_partial(vec![segment], String::new(), 1_000, Some(TRACK_REMOTE));

        assert!(matches!(
            events.as_slice(),
            [
                RecordingEvent::Partial { start_ms, end_ms, text, track },
                RecordingEvent::Hypothesis { text: hypothesis, track: hypothesis_track },
            ] if *start_ms == 1_250.0
                && *end_ms == 1_750.0
                && text == "系统声音定稿"
                && track.as_deref() == Some(TRACK_REMOTE)
                && hypothesis.is_empty()
                && hypothesis_track.as_deref() == Some(TRACK_REMOTE)
        ));
        assert_eq!(absolute_segments[0].start, Duration::from_millis(1_250));
        assert_eq!(absolute_segments[0].end, Duration::from_millis(1_750));
        assert_eq!(absolute_segments[0].speaker.as_deref(), Some(TRACK_REMOTE));
    }

    // ── 实时转译：译文摊回单元内每一段 ────────────────────────────────────

    #[test]
    fn translated_unit_is_spread_across_all_segments_of_the_unit() {
        let first = SegmentId::new();
        let second = SegmentId::new();
        let mut translations = HashMap::new();
        store_translated_unit(
            &mut translations,
            luna_jobqueue::TranslatedUnit {
                parts: vec![
                    luna_jobqueue::UnitPart {
                        id: first,
                        text: "你好，".into(),
                    },
                    luna_jobqueue::UnitPart {
                        id: second,
                        text: "世界。".into(),
                    },
                ],
                start: Duration::ZERO,
                end: Duration::from_millis(2000),
                translation: "Hello, world.".into(),
                track: Some(TRACK_REMOTE.into()),
            },
        );
        // 一句译文摊回两段（PRD-04-034）：**不再**只挂首段让第二段空着——那正是真机
        // 「译文与原文对不上」的成因。
        assert_eq!(translations.get(&first).map(String::as_str), Some("Hello,"));
        assert_eq!(
            translations.get(&second).map(String::as_str),
            Some("world.")
        );
    }

    /// 联网指示只在**真的在外发**时点亮：会外发 **且** 转写开着。
    ///
    /// 少了后半个条件就是误报：用户关掉实时转写后一个字节都不会外发，而指示照亮。
    /// 方向上「偏安全」并不能替它开脱——隐私指示的全部价值在于准确，一旦用户发现
    /// 关掉转写它还亮，真正外发的时候他也不会再看它。
    ///
    /// 同时钉住名字不会在开关翻转时被抹掉（那会让指示重新亮起时说不出「去了哪儿」）。
    #[test]
    fn the_uplink_indicator_is_on_only_while_audio_actually_leaves() {
        let local = UplinkIndicator {
            name: Some("本地 Whisper".into()),
            per_track: [false, false],
        };
        let cloud = UplinkIndicator {
            name: Some("组织服务端".into()),
            per_track: [true, false],
        };

        for gate in [true, false] {
            assert!(
                matches!(
                    local.event(gate),
                    RecordingEvent::Executor {
                        leaves_device: false,
                        ..
                    }
                ),
                "本机执行时不该点亮联网指示（转写开关={gate}）"
            );
        }
        assert!(
            matches!(
                cloud.event(true),
                RecordingEvent::Executor {
                    leaves_device: true,
                    ..
                }
            ),
            "云端执行且转写开着——必须点亮"
        );
        assert!(
            matches!(
                cloud.event(false),
                RecordingEvent::Executor {
                    leaves_device: false,
                    ..
                }
            ),
            "关掉实时转写后一个字节都不会外发，指示必须跟着灭"
        );
        // Both 模式：两条轨是**独立会话**，各自可能独立降级。主轨切回本机时若指示
        // 跟着灭，副轨仍在把会议对方的语音往外发——判据必须是两轨的**或**。
        let mut both = UplinkIndicator {
            name: Some("组织服务端".into()),
            per_track: [true, true],
        };
        both.per_track[UPLINK_PRIMARY] = false; // 主轨降级回本机
        assert!(
            both.leaves_device(),
            "主轨降级了但副轨还在上云——指示不得熄灭"
        );
        both.per_track[UPLINK_SYSTEM] = false; // 副轨也降了
        assert!(!both.leaves_device(), "两条轨都不再外发时指示才该灭");

        // 名字要留着：开关翻转后重新点亮时，用户仍要能看出「去了哪儿」。
        let RecordingEvent::Executor { display_name, .. } = cloud.event(false) else {
            panic!("应是 Executor 事件");
        };
        assert_eq!(display_name.as_deref(), Some("组织服务端"));
    }

    /// 转写终态的判据：哪些要打扰用户、哪些不要、哪些根本不是终态。
    ///
    /// 三件事一起钉住：
    /// ① `Failed` 必须带着**错误码**告诉用户——他刚刚可能已经被扣了额度，而「字幕
    ///    突然不出了」这件事本身解释不了原因；
    /// ② `Finished` / `Cancelled` 是我们自己收的尾（停录、切轨），弹提示只会让人困惑；
    /// ③ `None`（事件通道关闭）**也算终态**——Job 终态后 `recv()` 立即返回 `None`
    ///    而不是挂起，判成「还没结束」的话那条 `select!` 分支会空转烧满一个核。
    ///    云通道的额度耗尽/断网是常态，这条不是假想。
    #[test]
    fn terminal_asr_events_are_told_apart() {
        use luna_core::{JobError, ProviderId};

        let quota = Some(JobEvent::Failed(JobError::QuotaExhausted {
            dimension: Some("asr_seconds".into()),
        }));
        assert_eq!(
            transcribe_stopped_code(&quota),
            Some(Some("QUOTA_EXHAUSTED".to_owned())),
            "额度耗尽必须带码上报——用户要能看出该去加量而不是重试"
        );
        assert_eq!(
            transcribe_stopped_code(&Some(JobEvent::Failed(JobError::StrictLocalBlocked {
                capability: luna_core::Capability::Transcribe,
            }))),
            Some(Some("STRICT_LOCAL_BLOCKED".to_owned()))
        );

        // 我们自己收的尾：算终态（要置空句柄止住空转），但不打扰用户。
        assert_eq!(
            transcribe_stopped_code(&Some(JobEvent::Cancelled)),
            Some(None)
        );
        assert_eq!(
            transcribe_stopped_code(&None),
            Some(None),
            "通道关闭也是终态"
        );

        // 还在跑的帧不能被当成终态，否则转写会在第一帧就被掐掉。
        assert_eq!(transcribe_stopped_code(&Some(JobEvent::Started)), None);
        assert_eq!(
            transcribe_stopped_code(&Some(JobEvent::ExecutedBy(luna_core::ExecutorInfo {
                display_name: "x".into(),
                kind: luna_core::ExecutorKind::LocalRuntime,
                model: "m".into(),
                leaves_device: false,
            }))),
            None
        );
        let _ = ProviderId::from("x");
    }

    /// **「还在等」必须与「已经失败」分成两条判据**（ADR-078 ③，回归防哑）。
    ///
    /// `JobEvent::Queued` 此前在这条事件臂上**根本没有分支**——不打日志、不发 UI 事件、
    /// 无超时，于是「实时转写在等一个永远不会来的额度」与「一切正常」完全无法区分。
    /// 这条钉住：等待事件被识别成等待（不是终态、不掐句柄），而等满宽限期的拒绝走的是
    /// 既有的 `transcribe_stopped` 通路、带可行动的稳定错误码。
    #[test]
    fn waiting_for_admission_is_reported_and_is_not_mistaken_for_a_terminal_state() {
        let waiting = Some(JobEvent::AdmissionDelayed { waited_ms: 12_000 });
        assert_eq!(
            transcribe_admission_delay(&waiting),
            Some(12_000),
            "等待超阈值必须被识别出来——这正是此前完全静默的那一环"
        );
        assert_eq!(
            transcribe_stopped_code(&waiting),
            None,
            "等待不是终态：当成终态会把句柄掐掉，这条腿就再也接不上了"
        );

        // 正常帧不得被误读成「在等」，否则横幅会常驻。
        for normal in [
            Some(JobEvent::Queued),
            Some(JobEvent::Started),
            Some(JobEvent::Progress(0.5)),
            None,
        ] {
            assert_eq!(transcribe_admission_delay(&normal), None, "{normal:?}");
        }

        // 等满宽限期的放弃：走既有的「转写停了、录音继续」通路，码是可行动的。
        assert_eq!(
            transcribe_stopped_code(&Some(JobEvent::Failed(
                luna_core::JobError::LocalSessionBusy { waited_ms: 20_000 }
            ))),
            Some(Some("LOCAL_SESSION_BUSY".into()))
        );
    }

    /// 「转写停了」是 log-safe 的（只有错误码与轨道名），且**不是** `Failed`。
    ///
    /// 归进 `Failed` 会把整个录音置为 error——而录音完全正常，音频照常落盘。
    #[test]
    fn transcribe_stopped_is_not_a_recording_failure() {
        let event = RecordingEvent::TranscribeStopped {
            code: "QUOTA_EXHAUSTED".into(),
            track: Some(TRACK_REMOTE.into()),
        };
        assert!(recording_event_is_log_safe(&event));
        assert!(
            !matches!(event, RecordingEvent::Failed { .. }),
            "转写掉线不得表达为录音失败——用户会以为这一场白录了"
        );
    }

    #[test]
    fn degraded_event_is_log_safe() {
        let event = RecordingEvent::Degraded {
            tier: "yellow".into(),
            reason: "window_drop".into(),
        };
        assert!(
            recording_event_is_log_safe(&event),
            "Degraded 仅档位名与原因码，可入日志/通知"
        );
    }

    #[test]
    fn state_event_is_log_safe() {
        let event = RecordingEvent::State(RecordingState::idle());
        assert!(
            recording_event_is_log_safe(&event),
            "State 不含正文字段，可入日志/通知"
        );
    }

    #[test]
    fn archived_event_carries_only_path_not_content() {
        let event = RecordingEvent::Archived {
            note_rel: "Recordings/meeting.md".into(),
        };
        assert!(recording_event_is_log_safe(&event));
    }

    #[test]
    fn failed_event_carries_only_error_code_not_content() {
        let event = RecordingEvent::Failed {
            code: "ARCHIVE_FAILED".into(),
        };
        assert!(recording_event_is_log_safe(&event));
        // 错误码不含正文（仅大写错误标识符）。
        if let RecordingEvent::Failed { code } = &event {
            assert!(!code.contains("会议"), "错误码不应含业务正文");
        }
    }

    // ── 状态机：初始状态 ──────────────────────────────────────────────────

    #[test]
    fn default_controller_starts_idle() {
        let ctrl = RecordingController::default();
        assert_eq!(ctrl.state().status, RecordingStatus::Idle);
        assert!(ctrl.state().session_id.is_none());
        assert_eq!(ctrl.state().elapsed_ms, 0.0);
    }

    // ── 状态转移广播 + 异常结束复位（B2 / H2）────────────────────────────

    fn session_state(status: RecordingStatus, session_id: &str, elapsed_ms: u64) -> RecordingState {
        recording_state(
            status,
            SessionPurpose::Record,
            session_id,
            "rec.md",
            elapsed_ms,
            "recording:status.recording",
        )
    }

    /// 观察者只在 `status` / `session_id` 变化时被叫，计时推进不叫——否则主窗每秒收四条
    /// 与它无关的 IPC。
    #[test]
    fn observer_fires_on_transitions_but_not_on_elapsed_ticks() {
        use std::sync::{Arc, Mutex};
        let ctrl = RecordingController::default();
        let seen: Arc<Mutex<Vec<RecordingStatus>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        ctrl.set_state_observer(Box::new(move |state| {
            sink.lock().unwrap().push(state.status);
        }));

        ctrl.replace_state(session_state(RecordingStatus::Recording, "s1", 0));
        ctrl.replace_state(session_state(RecordingStatus::Recording, "s1", 250));
        ctrl.replace_state(session_state(RecordingStatus::Recording, "s1", 500));
        ctrl.replace_state(session_state(RecordingStatus::Paused, "s1", 500));
        ctrl.finish_active(RecordingState::idle());
        // 已经是 idle 再 dismiss：没有转移，不广播。
        assert!(ctrl.dismiss_idle());

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                RecordingStatus::Recording,
                RecordingStatus::Paused,
                RecordingStatus::Idle
            ]
        );
    }

    /// `active` 已空而状态仍是会话态 = 会话任务没走到终态就没了；快照必须自愈成 idle，
    /// 否则每一扇以它为准的窗都会永远停在「正在停止…」。
    #[test]
    fn state_snapshot_heals_a_session_state_left_behind_without_an_active_session() {
        let ctrl = RecordingController::default();
        for status in [
            RecordingStatus::Recording,
            RecordingStatus::Paused,
            RecordingStatus::Stopping,
        ] {
            ctrl.replace_state(session_state(status, "orphan", 6250));
            assert_eq!(
                ctrl.state().status,
                RecordingStatus::Idle,
                "{status:?} 应自愈"
            );
            assert!(ctrl.state().session_id.is_none());
        }
        // 非会话态残留（error / mic_busy）不属于本条：它们是要给用户看的结果，由 dismiss 清。
        ctrl.replace_state(RecordingState {
            status: RecordingStatus::Error,
            purpose: SessionPurpose::Record,
            session_id: None,
            note_rel: None,
            elapsed_ms: 0.0,
            message_key: Some("recording:error.capture_failed".into()),
            message_detail: None,
        });
        assert_eq!(ctrl.state().status, RecordingStatus::Error);
    }

    // ── 状态机：无活动会话时控制均静默成功 ──────────────────────────────

    #[test]
    fn dismiss_idle_resets_error_state_when_no_active_session() {
        let ctrl = RecordingController::default();
        // 模拟错误残留状态。
        ctrl.replace_state(RecordingState {
            status: RecordingStatus::Error,
            purpose: SessionPurpose::Record,
            session_id: None,
            note_rel: None,
            elapsed_ms: 0.0,
            message_key: Some("recording:error.capture_failed".into()),
            message_detail: None,
        });
        assert!(ctrl.dismiss_idle(), "无活动会话应复位成功");
        assert_eq!(ctrl.state().status, RecordingStatus::Idle);
    }

    #[test]
    fn dismiss_idle_is_noop_during_active_recording() {
        let ctrl = RecordingController::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        {
            let mut inner = ctrl.inner.lock().unwrap();
            inner.active = Some(ActiveRecording {
                session_id: "live".to_string(),
                control_tx: tx,
                mic_holder: MicHolder::Recording,
                on_event: None,
            });
            inner.state = recording_state(
                RecordingStatus::Recording,
                SessionPurpose::Record,
                "live",
                "n.md",
                0,
                "recording:status.recording",
            );
        }
        assert!(!ctrl.dismiss_idle(), "录音中应拒绝复位（no-op）");
        assert_eq!(
            ctrl.state().status,
            RecordingStatus::Recording,
            "录音中状态不应被改"
        );
    }

    #[test]
    fn stop_on_idle_controller_is_noop() {
        let ctrl = RecordingController::default();
        assert!(ctrl.stop(None).is_ok());
        assert_eq!(ctrl.state().status, RecordingStatus::Idle);
    }

    #[test]
    fn pause_on_idle_controller_is_noop() {
        let ctrl = RecordingController::default();
        let state = ctrl.pause(None).unwrap();
        assert_eq!(state.status, RecordingStatus::Idle);
    }

    #[test]
    fn resume_on_idle_controller_is_noop() {
        let ctrl = RecordingController::default();
        let state = ctrl.resume(None).unwrap();
        assert_eq!(state.status, RecordingStatus::Idle);
    }

    #[test]
    fn bookmark_on_idle_controller_is_noop() {
        let ctrl = RecordingController::default();
        assert!(ctrl.bookmark(None, 5000, None).is_ok());
    }

    // ── 命令定向：早到控制暂存 ──────────────────────────────────────────

    #[test]
    fn early_stop_with_session_id_is_queued_as_pending() {
        let ctrl = RecordingController::default();
        // 无活动会话时，带 session_id 的 stop 应被暂存。
        ctrl.stop(Some("early-session".into())).unwrap();
        let inner = ctrl.inner.lock().unwrap();
        assert!(
            inner.pending_control.is_some(),
            "早到 stop 应已暂存到 pending_control"
        );
        let (id, _) = inner.pending_control.as_ref().unwrap();
        assert_eq!(id, "early-session");
    }

    #[test]
    fn early_stop_without_session_id_is_discarded() {
        let ctrl = RecordingController::default();
        ctrl.stop(None).unwrap();
        let inner = ctrl.inner.lock().unwrap();
        assert!(
            inner.pending_control.is_none(),
            "无 session_id 的早到控制不应暂存"
        );
    }

    #[test]
    fn early_pause_with_session_id_is_queued() {
        let ctrl = RecordingController::default();
        ctrl.pause(Some("sess-abc".into())).unwrap();
        let inner = ctrl.inner.lock().unwrap();
        assert!(inner.pending_control.is_some());
    }

    // ── 命令定向：session_id 不匹配时显式拒绝控制 ──────────────────────

    #[test]
    fn control_targeting_wrong_session_returns_error_when_active() {
        // 构造一个有活动会话的控制器（直接写内部状态，绕过真实 start）。
        let ctrl = RecordingController::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        {
            let mut inner = ctrl.inner.lock().unwrap();
            inner.active = Some(ActiveRecording {
                session_id: "session-A".to_string(),
                control_tx: tx,
                mic_holder: MicHolder::Recording,
                on_event: None,
            });
            inner.state = recording_state(
                RecordingStatus::Recording,
                SessionPurpose::Record,
                "session-A",
                "notes/meeting.md",
                0,
                "recording:status.recording",
            );
        }
        // 发送到 session-B（不匹配） → 不应发送到 control_tx，并须显式报错让表面可恢复。
        let result = ctrl.stop(Some("session-B".into()));
        assert!(result.is_err(), "错误 session_id 不得静默成功");
        // 状态应仍为 Recording（未被改变）。
        assert_eq!(ctrl.state().status, RecordingStatus::Recording);
    }

    #[test]
    fn stop_enters_stopping_before_session_cleanup() {
        let ctrl = RecordingController::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut inner = ctrl.inner.lock().unwrap();
            inner.active = Some(ActiveRecording {
                session_id: "session-A".to_string(),
                control_tx: tx,
                mic_holder: MicHolder::Recording,
                on_event: None,
            });
            inner.state = recording_state(
                RecordingStatus::Recording,
                SessionPurpose::Caption,
                "session-A",
                "",
                1_250,
                "recording:status.recording",
            );
        }

        let returned = ctrl.stop(Some("session-A".into())).unwrap();

        assert!(matches!(rx.try_recv(), Ok(RecordingControl::Stop)));
        let state = ctrl.state();
        assert_eq!(state.status, RecordingStatus::Stopping);
        assert_eq!(returned.status, RecordingStatus::Stopping);
        assert_eq!(state.session_id.as_deref(), Some("session-A"));
        assert_eq!(state.elapsed_ms, 1_250.0);
    }

    #[tokio::test]
    async fn capture_event_drain_is_bounded_when_a_sender_is_accidentally_retained() {
        let (tx, mut rx) = mpsc::unbounded_channel::<CaptureEvent>();
        assert!(
            !drain_capture_events(&mut rx, Duration::from_millis(5)).await,
            "发送端仍存活时必须按时返回 false，停止控制面不得永久等待"
        );
        drop(tx);
        assert!(
            drain_capture_events(&mut rx, Duration::from_millis(50)).await,
            "释放最后发送端后应立即观察到 EOF"
        );
    }

    #[test]
    fn queued_level_state_cannot_regress_stopping_to_recording() {
        let ctrl = RecordingController::default();
        ctrl.replace_state(recording_state(
            RecordingStatus::Stopping,
            SessionPurpose::Caption,
            "session-A",
            "",
            1_250,
            "recording:status.stopping",
        ));

        ctrl.replace_state(recording_state(
            RecordingStatus::Recording,
            SessionPurpose::Caption,
            "session-A",
            "",
            1_300,
            "recording:status.recording",
        ));

        // 读原始状态而不是 `state()` 快照：这里没有活动会话，快照会把孤儿会话态自愈成
        // idle（H2），而本条要验的是 `replace_state` 的回退守卫本身。
        let raw = ctrl.inner.lock().unwrap().state.status;
        assert_eq!(raw, RecordingStatus::Stopping);
    }

    /// VPIO 集成冒烟（#[ignore]，真机 + 麦克风 TCC）：start_vpio_mic_capture 整链
    /// （VPIO → FrameInjector → 消费管线）应产出 Level 事件且无致命诊断。
    /// 运行：`cargo test -p luna-app --lib vpio_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "需真机麦克风（VPIO AudioUnit）"]
    fn vpio_smoke_produces_levels() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = CaptureConfig {
            mode: CaptureMode::RealtimeStream,
            emit_level: true,
            save: None,
            ..CaptureConfig::default()
        };
        let (handle, guard) =
            start_vpio_mic_capture(&cfg, tx, None).expect("VPIO 启动应成功（macOS 真机）");
        std::thread::sleep(std::time::Duration::from_secs(3));
        let mut guard = guard;
        guard.release();
        let _ = handle.stop();
        let mut levels = 0;
        let mut no_data = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                CaptureEvent::Level(_) => levels += 1,
                CaptureEvent::Diagnostic(CaptureDiagnostic::NoCaptureData { .. }) => {
                    no_data = true;
                }
                _ => {}
            }
        }
        assert!(levels > 10, "3s 内应产出连续 Level 事件（实得 {levels}）");
        assert!(!no_data, "VPIO 正常运行不得触发零回调看门狗");
    }

    /// Both 档系统轨时钟合理性（#[ignore]，真机 + 麦克风/系统音频录制 TCC；ADR-082）。
    ///
    /// **这是那个 bug 的现场复现夹具**：必须**同时**起 VPIO（mic 侧，ADR-021）与 CATap
    /// （系统轨）——VPIO 一启动就给默认输出设备装上一条 6 声道回声参考输入流，修复前它会
    /// 并进聚合设备的同一个 `AudioBufferList` 并排在 tap 之前，于是系统轨帧数多算 3.00×
    /// （drift ≈ +2,002,000ppm）。单起 tap 复现不出来，这也正是它躲过此前全部验收的原因。
    ///
    /// 运行：`cargo test -p luna-app --lib both_system_track_clock -- --ignored --nocapture`
    #[test]
    #[ignore = "需真机（VPIO 麦克风 + 系统音频录制 TCC），约 20s"]
    fn both_system_track_clock_stays_sane_with_vpio() {
        let cfg = CaptureConfig {
            mode: CaptureMode::RealtimeStream,
            emit_level: true,
            save: None,
            ..CaptureConfig::default()
        };
        let epoch = std::time::Instant::now();
        // mic 侧：VPIO（Both 档 macOS 主力）——本夹具的关键前置，不可省。
        let (mic_tx, _mic_rx) = std::sync::mpsc::channel();
        let (mic_handle, mut mic_guard) =
            start_vpio_mic_capture(&cfg, mic_tx, Some(luna_audio::CaptureClock::new(epoch)))
                .expect("VPIO 启动应成功（macOS 真机）");
        // 系统轨：CATap。
        let (sys_tx, sys_rx) = std::sync::mpsc::channel();
        let (sys_handle, mut sys_guard) = start_system_capture(
            &cfg,
            sys_tx,
            Some(luna_audio::CaptureClock::new(epoch)),
            None,
        )
        .expect("CATap 启动应成功（macOS 真机 + 系统音频录制 TCC）");

        // 漂移评估需 ≥5s 主机跨度，指标默认每 10 次评估上报一次 → 留够 ~18s。
        std::thread::sleep(std::time::Duration::from_secs(18));

        sys_guard.release();
        let _ = sys_handle.stop();
        mic_guard.release();
        let _ = mic_handle.stop();

        let mut drifts = Vec::new();
        let mut source_lost = false;
        while let Ok(ev) = sys_rx.try_recv() {
            match ev {
                CaptureEvent::Diagnostic(CaptureDiagnostic::ClockDrift { drift_ppm, .. }) => {
                    drifts.push(drift_ppm);
                }
                CaptureEvent::Diagnostic(CaptureDiagnostic::DeviceLost) => source_lost = true,
                _ => {}
            }
        }
        assert!(
            !source_lost,
            "交付布局守卫不得误报——正常授权态下 tap 应是聚合设备唯一输入流"
        );
        // 真机验收要看的就是这一行（`-- --nocapture`）：修复前是 +2,002,000ppm 一路涨。
        eprintln!("[验收] 系统轨 drift 样本（ppm）：{drifts:?}");
        assert!(
            !drifts.is_empty(),
            "18s 内系统轨应至少上报一次 ClockDrift 指标"
        );
        for drift in &drifts {
            assert!(
                drift.abs() < RATE_MISMATCH_PPM / 10.0,
                "系统轨 drift 应在正常量级（<10000ppm），实得 {drift}ppm——\
                 修复前这里是 +2,002,000ppm（6ch 参考流被当成 2ch tap）"
            );
        }
    }
}
