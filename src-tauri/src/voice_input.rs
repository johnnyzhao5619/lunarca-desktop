//! AI 语音输入编排（Phase 4 C2/D）。业务状态与落地决策归 Rust，前端只渲染。

use luna_audio::{start_capture, CaptureConfig, CaptureEvent, CaptureMode, SpeechEvent};
use luna_core::{
    AudioRef, Instruction, Job, JobEvent, JobKind, JobOrigin, Lang, PartialPayload, Priority,
    PromptRegistry, PromptScope, ResultPayload, Segment, StreamId, TextRef, WindowInput,
};
// 个人词典是**域类型**（ASR 提示 / canonical 兜底 / 翻译与润色约束四路共用），
// 事实源在 `luna_core::glossary`；本模块只做语音输入侧的编排，不再自持一份实现。
pub use luna_core::glossary::{
    apply_glossary, apply_glossary_to_segments, glossary_initial_prompt, parse_glossary_import,
    with_glossary_constraint, GlossaryEntry,
};
use luna_jobqueue::{JobHandle, JobQueue, JobSink};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use tokio::sync::mpsc;

use crate::arbiter::{MicArbiter, MicHolder};
#[cfg(target_os = "macos")]
use crate::platform::TextInjector as _;
use crate::popup::PopupSession;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceInputSink {
    CursorOrClipboard,
    PopupTranslateInput,
    /// 创作空间脚本大纲的「口述一段」（ADR-162 ⑮，PRD-19-013）：原话落到脚本光标处。与弹窗
    /// 翻译输入框同属「应用内输入框」——原始转写、出终稿即落地、落地后会话结束。
    ScriptDraft,
}

/// 是不是「应用内输入框」一类的落点：原话经 [`VoiceInputEvent::FieldInput`] 交给发起它的界面，
/// 不注入光标、不进剪贴板、不存速记。
#[must_use]
pub fn is_field_sink(sink: VoiceInputSink) -> bool {
    matches!(
        sink,
        VoiceInputSink::PopupTranslateInput | VoiceInputSink::ScriptDraft
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceRefineMode {
    General,
    Email,
    Slack,
    CodeComment,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceInputOutputKind {
    Raw,
    Refined,
    Translated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefinePolicy {
    RunRefine,
    UseRawTranscript,
}

/// 将导入条目并入现有词典：按词条（term）精确去重，已有词条不覆盖（用户手工维护优先）。
/// 返回（合并结果, 新增数, 跳过数）。
#[must_use]
pub fn merge_glossary_entries(
    existing: Vec<GlossaryEntry>,
    incoming: Vec<GlossaryEntry>,
) -> (Vec<GlossaryEntry>, u32, u32) {
    let mut seen: std::collections::HashSet<String> =
        existing.iter().map(|e| e.term.clone()).collect();
    let mut merged = existing;
    let mut imported = 0u32;
    let mut skipped = 0u32;
    for entry in incoming {
        if seen.insert(entry.term.clone()) {
            merged.push(entry);
            imported += 1;
        } else {
            skipped += 1;
        }
    }
    (merged, imported, skipped)
}

#[must_use]
pub fn refine_policy(
    auto_refine: bool,
    sink: VoiceInputSink,
    refine_mode: VoiceRefineMode,
) -> RefinePolicy {
    if auto_refine
        && !matches!(refine_mode, VoiceRefineMode::Raw)
        && matches!(sink, VoiceInputSink::CursorOrClipboard)
    {
        RefinePolicy::RunRefine
    } else {
        RefinePolicy::UseRawTranscript
    }
}

/// 语音输入各档的润色指令（模板归 [`PromptRegistry`]，ADR-050）。
///
/// 这些**是**能力模板提示词，不是场景数据：它们最终整句落进 `Refine` 的 system
/// 提示词里，措辞与 `refine.system` 互相牵制。留在代码里就意味着运营调得动一半、
/// 调不动另一半。术语约束是**用户内容派生**（命中的词表项），仍在客户端就地拼接。
#[must_use]
pub fn voice_refine_instruction(
    registry: &PromptRegistry,
    refine_mode: VoiceRefineMode,
    text: &str,
    glossary: &[GlossaryEntry],
) -> Option<Instruction> {
    if matches!(refine_mode, VoiceRefineMode::Raw) {
        return None;
    }

    let style_key = match refine_mode {
        VoiceRefineMode::General => "voice_style.general",
        VoiceRefineMode::Email => "voice_style.email",
        VoiceRefineMode::Slack => "voice_style.slack",
        VoiceRefineMode::CodeComment => "voice_style.code_comment",
        VoiceRefineMode::Raw => unreachable!("raw mode returned above"),
    };
    let style = registry.render(PromptScope::Refine, style_key, &[]);
    let instruction = registry.instruction(
        PromptScope::Refine,
        "voice_instruction",
        &[("style", &style)],
    );
    with_glossary_constraint(registry, Some(instruction), text, glossary)
}

#[must_use]
pub fn voice_translate_instruction(
    registry: &PromptRegistry,
    target_lang: Lang,
    text: &str,
    glossary: &[GlossaryEntry],
) -> Option<Instruction> {
    // 语言全称而非代码（TDD-03 §3.3）：`into en` 对小模型几乎不起作用，与翻译 system
    // 模板同一策略。
    let instruction = registry.instruction(
        PromptScope::Translate,
        "voice_instruction",
        &[("target_lang", target_lang.display_name())],
    );
    with_glossary_constraint(registry, Some(instruction), text, glossary)
}

#[must_use]
pub fn should_finish_after_landing(sink: VoiceInputSink) -> bool {
    is_field_sink(sink)
}

#[must_use]
pub fn should_defer_landing_until_user_action(sink: VoiceInputSink) -> bool {
    matches!(sink, VoiceInputSink::CursorOrClipboard)
}

#[derive(Clone, Debug, Type)]
#[specta(type = Vec<u8>)]
pub struct RawBytes(Vec<u8>);

impl IpcResponse for RawBytes {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.0))
    }
}

#[must_use]
pub fn segments_to_transcript(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LandingMode {
    Inject,
    Clipboard,
    QuickNote,
}

impl LandingMode {
    #[must_use]
    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("clipboard") => Self::Clipboard,
            Some("quick_note") => Self::QuickNote,
            _ => Self::Inject,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceInputMode {
    Hold,
    Toggle,
}

impl VoiceInputMode {
    #[must_use]
    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("toggle") => Self::Toggle,
            _ => Self::Hold,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTerminal {
    Finished,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingDecision {
    Inject,
    Clipboard,
    QuickNote,
    CopyFallback,
    Skip,
}

#[must_use]
pub fn landing_decision(terminal: SessionTerminal, mode: LandingMode) -> LandingDecision {
    match terminal {
        SessionTerminal::Finished => match mode {
            LandingMode::Inject => LandingDecision::Inject,
            LandingMode::Clipboard => LandingDecision::Clipboard,
            LandingMode::QuickNote => LandingDecision::QuickNote,
        },
        SessionTerminal::Cancelled | SessionTerminal::Failed => LandingDecision::Skip,
    }
}

#[must_use]
pub fn landing_failure_decision(mode: LandingMode) -> LandingDecision {
    match mode {
        LandingMode::Inject => LandingDecision::CopyFallback,
        LandingMode::Clipboard | LandingMode::QuickNote => LandingDecision::Skip,
    }
}

#[derive(Clone, Debug, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceInputStatus {
    Idle,
    Recording,
    Recognizing,
    Landing,
    CopiedFallback,
    Cancelled,
    Error,
    MicBusy,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct VoiceInputState {
    pub status: VoiceInputStatus,
    pub landing_mode: LandingMode,
    pub input_mode: VoiceInputMode,
    pub auto_refine: bool,
    pub translate_enabled: bool,
    pub translate_target_lang: String,
    pub message_key: Option<String>,
    /// 补充细节（路径、原始错误串等**与语言无关**的具体值）。
    ///
    /// 它此前叫 `guidance_zh` 并直接装中文句子，而前端把它原样渲染——英文界面的用户
    /// 会读到中文。可本地化的那一半一律走 `message_key`（前端 `t(key)`），这里只留
    /// "翻译不了也不该翻译"的那一半。
    pub message_detail: Option<String>,
    /// 有一份识别结果还没落地到任何去处（注入 / 复制 / 存为速记 / 填入弹窗都算落地）。
    ///
    /// 关闭 HUD 会丢掉它，HUD 据此先确认（PRD-02-022「先审阅再落地」的另一面）。判定在 Rust：
    /// 前端此前按五个落地文案键自己比对——同一件事的第二份定义，新增一种落地就会漏（ADR-003）。
    pub result_pending: bool,
}

/// 会话留下的原始录音（PRD-02-030）：临时目录里的 wav，落成速记时搬进笔记附件目录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceAudioFile {
    pub path: std::path::PathBuf,
    pub duration_ms: u64,
}

impl VoiceAudioFile {
    /// 会话没落成速记：临时录音没有归宿，删掉（删不掉也只是缓存目录多一个文件）。
    fn discard(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 采集停下后拿到的分片表 → 会话录音（速记极少滚动切分，只认第一片；多片按第一片算时长）。
fn voice_audio_from_parts(parts: &[luna_audio::PartInterval]) -> Option<VoiceAudioFile> {
    let first = parts.first()?;
    let samples = first.abs_end.saturating_sub(first.abs_start);
    Some(VoiceAudioFile {
        path: first.path.clone(),
        duration_ms: samples * 1000 / u64::from(luna_audio::TARGET_SAMPLE_RATE),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceQuickNoteDraft {
    pub raw: String,
    pub final_text: String,
    pub output_kind: VoiceInputOutputKind,
    /// 原始录音（设置 `voice_input.keep_audio` 开且采集落盘成功时有）。
    pub audio: Option<VoiceAudioFile>,
}

pub type SaveQuickNoteFn =
    Arc<dyn Fn(VoiceQuickNoteDraft) -> Result<String, String> + Send + Sync + 'static>;

#[derive(Clone)]
pub struct VoiceInputStartOptions {
    pub landing_mode: LandingMode,
    pub input_mode: VoiceInputMode,
    pub auto_refine: bool,
    pub refine_mode: VoiceRefineMode,
    pub translate_enabled: bool,
    pub translate_target_lang: Lang,
    pub glossary: Vec<GlossaryEntry>,
    pub lang_hint: Option<Lang>,
    pub sink: VoiceInputSink,
    pub mic_holder: MicHolder,
    pub popup_session: Option<Arc<Mutex<PopupSession>>>,
    pub save_quick_note: Option<SaveQuickNoteFn>,
    /// 原始录音的临时落盘目录（`None` = 不留音频，PRD-02-030）。
    pub audio_capture_dir: Option<std::path::PathBuf>,
    pub client_session_id: Option<String>,
    /// 采集调参（波形帧率 / VAD 灵敏度…；来自设置，投影点见 `ipc::workspace::capture_tuning`）。
    ///
    /// 语音输入此前用 `CaptureConfig::default()` 起会话，**整个绕过了这条投影**：
    /// 于是 `recording.waveform_fps` 对语音输入 HUD 的波形一直无效，新接的
    /// `recording.vad_sensitivity` 若不一并接上，就会重演同一个形态——设置页有旋钮、
    /// 某一条采集路径上不生效，而没有任何东西会因此变红。
    pub tuning: luna_audio::CaptureTuning,
    /// VAD 实现（`None` = 能量 VAD，ADR-012 / ADR-077）。
    pub vad: Option<luna_audio::VadFactory>,
    /// 流式实时识别的会话句柄来源（[`luna_core::StreamingAsr`] 接缝，与录音同构，
    /// TDD-02 §3.3-B）。可能是本机 whisper、服务端实时通道或两者的降级组合（ADR-038），
    /// 采集侧不区分。`None` = 无可用转写通道 → 会话直接以 MODEL_MISSING 失败
    ///（语音输入离开 ASR 无意义，不同于录音的纯录音降级）。
    pub asr_provider: Option<Arc<dyn luna_core::StreamingAsr>>,
}

#[derive(Clone)]
pub struct VoiceTextLandingOptions {
    pub landing_mode: LandingMode,
    pub sink: VoiceInputSink,
    pub popup_session: Option<Arc<Mutex<PopupSession>>>,
    pub save_quick_note: Option<SaveQuickNoteFn>,
    /// 「注入光标」的目标窗口（Windows：会话开始时的前台窗口，注入前核对它仍在前台，
    /// ADR-163 ③）。其它平台恒为 `None`。
    pub inject_target: Option<isize>,
}

impl From<&VoiceInputStartOptions> for VoiceTextLandingOptions {
    fn from(options: &VoiceInputStartOptions) -> Self {
        Self {
            landing_mode: options.landing_mode,
            sink: options.sink,
            popup_session: options.popup_session.clone(),
            save_quick_note: options.save_quick_note.clone(),
            inject_target: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum VoiceInputEvent {
    State(VoiceInputState),
    /// 按住说话期间的实时识别反馈（PRD-02-004）：`text` 为本会话累计已定稿（committed）
    /// 转写，`hypothesis` 为可改写的暂定尾（HUD 灰显）。与录音 Partial 同源
    /// （LocalAgreement-2，TDD-02 §3.3-B）；发往用户自己的 HUD，绝不入日志。
    Partial {
        text: String,
        hypothesis: String,
    },
    /// 应用内输入框的原话（弹窗翻译输入框 / 脚本口述，[`is_field_sink`]）。
    FieldInput {
        text: String,
    },
    Landed {
        mode: String,
    },
    Failed {
        code: String,
    },
    /// 这次识别由谁执行、音频会不会**离开这台设备**（ADR-038 ④ / `PRD-04-060`）。
    ///
    /// 语音输入是**最高频**的麦克风路径——用户一天可能按几十次。单次只有几秒，但累计
    /// 外发量不小，而「音频离开设备」这件事在这里此前完全没有提示：录音与实时字幕都做了，
    /// 唯独最常用的这条没有。
    ///
    /// 与录音同一条纪律：会话建立时按 Provider 自报的位置立刻发（不等服务端回话），
    /// 服务端回报 `executed_by` 后再发一次补上名字并按它校正（降级切回本机时熄灭）。
    Executor {
        /// 执行者展示名；`None` = 会话刚建立，服务端尚未回报。
        display_name: Option<String>,
        /// 音频会离开这台设备吗。UI 据此点亮联网指示。
        leaves_device: bool,
    },
    Transcript {
        raw: String,
        final_text: String,
        refined: bool,
        output_kind: VoiceInputOutputKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Stop,
    Cancel,
}

struct ActiveSession {
    tx: mpsc::UnboundedSender<Control>,
    client_session_id: Option<String>,
    mic_holder: MicHolder,
}

#[derive(Clone, Debug)]
struct PendingVoiceOutput {
    raw: String,
    final_text: String,
    output_kind: VoiceInputOutputKind,
    /// 原始录音（落成速记时随之归档；换成别的落点时删）。
    audio: Option<VoiceAudioFile>,
    /// 这份结果已经存成的速记路径（`None` = 还没存过）。
    ///
    /// 落地不清空 `pending_output`（用户存完还可以再注入 / 再复制），于是「存入速记笔记」
    /// 再点一次就会再建一篇——同一段话两份文件（真机 2026-09-03）。存过就记下路径，
    /// 再存只回同一条路径，不再落盘。
    landed_note: Option<String>,
    /// 已经成功落地过至少一次（任一去处）。之后关掉 HUD 不算丢结果。
    landed: bool,
}

#[derive(Clone, Debug)]
struct VoiceTextOutput {
    raw: String,
    final_text: String,
    output_kind: VoiceInputOutputKind,
    audio: Option<VoiceAudioFile>,
}

impl VoiceTextOutput {
    fn into_quick_note_draft(self) -> VoiceQuickNoteDraft {
        VoiceQuickNoteDraft {
            raw: self.raw,
            final_text: self.final_text,
            output_kind: self.output_kind,
            audio: self.audio,
        }
    }
}

struct ControllerInner {
    state: VoiceInputState,
    active: Option<ActiveSession>,
    pending_control: Option<(String, Control)>,
    level_channel: Option<Channel<RawBytes>>,
    pending_output: Option<PendingVoiceOutput>,
    /// 本次会话是否已经准备过（`Some` = 已准备，值是当时用的预设）。
    ///
    /// 它存在只为回答一个问题：**这次唤起是新会话还是同一个 idle 会话被再点了一下**。
    /// 分不清的话，用户在 HUD 里选好落点、还没开口就又按了一次快捷键，他刚选的会被
    /// 「读回持久默认」悄悄抹掉（TDD-07 §2.3「重复唤起同一 idle 会话不重置用户选择」）。
    prepared: Option<HudPreset>,
    /// 本次会话「注入光标」的目标窗口（会话开始时记下，[`crate::platform::inject_target_now`]）。
    inject_target: Option<isize>,
}

/// 一次唤起 HUD 时的落点预设（TDD-07 §2.3）。
///
/// **它只改本次会话的有效默认动作，绝不写设置**：用户从「语音速记」进来这一次想存成
/// 速记，不等于他要把全局默认改掉——把入口的意图写进持久设置，是拿一次点击替他做了一个
/// 长期决定。故预设初始化与清除都不写设置、不发 `SettingsChanged`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudPreset {
    /// 普通入口：按用户此刻的持久化默认落点。
    Current,
    /// 「语音速记」入口：本次默认存为速记（PRD-02-020 / PRD-07-069）。
    QuickNote,
}

pub struct VoiceInputController {
    inner: Mutex<ControllerInner>,
}

impl Default for VoiceInputController {
    fn default() -> Self {
        Self {
            inner: Mutex::new(ControllerInner {
                state: VoiceInputState {
                    status: VoiceInputStatus::Idle,
                    landing_mode: LandingMode::Inject,
                    input_mode: VoiceInputMode::Hold,
                    auto_refine: true,
                    translate_enabled: false,
                    translate_target_lang: "en".into(),
                    message_key: None,
                    message_detail: None,
                    result_pending: false,
                },
                active: None,
                pending_control: None,
                level_channel: None,
                pending_output: None,
                prepared: None,
                inject_target: None,
            }),
        }
    }
}

impl VoiceInputController {
    #[must_use]
    pub fn state(&self) -> VoiceInputState {
        self.inner
            .lock()
            .map(|inner| inner.state.clone())
            .unwrap_or_else(|_| VoiceInputState {
                status: VoiceInputStatus::Error,
                landing_mode: LandingMode::Inject,
                input_mode: VoiceInputMode::Hold,
                auto_refine: true,
                translate_enabled: false,
                translate_target_lang: "en".into(),
                message_key: Some("voice:error.state_lock".into()),
                message_detail: None,
                result_pending: false,
            })
    }

    pub fn subscribe_levels(&self, channel: Channel<RawBytes>) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "语音输入状态锁中毒".to_string())?;
        inner.level_channel = Some(channel);
        Ok(())
    }

    pub fn stop(&self, client_session_id: Option<String>) -> Result<(), String> {
        self.send_control(Control::Stop, client_session_id)
    }

    pub fn cancel(&self, client_session_id: Option<String>) -> Result<(), String> {
        self.send_control(Control::Cancel, client_session_id)
    }

    pub fn stop_holder(&self, holder: MicHolder) -> Result<(), String> {
        self.send_holder_control(Control::Stop, holder)
    }

    pub fn cancel_holder(&self, holder: MicHolder) -> Result<(), String> {
        self.send_holder_control(Control::Cancel, holder)
    }

    fn send_holder_control(&self, control: Control, holder: MicHolder) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "语音输入状态锁中毒".to_string())?;
        let Some(active) = &inner.active else {
            return Ok(());
        };
        if !control_targets_holder(active.mic_holder, holder) {
            return Ok(());
        }
        active
            .tx
            .send(control)
            .map_err(|_| "语音输入会话已结束".to_string())
    }

    fn send_control(
        &self,
        control: Control,
        client_session_id: Option<String>,
    ) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "语音输入状态锁中毒".to_string())?;
        let Some(active) = &inner.active else {
            if should_queue_pending_control(false, client_session_id.as_deref()) {
                if let Some(id) = client_session_id {
                    inner.pending_control = Some((id, control));
                }
                eprintln!("[F2] 控制事件早于 active session 到达，已按 client session 暂存。");
            }
            return Ok(());
        };
        if !control_targets_active(
            active.client_session_id.as_deref(),
            client_session_id.as_deref(),
        ) {
            eprintln!("[F2] 忽略非当前 client session 的语音控制事件。");
            return Ok(());
        }
        active
            .tx
            .send(control)
            .map_err(|_| "语音输入会话已结束".to_string())
    }

    /// 准备一次 HUD 会话的落点预设（TDD-07 §2.3 的唯一入口）。
    ///
    /// **所有唤起路径都必须经它**——应用内入口、全局快捷键、双击手势、托盘菜单。绕过
    /// 它直接 `show` 窗口，就会出现「从『语音速记』进来却按注入光标落地」这种入口与行为
    /// 对不上的情况，而用户看不出是哪儿错了。
    ///
    /// 返回 `Err(VOICE_SESSION_BUSY)` 表示已有会话在采集/处理/审阅——调用方仍应把已有
    /// HUD 显示出来（用户要的是回到那个会话），但**不改原会话的任何设定**。
    ///
    /// `persisted` 是此刻的持久化默认落点，由调用方从设置读——控制器不认识设置键。
    pub fn prepare_session(
        &self,
        preset: HudPreset,
        persisted: LandingMode,
    ) -> Result<VoiceInputState, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "语音输入状态锁中毒".to_string())?;
        let busy = !matches!(
            inner.state.status,
            VoiceInputStatus::Idle
                | VoiceInputStatus::Cancelled
                | VoiceInputStatus::Error
                | VoiceInputStatus::CopiedFallback
        ) || inner.pending_output.is_some();
        if busy {
            return Err("VOICE_SESSION_BUSY: 已有一个语音会话在进行中。".to_owned());
        }
        // 同一个 idle 会话被再点一下：什么都不改。用户可能已经在 HUD 里选好了落点。
        if inner.prepared.is_none() {
            inner.state.landing_mode = match preset {
                // 每次新会话都**重新读**持久值，不用上次遗留的运行态：上一次会话里
                // 用户临时改成剪贴板，不该变成下一次的默认（TDD-07 §2.3）。
                HudPreset::Current => persisted,
                HudPreset::QuickNote => LandingMode::QuickNote,
            };
            inner.prepared = Some(preset);
        }
        Ok(inner.state.clone())
    }

    /// 清掉本次会话的预设——**只在会话真正结束时**（成功落地 / 取消 / 明确丢弃并关闭）。
    ///
    /// ASR 出终稿、停止采集都**不清**：那时用户正在审阅结果，会话还没结束；重说也保留
    /// （同一次意图的第二次尝试），落地失败同样保留以便重试。清早了的表现是：他点了重试，
    /// 落点却从速记变回了注入光标。
    pub fn clear_session_preset(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.prepared = None;
        }
    }

    pub fn set_mode(
        &self,
        landing_mode: LandingMode,
        input_mode: VoiceInputMode,
        auto_refine: bool,
        translate_enabled: bool,
        translate_target_lang: String,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state.landing_mode = landing_mode;
            inner.state.input_mode = input_mode;
            inner.state.auto_refine = auto_refine;
            inner.state.translate_enabled = translate_enabled;
            inner.state.translate_target_lang = translate_target_lang;
        }
    }

    pub async fn land_pending(
        &self,
        landing_mode: LandingMode,
        save_quick_note: Option<SaveQuickNoteFn>,
        // 用户在 HUD 结果区的编辑稿（PRD-02-022）：非空白时覆盖 final_text 落地；raw 保留对照。
        edited_text: Option<String>,
    ) -> Result<VoiceInputState, String> {
        let (output, mut state, inject_target) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "语音输入状态锁中毒".to_string())?;
            let Some(pending) = inner.pending_output.clone() else {
                return Err("NO_PENDING_OUTPUT: 没有可输出的语音识别结果。".into());
            };
            // 同一份结果只存一篇速记：已经存过就把那条路径原样再报一次，不再落盘。
            if landing_mode == LandingMode::QuickNote {
                if let Some(path) = pending.landed_note.clone() {
                    let mut state = inner.state.clone();
                    state.status = VoiceInputStatus::Idle;
                    state.message_key = Some("voice:status.saved_note".into());
                    state.message_detail = Some(path);
                    state.result_pending = false;
                    inner.state = state.clone();
                    return Ok(state);
                }
            }
            inner.state.status = VoiceInputStatus::Landing;
            inner.state.message_key = Some("voice:status.landing".into());
            let final_text = edited_text
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(pending.final_text);
            (
                VoiceTextOutput {
                    raw: pending.raw,
                    final_text,
                    output_kind: pending.output_kind,
                    audio: pending.audio,
                },
                inner.state.clone(),
                inner.inject_target,
            )
        };

        let options = VoiceTextLandingOptions {
            landing_mode,
            sink: VoiceInputSink::CursorOrClipboard,
            popup_session: None,
            save_quick_note,
            inject_target,
        };
        let landed = land_text(&options, output, None).await;
        let (status, key) = match &landed {
            LandTextOutcome::Injected => (VoiceInputStatus::Idle, "voice:status.injected"),
            LandTextOutcome::InjectedClipboardNotRestored => (
                VoiceInputStatus::Idle,
                "voice:status.injected_clipboard_not_restored",
            ),
            LandTextOutcome::Copied => (VoiceInputStatus::Idle, "voice:status.copied"),
            LandTextOutcome::CopiedFallback => (
                VoiceInputStatus::CopiedFallback,
                "voice:status.copied_fallback",
            ),
            LandTextOutcome::TargetChanged => (
                VoiceInputStatus::CopiedFallback,
                "voice:status.target_changed",
            ),
            LandTextOutcome::QuickNote { .. } => {
                (VoiceInputStatus::Idle, "voice:status.saved_note")
            }
            LandTextOutcome::FieldInput(sink) => (VoiceInputStatus::Idle, field_filled_key(*sink)),
            LandTextOutcome::Failed(_) => (VoiceInputStatus::Error, "voice:status.error"),
        };
        state.status = status;
        state.message_key = Some(key.into());
        match landed {
            LandTextOutcome::QuickNote { path } => {
                // 此前是 `format!("已存为速记笔记：{path}")` —— 一句中文加一个路径拼在
                // 一起塞进同一个字段，英文界面照样显示中文。可本地化的那半由上面的
                // `message_key` 承担，这里只留路径本身（它不该被翻译）。
                state.message_detail = Some(path.clone());
                if let Ok(mut inner) = self.inner.lock() {
                    if let Some(pending) = inner.pending_output.as_mut() {
                        pending.landed_note = Some(path.clone());
                    }
                }
            }
            LandTextOutcome::Failed(err) => {
                state.message_detail = Some(err);
            }
            _ => {
                state.message_detail = None;
            }
        }
        let landed_ok = !matches!(state.status, VoiceInputStatus::Error);
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(pending) = inner.pending_output.as_mut() {
                pending.landed |= landed_ok;
                state.result_pending = !pending.landed;
            }
        }
        self.replace_state(state.clone());
        Ok(state)
    }

    /// 明确丢弃并关闭（`voice_input_close`）：待落地的结果一并扔掉，没存成速记的录音删掉。
    /// 此前关闭只取消采集，结果仍挂在控制器上——界面上已经看不见它，状态却还说「有结果」。
    pub fn discard_pending_output(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(old) = inner.pending_output.take() {
                if old.landed_note.is_none() {
                    if let Some(audio) = old.audio.as_ref() {
                        audio.discard();
                    }
                }
            }
            inner.state.result_pending = false;
        }
    }

    pub fn start(
        self: &Arc<Self>,
        queue: Arc<JobQueue>,
        mic: Arc<Mutex<MicArbiter>>,
        options: VoiceInputStartOptions,
        on_event: Channel<VoiceInputEvent>,
    ) -> Result<VoiceInputState, String> {
        {
            let mut arbiter = mic.lock().map_err(|_| "麦克风仲裁锁中毒".to_string())?;
            if let Err(err) = arbiter.try_acquire(options.mic_holder) {
                self.forget_pending_control(options.client_session_id.as_deref());
                let state = VoiceInputState {
                    status: VoiceInputStatus::MicBusy,
                    landing_mode: options.landing_mode,
                    input_mode: options.input_mode,
                    auto_refine: options.auto_refine,
                    translate_enabled: options.translate_enabled,
                    translate_target_lang: options.translate_target_lang.to_string(),
                    message_key: Some(err.i18n_key().into()),
                    message_detail: None,
                    result_pending: false,
                };
                self.replace_state(state.clone());
                let _ = on_event.send(VoiceInputEvent::State(state.clone()));
                return Ok(state);
            }
        }

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "语音输入状态锁中毒".to_string())?;
        if inner.active.is_some() {
            let state = VoiceInputState {
                status: VoiceInputStatus::MicBusy,
                landing_mode: options.landing_mode,
                input_mode: options.input_mode,
                auto_refine: options.auto_refine,
                translate_enabled: options.translate_enabled,
                translate_target_lang: options.translate_target_lang.to_string(),
                message_key: Some("mic.busy.capture_active".into()),
                message_detail: None,
                result_pending: false,
            };
            drop(inner);
            // 不释放麦克风：已有活动会话仍**合法持有**它（上面的 try_acquire 对同一持有者是
            // 幂等的，本次调用并未真正占用）。若在此释放，会把活动会话的租约从其脚下抽走，
            // 导致录音等可并发抢占麦克风——破坏硬互斥不变量（TDD-05 §5.3）。
            self.replace_state(state.clone());
            let _ = on_event.send(VoiceInputEvent::State(state.clone()));
            return Ok(state);
        }

        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let pending_control = options.client_session_id.as_ref().and_then(|id| {
            inner
                .pending_control
                .take()
                .and_then(|(pending_id, control)| (pending_id == *id).then_some(control))
        });
        inner.pending_output = None;
        // 「注入光标」的目标 = 此刻的前台窗口：HUD 不夺焦，按下开始时前台仍是用户要打字的那个应用。
        inner.inject_target = crate::platform::inject_target_now();
        inner.active = Some(ActiveSession {
            tx: control_tx.clone(),
            client_session_id: options.client_session_id.clone(),
            mic_holder: options.mic_holder,
        });
        let state = session_state(
            &options,
            VoiceInputStatus::Recording,
            "voice:status.recording",
        );
        inner.state = state.clone();
        drop(inner);

        let _ = on_event.send(VoiceInputEvent::State(state.clone()));
        let controller = Arc::clone(self);
        // 必须用 tauri::async_runtime::spawn（非裸 tokio::spawn）：本方法由同步 Tauri 命令
        // 调用，该线程未进入 tokio runtime 上下文，裸 tokio::spawn 会 panic「no reactor running」。
        // 与本仓既有 spawn（lib.rs）保持一致。
        tauri::async_runtime::spawn(async move {
            controller
                .run_session(queue, mic, options, on_event, control_rx)
                .await;
        });
        if let Some(control) = pending_control {
            eprintln!("[F2] start 消费早到控制事件，立即收口当前 hold 会话。");
            let _ = control_tx.send(control);
        }

        Ok(state)
    }

    fn forget_pending_control(&self, client_session_id: Option<&str>) {
        let Some(id) = client_session_id else {
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

    fn replace_state(&self, state: VoiceInputState) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state = state;
        }
    }

    fn finish_active(&self, state: VoiceInputState, clear_pending_output: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.active = None;
            inner.state = state;
            if clear_pending_output {
                inner.pending_output = None;
            }
        }
    }

    fn send_level(&self, bytes: Vec<u8>) {
        if let Ok(inner) = self.inner.lock() {
            if let Some(channel) = &inner.level_channel {
                let _ = channel.send(voice_level_payload(bytes));
            }
        }
    }

    /// 本次会话「注入光标」的目标窗口。
    fn inject_target(&self) -> Option<isize> {
        self.inner.lock().ok().and_then(|inner| inner.inject_target)
    }

    fn store_pending_output(
        &self,
        raw: String,
        final_text: String,
        output_kind: VoiceInputOutputKind,
        audio: Option<VoiceAudioFile>,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            // 上一份没落地就被顶替：它的录音也没归宿了。
            if let Some(old) = inner.pending_output.take() {
                if old.landed_note.is_none() {
                    if let Some(a) = old.audio.as_ref() {
                        a.discard();
                    }
                }
            }
            inner.pending_output = Some(PendingVoiceOutput {
                raw,
                final_text,
                output_kind,
                audio,
                landed_note: None,
                landed: false,
            });
        }
    }

    /// 会话主循环（流式实时识别路径，PRD-02-004 / TDD-02 §3.3-B）。
    ///
    /// 架构（ADR-001/003/009/010，与录音同构）：
    /// - 采集 `CaptureMode::RealtimeStream`：VAD 门控滑动窗（会话绝对时钟）直喂 whisper；
    /// - `register_stream` + `Transcribe{AudioRef::Stream}` 走单一 AI 主干，LocalAgreement-2
    ///   产出 committed/hypothesis `Partial` → HUD 按住说话期间实时显示；
    /// - 停止（松开）→ 关流（发 `None`）→ `Finished` 全量段为**权威转写**（含未决尾解码），
    ///   再进入既有优化/翻译/落地管线——识别在按住期间已完成大半，松开到结果显著更快。
    async fn run_session(
        &self,
        queue: Arc<JobQueue>,
        mic: Arc<Mutex<MicArbiter>>,
        options: VoiceInputStartOptions,
        on_event: Channel<VoiceInputEvent>,
        mut control_rx: mpsc::UnboundedReceiver<Control>,
    ) {
        // 无 ASR 模型 → 直接失败（命令层已 ensure_asr_provider_ready，此处为后端防线；
        // 语音输入离开识别无意义，不同于录音的纯录音降级）。
        let Some(asr_provider) = options.asr_provider.clone() else {
            let _ = mic
                .lock()
                .map(|mut arbiter| arbiter.release(options.mic_holder));
            let state = session_state(
                &options,
                VoiceInputStatus::Error,
                voice_error_message_key("MODEL_MISSING"),
            );
            self.finish_active(state.clone(), true);
            let _ = on_event.send(VoiceInputEvent::Failed {
                code: "MODEL_MISSING".into(),
            });
            let _ = on_event.send(VoiceInputEvent::State(state));
            return;
        };

        // 原始录音（PRD-02-030）：设置开着就把这段会话落成 16k 单声道 wav 到临时目录，
        // 落成速记时搬进笔记附件目录；别的落点会话结束即删。
        let save = options.audio_capture_dir.as_ref().and_then(|dir| {
            std::fs::create_dir_all(dir).ok()?;
            Some(luna_audio::SaveSpec {
                format: luna_audio::RecordingFormat::Wav,
                path: dir
                    .join(format!("voice-{}.wav", uuid::Uuid::now_v7()))
                    .to_string_lossy()
                    .into_owned(),
                sample_rate: luna_audio::TARGET_SAMPLE_RATE,
            })
        });
        let cfg = CaptureConfig {
            mode: CaptureMode::RealtimeStream,
            tuning: options.tuning,
            save,
            vad: options.vad.clone(),
            ..CaptureConfig::default()
        };
        let mut audio_parts: Vec<luna_audio::PartInterval> = Vec::new();
        let (capture_tx, capture_rx) = std::sync::mpsc::channel();
        let capture = match start_capture(&cfg, capture_tx, None, None) {
            Ok(handle) => handle,
            Err(e) => {
                let mut state = session_state(
                    &options,
                    VoiceInputStatus::Error,
                    "voice:error.capture_failed",
                );
                state.message_detail = Some(e.to_string());
                let _ = mic
                    .lock()
                    .map(|mut arbiter| arbiter.release(options.mic_holder));
                self.finish_active(state.clone(), true);
                let _ = on_event.send(VoiceInputEvent::State(state));
                return;
            }
        };

        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("luna-voice-input-events".into())
            .spawn(move || {
                while let Ok(event) = capture_rx.recv() {
                    if audio_tx.send(event).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn voice input capture bridge");

        // 联网指示在**任何音频离开设备之前**发出（ADR-038 ④）：判据是 Provider
        // 自报的位置，不等服务端回话——只等回话的话，握手到首帧之间会有一段「在录、
        // 但没有任何联网提示」的时间。语音输入没有「转写开关」（不转写就没有语音输入
        // 这回事），故不像录音那样还要与开关合取。
        let uplink_leaves_device = asr_provider.descriptor().location.is_external();
        let _ = on_event.send(VoiceInputEvent::Executor {
            display_name: None,
            leaves_device: uplink_leaves_device,
        });
        // 流式 ASR 会话：register_stream（具体 Provider）+ AudioRef::Stream Job（单一主干）。
        let stream_id = StreamId::new();
        let reg = asr_provider.register_stream(stream_id);
        let window_tx = reg.window_tx;
        // 会话**登记**的回收句柄，必须单独接住：只取 `window_tx` 的写法会把它就地丢掉，
        // 那等于在 `execute` 摘走之前把会话从登记表里撤了（随后报「未注册的流式会话」）。
        //
        // 它存在的理由：Job 有走不到 `execute` 的出口（会话池准入到期即拒，ADR-078 ③
        // ——「松开热键、立刻再按」正是它要处理的瞬态；以及路由解析失败），而登记此前只由
        // `execute` 回收，于是那些会话连同各自的窗缓冲永久滞留——语音输入是最高频的那条
        // 麦克风路径（一天几十次），而 Provider 实例是复用的，一次一条地叠加。
        //
        // 持到本函数收场即可：本会话的全部出口（取消 / 失败 / 正常收口）都在这个作用域里，
        // 且都紧跟着 return，不像录音那样在 Job 终态之后还要继续跑几十分钟。
        let _stream_release = reg.release;
        let mut job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(stream_id),
                lang_hint: options.lang_hint.clone(),
                initial_prompt: glossary_initial_prompt(&options.glossary),
            },
            JobOrigin::VoiceInput,
        );
        // 会话绑定在**这一个** Provider 实例上（`StreamId` 登在它的表里）。
        // 不钉的话，注册表里另一个流式转写 Provider 也满足能力匹配，路由可能选中它，
        // 而它的登记表里根本没有这个会话。
        job.routing.preferred_provider = Some(asr_provider.id());
        job.priority = Priority::Interactive;
        let mut asr_handle = queue.submit(job);

        let mut capture = Some(capture);
        let mut cancelled = false;
        let mut asr_failed: Option<String> = None;
        // 会话累计已定稿转写（HUD 实时反馈；权威 raw 以收口 Finished 为准，此处仅显示用）。
        let mut committed_text = String::new();
        // 是否已就「转写腿还在等本机会话额度」提示过（ADR-078 ③；额度到手即撤）。
        let mut admission_waiting = false;

        // ── 采集循环（三臂 select：控制 / 采集事件 / ASR Partial）──────────
        'capture: loop {
            tokio::select! {
                Some(control) = control_rx.recv() => {
                    match control {
                        Control::Stop => {
                            // 松开/停止：停采集 → 桥接线程 EOF → 下方 audio_rx None 收口。
                            if let Some(handle) = capture.take() {
                                audio_parts = handle.stop();
                            }
                        }
                        Control::Cancel => {
                            cancelled = true;
                            break 'capture;
                        }
                    }
                }
                event = audio_rx.recv() => {
                    let Some(event) = event else {
                        break 'capture; // 采集已停且事件耗尽
                    };
                    match event {
                        CaptureEvent::Level(bytes) => {
                            self.send_level(bytes);
                        }
                        CaptureEvent::Window(SpeechEvent::Window {
                            samples,
                            abs_start,
                            speech_len,
                            ..
                        }) => {
                            // try_send 不阻塞事件循环；满 = 解码积压（短口述极少见），
                            // 丢窗自愈——权威收口 Finished 仍覆盖全部已入流窗口。
                            if window_tx
                                .try_send(Some(WindowInput {
                                    samples,
                                    abs_start,
                                    speech_len,
                                }))
                                .is_err()
                            {
                                eprintln!("[F2] ASR 窗通道满，丢弃 1 窗（abs_start={abs_start}）。");
                            }
                        }
                        CaptureEvent::Diagnostic(diag) => {
                            // **补 stderr 痕迹，UI 行为一字不改**：这条通路把**所有**诊断
                            // 一律翻译成同一句 HUD 文案（`voice:status.diagnostic`），于是
                            // 「采不到 / 采到静音 / 电平够不够」在日志里全都长成同一个样子——
                            // 语音输入听不见时没有任何可定位的数据。电平类诊断的打印口径与
                            // 录音/字幕两条入口共用同一个函数（绝不复制三份格式）。
                            // 语音输入恒为麦克风采集，故轨标识固定 `mic`。
                            crate::recording::trace_input_level("mic", &diag);
                            let state = session_state(
                                &options,
                                VoiceInputStatus::Recording,
                                "voice:status.diagnostic",
                            );
                            self.replace_state(state.clone());
                            let _ = on_event.send(VoiceInputEvent::State(state));
                        }
                        // RealtimeStream 模式不产端点整段事件（VoiceInput 旧路径事件）。
                        CaptureEvent::SpeechStarted
                        | CaptureEvent::Utterance { .. }
                        | CaptureEvent::Window(SpeechEvent::Utterance { .. }) => {}
                    }
                }
                job_ev = asr_handle.recv() => {
                    match job_ev {
                        // **还在等本机会话额度**（ADR-078 ③）。语音输入是最高频的那条
                        // 麦克风路径（单次几秒、一天几十次），而它此前对这件事完全静默：
                        // 用户按下热键、说完、松开，然后什么都没发生。麦克风硬互斥挡住的是
                        // 「同时采集」，挡不住「上一场刚停、句柄还没收干净」那个瞬态——正是
                        // 会话池宽限期存在的那个理由，所以这条路径必然会被走到。
                        Some(JobEvent::AdmissionDelayed { waited_ms }) => {
                            admission_waiting = true;
                            eprintln!(
                                "[F2] 语音输入的转写腿已等本机会话额度 {waited_ms}ms 仍未开始（采集照常进行）。"
                            );
                            let state = session_state(
                                &options,
                                VoiceInputStatus::Recording,
                                "voice:status.admission_wait",
                            );
                            self.replace_state(state.clone());
                            let _ = on_event.send(VoiceInputEvent::State(state));
                        }
                        // 额度到手 → 撤掉「还在等」。一条挂着不走的等待提示，在识别已经
                        // 开始之后就是在说假话（与录音侧同义，不另造一套语义）。
                        Some(JobEvent::Started) if admission_waiting => {
                            admission_waiting = false;
                            let state = session_state(
                                &options,
                                VoiceInputStatus::Recording,
                                "voice:status.recording",
                            );
                            self.replace_state(state.clone());
                            let _ = on_event.send(VoiceInputEvent::State(state));
                        }
                        // 服务端回报执行者：补上名字，并按它的性质校正指示
                        // （降级切回本机时这一条会把指示灭掉）。
                        Some(JobEvent::ExecutedBy(info)) => {
                            let _ = on_event.send(VoiceInputEvent::Executor {
                                display_name: Some(info.display_name.clone()),
                                leaves_device: info.leaves_device,
                            });
                        }
                        Some(JobEvent::Partial(PartialPayload::Asr { committed_delta, hypothesis, .. })) => {
                            forward_voice_partial(
                                &mut committed_text,
                                committed_delta,
                                hypothesis,
                                &options.glossary,
                                &on_event,
                            );
                        }
                        Some(JobEvent::Failed(err)) => {
                            asr_failed = Some(err.code().into());
                            if let Some(handle) = capture.take() {
                                audio_parts = handle.stop();
                            }
                        }
                        Some(JobEvent::Cancelled) => {
                            cancelled = true;
                            break 'capture;
                        }
                        Some(_) => {}
                        // 句柄流关闭（不应先于关流发生）：交由收口阶段按失败处理。
                        None => {
                            asr_failed.get_or_insert_with(|| "JOB_STREAM_ENDED".into());
                            if let Some(handle) = capture.take() {
                                audio_parts = handle.stop();
                            }
                        }
                    }
                }
                else => break 'capture,
            }
        }

        if let Some(handle) = capture.take() {
            audio_parts = handle.stop();
        }
        let audio = voice_audio_from_parts(&audio_parts);
        // drain 桥接通道：段尾窗继续喂入（松开瞬间的语音不丢），让桥接线程干净退出。
        while let Some(event) = audio_rx.recv().await {
            match event {
                CaptureEvent::Window(SpeechEvent::Window {
                    samples,
                    abs_start,
                    speech_len,
                    ..
                }) if !cancelled && asr_failed.is_none() => {
                    let _ = window_tx.try_send(Some(WindowInput {
                        samples,
                        abs_start,
                        speech_len,
                    }));
                }
                CaptureEvent::Level(bytes) => self.send_level(bytes),
                _ => {}
            }
        }
        // 采集结束即释放麦克风（识别/优化收口纯计算，不必占用采集互斥）。
        let _ = mic
            .lock()
            .map(|mut arbiter| arbiter.release(options.mic_holder));

        if cancelled {
            asr_handle.cancel();
            if let Some(a) = audio.as_ref() {
                a.discard();
            }
            self.finish_cancelled(&options, &on_event);
            return;
        }
        if let Some(code) = asr_failed {
            asr_handle.cancel();
            self.finish_failed(&options, &on_event, &code);
            return;
        }

        // ── 识别收口：关流（发 None）→ 继续转发 Partial → Finished 得权威全量段 ──
        let recognizing = session_state(
            &options,
            VoiceInputStatus::Recognizing,
            "voice:status.recognizing",
        );
        self.replace_state(recognizing.clone());
        let _ = on_event.send(VoiceInputEvent::State(recognizing));
        let _ = window_tx.send(None);

        let raw = loop {
            tokio::select! {
                Some(control) = control_rx.recv() => {
                    if matches!(control, Control::Cancel) {
                        asr_handle.cancel();
                        if let Some(a) = audio.as_ref() {
                        a.discard();
                    }
                    self.finish_cancelled(&options, &on_event);
                        return;
                    }
                }
                event = asr_handle.recv() => {
                    match event {
                        // 收口阶段同样可能撞上说出口阈值：一次口述常在 10s 之内就松手了，
                        // 于是「等额度」这件事会跨过采集结束落在这一段。两处同一语义。
                        Some(JobEvent::AdmissionDelayed { waited_ms }) => {
                            eprintln!(
                                "[F2] 语音输入的转写腿已等本机会话额度 {waited_ms}ms 仍未开始（已停止采集，等待识别）。"
                            );
                            let state = session_state(
                                &options,
                                VoiceInputStatus::Recognizing,
                                "voice:status.admission_wait",
                            );
                            self.replace_state(state.clone());
                            let _ = on_event.send(VoiceInputEvent::State(state));
                        }
                        Some(JobEvent::Partial(PartialPayload::Asr { committed_delta, hypothesis, .. })) => {
                            forward_voice_partial(
                                &mut committed_text,
                                committed_delta,
                                hypothesis,
                                &options.glossary,
                                &on_event,
                            );
                        }
                        Some(JobEvent::Finished(result)) => match result.payload {
                            ResultPayload::Segments(segments) => {
                                break segments_to_transcript(&segments);
                            }
                            _ => {
                                self.finish_failed(&options, &on_event, "INVALID_PAYLOAD");
                                return;
                            }
                        },
                        Some(JobEvent::Failed(err)) => {
                            self.finish_failed(&options, &on_event, err.code());
                            return;
                        }
                        Some(JobEvent::Cancelled) => {
                            if let Some(a) = audio.as_ref() {
                        a.discard();
                    }
                    self.finish_cancelled(&options, &on_event);
                            return;
                        }
                        Some(_) => {}
                        None => {
                            self.finish_failed(&options, &on_event, "JOB_STREAM_ENDED");
                            return;
                        }
                    }
                }
            }
        };

        let raw = apply_glossary(&raw, &options.glossary);
        if raw.trim().is_empty() {
            // 无语音内容：回到空闲（与旧路径静音无 utterance 的语义一致）；录音也没有留的意义。
            if let Some(a) = audio.as_ref() {
                a.discard();
            }
            let state = session_state(&options, VoiceInputStatus::Idle, "voice:status.idle");
            self.finish_active(state.clone(), true);
            let _ = on_event.send(VoiceInputEvent::State(state));
            return;
        }

        // ── 优化/翻译（阶段回报，PRD-02-016）→ 落地/暂存（PRD-02-022）─────
        let report_stage = |key: &str| {
            let state = session_state(&options, VoiceInputStatus::Recognizing, key);
            self.replace_state(state.clone());
            let _ = on_event.send(VoiceInputEvent::State(state));
        };
        match refine_or_translate(&queue, raw, &options, &mut control_rx, &report_stage).await {
            PipelineOutcome::Text {
                raw,
                final_text,
                refined,
                output_kind,
            } => {
                // 落地前复查取消（PRD-02-005：取消绝不注入）。
                if matches!(control_rx.try_recv(), Ok(Control::Cancel)) {
                    if let Some(a) = audio.as_ref() {
                        a.discard();
                    }
                    self.finish_cancelled(&options, &on_event);
                    return;
                }
                let _ = on_event.send(VoiceInputEvent::Transcript {
                    raw: raw.clone(),
                    final_text: final_text.clone(),
                    refined,
                    output_kind,
                });
                if should_defer_landing_until_user_action(options.sink) {
                    // 结果先编辑再落地（PRD-02-022）：暂存待 `voice_input_land`。
                    self.store_pending_output(raw, final_text, output_kind, audio);
                    let mut state = session_state(
                        &options,
                        VoiceInputStatus::Idle,
                        "voice:status.ready_to_land",
                    );
                    state.result_pending = true;
                    self.finish_active(state.clone(), false);
                    let _ = on_event.send(VoiceInputEvent::State(state));
                } else {
                    let landing_state =
                        session_state(&options, VoiceInputStatus::Landing, "voice:status.landing");
                    self.replace_state(landing_state.clone());
                    let _ = on_event.send(VoiceInputEvent::State(landing_state));
                    let landing = VoiceTextLandingOptions {
                        inject_target: self.inject_target(),
                        ..VoiceTextLandingOptions::from(&options)
                    };
                    let landed = land_text(
                        &landing,
                        VoiceTextOutput {
                            raw,
                            final_text,
                            output_kind,
                            audio,
                        },
                        Some(&on_event),
                    )
                    .await;
                    let (status, mode, key) = match &landed {
                        LandTextOutcome::Injected => {
                            (VoiceInputStatus::Idle, "injected", "voice:status.injected")
                        }
                        LandTextOutcome::InjectedClipboardNotRestored => (
                            VoiceInputStatus::Idle,
                            "injected",
                            "voice:status.injected_clipboard_not_restored",
                        ),
                        LandTextOutcome::Copied => {
                            (VoiceInputStatus::Idle, "clipboard", "voice:status.copied")
                        }
                        LandTextOutcome::CopiedFallback => (
                            VoiceInputStatus::CopiedFallback,
                            "copied_fallback",
                            "voice:status.copied_fallback",
                        ),
                        LandTextOutcome::TargetChanged => (
                            VoiceInputStatus::CopiedFallback,
                            "copied_fallback",
                            "voice:status.target_changed",
                        ),
                        LandTextOutcome::QuickNote { .. } => (
                            VoiceInputStatus::Idle,
                            "quick_note",
                            "voice:status.saved_note",
                        ),
                        LandTextOutcome::FieldInput(sink) => (
                            VoiceInputStatus::Idle,
                            "field_input",
                            field_filled_key(*sink),
                        ),
                        LandTextOutcome::Failed(_) => {
                            (VoiceInputStatus::Error, "failed", "voice:status.error")
                        }
                    };
                    let landed_ok = !matches!(status, VoiceInputStatus::Error);
                    let mut state = session_state(&options, status, key);
                    if let LandTextOutcome::Failed(err) = landed {
                        state.message_detail = Some(err);
                    }
                    self.finish_active(state.clone(), false);
                    if landed_ok {
                        let _ = on_event.send(VoiceInputEvent::Landed { mode: mode.into() });
                    }
                    let _ = on_event.send(VoiceInputEvent::State(state));
                }
            }
            PipelineOutcome::Cancelled => {
                if let Some(a) = audio.as_ref() {
                    a.discard();
                }
                self.finish_cancelled(&options, &on_event);
            }
        }
    }

    /// 终态：取消（清 pending 输出 + 通知 HUD）。
    fn finish_cancelled(
        &self,
        options: &VoiceInputStartOptions,
        on_event: &Channel<VoiceInputEvent>,
    ) {
        let state = session_state(
            options,
            VoiceInputStatus::Cancelled,
            "voice:status.cancelled",
        );
        self.finish_active(state.clone(), true);
        let _ = on_event.send(VoiceInputEvent::State(state));
    }

    /// 终态：失败（错误码 → 稳定 message_key + `Failed` 事件）。
    fn finish_failed(
        &self,
        options: &VoiceInputStartOptions,
        on_event: &Channel<VoiceInputEvent>,
        code: &str,
    ) {
        let state = session_state(
            options,
            VoiceInputStatus::Error,
            voice_error_message_key(code),
        );
        self.finish_active(state.clone(), true);
        let _ = on_event.send(VoiceInputEvent::Failed { code: code.into() });
        let _ = on_event.send(VoiceInputEvent::State(state));
    }
}

/// 追加 committed 段到会话累计转写并回报 HUD（实时反馈，PRD-02-004）。
///
/// committed 永不回退（LocalAgreement-2），故只增量追加；hypothesis 每次全量替换
/// （空串 = 上一暂定已并入 committed）。词典替换与最终稿同规则，显示与结果一致。
fn forward_voice_partial(
    committed_text: &mut String,
    committed_delta: Vec<Segment>,
    hypothesis: String,
    glossary: &[GlossaryEntry],
    on_event: &Channel<VoiceInputEvent>,
) {
    let delta = apply_glossary_to_segments(committed_delta, glossary);
    let delta_text = segments_to_transcript(&delta);
    if !delta_text.is_empty() {
        if !committed_text.is_empty() {
            committed_text.push(' ');
        }
        committed_text.push_str(&delta_text);
    }
    let hypothesis = apply_glossary(&hypothesis, glossary);
    let _ = on_event.send(VoiceInputEvent::Partial {
        text: committed_text.clone(),
        hypothesis,
    });
}

fn control_targets_active(
    active_session_id: Option<&str>,
    requested_session_id: Option<&str>,
) -> bool {
    requested_session_id.is_none_or(|id| active_session_id == Some(id))
}

fn control_targets_holder(active_holder: MicHolder, requested_holder: MicHolder) -> bool {
    active_holder == requested_holder
}

fn should_queue_pending_control(active_exists: bool, requested_session_id: Option<&str>) -> bool {
    !active_exists && requested_session_id.is_some()
}

/// 以会话选项 + 状态 + i18n key 构造一个无引导文案的 [`VoiceInputState`]（去样板，单一来源）。
fn session_state(
    options: &VoiceInputStartOptions,
    status: VoiceInputStatus,
    message_key: impl Into<String>,
) -> VoiceInputState {
    VoiceInputState {
        status,
        landing_mode: options.landing_mode,
        input_mode: options.input_mode,
        auto_refine: options.auto_refine,
        translate_enabled: options.translate_enabled,
        translate_target_lang: options.translate_target_lang.to_string(),
        message_key: Some(message_key.into()),
        message_detail: None,
        result_pending: false,
    }
}

enum PipelineOutcome {
    Text {
        raw: String,
        final_text: String,
        refined: bool,
        output_kind: VoiceInputOutputKind,
    },
    Cancelled,
}

/// 优化/翻译后处理（识别已在流式会话内完成，`raw` 为词典替换后的权威转写）。
///
/// 阶段回报（PRD-02-016）：优化/翻译各阶段发独立 message_key，HUD 据此显示进度。
async fn refine_or_translate(
    queue: &JobQueue,
    raw: String,
    options: &VoiceInputStartOptions,
    control_rx: &mut mpsc::UnboundedReceiver<Control>,
    report_stage: &(dyn Fn(&str) + Send + Sync),
) -> PipelineOutcome {
    if options.translate_enabled && matches!(options.sink, VoiceInputSink::CursorOrClipboard) {
        report_stage("voice:status.translating");
        let mut translate = Job::new(
            JobKind::Translate {
                text: TextRef::Inline(raw.clone()),
                source: options.lang_hint.clone(),
                target: options.translate_target_lang.clone(),
                instruction: voice_translate_instruction(
                    luna_core::prompt::registry(),
                    options.translate_target_lang.clone(),
                    &raw,
                    &options.glossary,
                ),
                template: None,
            },
            JobOrigin::VoiceInput,
        );
        translate.priority = Priority::Interactive;
        let mut handle = queue.submit(translate);
        return match wait_for_text(&mut handle, control_rx).await {
            WaitJobOutcome::Text(text) if !text.trim().is_empty() => PipelineOutcome::Text {
                raw,
                final_text: text,
                refined: true,
                output_kind: VoiceInputOutputKind::Translated,
            },
            WaitJobOutcome::Cancelled => PipelineOutcome::Cancelled,
            _ => PipelineOutcome::Text {
                final_text: raw.clone(),
                raw,
                refined: false,
                output_kind: VoiceInputOutputKind::Raw,
            },
        };
    }

    if refine_policy(options.auto_refine, options.sink, options.refine_mode)
        == RefinePolicy::UseRawTranscript
    {
        return PipelineOutcome::Text {
            final_text: raw.clone(),
            raw,
            refined: false,
            output_kind: VoiceInputOutputKind::Raw,
        };
    }

    report_stage("voice:status.refining");
    let mut refine = Job::new(
        JobKind::Refine {
            text: TextRef::Inline(raw.clone()),
            instruction: voice_refine_instruction(
                luna_core::prompt::registry(),
                options.refine_mode,
                &raw,
                &options.glossary,
            ),
        },
        JobOrigin::VoiceInput,
    );
    refine.priority = Priority::Interactive;
    let mut handle = queue.submit(refine);
    match wait_for_text(&mut handle, control_rx).await {
        WaitJobOutcome::Text(text) if !text.trim().is_empty() => PipelineOutcome::Text {
            raw,
            final_text: text,
            refined: true,
            output_kind: VoiceInputOutputKind::Refined,
        },
        // 取消必须传播（用户主动放弃）。
        WaitJobOutcome::Cancelled => PipelineOutcome::Cancelled,
        // 可选的自动优化失败/空/类型错（如本机引擎未就绪）**绝不能丢掉已得的语音**——
        // 回退落地**原始转录**（PRD-02-010/011：优化是可选增强，失败退原文而非整体失败）。
        _ => PipelineOutcome::Text {
            final_text: raw.clone(),
            raw,
            refined: false,
            output_kind: VoiceInputOutputKind::Raw,
        },
    }
}

enum WaitJobOutcome {
    Text(String),
    Cancelled,
    /// 优化/翻译 Job 失败/流中断/载荷类型错（调用方一律回退原始转录，不读错误码）。
    Failed,
}

async fn wait_for_text(
    handle: &mut JobHandle,
    control_rx: &mut mpsc::UnboundedReceiver<Control>,
) -> WaitJobOutcome {
    loop {
        tokio::select! {
            Some(control) = control_rx.recv() => {
                if matches!(control, Control::Cancel) {
                    handle.cancel();
                    return WaitJobOutcome::Cancelled;
                }
            }
            event = handle.recv() => {
                match event {
                    Some(JobEvent::Finished(result)) => {
                        return match result.payload {
                            ResultPayload::Text(text) => WaitJobOutcome::Text(text),
                            _ => WaitJobOutcome::Failed,
                        };
                    }
                    Some(JobEvent::Failed(_)) => return WaitJobOutcome::Failed,
                    Some(JobEvent::Cancelled) => return WaitJobOutcome::Cancelled,
                    Some(_) => {}
                    None => return WaitJobOutcome::Failed,
                }
            }
        }
    }
}

// 注入与剪贴板落点在 macOS 与 Windows 实现（ADR-163 ③）。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
enum LandTextOutcome {
    Injected,
    /// 注入做成了，但借用剪贴板粘贴之后没能把用户原来的内容放回去（Windows 粘贴路径）。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    InjectedClipboardNotRestored,
    Copied,
    CopiedFallback,
    /// 注入前核对发现目标窗口已不在前台（切走了 / 关了 / 管理员窗口），改落剪贴板。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    TargetChanged,
    QuickNote {
        path: String,
    },
    /// 交给了应用内输入框（哪一个决定提示语）。
    FieldInput(VoiceInputSink),
    Failed(String),
}

/// 交给应用内输入框之后的提示语。
fn field_filled_key(sink: VoiceInputSink) -> &'static str {
    match sink {
        VoiceInputSink::ScriptDraft => "voice:status.script_filled",
        _ => "voice:status.popup_filled",
    }
}

/// 落地；没落成速记的会话，原始录音没有归宿，随手删掉（PRD-02-030 只留速记的）。
async fn land_text(
    options: &VoiceTextLandingOptions,
    output: VoiceTextOutput,
    on_event: Option<&Channel<VoiceInputEvent>>,
) -> LandTextOutcome {
    let audio = output.audio.clone();
    let outcome = land_text_inner(options, output, on_event).await;
    if !matches!(outcome, LandTextOutcome::QuickNote { .. }) {
        if let Some(a) = audio.as_ref() {
            a.discard();
        }
    }
    outcome
}

/// 存成速记（三个平台分支共用）。
fn land_quick_note(options: &VoiceTextLandingOptions, output: VoiceTextOutput) -> LandTextOutcome {
    match &options.save_quick_note {
        Some(save) => match save(output.into_quick_note_draft()) {
            Ok(path) => LandTextOutcome::QuickNote { path },
            Err(err) => LandTextOutcome::Failed(err),
        },
        None => LandTextOutcome::Failed("QUICK_NOTE_UNAVAILABLE: 速记笔记保存不可用".into()),
    }
}

// 非 macOS 分支提前返回，macOS 分支在其后。
#[cfg_attr(not(target_os = "macos"), allow(clippy::needless_return))]
async fn land_text_inner(
    options: &VoiceTextLandingOptions,
    output: VoiceTextOutput,
    on_event: Option<&Channel<VoiceInputEvent>>,
) -> LandTextOutcome {
    let text = output.final_text.clone();
    if is_field_sink(options.sink) {
        if let Some(session) = &options.popup_session {
            if let Ok(mut session) = session.lock() {
                session.set_input(text.clone());
            }
        }
        if let Some(on_event) = on_event {
            let _ = on_event.send(VoiceInputEvent::FieldInput { text });
        }
        return LandTextOutcome::FieldInput(options.sink);
    }

    // Windows（ADR-163 ③）：注入前核对目标窗口仍在前台 → 逐字输入 / 粘贴；核对不过或按键被拒
    // 都改落剪贴板，绝不打进别的窗口。判定在 `platform::windows::routing`（纯函数、有单测）。
    #[cfg(target_os = "windows")]
    {
        use crate::platform::windows::injector::{InjectFailure, WinTextInjector};
        use crate::platform::TextInjector as _;
        let injector = WinTextInjector;
        return match landing_decision(SessionTerminal::Finished, options.landing_mode) {
            LandingDecision::Clipboard => match injector.place_in_clipboard(&text) {
                Ok(()) => LandTextOutcome::Copied,
                Err(err) => LandTextOutcome::Failed(format!("CLIPBOARD_UNAVAILABLE: {err}")),
            },
            LandingDecision::QuickNote => land_quick_note(options, output),
            LandingDecision::Inject => {
                match injector
                    .inject_into(options.inject_target, text.clone())
                    .await
                {
                    Ok(report) if report.clipboard_intact() => LandTextOutcome::Injected,
                    Ok(_) => LandTextOutcome::InjectedClipboardNotRestored,
                    Err(failure) => {
                        // 注入不成 → 按决策降级剪贴板（PRD-02-021）。
                        if landing_failure_decision(options.landing_mode)
                            == LandingDecision::CopyFallback
                        {
                            let _ = injector.place_in_clipboard(&text);
                        }
                        match failure {
                            InjectFailure::Fallback(_) => LandTextOutcome::TargetChanged,
                            InjectFailure::SelectionChanged | InjectFailure::SendFailed => {
                                LandTextOutcome::CopiedFallback
                            }
                        }
                    }
                }
            }
            // Finished + 合法 mode 不会得到 Skip/CopyFallback；兜底不落地，保守不注入。
            LandingDecision::CopyFallback | LandingDecision::Skip => {
                LandTextOutcome::CopiedFallback
            }
        };
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return match landing_decision(SessionTerminal::Finished, options.landing_mode) {
            LandingDecision::QuickNote => land_quick_note(options, output),
            _ => {
                let _ = &text;
                LandTextOutcome::CopiedFallback
            }
        };
    }

    // 落地决策走 `landing_decision`/`landing_failure_decision`（纯逻辑、有单测的唯一真相源，
    // 不在此内联重复判定）。本函数仅在转写成功（SessionTerminal::Finished）后被调用。
    #[cfg(target_os = "macos")]
    {
        let injector = crate::platform::macos::MacTextInjector;
        match landing_decision(SessionTerminal::Finished, options.landing_mode) {
            LandingDecision::Clipboard => {
                let _ = injector.place_in_clipboard(&text);
                LandTextOutcome::Copied
            }
            LandingDecision::QuickNote => land_quick_note(options, output),
            LandingDecision::Inject => match injector.inject_at_cursor(text.clone()).await {
                Ok(()) => LandTextOutcome::Injected,
                // 注入失败（App 不支持/未授权）→ 按决策降级剪贴板（PRD-02-021）。
                Err(_)
                    if landing_failure_decision(options.landing_mode)
                        == LandingDecision::CopyFallback =>
                {
                    let _ = injector.place_in_clipboard(&text);
                    LandTextOutcome::CopiedFallback
                }
                Err(_) => LandTextOutcome::CopiedFallback,
            },
            // Finished + 合法 mode 不会得到 Skip/CopyFallback；兜底不落地，保守不注入。
            LandingDecision::CopyFallback | LandingDecision::Skip => {
                LandTextOutcome::CopiedFallback
            }
        }
    }
}

#[must_use]
pub fn voice_error_message_key(code: &str) -> String {
    format!("errors:{}", code.to_lowercase())
}

#[must_use]
pub fn voice_level_payload(bytes: Vec<u8>) -> RawBytes {
    RawBytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **语音输入这条腿也必须把「等待」与「拒绝」说出口**（ADR-078 ③）。
    ///
    /// 它与录音是**两条 HUD、两份文案表**：录音侧补了 `recording.json`，语音输入侧
    /// 走的是 `voice_error_message_key` → `voice-input.json`。少补一份的后果是——用户
    /// 按下语音输入之后先静默十几秒（没有等待信号），再拿到一句中性的「语音输入不可用」，
    /// 而这个码恰恰有一个明确可执行的下一步（等一下再试）。
    ///
    /// **它不是不可达**：麦克风硬互斥挡住的是「同时采集」，挡不住「上一场刚停、句柄还没
    /// 收干净」那个瞬态——正是会话池宽限期存在的那个理由。
    #[test]
    fn voice_input_speaks_up_while_waiting_for_admission_and_after_giving_up() {
        // 等待：被识别成等待，且**不是**终态（当成终态会把这次口述整个判失败）。
        let waiting = JobEvent::AdmissionDelayed { waited_ms: 12_000 };
        assert!(
            !matches!(
                waiting,
                JobEvent::Failed(_) | JobEvent::Finished(_) | JobEvent::Cancelled
            ),
            "等待不是终态"
        );

        // 拒绝：码可翻译——`voice_error_message_key` 产出的 key 必须在 HUD 的映射表里
        // 有对应项，否则会落到中性兜底。这里钉住 key 的形状；两份 locale 的存在性由
        // 前端 i18n 门禁（locale-parity / static-keys）守。
        let key = voice_error_message_key(
            luna_core::JobError::LocalSessionBusy { waited_ms: 20_000 }.code(),
        );
        assert_eq!(
            key, "errors:local_session_busy",
            "这个 key 必须与 VoiceInputHud 的 `case \"errors:local_session_busy\"` 逐字一致——\
             对不上时 HUD 静默回落到中性文案，而不会有任何东西变红"
        );

        // 兜底**不得**把裸码显示给用户：`finish_failed` 送出的 message_key 恒是
        // `errors:<小写码>` 这种内部形状，展示层永远经映射表，绝不直接渲染 code。
        assert!(
            !key.contains("LOCAL_SESSION_BUSY"),
            "面向 UI 的 key 里不该出现大写裸码：{key}"
        );
    }

    /// 联网指示必须覆盖**全部三个**麦克风表面（ADR-038 ④ / `PRD-04-060`）。
    ///
    /// 录音与实时字幕先做了，语音输入漏了近一批——而它恰恰是**最高频**的那条路径
    /// （单次几秒，一天几十次）。少一个表面，用户就会在那个表面上以为「没在联网」。
    ///
    /// 这条钉的是**事件本身存在且 log-safe**：判据（Provider 自报的位置）与另外两处
    /// 同源，文案也共用同一份 i18n key。
    #[test]
    fn voice_input_reports_whether_audio_leaves_the_device() {
        let event = VoiceInputEvent::Executor {
            display_name: Some("组织服务端".into()),
            leaves_device: true,
        };
        let json = serde_json::to_string(&event).expect("序列化");
        assert!(
            json.contains("\"leaves_device\":true"),
            "联网指示没有如实下发：{json}"
        );
        // 只有展示名与一个布尔，无正文——与录音那条同一条脱敏边界。
        for leak in ["transcript", "hypothesis", "committed"] {
            assert!(
                !json.contains(leak),
                "语音输入的执行者事件泄露了 `{leak}`：{json}"
            );
        }
    }
    use luna_core::{Segment, SegmentId};
    use std::time::Duration;

    fn segment(text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::ZERO,
            end: Duration::from_millis(500),
            text: text.to_string(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    #[test]
    fn voice_input_refines_when_auto_refine_is_enabled() {
        assert_eq!(
            refine_policy(
                true,
                VoiceInputSink::CursorOrClipboard,
                VoiceRefineMode::General
            ),
            RefinePolicy::RunRefine
        );
    }

    #[test]
    fn popup_voice_sink_never_runs_auto_refine() {
        assert_eq!(
            refine_policy(
                true,
                VoiceInputSink::PopupTranslateInput,
                VoiceRefineMode::Email
            ),
            RefinePolicy::UseRawTranscript
        );
    }

    #[test]
    fn script_dictation_is_a_raw_field_sink_that_lands_at_once() {
        assert_eq!(
            refine_policy(true, VoiceInputSink::ScriptDraft, VoiceRefineMode::General),
            RefinePolicy::UseRawTranscript
        );
        assert!(should_finish_after_landing(VoiceInputSink::ScriptDraft));
        assert!(!should_defer_landing_until_user_action(
            VoiceInputSink::ScriptDraft
        ));
        assert_eq!(
            field_filled_key(VoiceInputSink::ScriptDraft),
            "voice:status.script_filled"
        );
        assert_eq!(
            field_filled_key(VoiceInputSink::PopupTranslateInput),
            "voice:status.popup_filled"
        );
    }

    #[test]
    fn popup_voice_session_finishes_after_first_landing() {
        assert!(should_finish_after_landing(
            VoiceInputSink::PopupTranslateInput
        ));
        assert!(!should_finish_after_landing(
            VoiceInputSink::CursorOrClipboard
        ));
    }

    #[test]
    fn cursor_voice_output_is_deferred_until_user_lands_it() {
        assert!(should_defer_landing_until_user_action(
            VoiceInputSink::CursorOrClipboard
        ));
        assert!(!should_defer_landing_until_user_action(
            VoiceInputSink::PopupTranslateInput
        ));
    }

    #[test]
    fn voice_input_uses_raw_transcript_when_auto_refine_is_off() {
        assert_eq!(
            refine_policy(
                false,
                VoiceInputSink::CursorOrClipboard,
                VoiceRefineMode::General
            ),
            RefinePolicy::UseRawTranscript
        );
    }

    #[test]
    fn raw_voice_mode_skips_refine_even_when_auto_refine_is_on() {
        assert_eq!(
            refine_policy(
                true,
                VoiceInputSink::CursorOrClipboard,
                VoiceRefineMode::Raw
            ),
            RefinePolicy::UseRawTranscript
        );
    }

    #[test]
    fn quick_note_landing_mode_is_a_first_class_destination() {
        assert_eq!(
            LandingMode::from_setting(Some("quick_note".into())),
            LandingMode::QuickNote
        );
        assert_eq!(
            landing_decision(SessionTerminal::Finished, LandingMode::QuickNote),
            LandingDecision::QuickNote
        );
        assert_eq!(
            landing_failure_decision(LandingMode::QuickNote),
            LandingDecision::Skip
        );
    }

    #[test]
    fn voice_mode_refine_instruction_carries_style_and_glossary() {
        let instruction = voice_refine_instruction(
            &PromptRegistry::bundled(),
            VoiceRefineMode::Slack,
            "ship luna today",
            &[GlossaryEntry {
                term: "luna".into(),
                canonical: "Lunarca".into(),
            }],
        )
        .expect("slack mode should refine");
        assert!(instruction.contains("team chat"));
        assert!(instruction.contains("luna => Lunarca"));
        assert!(instruction.contains("Do not add or remove facts"));
    }

    #[test]
    fn voice_refine_instruction_preserves_source_language() {
        let instruction = voice_refine_instruction(
            &PromptRegistry::bundled(),
            VoiceRefineMode::General,
            "hello",
            &[],
        )
        .expect("general mode refines");
        assert!(
            instruction.contains("same language as the transcript"),
            "voice polish must not silently translate the user's dictated language"
        );
    }

    #[test]
    fn voice_translate_instruction_names_target_language() {
        let registry = PromptRegistry::bundled();
        let instruction = voice_translate_instruction(&registry, Lang::from("en"), "hello", &[])
            .expect("voice translation should provide a translation instruction");
        // 期望值从注册表现取（同 preset 用例的判据）：目标语言必须被真的代进模板。
        let expected = registry.render(
            PromptScope::Translate,
            "voice_instruction",
            &[("target_lang", "English")],
        );
        assert_eq!(instruction.as_str(), expected);
        assert!(
            instruction.contains("into English"),
            "目标语言必须以全称出现——小模型对 `into en` 不敏感（TDD-03 §3.3）"
        );
        assert!(
            !instruction.contains("{target_lang}"),
            "占位符没被替换——模型会收到一句字面写着 {{target_lang}} 的指令"
        );
        assert!(instruction.contains("Do not add or remove facts"));
    }

    #[test]
    fn glossary_applies_longest_first_and_preserves_unknown_text() {
        let entries = vec![
            GlossaryEntry {
                term: "luna".into(),
                canonical: "Lunarca".into(),
            },
            GlossaryEntry {
                term: "luna ai".into(),
                canonical: "Lunarca AI".into(),
            },
        ];
        assert_eq!(
            apply_glossary("today i used luna ai and luna", &entries),
            "today i used Lunarca AI and Lunarca"
        );
    }

    #[test]
    fn segments_join_to_single_transcript_without_logging_boundaries() {
        assert_eq!(
            segments_to_transcript(&[segment("hello"), segment("world")]),
            "hello world"
        );
    }

    #[test]
    fn cancelled_session_never_lands_text() {
        assert_eq!(
            landing_decision(SessionTerminal::Cancelled, LandingMode::Inject),
            LandingDecision::Skip
        );
    }

    #[test]
    fn injection_failure_falls_back_to_clipboard() {
        assert_eq!(
            landing_failure_decision(LandingMode::Inject),
            LandingDecision::CopyFallback
        );
    }

    #[test]
    fn popup_voice_start_options_use_popup_holder_and_raw_sink() {
        let options = VoiceInputStartOptions {
            landing_mode: LandingMode::Inject,
            input_mode: VoiceInputMode::Hold,
            auto_refine: true,
            refine_mode: VoiceRefineMode::General,
            translate_enabled: false,
            translate_target_lang: Lang::from("en"),
            glossary: Vec::new(),
            lang_hint: None,
            sink: VoiceInputSink::PopupTranslateInput,
            mic_holder: MicHolder::PopupVoiceButton,
            popup_session: None,
            save_quick_note: None,
            audio_capture_dir: None,
            vad: None,
            client_session_id: None,
            tuning: luna_audio::CaptureTuning::default(),
            asr_provider: None,
        };

        assert_eq!(options.sink, VoiceInputSink::PopupTranslateInput);
        assert_eq!(options.mic_holder, MicHolder::PopupVoiceButton);
        assert_eq!(
            refine_policy(options.auto_refine, options.sink, VoiceRefineMode::General),
            RefinePolicy::UseRawTranscript
        );
    }

    #[test]
    fn model_missing_error_uses_stable_message_key() {
        assert_eq!(
            voice_error_message_key("MODEL_MISSING"),
            "errors:model_missing"
        );
    }

    #[test]
    fn glossary_import_parses_tab_comma_and_single_column_lines() {
        let parsed = parse_glossary_import(
            "# 注释行\nsorbet\tSoRBET\nkube ctl,kubectl\nAnthropic\n\n  ,  \n",
        );
        assert_eq!(
            parsed,
            vec![
                GlossaryEntry {
                    term: "sorbet".into(),
                    canonical: "SoRBET".into()
                },
                GlossaryEntry {
                    term: "kube ctl".into(),
                    canonical: "kubectl".into()
                },
                GlossaryEntry {
                    term: "Anthropic".into(),
                    canonical: "Anthropic".into()
                },
            ]
        );
    }

    #[test]
    fn glossary_import_keeps_extra_delimiters_in_canonical() {
        let parsed = parse_glossary_import("foo,bar,baz");
        assert_eq!(
            parsed,
            vec![GlossaryEntry {
                term: "foo".into(),
                canonical: "bar,baz".into()
            }]
        );
    }

    /// PRD-02-016：优化/翻译阶段必须经 report_stage 发出独立 message_key（HUD 阶段反馈的事实源）。
    #[tokio::test]
    async fn pipeline_reports_refining_stage_via_report_stage() {
        use luna_core::{Capability, ProviderLocation};
        use luna_provider::{capability_set, MockProvider, MockScript, ProviderRegistry};

        let reg = ProviderRegistry::new();
        reg.register(Arc::new(MockProvider::new(
            "llm",
            capability_set(
                &[Capability::Refine, Capability::Translate],
                ProviderLocation::LocalService,
            ),
            MockScript::TextResult("Hello world.".into()),
        )));
        let queue = JobQueue::new(
            Arc::new(reg),
            luna_jobqueue::AdmissionCapacity::for_capture_tracks(2),
        );

        let stages: Arc<Mutex<Vec<String>>> = Arc::default();
        let report = {
            let stages = Arc::clone(&stages);
            move |key: &str| stages.lock().unwrap().push(key.to_string())
        };

        let options = VoiceInputStartOptions {
            landing_mode: LandingMode::Inject,
            input_mode: VoiceInputMode::Hold,
            auto_refine: true,
            refine_mode: VoiceRefineMode::General,
            translate_enabled: false,
            translate_target_lang: Lang::from("en"),
            glossary: Vec::new(),
            lang_hint: None,
            sink: VoiceInputSink::CursorOrClipboard,
            mic_holder: MicHolder::VoiceInput,
            popup_session: None,
            save_quick_note: None,
            audio_capture_dir: None,
            vad: None,
            client_session_id: None,
            tuning: luna_audio::CaptureTuning::default(),
            asr_provider: None,
        };
        let (_tx, mut control_rx) = mpsc::unbounded_channel();

        let outcome = refine_or_translate(
            &queue,
            "hello world".into(),
            &options,
            &mut control_rx,
            &report,
        )
        .await;

        assert!(matches!(
            outcome,
            PipelineOutcome::Text {
                refined: true,
                output_kind: VoiceInputOutputKind::Refined,
                ..
            }
        ));
        assert_eq!(
            stages.lock().unwrap().as_slice(),
            ["voice:status.refining"],
            "自动优化路径必须回报优化阶段"
        );
    }

    /// PRD-02-016：翻译输出路径回报 translating 阶段（且不再进入 refine）。
    #[tokio::test]
    async fn pipeline_reports_translating_stage_via_report_stage() {
        use luna_core::{Capability, ProviderLocation};
        use luna_provider::{capability_set, MockProvider, MockScript, ProviderRegistry};

        let reg = ProviderRegistry::new();
        reg.register(Arc::new(MockProvider::new(
            "llm",
            capability_set(
                &[Capability::Refine, Capability::Translate],
                ProviderLocation::LocalService,
            ),
            MockScript::TextResult("Hello world.".into()),
        )));
        let queue = JobQueue::new(
            Arc::new(reg),
            luna_jobqueue::AdmissionCapacity::for_capture_tracks(2),
        );

        let stages: Arc<Mutex<Vec<String>>> = Arc::default();
        let report = {
            let stages = Arc::clone(&stages);
            move |key: &str| stages.lock().unwrap().push(key.to_string())
        };

        let options = VoiceInputStartOptions {
            landing_mode: LandingMode::Inject,
            input_mode: VoiceInputMode::Hold,
            auto_refine: true,
            refine_mode: VoiceRefineMode::General,
            translate_enabled: true,
            translate_target_lang: Lang::from("en"),
            glossary: Vec::new(),
            lang_hint: None,
            sink: VoiceInputSink::CursorOrClipboard,
            mic_holder: MicHolder::VoiceInput,
            popup_session: None,
            save_quick_note: None,
            audio_capture_dir: None,
            vad: None,
            client_session_id: None,
            tuning: luna_audio::CaptureTuning::default(),
            asr_provider: None,
        };
        let (_tx, mut control_rx) = mpsc::unbounded_channel();

        let outcome = refine_or_translate(
            &queue,
            "你好世界".into(),
            &options,
            &mut control_rx,
            &report,
        )
        .await;

        assert!(matches!(
            outcome,
            PipelineOutcome::Text {
                output_kind: VoiceInputOutputKind::Translated,
                ..
            }
        ));
        assert_eq!(
            stages.lock().unwrap().as_slice(),
            ["voice:status.translating"],
            "翻译输出路径必须回报翻译阶段"
        );
    }

    #[test]
    fn glossary_merge_skips_existing_terms_and_dedupes_incoming() {
        let existing = vec![GlossaryEntry {
            term: "sorbet".into(),
            canonical: "手工维护".into(),
        }];
        let incoming = vec![
            GlossaryEntry {
                term: "sorbet".into(),
                canonical: "导入不覆盖".into(),
            },
            GlossaryEntry {
                term: "kubectl".into(),
                canonical: "kubectl".into(),
            },
            GlossaryEntry {
                term: "kubectl".into(),
                canonical: "重复".into(),
            },
        ];
        let (merged, imported, skipped) = merge_glossary_entries(existing, incoming);
        assert_eq!(imported, 1);
        assert_eq!(skipped, 2);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].canonical, "手工维护");
        assert_eq!(merged[1].term, "kubectl");
    }

    #[test]
    fn transcript_feedback_event_exposes_raw_and_final_text_for_hud_only() {
        let event = VoiceInputEvent::Transcript {
            raw: "hello world".into(),
            final_text: "Hello world.".into(),
            refined: true,
            output_kind: VoiceInputOutputKind::Refined,
        };

        let json = serde_json::to_value(event).expect("serializes voice input event");
        assert_eq!(json["kind"], "transcript");
        assert_eq!(json["data"]["raw"], "hello world");
        assert_eq!(json["data"]["final_text"], "Hello world.");
        assert_eq!(json["data"]["refined"], true);
        assert_eq!(json["data"]["output_kind"], "refined");
    }

    #[test]
    fn controls_without_session_target_legacy_active_session() {
        assert!(control_targets_active(Some("active"), None));
    }

    #[test]
    fn controls_with_matching_session_target_active_session() {
        assert!(control_targets_active(Some("active"), Some("active")));
    }

    #[test]
    fn controls_with_stale_session_do_not_target_active_session() {
        assert!(!control_targets_active(Some("active"), Some("stale")));
    }

    #[test]
    fn early_control_with_session_is_queued_until_start_consumes_it() {
        assert!(should_queue_pending_control(false, Some("session")));
        assert!(!should_queue_pending_control(true, Some("session")));
        assert!(!should_queue_pending_control(false, None));
    }

    #[test]
    fn holder_scoped_control_only_targets_matching_active_holder() {
        assert!(control_targets_holder(
            MicHolder::PopupVoiceButton,
            MicHolder::PopupVoiceButton
        ));
        assert!(!control_targets_holder(
            MicHolder::VoiceInput,
            MicHolder::PopupVoiceButton
        ));
    }
}

#[cfg(test)]
mod hud_preset_tests {
    use super::*;

    #[test]
    fn a_fresh_session_reads_the_persisted_default_every_time() {
        // 上一次会话里用户临时改成剪贴板，不该变成下一次的默认（TDD-07 §2.3）。
        let c = VoiceInputController::default();
        c.prepare_session(HudPreset::Current, LandingMode::Inject)
            .unwrap();
        c.set_mode(
            LandingMode::Clipboard,
            VoiceInputMode::Hold,
            true,
            false,
            "en".into(),
        );
        c.clear_session_preset(); // 会话结束
        let next = c
            .prepare_session(HudPreset::Current, LandingMode::Inject)
            .unwrap();
        assert_eq!(
            next.landing_mode,
            LandingMode::Inject,
            "遗留运行态不该被继承"
        );
    }

    #[test]
    fn the_quick_note_entry_overrides_this_session_only() {
        let c = VoiceInputController::default();
        let s = c
            .prepare_session(HudPreset::QuickNote, LandingMode::Inject)
            .unwrap();
        assert_eq!(s.landing_mode, LandingMode::QuickNote);
        c.clear_session_preset();
        // 下一次普通入口回到持久默认——预设从不写设置。
        let next = c
            .prepare_session(HudPreset::Current, LandingMode::Inject)
            .unwrap();
        assert_eq!(next.landing_mode, LandingMode::Inject);
    }

    #[test]
    fn re_invoking_the_same_idle_session_keeps_what_the_user_picked() {
        // 用户在 HUD 里选好落点、还没开口就又按了一次快捷键——他刚选的不该被抹掉。
        let c = VoiceInputController::default();
        c.prepare_session(HudPreset::QuickNote, LandingMode::Inject)
            .unwrap();
        c.set_mode(
            LandingMode::Clipboard,
            VoiceInputMode::Hold,
            true,
            false,
            "en".into(),
        );
        let again = c
            .prepare_session(HudPreset::Current, LandingMode::Inject)
            .unwrap();
        assert_eq!(
            again.landing_mode,
            LandingMode::Clipboard,
            "同一个 idle 会话被再点一下，用户的选择必须留着"
        );
    }

    #[tokio::test]
    async fn landing_the_same_result_as_a_quick_note_twice_saves_only_once() {
        // 真机 2026-09-03：存完速记后 HUD 仍留着「存入速记笔记」，再点一次就多一篇。
        let c = VoiceInputController::default();
        c.prepare_session(HudPreset::QuickNote, LandingMode::QuickNote)
            .unwrap();
        c.store_pending_output("x".into(), "x".into(), VoiceInputOutputKind::Raw, None);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let saver: SaveQuickNoteFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_draft: VoiceQuickNoteDraft| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok("QuickNotes/x.md".to_owned())
            })
        };
        let first = c
            .land_pending(LandingMode::QuickNote, Some(saver.clone()), None)
            .await
            .unwrap();
        let second = c
            .land_pending(LandingMode::QuickNote, Some(saver), None)
            .await
            .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(first.message_detail.as_deref(), Some("QuickNotes/x.md"));
        assert_eq!(second.message_detail.as_deref(), Some("QuickNotes/x.md"));
        assert!(matches!(second.status, VoiceInputStatus::Idle));
    }

    /// 「关掉会不会丢结果」由 Rust 判定（`result_pending`）：出结果即为真，任一去处落地成功、
    /// 或明确丢弃后为假；落地失败仍为真（结果还在、可以重试）。
    #[tokio::test]
    async fn result_pending_tracks_whether_closing_would_lose_the_result() {
        let c = VoiceInputController::default();
        c.prepare_session(HudPreset::QuickNote, LandingMode::QuickNote)
            .unwrap();
        c.store_pending_output("x".into(), "x".into(), VoiceInputOutputKind::Raw, None);
        c.replace_state(VoiceInputState {
            result_pending: true,
            ..c.state()
        });
        let failing: SaveQuickNoteFn = Arc::new(|_draft| Err("VAULT_IO: 磁盘已满".to_owned()));
        let failed = c
            .land_pending(LandingMode::QuickNote, Some(failing), None)
            .await
            .unwrap();
        assert!(failed.result_pending, "落地失败：结果还在，关掉仍会丢");
        let saver: SaveQuickNoteFn = Arc::new(|_draft| Ok("QuickNotes/x.md".to_owned()));
        let landed = c
            .land_pending(LandingMode::QuickNote, Some(saver), None)
            .await
            .unwrap();
        assert!(!landed.result_pending, "存成速记之后关掉不算丢");

        c.store_pending_output("y".into(), "y".into(), VoiceInputOutputKind::Raw, None);
        c.replace_state(VoiceInputState {
            result_pending: true,
            ..c.state()
        });
        c.discard_pending_output();
        assert!(!c.state().result_pending);
        assert!(
            c.prepare_session(HudPreset::Current, LandingMode::Inject)
                .is_ok(),
            "丢弃之后不再报忙"
        );
    }

    #[test]
    fn a_pending_result_makes_the_session_busy_and_the_original_is_untouched() {
        // 审阅期间从另一个入口请求新 preset：报忙，且**不改**原会话的落点。
        let c = VoiceInputController::default();
        c.prepare_session(HudPreset::Current, LandingMode::Inject)
            .unwrap();
        if let Ok(mut inner) = c.inner.lock() {
            inner.pending_output = Some(PendingVoiceOutput {
                raw: "x".into(),
                final_text: "x".into(),
                output_kind: VoiceInputOutputKind::Raw,
                audio: None,
                landed_note: None,
                landed: false,
            });
        }
        let err = c
            .prepare_session(HudPreset::QuickNote, LandingMode::Inject)
            .unwrap_err();
        assert!(err.starts_with("VOICE_SESSION_BUSY"), "{err}");
        assert_eq!(
            c.state().landing_mode,
            LandingMode::Inject,
            "报忙时绝不能顺手改掉原会话的落点"
        );
    }
}
