//! ASS（Advanced SubStation Alpha）导出器（TDD-10 §2：最小样式头 + `Dialogue:` 行）。
//! 输入先过 Segments → Cues 重切分（`subtitle_cues`，格式默认约束）。
//!
//! 格式语义如实实现（TDD-10 §2 纪律）：
//! - 时间戳 `H:MM:SS.cc`（厘秒，ASS 规范精度）；
//! - `Dialogue:` 的 `Text` 是**末字段**——正文内逗号合法无需转义；花括号会被解析为
//!   override 标签，转义为 `\{`/`\}`；cue 内换行用 `\N`；
//! - 说话人进 `Name` 字段（ASS 原生字段），开启前缀选项时正文再带 `名字: `（与其他
//!   格式一致的可读回退——多数播放器不显示 Name 字段）。

use crate::{
    cue_lines, cue_speaker, subtitle_constraints, subtitle_cues, ExportError, ExportFormat,
    ExportOptions, Exportable, Exporter,
};
use std::fmt::Write as _;
use std::time::Duration;

/// ASS 字幕导出器（仅接受 [`Exportable::Segments`]；双语 = 同一 Dialogue 内 `\N` 两行）。
pub struct AssExporter;

/// 最小可用样式头（PlayRes 与字号为通行默认；样式定制列后续选项，禁硬编码到多处）。
const ASS_HEADER: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1280\nPlayResY: 720\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,20,20,20,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

/// ASS 时间戳（`H:MM:SS.cc`，厘秒）。
fn ass_timestamp(d: Duration) -> String {
    let total_cs = d.as_millis() as u64 / 10;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h}:{m:02}:{s:02}.{cs:02}")
}

/// Text 字段转义：花括号是 override 标签定界符（`\{`/`\}` 出字面量）；换行转 `\N`。
/// 逗号在末字段合法，不转义（如实实现，不造假字段）。
fn ass_escape(text: &str) -> String {
    text.replace('{', "\\{").replace('}', "\\}")
}

impl Exporter for AssExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Ass
    }

    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError> {
        let Exportable::Segments(segments) = content else {
            return Err(ExportError::MismatchedInput(
                "ASS 需要带时间戳的 Segments 输入",
            ));
        };
        let cues = subtitle_cues(segments, &subtitle_constraints(segments, opts), opts);
        if cues.is_empty() {
            return Err(ExportError::EmptyContent);
        }
        let mut out = String::from(ASS_HEADER);
        for cue in &cues {
            let speaker = cue_speaker(cue, opts);
            let name = speaker.unwrap_or("");
            let mut lines = cue_lines(cue, opts.bilingual);
            if let (Some(name), Some(first)) = (speaker, lines.first_mut()) {
                *first = format!("{name}: {first}");
            }
            let text = ass_escape(&lines.join("\\N"));
            let _ = writeln!(
                out,
                "Dialogue: 0,{},{},Default,{},0,0,0,,{}",
                ass_timestamp(cue.start),
                ass_timestamp(cue.end),
                name,
                text
            );
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
    fn dialogue_line_with_centisecond_timestamps() {
        let content = Exportable::Segments(vec![seg(0, 1_500, "hello, world", None)]);
        let out = AssExporter
            .export(&content, &ExportOptions::default())
            .unwrap();
        assert!(out.starts_with("[Script Info]"));
        assert!(
            out.contains("Dialogue: 0,0:00:00.00,0:00:01.50,Default,,0,0,0,,hello, world\n"),
            "正文逗号在末字段合法保留：{out}"
        );
    }

    #[test]
    fn braces_are_escaped_and_bilingual_joined_with_hard_break() {
        let content = Exportable::Segments(vec![seg(0, 2_000, "a {tag} b", Some("译文"))]);
        let opts = ExportOptions {
            bilingual: Bilingual::Both,
            ..ExportOptions::default()
        };
        let out = AssExporter.export(&content, &opts).unwrap();
        assert!(
            out.contains(",,a \\{tag\\} b\\N译文\n"),
            "花括号转义 + \\N 双语换行：{out}"
        );
    }

    #[test]
    fn speaker_goes_to_name_field_and_prefix() {
        let mut s = seg(0, 1_000, "你好", None);
        s.speaker = Some("张三".into());
        let opts = ExportOptions {
            speaker_prefix: true,
            ..ExportOptions::default()
        };
        let out = AssExporter
            .export(&Exportable::Segments(vec![s]), &opts)
            .unwrap();
        assert!(
            out.contains("Default,张三,0,0,0,,张三: 你好\n"),
            "Name 字段 + 正文前缀双写：{out}"
        );
    }

    #[test]
    fn empty_segments_are_rejected() {
        let err = AssExporter
            .export(&Exportable::Segments(vec![]), &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::EmptyContent));
    }
}
