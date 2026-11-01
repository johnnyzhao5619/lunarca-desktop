//! Silero VAD（ADR-012 目标态，2026-09-06 接入）——经已随包的 `sherpa-onnx` 运行时跑，
//! **不引 `ort`、不另发 onnxruntime 二进制**：说话人细分早已把 sherpa-onnx（内含 onnxruntime）
//! 带进了依赖树与许可清单，Silero 只多一个 ~630 KB 的 ONNX 权重，走与细分模型同一条受管
//! 下载路径（模型目录 `luna-model-catalog` 的 `silero-vad` 条目，MIT）。
//!
//! 两条硬性实现约束（TDD-02 §3.2）：
//! 1. **定长 512 采样 @16k**：管线送来的是 20 ms（320 采样）帧，这里用一个缓冲重切成 512；
//!    标签取「到本帧为止最近一次判定」，多出的十几毫秒延迟由端点器的 hangover 吸收。
//! 2. **RNN 有状态，逐会话重置**：`reset()` 调运行时的 `reset`，不保留上一会话的隐藏状态。
//!
//! 能量 VAD（ADR-077）退居兜底：设置选了 Silero 但权重不在场 / 加载失败时，会话按能量 VAD
//! 起，并在日志里说清（不静默降级成"怎么调都不对"的形状）。

use luna_audio::{dbfs, Vad, VadDecision, VadFactory, VadLabel, VadSensitivity};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 权重在模型存储根下的约定路径（与目录条目 `silero-vad` 的 `weight_rel` 同一份，测试守着）。
pub const SILERO_WEIGHT_REL: &str = "vad/silero_vad.onnx";
/// Silero v5 的定长输入帧（采样数 @16k）。
const WINDOW_SAMPLES: usize = 512;
const SAMPLE_RATE: i32 = 16_000;

#[must_use]
pub fn silero_model_path(models_root: &Path) -> PathBuf {
    models_root.join(SILERO_WEIGHT_REL)
}

/// 灵敏度 → Silero 语音概率阈值：与能量 VAD 的三档同一个旋钮（ADR-077 ⑤），只是量纲不同
/// （概率而非 dB 余量）。低灵敏 = 更高的门槛。
#[must_use]
pub const fn threshold_for(sensitivity: VadSensitivity) -> f32 {
    match sensitivity {
        VadSensitivity::Low => 0.7,
        VadSensitivity::Normal => 0.5,
        VadSensitivity::High => 0.35,
    }
}

/// 构造工厂：每个采集会话（每条轨）各得一个独立实例（ADR-077 ④ 同精神——状态不跨轨）。
/// 权重不在场返回 `None`，调用方按能量 VAD 走。
#[must_use]
pub fn vad_factory(
    models_root: &Path,
    sensitivity: VadSensitivity,
    num_threads: i32,
) -> Option<VadFactory> {
    let path = silero_model_path(models_root);
    if !path.is_file() {
        return None;
    }
    let path_text = path.to_string_lossy().into_owned();
    Some(VadFactory(Arc::new(move || {
        match SileroVad::open(&path_text, sensitivity, num_threads) {
            Some(vad) => Box::new(vad) as Box<dyn Vad>,
            None => {
                eprintln!("[vad] Silero 加载失败，本会话回落能量 VAD（ADR-077）");
                Box::new(luna_audio::EnergyVad::new(sensitivity))
            }
        }
    })))
}

/// 一条轨上的 Silero 实例。
pub struct SileroVad {
    detector: VoiceActivityDetector,
    buf: Vec<f32>,
    last: VadDecision,
}

impl SileroVad {
    fn open(model_path: &str, sensitivity: VadSensitivity, num_threads: i32) -> Option<Self> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model_path.to_owned()),
                threshold: threshold_for(sensitivity),
                // 尾静音与最短语音都交给本仓的端点器 / 分段器裁决（hangover、最短时长在那儿），
                // 这里只要一个尽量贴近逐帧的判定，故两者都取很小的值。
                min_silence_duration: 0.1,
                min_speech_duration: 0.05,
                window_size: WINDOW_SAMPLES as i32,
                max_speech_duration: 30.0,
            },
            sample_rate: SAMPLE_RATE,
            num_threads,
            provider: Some("cpu".to_owned()),
            ..VadModelConfig::default()
        };
        let detector = VoiceActivityDetector::create(&config, 30.0)?;
        Some(Self {
            detector,
            buf: Vec::with_capacity(WINDOW_SAMPLES * 2),
            last: VadDecision::Neural {
                speech: false,
                rms_dbfs: luna_audio::DBFS_FLOOR,
            },
        })
    }
}

impl Vad for SileroVad {
    fn score(&mut self, frame: &[f32]) -> VadLabel {
        let rms = if frame.is_empty() {
            0.0
        } else {
            (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
        };
        self.buf.extend_from_slice(frame);
        while self.buf.len() >= WINDOW_SAMPLES {
            let window: Vec<f32> = self.buf.drain(..WINDOW_SAMPLES).collect();
            self.detector.accept_waveform(&window);
        }
        // 只要逐帧判定，不消费分段：把内部队列清掉，免得整场会话都攒在那儿。
        while !self.detector.is_empty() {
            self.detector.pop();
        }
        let speech = self.detector.detected();
        self.last = VadDecision::Neural {
            speech,
            rms_dbfs: dbfs(rms),
        };
        if speech {
            VadLabel::Speech
        } else {
            VadLabel::Silence
        }
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.buf.clear();
        self.last = VadDecision::Neural {
            speech: false,
            rms_dbfs: luna_audio::DBFS_FLOOR,
        };
    }

    fn last_decision(&self) -> VadDecision {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_follows_the_shared_sensitivity_knob() {
        assert!(threshold_for(VadSensitivity::Low) > threshold_for(VadSensitivity::Normal));
        assert!(threshold_for(VadSensitivity::Normal) > threshold_for(VadSensitivity::High));
    }

    #[test]
    fn factory_is_none_without_the_weight() {
        let dir = tempfile::tempdir().unwrap();
        assert!(vad_factory(dir.path(), VadSensitivity::Normal, 1).is_none());
        assert!(!silero_model_path(dir.path()).is_file());
    }

    /// 真机项：`LUNA_SILERO_MODEL_DIR=<含 vad/silero_vad.onnx 的根> cargo test -- --ignored silero`；
    /// 未设时取本机模型根，权重不在场即跳过（不伪装通过）。
    #[test]
    #[ignore = "需要 silero_vad.onnx 权重"]
    fn real_model_stays_silent_on_silence_and_resets_between_sessions() {
        let root = std::env::var("LUNA_SILERO_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                    .join("Library/Application Support")
                    .join(luna_config::identity::APP_IDENTIFIER)
                    .join("models")
            });
        let Some(factory) = vad_factory(&root, VadSensitivity::Normal, 2) else {
            eprintln!("权重不在场，跳过");
            return;
        };
        let mut vad = (factory.0)();
        let silence = vec![0.0f32; 320];
        for _ in 0..100 {
            assert_eq!(vad.score(&silence), VadLabel::Silence);
        }
        // 合成白噪不是真实底噪，Silero 对它并非零响应：实测（2026-09-06，k2-fsa 转换版 v5）
        // −34 dBFS 白噪 100 帧里判成语音 16 帧、−20 dBFS 判 36 帧——每次触发都带 0.1 s 的最短
        // 静音尾巴，故按"次"算只有三四次。这里只钉一个不至于把底噪当成连续说话的上界；
        // 真实风扇 / 键盘声的表现归真机验收，不在合成信号上下结论。
        let mut seed = 0x1234_5678_u32;
        let noise: Vec<f32> = (0..320)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.04
            })
            .collect();
        let mut speech_frames = 0;
        for _ in 0..100 {
            if vad.score(&noise) == VadLabel::Speech {
                speech_frames += 1;
            }
        }
        assert!(
            speech_frames < 30,
            "底噪量级白噪被判成连续说话 {speech_frames}/100 帧"
        );
        vad.reset();
        assert_eq!(
            vad.last_decision(),
            VadDecision::Neural {
                speech: false,
                rms_dbfs: luna_audio::DBFS_FLOOR
            }
        );
    }
}
