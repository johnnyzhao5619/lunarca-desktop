//! 双轨 → 立体声 WAV 离线混流（Both 双源录音的落盘收口，TDD-02 §3.1-I / PRD-04-052）。
//!
//! 设计取舍（ADR-019「落盘双轨」的容器选择）：**单一立体声文件（L=麦克风/我，R=系统/对方）**
//! 而非双文件——回放器/权威重转写/导出全部按"单文件单时间轴"设计，立体声容器让下游**零改动**；
//! 分轨信息仍完整保留（声道即轨），后续 L1 说话人细分可按声道直接归属。
//!
//! 离线混流（part 收尾时执行）而非实时交错写：避免把双时钟对齐问题引入实时写盘路径——
//! 两轨各自经独立 writer 落 16k mono，收尾时按样本对齐合并（短者尾部补零）。
//! **启动偏差校正（TDD-02 §3.1-I 任务④）**：两采集会话先后启动的毫秒级差由编排层用采集
//! 主机时钟锚点（`luna_audio::CaptureClock`）算得，转 16k 样本经 `lead_in_*` 传入——对**晚
//! 启动轨头部补零**，令两轨在同一主机时间轴上对齐。长会时钟漂移由消费线程 rate-aware
//! 重采样在轨内消除（§3.1-C），不在此处理。
//!
//! **轨级响度对齐（TDD-02 §3.1-I，真机「对方声音大、我方声音小」根因）**：系统轨是
//! 数字满电平信号、麦克风轨是房间声学电平，天然差 10–20dB——混流时各轨独立按
//! **活跃样本 RMS** 对齐到同一目标响度（长静默不拉低估计），增益带上下限防
//! 近静音轨被放大成底噪，并按峰值限幅防削波。离线全轨可见，估计稳定（优于实时 AGC）。

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::Path;

/// 响度对齐目标（活跃样本 RMS，线性幅度；≈ -20 dBFS，会议语音常用基线）。
const TARGET_ACTIVE_RMS: f32 = 0.1;
/// 活跃样本判定阈值（线性幅度；≈ -50 dBFS，低于此视为静默不计入响度估计）。
const ACTIVE_SAMPLE_THRESHOLD: f32 = 0.003_162;
/// 增益上下限（线性；±18dB）：防近静音轨爆底噪 / 满电平轨被过度压低。
const MAX_GAIN: f32 = 8.0;
const MIN_GAIN: f32 = 0.125;
/// 峰值限幅余量（增益后峰值不超过满刻度的 98%，防削波）。
const PEAK_HEADROOM: f32 = 0.98;

/// 计算单轨响度对齐增益（纯函数）：活跃样本 RMS → 目标响度的线性增益，
/// 经上下限与峰值限幅收口。无活跃样本（整轨静默）→ 1.0（不放大底噪）。
#[must_use]
fn track_gain(samples: &[i16]) -> f32 {
    let mut active_energy = 0.0f64;
    let mut active_count = 0usize;
    let mut peak = 0.0f32;
    for &s in samples {
        let a = f32::from(s) / 32768.0;
        let abs = a.abs();
        peak = peak.max(abs);
        if abs >= ACTIVE_SAMPLE_THRESHOLD {
            active_energy += f64::from(a) * f64::from(a);
            active_count += 1;
        }
    }
    if active_count == 0 {
        return 1.0;
    }
    let rms = (active_energy / active_count as f64).sqrt() as f32;
    let mut gain = (TARGET_ACTIVE_RMS / rms).clamp(MIN_GAIN, MAX_GAIN);
    if peak * gain > PEAK_HEADROOM {
        gain = PEAK_HEADROOM / peak;
    }
    gain
}

/// 对单样本施加增益（饱和取整，绝不回绕）。
#[inline]
fn apply_gain(sample: i16, gain: f32) -> i16 {
    (f32::from(sample) * gain)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

/// 混流错误（仅 IO/格式，无音频内容——日志脱敏）。
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    /// 读输入 WAV 失败。
    #[error("failed to read the track file: {0}")]
    Read(String),
    /// 输入格式不符（须同采样率、单声道、i16）。
    #[error("track format mismatch: {0}")]
    Format(String),
    /// 写输出失败。
    #[error("failed to write the stereo file: {0}")]
    Write(String),
}

/// 把两个**同采样率 16-bit 单声道** WAV 合并为立体声 WAV（`left`=麦克风，`right`=系统），
/// 并对每轨独立施加**响度对齐增益**（活跃 RMS → 目标响度，见模块注）。
///
/// `lead_in_left`/`lead_in_right` = 各轨**头部补零样本数**（启动偏差对齐，§3.1-I 任务④）：
/// 对晚启动轨头部补零使两轨落到同一主机时间轴；通常至多一轨非零（另一轨为 0）。
///
/// 长度不等时短者尾部补零（数字静默）；返回输出**帧数**（= 含 lead-in 的较长轨样本数）。
/// 输入文件保持原样（调用方决定是否删除）。
///
/// # Errors
/// 输入缺失/格式不符/写失败时返回 [`MuxError`]，不产生半成品输出文件（写失败时清理）。
pub fn mux_mono_to_stereo(
    left: &Path,
    right: &Path,
    out: &Path,
    lead_in_left: u64,
    lead_in_right: u64,
) -> Result<u64, MuxError> {
    let (l_spec, l_samples) = read_mono_i16(left)?;
    let (r_spec, r_samples) = read_mono_i16(right)?;
    if l_spec.sample_rate != r_spec.sample_rate {
        return Err(MuxError::Format(format!(
            "采样率不一致: {} vs {}",
            l_spec.sample_rate, r_spec.sample_rate
        )));
    }
    // 响度增益按**原始样本**估计（头部补零是 <阈值 的数字静默，不影响活跃 RMS 估计）。
    let l_gain = track_gain(&l_samples);
    let r_gain = track_gain(&r_samples);

    let lead_l = lead_in_left as usize;
    let lead_r = lead_in_right as usize;
    let frames = (lead_l + l_samples.len()).max(lead_r + r_samples.len());
    // 头部补零的取样器：i < lead 段为数字静默，其后取原轨 i-lead 处（越界=尾部补零）。
    let sample_at = |samples: &[i16], lead: usize, i: usize| -> i16 {
        if i < lead {
            0
        } else {
            samples.get(i - lead).copied().unwrap_or(0)
        }
    };
    let spec = WavSpec {
        channels: 2,
        sample_rate: l_spec.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(out, spec).map_err(|e| MuxError::Write(e.to_string()))?;
    let result: Result<(), hound::Error> = (0..frames).try_for_each(|i| {
        writer.write_sample(apply_gain(sample_at(&l_samples, lead_l, i), l_gain))?;
        writer.write_sample(apply_gain(sample_at(&r_samples, lead_r, i), r_gain))
    });
    let finalized = result
        .map_err(|e| MuxError::Write(e.to_string()))
        .and_then(|()| {
            writer
                .finalize()
                .map_err(|e| MuxError::Write(e.to_string()))
        });
    if let Err(e) = finalized {
        let _ = std::fs::remove_file(out); // 不留半成品
        return Err(e);
    }
    Ok(frames as u64)
}

fn read_mono_i16(path: &Path) -> Result<(WavSpec, Vec<i16>), MuxError> {
    let mut reader = WavReader::open(path).map_err(|e| MuxError::Read(e.to_string()))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 16 || spec.sample_format != SampleFormat::Int {
        return Err(MuxError::Format(format!(
            "期望 16-bit 单声道 PCM，实际 {}ch/{}bit",
            spec.channels, spec.bits_per_sample
        )));
    }
    let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
    Ok((spec, samples.map_err(|e| MuxError::Read(e.to_string()))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_mono(path: &Path, rate: u32, samples: &[i16]) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("luna-muxer-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn mux_interleaves_and_pads_shorter_track() {
        let l = tmp("l.wav");
        let r = tmp("r.wav");
        let out = tmp("out.wav");
        let l_samples = [100i16, 200, 300];
        let r_samples = [-1i16, -2]; // 短一帧 → 尾部补零
        write_mono(&l, 16_000, &l_samples);
        write_mono(&r, 16_000, &r_samples);
        let frames = mux_mono_to_stereo(&l, &r, &out, 0, 0).unwrap();
        assert_eq!(frames, 3);

        let mut rd = WavReader::open(&out).unwrap();
        assert_eq!(rd.spec().channels, 2);
        assert_eq!(rd.spec().sample_rate, 16_000);
        let all: Vec<i16> = rd.samples::<i16>().map(Result::unwrap).collect();
        // 交错结构 + 补零位（各轨样本经响度对齐增益，期望值经同一纯函数推导）。
        let lg = track_gain(&l_samples);
        let rg = track_gain(&r_samples);
        let expect: Vec<i16> = vec![
            apply_gain(100, lg),
            apply_gain(-1, rg),
            apply_gain(200, lg),
            apply_gain(-2, rg),
            apply_gain(300, lg),
            0,
        ];
        assert_eq!(all, expect);
        assert_eq!(all[5], 0, "补零样本经增益仍为数字静默");
    }

    // ── 轨级响度对齐（TDD-02 §3.1-I：mic 房间电平 vs system 数字满电平）────────

    /// 生成幅度为 `amp` 的 440Hz 正弦（i16）。
    fn tone_i16(amp: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                ((2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * amp * 32767.0)
                    as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f32 {
        let sum: f64 = samples
            .iter()
            .map(|&s| f64::from(s) / 32768.0)
            .map(|a| a * a)
            .sum();
        ((sum / samples.len() as f64) as f32).sqrt()
    }

    #[test]
    fn quiet_mic_track_is_boosted_and_loud_system_track_attenuated() {
        // 安静 mic（≈-40dBFS RMS）提升、满电平 system（≈-1dBFS RMS）压低——两轨靠拢目标响度。
        let quiet = tone_i16(0.014, 16_000);
        let loud = tone_i16(0.9, 16_000);
        let qg = track_gain(&quiet);
        let lg = track_gain(&loud);
        assert!(qg > 4.0, "安静轨应显著提升（实得 {qg}）");
        assert!(lg < 0.3, "满电平轨应显著压低（实得 {lg}）");
        let boosted: Vec<i16> = quiet.iter().map(|&s| apply_gain(s, qg)).collect();
        let attenuated: Vec<i16> = loud.iter().map(|&s| apply_gain(s, lg)).collect();
        let (br, ar) = (rms(&boosted), rms(&attenuated));
        // 对齐后两轨 RMS 差距应在一个数量级内（原始差 ~36dB）。
        assert!(
            br * 4.0 > ar && ar * 4.0 > br,
            "对齐后响度应可比：mic {br} vs sys {ar}"
        );
    }

    #[test]
    fn silent_track_keeps_unit_gain() {
        // 整轨静默（无活跃样本）不放大底噪。
        assert_eq!(track_gain(&[0, 1, -1, 2]), 1.0);
    }

    #[test]
    fn peak_headroom_prevents_clipping() {
        // 低 RMS 但含高峰值的轨：增益受峰值限幅约束，绝不削波。
        let mut samples = tone_i16(0.01, 16_000);
        samples[8_000] = (0.9 * 32767.0) as i16;
        let g = track_gain(&samples);
        let peak_after = samples
            .iter()
            .map(|&s| apply_gain(s, g).unsigned_abs())
            .max()
            .unwrap();
        assert!(
            f32::from(peak_after) / 32768.0 <= PEAK_HEADROOM + 0.01,
            "增益后峰值须在限幅余量内（实得 {peak_after}）"
        );
    }

    #[test]
    fn mux_rejects_rate_mismatch_and_keeps_no_output() {
        let l = tmp("l2.wav");
        let r = tmp("r2.wav");
        let out = tmp("out2.wav");
        write_mono(&l, 16_000, &[1]);
        write_mono(&r, 48_000, &[1]);
        assert!(matches!(
            mux_mono_to_stereo(&l, &r, &out, 0, 0),
            Err(MuxError::Format(_))
        ));
    }

    #[test]
    fn mux_rejects_missing_input() {
        let out = tmp("out3.wav");
        assert!(matches!(
            mux_mono_to_stereo(
                Path::new("/nonexistent-l.wav"),
                Path::new("/nonexistent-r.wav"),
                &out,
                0,
                0
            ),
            Err(MuxError::Read(_))
        ));
    }

    #[test]
    fn lead_in_pads_late_track_head_and_aligns_timeline() {
        // 启动偏差校正（§3.1-I 任务④）：右轨（系统）晚启动 2 样本 → 右轨头部补 2 零，
        // 使两轨对齐到同一主机时间轴；左轨无补零、右轨整体后移。
        let l = tmp("lead_l.wav");
        let r = tmp("lead_r.wav");
        let out = tmp("lead_out.wav");
        write_mono(&l, 16_000, &[100, 200, 300]);
        write_mono(&r, 16_000, &[-10, -20, -30]);
        let frames = mux_mono_to_stereo(&l, &r, &out, 0, 2).unwrap();
        assert_eq!(frames, 5, "帧数 = max(0+3, 2+3) = 5");

        let mut rd = WavReader::open(&out).unwrap();
        let all: Vec<i16> = rd.samples::<i16>().map(Result::unwrap).collect();
        let lg = track_gain(&[100, 200, 300]);
        let rg = track_gain(&[-10, -20, -30]);
        // 交错帧：L 无补零、尾部补零；R 头部补 2 零后接原样本。
        let expect: Vec<i16> = vec![
            apply_gain(100, lg),
            0, // 帧0：R 头部补零
            apply_gain(200, lg),
            0, // 帧1：R 头部补零
            apply_gain(300, lg),
            apply_gain(-10, rg), // 帧2：R 首个真实样本
            0,
            apply_gain(-20, rg), // 帧3：L 尾部补零
            0,
            apply_gain(-30, rg), // 帧4：L 尾部补零
        ];
        assert_eq!(all, expect);
    }
}
