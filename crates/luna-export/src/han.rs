//! 中文字形转换（PRD-11-013，ADR-162 ⑪）：确定性词典转换（OpenCC 词典），不经模型。
//!
//! 每种变体的转换器装载一次后常驻（词典随 crate 内嵌，装载是纯 CPU 的一次性开销）。

use crate::{ExportError, Exportable, HanVariant};
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use std::sync::OnceLock;

fn converter(variant: HanVariant) -> Result<Option<&'static OpenCC>, ExportError> {
    static SIMPLIFIED: OnceLock<Result<OpenCC, String>> = OnceLock::new();
    static TAIWAN: OnceLock<Result<OpenCC, String>> = OnceLock::new();
    static HONG_KONG: OnceLock<Result<OpenCC, String>> = OnceLock::new();
    let (cell, config) = match variant {
        HanVariant::Keep => return Ok(None),
        HanVariant::Simplified => (&SIMPLIFIED, BuiltinConfig::T2s),
        // 台湾：只换成台湾标准字形，不换词。`s2twp` 的词组表按 IT 语境把「脚本」换成「指令碼」
        // （真机 2026-10-17：视频脚本成了「影片指令碼」）——换词是改写，会改变原意（PRD-19-019）。
        HanVariant::TraditionalTaiwan => (&TAIWAN, BuiltinConfig::S2tw),
        HanVariant::TraditionalHongKong => (&HONG_KONG, BuiltinConfig::S2hk),
    };
    cell.get_or_init(|| OpenCC::from_config(config).map_err(|e| e.to_string()))
        .as_ref()
        .map(Some)
        .map_err(|e| ExportError::HanConversion(e.clone()))
}

/// 转成简体（词典装载失败时原样返回）。提词器跟读用它把脚本与识别文字统一成同一种字形：
/// Whisper 识别普通话时常出繁体（真机 2026-10-17），简体脚本因此对不上。
#[must_use]
pub fn to_simplified(text: &str) -> String {
    match converter(HanVariant::Simplified) {
        Ok(Some(cc)) => cc.convert(text),
        _ => text.to_owned(),
    }
}

/// 按变体转换整份导出内容；`Keep` 返回 `None`（调用方用原件，免一次克隆）。
pub(crate) fn convert_content(
    content: &Exportable,
    variant: HanVariant,
) -> Result<Option<Exportable>, ExportError> {
    let Some(cc) = converter(variant)? else {
        return Ok(None);
    };
    Ok(Some(match content {
        Exportable::Text(text) => Exportable::Text(cc.convert(text)),
        Exportable::Segments(segments) => Exportable::Segments(
            segments
                .iter()
                .map(|segment| {
                    let mut segment = segment.clone();
                    segment.text = cc.convert(&segment.text);
                    segment.translation = segment.translation.as_deref().map(|t| cc.convert(t));
                    segment
                })
                .collect(),
        ),
    }))
}
