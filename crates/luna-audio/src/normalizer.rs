//! 音频归一化：任意设备格式 → **16k mono f32**。事实源：TDD-02 §3.1-B/C。
//!
//! 四步（§3.1-B/C/J）：
//! 1. **全 SampleFormat 分支统一转 f32**（i16 `/32768`、u16 减偏置再 `/32768`）。
//! 2. **去交错并降混为单声道**（多声道取均值）。
//! 3. **rubato 抗混叠重采样**到 16k（sinc 带低通）——**禁 naive 抽取**（混叠对 ASR 是灾难）。
//! 4. **一阶高通去直流（DC blocker，~80Hz）**——移除低频电流噪声（见下）。
//!
//! **去直流高通（§3.1-J，真机「录音总有电流声」根因）**：麦克风/声卡的**直流偏置 + 50/60Hz
//! 市电嗡声 + 亚音速隆隆声**会原样保留到落盘 WAV 与 ASR 输入。语音基频 ≥85Hz，80Hz 高通对人声
//! 无损却干净滤除低频电流噪声，并提升 ASR 输入信噪比。落盘 WAV / 波形 / ASR 三路同源归一化输出，
//! 故一处施加即全覆盖（[`crate::pipeline`] `process` 唯一收口）。
//!
//! 重采样移到环形缓冲**之后**（消费线程内），**绝不在 cpal 回调里做**（ADR-009，§2 修正 1）。

use crate::types::{MonoF32Frame, RawFrame, RawSamples};
use crate::TARGET_SAMPLE_RATE;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// 重采样器每次处理的输入帧数（SincFixedIn 定长输入；越大吞吐越高、延迟越大）。
const RESAMPLE_CHUNK: usize = 1024;

/// 去直流高通截止频率（Hz）。语音基频 ≥85Hz，取 80Hz 对人声无损、干净滤除直流与市电嗡声。
const HIGH_PASS_CUTOFF_HZ: f32 = 80.0;

/// 一阶去直流高通滤波器（DC blocker）：`y[n] = R·(y[n-1] + x[n] − x[n-1])`。
///
/// 移除直流偏置 + 亚音速隆隆 + 大部分 50/60Hz 市电嗡声（「电流声」）。极点 `R` 由截止频率与
/// 采样率解算：`R = 1 − 2π·fc/fs`。**有状态**（跨帧连续），随 [`AudioNormalizer`] 每会话一个。
struct DcBlocker {
    r: f32,
    prev_in: f32,
    prev_out: f32,
}

impl DcBlocker {
    fn new(cutoff_hz: f32, sample_rate: u32) -> Self {
        let r =
            (1.0 - 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate as f32).clamp(0.0, 0.9999);
        Self {
            r,
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    /// 原地滤波一批样本（连续帧共享状态）。
    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let xn = *x;
            let yn = self.r * (self.prev_out + xn - self.prev_in);
            self.prev_in = xn;
            self.prev_out = yn;
            *x = yn;
        }
    }
}

/// 归一化错误。
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    /// rubato 重采样器构造/处理失败。
    #[error("resample failed: {0}")]
    Resample(String),
}

/// 把任意 [`RawSamples`] 转为 f32（交错布局不变）。
#[must_use]
pub fn to_f32_interleaved(samples: &RawSamples) -> Vec<f32> {
    match samples {
        RawSamples::F32(v) => v.clone(),
        RawSamples::I16(v) => v.iter().map(|&s| f32::from(s) / 32768.0).collect(),
        // u16：以 32768 为零点，映射到 [-1, 1)。
        RawSamples::U16(v) => v
            .iter()
            .map(|&s| (f32::from(s) - 32768.0) / 32768.0)
            .collect(),
    }
}

/// 把交错 f32 按声道数降混为单声道（取声道均值）。`channels==0` 视为 1。
#[must_use]
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// 归一化器（每采集会话一个；持有重采样状态，**有状态、不可跨会话复用**）。
///
/// 输入设备采样率固定（采集会话内不变），输出恒为 [`TARGET_SAMPLE_RATE`] 单声道。
/// 设备率 == 16k 时为 passthrough（不建重采样器、零额外延迟）。
pub struct AudioNormalizer {
    input_sample_rate: u32,
    // None = passthrough（输入已是 16k）。
    resampler: Option<SincFixedIn<f32>>,
    // 重采样输入累积缓冲（mono，攒够 RESAMPLE_CHUNK 再喂）。
    in_buf: Vec<f32>,
    // 去直流高通（运行于 16k 输出侧，跨帧连续）。
    dc_blocker: DcBlocker,
    // 累计 16k mono 输出样本数（时钟漂移闭环的控制观测量，TDD-02 §3.1-I）。
    total_output_samples: u64,
}

impl AudioNormalizer {
    /// 以设备输入采样率构造。
    ///
    /// # Errors
    /// rubato 重采样器构造失败时返回 [`NormalizeError::Resample`]。
    pub fn new(input_sample_rate: u32) -> Result<Self, NormalizeError> {
        let resampler = if input_sample_rate == TARGET_SAMPLE_RATE {
            None
        } else {
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let ratio = f64::from(TARGET_SAMPLE_RATE) / f64::from(input_sample_rate);
            let r = SincFixedIn::<f32>::new(ratio, 1.1, params, RESAMPLE_CHUNK, 1)
                .map_err(|e| NormalizeError::Resample(e.to_string()))?;
            Some(r)
        };
        Ok(Self {
            input_sample_rate,
            resampler,
            in_buf: Vec::new(),
            dc_blocker: DcBlocker::new(HIGH_PASS_CUTOFF_HZ, TARGET_SAMPLE_RATE),
            total_output_samples: 0,
        })
    }

    /// 会话累计 16k mono 输出样本数（时钟漂移闭环的控制观测量，§3.1-I）。
    #[must_use]
    pub fn total_output_samples(&self) -> u64 {
        self.total_output_samples
    }

    /// rate-aware 微调重采样比例（相对基准 ratio；漂移校正用，§3.1-C）。
    ///
    /// passthrough（设备率 == 16k，无重采样器）时不适用，返回 `false`（诚实：无校正手段）。
    /// `rel` 越界（超出构造时 `max_relative_ratio`）由 rubato 拒绝并返回 `false`。
    pub fn set_resample_ratio_relative(&mut self, rel: f64) -> bool {
        match self.resampler.as_mut() {
            // ramp=true：跨块线性过渡，避免 ratio 阶跃引入音染（§3.1-C「防音染」）。
            Some(r) => r.set_resample_ratio_relative(rel, true).is_ok(),
            None => false,
        }
    }

    /// 设备输入采样率。
    #[must_use]
    pub fn input_sample_rate(&self) -> u32 {
        self.input_sample_rate
    }

    /// 处理一帧原始采集数据 → 0..N 个 16k mono 样本（变长，因重采样按定长块处理）。
    ///
    /// # Errors
    /// rubato 处理失败时返回 [`NormalizeError::Resample`]。
    pub fn process(&mut self, raw: &RawFrame) -> Result<MonoF32Frame, NormalizeError> {
        let f32s = to_f32_interleaved(&raw.samples);
        let mono = downmix_to_mono(&f32s, raw.channels);
        let mut samples = self.resample_mono(&mono)?;
        // 去直流高通（滤除电流声/市电嗡声/隆隆，§3.1-J）——落盘/波形/ASR 三路同源此输出。
        self.dc_blocker.process(&mut samples);
        self.total_output_samples += samples.len() as u64;
        Ok(MonoF32Frame {
            sample_rate: TARGET_SAMPLE_RATE,
            samples,
        })
    }

    /// 会话结束：把重采样器内残余（不足一块）补零冲出，避免丢尾。
    ///
    /// # Errors
    /// rubato 处理失败时返回 [`NormalizeError::Resample`]。
    pub fn flush(&mut self) -> Result<Vec<f32>, NormalizeError> {
        let Some(resampler) = self.resampler.as_mut() else {
            // passthrough：残余即 in_buf（passthrough 不入 in_buf，故为空）。
            return Ok(Vec::new());
        };
        if self.in_buf.is_empty() {
            return Ok(Vec::new());
        }
        // 补零到一个完整块后处理一次。
        self.in_buf.resize(RESAMPLE_CHUNK, 0.0);
        let out = resampler
            .process(&[std::mem::take(&mut self.in_buf)], None)
            .map_err(|e| NormalizeError::Resample(e.to_string()))?;
        let mut tail = out.into_iter().next().unwrap_or_default();
        // 收尾残余同样过高通，保持滤波连续。
        self.dc_blocker.process(&mut tail);
        self.total_output_samples += tail.len() as u64;
        Ok(tail)
    }

    fn resample_mono(&mut self, mono: &[f32]) -> Result<Vec<f32>, NormalizeError> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(mono.to_vec()); // passthrough（16k）。
        };
        self.in_buf.extend_from_slice(mono);
        let mut out = Vec::new();
        while self.in_buf.len() >= RESAMPLE_CHUNK {
            let chunk: Vec<f32> = self.in_buf.drain(..RESAMPLE_CHUNK).collect();
            let resampled = resampler
                .process(&[chunk], None)
                .map_err(|e| NormalizeError::Resample(e.to_string()))?;
            if let Some(ch0) = resampled.into_iter().next() {
                out.extend(ch0);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_full_scale_maps_near_unit() {
        let s = RawSamples::I16(vec![i16::MAX, i16::MIN, 0]);
        let f = to_f32_interleaved(&s);
        assert!((f[0] - 0.99997).abs() < 1e-3);
        assert!((f[1] + 1.0).abs() < 1e-3);
        assert_eq!(f[2], 0.0);
    }

    #[test]
    fn u16_midpoint_is_zero() {
        let s = RawSamples::U16(vec![32768, 65535, 0]);
        let f = to_f32_interleaved(&s);
        assert_eq!(f[0], 0.0);
        assert!((f[1] - 1.0).abs() < 1e-3);
        assert!((f[2] + 1.0).abs() < 1e-3);
    }

    #[test]
    fn stereo_downmix_averages_channels() {
        // L=1.0, R=0.0 交错 → mono 0.5。
        let inter = [1.0, 0.0, 1.0, 0.0];
        let mono = downmix_to_mono(&inter, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn passthrough_at_16k_preserves_length() {
        let mut n = AudioNormalizer::new(16_000).unwrap();
        assert!(n.resampler.is_none());
        let raw = RawFrame {
            sample_rate: 16_000,
            channels: 1,
            samples: RawSamples::F32(vec![0.1; 800]),
        };
        let out = n.process(&raw).unwrap();
        assert_eq!(out.sample_rate, 16_000);
        assert_eq!(out.samples.len(), 800);
    }

    #[test]
    fn resample_48k_to_16k_reduces_length_roughly_threefold() {
        let mut n = AudioNormalizer::new(48_000).unwrap();
        assert!(n.resampler.is_some());
        // 喂 48000 个样本（1s @48k）→ 期望 ~16000 个（1s @16k），含重采样器延迟容差。
        let raw = RawFrame {
            sample_rate: 48_000,
            channels: 1,
            samples: RawSamples::F32(vec![0.2; 48_000]),
        };
        let mut total = n.process(&raw).unwrap().samples.len();
        total += n.flush().unwrap().len();
        // sinc 重采样器有起始延迟，允许 ±5% 容差。
        let expected = 16_000.0;
        let ratio = total as f64 / expected;
        assert!(
            (0.95..=1.05).contains(&ratio),
            "48k→16k 长度应约为 1/3: got {total} (expected ~16000, ratio {ratio:.3})"
        );
    }

    #[test]
    fn high_pass_removes_dc_offset() {
        // 去直流高通（电流声修复）：恒定直流 0.2 输入 → 输出稳态均值应趋近 0。
        let mut n = AudioNormalizer::new(16_000).unwrap();
        let raw = RawFrame {
            sample_rate: 16_000,
            channels: 1,
            samples: RawSamples::F32(vec![0.2; 16_000]),
        };
        let out = n.process(&raw).unwrap().samples;
        // 取尾段（滤波已收敛）。
        let tail = &out[out.len() * 3 / 4..];
        let mean: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.01, "直流应被高通移除, got {mean}");
    }

    #[test]
    fn high_pass_passes_voice_band_tone() {
        // 200Hz 正弦（人声基频带内）应基本保留幅度（高通仅切 <80Hz）。
        let mut n = AudioNormalizer::new(16_000).unwrap();
        let n_samples = 16_000usize;
        let tone: Vec<f32> = (0..n_samples)
            .map(|i| (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let raw = RawFrame {
            sample_rate: 16_000,
            channels: 1,
            samples: RawSamples::F32(tone),
        };
        let out = n.process(&raw).unwrap().samples;
        let tail = &out[out.len() / 2..];
        let peak = tail.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.4, "200Hz 人声带内应基本保留（峰值 {peak}）");
    }

    #[test]
    fn high_pass_attenuates_low_freq_hum() {
        // 低频「电流声」代表（30Hz）应被显著衰减（相对 200Hz 人声带）。
        fn peak_after(freq: f32) -> f32 {
            let mut n = AudioNormalizer::new(16_000).unwrap();
            let tone: Vec<f32> = (0..16_000)
                .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / 16_000.0).sin() * 0.5)
                .collect();
            let raw = RawFrame {
                sample_rate: 16_000,
                channels: 1,
                samples: RawSamples::F32(tone),
            };
            let out = n.process(&raw).unwrap().samples;
            out[out.len() / 2..]
                .iter()
                .fold(0.0f32, |m, &s| m.max(s.abs()))
        }
        assert!(
            peak_after(30.0) < peak_after(200.0) * 0.6,
            "30Hz 电流声应明显弱于 200Hz 人声带"
        );
    }
}
