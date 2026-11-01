//! LRC 歌词导出器（TDD-10 §2：`[mm:ss.xx]` 行式；双语 = 同时间戳双行）。
//! 输入先过 Segments → Cues 重切分（行式约束，见 [`LRC_CONSTRAINTS`]）。
//!
//! 格式语义如实实现（TDD-10 §2 纪律）：LRC **没有显式 end 时间**——下一行时间戳即
//! 隐式终点；不为对齐 SRT 造假字段。分钟为总分钟（可 >59，LRC 惯例）。

use crate::{
    cue_lines, cue_speaker, subtitle_cues, ExportError, ExportFormat, ExportOptions, Exportable,
    Exporter,
};
use luna_core::CueConstraints;
use std::fmt::Write as _;
use std::time::Duration;

/// LRC 行式约束：不折行（LRC 无换行语义，`max_lines=1` + 行长不限）；时长/CPS 约束
/// 沿用默认（合并过短段仍有价值——歌词行粒度）。
fn lrc_constraints(opts: &ExportOptions) -> CueConstraints {
    CueConstraints {
        // 行长默认不限（LRC 一行一时间戳）；用户显式给了行长覆盖则按它切成多条。
        max_line_len: opts.max_line_len.unwrap_or(0),
        max_lines: 1,
        ..CueConstraints::default()
    }
}

/// LRC 歌词导出器（仅接受 [`Exportable::Segments`]）。
pub struct LrcExporter;

/// LRC 时间戳（`[mm:ss.xx]`，xx = 厘秒；mm 为总分钟可 >59）。
fn lrc_timestamp(d: Duration) -> String {
    let total_cs = d.as_millis() as u64 / 10;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let m = total_s / 60;
    format!("[{m:02}:{s:02}.{cs:02}]")
}

impl Exporter for LrcExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Lrc
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        let Exportable::Segments(segments) = content else {
            return Err(ExportError::MismatchedInput(
                "LRC 需要带时间戳的 Segments 输入",
            ));
        };
        let cues = subtitle_cues(segments, &lrc_constraints(opts), opts);
        if cues.is_empty() {
            return Err(ExportError::EmptyContent);
        }
        let mut out = String::new();
        for cue in &cues {
            let ts = lrc_timestamp(cue.start);
            let speaker = cue_speaker(cue, opts);
            for (i, line) in cue_lines(cue, opts.bilingual).into_iter().enumerate() {
                // 双语 = 同时间戳双行（源行在前译行在后，LRC 双语惯例）。
                let _ = write!(out, "{ts}");
                if i == 0 {
                    if let Some(name) = speaker {
                        let _ = write!(out, "{name}: ");
                    }
                }
                let _ = writeln!(out, "{line}");
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::seg;
    use crate::Bilingual;

    #[test]
    fn timestamped_lines_with_centiseconds_and_total_minutes() {
        // 61 分钟处的行：mm 为总分钟（61），不进位小时（LRC 惯例）。
        let content = Exportable::Segments(vec![
            seg(0, 2_000, "第一行", None),
            seg(3_660_500, 3_662_000, "一小时后", None),
        ]);
        let out = LrcExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert!(out.starts_with("[00:00.00]第一行\n"), "{out}");
        assert!(out.contains("[61:00.50]一小时后\n"), "{out}");
    }

    #[test]
    fn bilingual_emits_double_lines_same_timestamp() {
        let content = Exportable::Segments(vec![seg(1_000, 3_000, "你好", Some("Hello"))]);
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            ..ExportOptions::default()
        };
        let out = LrcExporter.export(&content, &opts).unwrap();
        assert_eq!(out, "[00:01.00]你好\n[00:01.00]Hello\n");
    }

    #[test]
    fn speaker_prefix_on_source_line_only() {
        let mut s = seg(0, 2_000, "你好", Some("Hello"));
        s.speaker = Some("张三".into());
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            speaker_prefix: true,
            ..ExportOptions::default()
        };
        let out = LrcExporter
            .export(&Exportable::Segments(vec![s]), &opts)
            .unwrap();
        assert_eq!(out, "[00:00.00]张三: 你好\n[00:00.00]Hello\n");
    }

    #[test]
    fn empty_segments_are_rejected() {
        let err = LrcExporter
            .export(&Exportable::Segments(vec![]), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::EmptyContent));
    }
}
