//! RTF（实时率）滑窗计算器。事实源：TDD-02 §5.1。
//!
//! `RtfTracker` 是可复用算子——实例由 `RealtimeSession`（编排层）持有，
//! 不在 `luna-audio` 内持有决策（TDD-02 §5.2 决策归属）。

use std::collections::VecDeque;

/// 默认滑动窗大小（采样数）。
pub const DEFAULT_RTF_WINDOW: usize = 5;

/// RTF 滑窗计算器（TDD-02 §5.1 逐行实现）：滑窗内 **解码耗时之和 ÷ 流过的会话时长之和**，
/// 即解码器的忙碌占比——它 ≥ 1 才真的跟不上实时。
///
/// 分母是**会话时长**（消费线程按 `SpeechWindow::elapsed_samples` 累计，含静音与被跳过的窗），
/// 不是窗里有多少语音：底噪峰值切出的短窗每个都付一次整窗解码，按语音时长算会把一成多的
/// 真实负载算成 1–2.6。`T_inference` 由 ASR worker 内部结果通道回传（不污染对外 `JobEvent`）。
/// 聚合与决策归 `RealtimeSession`（TDD-01 §5.9）。
pub struct RtfTracker {
    window_infer_times: VecDeque<f32>,
    window_elapsed_times: VecDeque<f32>,
    limit: usize,
}

impl RtfTracker {
    /// 以给定滑窗大小构造（默认 5 个采样点）。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            window_infer_times: VecDeque::with_capacity(limit),
            window_elapsed_times: VecDeque::with_capacity(limit),
            limit,
        }
    }

    /// 默认窗大小（5）的构造器。
    #[must_use]
    pub fn default_window() -> Self {
        Self::new(DEFAULT_RTF_WINDOW)
    }

    /// 添加一次测量（`infer_ms` = 这段时间里完成的 ASR 推理耗时，`elapsed_ms` = 这段时间
    /// 流过的会话时长）。
    pub fn add_measurement(&mut self, infer_ms: f32, elapsed_ms: f32) {
        if self.window_infer_times.len() >= self.limit {
            self.window_infer_times.pop_front();
            self.window_elapsed_times.pop_front();
        }
        self.window_infer_times.push_back(infer_ms);
        self.window_elapsed_times.push_back(elapsed_ms);
    }

    /// 当前滑窗 RTF（= 推理总时长 / 会话时长总和；无数据时为 0）。
    #[must_use]
    pub fn current_rtf(&self) -> f32 {
        let total_infer: f32 = self.window_infer_times.iter().sum();
        let total_audio: f32 = self.window_elapsed_times.iter().sum();
        if total_audio == 0.0 {
            0.0
        } else {
            total_infer / total_audio
        }
    }

    /// 当前采样数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.window_infer_times.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.window_infer_times.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_zero_when_empty() {
        let tracker = RtfTracker::default_window();
        assert_eq!(tracker.current_rtf(), 0.0);
    }

    #[test]
    fn rtf_equals_infer_over_audio() {
        let mut tracker = RtfTracker::new(3);
        // RTF = (100+200) / (200+400) = 300/600 = 0.5
        tracker.add_measurement(100.0, 200.0);
        tracker.add_measurement(200.0, 400.0);
        let rtf = tracker.current_rtf();
        assert!((rtf - 0.5).abs() < 1e-5, "expected 0.5, got {rtf}");
    }

    #[test]
    fn sliding_window_evicts_oldest() {
        let mut tracker = RtfTracker::new(2);
        // 先填满
        tracker.add_measurement(500.0, 100.0); // RTF=5.0
        tracker.add_measurement(100.0, 100.0); // RTF=3.0（均值）
                                               // 再加一条，第一条应被弹出
        tracker.add_measurement(50.0, 100.0); // 窗内: (100+50)/(100+100) = 0.75
        let rtf = tracker.current_rtf();
        assert!((rtf - 0.75).abs() < 1e-5, "expected 0.75, got {rtf}");
        assert_eq!(tracker.len(), 2);
    }

    #[test]
    fn rtf_zero_when_audio_zero() {
        let mut tracker = RtfTracker::new(3);
        tracker.add_measurement(100.0, 0.0);
        assert_eq!(tracker.current_rtf(), 0.0, "分母为零时应返回 0");
    }
}
