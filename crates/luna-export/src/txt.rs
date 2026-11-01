//! txt 导出器（TDD-10 §2：纯文本；可选行首时间标记，与 Vault 时间码同构）。

use crate::{
    content_lines, presentable, speaker_of, ExportError, ExportFormat, ExportOptions, Exportable,
    Exporter,
};
use luna_vault::timecode::format_timecode;

/// 纯文本导出器：`Segments` → 逐行文本（可选 `[HH:MM:SS.mmm]` 前缀）；`Text` → 原样。
pub struct TxtExporter;

impl Exporter for TxtExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Txt
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        match content {
            Exportable::Text(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Err(ExportError::EmptyContent);
                }
                let mut out = trimmed.to_string();
                out.push('\n');
                Ok(out)
            }
            Exportable::Segments(segments) => {
                let mut out = String::new();
                for seg in presentable(segments) {
                    let lines = content_lines(seg, opts.bilingual);
                    let timecode = opts
                        .include_timestamps
                        .then(|| format_timecode(crate::shifted(seg.start, opts.time_offset_ms)));
                    let speaker = speaker_of(seg, opts);
                    for (i, line) in lines.iter().enumerate() {
                        match &timecode {
                            Some(tc) if i == 0 => {
                                out.push_str(tc);
                                out.push(' ');
                            }
                            Some(tc) => {
                                // 译文行与首行文本左对齐（时间码宽度 + 空格）。
                                for _ in 0..=tc.chars().count() {
                                    out.push(' ');
                                }
                            }
                            None => {}
                        }
                        // 说话人前缀（PRD-04-054）：首行 `名字: 文本`（显示名已预解析）。
                        if i == 0 {
                            if let Some(name) = speaker {
                                out.push_str(name);
                                out.push_str(": ");
                            }
                        }
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                if out.is_empty() {
                    return Err(ExportError::EmptyContent);
                }
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::seg;
    use crate::Bilingual;

    #[test]
    fn plain_segments_render_text_only() {
        let content =
            Exportable::Segments(vec![seg(0, 1000, "a", None), seg(1000, 2000, "b", None)]);
        let out = TxtExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert_eq!(out, "a\nb\n");
    }

    #[test]
    fn timestamped_lines_use_vault_timecode() {
        let content = Exportable::Segments(vec![seg(12_480, 13_000, "大家好", None)]);
        let opts = ExportOptions {
            include_timestamps: true,
            ..ExportOptions::default()
        };
        let out = TxtExporter.export(&content, &opts).unwrap();
        assert_eq!(out, "[00:00:12.480] 大家好\n");
    }

    #[test]
    fn target_only_falls_back_to_source_when_missing() {
        let content = Exportable::Segments(vec![
            seg(0, 1000, "你好", Some("Hello")),
            seg(1000, 2000, "没有译文", None),
        ]);
        let opts = ExportOptions {
            bilingual: Bilingual::TargetOnly,
            ..ExportOptions::default()
        };
        let out = TxtExporter.export(&content, &opts).unwrap();
        assert_eq!(out, "Hello\n没有译文\n");
    }

    #[test]
    fn bilingual_timestamped_aligns_translation_line() {
        let content = Exportable::Segments(vec![seg(0, 1000, "你好。", Some("Hello."))]);
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            include_timestamps: true,
            ..ExportOptions::default()
        };
        let out = TxtExporter.export(&content, &opts).unwrap();
        assert_eq!(out, "[00:00:00.000] 你好。\n               Hello.\n");
    }

    #[test]
    fn speaker_prefix_renders_after_timecode() {
        let mut s = seg(0, 1000, "开始吧", None);
        s.speaker = Some("张三".into());
        let opts = ExportOptions {
            include_timestamps: true,
            speaker_prefix: true,
            ..ExportOptions::default()
        };
        let out = TxtExporter
            .export(&Exportable::Segments(vec![s]), &opts)
            .unwrap();
        assert_eq!(out, "[00:00:00.000] 张三: 开始吧\n");
    }

    #[test]
    fn text_passthrough_trims_and_newline_terminates() {
        let out = TxtExporter
            .export(
                &Exportable::Text("  正文内容  ".into()),
                &ExportOptions::default(),
            )
            .unwrap();
        assert_eq!(out, "正文内容\n");
    }

    #[test]
    fn empty_inputs_are_rejected() {
        let opts = ExportOptions::default();
        assert!(matches!(
            TxtExporter.export(&Exportable::Text("   ".into()), &opts),
            Err(ExportError::EmptyContent)
        ));
        assert!(matches!(
            TxtExporter.export(&Exportable::Segments(vec![seg(0, 1, "  ", None)]), &opts),
            Err(ExportError::EmptyContent)
        ));
    }
}
