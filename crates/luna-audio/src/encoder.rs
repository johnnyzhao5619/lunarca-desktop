//! 录音编码器 trait + 实现。事实源：TDD-02 §3.5。
//!
//! `Encoder` trait 抽象编码格式，目前实现 `WavEncoder`（hound，MIT，增量写）。
//! `FlacEncoder` 留 trait 后续接入（Phase 8+ spike，见 Task 2 假设）。
//!
//! > 假设：`flacenc` 0.5.x 编码 API 为批量（整段源→内存 `Stream`），不适合长录音逐块追加落盘。
//! > `encode_fixed_size_frame` 可产单帧字节但需自行组装 FLAC stream 头并在停录后 seek-back
//! > 回写 `STREAMINFO.total_samples`，复杂度超出 Task 2 范围。本期回退 `hound` WAV 直写
//! > （MIT、增量写、自动修补头部、已在依赖树）；滚动切分守护对任意格式始终开启，数据安全不退；
//! > `FlacEncoder` 留 trait 后续接入。

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// 编码器错误。
#[derive(Debug, thiserror::Error)]
pub enum EncoderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wav encode error: {0}")]
    Wav(String),
}

impl From<hound::Error> for EncoderError {
    fn from(e: hound::Error) -> Self {
        Self::Wav(e.to_string())
    }
}

/// 录音格式（TDD-02 §3.5）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFormat {
    /// WAV 直写（无损、最大兼容；4GB 实际上限由滚动切分守护覆盖）。
    #[default]
    Wav,
    // Flac 留后续接入（Phase 8+ spike，本期不实现）。
}

impl RecordingFormat {
    /// 对应文件扩展名。
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
        }
    }
}

/// 录音编码器 trait（TDD-02 §3.5）。
///
/// 实现须：
/// - 支持逐块增量写（不要求整段 buffer）。
/// - `finalize` 后文件可独立解码读回。
/// - `bytes_written` 暴露已写字节数供滚动切分检测。
pub trait Encoder: Send {
    /// 写入一批单声道 16k f32 样本。
    ///
    /// # Errors
    /// IO 或编码失败时返回错误（调用方应升级诊断并停录）。
    fn write_samples(&mut self, samples: &[f32]) -> Result<(), EncoderError>;

    /// 收尾：修补文件头（WAV 长度字段等）并刷盘。消费后不再可用。
    ///
    /// # Errors
    /// 收尾失败时返回错误（已落盘部分不丢失）。
    fn finalize(self: Box<Self>) -> Result<(), EncoderError>;

    /// 已写入的字节数（供滚动切分监测）。
    fn bytes_written(&self) -> u64;
}

/// WAV 编码器（hound，MIT 许可，增量写）。
///
/// 使用 `BufWriter` 减少 syscall；`finalize` 时 hound 自动修补 RIFF chunk 大小。
pub struct WavEncoder {
    writer: hound::WavWriter<BufWriter<File>>,
    bytes_written: u64,
}

impl WavEncoder {
    /// 创建新 WAV 文件编码器。
    ///
    /// `path` 应为临时文件路径（`.tmp_recording.wav` 或 `partN.wav`），
    /// 由 `AudioFileWriter` 管理。
    ///
    /// # Errors
    /// 文件创建失败时返回错误。
    pub fn create(path: impl AsRef<Path>, sample_rate: u32) -> Result<Self, EncoderError> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)?;
        Ok(Self {
            writer,
            bytes_written: 0,
        })
    }

    /// 创建编码器并追加写入已存在的文件（滚动切分后新分片）。
    ///
    /// # Errors
    /// 文件创建失败时返回错误。
    pub fn create_new_part(path: impl AsRef<Path>, sample_rate: u32) -> Result<Self, EncoderError> {
        Self::create(path, sample_rate)
    }
}

impl Encoder for WavEncoder {
    fn write_samples(&mut self, samples: &[f32]) -> Result<(), EncoderError> {
        for &s in samples {
            // f32 [-1,1] → i16（clamp 防溢出）。
            let i = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.writer.write_sample(i)?;
            self.bytes_written += 2; // 16-bit = 2 bytes per sample
        }
        Ok(())
    }

    fn finalize(self: Box<Self>) -> Result<(), EncoderError> {
        self.writer.finalize()?;
        Ok(())
    }

    fn bytes_written(&self) -> u64 {
        // WAV header (~44 bytes) + data
        self.bytes_written + 44
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// WAV 编码可读回（write + finalize → hound 读回样本数一致）。
    #[test]
    fn wav_encoder_round_trip() {
        let dir = std::env::temp_dir();
        let path = dir.join("luna_test_wav_round_trip.wav");
        let samples: Vec<f32> = (0..1600).map(|i| (i as f32 / 1600.0) * 0.5).collect();

        let mut enc = WavEncoder::create(&path, 16_000).expect("create WAV");
        enc.write_samples(&samples).expect("write samples");
        let bytes_before = enc.bytes_written();
        assert!(bytes_before > 0);
        Box::new(enc).finalize().expect("finalize WAV");

        // 读回验证
        let mut reader = hound::WavReader::open(&path).expect("open WAV");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        let read_samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(read_samples.len(), 1600, "样本数应一致");

        // 清理
        let _ = std::fs::remove_file(&path);
    }

    /// `bytes_written` 随写入增加。
    #[test]
    fn bytes_written_increases() {
        let path = std::env::temp_dir().join("luna_test_bytes_written.wav");
        let mut enc = WavEncoder::create(&path, 16_000).expect("create");
        let b0 = enc.bytes_written();
        enc.write_samples(&[0.0; 160]).expect("write");
        assert!(enc.bytes_written() > b0);
        let _ = Box::new(enc).finalize();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recording_format_extension() {
        assert_eq!(RecordingFormat::Wav.extension(), "wav");
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn f32_to_i16_clamp_does_not_overflow() {
        let path = std::env::temp_dir().join("luna_test_clamp.wav");
        let mut enc = WavEncoder::create(&path, 16_000).unwrap();
        // 超出 [-1, 1] 范围的值应被 clamp 不 panic
        enc.write_samples(&[1.5, -1.5, 0.0]).unwrap();
        let _ = Box::new(enc).finalize();
        let _ = std::fs::remove_file(PathBuf::from(&path));
    }
}
