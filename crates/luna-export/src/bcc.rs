//! B 站 CC 字幕导出器（PRD-11-014 / PRD-19-020，ADR-162 ⑪）。
//!
//! 只生成文件，由用户自行上传，不接 B 站账户接口。结构按通行的 `.bcc` JSON：样式头 +
//! `body: [{from, to, location, content}]`，时间以秒计；与 SRT 共用同一份重切分 cue，
//! 双语时 `content` 以换行连接两行。
//! > 假设：B 站没有公开 CC 字幕格式规范，上传页接受的就是这种结构；结构若变只影响这一个导出器。

use crate::{
    cue_lines, cue_speaker, subtitle_constraints, subtitle_cues, ExportError, ExportFormat,
    ExportOptions, Exportable, Exporter,
};
use serde::Serialize;
use std::time::Duration;

/// B 站 CC 字幕导出器（仅接受 [`Exportable::Segments`]）。
pub struct BccExporter;

#[derive(Serialize)]
struct BccFile {
    font_size: f64,
    font_color: &'static str,
    background_alpha: f64,
    background_color: &'static str,
    #[serde(rename = "Stroke")]
    stroke: &'static str,
    body: Vec<BccLine>,
}

#[derive(Serialize)]
struct BccLine {
    from: f64,
    to: f64,
    /// 2 = 底部居中（B 站播放器的默认位置）。
    location: u8,
    content: String,
}

/// 秒，保留到毫秒。
fn seconds(d: Duration) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let ms = d.as_millis() as f64;
    ms / 1000.0
}

impl Exporter for BccExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Bcc
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        let Exportable::Segments(segments) = content else {
            return Err(ExportError::MismatchedInput(
                "BCC 需要带时间戳的 Segments 输入",
            ));
        };
        let cues = subtitle_cues(segments, &subtitle_constraints(segments, opts), opts);
        if cues.is_empty() {
            return Err(ExportError::EmptyContent);
        }
        let body = cues
            .iter()
            .map(|cue| {
                let mut text = cue_lines(cue, opts.bilingual).join("\n");
                if let Some(name) = cue_speaker(cue, opts) {
                    text = format!("{name}: {text}");
                }
                BccLine {
                    from: seconds(cue.start),
                    to: seconds(cue.end),
                    location: 2,
                    content: text,
                }
            })
            .collect();
        let file = BccFile {
            font_size: 0.4,
            font_color: "#FFFFFF",
            background_alpha: 0.5,
            background_color: "#9C27B0",
            stroke: "none",
            body,
        };
        serde_json::to_string_pretty(&file).map_err(|_| ExportError::EmptyContent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::seg;
    use crate::{Bilingual, ExportRegistry};

    #[test]
    fn bcc_is_json_in_seconds_with_both_lines_when_bilingual() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![
            seg(0, 2500, "大家好", Some("Hello everyone")),
            // 中间隔开，免得两条短句被重切分合并成一条。
            seg(6000, 8250, "今天聊脚本", None),
        ]);
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            utf8_bom: true,
            ..ExportOptions::default()
        };
        let out = registry.export(ExportFormat::Bcc, &content, &opts).unwrap();
        assert!(!out.starts_with('\u{FEFF}'), "BCC 是 JSON，不加 BOM");
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        let body = json["body"].as_array().unwrap();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0]["from"], 0.0);
        assert_eq!(body[0]["to"], 2.5);
        assert_eq!(body[0]["content"], "大家好\nHello everyone");
        assert_eq!(body[1]["from"], 6.0);
        assert_eq!(body[1]["to"], 8.25);
        assert_eq!(body[1]["content"], "今天聊脚本");
        assert_eq!(body[0]["location"], 2);
        assert_eq!(json["Stroke"], "none");
        assert_eq!(ExportFormat::Bcc.extension(), "bcc");
    }

    #[test]
    fn bcc_rejects_plain_text() {
        let err = BccExporter
            .export(&Exportable::Text("x".into()), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::MismatchedInput(_)));
    }
}
