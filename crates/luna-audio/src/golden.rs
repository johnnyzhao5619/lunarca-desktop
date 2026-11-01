//! 黄金音频夹具回归（TDD-13 §2「固定音频样本 → 基线回归」的采集侧半边；**仅 `cfg(test)`**）。
//!
//! # 守什么
//!
//! VAD 门限、端点收尾、流式分段窗与 48k→16k 重采样此前各有单测，但没有一条"整段固定信号
//! 进、整串事件出"的**基线**：改一个时间常数、换一版重采样器，所有单测照样全绿，而现场
//! 的表现（起音晚了半帧、窗切在别处）没有任何东西会变红。本模块把三条固定信号的**整串
//! 输出**钉成文本快照：
//!
//! - `voice_input_16k`：语音输入路径，16k 单声道，两段语音夹底噪 → 起音 / utterance / 诊断；
//! - `realtime_48k_stereo`：实时转录路径，48k 立体声（现场最常见的设备格式）→ 流式窗序列
//!   （`abs_start` / `speech_len` / `speech_delta`），顺带钉住重采样后的电平指纹；
//! - `silence_16k`：只有底噪 → 不得产 utterance，且要出「疑似静音」诊断。
//!
//! 信号全部由 [`crate::testsig`] 解析式 / 伪随机合成——确定性、无真机、无版权文件。
//!
//! # 怎么更新
//!
//! 行为**有意**改变时：`LUNA_UPDATE_GOLDEN=1 cargo test -p luna-audio golden`，然后**逐行读
//! diff** 再提交——快照文件的 diff 就是这次改动对采集链路的全部影响清单。无意改变时它红，
//! 这正是它存在的理由。
//!
//! # 指纹口径
//!
//! 浮点逐样本比对在不同 CPU 的 FMA 行为下会抖，故样本只取 **RMS / 峰值（dBFS，0.1 dB）+
//! 长度**——足以抓住"重采样器换了增益 / 少了半帧"，又不会因最后一位尾数而红。

use crate::capture::CaptureTuning;
use crate::endpointer::FinalizeReason;
use crate::pipeline::{CaptureEvent, ConsumerPipeline, PipelineConfig, PipelineMode};
use crate::testsig::{noise, tone, Rng};
use crate::types::{CaptureDiagnostic, RawFrame, RawSamples, SpeechEvent};
use std::fmt::Write as _;
use std::path::PathBuf;

const TARGET: u32 = 16_000;

fn pipeline(mode: PipelineMode, input_sample_rate: u32) -> ConsumerPipeline {
    ConsumerPipeline::new(PipelineConfig::for_mode(
        mode,
        input_sample_rate,
        &CaptureTuning::default(),
    ))
    .expect("pipeline")
}

fn mono_frame(samples: Vec<f32>, sample_rate: u32) -> RawFrame {
    RawFrame {
        sample_rate,
        channels: 1,
        samples: RawSamples::F32(samples),
    }
}

/// 单声道 → 交错立体声（两路相同）：现场设备最常见的形态。
fn stereo_frame(mono: &[f32], sample_rate: u32) -> RawFrame {
    let mut inter = Vec::with_capacity(mono.len() * 2);
    for &s in mono {
        inter.push(s);
        inter.push(s);
    }
    RawFrame {
        sample_rate,
        channels: 2,
        samples: RawSamples::F32(inter),
    }
}

/// 电平指纹：`len / rms_dbfs / peak_dbfs`（0.1 dB 粒度）。
fn fingerprint(samples: &[f32]) -> String {
    if samples.is_empty() {
        return "len=0".to_owned();
    }
    let rms = (samples
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    let peak = samples.iter().fold(0f64, |m, s| m.max(f64::from(s.abs())));
    let db = |v: f64| {
        if v <= 0.0 {
            "-inf".to_owned()
        } else {
            format!("{:.1}", 20.0 * v.log10())
        }
    };
    format!(
        "len={} rms={}dBFS peak={}dBFS",
        samples.len(),
        db(rms),
        db(peak)
    )
}

fn diagnostic_name(d: &CaptureDiagnostic) -> String {
    let text = format!("{d:?}");
    text.split(|c: char| c == '{' || c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("?")
        .to_owned()
}

/// 事件流 → 逐行文本；波形电平只计数（逐帧字节是频率乘以时长的常量，钉数量即可）。
fn render(events: &[CaptureEvent]) -> String {
    let mut out = String::new();
    let mut levels = 0usize;
    for event in events {
        match event {
            CaptureEvent::SpeechStarted => out.push_str("speech_started\n"),
            CaptureEvent::Utterance { samples, reason } => {
                let reason = match reason {
                    FinalizeReason::Hangover => "hangover",
                    FinalizeReason::MaxDuration => "max_duration",
                    FinalizeReason::ManualStop => "manual_stop",
                };
                let _ = writeln!(out, "utterance reason={reason} {}", fingerprint(samples));
            }
            CaptureEvent::Window(SpeechEvent::Window {
                samples,
                abs_start,
                speech_len,
                speech_delta,
                ..
            }) => {
                let _ = writeln!(
                    out,
                    "window abs_start={abs_start} speech_len={speech_len} speech_delta={speech_delta} {}",
                    fingerprint(samples)
                );
            }
            CaptureEvent::Window(other) => {
                let _ = writeln!(out, "window_event {}", diagnostic_like(other));
            }
            CaptureEvent::Level(_) => levels += 1,
            CaptureEvent::Diagnostic(d) => {
                let _ = writeln!(out, "diagnostic {}", diagnostic_name(d));
            }
        }
    }
    let _ = writeln!(out, "levels={levels}");
    out
}

fn diagnostic_like<T: std::fmt::Debug>(value: &T) -> String {
    let text = format!("{value:?}");
    text.split(|c: char| c == '{' || c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("?")
        .to_owned()
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"))
}

/// 与快照比对；`LUNA_UPDATE_GOLDEN=1` 时改写快照并通过。
fn assert_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if std::env::var_os("LUNA_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("mkdir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "缺黄金快照 {}（{e}）；行为确认无误后用 LUNA_UPDATE_GOLDEN=1 生成",
            path.display()
        )
    });
    assert_eq!(
        actual,
        expected,
        "黄金快照 {name} 不一致——采集链路行为变了。有意为之则 LUNA_UPDATE_GOLDEN=1 更新并逐行审 diff"
    );
}

/// 100 ms 一帧喂：`secs` 秒底噪。
fn feed_noise(
    p: &mut ConsumerPipeline,
    rng: &mut Rng,
    sr: u32,
    secs: f32,
    stereo: bool,
    out: &mut Vec<CaptureEvent>,
) {
    let chunk = (sr / 10) as usize;
    let frames = (secs * 10.0).round() as usize;
    for _ in 0..frames {
        let mono = noise(rng, -60.0, chunk);
        let frame = if stereo {
            stereo_frame(&mono, sr)
        } else {
            mono_frame(mono, sr)
        };
        out.extend(p.process(&frame).expect("process"));
    }
}

/// 100 ms 一帧喂：`secs` 秒 `dbfs` 的正弦（相位跨帧连续）。
fn feed_tone(
    p: &mut ConsumerPipeline,
    sr: u32,
    secs: f32,
    dbfs: f32,
    stereo: bool,
    out: &mut Vec<CaptureEvent>,
) {
    let chunk = (sr / 10) as usize;
    let frames = (secs * 10.0).round() as usize;
    for k in 0..frames {
        let mono = tone(dbfs, chunk, k * chunk);
        let frame = if stereo {
            stereo_frame(&mono, sr)
        } else {
            mono_frame(mono, sr)
        };
        out.extend(p.process(&frame).expect("process"));
    }
}

#[test]
fn golden_voice_input_16k() {
    let mut rng = Rng::new(0x601D_0001);
    let mut p = pipeline(PipelineMode::VoiceInput, TARGET);
    let mut all = Vec::new();
    feed_noise(&mut p, &mut rng, TARGET, 0.5, false, &mut all);
    feed_tone(&mut p, TARGET, 1.0, -35.0, false, &mut all);
    feed_noise(&mut p, &mut rng, TARGET, 1.0, false, &mut all);
    feed_tone(&mut p, TARGET, 0.6, -30.0, false, &mut all);
    feed_noise(&mut p, &mut rng, TARGET, 1.2, false, &mut all);
    all.extend(p.finish().expect("finish"));
    assert_golden("voice_input_16k", &render(&all));
}

#[test]
fn golden_realtime_48k_stereo() {
    let mut rng = Rng::new(0x601D_0002);
    let mut p = pipeline(PipelineMode::RealtimeStream, 48_000);
    let mut all = Vec::new();
    feed_noise(&mut p, &mut rng, 48_000, 0.5, true, &mut all);
    feed_tone(&mut p, 48_000, 2.0, -35.0, true, &mut all);
    feed_noise(&mut p, &mut rng, 48_000, 1.0, true, &mut all);
    all.extend(p.finish().expect("finish"));
    assert_golden("realtime_48k_stereo", &render(&all));
}

#[test]
fn golden_silence_16k() {
    let mut rng = Rng::new(0x601D_0003);
    let mut p = pipeline(PipelineMode::VoiceInput, TARGET);
    let mut all = Vec::new();
    feed_noise(&mut p, &mut rng, TARGET, 4.0, false, &mut all);
    all.extend(p.finish().expect("finish"));
    let text = render(&all);
    assert!(
        !text.contains("utterance"),
        "纯底噪不得产 utterance：\n{text}"
    );
    assert_golden("silence_16k", &text);
}
