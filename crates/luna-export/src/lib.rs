//! `luna-export` — 导出管线（TDD-10，对应 PRD-11 导出）。
//!
//! **注册式**：[`ExportRegistry`] 按 [`ExportFormat`] 取 [`Exporter`]，新增格式只加一个
//! 实现（开放扩展）。导出器是**纯函数**（`Exportable → String`），零 IO——写盘/选目录
//! 一律在调用方（Tauri 命令 / CLI），GUI 与 CLI 共用（TDD-10 §4）。
//!
//! **统一输入**（TDD-10 §1）：[`Exportable::Segments`]（有时间戳 → 字幕类）/
//! [`Exportable::Text`]（无时间戳 → 文本类）。导出器不感知内容来源（弹窗翻译/录音/
//! 批量/二次处理），一套导出器服务所有来源（DRY）。
//!
//! > 实现状态（2026-07-16，批次 5）：注册管线 + txt/SRT/VTT/ASS/LRC。字幕类输入统一
//! > 先过 `Segments → Cues` 重切分（[`subtitle_cues`]，TDD-10 §2；跨说话人不合并 +
//! > 双语译文全保留由 `luna_core::cue` 保证）；txt 保持 segment 1:1（行首时间标记的
//! > 语义是「段」非「字幕」）。**时移 / BOM / 折行覆盖已落地（2026-09-06，PRD-11-004/007）**：
//! > 时移作用于全部时间戳（含 txt 行首时间标记），下限钳在 0；BOM 只加在 SRT/VTT；折行
//! > 覆盖只改行长与行数，CPS 与时长约束仍按语系档。

#![forbid(unsafe_code)]

use luna_core::{Cue, CueConstraints, Segment};
use std::collections::HashMap;

mod ass;
mod bcc;
mod han;
mod lrc;
mod srt;
mod txt;
mod vtt;

pub use ass::AssExporter;
pub use bcc::BccExporter;
pub use han::to_simplified;
pub use lrc::LrcExporter;
pub use srt::SrtExporter;
pub use txt::TxtExporter;
pub use vtt::VttExporter;

// ─── 统一输入与选项 ───────────────────────────────────────────────────────────

/// 导出格式（TDD-10 §2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    /// 纯文本（可选行首时间标记）。
    Txt,
    /// SubRip 字幕（序号 + `HH:MM:SS,mmm --> ...`，逗号毫秒）。
    Srt,
    /// WebVTT 字幕（`WEBVTT` 头 + 点毫秒）。
    Vtt,
    /// Advanced SubStation Alpha（最小样式头 + `Dialogue:` 行，厘秒）。
    Ass,
    /// LRC 歌词（`[mm:ss.xx]` 行；无显式 end——下一行即隐式终点，格式语义如实）。
    Lrc,
    /// B 站 CC 字幕（JSON，秒为单位；PRD-11-014）。
    Bcc,
}

impl ExportFormat {
    /// 目标文件扩展名。
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Ass => "ass",
            Self::Lrc => "lrc",
            Self::Bcc => "bcc",
        }
    }
}

/// 双语导出三态（TDD-10 §3；与回放视图显示三态同源，PRD-04-025/PRD-11-003）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bilingual {
    /// 仅源文。
    #[default]
    SourceOnly,
    /// 仅译文（缺译回退源文，与回放视图一致）。
    TargetOnly,
    /// 双语对照（源上译下；缺译只出源行）。
    Both,
}

/// 中文字形变体（PRD-11-013）：导出前做确定性词典转换，不经模型改写。非汉字原样保留，
/// 所以源文与译文整份施加，不必分辨哪一轨是中文。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HanVariant {
    /// 不转换。
    #[default]
    Keep,
    /// 转成简体。
    Simplified,
    /// 转成繁体（台湾标准字形；只换字、不换词）。
    TraditionalTaiwan,
    /// 转成繁体（香港用字）。
    TraditionalHongKong,
}

/// 导出选项（TDD-10 §3）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ExportOptions {
    /// 双语三态。
    pub bilingual: Bilingual,
    /// txt 专用：行首输出 `[HH:MM:SS.mmm]` 时间标记（与 Vault 正文时间码同构，
    /// TDD-06 §2.2）；字幕类格式忽略此项（时间戳是格式本体）。
    pub include_timestamps: bool,
    /// 说话人前缀（PRD-04-054 / TDD-03 §10.3）：段带 `speaker` 时首行输出
    /// `名字: 文本`。调用方须先把 speaker id（`self`/`remote:N`…）解析为**显示名**
    /// （改名映射优先、默认轨标签兜底）再喂导出——导出器不感知映射来源（DRY）。
    pub speaker_prefix: bool,
    /// 整体时移（毫秒，可负；PRD-11-004）：作用于全部时间戳，含 txt 行首时间标记；
    /// 结果下限钳在 0（字幕格式没有负时间）。
    pub time_offset_ms: i64,
    /// SRT / VTT 输出 UTF-8 BOM（PRD-11-007，部分老旧播放器需要它才认中文）；其余格式忽略。
    pub utf8_bom: bool,
    /// 折行覆盖：每行最大字符数（`None` = 按语系档自动；`Some(0)` = 不限）。
    pub max_line_len: Option<usize>,
    /// 折行覆盖：每条最大行数（`None` = 按语系档，即 2）。LRC 恒为 1，不受此项影响。
    pub max_lines: Option<u8>,
    /// 中文字形转换（PRD-11-013）。在重切分**之前**施加：词组转换会改变字数（「内存」→
    /// 「記憶體」），断行与读速要按最终的文字算。
    pub han_variant: HanVariant,
}

/// 时移（钳在 0）。
fn shifted(d: std::time::Duration, offset_ms: i64) -> std::time::Duration {
    let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    std::time::Duration::from_millis(u64::try_from(ms.saturating_add(offset_ms)).unwrap_or(0))
}

/// 字幕类的重切分约束：语系档为底，叠加用户的折行覆盖（PRD-11-004）。
fn subtitle_constraints(segments: &[Segment], opts: &ExportOptions) -> CueConstraints {
    let mut constraints = CueConstraints::for_segments(segments);
    if let Some(len) = opts.max_line_len {
        constraints.max_line_len = len;
    }
    if let Some(lines) = opts.max_lines {
        constraints.max_lines = lines.max(1);
    }
    constraints
}

/// 统一导出输入（TDD-10 §1）。
#[derive(Clone, Debug)]
pub enum Exportable {
    /// 有时间戳的转写/转译段（字幕类 + 带时间标记文本）。
    Segments(Vec<Segment>),
    /// 无时间戳纯文本（笔记正文等）。
    Text(String),
}

/// 导出错误。
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// 该格式未注册。
    #[error("export format not registered: {0:?}")]
    UnsupportedFormat(ExportFormat),
    /// 输入与格式不匹配（如向字幕格式喂纯文本）。
    #[error("export input does not match the format: {0}")]
    MismatchedInput(&'static str),
    /// 无可导出内容（空段列表/空文本）。
    #[error("nothing to export")]
    EmptyContent,
    /// 字形转换词典装载失败（词典随 crate 内嵌，正常不会发生）。
    #[error("Chinese script conversion unavailable: {0}")]
    HanConversion(String),
}

// ─── Exporter trait 与注册表 ──────────────────────────────────────────────────

/// 单一格式导出器（纯函数，零 IO；TDD-10 §1）。
pub trait Exporter {
    /// 本导出器负责的格式。
    fn format(&self) -> ExportFormat;
    /// 渲染导出内容为目标格式字符串。
    ///
    /// # Errors
    /// 输入与格式不匹配或内容为空时返回 [`ExportError`]。
    fn export(&self, content: &Exportable, opts: &ExportOptions) -> Result<String, ExportError>;
}

/// 导出注册表：按格式分发到对应导出器（开放扩展，TDD-10 §1）。
pub struct ExportRegistry {
    exporters: HashMap<ExportFormat, Box<dyn Exporter + Send + Sync>>,
}

impl ExportRegistry {
    /// 空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self {
            exporters: HashMap::new(),
        }
    }

    /// 含全部内置导出器（txt + SRT/VTT/ASS/LRC）的注册表。
    #[must_use]
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(TxtExporter));
        registry.register(Box::new(SrtExporter));
        registry.register(Box::new(VttExporter));
        registry.register(Box::new(AssExporter));
        registry.register(Box::new(LrcExporter));
        registry.register(Box::new(BccExporter));
        registry
    }

    /// 注册一个导出器（同格式后注册者覆盖）。
    pub fn register(&mut self, exporter: Box<dyn Exporter + Send + Sync>) {
        self.exporters.insert(exporter.format(), exporter);
    }

    /// 按格式导出。
    ///
    /// # Errors
    /// 格式未注册或导出器报错时返回 [`ExportError`]。
    pub fn export(
        &self,
        format: ExportFormat,
        content: &Exportable,
        opts: &ExportOptions,
    ) -> Result<String, ExportError> {
        let exporter = self
            .exporters
            .get(&format)
            .ok_or(ExportError::UnsupportedFormat(format))?;
        // 字形转换在这一处统一做（与 BOM 同理），格式实现不感知；先转后切（见 `han_variant`）。
        let converted = han::convert_content(content, opts.han_variant)?;
        let rendered = exporter.export(converted.as_ref().unwrap_or(content), opts)?;
        // BOM 只在 SRT/VTT 上有意义（PRD-11-007）；在这一处统一加，格式实现不各自记。
        if opts.utf8_bom && matches!(format, ExportFormat::Srt | ExportFormat::Vtt) {
            return Ok(format!("\u{FEFF}{rendered}"));
        }
        Ok(rendered)
    }
}

impl Default for ExportRegistry {
    fn default() -> Self {
        Self::with_builtin()
    }
}

// ─── 共享辅助（txt/SRT 共用，DRY）────────────────────────────────────────────

/// 段 → 文本行（双语三态；`Both` 返回 1–2 行）。缺译回退语义与回放视图一致。
fn content_lines(seg: &Segment, bilingual: Bilingual) -> Vec<&str> {
    let translation = seg
        .translation
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    match bilingual {
        Bilingual::SourceOnly => vec![seg.text.as_str()],
        Bilingual::TargetOnly => vec![translation.unwrap_or(seg.text.as_str())],
        Bilingual::Both => match translation {
            Some(t) => vec![seg.text.as_str(), t],
            None => vec![seg.text.as_str()],
        },
    }
}

/// 过滤出非空白文本段（空白段不进任何导出产物）。
fn presentable(segments: &[Segment]) -> impl Iterator<Item = &Segment> {
    segments.iter().filter(|s| !s.text.trim().is_empty())
}

/// 段的说话人前缀显示名（选项开启且段带非空 `speaker` 时；txt/SRT 共用，DRY）。
fn speaker_of<'a>(seg: &'a Segment, opts: &ExportOptions) -> Option<&'a str> {
    if !opts.speaker_prefix {
        return None;
    }
    seg.speaker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

// ─── 字幕类共享：重切分 + cue 行渲染（SRT/VTT/ASS/LRC 共用，DRY）────────────────

/// 字幕类导出的重切分前处理（TDD-10 §2）：非空段 → `segments_to_cues` → 时移。
/// 约束**按整篇语系选档**（`CueConstraints::for_segments`：拉丁 42 字/行 ×2 行 CPS 20 /
/// CJK 20 字/行 ×2 行 CPS 9，Netflix TTSG 口径；LRC 用行式约束见 [`lrc`]）——用拉丁档量
/// 中文等于默认"一行 42 个汉字"，那是没人读得完的字幕。
/// 说话人边界即切 + 双语译文合并保留由 `luna_core::cue` 保证。时移在切分**之后**做：
/// 切分看的是相对时长与读速，与绝对起点无关。
fn subtitle_cues(
    segments: &[Segment],
    constraints: &CueConstraints,
    opts: &ExportOptions,
) -> Vec<Cue> {
    let presentable: Vec<Segment> = presentable(segments).cloned().collect();
    let mut cues = luna_core::segments_to_cues(&presentable, constraints);
    if opts.time_offset_ms != 0 {
        for cue in &mut cues {
            cue.start = shifted(cue.start, opts.time_offset_ms);
            cue.end = shifted(cue.end, opts.time_offset_ms);
        }
    }
    cues
}

/// cue → 文本行（双语三态；`Both` = 源行在前译行在后，缺译只出源行；与段级
/// [`content_lines`] 同语义）。cue 正文已折行（`\n`），此处按行拆开供格式各自拼装。
fn cue_lines(cue: &Cue, bilingual: Bilingual) -> Vec<String> {
    let translation = cue
        .translation
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let source_lines = || cue.text.lines().map(str::to_string).collect::<Vec<_>>();
    match bilingual {
        Bilingual::SourceOnly => source_lines(),
        Bilingual::TargetOnly => match translation {
            Some(t) => vec![t.to_string()],
            None => source_lines(),
        },
        Bilingual::Both => {
            let mut lines = source_lines();
            if let Some(t) = translation {
                lines.push(t.to_string());
            }
            lines
        }
    }
}

/// cue 的说话人前缀显示名（选项开启且带非空 `speaker` 时）。
fn cue_speaker<'a>(cue: &'a Cue, opts: &ExportOptions) -> Option<&'a str> {
    if !opts.speaker_prefix {
        return None;
    }
    cue.speaker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::SegmentId;
    use std::time::Duration;

    pub(crate) fn seg(
        start_ms: u64,
        end_ms: u64,
        text: &str,
        translation: Option<&str>,
    ) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_string(),
            translation: translation.map(str::to_string),
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    #[test]
    fn registry_dispatches_by_format() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![seg(0, 1000, "hello", None)]);
        let opts = ExportOptions::default();
        assert!(registry.export(ExportFormat::Txt, &content, &opts).is_ok());
        assert!(registry.export(ExportFormat::Srt, &content, &opts).is_ok());
    }

    #[test]
    fn empty_registry_reports_unsupported() {
        let registry = ExportRegistry::new();
        let content = Exportable::Text("x".into());
        let err = registry
            .export(ExportFormat::Txt, &content, &ExportOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExportError::UnsupportedFormat(_)));
    }

    #[test]
    fn time_offset_shifts_every_timestamp_and_clamps_at_zero() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![seg(500, 2000, "hello world", None)]);
        let plus = ExportOptions {
            time_offset_ms: 1500,
            ..ExportOptions::default()
        };
        let srt = registry.export(ExportFormat::Srt, &content, &plus).unwrap();
        assert!(srt.contains("00:00:02,000 --> 00:00:03,500"), "{srt}");
        let minus = ExportOptions {
            time_offset_ms: -1000,
            ..ExportOptions::default()
        };
        let srt = registry
            .export(ExportFormat::Srt, &content, &minus)
            .unwrap();
        assert!(
            srt.contains("00:00:00,000 --> 00:00:01,000"),
            "钳在 0：{srt}"
        );
        let txt = registry
            .export(
                ExportFormat::Txt,
                &content,
                &ExportOptions {
                    include_timestamps: true,
                    time_offset_ms: 1500,
                    ..ExportOptions::default()
                },
            )
            .unwrap();
        assert!(txt.starts_with("[00:00:02.000]"), "{txt}");
    }

    #[test]
    fn bom_only_for_srt_and_vtt() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![seg(0, 1000, "hi", None)]);
        let opts = ExportOptions {
            utf8_bom: true,
            ..ExportOptions::default()
        };
        assert!(registry
            .export(ExportFormat::Srt, &content, &opts)
            .unwrap()
            .starts_with('\u{FEFF}'));
        assert!(registry
            .export(ExportFormat::Vtt, &content, &opts)
            .unwrap()
            .starts_with('\u{FEFF}'));
        assert!(!registry
            .export(ExportFormat::Ass, &content, &opts)
            .unwrap()
            .starts_with('\u{FEFF}'));
        assert!(!registry
            .export(ExportFormat::Txt, &content, &opts)
            .unwrap()
            .starts_with('\u{FEFF}'));
    }

    #[test]
    fn line_length_override_rewraps_cues() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![seg(0, 4000, "one two three four five six", None)]);
        let narrow = ExportOptions {
            max_line_len: Some(10),
            max_lines: Some(2),
            ..ExportOptions::default()
        };
        let srt = registry
            .export(ExportFormat::Srt, &content, &narrow)
            .unwrap();
        let longest = srt
            .lines()
            .filter(|l| !l.contains("-->") && l.parse::<u32>().is_err() && !l.is_empty())
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        // 容量拆分后每行不超过覆盖值（兜底行允许超宽，但这段短语拆得开）。
        assert!(longest <= 10, "{srt}");
    }

    #[test]
    fn han_variant_converts_source_and_translation_before_cue_splitting() {
        let registry = ExportRegistry::with_builtin();
        let content = Exportable::Segments(vec![seg(
            0,
            3000,
            "开放中文转换",
            Some("Open Chinese Convert"),
        )]);
        let both = ExportOptions {
            bilingual: Bilingual::Both,
            han_variant: HanVariant::TraditionalTaiwan,
            ..ExportOptions::default()
        };
        let srt = registry.export(ExportFormat::Srt, &content, &both).unwrap();
        assert!(srt.contains("開放中文轉換"), "{srt}");
        assert!(srt.contains("Open Chinese Convert"), "非汉字原样：{srt}");
        let back = Exportable::Segments(vec![seg(0, 3000, "開放中文轉換", None)]);
        let simplified = ExportOptions {
            han_variant: HanVariant::Simplified,
            ..ExportOptions::default()
        };
        let txt = registry
            .export(ExportFormat::Txt, &back, &simplified)
            .unwrap();
        assert!(txt.contains("开放中文转换"), "{txt}");
        // 只换字形、不换词：「脚本」不能变成 IT 语境的「指令碼」（真机 2026-10-17）。
        let script = Exportable::Segments(vec![seg(0, 3000, "视频脚本", None)]);
        let taiwan = ExportOptions {
            han_variant: HanVariant::TraditionalTaiwan,
            ..ExportOptions::default()
        };
        let txt = registry
            .export(ExportFormat::Txt, &script, &taiwan)
            .unwrap();
        assert!(txt.contains("視頻腳本"), "{txt}");
        // 不转换就原样。
        let keep = registry
            .export(ExportFormat::Txt, &back, &ExportOptions::default())
            .unwrap();
        assert!(keep.contains("開放中文轉換"), "{keep}");
    }

    #[test]
    fn extension_matches_format() {
        assert_eq!(ExportFormat::Txt.extension(), "txt");
        assert_eq!(ExportFormat::Srt.extension(), "srt");
    }
}
