//! 无锁 SPSC 环形缓冲的**容量预算**（纯逻辑，可单测）。事实源：TDD-02 §3.1-A。
//!
//! 容量公式（**别拍脑袋给 2×**）：容量须能扛住**最坏消费停顿**——消费线程最坏会被
//! 一次重型推理或 GC 抢占阻塞数十~数百 ms。低估会丢转写音频（不可丢类，§5），故宁可偏大。
//!
//! 真正的 `rtrb::RingBuffer<f32>` 在采集模块（`capture`）按此容量构造；本模块只算容量，
//! 保持纯逻辑可在无设备的 headless CI 下回归。

/// 默认目标缓冲秒数（"最坏消费停顿 + 安全余量"，建议 ≥1s 可配，§3.1-A）。
pub const DEFAULT_BUFFER_SECS: f32 = 1.0;

/// 环形缓冲容量（**样本数**，含所有声道交错）。
///
/// `容量 = 采样率 × 声道数 × 目标缓冲秒数`，向上取整。
///
/// 例：48kHz 立体声、缓冲 1s ⇒ `48000 × 2 × 1 = 96000` 个 f32 ≈ 384KB（§3.1-A）。
///
/// # Panics
/// 入参为 0 时容量退化——调用方应保证 `sample_rate`/`channels` 来自设备探测（非 0）；
/// 这里对 0 做防御：至少返回 1（避免构造零容量环形缓冲）。
#[must_use]
pub fn ring_capacity(sample_rate: u32, channels: u16, buffer_secs: f32) -> usize {
    let raw = f64::from(sample_rate) * f64::from(channels) * f64::from(buffer_secs.max(0.0));
    (raw.ceil() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_tdd_worked_example() {
        // TDD-02 §3.1-A 工作例：48k 立体声 1s = 96000。
        assert_eq!(ring_capacity(48_000, 2, 1.0), 96_000);
    }

    #[test]
    fn mono_16k_default() {
        assert_eq!(ring_capacity(16_000, 1, DEFAULT_BUFFER_SECS), 16_000);
    }

    #[test]
    fn rounds_up_fractional_seconds() {
        // 44.1k mono 0.5s = 22050（整），1.3s 向上取整。
        assert_eq!(ring_capacity(44_100, 1, 0.5), 22_050);
        assert_eq!(ring_capacity(44_100, 1, 1.3), 57_330);
    }

    #[test]
    fn never_zero() {
        assert_eq!(ring_capacity(0, 0, 0.0), 1);
    }
}
