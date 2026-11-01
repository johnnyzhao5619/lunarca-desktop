//! SRT 导出器（TDD-10 §2：序号 + `HH:MM:SS,mmm --> HH:MM:SS,mmm`，逗号毫秒）。
//! 输入先过 Segments → Cues 重切分（`subtitle_cues`，格式默认约束）。

use crate::{
    cue_lines, cue_speaker, subtitle_constraints, subtitle_cues, ExportError, ExportFormat,
    ExportOptions, Exportable, Exporter,
};
use std::time::Duration;

/// SubRip 字幕导出器（仅接受 [`Exportable::Segments`]；双语 = 一块内上下两行）。
pub struct SrtExporter;

/// SRT 时间戳（`HH:MM:SS,mmm`；逗号毫秒为 SubRip 规范，区别于 VTT 的点毫秒）。
fn srt_timestamp(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

impl Exporter for SrtExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Srt
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        let Exportable::Segments(segments) = content else {
            return Err(ExportError::MismatchedInput(
                "SRT 需要带时间戳的 Segments 输入",
            ));
        };
        // 重切分（TDD-10 §2）：屏读友好的 cue 粒度；说话人边界/双语语义由 cue 化保证。
        let cues = subtitle_cues(segments, &subtitle_constraints(segments, opts), opts);
        let mut out = String::new();
        for (index, cue) in (1u32..).zip(&cues) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&index.to_string());
            out.push('\n');
            out.push_str(&srt_timestamp(cue.start));
            out.push_str(" --> ");
            out.push_str(&srt_timestamp(cue.end));
            out.push('\n');
            // 说话人前缀（PRD-04-054）：块首行 `名字: 文本`（显示名由调用方预解析）。
            let speaker = cue_speaker(cue, opts);
            for (i, line) in cue_lines(cue, opts.bilingual).into_iter().enumerate() {
                if i == 0 {
                    if let Some(name) = speaker {
                        out.push_str(name);
                        out.push_str(": ");
                    }
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
        if out.is_empty() {
            return Err(ExportError::EmptyContent);
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
    fn blocks_are_indexed_with_comma_millis() {
        let content = Exportable::Segments(vec![
            seg(0, 1500, "hello", None),
            seg(1500, 62_480, "world", None),
        ]);
        let out = SrtExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert_eq!(
            out,
            "1\n00:00:00,000 --> 00:00:01,500\nhello\n\n2\n00:00:01,500 --> 00:01:02,480\nworld\n"
        );
    }

    #[test]
    fn bilingual_puts_source_and_translation_in_one_block() {
        let content = Exportable::Segments(vec![seg(0, 1000, "你好。", Some("Hello."))]);
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            ..ExportOptions::default()
        };
        let out = SrtExporter.export(&content, &opts).unwrap();
        assert_eq!(out, "1\n00:00:00,000 --> 00:00:01,000\n你好。\nHello.\n");
    }

    #[test]
    fn blank_segments_do_not_consume_indices() {
        let content =
            Exportable::Segments(vec![seg(0, 1000, "   ", None), seg(1000, 2000, "b", None)]);
        let out = SrtExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert!(out.starts_with("1\n"), "空白段不占序号");
        assert!(out.contains("b\n"));
    }

    #[test]
    fn speaker_prefix_prefixes_first_line_only() {
        // 显示名由调用方预解析进 seg.speaker；双语块仅源行带前缀。
        let mut s = seg(0, 1000, "你好。", Some("Hello."));
        s.speaker = Some("张三".into());
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            speaker_prefix: true,
            ..ExportOptions::default()
        };
        let out = SrtExporter
            .export(&Exportable::Segments(vec![s]), &opts)
            .unwrap();
        assert_eq!(
            out,
            "1\n00:00:00,000 --> 00:00:01,000\n张三: 你好。\nHello.\n"
        );
    }

    #[test]
    fn speaker_prefix_off_by_default() {
        let mut s = seg(0, 1000, "hello", None);
        s.speaker = Some("张三".into());
        let out = SrtExporter
            .export(&Exportable::Segments(vec![s]), &ExportOptions::default())
            .unwrap();
        assert!(!out.contains("张三"), "未开启选项不得输出前缀");
    }

    #[test]
    fn text_input_is_rejected() {
        let err = SrtExporter
            .export(&Exportable::Text("x".into()), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::MismatchedInput(_)));
    }

    #[test]
    fn empty_segments_are_rejected() {
        let err = SrtExporter
            .export(&Exportable::Segments(vec![]), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::EmptyContent));
    }
}
