//! 视频章节时间戳（PRD-19-021，ADR-162 ⑥⑪）：解析、YouTube 规则校验、自动修正与格式化。
//!
//! 章节在界面上就是 YouTube 简介里的那几行文字（`0:00 开场`），用户直接改文字；这里负责
//! 把文字读成章节、按规则逐条说清哪里不合规、能修的给出修正后的文字。零 IO、纯函数，
//! 前端只渲染（ADR-003）。
//!
//! YouTube 的规则：第一个时间戳是 `0:00`；至少三个；每章至少 10 秒；时间递增。

/// 至少几章。
pub const MIN_CHAPTERS: usize = 3;
/// 每章至少多长（毫秒）。
pub const MIN_CHAPTER_MS: u64 = 10_000;

/// 一个章节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chapter {
    /// 起点（毫秒）。
    pub start_ms: u64,
    /// 标题。
    pub title: String,
}

/// 一处不合规（`index` 是解析出的第几章，0 起；`line` 是文字里的第几行，1 起）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChapterIssue {
    /// 这一行读不出「时间 标题」。
    Unparsed { line: usize },
    /// 第一个时间戳不是 `0:00`。
    FirstNotAtZero,
    /// 少于三章。
    TooFew { count: usize },
    /// 这一章不到 10 秒。
    TooShort { index: usize, seconds: u64 },
    /// 这一章的时间不比上一章晚。
    NotIncreasing { index: usize },
    /// 这一章没有标题。
    EmptyTitle { index: usize },
    /// 这一章的起点超出了成片长度。
    PastEnd { index: usize },
}

/// 把 `0:00`、`1:02:03`、`75`（秒）读成毫秒；读不出为 `None`。
#[must_use]
pub fn parse_timestamp(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let value: u64 = part.parse().ok()?;
        // 分、秒两位里不能超过 59（`1:75` 是笔误，不是 2:15）。
        if i > 0 && value >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(value)?;
    }
    total.checked_mul(1000)
}

/// 毫秒 → `m:ss`；满一小时为 `h:mm:ss`（YouTube 简介里的写法）。
#[must_use]
pub fn format_timestamp(ms: u64, with_hours: bool) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if with_hours {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{}:{s:02}", h * 60 + m)
    }
}

/// 读一段章节文字：每行「时间 标题」，时间与标题之间可以有空格、`-`、`–`、`—`、`|`。
/// 空行跳过；读不出的行记进第二个返回值（行号，1 起）。
#[must_use]
pub fn parse_chapters(text: &str) -> (Vec<Chapter>, Vec<usize>) {
    let mut chapters = Vec::new();
    let mut unparsed = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let split = line.find(|c: char| c.is_whitespace()).unwrap_or(line.len());
        let (time, rest) = line.split_at(split);
        match parse_timestamp(time) {
            Some(start_ms) => chapters.push(Chapter {
                start_ms,
                title: rest
                    .trim()
                    .trim_start_matches(['-', '–', '—', '|', ':'])
                    .trim()
                    .to_owned(),
            }),
            None => unparsed.push(index + 1),
        }
    }
    (chapters, unparsed)
}

/// 章节 → 文字（每行 `时间 标题`；任一章满一小时则全部用 `h:mm:ss`，免得同一张表两种写法）。
#[must_use]
pub fn format_chapters(chapters: &[Chapter]) -> String {
    let with_hours = chapters.iter().any(|c| c.start_ms >= 3_600_000);
    chapters
        .iter()
        .map(|c| format!("{} {}", format_timestamp(c.start_ms, with_hours), c.title))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 按 YouTube 规则逐条检查（`duration_ms` 已知时还查最后一章的长度与是否超出片长）。
#[must_use]
pub fn check_chapters(chapters: &[Chapter], duration_ms: Option<u64>) -> Vec<ChapterIssue> {
    let mut issues = Vec::new();
    if chapters.first().is_some_and(|c| c.start_ms != 0) {
        issues.push(ChapterIssue::FirstNotAtZero);
    }
    if chapters.len() < MIN_CHAPTERS {
        issues.push(ChapterIssue::TooFew {
            count: chapters.len(),
        });
    }
    for (index, chapter) in chapters.iter().enumerate() {
        if chapter.title.trim().is_empty() {
            issues.push(ChapterIssue::EmptyTitle { index });
        }
        if duration_ms.is_some_and(|d| chapter.start_ms >= d) {
            issues.push(ChapterIssue::PastEnd { index });
            continue;
        }
        if index > 0 && chapter.start_ms <= chapters[index - 1].start_ms {
            issues.push(ChapterIssue::NotIncreasing { index });
            continue;
        }
        let end = chapters
            .get(index + 1)
            .map(|next| next.start_ms)
            .or(duration_ms);
        if let Some(end) = end {
            if end > chapter.start_ms && end - chapter.start_ms < MIN_CHAPTER_MS {
                issues.push(ChapterIssue::TooShort {
                    index,
                    seconds: (end - chapter.start_ms) / 1000,
                });
            }
        }
    }
    issues
}

/// 能自动修的都修掉：按时间排序、同一时刻只留第一章、超出片长的丢掉、不足 10 秒的章并入
/// 前一章（最后一章不足 10 秒就把它并进前一章）、第一个时间改成 `0:00`。章数不够修不出来。
#[must_use]
pub fn fix_chapters(mut chapters: Vec<Chapter>, duration_ms: Option<u64>) -> Vec<Chapter> {
    chapters.retain(|c| !c.title.trim().is_empty());
    if let Some(duration) = duration_ms {
        chapters.retain(|c| c.start_ms < duration);
    }
    chapters.sort_by_key(|c| c.start_ms);
    chapters.dedup_by_key(|c| c.start_ms);
    if let Some(first) = chapters.first_mut() {
        first.start_ms = 0;
    }
    let mut fixed: Vec<Chapter> = Vec::with_capacity(chapters.len());
    for chapter in chapters {
        match fixed.last() {
            Some(previous) if chapter.start_ms - previous.start_ms < MIN_CHAPTER_MS => {}
            _ => fixed.push(chapter),
        }
    }
    if let (Some(duration), true) = (duration_ms, fixed.len() > 1) {
        if fixed
            .last()
            .is_some_and(|last| duration.saturating_sub(last.start_ms) < MIN_CHAPTER_MS)
        {
            fixed.pop();
        }
    }
    fixed
}

/// 把一个时间吸附到最近的句子起点（`starts` 升序）；没有句子时原样返回。
#[must_use]
pub fn snap_to_nearest(starts: &[u64], ms: u64) -> u64 {
    let at = starts.partition_point(|&s| s < ms);
    let before = at.checked_sub(1).and_then(|i| starts.get(i)).copied();
    let after = starts.get(at).copied();
    match (before, after) {
        (Some(b), Some(a)) => {
            if ms - b <= a - ms {
                b
            } else {
                a
            }
        }
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => ms,
    }
}

/// 送给模型的转写：按时间窗合并成「`[m:ss] 文字`」行。超出 `max_chars` 就把窗口加宽、每行截短，
/// 而不是截掉片尾——片尾也要有章节。`lines` 是（起点毫秒，文字）。
#[must_use]
pub fn chapter_source(lines: &[(u64, String)], max_chars: usize) -> String {
    let mut window_ms: u64 = 15_000;
    let mut line_cap: usize = 400;
    loop {
        let mut out = String::new();
        let mut current: Option<(u64, String)> = None;
        let flush = |block: Option<(u64, String)>, out: &mut String| {
            if let Some((start, text)) = block {
                let text: String = text.chars().take(line_cap).collect();
                out.push_str(&format!(
                    "[{}] {}\n",
                    format_timestamp(start, false),
                    text.trim()
                ));
            }
        };
        for (start, text) in lines {
            match current.as_mut() {
                Some((block_start, block_text))
                    if start.saturating_sub(*block_start) < window_ms =>
                {
                    block_text.push(' ');
                    block_text.push_str(text.trim());
                }
                _ => {
                    flush(current.take(), &mut out);
                    current = Some((*start, text.trim().to_owned()));
                }
            }
        }
        flush(current.take(), &mut out);
        if out.chars().count() <= max_chars || window_ms >= 600_000 {
            return out;
        }
        window_ms *= 2;
        line_cap = (line_cap * 2 / 3).max(60);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(start_s: u64, title: &str) -> Chapter {
        Chapter {
            start_ms: start_s * 1000,
            title: title.into(),
        }
    }

    #[test]
    fn timestamps_read_and_write_the_way_youtube_writes_them() {
        assert_eq!(parse_timestamp("0:00"), Some(0));
        assert_eq!(parse_timestamp("1:05"), Some(65_000));
        assert_eq!(parse_timestamp("1:02:03"), Some(3_723_000));
        assert_eq!(parse_timestamp("75"), Some(75_000));
        assert_eq!(parse_timestamp("1:75"), None, "秒位过 59 是笔误");
        assert_eq!(parse_timestamp("a:00"), None);
        assert_eq!(format_timestamp(65_000, false), "1:05");
        assert_eq!(format_timestamp(3_723_000, true), "1:02:03");
        assert_eq!(format_timestamp(65_000, true), "0:01:05");
    }

    #[test]
    fn lines_parse_with_common_separators_and_bad_lines_are_reported() {
        let (chapters, unparsed) =
            parse_chapters("0:00 开场\n1:05 - 第一件事\n\n这一行没有时间\n2:30｜结尾");
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1], ch(65, "第一件事"));
        assert_eq!(unparsed, [4, 5], "全角竖线前没有空格，时间读不出");
        let (chapters, _) = parse_chapters("2:30 | 结尾");
        assert_eq!(chapters[0].title, "结尾");
    }

    #[test]
    fn every_youtube_rule_is_reported() {
        let issues = check_chapters(&[ch(5, "开场"), ch(12, ""), ch(12, "重复")], Some(60_000));
        assert!(issues.contains(&ChapterIssue::FirstNotAtZero));
        assert!(issues.contains(&ChapterIssue::TooShort {
            index: 0,
            seconds: 7
        }));
        assert!(issues.contains(&ChapterIssue::EmptyTitle { index: 1 }));
        assert!(issues.contains(&ChapterIssue::NotIncreasing { index: 2 }));
        assert_eq!(
            check_chapters(&[ch(0, "开场"), ch(30, "中段")], None),
            [ChapterIssue::TooFew { count: 2 }]
        );
        assert!(check_chapters(&[ch(0, "a"), ch(20, "b"), ch(40, "c")], Some(60_000)).is_empty());
        // 最后一章也要满 10 秒；超出片长单独报。
        assert!(
            check_chapters(&[ch(0, "a"), ch(20, "b"), ch(55, "c")], Some(60_000)).contains(
                &ChapterIssue::TooShort {
                    index: 2,
                    seconds: 5
                }
            )
        );
        assert!(
            check_chapters(&[ch(0, "a"), ch(20, "b"), ch(70, "c")], Some(60_000))
                .contains(&ChapterIssue::PastEnd { index: 2 })
        );
    }

    #[test]
    fn fixing_sorts_merges_short_chapters_and_starts_at_zero() {
        let fixed = fix_chapters(
            vec![
                ch(40, "c"),
                ch(3, "a"),
                ch(20, "b"),
                ch(25, "太短"),
                ch(20, "同一时刻"),
                ch(99, "片外"),
            ],
            Some(60_000),
        );
        assert_eq!(fixed, [ch(0, "a"), ch(20, "b"), ch(40, "c")]);
        assert!(check_chapters(&fixed, Some(60_000)).is_empty());
        // 最后一章不足 10 秒：并进前一章。
        let fixed = fix_chapters(
            vec![ch(0, "a"), ch(20, "b"), ch(40, "c"), ch(55, "尾")],
            Some(60_000),
        );
        assert_eq!(fixed.last().unwrap().title, "c");
        // 章数不够修不出来。
        assert_eq!(fix_chapters(vec![ch(0, "a")], None).len(), 1);
    }

    #[test]
    fn formatting_switches_to_hours_for_the_whole_list() {
        assert_eq!(
            format_chapters(&[ch(0, "开场"), ch(65, "正文")]),
            "0:00 开场\n1:05 正文"
        );
        assert_eq!(
            format_chapters(&[ch(0, "开场"), ch(3700, "后半")]),
            "0:00:00 开场\n1:01:40 后半"
        );
    }

    #[test]
    fn times_snap_to_the_nearest_sentence_start() {
        let starts = [0, 6_000, 12_000];
        assert_eq!(snap_to_nearest(&starts, 7_000), 6_000);
        assert_eq!(snap_to_nearest(&starts, 10_000), 12_000);
        assert_eq!(snap_to_nearest(&starts, 99_000), 12_000);
        assert_eq!(snap_to_nearest(&[], 5_000), 5_000);
    }

    #[test]
    fn a_long_transcript_widens_windows_instead_of_losing_the_end() {
        let lines: Vec<(u64, String)> = (0..600)
            .map(|i| (i * 5_000, format!("第 {i} 句话的内容比较长一些")))
            .collect();
        let short = chapter_source(&lines, 2_000);
        assert!(short.chars().count() <= 2_000, "{}", short.chars().count());
        assert!(short.starts_with("[0:00]"));
        // 片尾（第 599 句在 49:55）还在。
        let last = short.lines().last().unwrap();
        let last_start =
            parse_timestamp(last.trim_start_matches('[').split(']').next().unwrap()).unwrap();
        assert!(last_start >= 2_400_000, "{last}");
    }
}
