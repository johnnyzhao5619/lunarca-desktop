//! 实时会话编排器（TDD-01 §5.9）。
//!
//! `RealtimeSession` 是跨 ②音频消费 / ③ASR 线程池 / 可选译者 / ⑤前端 流水线的单一
//! owner，位于 `luna-jobqueue`（④ tokio 编排层），GUI 与 CLI 共享（无头低配同需降级，
//! TDD-09）。
//!
//! ## 职责
//! - **持有会话生命周期**：采集停止回调 + ASR Job 取消令牌 + 可选译者。
//! - **聚合 RTF**：`T_inference`（ASR 内部结果通道，不污染对外 `JobEvent`）÷ 这段时间流过的
//!   **会话时长**（含静音与被跳过的窗，TDD-02 §5.1）→ 滑窗 RTF = 解码器的忙碌占比。
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
    /// 上一次喂解码以来、被跳过 / 丢弃的窗带过来的会话时长（ms）——并进下一个
    /// 真正喂解码的窗的分母（见 [`Self::on_skipped_window`]）。
    pending_elapsed_ms: f32,
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
            pending_elapsed_ms: 0.0,
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
        self.pending_elapsed_ms = 0.0;
    }

    /// 一个窗**没有**喂解码（档位跳过 / 通道满丢弃）时调用，`elapsed_ms` 同
    /// [`Self::on_audio_window`]。
    ///
    /// 这段时间解码器没在为它干活，但时间是真流过去了：不记下来，下一个喂解码的窗就只
    /// 带着自己那一小段时长去除解码耗时——Yellow 跳快照后只剩终窗、Red 只剩探测窗，
    /// RTF 永远被算高、档位降不回来。只累计，不驱动控制器（没有新的推理测量）。
    pub fn on_skipped_window(&mut self, elapsed_ms: f32) {
        self.pending_elapsed_ms += elapsed_ms;
    }

    /// 每个**喂进解码**的窗调用一次。`elapsed_ms` = 自上一个窗（不论喂没喂解码）以来流过的
    /// **会话时长**（`SpeechWindow::elapsed_samples` 换算；含静音），**不是**窗里的语音时长。
    ///
    /// 为什么是会话时长（TDD-02 §5.1）：降级要答的是「解码器跟不跟得上实时」，即这段时间里
    /// 它忙了多久。按语音时长算时，底噪峰值切出的 100–300 ms 短窗每个都付一次整窗解码
    /// （tiny 档约 0.3 s），RTF 被算成 1–2.6、档位掉到 Yellow/Red、字幕停更，而解码器实际
    /// 只忙了一成多的时间（2026-11-04 真机：-75 dBFS 底噪、几乎无人声）。连续语音时快照之间
    /// 流过的正好是快照间隔，两种口径逐窗相等——正常说话的档位判定不变。
    ///
    /// 从 `infer_rx` 非阻塞拉取**全部**已完成的 `T_inference`（分子是这段时间里解码器
    /// 忙的总时长，只取一条会让积压的测量永远滞后）；聚合 RTF；驱动降级控制器。
    /// 仅当档位**变化**时发出 [`DegradedEvent`]（低频；同档位重复调用返回 `None`）。
    pub fn on_audio_window(&mut self, elapsed_ms: f32) -> Option<DegradedEvent> {
        let infer_ms = self
            .infer_rx
            .as_ref()
            .map_or(0.0, |rx| rx.try_iter().map(|ms| ms as f32).sum::<f32>());
        let elapsed_ms = elapsed_ms + std::mem::take(&mut self.pending_elapsed_ms);

        self.rtf_tracker.add_measurement(infer_ms, elapsed_ms);
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
    use luna_audio::{DegradationState, SegmenterConfig, StreamSegmenter, VadLabel};
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

        // T_inference = 600ms, 流过 1000ms → RTF ≈ 0.6
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

    // ── RTF 分母 = 流过的会话时长（TDD-02 §5.1）──────────────────────────────

    /// 把一串 VAD 标签（每帧 20 ms）经真实分段器回放给会话：每个喂解码的窗付 `decode_ms`
    /// 推理耗时；档位不是 Green 时按 `should_decode_window` 的语义跳窗（Yellow/Orange 跳
    /// 快照、Red 全跳——不模拟探测配额，只会让 Red 更难恢复，是更严的条件）。
    /// 返回途经的最高实时档位与喂解码的窗数。
    fn replay(
        labels: &[VadLabel],
        snapshot_ms: usize,
        decode_ms: u64,
    ) -> (DegradationState, usize) {
        let mut cfg = SegmenterConfig::new(16_000);
        cfg.snapshot_interval_samples = 16 * snapshot_ms;
        let mut seg = StreamSegmenter::new(cfg);
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(64);
        let (mut session, _) = RealtimeSession::new(
            None,
            CancelToken::new(),
            Some(infer_rx),
            AsrLoadGauge::new(),
        );
        let frame = vec![0.01f32; 320];
        let mut worst = DegradationState::Green;
        let mut fed = 0;
        for &label in labels {
            for w in seg.push(&frame, label) {
                let elapsed_ms = w.elapsed_samples as f32 / 16.0;
                let skip = match session.realtime_decode_tier() {
                    DegradationState::Green => false,
                    DegradationState::Yellow | DegradationState::Orange => w.is_snapshot,
                    DegradationState::Red => true,
                };
                if skip {
                    session.on_skipped_window(elapsed_ms);
                    continue;
                }
                fed += 1;
                infer_tx.send(decode_ms).unwrap();
                session.on_audio_window(elapsed_ms);
                if session.current_tier() as u8 > worst as u8 {
                    worst = session.current_tier();
                }
            }
        }
        (worst, fed)
    }

    /// 底噪峰值：0.3–3 s 的静音之间夹 60–300 ms 被 VAD 判成语音的尖峰，共 ~45 s
    /// （确定性伪随机，复刻 2026-11-04 真机：-75 dBFS 底噪、峰值 -52 dBFS、几乎无人声）。
    fn noise_floor_labels() -> Vec<VadLabel> {
        let mut rng: u64 = 0x1234_5678;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut labels = Vec::new();
        while labels.len() < 45 * 50 {
            let gap = 15 + (next() % 135) as usize;
            labels.extend(std::iter::repeat_n(VadLabel::Silence, gap));
            let burst = 3 + (next() % 12) as usize;
            labels.extend(std::iter::repeat_n(VadLabel::Speech, burst));
        }
        labels
    }

    /// **底噪不得把实时档位推下 Green**（提词器跟读 / 实时字幕停更的根因回归锁）。
    ///
    /// 这串底噪切出 ~22 个 100–300 ms 的窗，每个都付一次整窗解码（tiny 档 ~0.3 s）：解码器
    /// 45 s 里只忙了 ~7 s。旧口径（分母 = 窗里新增的语音时长）把同一串窗算成滑窗 RTF
    /// 最高 2.59 → Red，真机上 1.5 s 节奏掉到 Yellow、1 s 节奏到过 Red；本用例在旧口径下红。
    #[test]
    fn noise_floor_blips_do_not_degrade_the_live_tier() {
        let labels = noise_floor_labels();
        for snapshot_ms in [1_500, 1_000] {
            let (worst, fed) = replay(&labels, snapshot_ms, 300);
            assert_eq!(
                worst,
                DegradationState::Green,
                "底噪短窗不是负载：{snapshot_ms} ms 快照节奏下档位必须保持 Green"
            );
            assert!(
                fed >= 15,
                "夹具应切出足够多的短窗且全部喂解码（实得 {fed}）"
            );
        }
    }

    /// 连续语音时口径不变：快照之间流过的正好是快照间隔，RTF = 解码耗时 ÷ 快照间隔，
    /// 真跟不上（每窗解码 > 快照间隔）仍然立刻升档——修法没有把真实过载藏起来。
    #[test]
    fn continuous_speech_keeps_its_rtf_and_real_overload_still_escalates() {
        let speech = vec![VadLabel::Speech; 8 * 50];
        let (worst, _) = replay(&speech, 1_000, 300);
        assert_eq!(
            worst,
            DegradationState::Green,
            "0.3 s 解码 / 1 s 快照 = RTF 0.3"
        );

        let (worst, _) = replay(&speech, 1_000, 1_200);
        assert_eq!(
            worst,
            DegradationState::Red,
            "每 1 s 快照要解 1.2 s = 真跟不上，必须升到 Red"
        );
    }

    /// 被跳过的窗时间照样流过：降档后只剩终窗在解码，RTF 必须看得见这段空闲才能降回来。
    ///
    /// 反向验证：不调 `on_skipped_window`（旧行为）时，终窗只带自己 100 ms 去除 300 ms
    /// 解码，RTF 恒为 3.0——跳快照这件事本身让档位永远降不回来。
    #[test]
    fn skipped_windows_let_the_tier_recover() {
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(64);
        let (mut session, _) = RealtimeSession::new(
            None,
            CancelToken::new(),
            Some(infer_rx),
            AsrLoadGauge::new(),
        );
        for _ in 0..5 {
            infer_tx.send(900).unwrap();
            session.on_audio_window(1_000.0);
        }
        assert_eq!(session.current_tier(), DegradationState::Orange);

        // 之后每 utterance：两个快照被跳（各流过 1 s），终窗喂解码（距上一窗 100 ms）。
        for _ in 0..8 {
            session.on_skipped_window(1_000.0);
            session.on_skipped_window(1_000.0);
            infer_tx.send(300).unwrap();
            session.on_audio_window(100.0);
        }
        assert_eq!(
            session.current_tier(),
            DegradationState::Green,
            "真实负载 300 ms / 2.1 s ≈ 0.14，档位必须能降回 Green（实得 RTF {:.2}）",
            session.current_rtf()
        );
    }

    /// 分子取这段时间里**全部**完成的推理：一次喂解码之间解码器可能交回多条测量
    /// （跳窗期间、或解码追上积压时），只取一条会让 RTF 系统性偏低且测量永远滞后。
    #[test]
    fn all_completed_inference_counts_toward_the_numerator() {
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(8);
        let (mut session, _) = RealtimeSession::new(
            None,
            CancelToken::new(),
            Some(infer_rx),
            AsrLoadGauge::new(),
        );
        infer_tx.send(300).unwrap();
        infer_tx.send(300).unwrap();
        session.on_audio_window(1_000.0);
        let rtf = session.current_rtf();
        assert!((rtf - 0.6).abs() < 1e-4, "两条测量都要计入（got {rtf:.4}）");
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
