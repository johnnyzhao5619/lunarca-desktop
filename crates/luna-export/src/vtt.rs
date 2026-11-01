//! WebVTT 导出器（TDD-10 §2：`WEBVTT` 头 + `HH:MM:SS.mmm --> HH:MM:SS.mmm`，点毫秒）。
//! 输入先过 Segments → Cues 重切分（`subtitle_cues`，格式默认约束）。

use crate::{
    cue_lines, cue_speaker, subtitle_constraints, subtitle_cues, ExportError, ExportFormat,
    ExportOptions, Exportable, Exporter,
};
use std::time::Duration;

/// WebVTT 字幕导出器（仅接受 [`Exportable::Segments`]；双语 = 一块内上下两行）。
pub struct VttExporter;

/// VTT 时间戳（`HH:MM:SS.mmm`；点毫秒为 WebVTT 规范，区别于 SRT 的逗号毫秒）。
fn vtt_timestamp(d: Duration) -> String {
    let total_ms = d.as_millis() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

impl Exporter for VttExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Vtt
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        let Exportable::Segments(segments) = content else {
            return Err(ExportError::MismatchedInput(
                "VTT 需要带时间戳的 Segments 输入",
            ));
        };
        let cues = subtitle_cues(segments, &subtitle_constraints(segments, opts), opts);
        if cues.is_empty() {
            return Err(ExportError::EmptyContent);
        }
        let mut out = String::from("WEBVTT\n");
        for cue in &cues {
            out.push('\n');
            out.push_str(&vtt_timestamp(cue.start));
            out.push_str(" --> ");
            out.push_str(&vtt_timestamp(cue.end));
            out.push('\n');
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
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::seg;
    use crate::Bilingual;

    #[test]
    fn header_and_dot_millis() {
        let content = Exportable::Segments(vec![seg(0, 1500, "hello", None)]);
        let out = VttExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert_eq!(out, "WEBVTT\n\n00:00:00.000 --> 00:00:01.500\nhello\n");
    }

    #[test]
    fn bilingual_and_speaker_prefix() {
        let mut s = seg(0, 2_000, "你好。", Some("Hello."));
        s.speaker = Some("张三".into());
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            speaker_prefix: true,
            ..ExportOptions::default()
        };
        let out = VttExporter
            .export(&Exportable::Segments(vec![s]), &opts)
            .unwrap();
        assert_eq!(
            out,
            "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\n张三: 你好。\nHello.\n"
        );
    }

    #[test]
    fn empty_segments_are_rejected() {
        let err = VttExporter
            .export(&Exportable::Segments(vec![]), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::EmptyContent));
    }

    #[test]
    fn text_input_is_rejected() {
        let err = VttExporter
            .export(&Exportable::Text("x".into()), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::MismatchedInput(_)));
    }
}
