//! 零阻塞录音写线程（AudioFileWriter）。事实源：TDD-02 §5.3。
//!
//! **绝对禁止在音频消费线程内执行同步磁盘写 IO**（ADR-009）。消费线程仅通过有界 channel 投递
//! PCM 数据给专用的 `AudioFileWriter` OS 线程；写线程追加落盘、周期查询磁盘空间、逼近容器上限
//! 时自动滚动切分，并通过诊断通道反馈 `DiskLow`/`DiskCritical`/`RolledToPart`。
//!
//! **滚动切分守护始终开启**（任意容器格式）：WAV RIFF 32-bit 字段最大 ~4GB；逼近上限前自动
//! 切到 `name.partN.ext`，各分片记录 `[abs_start, abs_end]` 绝对时间区间，对用户呈现一条录音。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::encoder::{Encoder, EncoderError, RecordingFormat, WavEncoder};
use crate::types::CaptureDiagnostic;

/// 默认磁盘剩余空间查询间隔（避免在音频热路径频繁 syscall）。可经 [`WriterConfig`] 注入覆盖，
/// 使「磁盘将满优雅停录 / 滚动切分」的数据安全路径可被确定性测试（TDD-02 §5.3、§8）。
pub const DEFAULT_DISK_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// 默认滚动切分阈值（逼近 WAV 容器上限前触发，留余量）。
pub const DEFAULT_ROLLING_SPLIT_MAX_BYTES: u64 = 3_800_000_000;

/// 默认磁盘软阈值（~1GB 剩余时预警）。
pub const DEFAULT_DISK_SOFT_THRESHOLD_BYTES: u64 = 1_000_000_000;

/// 默认磁盘硬阈值（~100MB 剩余时强制停录）。
pub const DEFAULT_DISK_HARD_THRESHOLD_BYTES: u64 = 100_000_000;

/// 内部写入通道消息。
enum WriterMsg {
    /// 一批单声道 16k f32 样本。
    Samples(Vec<f32>),
    /// 通知写线程优雅停止。
    Stop,
}

/// 分片区间表条目（TDD-02 §5.3 分片时间轴索引）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartInterval {
    /// 分片编号（从 0 起，part0 = 原始文件或第一个分片）。
    pub part: u32,
    /// 分片在会话中的绝对起点（采样数，÷ sample_rate = 绝对秒）。
    pub abs_start: u64,
    /// 分片在会话中的绝对终点（exclusive，采样数）。
    pub abs_end: u64,
    /// 对应落盘文件路径。
    pub path: PathBuf,
}

/// `AudioFileWriter` 配置（TDD-02 §5.3）。
#[derive(Clone, Debug)]
pub struct WriterConfig {
    /// 基础录音路径（含扩展名，如 `<vault>/Recordings/session.wav`）；
    /// 若发生滚动切分，分片文件名为 `session.part1.wav`、`session.part2.wav` 等。
    pub base_path: PathBuf,
    /// 录音格式。
    pub format: RecordingFormat,
    /// 工作采样率（通常 16k）。
    pub sample_rate: u32,
    /// 磁盘软阈值：低于此发 `DiskLow` 预警（字节）。
    pub disk_soft_threshold_bytes: u64,
    /// 磁盘硬阈值：低于此优雅停录（字节）。
    pub disk_hard_threshold_bytes: u64,
    /// 当前文件逼近此大小时触发滚动切分（字节）。
    pub rolling_split_max_bytes: u64,
    /// 磁盘剩余空间查询间隔（低频，不在采样热路径 per-sample 触发）。
    /// 测试可注入极小值以确定性触发优雅停录/滚动切分路径（TDD-02 §8）。
    pub disk_check_interval: Duration,
}

impl WriterConfig {
    /// 以默认阈值构造配置。
    #[must_use]
    pub fn new(base_path: PathBuf, format: RecordingFormat, sample_rate: u32) -> Self {
        Self {
            base_path,
            format,
            sample_rate,
            disk_soft_threshold_bytes: DEFAULT_DISK_SOFT_THRESHOLD_BYTES,
            disk_hard_threshold_bytes: DEFAULT_DISK_HARD_THRESHOLD_BYTES,
            rolling_split_max_bytes: DEFAULT_ROLLING_SPLIT_MAX_BYTES,
            disk_check_interval: DEFAULT_DISK_CHECK_INTERVAL,
        }
    }
}

/// 写线程句柄；stop/drop 发停止信号并等待线程退出，返回分片区间表。
pub struct AudioFileWriter {
    sender: SyncSender<WriterMsg>,
    thread: Option<JoinHandle<Vec<PartInterval>>>,
}

impl AudioFileWriter {
    /// 启动写线程，返回 `AudioFileWriter` 句柄。
    ///
    /// `diag_sink` 接收 `DiskLow`/`DiskCritical`/`RolledToPart` 诊断事件。
    /// `force_stop_tx`（可选）：磁盘硬阈值触发时写线程发出强制停录信号，
    /// 调用方检测到后应停止采集（TDD-02 §5.3 优雅停录）。
    ///
    /// 内部通道有界容量由调用方指定（建议 128，背压测试可缩小）。
    ///
    /// # Errors
    /// 初始化编码器（创建文件）失败时立即返回错误。
    pub fn spawn(
        cfg: WriterConfig,
        channel_capacity: usize,
        diag_sink: std::sync::mpsc::Sender<CaptureDiagnostic>,
        force_stop_tx: Option<std::sync::mpsc::Sender<()>>,
    ) -> Result<Self, EncoderError> {
        let (sender, receiver) = mpsc::sync_channel::<WriterMsg>(channel_capacity);

        // 预先创建第一个编码器（若失败立即返回错误，不启动线程）。
        let first_path = cfg.base_path.clone();
        let first_enc = make_encoder(cfg.format, &first_path, cfg.sample_rate)?;

        let thread = thread::Builder::new()
            .name("luna-audio-writer".into())
            .spawn(move || {
                run_writer(
                    cfg,
                    first_path,
                    first_enc,
                    receiver,
                    diag_sink,
                    force_stop_tx,
                )
            })
            .expect("spawn writer thread");

        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    /// 发送 PCM 样本到写线程（非阻塞 try_send；若通道满则背压——不丢样本，返回 false）。
    pub fn send_samples(&self, samples: Vec<f32>) -> bool {
        self.sender.try_send(WriterMsg::Samples(samples)).is_ok()
    }

    /// 发送阻塞性样本（用于测试或非热路径调用，保证送达）。
    pub fn send_samples_blocking(&self, samples: Vec<f32>) -> bool {
        self.sender.send(WriterMsg::Samples(samples)).is_ok()
    }

    /// 停止写线程并返回分片区间表（阻塞等待写线程 flush 完成）。
    #[must_use]
    pub fn stop(mut self) -> Vec<PartInterval> {
        let _ = self.sender.send(WriterMsg::Stop);
        if let Some(h) = self.thread.take() {
            h.join().unwrap_or_default()
        } else {
            Vec::new()
        }
    }
}

impl Drop for AudioFileWriter {
    fn drop(&mut self) {
        // 兜底停止（未主动 stop 时的 drop 路径）。
        let _ = self.sender.try_send(WriterMsg::Stop);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

// ─── 写线程内部实现 ────────────────────────────────────────────────────────────

/// 构造编码器（按 format 选择实现）。
///
/// **落盘目录健壮性收敛点**：writer 是所有音频文件（base + 滚动 partN）创建的唯一入口，
/// 故在此确保父目录存在（`Recordings/` 等可能尚未建）。`create_dir_all` 幂等无害，
/// 让「给定路径即可写」的契约对所有调用方与所有分片都成立（消除 ENOENT 落盘失败）。
fn make_encoder(
    format: RecordingFormat,
    path: &Path,
    sample_rate: u32,
) -> Result<Box<dyn Encoder>, EncoderError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    match format {
        RecordingFormat::Wav => Ok(Box::new(WavEncoder::create(path, sample_rate)?)),
    }
}

/// 写线程主循环（TDD-02 §5.3）。
#[allow(clippy::too_many_arguments)]
fn run_writer(
    cfg: WriterConfig,
    first_path: PathBuf,
    first_enc: Box<dyn Encoder>,
    receiver: Receiver<WriterMsg>,
    diag_sink: std::sync::mpsc::Sender<CaptureDiagnostic>,
    force_stop_tx: Option<std::sync::mpsc::Sender<()>>,
) -> Vec<PartInterval> {
    let mut enc = first_enc;
    let mut current_part: u32 = 0;
    let mut current_part_path = first_path;
    let mut current_part_start: u64 = 0; // 绝对采样起点
    let mut abs_written: u64 = 0; // 会话总已写采样数
    let mut intervals: Vec<PartInterval> = Vec::new();
    let mut last_disk_check = Instant::now();
    let mut disk_soft_warned = false;
    let mut forced_stop = false;

    while let Ok(msg) = receiver.recv() {
        match msg {
            WriterMsg::Stop => break,
            WriterMsg::Samples(samples) => {
                if forced_stop {
                    continue; // 已强制停录，丢弃后续样本
                }

                // 写入样本（IO 错误记诊断但不 panic）。
                if let Err(e) = enc.write_samples(&samples) {
                    let _ = diag_sink.send(CaptureDiagnostic::DiskCritical { free_bytes: 0 });
                    tracing_error(&format!("write_samples error: {e}"));
                    break;
                }
                abs_written += samples.len() as u64;

                // 周期磁盘空间检查（低频，不在采样热路径 per-sample 触发）。
                if last_disk_check.elapsed() >= cfg.disk_check_interval {
                    last_disk_check = Instant::now();
                    if let Some(free) = query_free_space(&current_part_path) {
                        if free < cfg.disk_hard_threshold_bytes {
                            // 硬阈值：优雅停录——先落盘已有，再发通知。
                            finalize_current(
                                &mut enc,
                                &current_part_path,
                                current_part,
                                current_part_start,
                                abs_written,
                                &mut intervals,
                            );
                            let _ = diag_sink
                                .send(CaptureDiagnostic::DiskCritical { free_bytes: free });
                            if let Some(ref tx) = force_stop_tx {
                                let _ = tx.send(());
                            }
                            forced_stop = true;
                            break;
                        } else if free < cfg.disk_soft_threshold_bytes && !disk_soft_warned {
                            disk_soft_warned = true;
                            let _ = diag_sink.send(CaptureDiagnostic::DiskLow { free_bytes: free });
                        }
                    }
                }

                // 滚动切分检查：逼近容器上限前切换到下一个分片。
                if enc.bytes_written() >= cfg.rolling_split_max_bytes {
                    let new_part = current_part + 1;
                    let new_path = make_part_path(&cfg.base_path, new_part, cfg.format);

                    // 收尾当前分片并记录区间（replace 保持 enc 始终有效）。
                    let old_abs_start = current_part_start;
                    let old_part = current_part;
                    let old_path = current_part_path.clone();
                    let old_enc = std::mem::replace(&mut enc, Box::new(NoopEncoder));
                    finalize_and_push(
                        old_enc,
                        old_part,
                        old_abs_start,
                        abs_written,
                        old_path,
                        &mut intervals,
                    );

                    // 创建新分片编码器。
                    match make_encoder(cfg.format, &new_path, cfg.sample_rate) {
                        Ok(new_enc) => {
                            enc = new_enc;
                            current_part = new_part;
                            current_part_path = new_path;
                            current_part_start = abs_written;
                            last_disk_check = Instant::now(); // 切分后立即查盘
                            let _ =
                                diag_sink.send(CaptureDiagnostic::RolledToPart { part: new_part });
                        }
                        Err(e) => {
                            tracing_error(&format!("roll-to-part encoder error: {e}"));
                            // 无法创建新分片 → 强制停录（已录数据已在 intervals 中）。
                            let _ =
                                diag_sink.send(CaptureDiagnostic::DiskCritical { free_bytes: 0 });
                            if let Some(ref tx) = force_stop_tx {
                                let _ = tx.send(());
                            }
                            forced_stop = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    // 收尾最后一个分片（若未被 forced_stop 提前处理）。
    if !forced_stop {
        finalize_current(
            &mut enc,
            &current_part_path,
            current_part,
            current_part_start,
            abs_written,
            &mut intervals,
        );
    }

    intervals
}

/// 收尾当前分片并推入区间表。
fn finalize_current(
    enc: &mut Box<dyn Encoder>,
    path: &Path,
    part: u32,
    abs_start: u64,
    abs_end: u64,
    intervals: &mut Vec<PartInterval>,
) {
    // 用 Default 替换 enc，然后 finalize 旧的。
    let old = std::mem::replace(enc, Box::new(NoopEncoder));
    finalize_and_push(old, part, abs_start, abs_end, path.to_path_buf(), intervals);
}

/// finalize 编码器并将区间推入 intervals。
fn finalize_and_push(
    enc: Box<dyn Encoder>,
    part: u32,
    abs_start: u64,
    abs_end: u64,
    path: PathBuf,
    intervals: &mut Vec<PartInterval>,
) {
    if let Err(e) = enc.finalize() {
        tracing_error(&format!("finalize encoder error: {e}"));
    }
    intervals.push(PartInterval {
        part,
        abs_start,
        abs_end,
        path,
    });
}

/// 构造分片文件路径：`base.part1.wav`、`base.part2.wav` 等。
fn make_part_path(base: &Path, part: u32, format: RecordingFormat) -> PathBuf {
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let dir = base.parent().unwrap_or(Path::new("."));
    dir.join(format!("{stem}.part{part}.{}", format.extension()))
}

/// 查询 `path` 所在卷的可用磁盘空间（字节）；查询失败时返回 `None`。
fn query_free_space(path: &Path) -> Option<u64> {
    use fs4::available_space;
    let dir = path.parent().unwrap_or(Path::new("."));
    available_space(dir).ok()
}

/// 无操作编码器（用于 `std::mem::replace` 占位）。
struct NoopEncoder;

impl Encoder for NoopEncoder {
    fn write_samples(&mut self, _: &[f32]) -> Result<(), EncoderError> {
        Ok(())
    }
    fn finalize(self: Box<Self>) -> Result<(), EncoderError> {
        Ok(())
    }
    fn bytes_written(&self) -> u64 {
        0
    }
}

/// 写线程 IO/编码错误日志（脱敏：仅含 IO/路径级错误，**绝不含音频样本内容**）。
/// 与本仓既有 `eprintln!` 诊断风格一致；不静默吞错（m4 收口）。
fn tracing_error(msg: &str) {
    eprintln!("[luna-audio-writer] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CaptureDiagnostic;
    use std::sync::mpsc;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        // 可能有 partN 文件
        for i in 1..=10u32 {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let ext = path.extension().unwrap_or_default().to_str().unwrap();
            let _ =
                std::fs::remove_file(path.parent().unwrap().join(format!("{stem}.part{i}.{ext}")));
        }
    }

    fn make_cfg_with_thresholds(
        path: PathBuf,
        disk_soft: u64,
        disk_hard: u64,
        rolling: u64,
    ) -> WriterConfig {
        WriterConfig {
            base_path: path,
            format: RecordingFormat::Wav,
            sample_rate: 16_000,
            disk_soft_threshold_bytes: disk_soft,
            disk_hard_threshold_bytes: disk_hard,
            rolling_split_max_bytes: rolling,
            // 间隔 0：每次收到样本即查盘 → 磁盘数据安全路径可确定性测试（m2 收口）。
            disk_check_interval: Duration::ZERO,
        }
    }

    /// 正常写入 + stop → WAV 可读回。
    #[test]
    fn write_and_stop_produces_readable_wav() {
        let path = tmp_path("luna_writer_basic.wav");
        cleanup(&path);

        let cfg = WriterConfig::new(path.clone(), RecordingFormat::Wav, 16_000);
        let (diag_tx, _diag_rx) = mpsc::channel();
        let writer = AudioFileWriter::spawn(cfg, 64, diag_tx, None).expect("spawn");

        let samples: Vec<f32> = (0..1600).map(|i| i as f32 / 1600.0 * 0.5).collect();
        writer.send_samples_blocking(samples);
        let intervals = writer.stop();

        assert_eq!(intervals.len(), 1, "应有一个分片");
        assert_eq!(intervals[0].part, 0);
        assert_eq!(intervals[0].abs_start, 0);
        assert_eq!(intervals[0].abs_end, 1600);

        let reader = hound::WavReader::open(&path).expect("读回 WAV");
        assert_eq!(reader.len(), 1600);

        cleanup(&path);
    }

    /// 落盘目录不存在时 writer 自动建目录并成功落盘（Fix A：消除 ENOENT 落盘失败）。
    #[test]
    fn writer_creates_missing_parent_directory() {
        // 在 temp 下构造一个**尚不存在**的多级子目录路径（模拟 vault/Recordings 未预建）。
        let dir = std::env::temp_dir().join("luna_writer_mkdir_test/Recordings");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let path = dir.join("session-p00.wav");
        assert!(!dir.exists(), "前置：父目录应不存在");

        let cfg = WriterConfig::new(path.clone(), RecordingFormat::Wav, 16_000);
        let (diag_tx, _diag_rx) = mpsc::channel();
        // spawn 内部 make_encoder 应 create_dir_all 父目录 → 不返回 ENOENT。
        let writer = AudioFileWriter::spawn(cfg, 64, diag_tx, None)
            .expect("父目录缺失时应自动创建，不应初始化失败");
        writer.send_samples_blocking(vec![0.1f32; 160]);
        let intervals = writer.stop();

        assert_eq!(intervals.len(), 1, "应正常落盘一个分片");
        assert!(path.exists(), "WAV 文件应已写入新建目录");
        let reader = hound::WavReader::open(&path).expect("读回 WAV");
        assert_eq!(reader.len(), 160);

        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// 小滚动上限 → 多分片 + 区间连续。
    #[test]
    fn rolling_split_produces_consecutive_intervals() {
        let path = tmp_path("luna_writer_rolling.wav");
        cleanup(&path);

        // 每个 WAV 分片 ~320 样本时触发（320 samples * 2 bytes = 640 bytes + 44 header ≈ 684）
        // 我们设 rolling_split_max_bytes = 500 来强制在 ~228 样本后触发。
        // 磁盘阈值置 0 = 关闭磁盘检查（free < 0 恒 false），只考察滚动切分（与零间隔不冲突）。
        let cfg = make_cfg_with_thresholds(path.clone(), 0, 0, 500);
        let (diag_tx, diag_rx) = mpsc::channel();
        let writer = AudioFileWriter::spawn(cfg, 256, diag_tx, None).expect("spawn");

        // 发送 1600 样本（足够触发多次切分）
        let samples: Vec<f32> = vec![0.1f32; 160];
        for _ in 0..10 {
            writer.send_samples_blocking(samples.clone());
        }
        let intervals = writer.stop();

        assert!(
            intervals.len() >= 2,
            "应有至少 2 个分片，实际 {}",
            intervals.len()
        );

        // 验证区间连续（每个分片的 abs_end == 下一个的 abs_start）。
        for i in 0..intervals.len() - 1 {
            assert_eq!(
                intervals[i].abs_end,
                intervals[i + 1].abs_start,
                "分片 {} 和 {} 区间不连续",
                i,
                i + 1
            );
        }

        // 验证总区间 = 所有发送的样本数。
        let total = intervals.last().map_or(0, |iv| iv.abs_end);
        assert_eq!(total, 1600, "总区间应等于发送样本数");

        // 应收到 RolledToPart 诊断。
        let rolled_count = {
            let mut count = 0;
            while let Ok(d) = diag_rx.try_recv() {
                if matches!(d, CaptureDiagnostic::RolledToPart { .. }) {
                    count += 1;
                }
            }
            count
        };
        assert!(rolled_count >= 1, "应收到至少一次 RolledToPart 诊断");

        cleanup(&path);
    }

    /// 注入硬阈值 = u64::MAX-1（真实磁盘剩余必 < 之）+ 间隔 0 → **确定性**触发优雅停录：
    /// 发 DiskCritical 诊断 + force_stop 信号 + 收尾已落盘分片（数据不丢，TDD-02 §5.3）。
    #[test]
    fn disk_critical_triggers_graceful_stop_deterministically() {
        let path = tmp_path("luna_writer_disk_critical.wav");
        cleanup(&path);

        // 硬阈值接近 u64::MAX → 真实磁盘剩余必然 < 硬阈值；间隔 0 → 首批样本即触发。
        let cfg = make_cfg_with_thresholds(path.clone(), u64::MAX, u64::MAX - 1, u64::MAX);
        let (diag_tx, diag_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let writer = AudioFileWriter::spawn(cfg, 256, diag_tx, Some(stop_tx)).expect("spawn");

        writer.send_samples_blocking(vec![0.0f32; 160]);
        let intervals = writer.stop();

        // 优雅停录：已落盘分片被收尾（数据不丢）。
        assert_eq!(intervals.len(), 1, "停录前应收尾已落盘分片");
        // 收到 DiskCritical 诊断。
        let got_critical = std::iter::from_fn(|| diag_rx.try_recv().ok())
            .any(|d| matches!(d, CaptureDiagnostic::DiskCritical { .. }));
        assert!(got_critical, "应收到 DiskCritical 诊断");
        // 强制停录信号已发出（上层据此停采）。
        assert!(stop_rx.try_recv().is_ok(), "硬阈值应触发 force_stop 信号");

        cleanup(&path);
    }

    /// 注入软阈值 = u64::MAX-1（真实磁盘剩余必 < 之）+ 硬阈值 0 + 间隔 0 →
    /// **确定性**触发 DiskLow 预警（不阻断，不停录，TDD-02 §5.3）。
    #[test]
    fn disk_soft_warning_emits_disklow_deterministically() {
        let path = tmp_path("luna_writer_disk_soft.wav");
        cleanup(&path);

        // 软阈值接近 u64::MAX → 真实剩余必 < 软阈值 → DiskLow；硬阈值 0 → 不 Critical。
        let cfg = make_cfg_with_thresholds(path.clone(), u64::MAX - 1, 0, u64::MAX);
        let (diag_tx, diag_rx) = mpsc::channel();
        let writer = AudioFileWriter::spawn(cfg, 256, diag_tx, None).expect("spawn");

        writer.send_samples_blocking(vec![0.0f32; 160]);
        let intervals = writer.stop();

        // 软预警不停录：正常收尾一个分片。
        assert_eq!(intervals.len(), 1, "软预警不应中断录音");
        let got_low = std::iter::from_fn(|| diag_rx.try_recv().ok())
            .any(|d| matches!(d, CaptureDiagnostic::DiskLow { .. }));
        assert!(got_low, "应收到 DiskLow 预警诊断");

        cleanup(&path);
    }

    /// 写线程不阻塞消费（背压测试）：try_send 在通道满时返回 false 而非 block。
    #[test]
    fn backpressure_try_send_does_not_block() {
        let path = tmp_path("luna_writer_backpressure.wav");
        cleanup(&path);

        // 通道容量设为 2，人为造成背压。
        let cfg = WriterConfig::new(path.clone(), RecordingFormat::Wav, 16_000);
        let (diag_tx, _) = mpsc::channel();
        let writer = AudioFileWriter::spawn(cfg, 2, diag_tx, None).expect("spawn");

        // 快速塞满通道（写线程可能来不及消费）
        let mut rejected = 0usize;
        let samples = vec![0.0f32; 160];
        for _ in 0..20 {
            if !writer.send_samples(samples.clone()) {
                rejected += 1;
            }
        }
        // try_send 应在某些时刻返回 false（通道满背压），不阻塞主线程。
        // 即使全部成功也不代表失败（取决于调度），重要的是无 panic/无 block。
        let _ = writer.stop();
        let _ = rejected; // 背压次数仅供调试

        cleanup(&path);
    }

    /// 分片路径命名正确：part1/part2 后缀。
    #[test]
    fn make_part_path_naming() {
        let base = PathBuf::from("/tmp/session.wav");
        let p1 = make_part_path(&base, 1, RecordingFormat::Wav);
        let p2 = make_part_path(&base, 2, RecordingFormat::Wav);
        assert_eq!(p1, PathBuf::from("/tmp/session.part1.wav"));
        assert_eq!(p2, PathBuf::from("/tmp/session.part2.wav"));
    }
}
