//! Provider 能力声明（用于路由匹配）。事实源：TDD-01 §2.1。

use serde::{Deserialize, Serialize};

/// Provider 能做的事；Router 据此把 Job 按能力匹配到 Provider（而非硬编码
/// "翻译就用 X"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// 语音 → 文本（ASR）。
    Transcribe,
    /// 文本 → 文本（跨语言）。
    Translate,
    /// 文本总结。
    Summarize,
    /// 文本优化（语音输入后的"自动优化"、框选优化）。
    Refine,
    /// 多轮对话。
    Chat,
    /// 图像 → 文本（OCR；本地 Vision/PaddleOCR 或云 OCR API，同走 Provider 主干，ADR-001）。
    Ocr,
    /// 说话人细分（diarization：音频 → 说话人时间区间；本地 sherpa-onnx 管线，
    /// ADR-020 / TDD-03 §10，同走 Provider 主干）。
    Diarize,
    /// 文本 → 向量（嵌入；随包引擎的嵌入端点，ADR-027 / TDD-06 §3.5，同走 Provider 主干）。
    ///
    /// 与其余能力的一处不同：它**没有面向用户的直接入口**——用户从不"点一下做嵌入"，
    /// 向量只是让检索变好的派生数据。故它的缺席不是"某个按钮不能用"，而是检索静默降级为
    /// 纯关键词（PRD-16-029）。
    Embed,
    /// 模板 / 任意提示词驱动的文本任务。
    Custom,
}

impl Capability {
    /// 全部能力（能力清单/路由匹配的单一来源，DRY——前端能力清单亦由此派生）。
    pub const ALL: [Capability; 9] = [
        Self::Transcribe,
        Self::Translate,
        Self::Summarize,
        Self::Refine,
        Self::Chat,
        Self::Ocr,
        Self::Diarize,
        Self::Embed,
        Self::Custom,
    ];

    /// 稳定 snake_case 名（与 serde 表示一致；不随文案变）。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Transcribe => "transcribe",
            Self::Translate => "translate",
            Self::Summarize => "summarize",
            Self::Refine => "refine",
            Self::Chat => "chat",
            Self::Ocr => "ocr",
            Self::Diarize => "diarize",
            Self::Embed => "embed",
            Self::Custom => "custom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&Capability::Transcribe).unwrap(),
            "\"transcribe\""
        );
        let back: Capability = serde_json::from_str("\"refine\"").unwrap();
        assert_eq!(back, Capability::Refine);
    }

    #[test]
    fn name_matches_serde_repr_for_all() {
        for c in Capability::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.name()));
        }
    }

    /// 变体总数：与下方 `assert_known` 的穷尽 match 同步维护——新增变体时那个
    /// match 编译失败（E0004）会把改动者带到这里。
    const VARIANT_COUNT: usize = 9;

    /// 穷尽 match（刻意无通配臂）：新增 `Capability` 变体而未同步能力清单时，
    /// 此处编译失败，倒逼更新 `VARIANT_COUNT` 与 `Capability::ALL`。
    fn assert_known(c: Capability) {
        match c {
            Capability::Transcribe
            | Capability::Translate
            | Capability::Summarize
            | Capability::Refine
            | Capability::Chat
            | Capability::Ocr
            | Capability::Diarize
            | Capability::Embed
            | Capability::Custom => {}
        }
    }

    /// `ALL` 必须恰好收录每个变体一次：长度等于变体总数 + 两两不同 ⇒ 满射（鸽笼）。
    /// `ALL` 是前端能力清单与路由匹配的单一来源，漏收会让该能力在清单里静默消失
    /// 而所有门禁照绿（Diarize 曾因此缺席）。
    #[test]
    fn all_lists_every_variant_exactly_once() {
        assert_eq!(
            Capability::ALL.len(),
            VARIANT_COUNT,
            "Capability::ALL 与变体总数不一致"
        );
        for (i, a) in Capability::ALL.iter().enumerate() {
            assert_known(*a);
            for b in &Capability::ALL[i + 1..] {
                assert_ne!(a, b, "Capability::ALL 重复收录 {a:?}");
            }
        }
    }
}
