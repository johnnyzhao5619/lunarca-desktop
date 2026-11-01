//! 端点检测状态机（语音输入：端点检测后整段转写）。事实源：TDD-02 §3.3-A。
//!
//! 裸 VAD 逐帧标签须经**迟滞状态机**转为"一句话"，杜绝吃头/切碎：
//! - **pre-roll 预滚缓冲(~300ms)**：保留语音起点**之前**的音频（首音素不被吃，开头丢字根因）。
//! - **起音确认(~150ms)**：连续有声才判"开始"，滤掉咳嗽/敲键瞬态。
//! - **尾静音 hangover(~700ms)**：静音够久才判"说完"。
//! - **min/max 时长**：过短(<300ms)丢弃为噪声；过长(>30s)强制收尾防失控——达 max 即定稿
//!   并落地该段，用户继续说则自动开始下一段、依次连续落地（连续注入，PRD-02-008）。
//!
//! 纯逻辑、零 IO、确定性——可在 headless CI 用合成样本回归（黄金夹具，§8）。本状态机
//! **同时**服务 P0 幻觉抑制：纯静音/背景噪声**永不**产出 utterance（不喂 whisper，
//! PRD-02-007、§3.3-C）。

use std::collections::VecDeque;
use std::time::Duration;

/// 端点检测参数（默认值见各常量，可由设置覆盖，PRD-09 §2.6）。
#[derive(Clone, Copy, Debug)]
pub struct EndpointConfig {
    /// 工作采样率（须为单声道，通常 [`crate::TARGET_SAMPLE_RATE`]）。
    pub sample_rate: u32,
    /// pre-roll 预滚时长。
    pub preroll: Duration,
    /// 起音确认时长（连续语音）。
    pub attack: Duration,
    /// 尾静音挂起时长。
    pub hangover: Duration,
    /// 最短 utterance（短于此丢弃为噪声）。
    pub min_utterance: Duration,
    /// 最长 utterance（达此强制收尾并落地，PRD-02-008）。
    pub max_utterance: Duration,
}

impl EndpointConfig {
    /// 以给定采样率构造默认参数（§3.3-A 推荐值）。
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            preroll: Duration::from_millis(300),
            attack: Duration::from_millis(150),
            hangover: Duration::from_millis(700),
            min_utterance: Duration::from_millis(300),
            max_utterance: Duration::from_secs(30),
        }
    }

    fn samples(&self, d: Duration) -> usize {
        (d.as_secs_f64() * f64::from(self.sample_rate)).round() as usize
    }
}

/// utterance 收尾原因（诊断/编排用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalizeReason {
    /// 尾静音挂起达阈值（自然说完）。
    Hangover,
    /// 达最长时长强制收尾（PRD-02-008 连续落地）。
    MaxDuration,
    /// 用户手动结束（点按结束/松开按住，[`Endpointer::flush`]）。
    ManualStop,
}

/// 端点状态机产出的事件。
#[derive(Clone, Debug, PartialEq)]
pub enum EndpointEvent {
    /// 起音已确认（HUD 可切到"识别中…"，仅状态提示，PRD-02-004）。
    SpeechStarted,
    /// 一段完整 utterance（mono 16k 样本，含 pre-roll、已去尾静音）。
    Utterance {
        /// 样本（单声道，工作采样率）。
        samples: Vec<f32>,
        /// 收尾原因。
        reason: FinalizeReason,
    },
    /// 一段被判过短（<min）而丢弃——不产出文本（幻觉/误触防护，PRD-02-007）。
    Discarded {
        /// 被丢弃的样本数。
        samples_len: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// 未在语音中（维护 pre-roll，累计起音）。
    Idle,
    /// 语音中（累积 utterance，累计尾静音）。
    Active,
}

/// 端点检测器：喂入 (mono 帧, VAD 标签) → 产出 utterance 事件。
///
/// 契约：调用方每帧先用 [`crate::Vad::score`] 评分，再把**同一帧**与其标签传入 [`push`]。
/// 帧应为单声道、工作采样率（重采样到 16k 之后，§3.2 硬约束 1 的同位置）。
///
/// [`push`]: Endpointer::push
#[derive(Clone, Debug)]
pub struct Endpointer {
    preroll_cap: usize,
    attack_samples: usize,
    hangover_samples: usize,
    min_samples: usize,
    max_samples: usize,

    state: State,
    /// Idle 时的预滚环（容量 `preroll_cap`）。
    preroll: VecDeque<f32>,
    /// Idle 时连续语音样本计数（达 attack 即确认起音）。
    speech_run: usize,
    /// Active 时的 utterance 累积缓冲。
    current: Vec<f32>,
    /// Active 时连续尾静音样本计数（达 hangover 即收尾）。
    silence_run: usize,
}

use crate::vad::VadLabel;

impl Endpointer {
    /// 用参数构造（会话开始时新建，保证状态干净）。
    #[must_use]
    pub fn new(cfg: EndpointConfig) -> Self {
        let preroll_cap = cfg.samples(cfg.preroll).max(1);
        Self {
            preroll_cap,
            attack_samples: cfg.samples(cfg.attack).max(1),
            hangover_samples: cfg.samples(cfg.hangover).max(1),
            min_samples: cfg.samples(cfg.min_utterance),
            max_samples: cfg.samples(cfg.max_utterance).max(1),
            state: State::Idle,
            preroll: VecDeque::with_capacity(preroll_cap),
            speech_run: 0,
            current: Vec::new(),
            silence_run: 0,
        }
    }

    /// 喂入一帧（单声道）及其 VAD 标签，返回本帧触发的事件（0..N）。
    pub fn push(&mut self, frame: &[f32], label: VadLabel) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        match self.state {
            State::Idle => self.push_idle(frame, label, &mut events),
            State::Active => self.push_active(frame, label, &mut events),
        }
        events
    }

    fn push_idle(&mut self, frame: &[f32], label: VadLabel, events: &mut Vec<EndpointEvent>) {
        // 始终把样本灌入预滚环（保留语音起点之前的上下文）。
        for &s in frame {
            if self.preroll.len() == self.preroll_cap {
                self.preroll.pop_front();
            }
            self.preroll.push_back(s);
        }
        match label {
            VadLabel::Speech => {
                self.speech_run += frame.len();
                if self.speech_run >= self.attack_samples {
                    // 起音确认：把预滚（含 pre-roll + 起音段）转入 current。
                    self.state = State::Active;
                    self.current = self.preroll.drain(..).collect();
                    self.preroll.clear();
                    self.speech_run = 0;
                    self.silence_run = 0;
                    events.push(EndpointEvent::SpeechStarted);
                }
            }
            VadLabel::Silence => {
                // 起音未达阈值前的瞬态语音（咳嗽/敲键）被静音打断 → 复位计数。
                self.speech_run = 0;
            }
        }
    }

    fn push_active(&mut self, frame: &[f32], label: VadLabel, events: &mut Vec<EndpointEvent>) {
        self.current.extend_from_slice(frame);
        match label {
            VadLabel::Speech => self.silence_run = 0,
            VadLabel::Silence => self.silence_run += frame.len(),
        }

        // 达最长时长：强制收尾并落地该段；若用户继续说，立即开始下一段（连续落地）。
        if self.current.len() >= self.max_samples {
            self.finalize(FinalizeReason::MaxDuration, events);
            return;
        }
        // 尾静音挂起达阈值：自然收尾。
        if self.silence_run >= self.hangover_samples {
            self.finalize(FinalizeReason::Hangover, events);
        }
    }

    /// 手动结束（点按结束/松开按住）：若在语音中则收尾当前段，否则无产出。
    pub fn flush(&mut self) -> Vec<EndpointEvent> {
        let mut events = Vec::new();
        if self.state == State::Active {
            self.finalize(FinalizeReason::ManualStop, &mut events);
        }
        // 复位（会话结束）。
        self.state = State::Idle;
        self.preroll.clear();
        self.speech_run = 0;
        self.silence_run = 0;
        self.current.clear();
        events
    }

    /// 收尾当前 utterance：去尾静音 → 判 min → 产 Utterance 或 Discarded → 回 Idle。
    fn finalize(&mut self, reason: FinalizeReason, events: &mut Vec<EndpointEvent>) {
        let mut samples = std::mem::take(&mut self.current);
        // 去掉本段尾部检测到的连续静音（减少无效推理 + 抑制尾部幻觉）；MaxDuration 不去尾
        // （达上限时多为持续语音、尾静音为 0）。
        if matches!(reason, FinalizeReason::Hangover) {
            let trim = self.silence_run.min(samples.len());
            samples.truncate(samples.len() - trim);
        }
        let reenter_active = matches!(reason, FinalizeReason::MaxDuration);
        if samples.len() >= self.min_samples && !samples.is_empty() {
            events.push(EndpointEvent::Utterance { samples, reason });
        } else {
            events.push(EndpointEvent::Discarded {
                samples_len: samples.len(),
            });
        }
        // 复位段级累积。MaxDuration 收尾后用户仍在说话 → 直接续 Active 开下一段。
        self.silence_run = 0;
        self.speech_run = 0;
        if reenter_active {
            self.state = State::Active;
            self.current = Vec::new();
        } else {
            self.state = State::Idle;
            self.current = Vec::new();
            self.preroll.clear();
        }
    }

    /// 当前是否在语音中（HUD 状态查询）。
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == State::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    fn cfg() -> EndpointConfig {
        EndpointConfig::new(SR)
    }

    // 生成 n 个样本的语音帧（幅度 0.5）/ 静音帧（0）。
    fn speech(n: usize) -> Vec<f32> {
        vec![0.5; n]
    }
    fn silence(n: usize) -> Vec<f32> {
        vec![0.0; n]
    }

    // 把样本数按毫秒换算。
    fn ms(n: u64) -> usize {
        (n as usize * SR as usize) / 1000
    }

    #[test]
    fn pure_silence_never_produces_utterance() {
        // P0 幻觉抑制：纯静音输入永不产 Utterance。
        let mut ep = Endpointer::new(cfg());
        for _ in 0..200 {
            let ev = ep.push(&silence(ms(20)), VadLabel::Silence);
            assert!(ev.is_empty(), "静音不应触发任何事件: {ev:?}");
        }
        let f = ep.flush();
        assert!(f.is_empty());
    }

    #[test]
    fn speech_then_hangover_yields_one_utterance_with_preroll() {
        let mut ep = Endpointer::new(cfg());
        // 先喂 200ms 静音（进入预滚），再 1s 语音，再 800ms 静音触发 hangover。
        let mut events = Vec::new();
        for _ in 0..10 {
            events.extend(ep.push(&silence(ms(20)), VadLabel::Silence));
        }
        for _ in 0..50 {
            events.extend(ep.push(&speech(ms(20)), VadLabel::Speech));
        }
        for _ in 0..40 {
            events.extend(ep.push(&silence(ms(20)), VadLabel::Silence));
        }
        let started = events
            .iter()
            .filter(|e| matches!(e, EndpointEvent::SpeechStarted))
            .count();
        assert_eq!(started, 1, "应恰好一次起音确认");
        let utts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                EndpointEvent::Utterance { samples, reason } => Some((samples.len(), *reason)),
                _ => None,
            })
            .collect();
        assert_eq!(utts.len(), 1, "应恰好一段 utterance");
        let (len, reason) = utts[0];
        assert_eq!(reason, FinalizeReason::Hangover);
        // 含 pre-roll（~300ms）+ 1s 语音；尾静音被去除 → 长度 > 1s 且 < 1s+预滚+少量。
        assert!(len >= ms(1000), "应含完整语音, got {len} (1s={})", ms(1000));
        assert!(len <= ms(1000) + ms(350), "尾静音应被去除, got {len}");
    }

    #[test]
    fn short_blip_below_attack_is_ignored() {
        // 50ms 语音瞬态（< 150ms attack）被静音打断 → 不起音、无 utterance。
        let mut ep = Endpointer::new(cfg());
        let mut events = Vec::new();
        events.extend(ep.push(&speech(ms(50)), VadLabel::Speech));
        for _ in 0..40 {
            events.extend(ep.push(&silence(ms(20)), VadLabel::Silence));
        }
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EndpointEvent::SpeechStarted)),
            "瞬态不应确认起音"
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, EndpointEvent::Utterance { .. })));
    }

    #[test]
    fn manual_stop_finalizes_current_speech() {
        // 按住说话松开：1s 语音后立即 flush（无需等 hangover）。
        let mut ep = Endpointer::new(cfg());
        for _ in 0..50 {
            ep.push(&speech(ms(20)), VadLabel::Speech);
        }
        assert!(ep.is_active());
        let events = ep.flush();
        let utt = events.iter().find_map(|e| match e {
            EndpointEvent::Utterance { samples, reason } => Some((samples.len(), *reason)),
            _ => None,
        });
        let (len, reason) = utt.expect("flush 应产出 utterance");
        assert_eq!(reason, FinalizeReason::ManualStop);
        assert!(len >= ms(1000));
        assert!(!ep.is_active(), "flush 后复位为 Idle");
    }

    #[test]
    fn max_duration_finalizes_and_continues_next_segment() {
        // PRD-02-008：达 30s 上限强制收尾该段，继续说则开下一段、连续落地。
        let mut ep = Endpointer::new(cfg());
        let mut events = Vec::new();
        // 喂 35s 连续语音（每帧 100ms）。
        for _ in 0..350 {
            events.extend(ep.push(&speech(ms(100)), VadLabel::Speech));
        }
        let max_utts = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    EndpointEvent::Utterance {
                        reason: FinalizeReason::MaxDuration,
                        ..
                    }
                )
            })
            .count();
        assert!(max_utts >= 1, "应至少一次 MaxDuration 强制收尾");
        // 强制收尾后仍在 Active（继续累积下一段）。
        assert!(ep.is_active(), "MaxDuration 后应续 Active 连续落地");
    }
}
