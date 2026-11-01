//! 电平/波形计量（RMS/峰值 + Rust 侧节流 + 二进制编码）。事实源：TDD-02 §3.4、ADR-011。
//!
//! **帧率口径唯一真相源 = Rust 侧节流（~15–20fps，上限可配）**；前端只用 rAF 合并到达
//! 事件，不自设速率（§3.4 与 TDD-07 §2/§8 对齐，勿写 30fps）。波形经 `ipc::Channel`
//! **二进制峰值字节**发送，**禁 JSON 数字数组**（ADR-011）。

/// 默认波形帧率（fps，§3.4 的 15–20fps 区间取中）。
pub const DEFAULT_WAVEFORM_FPS: u32 = 18;

/// 一个节流窗的电平（单声道 16k 上聚合）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelFrame {
    /// 窗内均方根幅度（0..1）。
    pub rms: f32,
    /// 窗内峰值绝对幅度（0..1）。
    pub peak: f32,
}

impl LevelFrame {
    /// 峰值量化为 1 字节（0..=255）供二进制波形通道（ADR-011）。
    #[must_use]
    pub fn peak_byte(self) -> u8 {
        (self.peak.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// RMS 量化为 1 字节。
    #[must_use]
    pub fn rms_byte(self) -> u8 {
        (self.rms.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

/// 把一批电平帧编码为二进制（每帧 2 字节：`[peak, rms]`），供 `ipc::Channel` 发送。
#[must_use]
pub fn encode_levels(frames: &[LevelFrame]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.len() * 2);
    for f in frames {
        out.push(f.peak_byte());
        out.push(f.rms_byte());
    }
    out
}

// ─── 轨标识（录音电平通道，TDD-02 §6 双轨电平 / PRD-04-052）────────────────────
//
// Both 双轨会话两条采集管线各产电平，**合并进单通道广播**（HUD 订阅面不变）：
// 消息首字节 = 轨 id，其余为该轨的 `[peak, rms]` 帧对（一条消息恒单轨——每条
// `CaptureEvent::Level` 本就来自单一管线，按消息打标零逐帧开销，ADR-011 二进制纪律）。

/// 主轨（麦克风 / 单源会话）轨 id。
pub const LEVEL_TRACK_PRIMARY: u8 = 0;
/// 副轨（Both 的系统音频轨）轨 id。
pub const LEVEL_TRACK_SYSTEM: u8 = 1;

/// 给一批 `[peak, rms]` 电平字节加轨前缀（首字节 = 轨 id）。
#[must_use]
pub fn tag_track_levels(track: u8, levels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(levels.len() + 1);
    out.push(track);
    out.extend_from_slice(levels);
    out
}

/// 波形计量器：把消费线程的变长 mono 帧聚合成固定帧率的 [`LevelFrame`]。
///
/// 按 `samples_per_tick = sample_rate / fps` 切窗；`push` 累积、满窗即产一帧。
/// 缓冲满丢最旧帧的"可丢"策略由上层 `ipc::Channel` 背压处理（§3.4）。
#[derive(Clone, Debug)]
pub struct WaveformMeter {
    samples_per_tick: usize,
    // 当前窗累积量：样本计数、平方和（算 RMS）、峰值。
    count: usize,
    sum_sq: f32,
    peak: f32,
}

impl WaveformMeter {
    /// 按采样率与目标帧率构造。
    ///
    /// `fps`/`sample_rate` 为 0 时退化为每 1 样本一帧（防除零）。
    #[must_use]
    pub fn new(sample_rate: u32, fps: u32) -> Self {
        let samples_per_tick = (sample_rate / fps.max(1)).max(1) as usize;
        Self {
            samples_per_tick,
            count: 0,
            sum_sq: 0.0,
            peak: 0.0,
        }
    }

    /// 用默认帧率构造（16k 目标采样率）。
    #[must_use]
    pub fn with_default_fps(sample_rate: u32) -> Self {
        Self::new(sample_rate, DEFAULT_WAVEFORM_FPS)
    }

    /// 喂入一段 mono 样本，返回本次跨越的所有满窗电平帧（变长帧 → 0..N 帧）。
    pub fn push(&mut self, samples: &[f32]) -> Vec<LevelFrame> {
        let mut frames = Vec::new();
        for &s in samples {
            let a = s.abs();
            self.sum_sq += s * s;
            if a > self.peak {
                self.peak = a;
            }
            self.count += 1;
            if self.count >= self.samples_per_tick {
                frames.push(self.take_frame());
            }
        }
        frames
    }

    /// 强制产出当前未满窗的残余帧（会话结束时调用）；无残余返回 `None`。
    pub fn flush(&mut self) -> Option<LevelFrame> {
        if self.count == 0 {
            None
        } else {
            Some(self.take_frame())
        }
    }

    fn take_frame(&mut self) -> LevelFrame {
        let rms = if self.count == 0 {
            0.0
        } else {
            (self.sum_sq / self.count as f32).sqrt()
        };
        let frame = LevelFrame {
            rms,
            peak: self.peak,
        };
        self.count = 0;
        self.sum_sq = 0.0;
        self.peak = 0.0;
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_one_frame_per_window() {
        // 16k @ 16fps = 1000 样本/窗。喂 2500 样本 → 2 满窗 + 残余 500。
        let mut m = WaveformMeter::new(16_000, 16);
        let frames = m.push(&vec![0.5_f32; 2500]);
        assert_eq!(frames.len(), 2);
        for f in &frames {
            assert!((f.peak - 0.5).abs() < 1e-6);
            assert!((f.rms - 0.5).abs() < 1e-6);
        }
        // 残余 500 由 flush 产出。
        assert!(m.flush().is_some());
        assert!(m.flush().is_none());
    }

    #[test]
    fn peak_tracks_max_abs() {
        let mut m = WaveformMeter::new(10, 10); // 1 样本/窗
        let frames = m.push(&[-0.9, 0.1]);
        assert_eq!(frames.len(), 2);
        assert!((frames[0].peak - 0.9).abs() < 1e-6);
    }

    #[test]
    fn binary_encoding_is_two_bytes_per_frame() {
        let frames = [
            LevelFrame {
                rms: 0.0,
                peak: 1.0,
            },
            LevelFrame {
                rms: 1.0,
                peak: 0.0,
            },
        ];
        let bytes = encode_levels(&frames);
        assert_eq!(bytes, vec![255, 0, 0, 255]);
    }

    #[test]
    fn track_tag_prefixes_single_byte() {
        // 双轨电平（TDD-02 §6）：消息 = [轨 id, peak, rms, …]；空帧组也带轨 id（合法消息）。
        assert_eq!(
            tag_track_levels(LEVEL_TRACK_SYSTEM, &[255, 0]),
            vec![1, 255, 0]
        );
        assert_eq!(tag_track_levels(LEVEL_TRACK_PRIMARY, &[]), vec![0]);
    }
}
