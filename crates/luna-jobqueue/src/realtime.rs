//! 实时会话编排器（TDD-01 §5.9）。
//!
//! `RealtimeSession` 是跨 ②音频消费 / ③ASR 线程池 / 可选译者 / ⑤前端 流水线的单一
//! owner，位于 `luna-jobqueue`（④ tokio 编排层），GUI 与 CLI 共享（无头低配同需降级，
//! TDD-09）。
//!
//! ## 职责
//! - **持有会话生命周期**：采集停止回调 + ASR Job 取消令牌 + 可选译者。
//! - **聚合 RTF**：`T_audio`（音频窗口时长）+ `T_inference`（ASR 内部结果通道，不污染
//!   对外 `JobEvent`）→ 滑窗 RTF。
//! - **单实例滞后降级控制器**：仅此处持有 `DegradationController` 决策实例（TDD-02 §5.2
//!   决策归属）；`luna-audio` 只产指标，不持有决策。降级的**执行**分两级，顺序是固定的
//!   （ADR-078 ②）：**先停批式转写的准入**（`AsrLoadGauge` 的闸；它的音频已全量落盘、
//!   没有截止期，晚三十秒解出来的字一模一样），批式卸干净了负载还是下不来，**才**轮到
//!   消费端据档位跳实时窗（快照 / Red 探测重试，见 `luna-app`
//!   `recording::should_decode_window`）。档位语义本身不变，变的是「谁先买单」。
//! - **告知 UI**：仅当档位**变化**时发一条低频 typed event（`realtime://degraded`），
//!   通过 `tokio::sync::watch` 推送——不污染高频 IPC 通道（TDD-01 §5.9）。
//! - **引擎分治（转译独立于 ASR 降级）**：ASR（whisper）与转译（随包 llama-server，进程外独立引擎）
//!   互不阻塞——ASR 负载**绝不**挂起转译（那只会让译文更遗漏而无助于 ASR）。转译自身的
//!   积压由实时转译管线**就近优先**卸载（TDD-08 §4），本编排器不介入。
//! - **取消传播**（§5.5）：`stop()` 同时 cancel ASR Job + 调用采集停止回调。
//!
//! ## 边界
//! - **批式 Job 仍不挂此控制器，但控制器现在看得见它**（2026-08-18 修订，ADR-078 ②）。
//!   原来那句「批量 Job 不挂此控制器」被当成了「控制器不需要知道批量的存在」，于是
//!   `DegradationController` 只看得见**被批式推高的实时 RTF**、看不见批式本身——它唯一
//!   能做的动作就是跳实时快照窗，结果是用户点了「二次转写」，代价由同一时刻的实时字幕付。
//!   现在批式的在飞条数经 `AsrLoadGauge` 进入 `AsrLoad`：批量 Job 依然不受本控制器的档位
//!   门控（它没有快照窗可跳），但它是**被卸载的第一顺位**。
//! - 入口仲裁（麦克风硬互斥）归 `luna-app`（TDD-05 §5.3）；此处无仲裁逻辑。
//! - 模型换档（Orange → turbo）为重操作，须在 utterance 边界执行，本期仅记录档位，
//!   换档信号由上层解释。

use crate::AsrLoadGauge;
use luna_audio::{
    AsrLoad, DegradationController, DegradationDecision, DegradationState, RtfTracker,
    SheddingTarget,
};
use luna_core::CancelToken;

// ─── DegradationReason ────────────────────────────────────────────────────────

/// 降级原因（typed event `realtime://degraded` 的字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradationReason {
    /// RTF 超阈值触发自动降级（高负载自愈）。
    HighRtf,
    /// 用户或上层手动触发（保留扩展，本期未用）。
    ManualOverride,
}

// ─── DegradedEvent ────────────────────────────────────────────────────────────

/// 降级事件（`realtime://degraded { tier, reason }`，TDD-01 §5.9 低频 typed event）。
///
/// 只在档位**变化**时发出——前端显示非阻断横幅（PRD-04-013）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DegradedEvent {
    /// 新档位。
    pub tier: DegradationState,
    /// 触发原因。
    pub reason: DegradationReason,
}

// ─── RealtimeSession ──────────────────────────────────────────────────────────

/// 实时转录会话编排者（TDD-01 §5.9 单实例 owner）。
///
/// 持有会话生命周期：采集停止回调 + ASR Job 取消令牌 + 可选译者 + RTF 聚合 + 降级决策。
pub struct RealtimeSession {
    /// 采集停止回调（调用时停止 `CaptureHandle`；测试可为 `None`）。
    stop_capture: Option<Box<dyn FnOnce() + Send>>,
    /// ASR Job 取消令牌（§5.5 取消传播）。
    job_cancel: CancelToken,
    /// RTF 滑窗计算器（编排层持有，TDD-02 §5.2 决策归属）。
    rtf_tracker: RtfTracker,
    /// 降级状态机（单实例；`luna-audio` 只产指标，不持有此实例）。
    degradation: DegradationController,
    /// 上次上报的档位（去重：仅变化时发事件）。
    last_reported_tier: DegradationState,
    /// 上一次负载采样的裁决（档位 + 该卸谁）。消费端据 `realtime_tier()` 决定跳不跳窗。
    last_decision: DegradationDecision,
    /// 本机 ASR 负载探针：读「还有几件批式在飞」，写「批式准入闸」。
    load: AsrLoadGauge,
    /// 降级事件发送端（watch channel；低频，不污染高频 IPC 通道）。
    degraded_tx: tokio::sync::watch::Sender<Option<DegradedEvent>>,
    /// ASR 内部推理耗时接收端（每窗 T_inference ms，非 JobEvent）。
    infer_rx: Option<std::sync::mpsc::Receiver<u64>>,
}

impl RealtimeSession {
    /// 构造会话并返回降级事件订阅句柄。
    ///
    /// `stop_capture`：生产代码传 `Some(Box::new(move || capture.stop()))`；
    /// 单元测试可传 `None`（无真实采集句柄）。
    pub fn new(
        stop_capture: Option<Box<dyn FnOnce() + Send>>,
        job_cancel: CancelToken,
        infer_rx: Option<std::sync::mpsc::Receiver<u64>>,
        load: AsrLoadGauge,
    ) -> (Self, tokio::sync::watch::Receiver<Option<DegradedEvent>>) {
        let (degraded_tx, degraded_rx) = tokio::sync::watch::channel(None);
        let session = Self {
            stop_capture,
            job_cancel,
            rtf_tracker: RtfTracker::default_window(),
            degradation: DegradationController::new(),
            last_reported_tier: DegradationState::Green,
            last_decision: DegradationDecision {
                tier: DegradationState::Green,
                shed: SheddingTarget::None,
            },
            load,
            degraded_tx,
            infer_rx,
        };
        (session, degraded_rx)
    }

    /// 换一条 RTF 探针（`register_stream` 重新建腿时，ADR-080）。
    ///
    /// **顺带把 RTF 滑窗清零**：旧腿的测量属于另一条流、另一段时间——用户把实时转写
    /// 关了几分钟再打开，那几条陈旧样本会让新腿一开张就背着一个不属于它的档位。清零后
    /// 头几窗按 Green 走，档位在几窗之内由新腿自己的数重新长出来。
    pub fn attach_infer(&mut self, infer_rx: Option<std::sync::mpsc::Receiver<u64>>) {
        self.infer_rx = infer_rx;
        self.rtf_tracker = RtfTracker::default_window();
    }

    /// 每个音频窗口送达时调用（`audio_ms` = 本窗口音频物理时长 ms）。
    ///
    /// 从 `infer_rx` 非阻塞拉取最新 `T_inference`；聚合 RTF；驱动降级控制器。
    /// 仅当档位**变化**时发出 [`DegradedEvent`]（低频；同档位重复调用返回 `None`）。
    pub fn on_audio_window(&mut self, audio_ms: f32) -> Option<DegradedEvent> {
        // 非阻塞拉取 ASR 内部推理耗时
        let infer_ms = self
            .infer_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
            .map_or(0.0, |ms| ms as f32);

        self.rtf_tracker.add_measurement(infer_ms, audio_ms);
        let rtf = self.rtf_tracker.current_rtf();
        // 控制器的输入是**本机 ASR 负载**，不是单条流的 RTF：只看 RTF 答不上
        // 「是谁让实时腿跟不上的」，而那正是决定该卸谁的唯一问题（ADR-078 ②）。
        let decision = self.degradation.update(AsrLoad {
            realtime_rtf: rtf,
            batch_inflight: self.load.batch_inflight(),
        });
        self.last_decision = decision;
        // 把裁决落到队列侧的闸：批式该让路时，它的**准入**停下（已经在跑的那件跑完，
        // 引擎线程策略不动，ADR-010 边界）。
        self.load
            .set_batch_admission_paused(decision.shed == SheddingTarget::BatchAsr);
        let new_tier = decision.tier;

        if new_tier != self.last_reported_tier {
            // **档位变化要留痕**：它决定了消费端跳不跳窗（`recording::should_decode_window`），
            // 而 Yellow/Orange 会跳掉**全部快照窗**——用户看到的是「字幕忽然不更新」。界面上
            // 它只是一个角标，stdout 上此前一个字都没有；排障时「是不是降级了、降到哪一档、
            // RTF 多少」全靠猜。零内容，只有档位与 RTF 数值。
            eprintln!(
                "[F4] 实时档位 {:?} → {new_tier:?}（RTF={rtf:.2}，批式在飞 {}；本档先卸 {:?}）",
                self.last_reported_tier,
                self.load.batch_inflight(),
                decision.shed,
            );
            let event = DegradedEvent {
                tier: new_tier,
                reason: DegradationReason::HighRtf,
            };
            self.last_reported_tier = new_tier;
            // 仅广播档位事件供消费端决策（recording::should_decode_window 用的是
            // `realtime_decode_tier()`，即「批式已经卸完了才轮到实时」的那个档位）；
            // 转译独立引擎不受 ASR 降级挂起（引擎分治，见模块文档 + TDD-08 §4）。
            let _ = self.degraded_tx.send(Some(event.clone()));
            Some(event)
        } else {
            None
        }
    }

    /// 当前降级档位（Green/Yellow/Orange/Red）——**面向 UI 的那个数**：它回答
    /// 「这台机器此刻吃不吃得消」，与「谁被节流」无关。
    #[must_use]
    pub fn current_tier(&self) -> DegradationState {
        self.last_reported_tier
    }

    /// 喂给 `recording::should_decode_window` 的**实时档位**（ADR-078 ②）。
    ///
    /// 与 [`Self::current_tier`] 分开是本次修复的核心：只要还有批式可卸，这一位就是
    /// `Green`——实时快照窗照常解码，让路的是那份没有截止期的活。合成一个数就等于回到
    /// 「被批式推高的负载由实时字幕买单」，而那正是用户看到的「二次转写期间字幕停更」。
    #[must_use]
    pub fn realtime_decode_tier(&self) -> DegradationState {
        self.last_decision.realtime_tier()
    }

    /// 本档位下先卸哪一侧（诊断/单测用）。
    #[must_use]
    pub fn shedding_target(&self) -> SheddingTarget {
        self.last_decision.shed
    }

    /// 当前 RTF 值（滑窗均值；无数据时为 0.0）。
    #[must_use]
    pub fn current_rtf(&self) -> f32 {
        self.rtf_tracker.current_rtf()
    }

    /// 停止会话：取消 ASR Job + 调用采集停止回调（§5.5 取消传播）。
    ///
    /// 消费 `self`，不可复用。
    pub fn stop(mut self) {
        // 1. 取消 ASR Job（CancelToken 传播到 whisper abort）
        self.job_cancel.cancel();
        // 2. 停止音频采集
        if let Some(stop_fn) = self.stop_capture.take() {
            stop_fn();
        }
        // 3. 批式准入闸由 `Drop` 归还——见下。
    }
}

impl Drop for RealtimeSession {
    /// **闸必须在这里归还，不能只在 `stop()` 里**：会话也可能因为 part 收尾、panic
    /// 展开或任何一条提前 return 而被直接 drop，而一个没人归还的「暂停批式」闸意味着
    /// 用户此后**再也跑不动一次二次转写**，且界面上没有任何东西会说明为什么。
    fn drop(&mut self) {
        self.load.set_batch_admission_paused(false);
    }
}

// ─── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use luna_audio::DegradationState;
    use luna_core::CancelToken;

    fn noop_session() -> (
        RealtimeSession,
        tokio::sync::watch::Receiver<Option<DegradedEvent>>,
    ) {
        let cancel = CancelToken::new();
        RealtimeSession::new(None, cancel, None, AsrLoadGauge::new())
    }

    // ── RTF 聚合 ──────────────────────────────────────────────────────────────

    #[test]
    fn rtf_aggregation_from_audio_and_infer_windows() {
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(8);
        let cancel = CancelToken::new();
        let (mut session, _) =
            RealtimeSession::new(None, cancel, Some(infer_rx), AsrLoadGauge::new());

        // T_inference = 600ms, T_audio = 1000ms → RTF ≈ 0.6
        infer_tx.send(600).unwrap();
        session.on_audio_window(1000.0);

        let rtf = session.current_rtf();
        assert!((rtf - 0.6).abs() < 1e-4, "RTF 应 ≈ 0.6（got {rtf:.4}）");
    }

    /// 换腿之后 RTF 从零起算（ADR-080：转写闸门关掉再打开 = 一条新腿）。
    ///
    /// 反向验证：`attach_infer` 只换 `infer_rx` 而不清滑窗，新腿的第一窗会被旧腿那条
    /// 慢测量拉住（这里断言的 0.1 会变成 ≈0.85），于是用户刚把字幕打开就先吃一次
    /// 不属于这条腿的降档。
    #[test]
    fn attaching_a_new_probe_forgets_the_dead_legs_rtf() {
        let (old_tx, old_rx) = std::sync::mpsc::sync_channel::<u64>(8);
        let cancel = CancelToken::new();
        let (mut session, _) =
            RealtimeSession::new(None, cancel, Some(old_rx), AsrLoadGauge::new());
        // 旧腿：又慢又真实（RTF 1.6）。
        old_tx.send(1600).unwrap();
        session.on_audio_window(1000.0);
        assert!(session.current_rtf() > 1.0);

        let (new_tx, new_rx) = std::sync::mpsc::sync_channel::<u64>(8);
        session.attach_infer(Some(new_rx));
        new_tx.send(100).unwrap();
        session.on_audio_window(1000.0);

        let rtf = session.current_rtf();
        assert!(
            (rtf - 0.1).abs() < 1e-4,
            "新腿的 RTF 只能由新腿的测量决定（got {rtf:.4}）"
        );
    }

    #[test]
    fn rtf_zero_when_no_infer_rx() {
        let (mut session, _) = noop_session();
        session.on_audio_window(1000.0); // infer_rx=None → infer_ms=0
        assert_eq!(session.current_rtf(), 0.0);
    }

    // ── 升降档事件（仅变化时发出）────────────────────────────────────────────

    #[test]
    fn degraded_event_only_emitted_on_tier_change() {
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(16);
        let cancel = CancelToken::new();
        let (mut session, _) =
            RealtimeSession::new(None, cancel, Some(infer_rx), AsrLoadGauge::new());

        // RTF < 0.6 → Green：不触发事件（初始已是 Green）
        infer_tx.send(100).unwrap();
        let ev = session.on_audio_window(1000.0); // RTF=0.1
        assert!(ev.is_none(), "初始 Green→Green 不应发事件");

        // RTF ≥ 0.6 → Yellow：触发事件
        infer_tx.send(700).unwrap();
        let _ev = session.on_audio_window(1000.0); // RTF=0.7 (单窗；滑窗均值)
                                                   // 注：滑窗为 5 点，这里只有 2 个采样点，均值 = (100+700)/(1000+1000)=0.4，未升档
                                                   // 让滑窗充满到超阈值
        for _ in 0..5 {
            infer_tx.send(700).unwrap();
            session.on_audio_window(1000.0);
        }
        let tier = session.current_tier();
        // 此时滑窗 5 点均为 700/1000=0.7 → Yellow
        assert!(
            matches!(
                tier,
                DegradationState::Yellow | DegradationState::Orange | DegradationState::Red
            ),
            "持续高 RTF 应升档（got {tier:?}）"
        );

        // 再次调用相同 RTF → 不应发新事件（档位未变）
        infer_tx.send(700).unwrap();
        let ev2 = session.on_audio_window(1000.0);
        assert!(ev2.is_none(), "档位不变时不应重复发事件");
    }

    #[test]
    fn escalation_triggers_event_immediately() {
        // 用单窗滑窗（window=1）让 RTF 立刻反映
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(8);
        let cancel = CancelToken::new();
        // 注入 RtfTracker window=1 — 通过构造自定义 session，此处直接测 DegradationController 路径
        // 改用大 RTF 确保即使滑窗均值也超阈值
        let (mut session, _) =
            RealtimeSession::new(None, cancel, Some(infer_rx), AsrLoadGauge::new());

        // 发 5 次 RTF≥1.0（滑窗=5，全部充满后均值>=1.0）
        for _ in 0..5 {
            infer_tx.send(2_000).unwrap(); // 2000ms infer
            session.on_audio_window(1000.0); // 1000ms audio → RTF=2.0
        }
        let _ev = session.on_audio_window(1000.0);
        // 此时已经在 Red，on_audio_window 返回 None（档位不再变化）
        // 但在充满过程中应已发过 event
        assert_eq!(
            session.current_tier(),
            DegradationState::Red,
            "RTF=2.0 持续应升至 Red"
        );
    }

    // ── 取消传播 ──────────────────────────────────────────────────────────────

    #[test]
    fn cancel_propagates_to_job_token() {
        let cancel = CancelToken::new();
        let cancel_check = cancel.clone();
        let (session, _) = RealtimeSession::new(None, cancel, None, AsrLoadGauge::new());

        assert!(!cancel_check.is_cancelled(), "停止前令牌未取消");
        session.stop();
        assert!(cancel_check.is_cancelled(), "stop() 应取消 Job 令牌");
    }

    #[test]
    fn cancel_calls_stop_capture_callback() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = stopped.clone();

        let cancel = CancelToken::new();
        let stop_fn: Box<dyn FnOnce() + Send> = Box::new(move || {
            stopped_clone.store(true, Ordering::SeqCst);
        });

        let (session, _) = RealtimeSession::new(Some(stop_fn), cancel, None, AsrLoadGauge::new());

        assert!(!stopped.load(Ordering::SeqCst), "停止前回调未触发");
        session.stop();
        assert!(stopped.load(Ordering::SeqCst), "stop() 应调用采集停止回调");
    }
}
