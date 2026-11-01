//! 时间轴行标记 `[HH:MM:SS.mmm]` 的写入与解析（TDD-06 §2.2）。
//!
//! **毫秒精度恒定落盘**：[`format_timecode`] 始终输出 `.mmm` 后缀；秒级仅用于展示或
//! 读取由旧工具写入的标记（[`parse_timecode`] 秒级降级读）。
//!
//! FTS 卫生：[`crate::parse::clean_for_fts`] 已在入库前剥除时间码与 `luna:` 块，
//! 搜索不会命中时间码字符或 frontmatter 键——此模块仅负责序列化/反序列化。

use std::time::Duration;

/// 将 [`Duration`] 格式化为 `[HH:MM:SS.mmm]`（毫秒精度恒定）。
///
/// # Examples
/// ```
/// # use luna_vault::timecode::format_timecode;
/// # use std::time::Duration;
/// assert_eq!(format_timecode(Duration::from_millis(12480)), "[00:00:12.480]");
/// assert_eq!(format_timecode(Duration::from_millis(3723001)), "[01:02:03.001]");
/// ```
#[must_use]
pub fn format_timecode(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("[{h:02}:{m:02}:{s:02}.{ms:03}]")
}

/// 解析字符串起始处的行标记 `[HH:MM:SS.mmm]` 或 `[HH:MM:SS]`。
///
/// 成功返回 `(Duration, remaining)` 其中 `remaining` 是标记之后的字符串切片；
/// 不匹配则返回 `None`。秒级标记（无 `.mmm`）按整秒解析（就近降级读，§2.2）。
///
/// # Examples
/// ```
/// # use luna_vault::timecode::parse_timecode;
/// # use std::time::Duration;
/// let (dur, rest) = parse_timecode("[00:00:12.480] 大家好").unwrap();
/// assert_eq!(dur, Duration::from_millis(12480));
/// assert_eq!(rest, " 大家好");
///
/// // 秒级降级
/// let (dur2, _) = parse_timecode("[01:02:03] 第二段").unwrap();
/// assert_eq!(dur2, Duration::from_secs(3723));
/// ```
#[must_use]
pub fn parse_timecode(s: &str) -> Option<(Duration, &str)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    // 最短形式 [HH:MM:SS] = 10 字节
    if bytes.len() < 10 {
        return None;
    }

    let parse2 = |idx: usize| -> Option<u64> {
        let a = bytes.get(idx)?.wrapping_sub(b'0');
        let b = bytes.get(idx + 1)?.wrapping_sub(b'0');
        if a > 9 || b > 9 {
            return None;
        }
        Some(a as u64 * 10 + b as u64)
    };

    let h = parse2(1)?;
    if bytes.get(3) != Some(&b':') {
        return None;
    }
    let m = parse2(4)?;
    if bytes.get(6) != Some(&b':') {
        return None;
    }
    let sec = parse2(7)?;

    let (total_ms, end_idx) = if bytes.get(9) == Some(&b'.') {
        // 解析 .d{1,3}（补齐：1位→×100, 2位→×10, 3位→×1）
        let mut digits = [0u64; 3];
        let mut n = 0usize;
        let mut i = 10;
        while n < 3 {
            match bytes.get(i) {
                Some(&d) if d.is_ascii_digit() => {
                    digits[n] = (d - b'0') as u64;
                    n += 1;
                    i += 1;
                }
                _ => break,
            }
        }
        if n == 0 {
            return None;
        }
        if bytes.get(i) != Some(&b']') {
            return None;
        }
        let ms_val = digits[0] * 100 + digits[1] * 10 + digits[2];
        let base_ms = (h * 3600 + m * 60 + sec) * 1000;
        (base_ms + ms_val, i + 1)
    } else if bytes.get(9) == Some(&b']') {
        // 秒级降级
        let base_ms = (h * 3600 + m * 60 + sec) * 1000;
        (base_ms, 10)
    } else {
        return None;
    };

    Some((Duration::from_millis(total_ms), &s[end_idx..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_millisecond_precision() {
        assert_eq!(
            format_timecode(Duration::from_millis(12480)),
            "[00:00:12.480]"
        );
        assert_eq!(
            format_timecode(Duration::from_millis(3723001)),
            "[01:02:03.001]"
        );
        assert_eq!(format_timecode(Duration::ZERO), "[00:00:00.000]");
        assert_eq!(
            format_timecode(Duration::from_millis(3599999)),
            "[00:59:59.999]"
        );
        // 超过 1 小时
        assert_eq!(
            format_timecode(Duration::from_millis(7384123)),
            "[02:03:04.123]"
        );
    }

    #[test]
    fn round_trip_millisecond() {
        // 毫秒精度 round-trip：format → parse → 同值。
        let cases = [
            Duration::ZERO,
            Duration::from_millis(12480),
            Duration::from_millis(3723456),
            Duration::from_millis(7199999),
            Duration::from_millis(1),
            Duration::from_millis(999),
        ];
        for d in cases {
            let s = format_timecode(d);
            let (parsed, rest) = parse_timecode(&s).unwrap_or_else(|| panic!("解析失败：{s}"));
            assert_eq!(parsed, d, "round-trip 失败：{s}");
            assert_eq!(rest, "", "应无剩余字符：{s}");
        }
    }

    #[test]
    fn parse_with_trailing_text() {
        let (dur, rest) = parse_timecode("[00:00:12.480] 大家好").unwrap();
        assert_eq!(dur, Duration::from_millis(12480));
        assert_eq!(rest, " 大家好");
    }

    #[test]
    fn parse_second_level_degrades_to_whole_second() {
        // 就近降级读：秒级标记按整秒解析。
        let (dur, rest) = parse_timecode("[01:02:03] 第二段").unwrap();
        assert_eq!(dur, Duration::from_secs(3723)); // 1*3600 + 2*60 + 3
        assert_eq!(rest, " 第二段");
    }

    #[test]
    fn parse_one_and_two_digit_ms() {
        // 1 位毫秒：.4 → 400 ms
        let (d1, _) = parse_timecode("[00:00:01.4]").unwrap();
        assert_eq!(d1, Duration::from_millis(1400));
        // 2 位毫秒：.48 → 480 ms
        let (d2, _) = parse_timecode("[00:00:01.48]").unwrap();
        assert_eq!(d2, Duration::from_millis(1480));
    }

    #[test]
    fn non_timecode_brackets_not_parsed() {
        assert!(parse_timecode("[1]").is_none());
        assert!(parse_timecode("[ ]").is_none());
        assert!(parse_timecode("[foo]").is_none());
        assert!(parse_timecode("普通文本").is_none());
        assert!(parse_timecode("[00:00]").is_none()); // 缺秒
        assert!(parse_timecode("[00:00:0x]").is_none()); // 非数字
    }
}
