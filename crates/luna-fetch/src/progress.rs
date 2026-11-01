//! 抓取进度：机器可读模板与解析（纯逻辑，TDD-03 §11.3）。
//!
//! yt-dlp 给人看的进度条是 ANSI 回车覆盖的一行文本，**不是契约**（措辞与列宽随版本变）。
//! 官方为此提供 `--progress-template`：我们自定一行带哨兵前缀的字段序列，解析就成了纯字符串
//! 拆分，与工具版本解耦。
//!
//! 字段选择的取舍：`total_bytes` 只在服务器给了 `Content-Length` 时存在，否则要用
//! `total_bytes_estimate`（DASH 分片流的常态）。二者皆缺就是**不定量进度**——如实呈现
//! 转圈而不是编一个假百分比。

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 进度行哨兵前缀：把我们的进度行与 `--print` 的落点行区分开（同走 stdout）。
pub const PROGRESS_PREFIX: &str = "LUNA_FETCH\t";

/// 传给 `--progress-template` 的模板。
///
/// 制表符分隔而非空格：字段值可能带空格（`%(progress...)s` 缺失时 yt-dlp 输出 `NA`，
/// 但未来字段若含空格，空格分隔就会静默错位）。
pub const PROGRESS_TEMPLATE: &str = "download:LUNA_FETCH\t%(progress.downloaded_bytes)s\t%(progress.total_bytes)s\t%(progress.total_bytes_estimate)s";

/// 抓取进度（字节口径）。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchProgress {
    /// 已下载字节。
    pub downloaded: u64,
    /// 总字节（`None` = 服务器未给且无估算 → 不定量）。
    pub total: Option<u64>,
}

impl FetchProgress {
    /// 队列进度以 **KiB** 计数上报（`ProcStatus::Running{done,total}` 是 `u32`，
    /// 而字节数轻易越过 4GiB；KiB 口径的 u32 上限是 4TiB，足够任何单个媒体文件）。
    #[must_use]
    pub fn as_kib(self) -> (u32, u32) {
        let to_kib = |bytes: u64| u32::try_from(bytes / 1024).unwrap_or(u32::MAX);
        (to_kib(self.downloaded), self.total.map_or(0, to_kib))
    }
}

/// 进度上报节流（**纯逻辑**，状态机可单测）。
///
/// 为什么必须节流：yt-dlp `--newline` 每次进度更新吐一行（约 10 行/秒），而队列的每次上报都是
/// **整队列 JSON 快照 + IPC 广播**（`emit_processing_queue`）。既有的转写进度是"每分片一次"
/// （几秒一次）故从来不需要节流，**抓取是第一个高频进度源**——不节流就是每秒十几次整队列
/// 序列化，与 ADR-011「高频流不走 JSON」的精神相悖。
///
/// 两条必须放行的例外，否则进度条会说谎：
/// - **首次**上报（否则进度条在头 250ms 里停在 0）；
/// - **下载完成**那一帧（`downloaded == total`，否则进度条永远差最后一格）。
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<Instant>,
}

impl ProgressThrottle {
    /// 默认间隔 250ms（≈4 次/秒：肉眼已足够平滑，而广播成本降一个数量级）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_interval(Duration::from_millis(250))
    }

    /// 自定间隔（测试用）。
    #[must_use]
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// 该帧是否应当上报；`now` 由调用方注入（测试可控时间，不睡真实时钟）。
    pub fn should_emit_at(&mut self, progress: FetchProgress, now: Instant) -> bool {
        let complete = progress
            .total
            .is_some_and(|total| total > 0 && progress.downloaded >= total);
        let due = self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        if due || complete {
            self.last = Some(now);
            return true;
        }
        false
    }

    /// [`Self::should_emit_at`] 的当前时钟版本（生产路径）。
    pub fn should_emit(&mut self, progress: FetchProgress) -> bool {
        self.should_emit_at(progress, Instant::now())
    }
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// 解析一行进度（**纯函数**）。返回 `None` = 这不是进度行（调用方按落点行处理）。
///
/// 容错口径：`NA`/空/非数 一律当"未知"。总量为 0 或小于已下载量时按未知处理——
/// 那会让进度条显示成 >100%，比不定量更糟。
#[must_use]
pub fn parse_progress_line(line: &str) -> Option<FetchProgress> {
    let rest = line.strip_prefix(PROGRESS_PREFIX)?;
    let fields: Vec<&str> = rest.split('\t').collect();
    let field = |i: usize| fields.get(i).copied().and_then(parse_bytes);
    let downloaded = field(0).unwrap_or(0);
    let total = field(1)
        .or_else(|| field(2))
        .filter(|total| *total >= downloaded && *total > 0);
    Some(FetchProgress { downloaded, total })
}

/// 字节字段解析：`NA` / 空 / 非数 → `None`；浮点（估算值常带小数）取整。
fn parse_bytes(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("na") || trimmed == "None" {
        return None;
    }
    if let Ok(exact) = trimmed.parse::<u64>() {
        return Some(exact);
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模板必须与解析器同源：前缀一致、字段顺序一致——二者分家是最典型的"进度恒为 0"。
    #[test]
    fn template_and_parser_agree() {
        assert!(PROGRESS_TEMPLATE.contains(PROGRESS_PREFIX));
        assert!(PROGRESS_TEMPLATE.starts_with("download:"));
        let fields: Vec<&str> = PROGRESS_TEMPLATE.split('\t').collect();
        assert_eq!(fields.len(), 4, "哨兵 + 三个字段：{PROGRESS_TEMPLATE}");
        assert!(fields[1].contains("downloaded_bytes"));
        assert!(fields[2].contains("total_bytes)"));
        assert!(fields[3].contains("total_bytes_estimate"));
    }

    #[test]
    fn parses_exact_total() {
        let line = "LUNA_FETCH\t1048576\t8388608\tNA";
        assert_eq!(
            parse_progress_line(line),
            Some(FetchProgress {
                downloaded: 1_048_576,
                total: Some(8_388_608)
            })
        );
    }

    /// `total_bytes` 缺失（DASH 分片流常态）时回落估算值，而不是直接判不定量。
    #[test]
    fn falls_back_to_estimate_when_exact_total_missing() {
        let line = "LUNA_FETCH\t500\tNA\t4096.7";
        assert_eq!(
            parse_progress_line(line),
            Some(FetchProgress {
                downloaded: 500,
                total: Some(4096)
            })
        );
    }

    /// 两个总量都缺 → 不定量（如实转圈），绝不编造百分比。
    #[test]
    fn both_totals_missing_is_indeterminate() {
        assert_eq!(
            parse_progress_line("LUNA_FETCH\t500\tNA\tNA"),
            Some(FetchProgress {
                downloaded: 500,
                total: None
            })
        );
    }

    /// 总量小于已下载 / 为 0 时按未知处理——显示 >100% 比不定量更让人怀疑程序坏了。
    #[test]
    fn nonsense_total_is_treated_as_unknown() {
        assert_eq!(
            parse_progress_line("LUNA_FETCH\t900\t100\tNA")
                .unwrap()
                .total,
            None
        );
        assert_eq!(
            parse_progress_line("LUNA_FETCH\t900\t0\t0").unwrap().total,
            None
        );
    }

    /// 节流：间隔内的中间帧被丢弃，但**首帧**与**完成帧**必须放行——否则进度条会在头
    /// 250ms 停在 0、并永远差最后一格。
    #[test]
    fn throttle_passes_first_and_final_frames_and_drops_the_middle() {
        let mut throttle = ProgressThrottle::with_interval(Duration::from_millis(250));
        let base = Instant::now();
        let frame = |downloaded, total| FetchProgress {
            downloaded,
            total: Some(total),
        };

        // 首帧必过（哪怕它紧跟在构造之后）。
        assert!(throttle.should_emit_at(frame(0, 1000), base));
        // 间隔内的中间帧一律丢弃。
        assert!(!throttle.should_emit_at(frame(100, 1000), base + Duration::from_millis(50)));
        assert!(!throttle.should_emit_at(frame(200, 1000), base + Duration::from_millis(240)));
        // 到点放行，并重置计时。
        assert!(throttle.should_emit_at(frame(300, 1000), base + Duration::from_millis(260)));
        assert!(!throttle.should_emit_at(frame(400, 1000), base + Duration::from_millis(300)));
        // 完成帧无论距上次多近都必过（否则进度条永远差最后一格）。
        assert!(throttle.should_emit_at(frame(1000, 1000), base + Duration::from_millis(310)));
    }

    /// 总量未知（不定量）时不存在"完成帧"，故一律按间隔节流——不能把每一帧都当完成放行。
    #[test]
    fn throttle_has_no_completion_shortcut_when_total_unknown() {
        let mut throttle = ProgressThrottle::with_interval(Duration::from_millis(250));
        let base = Instant::now();
        let indeterminate = FetchProgress {
            downloaded: 9_999,
            total: None,
        };
        assert!(throttle.should_emit_at(indeterminate, base));
        assert!(!throttle.should_emit_at(indeterminate, base + Duration::from_millis(100)));
        assert!(throttle.should_emit_at(indeterminate, base + Duration::from_millis(300)));
    }

    /// 落点行（`--print` 输出）走同一条 stdout，必须不被误判为进度。
    #[test]
    fn non_progress_lines_are_rejected() {
        assert!(parse_progress_line("/tmp/luna/会议.m4a").is_none());
        assert!(parse_progress_line("").is_none());
        assert!(parse_progress_line("[download] 100% of 8.00MiB").is_none());
    }

    /// KiB 换算：队列的 u32 计数口径。超 4TiB 饱和而不是回绕成小数字。
    #[test]
    fn kib_projection_saturates_instead_of_wrapping() {
        let (done, total) = FetchProgress {
            downloaded: 2048,
            total: Some(1024 * 1024),
        }
        .as_kib();
        assert_eq!((done, total), (2, 1024));

        let (huge, _) = FetchProgress {
            downloaded: u64::MAX,
            total: None,
        }
        .as_kib();
        assert_eq!(huge, u32::MAX, "饱和，不回绕");

        // 不定量：total=0 即前端的"不定量"约定（与转译阶段同口径）。
        assert_eq!(
            FetchProgress {
                downloaded: 0,
                total: None
            }
            .as_kib()
            .1,
            0
        );
    }
}
