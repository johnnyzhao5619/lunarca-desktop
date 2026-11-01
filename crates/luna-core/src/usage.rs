//! 用量计量领域模型（零 IO，TDD-01 §5.8 / ADR-016）。
//!
//! `UsageRecord` 只保存计数与元数据，绝不携带转写文本、剪贴板、笔记正文或音频内容。

use crate::{
    Capability, Job, JobId, JobKind, JobOrigin, JobResult, ModelId, ProviderId, ResultPayload,
    TruncatedBy, Usage,
};
use serde::{Deserialize, Serialize};

/// 单次任务的用量事实记录。计量不是计费，估算金额沿用 [`crate::Money`]。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Job id。
    pub job_id: JobId,
    /// 任务来源，用于设备级/Vault 级归属路由。
    pub origin: JobOrigin,
    /// 能力类型。
    pub capability: Capability,
    /// 这一次打的是**哪一档服务**（ADR-083 ②）。
    ///
    /// **它不是 `capability` 的附属信息，而是路由的另一半**：服务端按
    /// `(capability, class)` 取链，同一个能力的两档可以落在完全不同的上游上。真机
    /// （2026-08-31）弹窗翻译连续失败而同一台服务端的资料蒸馏一切正常，两者的唯一
    /// 差别就是这一格——没有它，账本上那两组记录看起来是同一件事的成功与失败。
    ///
    /// `None` 只有一种来源：**这一列出现之前**写下的历史行（`usage.sqlite` 是 durable
    /// 历史事实，加一列不重建它）。新记录恒为 `Some`。不给历史行猜一个档位——猜错会让
    /// 当时的实时档记录在账本上变成批量档，而这一格存在的全部理由正是分辨这两者。
    pub service_class: Option<crate::ServiceClass>,
    /// Provider。
    pub provider_id: ProviderId,
    /// 这一次**在哪跑的**（ADR-126 第 6 条）。
    ///
    /// 从 `provider_id` 反推是错的：id 是稳定标识不是位置声明，而「本机 / 服务端」这件事
    /// 有两个消费者都必须答对——执行记录的那一列（面板导语一直写着「在哪跑的」，而此前
    /// 界面上根本没有这一格），以及「本机省下多少 / 服务端花了多少」的分组（此前把服务端
    /// 跑的任务也算进了"为你节省"，那是把花费说成了节省）。
    ///
    /// `None` 只有一种来源：这一列出现之前写下的历史行（与 [`Self::service_class`] 同理，
    /// `usage.sqlite` 是 durable 历史事实，加一列不重建它）。不给历史行猜一个位置。
    pub provider_location: Option<crate::ProviderLocation>,
    /// 模型。
    pub model_id: Option<ModelId>,
    /// 用量计数。
    pub usage: Usage,
    /// 终态：`finished` / `failed` / `cancelled`。
    pub terminal_status: String,
    /// 这一次是否触顶收尾、以及**是哪一道闸**（`None` = 正文自然写完）。
    ///
    /// **触顶是「完成 + 标注」而不是失败**（ADR-089 ④），于是它在 `terminal_status`
    /// 里看不出来——一条触顶记录与一条正常完成的记录在这张表上曾经**一模一样**。
    /// 用户侧的出口就是这里：「我这个月有几篇被截断了」只有本机用量库回答得了，
    /// 而运营侧那半（`inference_jobs.truncated_by`）在服务端、用户看不到。
    pub output_truncated: Option<TruncatedBy>,
    /// 失败终态的稳定错误码（[`crate::JobError::code`]）；成功与取消恒为 `None`。
    ///
    /// **脱敏**：只有码本身（`SERVER_SIDE_FAILURE` 之类），不含上游消息、模型输出或
    /// 任何用户正文——那正是 `JobError` 的 `message` 字段留在进程内、不进这张表的理由。
    pub error_code: Option<String>,
    /// 完成时间 Unix ms。
    pub finished_at_ms: i64,
}

impl UsageRecord {
    /// 从成功结果构造记录。正文 payload 不会进入记录，只读取其中的计数元数据。
    #[must_use]
    pub fn from_finished(job: &Job, result: &JobResult, location: crate::ProviderLocation) -> Self {
        Self {
            job_id: job.id,
            origin: job.origin,
            capability: job.kind.capability(),
            service_class: Some(job.service_class()),
            provider_id: result.provider_id.clone(),
            provider_location: Some(location),
            model_id: result.model_id.clone(),
            usage: result.usage,
            terminal_status: "finished".to_string(),
            output_truncated: result.output_truncated,
            error_code: None,
            finished_at_ms: result.finished_at.0,
        }
    }

    /// 从失败/取消终态构造记录。
    ///
    /// `attribution` 说的是**这一次实际走到了哪、栽在什么码上**：路由阶段就没选出腿时
    /// Provider 为 [`UNRESOLVED_PROVIDER`]，那本身也是一条信息（「根本没路由到」）。
    #[must_use]
    pub fn from_terminal(
        job: &Job,
        terminal_status: impl Into<String>,
        usage: Usage,
        finished_at: crate::Timestamp,
        attribution: TerminalAttribution<'_>,
    ) -> Self {
        Self {
            job_id: job.id,
            origin: job.origin,
            capability: job.kind.capability(),
            service_class: Some(job.service_class()),
            provider_id: attribution
                .provider
                .map_or_else(|| ProviderId::from(UNRESOLVED_PROVIDER), |d| d.id.clone()),
            // 一条腿都没选出来时位置也无从谈起——那本身就是事实（「根本没路由到」）。
            provider_location: attribution.provider.map(|d| d.location),
            model_id: attribution
                .provider
                .and_then(|d| d.model.as_deref())
                .map(ModelId::from),
            usage,
            // 失败/取消没有"产出被截断"这回事：那道闸只在正文真的写出来的路上才会被撞到。
            output_truncated: None,
            error_code: attribution.error.map(|e| e.code().to_owned()),
            terminal_status: terminal_status.into(),
            finished_at_ms: finished_at.as_millis(),
        }
    }
}

/// 一条腿都没选出来时记在 `provider_id` 上的稳定占位值。
///
/// **它是一个真答案，不是「不知道」**：路由阶段就失败（能力无 Provider、严格本地拦下、
/// 流式会话的腿掉了）与「某条腿跑起来了然后报错」是两件完全不同的事，而事后只有这一格
/// 分得开它们。
pub const UNRESOLVED_PROVIDER: &str = "unresolved";

/// 失败/取消终态的归属证据（[`UsageRecord::from_terminal`] 的入参）。
///
/// # 为什么它必须存在
///
/// 失败记录此前**恒为** `provider_id = "unresolved"` + 无错误码：一条失败在这张表上只剩
/// 「某时某刻，某个来源的某个能力失败了」。真机（2026-08-31）弹窗翻译连续失败 9 次，
/// 本机账本上确实有 9 行，而那 9 行答不出任何一个有用的问题——哪条腿、哪一档、什么码，
/// 于是排查只能靠复现。用量库是这台设备上**唯一**的失败历史面（服务端账本只记它自己
/// 受理过的那些，而这次失败恰恰发生在它派发之前），它答不出问题就等于没有观测面。
#[derive(Clone, Copy, Default)]
pub struct TerminalAttribution<'a> {
    /// 已经选定并真正执行过的 Provider；`None` = 路由阶段就没选出任何一条腿。
    pub provider: Option<&'a crate::ProviderDescriptor>,
    /// 终态错误。`None` 只用于取消——取消不是失败，没有码。
    pub error: Option<&'a crate::JobError>,
}

impl<'a> TerminalAttribution<'a> {
    /// 只知道错误、还没选出腿（路由失败）。
    #[must_use]
    pub fn unrouted(error: &'a crate::JobError) -> Self {
        Self {
            provider: None,
            error: Some(error),
        }
    }
}

/// 判断结果 payload 是否携带正文类内容；测试隐私不变量时使用。
#[must_use]
pub fn payload_contains_private_text(payload: &ResultPayload) -> bool {
    match payload {
        ResultPayload::Segments(segments) => segments.iter().any(|segment| {
            !segment.text.is_empty()
                || segment
                    .translation
                    .as_deref()
                    .is_some_and(|t| !t.is_empty())
        }),
        ResultPayload::Text(text) => !text.is_empty(),
        ResultPayload::Chat(message) => !message.content.is_empty(),
        // 向量**也算私有内容**：它是用户笔记正文的有损但可攻击的表示（嵌入反演已是
        // 公开研究结果），把它当成"一串无意义的数字"写进日志/遥测与写正文没有本质区别。
        // 非空即视为携带私有内容，与其余三种同档（TDD-12 脱敏不变量按此断言）。
        ResultPayload::Embeddings(vectors) => vectors.iter().any(|v| !v.is_empty()),
    }
}

/// 判断 Job kind 是否携带用户输入正文或音频引用；测试隐私不变量时使用。
#[must_use]
pub fn job_kind_contains_private_input(kind: &JobKind) -> bool {
    match kind {
        // 音频/图像引用均属私有用户内容（转写/OCR/细分的输入是用户素材，脱敏不变量按此断言）。
        JobKind::Transcribe { .. } | JobKind::Ocr { .. } | JobKind::Diarize { .. } => true,
        JobKind::Translate { text, .. }
        | JobKind::Summarize { text, .. }
        | JobKind::Refine { text, .. }
        | JobKind::ExtractTags { text, .. } => {
            matches!(text, crate::TextRef::Inline(value) if !value.is_empty())
        }
        JobKind::Chat { messages, .. } => {
            messages.iter().any(|message| !message.content.is_empty())
        }
        JobKind::Custom { vars, .. } => vars.values().any(|value| !value.is_empty()),
        // 嵌入的输入就是笔记正文分片——与 Translate/Summarize 同档。
        JobKind::Embed { texts, .. } => texts.iter().any(|text| !text.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ChatRole, ResultPayload, TextRef, Timestamp, Timings};

    #[test]
    fn finished_record_excludes_payload_text() {
        let job = Job::new(
            JobKind::Refine {
                text: TextRef::Inline("private selected note body".into()),
                instruction: None,
            },
            JobOrigin::FloatingMenu,
        );
        let result = JobResult {
            job_id: job.id,
            provider_id: ProviderId::from("local-llm"),
            model_id: Some(ModelId::from("qwen3")),
            payload: ResultPayload::Text("private generated body".into()),
            usage: Usage {
                tokens_in: Some(11),
                tokens_out: Some(7),
                ..Default::default()
            },
            timings: Timings::default(),
            output_truncated: None,
            finished_at: Timestamp(42),
        };

        assert!(job_kind_contains_private_input(&job.kind));
        assert!(payload_contains_private_text(&result.payload));

        let record = UsageRecord::from_finished(&job, &result, crate::ProviderLocation::Device);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("private"));
        assert_eq!(record.origin, JobOrigin::FloatingMenu);
        assert_eq!(record.capability, Capability::Refine);
        assert_eq!(record.usage.tokens_out, Some(7));
    }

    #[test]
    fn chat_payload_is_private_but_record_is_metadata_only() {
        let job = Job::new(
            JobKind::Chat {
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    content: "private chat".into(),
                }],
                tools: Vec::new(),
            },
            JobOrigin::Chat,
        );
        let result = JobResult {
            job_id: job.id,
            provider_id: ProviderId::from("local-llm"),
            model_id: None,
            payload: ResultPayload::Chat(ChatMessage {
                role: ChatRole::Assistant,
                content: "private answer".into(),
            }),
            usage: Usage::default(),
            timings: Timings::default(),
            output_truncated: None,
            finished_at: Timestamp(7),
        };

        let record = UsageRecord::from_finished(&job, &result, crate::ProviderLocation::Device);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("private"));
        assert_eq!(record.capability, Capability::Chat);
    }
}
