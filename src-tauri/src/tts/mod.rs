//! 文本朗读（TTS）控制器（TDD-05 §9，播放型系统引擎，**不走 JobQueue**）。
//!
//! 定位（ADR-022）：朗读是**确定性系统播放服务**（macOS `AVSpeechSynthesizer`），产出的是实时
//! 播放而非可归档的结构化 `JobResult`——故与 `MediaServer`（回放）、`luna-audio`（采集）同属 AI
//! 主干之外的引擎/播放层，**不套用 `AIProvider + JobQueue + ResultSchema`**（`ResultPayload` 无
//! Audio 变体，硬塞会污染「result 只面向 Segments/Text/Chat 供导出」的 DRY 契约）。未来的「声音
//! 复刻 + 神经 TTS 生成音频文件」（PRD-10-060，规划、暂不实施）才是产出音频工件的 AI 动作，届时
//! 作为 `Capability::Speak` provider 走 JobQueue，本控制器负责其播放——此刻不加任何死代码。
//!
//! 结构照抄 [`crate::recording::RecordingController`]：`Mutex<Inner>` 持会话状态 + 单一发声者
//! （跨 HUD/编辑器多窗口共享，天然抢占——新朗读停旧朗读）。合成器**主线程亲和、非 `Send`**，本
//! 控制器经 Tauri `run_on_main_thread` 把 speak/pause/stop 编排到主线程，delegate 回调（主线程）经
//! 注入闭包推进下一句并广播状态事件 `tts://state`（低频、逐句，非高频流）。

pub mod plan;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

pub use plan::{detect_sample, plan_speech, SpeechSegment};

/// 朗读播放态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TtsStatus {
    /// 空闲（无朗读）。
    Idle,
    /// 朗读中。
    Speaking,
    /// 已暂停。
    Paused,
}

/// 朗读状态快照——同时用作 `tts_*` 命令返回值与 `tts://state` 广播事件负载（DRY）。
///
/// `session_id = 0` / `scope = ""` 表示空闲。各前端表面记住自己 `tts_speak` 返回的 `session_id`，
/// 据此判断「当前发声者是不是我」（不是我即复位按钮，实现跨窗口单一发声者抢占）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "tts://state")]
pub struct TtsStateChanged {
    /// 播放态。
    pub status: TtsStatus,
    /// 会话 id（`0` = 空闲；从 1 起单调递增）。
    pub session_id: u32,
    /// 发起表面标识（如 `hud:source` / `hud:translation` / `editor:<path>`；`""` = 空闲）。
    pub scope: String,
    /// 当前发声单元序号（0 基）。
    pub index: u32,
    /// 发声单元总数。
    pub total: u32,
    /// 当前句在源串中的 UTF-16 起（供编辑器逐句高亮，与 CodeMirror 位置同口径）。
    pub range_from: u32,
    /// 当前句在源串中的 UTF-16 止。
    pub range_to: u32,
}

impl TtsStateChanged {
    fn idle() -> Self {
        Self {
            status: TtsStatus::Idle,
            session_id: 0,
            scope: String::new(),
            index: 0,
            total: 0,
            range_from: 0,
            range_to: 0,
        }
    }
}

/// 进行中的朗读会话（内部态）。
struct PlayingSession {
    id: u32,
    scope: String,
    segments: Vec<SpeechSegment>,
    /// 当前正在朗读的句索引。
    index: usize,
    /// 会话语言（BCP-47 化，供每句选嗓音）；`None` = 用系统默认嗓音。
    voice_lang: Option<String>,
    rate: f32,
    paused: bool,
    /// 主线程闭包是否已真正开播本会话。**上一会话遗留的 `DidFinish` 会在此之前到达**——
    /// 不门控则会把新会话的 `index` 误推进一格（跳掉首句、且高亮与实际发声错位）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    started: bool,
    /// 暂停期间当前句恰好读完：合成器队列已空，`resume` 必须**重新 speak** 下一句——
    /// 此时 `continueSpeaking` 无事可续，会让朗读永久卡死（只能靠停止复位）。
    resume_needs_speak: bool,
}

impl PlayingSession {
    fn snapshot(&self) -> TtsStateChanged {
        let seg = self.segments.get(self.index);
        TtsStateChanged {
            status: if self.paused {
                TtsStatus::Paused
            } else {
                TtsStatus::Speaking
            },
            session_id: self.id,
            scope: self.scope.clone(),
            index: u32::try_from(self.index).unwrap_or(0),
            total: u32::try_from(self.segments.len()).unwrap_or(0),
            range_from: seg.map_or(0, |s| s.start_utf16),
            range_to: seg.map_or(0, |s| s.end_utf16),
        }
    }
}

#[derive(Default)]
struct TtsInner {
    session: Option<PlayingSession>,
}

/// `resume` 的恢复动作（在锁内判定、锁外执行，避免持锁调用平台层）。
enum ResumeAction {
    /// 无进行中的暂停会话，什么也不做。
    Nothing,
    /// 队列中仍有被暂停的句：续播。
    Continue,
    /// 暂停期间当前句已读完：重新朗读下一句（文本 / 语言 / 语速）。
    Fresh(String, Option<String>, f32),
}

/// 跨线程共享态（`Arc` 单例，进程生命周期；delegate 回调与命令方法共用）。
struct TtsShared {
    inner: Mutex<TtsInner>,
    seq: AtomicU32,
}

/// 文本朗读控制器（`AppState.tts`，`app.manage` 单例）。
pub struct TtsController {
    shared: Arc<TtsShared>,
}

impl Default for TtsController {
    fn default() -> Self {
        Self::new()
    }
}

impl TtsController {
    /// 构造（无 `AppHandle`——与录音控制器同，`AppHandle` 由命令方法按需传入）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(TtsShared {
                inner: Mutex::new(TtsInner::default()),
                seq: AtomicU32::new(0),
            }),
        }
    }

    /// 本平台是否支持朗读（仅 macOS 有原生实现；其它平台前端据此隐藏/禁用入口）。
    #[must_use]
    pub fn available() -> bool {
        cfg!(target_os = "macos")
    }

    /// 开始朗读已规划好的发声单元序列（断句/剥 Markdown 由命令层 [`plan_speech`] 完成——
    /// 控制器只负责播放状态机，职责单一）。
    ///
    /// `voice_lang` 已在命令层解析为 BCP-47（`None` = 用系统默认嗓音）。`rate_pct` 为语速百分比
    /// （50–200，100=常速）。返回新会话快照（`status=speaking`），前端据 `session_id` 认领。
    pub fn speak(
        &self,
        app: &AppHandle,
        segments: Vec<SpeechSegment>,
        voice_lang: Option<String>,
        scope: String,
        rate_pct: i64,
    ) -> TtsStateChanged {
        // 平台不支持：绝不建会话——否则永无 `DidFinish`，UI 会挂在「朗读中」永不收尾。
        if !Self::available() {
            return TtsStateChanged::idle();
        }
        // 无可朗读内容（纯符号/空白）：**不动**当前发声者。此前这里调 `stop`，导致在 A 处点一段
        // 纯符号会顺手掐掉 B 处正在进行的朗读。
        if segments.is_empty() {
            return self.state();
        }
        let rate = map_rate(rate_pct);
        let id = self.shared.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let first = segments[0].text.clone();
        let snapshot = {
            let mut inner = self.shared.inner.lock().expect("tts lock");
            inner.session = Some(PlayingSession {
                id,
                scope,
                segments,
                index: 0,
                voice_lang: voice_lang.clone(),
                rate,
                paused: false,
                started: false,
                resume_needs_speak: false,
            });
            inner
                .session
                .as_ref()
                .map_or_else(TtsStateChanged::idle, PlayingSession::snapshot)
        };
        // 先广播再开播：`emit` 是同步的，若放在 `drive_start` 之后，首句瞬间读完的
        // `DidFinish` 快照可能先到、再被这条陈旧快照覆盖成 index=0（UI 回退）。
        emit(app, &snapshot);
        drive_start(app, &self.shared, id, first, voice_lang, rate);
        snapshot
    }

    /// 暂停当前朗读。
    pub fn pause(&self, app: &AppHandle) -> TtsStateChanged {
        let snapshot = {
            let mut inner = self.shared.inner.lock().expect("tts lock");
            match inner.session.as_mut() {
                Some(session) if !session.paused => {
                    session.paused = true;
                    session.snapshot()
                }
                Some(session) => session.snapshot(),
                None => TtsStateChanged::idle(),
            }
        };
        if snapshot.status == TtsStatus::Paused {
            drive_pause(app);
            emit(app, &snapshot);
        }
        snapshot
    }

    /// 继续朗读。
    ///
    /// 两条恢复路径：**队列仍有暂停中的句** → `continueSpeaking` 续播；**暂停期间当前句已读完**
    /// （`resume_needs_speak`）→ 队列已空，必须重新 `speak` 下一句，否则续播无事可做、永久卡死。
    pub fn resume(&self, app: &AppHandle) -> TtsStateChanged {
        let (snapshot, action) = {
            let mut inner = self.shared.inner.lock().expect("tts lock");
            match inner.session.as_mut() {
                Some(session) if session.paused => {
                    session.paused = false;
                    let action = if std::mem::take(&mut session.resume_needs_speak) {
                        session.segments.get(session.index).map_or(
                            ResumeAction::Continue,
                            |segment| {
                                ResumeAction::Fresh(
                                    segment.text.clone(),
                                    session.voice_lang.clone(),
                                    session.rate,
                                )
                            },
                        )
                    } else {
                        ResumeAction::Continue
                    };
                    (session.snapshot(), action)
                }
                Some(session) => (session.snapshot(), ResumeAction::Nothing),
                None => (TtsStateChanged::idle(), ResumeAction::Nothing),
            }
        };
        match action {
            ResumeAction::Continue => {
                drive_resume(app);
                emit(app, &snapshot);
            }
            ResumeAction::Fresh(text, lang, rate) => {
                drive_speak(app, text, lang, rate);
                emit(app, &snapshot);
            }
            ResumeAction::Nothing => {}
        }
        snapshot
    }

    /// 停止朗读并复位空闲。
    pub fn stop(&self, app: &AppHandle) -> TtsStateChanged {
        let had_session = {
            let mut inner = self.shared.inner.lock().expect("tts lock");
            inner.session.take().is_some()
        };
        drive_stop(app);
        let snapshot = TtsStateChanged::idle();
        if had_session {
            emit(app, &snapshot);
        }
        snapshot
    }

    /// 调整当前会话语速（百分比 50–200）。对**后续句**即时生效（当前句读完后换速），
    /// 当前正在播报的句不打断（AVSpeechUtterance 速率播报中不可改）。无进行中会话则忽略。
    pub fn set_rate(&self, rate_pct: i64) {
        let mut inner = self.shared.inner.lock().expect("tts lock");
        if let Some(session) = inner.session.as_mut() {
            session.rate = map_rate(rate_pct);
        }
    }

    /// 当前状态快照（`tts_state` 命令）。
    #[must_use]
    pub fn state(&self) -> TtsStateChanged {
        let inner = self.shared.inner.lock().expect("tts lock");
        inner
            .session
            .as_ref()
            .map_or_else(TtsStateChanged::idle, PlayingSession::snapshot)
    }
}

/// delegate `DidFinish` 回调：推进到下一句或收尾（**在主线程执行**）。
#[cfg(target_os = "macos")]
fn on_utterance_finished(shared: &Arc<TtsShared>, app: &AppHandle) {
    let (next, snapshot) = {
        let mut inner = shared.inner.lock().expect("tts lock");
        let Some(session) = inner.session.as_mut() else {
            return; // 已被 stop/替换清空——忽略陈旧回调
        };
        // 新会话尚未开播：本回调属**上一会话**的遗留句（新 speak 已换掉 session，但主线程的
        // stop+speak 闭包还没跑）。推进它会跳掉新会话首句并让高亮与实际发声错位。
        if !session.started {
            return;
        }
        session.index += 1;
        if session.index < session.segments.len() {
            if session.paused {
                // 暂停恰好落在句边界：当前句已读完、队列已空。此处**只推进不开播**，并标记
                // resume 时须重新 speak——否则 `continueSpeaking` 无事可续，朗读永久卡死。
                session.resume_needs_speak = true;
                (None, session.snapshot())
            } else {
                let seg = &session.segments[session.index];
                (
                    Some((seg.text.clone(), session.voice_lang.clone(), session.rate)),
                    session.snapshot(),
                )
            }
        } else {
            inner.session = None;
            (None, TtsStateChanged::idle())
        }
    };
    if let Some((text, lang, rate)) = next {
        // 经主线程派发而非直接调用：`AVSpeechSynthesizerDelegate` 的回调线程未由 Apple 文档
        // 保证，而平台层合成器是主线程 `thread_local`——若回调不在主线程，直接调用会静默 no-op
        // （读完首句即永久停住）。主线程上 `run_on_main_thread` 内联执行，无额外延迟。
        drive_speak(app, text, lang, rate);
    }
    emit(app, &snapshot);
}

/// 把一次平台调用派发到主线程。**失败必须出声**：静默吞掉派发错误会让控制器状态与合成器
/// 永久脱钩（UI 显示朗读中却无声），且无自愈路径。日志只含动作名，绝不含朗读正文（TDD-12）。
#[cfg(target_os = "macos")]
fn dispatch_main<F: FnOnce() + Send + 'static>(app: &AppHandle, action: &'static str, task: F) {
    if let Err(err) = app.run_on_main_thread(task) {
        eprintln!("[TTS] 主线程派发失败 action={action} err={err}");
    }
}

/// 语速百分比（50–200，100=常速）→ `AVSpeechUtterance.rate`。
///
/// **分段非线性刻意为之**：`AVSpeechUtteranceDefaultSpeechRate` = 0.5 才是「常速」，而 rate 轴
/// 0.5→1.0 的听感跨度约 4 倍速（1.0 已基本听不清）。若线性把 200% 映到 1.0，用户选「2×」得到的
/// 是一段无法理解的高速语音。故以 0.5 为锚点两段映射：50%→0.25、100%→0.5、200%→0.75。
fn map_rate(pct: i64) -> f32 {
    /// `AVSpeechUtteranceDefaultSpeechRate`（常速锚点）。
    const DEFAULT_RATE: f32 = 0.5;
    /// 100%→200% 区间允许的 rate 增量（压缩高速区，保可懂度）。
    const FAST_SPAN: f32 = 0.25;
    let pct = pct.clamp(50, 200) as f32;
    if pct <= 100.0 {
        DEFAULT_RATE * pct / 100.0
    } else {
        DEFAULT_RATE + (pct - 100.0) / 100.0 * FAST_SPAN
    }
}

/// 广播状态事件（失败不致命——UI 下次事件自愈）。
fn emit(app: &AppHandle, snapshot: &TtsStateChanged) {
    let _ = snapshot.clone().emit(app);
}

// —— 平台驱动（主线程编排；非 macOS 为诚实空实现，配合 `available()==false`）——

#[cfg(target_os = "macos")]
fn drive_start(
    app: &AppHandle,
    shared: &Arc<TtsShared>,
    session_id: u32,
    first_text: String,
    voice_lang: Option<String>,
    rate: f32,
) {
    let shared_cb = shared.clone();
    let app_cb = app.clone();
    let callback: Box<dyn Fn(crate::platform::macos::tts::SpeechDelegateEvent) + Send + 'static> =
        Box::new(move |event| {
            if event == crate::platform::macos::tts::SpeechDelegateEvent::DidFinish {
                on_utterance_finished(&shared_cb, &app_cb);
            }
        });
    let shared_mark = shared.clone();
    dispatch_main(app, "start", move || {
        crate::platform::macos::tts::ensure(callback);
        crate::platform::macos::tts::stop();
        // 本闭包排队期间可能已被更晚的 speak 取代（连点 / 换表面）：只有仍是当前会话才开播，
        // 并在**同一临界区**内置 `started`——保证「已开播」与「可推进」严格同时生效。
        let still_current = shared_mark
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| {
                inner.session.as_mut().map(|session| {
                    let current = session.id == session_id;
                    if current {
                        session.started = true;
                    }
                    current
                })
            })
            .unwrap_or(false);
        if still_current {
            crate::platform::macos::tts::speak(&first_text, voice_lang.as_deref(), rate);
        }
    });
}

/// 从任意线程朗读一句（`resume` 在暂停跨句后重新开播用）。
#[cfg(target_os = "macos")]
fn drive_speak(app: &AppHandle, text: String, lang: Option<String>, rate: f32) {
    dispatch_main(app, "speak", move || {
        crate::platform::macos::tts::speak(&text, lang.as_deref(), rate);
    });
}

#[cfg(target_os = "macos")]
fn drive_pause(app: &AppHandle) {
    dispatch_main(app, "pause", crate::platform::macos::tts::pause);
}

#[cfg(target_os = "macos")]
fn drive_resume(app: &AppHandle) {
    dispatch_main(app, "resume", crate::platform::macos::tts::resume);
}

#[cfg(target_os = "macos")]
fn drive_stop(app: &AppHandle) {
    dispatch_main(app, "stop", crate::platform::macos::tts::stop);
}

#[cfg(not(target_os = "macos"))]
fn drive_start(
    _app: &AppHandle,
    _shared: &Arc<TtsShared>,
    _session_id: u32,
    _first_text: String,
    _voice_lang: Option<String>,
    _rate: f32,
) {
}

#[cfg(not(target_os = "macos"))]
fn drive_speak(_app: &AppHandle, _text: String, _lang: Option<String>, _rate: f32) {}

#[cfg(not(target_os = "macos"))]
fn drive_pause(_app: &AppHandle) {}

#[cfg(not(target_os = "macos"))]
fn drive_resume(_app: &AppHandle) {}

#[cfg(not(target_os = "macos"))]
fn drive_stop(_app: &AppHandle) {}

/// 把语言代码规整为 `AVSpeechSynthesisVoice.voiceWithLanguage:` 可用的 BCP-47（区域化）。
///
/// 已含区域（带 `-`）的原样传；仅主子标签的按常见区域补全（`zh`→`zh-CN`、`en`→`en-US`…），
/// 否则原样返回（不静默丢信息，系统会回退默认嗓音）。
#[must_use]
pub fn bcp47_for_voice(code: &str) -> String {
    if code.contains('-') {
        return code.to_string();
    }
    match code.to_ascii_lowercase().as_str() {
        "zh" => "zh-CN",
        "en" => "en-US",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "fr" => "fr-FR",
        "de" => "de-DE",
        "es" => "es-ES",
        "pt" => "pt-BR",
        "ru" => "ru-RU",
        "it" => "it-IT",
        "ar" => "ar-SA",
        "hi" => "hi-IN",
        "th" => "th-TH",
        "vi" => "vi-VN",
        "id" => "id-ID",
        "nl" => "nl-NL",
        "pl" => "pl-PL",
        "tr" => "tr-TR",
        _ => code,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_maps_percent_to_av_range() {
        // 100% 必须落在 AVSpeechUtteranceDefaultSpeechRate（0.5）——「常速」的定义。
        assert!((map_rate(100) - 0.5).abs() < f32::EPSILON);
        assert!((map_rate(50) - 0.25).abs() < f32::EPSILON);
        // 高速区被压缩：200% → 0.75 而非 1.0。rate 1.0 约 4 倍速、基本听不清，
        // 线性映射会让用户选「2×」得到无法理解的语音。
        assert!((map_rate(200) - 0.75).abs() < f32::EPSILON);
        assert!(map_rate(200) < 1.0, "绝不映到听不清的 rate 上限");
        // 单调递增。
        assert!(map_rate(50) < map_rate(100));
        assert!(map_rate(100) < map_rate(150));
        assert!(map_rate(150) < map_rate(200));
        // 越界钳制。
        assert!((map_rate(10) - 0.25).abs() < f32::EPSILON);
        assert!((map_rate(500) - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn bcp47_regionalizes_primary_only() {
        assert_eq!(bcp47_for_voice("zh"), "zh-CN");
        assert_eq!(bcp47_for_voice("en"), "en-US");
        assert_eq!(bcp47_for_voice("en-GB"), "en-GB"); // 已含区域原样
        assert_eq!(bcp47_for_voice("xx"), "xx"); // 未知原样
    }

    #[test]
    fn idle_snapshot_is_empty() {
        let controller = TtsController::new();
        let snap = controller.state();
        assert_eq!(snap.status, TtsStatus::Idle);
        assert_eq!(snap.session_id, 0);
        assert_eq!(snap.total, 0);
    }

    // —— 状态机回归（验收发现，2026-07-24）。这些路径无法在无 AppHandle 的单测里跑完整
    // 平台驱动，故直接对 `PlayingSession` 的推进语义建模断言（与 `on_utterance_finished`
    // / `resume` 中的同一套判定规则一一对应）。

    fn session_fixture(total: usize) -> PlayingSession {
        PlayingSession {
            id: 1,
            scope: "test".to_string(),
            segments: (0..total)
                .map(|i| SpeechSegment {
                    text: format!("句{i}"),
                    start_utf16: 0,
                    end_utf16: 1,
                })
                .collect(),
            index: 0,
            voice_lang: None,
            rate: 0.5,
            paused: false,
            started: false,
            resume_needs_speak: false,
        }
    }

    #[test]
    fn unstarted_session_must_not_advance() {
        // 上一会话遗留的 DidFinish 到达时，新会话 started=false —— 绝不能推进，
        // 否则新朗读会跳掉首句、且高亮与实际发声错位。
        let session = session_fixture(3);
        assert!(!session.started, "新建会话未开播");
        // `on_utterance_finished` 的门控条件即 `!started -> return`。
        assert_eq!(session.index, 0, "未开播时 index 必须保持 0");
    }

    #[test]
    fn pause_at_sentence_boundary_flags_resume_needs_speak() {
        // 暂停恰好落在句边界：当前句已读完、合成器队列已空。若此时只是「忽略推进」，
        // resume 的 continueSpeaking 无事可续 → 永久卡死。正确行为 = 推进并标记需重新开播。
        let mut session = session_fixture(3);
        session.started = true;
        session.paused = true;

        session.index += 1;
        assert!(session.index < session.segments.len());
        session.resume_needs_speak = true; // on_utterance_finished 在 paused 分支所做

        assert_eq!(session.index, 1, "暂停期间也必须推进，不能丢弃这次完成");
        assert!(
            session.resume_needs_speak,
            "resume 必须走重新 speak 而非续播"
        );

        // resume：取出标记 → 重新朗读 segments[index]
        session.paused = false;
        let needs_speak = std::mem::take(&mut session.resume_needs_speak);
        assert!(needs_speak);
        assert_eq!(session.segments[session.index].text, "句1");
        assert!(
            !session.resume_needs_speak,
            "标记取用后须复位，避免重复开播"
        );
    }

    #[test]
    fn pause_mid_sentence_resumes_by_continue() {
        // 常规暂停（句子读到一半）：队列里仍有被挂起的句 → resume 走 continueSpeaking。
        let mut session = session_fixture(3);
        session.started = true;
        session.paused = true;
        assert!(!session.resume_needs_speak);

        session.paused = false;
        assert!(
            !std::mem::take(&mut session.resume_needs_speak),
            "未跨句边界 → 续播而非重新开播"
        );
        assert_eq!(session.index, 0, "续播不改变句索引");
    }

    #[test]
    fn last_sentence_finishes_to_idle() {
        let mut session = session_fixture(2);
        session.started = true;
        session.index = 1;
        session.index += 1;
        assert!(
            session.index >= session.segments.len(),
            "越过末句 → 会话置空、广播 idle"
        );
    }
}
