//! 流式分段器（实时转录路径）。事实源：TDD-02 §3.3-B。
//!
//! `StreamSegmenter` 实现 VAD 门控滑动窗 + overlap + 短窗 padding + 会话绝对采样计数器。
//! 每产出一个窗携带 `abs_start`（会话级绝对采样偏移），供 LocalAgreement-2 换算绝对时间。
//!
//! **增长快照（实时首字延迟命门）**：连续语音期间，每积累 `snapshot_interval_samples`
//! 新语音即产出一个**同 `abs_start` 的增长快照窗**（内容 = 当前 utterance 的全量累积），
//! 供 whisper 端 LocalAgreement-2 以"同窗重解码 → 前缀一致提交"滚动出字——否则连续讲话
//! 要等 VAD 静音或 10s 上限才有第一个字（真机体感：长句期间 HUD 全程无字）。
//! utterance 收尾（静音/上限/flush）时产出的终窗与快照同 `abs_start`，是快照的超集，
//! 语义上**取代**全部快照（解码端据此合并过期快照，只解最新）。
//!
//! 纯逻辑、无 IO、headless 可单测。决策（降级/档位）归 `RealtimeSession`，本结构只产窗。

use crate::vad::VadLabel;

/// 流式分段器的配置参数（TDD-02 §3.3-B）。
#[derive(Clone, Copy, Debug)]
pub struct SegmenterConfig {
    /// 工作采样率（应为 16k mono，TARGET_SAMPLE_RATE）。
    pub sample_rate: u32,
    /// 窗长上限（speech_buf 达此即强制出窗，padding 到此后喂 whisper）。
    ///
    /// **实时性命门**：此值同时是「连续不停说话时多久出一窗」的延迟上限——VAD 检测不到句间
    /// 静音时（如持续语音/环境噪声），仅靠此上限切窗。30s 上限曾致连续说话 30s 内 HUD 无任何
    /// 实时文字（真机 K2）。取 ~10s 平衡 whisper 上下文与实时延迟（whisper 编码器对任意 ≤30s
    /// 输入成本近似恒定，窗变小不增单窗成本，仅增窗数；本机 RTF 余量内可接受）。
    pub window_samples: usize,
    /// 重叠样本数（~200–300ms，防跨窗断词）。
    pub overlap_samples: usize,
    /// 最小有效语音样本数（VAD 门控：短于此不产窗）。
    pub min_speech_samples: usize,
    /// 增长快照间隔（连续语音每积累这么多**新**语音样本，产一个同 `abs_start` 的
    /// 增长快照窗喂 LocalAgreement-2 滚动出字；见模块文档）。
    pub snapshot_interval_samples: usize,
    /// 尾静音迟滞（hangover）：utterance 内静音须**连续超过此样本数**才判「说完」出窗——
    /// 语音的音节/词间隙常达数百 ms，无迟滞会把连续话语切成音节级碎窗（补零后有效占比
    /// 个位数百分比 → whisper 对「碎片+静音垫」幻觉/误判语种，真机实时乱码根因）。
    /// 口径与语音输入 [`crate::endpointer::EndpointConfig::hangover`]（700ms）一致。
    pub hangover_samples: usize,
}

impl SegmenterConfig {
    /// 默认配置：16k、~10s 窗上限（实时延迟）、~250ms overlap、~100ms 最小语音、
    /// ~1.5s 增长快照间隔（hypothesis 首字延迟 ≈ 间隔 + 单窗解码耗时）、~700ms 尾静音迟滞。
    #[must_use]
    pub const fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as usize;
        Self {
            sample_rate,
            window_samples: 10 * sr, // 实时延迟上限（连续语音每 ≤10s 出窗，K2 修复）
            overlap_samples: sr / 4, // ~250ms
            min_speech_samples: sr / 10, // ~100ms
            snapshot_interval_samples: sr * 3 / 2, // ~1.5s
            hangover_samples: sr * 7 / 10, // ~700ms（与 Endpointer hangover 同口径）
        }
    }

    /// 窗长上限的**时长**口径：连续语音时「最坏情况下多久必定出一窗」。
    ///
    /// 它是实时路径对外承诺的**首窗延迟上界**——在这段时间之内，解码腿手上本来就一个窗
    /// 都没有。任何「解码腿还没就绪」的判据都该拿它当基准量纲，而不是各处再拍一个秒数：
    /// 窗长改了（这一周刚从 30s 改到 ~10s）而别处的秒数没跟着改，是一种不会有任何东西
    /// 变红的漂移。
    #[must_use]
    pub const fn window_limit(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            (self.window_samples as u64 * 1000) / self.sample_rate as u64,
        )
    }
}

/// 流式分段器产出的语音窗事件。
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechWindow {
    /// 单声道 16k f32 样本（已 padding 到 `window_samples` 或实际语音长 + overlap）。
    pub samples: Vec<f32>,
    /// 本窗在会话中的绝对起点（采样数，不含暂停期，÷ sample_rate = 绝对秒）。
    /// 同一 utterance 的增长快照与终窗共享同一 `abs_start`（解码端合并/LA-2 同窗语义）。
    pub abs_start: u64,
    /// 本窗实际语音内容的样本数（padding 之前）。
    pub speech_len: usize,
    /// 相比本 utterance 上一个已产出窗（快照/终窗）**新增**的语音样本数。
    /// RTF `T_audio` 只计增量——增长快照重复携带的旧语音不得重复计入音频时长。
    pub speech_delta: usize,
    /// 是否为**增长快照**（`emit_snapshot` 产出为 `true`；终窗/强切窗 `emit_window` 为 `false`）。
    /// 消费端据此在高负载档位跳过快照解码卸载负载（TDD-02 §5.2；见 [`crate::SpeechEvent::Window`]）。
    pub is_snapshot: bool,
}

/// 流式分段器：VAD 门控 → 滑动窗 + overlap + padding → 产 `SpeechWindow`。
///
/// 会话绝对时钟：`abs_samples` 只计实际传入的样本（`push` 调用），
/// **暂停期调用 [`pause`]/[`resume`] 来冻结/恢复计数**，保证时间轴与落盘逐样本对齐。
///
/// [`pause`]: StreamSegmenter::pause
/// [`resume`]: StreamSegmenter::resume
pub struct StreamSegmenter {
    cfg: SegmenterConfig,
    /// 会话绝对采样计数（仅计实际样本，暂停期冻结）。
    abs_samples: u64,
    /// 是否处于暂停状态（暂停期不计采样、不产窗）。
    paused: bool,
    /// 起音确认预滚缓冲（连续语音样本，达阈值才确认起音，避免瞬态触发）。
    preroll_buf: Vec<f32>,
    /// preroll_buf 对应的会话绝对起始偏移。
    preroll_start_abs: u64,
    /// 语音积累缓冲（已确认起音后的样本）。
    speech_buf: Vec<f32>,
    /// 本段语音积累的起始绝对偏移。
    speech_start_abs: u64,
    /// 当前是否处于已确认语音中。
    in_speech: bool,
    /// overlap 缓冲（上一窗尾部，下一窗前缀）。
    overlap_buf: Vec<f32>,
    /// 本 utterance 已随最近一次出窗（快照/终窗）计入的语音样本数
    /// （快照触发判定 + `speech_delta` 增量计算；utterance 收尾复位 0）。
    emitted_len: usize,
    /// utterance 内连续尾静音样本计数（达 `hangover_samples` 才判说完出窗）。
    silence_run: usize,
}

/// 起音确认阈值（~50ms @16k = 800 样本）。
const ATTACK_SAMPLES: usize = 800;

impl StreamSegmenter {
    /// 构造新实例（会话开始时调用，状态从零开始）。
    #[must_use]
    pub fn new(cfg: SegmenterConfig) -> Self {
        Self {
            cfg,
            abs_samples: 0,
            paused: false,
            preroll_buf: Vec::with_capacity(ATTACK_SAMPLES * 2),
            preroll_start_abs: 0,
            speech_buf: Vec::new(),
            speech_start_abs: 0,
            in_speech: false,
            overlap_buf: Vec::new(),
            emitted_len: 0,
            silence_run: 0,
        }
    }

    /// 暂停：冻结绝对时钟，后续 `push` 不计样本不产窗。
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// 恢复：解冻时钟，后续 `push` 正常计数产窗。
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// 当前会话绝对采样偏移。
    #[must_use]
    pub fn abs_samples(&self) -> u64 {
        self.abs_samples
    }

    /// 喂入一批 16k mono f32 样本及对应 VAD 标签（逐帧），产出 0..N 个 `SpeechWindow`。
    ///
    /// 调用方应已将原始帧归一化到 16k mono f32，再以 VAD 帧为单位分批传入。
    /// `label` 对整批样本统一生效（消费线程已按 VAD 定长帧切分后调用）。
    pub fn push(&mut self, samples: &[f32], label: VadLabel) -> Vec<SpeechWindow> {
        if self.paused || samples.is_empty() {
            return Vec::new();
        }
        // 暂停期之外才计绝对样本数。
        self.abs_samples += samples.len() as u64;

        let mut windows = Vec::new();

        match label {
            VadLabel::Speech => {
                self.silence_run = 0;
                if !self.in_speech {
                    // 起音确认阶段：把帧存入 preroll_buf，攒够 ATTACK_SAMPLES 才确认。
                    if self.preroll_buf.is_empty() {
                        // 记录本段预滚起始绝对偏移。
                        self.preroll_start_abs = self.abs_samples - samples.len() as u64;
                    }
                    self.preroll_buf.extend_from_slice(samples);
                    if self.preroll_buf.len() >= ATTACK_SAMPLES {
                        // 起音确认：把 overlap + preroll 全部转入 speech_buf。
                        self.in_speech = true;
                        // overlap 作为本段前缀（防**强制切窗**跨窗断词；utterance 正常收尾
                        // 不携带 overlap，见 emit_window）。overlap 是上一窗尾部的真实音频，
                        // 必须计入 abs_start，否则本窗全部时间戳晚 ~overlap 时长且解码端
                        // 无法按时间前沿识别重解码的尾字（跨窗重复根因之一）。
                        if self.overlap_buf.is_empty() {
                            self.speech_start_abs = self.preroll_start_abs;
                        } else {
                            self.speech_start_abs = self
                                .preroll_start_abs
                                .saturating_sub(self.overlap_buf.len() as u64);
                            let overlap = std::mem::take(&mut self.overlap_buf);
                            self.speech_buf.extend_from_slice(&overlap);
                        }
                        let preroll = std::mem::take(&mut self.preroll_buf);
                        self.speech_buf.extend_from_slice(&preroll);
                    }
                } else {
                    self.speech_buf.extend_from_slice(samples);
                    // 清空 preroll_buf（已确认语音，不再需要）。
                    self.preroll_buf.clear();
                }

                // 达窗长上限 → 终窗（强制切窗，保留 overlap 续接下一窗防断词）；
                // 否则积累够快照间隔 → 增长快照（同 abs_start）。
                if self.in_speech && self.speech_buf.len() >= self.cfg.window_samples {
                    if let Some(w) = self.emit_window(true) {
                        windows.push(w);
                    }
                } else if self.in_speech
                    && self.speech_buf.len() - self.emitted_len
                        >= self.cfg.snapshot_interval_samples
                {
                    windows.push(self.emit_snapshot());
                }
            }
            VadLabel::Silence => {
                if self.in_speech {
                    // 尾静音迟滞（hangover）：音节/词间隙（数百 ms）不终结 utterance——
                    // 静音帧照常并入缓冲（保持话语自然节奏，whisper 需要连续语境），
                    // 连续静音超 hangover 才判「说完」出窗。无迟滞会把连续话语切成
                    // 音节级碎窗（真机实时乱码根因：碎片+9s 补零 → 幻觉/误判语种）。
                    self.speech_buf.extend_from_slice(samples);
                    self.silence_run += samples.len();
                    if self.speech_buf.len() >= self.cfg.window_samples {
                        // 迟滞期也不越窗上限（强制切窗语义不变）。
                        if let Some(w) = self.emit_window(true) {
                            windows.push(w);
                        }
                    } else if self.silence_run >= self.cfg.hangover_samples {
                        // 说完：剔除已确认的尾静音（别把 hangover 静音垫喂给 whisper——
                        // 尾部静音正是幻觉温床），utterance 自然收尾。
                        let keep = self.speech_buf.len().saturating_sub(self.silence_run);
                        self.speech_buf.truncate(keep);
                        if self.speech_buf.len() >= self.cfg.min_speech_samples {
                            if let Some(w) = self.emit_window(false) {
                                windows.push(w);
                            }
                        } else {
                            // 语音太短，丢弃（幻觉抑制）
                            self.speech_buf.clear();
                            self.in_speech = false;
                            self.emitted_len = 0;
                        }
                        self.silence_run = 0;
                    }
                } else {
                    // 纯静音：复位预滚缓冲（防止瞬态误触发）
                    self.preroll_buf.clear();
                }
            }
        }

        windows
    }

    /// 手动停止（录音结束）：将缓冲中剩余语音产出最后一窗（若足够长）。
    pub fn flush(&mut self) -> Option<SpeechWindow> {
        // 剔除未定尾静音（同 hangover 收尾口径：静音垫不喂 whisper）。
        let keep = self.speech_buf.len().saturating_sub(self.silence_run);
        self.speech_buf.truncate(keep);
        self.silence_run = 0;
        if self.in_speech && self.speech_buf.len() >= self.cfg.min_speech_samples {
            self.emit_window(false)
        } else {
            self.speech_buf.clear();
            self.preroll_buf.clear();
            self.in_speech = false;
            self.emitted_len = 0;
            None
        }
    }

    /// 产出增长快照：speech_buf 全量拷贝（同 `abs_start`、padding 同终窗），**不重置**
    /// 段级状态——utterance 继续增长，终窗随后以同 `abs_start` 取代全部快照。
    fn emit_snapshot(&mut self) -> SpeechWindow {
        let speech_len = self.speech_buf.len();
        let speech_delta = speech_len - self.emitted_len;
        self.emitted_len = speech_len;

        let mut samples = self.speech_buf.clone();
        if samples.len() < self.cfg.window_samples {
            samples.resize(self.cfg.window_samples, 0.0);
        }
        SpeechWindow {
            samples,
            abs_start: self.speech_start_abs,
            speech_len,
            speech_delta,
            is_snapshot: true,
        }
    }

    /// 产出一个窗：取 speech_buf → padding → 构造 SpeechWindow → 重置。
    ///
    /// `carry_overlap`：仅**强制切窗**（窗长上限、话音仍在继续）时为 `true`——保留语音
    /// 尾部 overlap 拼到下一窗防断词；utterance 自然收尾（静音/flush）不携带。
    fn emit_window(&mut self, carry_overlap: bool) -> Option<SpeechWindow> {
        if self.speech_buf.is_empty() {
            return None;
        }
        let speech_len = self.speech_buf.len();
        let speech_delta = speech_len - self.emitted_len;
        let abs_start = self.speech_start_abs;

        // Padding：短窗补零到 window_samples（抑制幻觉）。
        let mut samples = std::mem::take(&mut self.speech_buf);
        if samples.len() < self.cfg.window_samples {
            samples.resize(self.cfg.window_samples, 0.0);
        }

        // 保留 overlap（取实际语音尾部，不取 padding 部分）。
        if carry_overlap {
            let tail_start = speech_len.saturating_sub(self.cfg.overlap_samples);
            self.overlap_buf = samples[tail_start..speech_len].to_vec();
        } else {
            self.overlap_buf.clear();
        }

        // 重置段级状态；下一段起点为当前 abs_samples（emit 后继续从这里计）。
        self.speech_start_abs = self.abs_samples;
        self.in_speech = false;
        self.preroll_buf.clear();
        self.emitted_len = 0;
        self.silence_run = 0;

        Some(SpeechWindow {
            samples,
            abs_start,
            speech_len,
            speech_delta,
            is_snapshot: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    fn cfg() -> SegmenterConfig {
        SegmenterConfig::new(SR)
    }

    /// 黄金夹具：开头即说话不吃头（连续足够语音即产窗）。
    #[test]
    fn speech_at_start_produces_window() {
        let mut seg = StreamSegmenter::new(cfg());
        let mut windows = Vec::new();
        // 连续喂 30s 语音（每次 320 样本 VAD 帧）
        let frame = vec![0.5f32; 320];
        for _ in 0..(30 * SR as usize / 320 + 1) {
            windows.extend(seg.push(&frame, VadLabel::Speech));
        }
        assert!(!windows.is_empty(), "30s 语音应产出至少一窗");
        let w = &windows[0];
        assert_eq!(
            w.samples.len(),
            cfg().window_samples,
            "窗长应 padding 到 window_samples"
        );
        assert!(
            w.abs_start < 320 * (ATTACK_SAMPLES / 320 + 2) as u64,
            "abs_start 应在会话头部"
        );
    }

    /// 黄金夹具：长静音不产窗。
    #[test]
    fn long_silence_produces_no_window() {
        let mut seg = StreamSegmenter::new(cfg());
        let frame = vec![0.0f32; 320];
        for _ in 0..(10 * SR as usize / 320) {
            let w = seg.push(&frame, VadLabel::Silence);
            assert!(w.is_empty(), "纯静音不应产窗");
        }
        assert!(seg.flush().is_none(), "flush 后仍无窗");
    }

    /// 黄金夹具：暂停后绝对时钟不计静默样本。
    #[test]
    fn pause_does_not_advance_abs_clock() {
        let mut seg = StreamSegmenter::new(cfg());
        // 喂一些语音
        let speech_frame = vec![0.5f32; 320];
        for _ in 0..10 {
            seg.push(&speech_frame, VadLabel::Speech);
        }
        let abs_before_pause = seg.abs_samples();

        // 暂停后喂样本
        seg.pause();
        for _ in 0..100 {
            seg.push(&speech_frame, VadLabel::Speech);
        }
        let abs_after_pause_push = seg.abs_samples();
        assert_eq!(
            abs_before_pause, abs_after_pause_push,
            "暂停期 abs_samples 不应增加"
        );

        // 恢复后正常计数
        seg.resume();
        seg.push(&speech_frame, VadLabel::Speech);
        assert_eq!(
            seg.abs_samples(),
            abs_before_pause + 320,
            "恢复后应正常计数"
        );
    }

    /// 增长快照：连续语音每 snapshot_interval 产同 abs_start 快照；终窗同 abs_start 取代快照。
    #[test]
    fn continuous_speech_emits_growing_snapshots_with_same_abs_start() {
        let mut seg = StreamSegmenter::new(cfg());
        let frame = vec![0.5f32; 320];
        let mut windows = Vec::new();
        // 喂 5s 连续语音（快照间隔 1.5s → 应产 ≥3 个快照，未达 10s 上限无终窗）。
        for _ in 0..(5 * SR as usize / 320) {
            windows.extend(seg.push(&frame, VadLabel::Speech));
        }
        assert!(windows.len() >= 3, "5s 连续语音应产 ≥3 个增长快照");
        let abs0 = windows[0].abs_start;
        assert!(
            windows.iter().all(|w| w.abs_start == abs0),
            "同一 utterance 的快照必须共享 abs_start"
        );
        for pair in windows.windows(2) {
            assert!(
                pair[1].speech_len > pair[0].speech_len,
                "快照 speech_len 必须单调增长"
            );
        }
        // 静音收尾（须超 hangover 迟滞，~700ms）→ 终窗与快照同 abs_start，speech_len 为全量。
        let mut finals = Vec::new();
        for _ in 0..40 {
            finals.extend(seg.push(&vec![0.0f32; 320], VadLabel::Silence));
        }
        assert_eq!(finals.len(), 1, "静音超迟滞应产终窗");
        assert_eq!(finals[0].abs_start, abs0, "终窗与快照同 abs_start");
        assert!(finals[0].speech_len >= windows.last().unwrap().speech_len);
    }

    /// 黄金夹具（真机实时乱码根因回归锁）：音节/词间隙（< hangover 的短静音）**绝不**
    /// 终结 utterance——旧行为一帧静音即切窗，把连续话语切成音节级碎窗（补零后有效占比
    /// 个位数 → whisper 幻觉/误判语种）。
    #[test]
    fn brief_pause_within_hangover_does_not_split_utterance() {
        let mut seg = StreamSegmenter::new(cfg());
        let speech = vec![0.5f32; 320];
        let quiet = vec![0.0f32; 320];
        let mut windows = Vec::new();
        // 1s 语音 → 400ms 间隙（< 700ms 迟滞）→ 1s 语音 → 超迟滞静音收尾。
        for _ in 0..(SR as usize / 320) {
            windows.extend(seg.push(&speech, VadLabel::Speech));
        }
        for _ in 0..20 {
            windows.extend(seg.push(&quiet, VadLabel::Silence)); // 400ms < hangover
        }
        for _ in 0..(SR as usize / 320) {
            windows.extend(seg.push(&speech, VadLabel::Speech));
        }
        for _ in 0..40 {
            windows.extend(seg.push(&quiet, VadLabel::Silence)); // 800ms ≥ hangover
        }
        let finals: Vec<_> = windows.iter().filter(|w| !w.is_snapshot).collect();
        assert_eq!(finals.len(), 1, "短间隙不得切窗——两段语音应在同一终窗");
        // 终窗涵盖两段语音 + 间隙（≈2.4s），且尾部 hangover 静音已剔除（<3s）。
        let len_s = finals[0].speech_len as f32 / SR as f32;
        assert!(
            (2.0..3.0).contains(&len_s),
            "终窗应含两段语音+间隙且剔除尾静音（实得 {len_s:.2}s）"
        );
    }

    /// speech_delta 增量：全部窗（快照+终窗）的 delta 之和 = 终窗全量 speech_len。
    #[test]
    fn speech_delta_sums_to_total_speech_len() {
        let mut seg = StreamSegmenter::new(cfg());
        let frame = vec![0.5f32; 320];
        let mut windows = Vec::new();
        for _ in 0..(5 * SR as usize / 320) {
            windows.extend(seg.push(&frame, VadLabel::Speech));
        }
        windows.extend(seg.push(&vec![0.0f32; 320], VadLabel::Silence));
        let total: usize = windows.iter().map(|w| w.speech_delta).sum();
        assert_eq!(
            total,
            windows.last().unwrap().speech_len,
            "delta 之和应等于终窗全量（RTF 不重复计音频）"
        );
    }

    /// 快照后进入下一 utterance：新窗 abs_start 前进、delta 计数复位。
    #[test]
    fn next_utterance_after_snapshot_resets_delta_and_advances_abs() {
        let mut seg = StreamSegmenter::new(cfg());
        let frame = vec![0.5f32; 320];
        let mut first: Vec<SpeechWindow> = Vec::new();
        for _ in 0..(2 * SR as usize / 320) {
            first.extend(seg.push(&frame, VadLabel::Speech));
        }
        first.extend(seg.push(&vec![0.0f32; 320], VadLabel::Silence));
        let abs0 = first[0].abs_start;
        // 1s 静音后再说 2s。
        for _ in 0..(SR as usize / 320) {
            seg.push(&vec![0.0f32; 320], VadLabel::Silence);
        }
        let mut second: Vec<SpeechWindow> = Vec::new();
        for _ in 0..(2 * SR as usize / 320) {
            second.extend(seg.push(&frame, VadLabel::Speech));
        }
        assert!(!second.is_empty(), "第二段应产窗");
        assert!(second[0].abs_start > abs0, "新 utterance abs_start 应前进");
        assert_eq!(
            second[0].speech_delta, second[0].speech_len,
            "新 utterance 首窗 delta = 全量"
        );
    }

    /// 静音收尾的 utterance 不携带 overlap：下一窗起点在静音之后、不含上一窗尾音
    /// （否则 whisper 会把上一窗尾字再转一遍 → 跨窗重复）。
    #[test]
    fn silence_ended_utterance_does_not_carry_overlap() {
        let mut seg = StreamSegmenter::new(cfg());
        let frame = vec![0.5f32; 320];
        // 第一段 2s 语音 + 静音收尾。
        let mut first: Vec<SpeechWindow> = Vec::new();
        for _ in 0..(2 * SR as usize / 320) {
            first.extend(seg.push(&frame, VadLabel::Speech));
        }
        first.extend(seg.push(&vec![0.0f32; 320], VadLabel::Silence));
        let w1 = first.last().expect("第一段应产终窗");
        let w1_end = w1.abs_start + w1.speech_len as u64;
        // 1s 静音后第二段 2s 语音。
        for _ in 0..(SR as usize / 320) {
            seg.push(&vec![0.0f32; 320], VadLabel::Silence);
        }
        let mut second: Vec<SpeechWindow> = Vec::new();
        for _ in 0..(2 * SR as usize / 320) {
            second.extend(seg.push(&frame, VadLabel::Speech));
        }
        let w2 = second.first().expect("第二段应产窗");
        assert!(
            w2.abs_start >= w1_end,
            "静音收尾后新窗起点必须在上一窗之后（不携带 overlap 尾音）：\
             w2.abs_start={} < w1_end={w1_end}",
            w2.abs_start
        );
    }

    /// 强制切窗（窗长上限）携带 overlap 且计入 abs_start：下一窗与上一窗按 overlap
    /// 时长重叠，时间轴逐样本对齐（解码端凭此按时间前沿去重尾字）。
    #[test]
    fn forced_split_carries_overlap_and_accounts_it_in_abs_start() {
        let c = cfg();
        let mut seg = StreamSegmenter::new(c);
        let frame = vec![0.5f32; 320];
        let mut windows: Vec<SpeechWindow> = Vec::new();
        // 25s 连续语音 → 至少 2 个 10s 终窗（快照窗与终窗同 abs_start，取 speech_len 最大者）。
        for _ in 0..(25 * SR as usize / 320) {
            windows.extend(seg.push(&frame, VadLabel::Speech));
        }
        // 按 abs_start 分组取每组最大 speech_len（终窗取代快照）。
        let mut finals: Vec<(u64, usize)> = Vec::new();
        for w in &windows {
            match finals.iter_mut().find(|(abs, _)| *abs == w.abs_start) {
                Some((_, len)) => *len = (*len).max(w.speech_len),
                None => finals.push((w.abs_start, w.speech_len)),
            }
        }
        assert!(finals.len() >= 2, "25s 连续语音应产生 ≥2 个窗组");
        let (abs1, len1) = finals[0];
        let (abs2, _) = finals[1];
        let w1_end = abs1 + len1 as u64;
        let expected = w1_end.saturating_sub(c.overlap_samples as u64);
        let tolerance = 2 * 320; // 允许两帧误差（起音确认帧界）
        assert!(
            abs2.abs_diff(expected) <= tolerance,
            "强制切窗后下一窗 abs_start 应 ≈ 上一窗末尾 - overlap：abs2={abs2} expected≈{expected}"
        );
    }

    /// 短语音（< min_speech_samples）不产窗（幻觉抑制）。
    #[test]
    fn short_speech_below_min_is_suppressed() {
        // min_speech_samples 设为大于 ATTACK_SAMPLES 的值，
        // 这样刚好确认起音后就遇到静音，语音长度不足 min，不应产窗。
        let mut seg = StreamSegmenter::new(SegmenterConfig {
            min_speech_samples: ATTACK_SAMPLES + 1600,
            ..cfg()
        });
        // 触发起音确认（刚好 ATTACK_SAMPLES）
        let frame = vec![0.5f32; ATTACK_SAMPLES];
        seg.push(&frame, VadLabel::Speech);
        // 立即静音（speech_buf.len() == ATTACK_SAMPLES < min_speech_samples）
        let w = seg.push(&vec![0.0f32; 320], VadLabel::Silence);
        assert!(w.is_empty(), "语音不足 min_speech_samples 不应产窗");
    }
}
