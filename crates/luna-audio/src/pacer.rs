//! Windows WASAPI loopback 的静音期补零节奏器（纯逻辑，headless 可单测）。
//! 事实源：TDD-02 §3.1-I —— loopback 流在系统**无声音播放时不产帧**，消费线程须按主机
//! 时钟补零对齐时间轴，否则落盘/转写时间轴被静默压缩（对齐 S11/调研口径）。
//!
//! 设计：调用方（消费线程）注入 `elapsed`（会话启动至今的主机时钟），本组件只做
//! 「期望样本数 −（真实到达 + 已补零）」的差额核算——不持有时钟，保证确定性单测。

use std::time::Duration;

/// 每次补零的最小差额（毫秒）：低于此值不补，避免与真实回调的正常抖动打架。
pub const PACER_MIN_GAP_MS: u64 = 100;

/// 单次补零上限（毫秒）：长挂起（睡眠/冻结）恢复后不一次性泄洪，分批补齐。
pub const PACER_MAX_CHUNK_MS: u64 = 1_000;

/// loopback 静音补零节奏器。
///
/// 单位约定：全部按**交错样本数**（帧数 × 声道数）核算，产出恒为声道对齐。
#[derive(Debug)]
pub struct LoopbackPacer {
    sample_rate: u32,
    channels: u16,
    /// 真实到达的交错样本累计。
    delivered: u64,
    /// 已补零的交错样本累计。
    injected: u64,
}

impl LoopbackPacer {
    /// 以设备采样率/声道数构造（与采集流一致）。
    #[must_use]
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels: channels.max(1),
            delivered: 0,
            injected: 0,
        }
    }

    /// 记录一批真实到达的交错样本。
    pub fn on_delivered(&mut self, interleaved_samples: usize) {
        self.delivered += interleaved_samples as u64;
    }

    /// 按主机时钟核算当前应补的零样本数（交错、声道对齐）；不足最小差额返回 0。
    ///
    /// 返回值已计入内部账目——调用方**必须**把等量零样本喂给管线，否则账目漂移。
    pub fn take_silence_deficit(&mut self, elapsed: Duration) -> usize {
        let ch = u64::from(self.channels);
        let expected_frames =
            (elapsed.as_micros() as u64) * u64::from(self.sample_rate) / 1_000_000;
        let expected = expected_frames * ch;
        let accounted = self.delivered + self.injected;
        let deficit = expected.saturating_sub(accounted);

        let min_gap = u64::from(self.sample_rate) * PACER_MIN_GAP_MS / 1_000 * ch;
        if deficit < min_gap {
            return 0;
        }
        let max_chunk = u64::from(self.sample_rate) * PACER_MAX_CHUNK_MS / 1_000 * ch;
        let take = deficit.min(max_chunk) / ch * ch; // 声道对齐
        self.injected += take;
        take as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_deficit_when_data_keeps_up() {
        let mut p = LoopbackPacer::new(48_000, 2);
        p.on_delivered(48_000 * 2); // 1s 的数据
        assert_eq!(p.take_silence_deficit(Duration::from_secs(1)), 0);
    }

    #[test]
    fn small_jitter_below_min_gap_not_filled() {
        let mut p = LoopbackPacer::new(48_000, 2);
        p.on_delivered(48_000 * 2 - 400); // 差 200 帧 ≈ 4ms
        assert_eq!(p.take_silence_deficit(Duration::from_secs(1)), 0);
    }

    #[test]
    fn silence_gap_filled_channel_aligned() {
        let mut p = LoopbackPacer::new(16_000, 2);
        // 1s 全静音（无真实数据）→ 期望 16000 帧 × 2ch = 32000 样本，且 ≤ 单次上限。
        let take = p.take_silence_deficit(Duration::from_secs(1));
        assert_eq!(take, 32_000);
        assert_eq!(take % 2, 0);
        // 立即再取：账目已计入，不重复补。
        assert_eq!(p.take_silence_deficit(Duration::from_secs(1)), 0);
    }

    #[test]
    fn long_suspend_drains_in_capped_chunks() {
        let mut p = LoopbackPacer::new(48_000, 2);
        // 10s 挂起后恢复：单次最多补 PACER_MAX_CHUNK_MS。
        let cap = (48_000 * PACER_MAX_CHUNK_MS as usize / 1_000) * 2;
        let take1 = p.take_silence_deficit(Duration::from_secs(10));
        assert_eq!(take1, cap);
        let take2 = p.take_silence_deficit(Duration::from_secs(10));
        assert_eq!(take2, cap);
    }

    #[test]
    fn resumes_accounting_after_real_data() {
        let mut p = LoopbackPacer::new(16_000, 1);
        // 1s 静音补齐后，第 2 秒真实数据到达 → 第 2 秒末无差额。
        assert_eq!(p.take_silence_deficit(Duration::from_secs(1)), 16_000);
        p.on_delivered(16_000);
        assert_eq!(p.take_silence_deficit(Duration::from_secs(2)), 0);
    }
}
