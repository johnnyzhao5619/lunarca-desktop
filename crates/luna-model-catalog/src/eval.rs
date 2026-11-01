//! 目录条目的评测证据（ADR-145 ⑤ B1）：评测集定义、报告形状与达标判定的**唯一**定义。
//!
//! 运营在本机用 `luna model-eval` 跑出报告（随包推理引擎、应用真实的提示词合成），上传到
//! 管理台；服务端用本模块的 [`verdict`] **重算**是否达标，不信报告里自报的结论。评测集、
//! 用例清单与阈值只在这里写一份：CLI 照它跑、服务端照它判，两边不可能各说各的。
//!
//! 零 IO：本模块只有数据与纯函数；跑模型的是 CLI。
//!
//! 阈值来源（2026-10-07 用户裁决：用 P0 实测基线）：
//! - 嵌入替换同一 profile：与参照模型逐条余弦 ≥ [`EMBED_EQUIVALENCE_MIN`]（P0-3 / 部署机实测
//!   bge-m3 Q8_0 对 F16 最小 0.9997）；
//! - 嵌入小检索集：top-1 命中率不低于当前默认嵌入模型（bge-m3 Q8_0）在同一集上的实测值
//!   [`EMBED_HIT_AT_1_BASELINE`]；
//! - 对话冒烟：每条用例的判据是确定的（语种、长度、能否解析成 JSON），全过才算达标。

use serde::{Deserialize, Serialize};

use crate::{GgufRole, ModelEntry, Task};

/// 报告格式标识（不认识的格式一律不收）。
pub const REPORT_SCHEMA: &str = "lunarca.model-eval/1";

/// 嵌入替换同一 profile 时与参照模型的最低逐条余弦。
pub const EMBED_EQUIVALENCE_MIN: f64 = 0.999;
/// 嵌入向量范数离 1 的最大偏差（profile 约定 L2 归一化）。
pub const EMBED_NORM_TOLERANCE: f64 = 1e-3;
/// 语义无关的两段文本余弦的上限：超过它说明向量退化（什么都跟什么像）。
pub const EMBED_DISTINCT_MAX: f64 = 0.9;
/// 小检索集上的 top-1 命中率基线：当前默认嵌入模型（bge-m3 Q8_0）实测值。
pub const EMBED_HIT_AT_1_BASELINE: f64 = 1.0;

/// 评测集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Suite {
    /// 通用对话模型：翻译 / 摘要 / 只输出 JSON / 关思考后正文非空。
    ChatSmoke,
    /// 专用翻译模型（`translation_only`）：只考翻译那几条。
    TranslateSmoke,
    /// 嵌入模型：维度、归一化、不退化、小检索集；可选与参照模型的等价性。
    EmbedSanity,
}

impl Suite {
    /// 当前版本号（用例或判据变了就加一，旧报告随之作废）。
    #[must_use]
    pub const fn version(self) -> u32 {
        1
    }

    /// 这一条目该考哪一套。不是 GGUF 对话 / 嵌入条目时 `None`（不过这道闸）。
    #[must_use]
    pub fn for_entry(entry: &ModelEntry) -> Option<Self> {
        let gguf = entry.gguf.as_ref()?;
        match (entry.task, &gguf.role) {
            (Task::Embed, GgufRole::Embed { .. }) => Some(Self::EmbedSanity),
            (Task::Llm, GgufRole::Chat { .. }) if gguf.translation_only => {
                Some(Self::TranslateSmoke)
            }
            (Task::Llm, GgufRole::Chat { .. }) => Some(Self::ChatSmoke),
            _ => None,
        }
    }

    /// 这一套必须出现、且必须全过的对话用例 id（嵌入套为空）。
    #[must_use]
    pub fn required_cases(self) -> &'static [&'static str] {
        match self {
            Self::ChatSmoke => &[
                CASE_ZH_TO_EN,
                CASE_EN_TO_ZH,
                CASE_QUESTION_NOT_ANSWERED,
                CASE_SUMMARY_BRIEF,
                CASE_JSON_ONLY,
                CASE_NONEMPTY_REPLY,
            ],
            Self::TranslateSmoke => &[CASE_ZH_TO_EN, CASE_EN_TO_ZH, CASE_QUESTION_NOT_ANSWERED],
            Self::EmbedSanity => &[],
        }
    }
}

/// 中译英：译文非空且不含汉字。
pub const CASE_ZH_TO_EN: &str = "translate_zh_to_en";
/// 英译中：译文含汉字、拉丁字母占比低。
pub const CASE_EN_TO_ZH: &str = "translate_en_to_zh";
/// 待译文本是一句问话：要的是译文，不是回答（小模型最常见的失败形态）。
pub const CASE_QUESTION_NOT_ANSWERED: &str = "translate_question_not_answered";
/// 摘要确实压缩了原文（不是原样照抄或越写越长）。**只是冒烟**：摘要写得好不好不在这里量。
///
/// 上限 90% 取自随包默认模型的实测（2026-10-07，应用真实的「一句话」摘要提示词）：0.8B / 2B
/// 把 172 字压到 130–143 字（75–83%），4B 压到 56 字。拍一个更严的数会把正在发的默认模型
/// 挡在目录外——阈值以当前默认模型为基线（用户裁决「用 P0 实测基线」）。
pub const CASE_SUMMARY_BRIEF: &str = "summary_brief";
/// 要求只输出 JSON 时能直接解析，且答对字段。
pub const CASE_JSON_ONLY: &str = "json_only";
/// 关思考后短请求的正文非空（思考模型不关时正文为空，P0-1）。
pub const CASE_NONEMPTY_REPLY: &str = "nonempty_reply";

/// 对话用例的输入（CLI 照此构造任务；判据见 [`case_passes`]）。
pub const ZH_TO_EN_INPUT: &str = "今天天气很好，我们下午去公园散步吧。";
pub const EN_TO_ZH_INPUT: &str = "The weekly meeting has been moved to Thursday afternoon.";
pub const QUESTION_INPUT: &str = "How are you feeling today?";
pub const SUMMARY_INPUT: &str = "本周项目例会讨论了三件事。第一，新版本的发布日期定在下周三，测试由小王负责，\
    回归用例需要在周一之前补齐。第二，客户反馈的导出乱码问题已经定位，是编码检测在短文本上误判，修复已合入主干，\
    周二随热修复包发布。第三，下季度的招聘计划：后端增加两名，前端增加一名，面试从下个月开始。\
    会上还提到办公室搬迁的进度，新场地预计月底交付，具体搬迁日期另行通知。";
pub const JSON_PROMPT: &str =
    "只输出一个 JSON 对象，不要任何解释或代码块标记。格式：{\"city\": \"城市名\"}。\
    问题：中国的首都是哪个城市？";
pub const NONEMPTY_PROMPT: &str = "用一句话介绍杭州。";

/// 一条对话用例的判定（确定性：只看语种、长度与可解析性）。
#[must_use]
pub fn case_passes(case: &str, output: &str) -> bool {
    let text = output.trim();
    if text.is_empty() {
        return false;
    }
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    let latin = text.chars().filter(char::is_ascii_alphabetic).count();
    match case {
        CASE_ZH_TO_EN => cjk == 0 && latin >= 10,
        CASE_EN_TO_ZH => cjk >= 4 && latin * 4 <= cjk,
        // 「你今天感觉怎么样？」这类译文短且仍是问句；回答（「我今天感觉很好……」）不是。
        CASE_QUESTION_NOT_ANSWERED => {
            cjk >= 3
                && text.chars().count() <= 24
                && (text.contains('？') || text.contains('?') || text.contains('吗'))
        }
        CASE_SUMMARY_BRIEF => {
            cjk >= 8 && text.chars().count() * 10 <= SUMMARY_INPUT.chars().count() * 9
        }
        CASE_JSON_ONLY => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| {
                value
                    .get("city")?
                    .as_str()
                    .map(|city| city.contains("北京"))
            })
            .unwrap_or(false),
        CASE_NONEMPTY_REPLY => cjk >= 4,
        _ => false,
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// 嵌入小检索集：每条查询的正确文档是同下标那一条；中英混合，覆盖跨语言。
pub const EMBED_DOCS: &[&str] = &[
    "下周三发布新版本，回归测试由小王负责。",
    "The quarterly revenue grew by twelve percent compared to last year.",
    "量子计算利用叠加态和纠缠来并行处理信息。",
    "Remember to buy milk, eggs and bread on the way home.",
    "西湖位于杭州市区西部，是著名的风景名胜区。",
    "The patient was prescribed antibiotics for a bacterial infection.",
    "这份合同的违约金条款需要法务再审一遍。",
    "Our team will migrate the database to PostgreSQL next month.",
];
/// 与 [`EMBED_DOCS`] 一一对应的查询（刻意换说法、换语种）。
pub const EMBED_QUERIES: &[&str] = &[
    "release date and who handles regression testing",
    "公司今年的营收增长了多少",
    "how does quantum computing work",
    "回家路上要买的东西",
    "famous lake scenery in Hangzhou",
    "医生给病人开了什么药",
    "contract penalty clause legal review",
    "数据库迁移计划",
];
/// 用来测「是否退化」的两段语义无关文本。
pub const EMBED_UNRELATED: [&str; 2] = [
    "猫在窗台上晒太阳，尾巴一甩一甩的。",
    "The compiler rejected the program because of a type mismatch.",
];

/// 一份评测报告（CLI 产出、管理台上传、服务端重算）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalReport {
    /// 恒为 [`REPORT_SCHEMA`]。
    pub schema: String,
    /// 目录条目 id。
    pub catalog_id: String,
    /// 被测权重文件的 sha256（CLI 现算，必须与条目一致）。
    pub sha256: String,
    pub suite: Suite,
    pub suite_version: u32,
    /// 引擎构建号（llama.cpp `b<N>`）。
    pub engine_build: u32,
    /// 产出工具与版本（如 `luna 1.1.0`）。
    pub tool: String,
    /// RFC 3339。
    pub produced_at: String,
    /// 对话用例逐条结果（嵌入套为空）。
    #[serde(default)]
    pub cases: Vec<CaseResult>,
    /// 嵌入套的度量（对话套为 `None`）。
    #[serde(default)]
    pub embed: Option<EmbedMetrics>,
}

/// 一条对话用例的结果。只记判定与长度，**不记模型输出原文**（报告要上传、要进审计）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseResult {
    pub id: String,
    pub passed: bool,
    pub output_chars: u32,
}

/// 嵌入套的度量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedMetrics {
    pub dimensions: u32,
    /// 全部向量范数离 1 的最大偏差。
    pub norm_max_deviation: f64,
    /// 两段无关文本的余弦。
    pub unrelated_cosine: f64,
    /// 小检索集 top-1 命中率。
    pub hit_at_1: f64,
    /// 与参照模型（替换同一 profile 时）的逐条最小余弦；不是替换时 `None`。
    #[serde(default)]
    pub reference_sha256: Option<String>,
    #[serde(default)]
    pub reference_cosine_min: Option<f64>,
}

/// 服务端对一份报告的判定：`Ok(())` = 达标；`Err` 带英文原因（进 API 与审计，不含用户内容）。
///
/// `expected_dimensions`：该条目 profile 的原生维度（服务端从 profile 表取，`None` = 不核）。
///
/// # Errors
/// 报告不属于这个条目、评测集不对、用例缺 / 不过、嵌入度量不达标。
pub fn verdict(
    report: &EvalReport,
    entry: &ModelEntry,
    expected_dimensions: Option<u32>,
) -> Result<(), String> {
    if report.schema != REPORT_SCHEMA {
        return Err(format!("unknown report schema {:?}", report.schema));
    }
    if report.catalog_id != entry.id {
        return Err(format!(
            "report is for {:?}, not {:?}",
            report.catalog_id, entry.id
        ));
    }
    if report.sha256 != entry.weights.sha256 {
        return Err("report sha256 does not match the entry's weights".to_owned());
    }
    let suite = Suite::for_entry(entry).ok_or("entry needs no model evaluation")?;
    if report.suite != suite {
        return Err(format!(
            "entry requires suite {suite:?}, report is {:?}",
            report.suite
        ));
    }
    if report.suite_version != suite.version() {
        return Err(format!(
            "suite version {} is outdated (current {})",
            report.suite_version,
            suite.version()
        ));
    }
    if let Some(gguf) = &entry.gguf {
        if report.engine_build < gguf.engine_min_build {
            return Err(format!(
                "evaluated on engine b{} older than the entry's minimum b{}",
                report.engine_build, gguf.engine_min_build
            ));
        }
    }
    for case in suite.required_cases() {
        match report.cases.iter().find(|result| result.id == *case) {
            None => return Err(format!("case {case} missing from report")),
            Some(result) if !result.passed => return Err(format!("case {case} failed")),
            Some(_) => {}
        }
    }
    if suite == Suite::EmbedSanity {
        let metrics = report.embed.as_ref().ok_or("embedding metrics missing")?;
        if let Some(expected) = expected_dimensions {
            if metrics.dimensions != expected {
                return Err(format!(
                    "dimensions {} differ from profile {expected}",
                    metrics.dimensions
                ));
            }
        }
        if !at_most(metrics.norm_max_deviation, EMBED_NORM_TOLERANCE) {
            return Err(format!(
                "vectors not L2-normalised (max deviation {})",
                metrics.norm_max_deviation
            ));
        }
        if !below(metrics.unrelated_cosine, EMBED_DISTINCT_MAX) {
            return Err(format!(
                "degenerate vectors (unrelated cosine {})",
                metrics.unrelated_cosine
            ));
        }
        if !at_least(metrics.hit_at_1, EMBED_HIT_AT_1_BASELINE) {
            return Err(format!(
                "retrieval hit@1 {} below baseline {EMBED_HIT_AT_1_BASELINE}",
                metrics.hit_at_1
            ));
        }
        match (&metrics.reference_sha256, metrics.reference_cosine_min) {
            (Some(_), Some(cos)) if !at_least(cos, EMBED_EQUIVALENCE_MIN) => {
                return Err(format!(
                    "cosine to reference {cos} below {EMBED_EQUIVALENCE_MIN}"
                ));
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err("reference sha256 and cosine must come together".to_owned());
            }
            _ => {}
        }
    }
    Ok(())
}

// 三个比较都让 NaN 判失败：度量算坏了（零向量、空批）不能因为「比较为假」而溜过闸。
fn at_least(value: f64, min: f64) -> bool {
    matches!(
        value.partial_cmp(&min),
        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
    )
}

fn at_most(value: f64, max: f64) -> bool {
    matches!(
        value.partial_cmp(&max),
        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
    )
}

fn below(value: f64, max: f64) -> bool {
    matches!(value.partial_cmp(&max), Some(std::cmp::Ordering::Less))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ModelEntry {
        crate::bundled()
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .clone()
    }

    fn chat_report(entry: &ModelEntry, failing: Option<&str>) -> EvalReport {
        let suite = Suite::for_entry(entry).unwrap();
        EvalReport {
            schema: REPORT_SCHEMA.into(),
            catalog_id: entry.id.clone(),
            sha256: entry.weights.sha256.clone(),
            suite,
            suite_version: suite.version(),
            engine_build: 11429,
            tool: "luna test".into(),
            produced_at: "2026-10-07T00:00:00Z".into(),
            cases: suite
                .required_cases()
                .iter()
                .map(|id| CaseResult {
                    id: (*id).into(),
                    passed: Some(*id) != failing,
                    output_chars: 10,
                })
                .collect(),
            embed: None,
        }
    }

    #[test]
    fn suites_follow_the_entry_role() {
        assert_eq!(
            Suite::for_entry(&entry("qwen3.5-0.8b-q8_0-gguf")),
            Some(Suite::ChatSmoke)
        );
        assert_eq!(
            Suite::for_entry(&entry("hy-mt2-1.8b-q8_0-gguf")),
            Some(Suite::TranslateSmoke)
        );
        assert_eq!(
            Suite::for_entry(&entry("bge-m3-q8_0-gguf")),
            Some(Suite::EmbedSanity)
        );
        assert_eq!(Suite::for_entry(&entry("whisper-base-ggml")), None);
    }

    #[test]
    fn chat_verdict_needs_every_case_passed_for_this_exact_artifact() {
        let qwen = entry("qwen3.5-0.8b-q8_0-gguf");
        assert!(verdict(&chat_report(&qwen, None), &qwen, None).is_ok());
        assert!(verdict(&chat_report(&qwen, Some(CASE_JSON_ONLY)), &qwen, None).is_err());
        let mut trimmed = chat_report(&qwen, None);
        trimmed.cases.pop();
        assert!(verdict(&trimmed, &qwen, None)
            .unwrap_err()
            .contains("missing"));
        let mut other = chat_report(&qwen, None);
        other.sha256 = "0".repeat(64);
        assert!(verdict(&other, &qwen, None).is_err());
        let mut old = chat_report(&qwen, None);
        old.engine_build = 1;
        assert!(verdict(&old, &qwen, None).is_err());
        // 专用翻译模型考翻译套，拿通用对话套的报告不算。
        let hy = entry("hy-mt2-1.8b-q8_0-gguf");
        let mut wrong_suite = chat_report(&hy, None);
        wrong_suite.suite = Suite::ChatSmoke;
        assert!(verdict(&wrong_suite, &hy, None).is_err());
        assert!(verdict(&chat_report(&hy, None), &hy, None).is_ok());
    }

    #[test]
    fn embed_verdict_checks_shape_degeneracy_retrieval_and_equivalence() {
        let bge = entry("bge-m3-q8_0-gguf");
        let good = EmbedMetrics {
            dimensions: 1024,
            norm_max_deviation: 1e-6,
            unrelated_cosine: 0.3,
            hit_at_1: 1.0,
            reference_sha256: None,
            reference_cosine_min: None,
        };
        let report = |metrics: EmbedMetrics| EvalReport {
            embed: Some(metrics),
            cases: Vec::new(),
            suite: Suite::EmbedSanity,
            ..chat_report(&entry("qwen3.5-0.8b-q8_0-gguf"), None)
        };
        let fix = |mut r: EvalReport| {
            r.catalog_id = bge.id.clone();
            r.sha256 = bge.weights.sha256.clone();
            r
        };
        assert!(verdict(&fix(report(good.clone())), &bge, Some(1024)).is_ok());
        assert!(verdict(&fix(report(good.clone())), &bge, Some(768)).is_err());
        let bad = |f: fn(&mut EmbedMetrics)| {
            let mut m = good.clone();
            f(&mut m);
            verdict(&fix(report(m)), &bge, Some(1024)).is_err()
        };
        assert!(bad(|m| m.norm_max_deviation = 0.1));
        assert!(bad(|m| m.unrelated_cosine = 0.95));
        assert!(bad(|m| m.hit_at_1 = 0.5));
        assert!(bad(|m| m.hit_at_1 = f64::NAN));
        assert!(bad(|m| {
            m.reference_sha256 = Some("a".repeat(64));
            m.reference_cosine_min = Some(0.99);
        }));
        assert!(bad(|m| m.reference_cosine_min = Some(0.9999)));
    }

    #[test]
    fn case_predicates_tell_translation_from_answering() {
        assert!(case_passes(
            CASE_ZH_TO_EN,
            "The weather is nice today, let's take a walk in the park."
        ));
        assert!(!case_passes(CASE_ZH_TO_EN, "今天天气很好"));
        assert!(case_passes(CASE_EN_TO_ZH, "每周例会改到了周四下午。"));
        assert!(case_passes(
            CASE_QUESTION_NOT_ANSWERED,
            "你今天感觉怎么样？"
        ));
        assert!(!case_passes(
            CASE_QUESTION_NOT_ANSWERED,
            "我今天感觉很好，谢谢你的关心，希望你也一切顺利。"
        ));
        assert!(case_passes(CASE_JSON_ONLY, r#"{"city": "北京"}"#));
        assert!(!case_passes(
            CASE_JSON_ONLY,
            "```json\n{\"city\": \"北京\"}\n```"
        ));
        assert!(case_passes(
            CASE_SUMMARY_BRIEF,
            "例会定了发布日期、修复了导出乱码并确定了招聘计划。"
        ));
        assert!(!case_passes(CASE_NONEMPTY_REPLY, "   "));
        assert_eq!(EMBED_DOCS.len(), EMBED_QUERIES.len());
    }
}
