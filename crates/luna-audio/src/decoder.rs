//! 媒体文件解码 → 16k mono f32（导入音/视频批量转写，TDD-02 §3.7）。
//!
//! 采集路径（cpal）产实时帧；本模块补齐**离线文件**入口：给定磁盘上的音/视频文件，
//! 解出其音轨并复用 [`AudioNormalizer`]（降混 + 抗混叠重采样，§3.1-B/C）归一化到统一的
//! 16k mono f32，产出 [`luna_core::PcmBuffer`]——正是 whisper 批量路径的输入约定，故导入、
//! 停录后重转写、手动二次转写三处共享同一条解码 → 转写主干（DRY，ADR-001）。
//!
//! 解码器为纯 Rust 的 symphonia（MPL-2.0，AGPL 兼容），支持 mp3/aac(m4a)/alac/flac/
//! vorbis(ogg)/wav 音频编解码与 isomp4(mp4/mov) 容器音轨——视频文件仅取音轨，视频帧丢弃。

use std::path::Path;

use luna_core::CancelToken;
use luna_core::PcmBuffer;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::normalizer::{AudioNormalizer, NormalizeError};
use crate::types::{RawFrame, RawSamples, TARGET_SAMPLE_RATE};

/// 媒体解码错误。
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// 打开文件失败（路径不存在/无权限）。
    #[error("failed to open the media file: {0}")]
    Open(String),
    /// 容器/编解码不受支持或探测失败。
    #[error("unsupported media format, or probing failed: {0}")]
    Unsupported(String),
    /// 文件不含可解码的音轨（如纯视频无音频）。
    #[error("the file contains no audio track")]
    NoAudioTrack,
    /// 音轨缺采样率元数据（无法归一化）。
    #[error("the audio track has no sample-rate metadata")]
    MissingSampleRate,
    /// 解码过程失败。
    #[error("decode failed: {0}")]
    Decode(String),
    /// 用户取消（PRD-05-010）：解码是导入的首个阶段，GB 级视频可跑数分钟，
    /// 必须能在包边界中断——否则"取消"要等到解码跑完才生效。
    #[error("decode cancelled")]
    Cancelled,
    /// 归一化（降混/重采样）失败。
    #[error(transparent)]
    Normalize(#[from] NormalizeError),
    /// 解码结果为空（无音频样本）。
    #[error("decode produced no output")]
    Empty,
}

/// 只读容器头，答出这个文件的音轨有多长（秒）；**不解码**。
///
/// 任务前预估（PRD-13-040 ②）要在入队前知道「约消耗多少分钟」，而真解码一遍 GB 级视频要几分钟。
/// 容器头里没写帧数（部分 VBR mp3、流式封装）、探测失败、没有音轨时一律 `None`——调用方据此
/// 不给预估，而不是猜一个数（预估从来不是闸，服务端预留才是）。
#[must_use]
pub fn probe_media_duration_secs(path: &Path) -> Option<f64> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?;
    let params = &track.codec_params;
    let frames = params.n_frames?;
    if let Some(time_base) = params.time_base {
        let time = time_base.calc_time(frames);
        return Some(time.seconds as f64 + time.frac);
    }
    let rate = params.sample_rate.filter(|rate| *rate > 0)?;
    Some(frames as f64 / f64::from(rate))
}

/// 把磁盘上的音/视频文件解码并归一化为 **16k mono f32** [`PcmBuffer`]。
///
/// 流式逐包解码 + 逐帧归一化（内存占用 ≈ 单包，不整文件驻留原始 PCM）；仅取默认音轨，
/// 视频帧忽略。返回的缓冲直接可作 `AudioRef::Pcm` 喂入 whisper 批量转写。
///
/// # Errors
/// 见 [`DecodeError`] 各变体：打开失败 / 格式不支持 / 无音轨 / 缺采样率 / 解码失败 /
/// 归一化失败 / 结果为空。
pub fn decode_media_to_pcm(path: &Path, cancel: &CancelToken) -> Result<PcmBuffer, DecodeError> {
    let file = std::fs::File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..FormatOptions::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;

    let mut format = probed.format;

    // 选默认音轨（跳过无编解码的占位轨，如纯视频轨）。
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let src_rate = track
        .codec_params
        .sample_rate
        .ok_or(DecodeError::MissingSampleRate)?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;

    let mut normalizer = AudioNormalizer::new(src_rate)?;
    let mut samples: Vec<f32> = Vec::new();

    loop {
        // 包边界是解码的天然检查点（单包 ~20ms 音频），取消在此即时生效。
        if cancel.is_cancelled() {
            return Err(DecodeError::Cancelled);
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 正常读到流尾：symphonia 以 IoError(UnexpectedEof) 表达；ResetRequired = 轨道变更，
            // 单文件场景视作收尾。二者均结束循环。
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue; // 非目标音轨（多轨文件）。
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count() as u16;
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);
                let frame = RawFrame {
                    sample_rate: src_rate,
                    channels,
                    samples: RawSamples::F32(sample_buf.samples().to_vec()),
                };
                let mono16k = normalizer.process(&frame)?;
                samples.extend(mono16k.samples);
            }
            // 单包解码错误（局部损坏）可跳过继续；IO 错误结束。
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(_)) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }

    // 冲出重采样器尾部残余。
    samples.extend(normalizer.flush()?);

    if samples.is_empty() {
        return Err(DecodeError::Empty);
    }

    Ok(PcmBuffer {
        sample_rate: TARGET_SAMPLE_RATE,
        channels: 1,
        samples,
    })
}

/// 把**双轨立体声**媒体文件按声道分别解码为两路 16k mono（`(左, 右)`）。
///
/// 服务 Both 双轨录音的权威重转写按声道归属（TDD-02 §3.1-I 容器决策：声道即轨，
/// L=麦克风/我，R=系统/对方）；调用方须先经 `MediaRef.tracks == "mic_system"` 判定为
/// Lunarca 双轨产物——**普通立体声导入文件绝不走本函数**（双声道内容相同/无轨语义）。
///
/// # Errors
/// 除 [`decode_media_to_pcm`] 同款错误外，源非双声道时返回 [`DecodeError::Unsupported`]。
pub fn decode_media_to_stereo_tracks(
    path: &Path,
    cancel: &CancelToken,
) -> Result<(PcmBuffer, PcmBuffer), DecodeError> {
    let file = std::fs::File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..FormatOptions::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let src_rate = track
        .codec_params
        .sample_rate
        .ok_or(DecodeError::MissingSampleRate)?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::Unsupported(e.to_string()))?;

    // 每声道独立归一化器（各自 16k 重采样 + 去直流），互不串扰。
    let mut norm_l = AudioNormalizer::new(src_rate)?;
    let mut norm_r = AudioNormalizer::new(src_rate)?;
    let mut left: Vec<f32> = Vec::new();
    let mut right: Vec<f32> = Vec::new();

    loop {
        // 包边界即取消检查点（与单轨解码同口径）。
        if cancel.is_cancelled() {
            return Err(DecodeError::Cancelled);
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count();
                if channels != 2 {
                    return Err(DecodeError::Unsupported(format!(
                        "双轨解码期望立体声，实际 {channels} 声道"
                    )));
                }
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);
                let interleaved = sample_buf.samples();
                let mut l = Vec::with_capacity(interleaved.len() / 2);
                let mut r = Vec::with_capacity(interleaved.len() / 2);
                for pair in interleaved.chunks_exact(2) {
                    l.push(pair[0]);
                    r.push(pair[1]);
                }
                for (norm, buf, out) in [(&mut norm_l, l, &mut left), (&mut norm_r, r, &mut right)]
                {
                    let frame = RawFrame {
                        sample_rate: src_rate,
                        channels: 1,
                        samples: RawSamples::F32(buf),
                    };
                    out.extend(norm.process(&frame)?.samples);
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(_)) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        }
    }
    left.extend(norm_l.flush()?);
    right.extend(norm_r.flush()?);
    if left.is_empty() && right.is_empty() {
        return Err(DecodeError::Empty);
    }
    let pcm = |samples: Vec<f32>| PcmBuffer {
        sample_rate: TARGET_SAMPLE_RATE,
        channels: 1,
        samples,
    };
    Ok((pcm(left), pcm(right)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::f32::consts::PI;

    /// 写一个正弦 WAV，返回临时文件路径（自持 tempdir 生命周期）。
    fn write_sine_wav(
        dir: &Path,
        name: &str,
        sample_rate: u32,
        secs: f32,
        freq: f32,
    ) -> std::path::PathBuf {
        let path = dir.join(name);
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        let n = (sample_rate as f32 * secs) as u32;
        for i in 0..n {
            let t = i as f32 / sample_rate as f32;
            let v = (2.0 * PI * freq * t).sin();
            w.write_sample((v * 16_384.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// 时长只读容器头：WAV 写了帧数，答得出；不是媒体的文件答不出（不猜）。
    #[test]
    fn probes_duration_from_the_header_without_decoding() {
        let dir = std::env::temp_dir().join(format!("luna-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_sine_wav(&dir, "two-and-a-half.wav", 16_000, 2.5, 440.0);
        let secs = probe_media_duration_secs(&path).expect("WAV 头里有帧数");
        assert!((secs - 2.5).abs() < 0.01, "{secs}");
        let text = dir.join("not-media.wav");
        std::fs::write(&text, b"hello").unwrap();
        assert_eq!(probe_media_duration_secs(&text), None);
        assert_eq!(probe_media_duration_secs(&dir.join("missing.wav")), None);
    }

    #[test]
    fn decodes_wav_and_resamples_to_16k_mono() {
        let dir = std::env::temp_dir().join(format!("luna-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 48k 源 → 期望重采样到 16k（长度约 1/3）。
        let path = write_sine_wav(&dir, "s48k.wav", 48_000, 1.0, 440.0);
        let pcm = decode_media_to_pcm(&path, &CancelToken::new()).unwrap();
        assert_eq!(pcm.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(pcm.channels, 1);
        // 1s @16k ≈ 16000 样本（重采样边界有少量出入）。
        let diff = (pcm.samples.len() as i64 - 16_000).abs();
        assert!(diff < 2_000, "长度应≈16000（got {}）", pcm.samples.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn passthrough_16k_wav_preserves_length() {
        let dir = std::env::temp_dir().join(format!("luna-decode-pt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_sine_wav(&dir, "s16k.wav", 16_000, 0.5, 220.0);
        let pcm = decode_media_to_pcm(&path, &CancelToken::new()).unwrap();
        assert_eq!(pcm.sample_rate, 16_000);
        let diff = (pcm.samples.len() as i64 - 8_000).abs();
        assert!(diff < 500, "0.5s@16k≈8000（got {}）", pcm.samples.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stereo_tracks_decode_separates_channels() {
        let dir = std::env::temp_dir().join("luna-decoder-stereo-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dual.wav");
        // L = 440Hz 正弦，R = 静音 → 分声道解码后 L 有能量、R 近零。
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        for i in 0..16_000 {
            let s = (2.0 * PI * 440.0 * (i as f32) / 16_000.0).sin();
            w.write_sample((s * 20_000.0) as i16).unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();

        let (l, r) = decode_media_to_stereo_tracks(&path, &CancelToken::new()).unwrap();
        assert_eq!(l.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(r.sample_rate, TARGET_SAMPLE_RATE);
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len().max(1) as f32).sqrt();
        assert!(rms(&l.samples) > 0.1, "左声道应有信号能量");
        assert!(rms(&r.samples) < 0.01, "右声道应近乎静音");
    }

    #[test]
    fn stereo_tracks_decode_rejects_mono() {
        let dir = std::env::temp_dir().join("luna-decoder-stereo-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_sine_wav(&dir, "mono-for-stereo.wav", 16_000, 0.2, 440.0);
        assert!(matches!(
            decode_media_to_stereo_tracks(&path, &CancelToken::new()),
            Err(DecodeError::Unsupported(_))
        ));
    }

    #[test]
    fn missing_file_errors_cleanly() {
        let err = decode_media_to_pcm(Path::new("/nonexistent/path/x.mp3"), &CancelToken::new())
            .unwrap_err();
        assert!(matches!(err, DecodeError::Open(_)));
    }
}
