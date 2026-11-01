//! 可脚本化的 `MockProvider`：用于契约测试与走查骨架（无网络 IO、可控事件流）。

use async_stream::stream;
use async_trait::async_trait;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, Job, JobError, JobEvent, JobEventStream,
    JobResult, PartialPayload, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation,
    ResultPayload, Segment, Timestamp, Timings, Usage, UsageEstimate,
};
use std::time::Duration;

/// 便捷构造一个能力集（流式、指定能力与位置）。
#[must_use]
pub fn capability_set(caps: &[Capability], location: ProviderLocation) -> CapabilitySet {
    CapabilitySet {
        capabilities: caps.iter().copied().collect(),
        streaming: true,
        location,
        explicit_only: false,
    }
}

/// `MockProvider` 的脚本，决定 `execute` 产出的事件序列。
#[derive(Clone, Debug)]
pub enum MockScript {
    /// 逐字符流式文本，末尾 `Finished(Text)`。
    TextResult(String),
    /// 逐段 committed，末尾 `Finished(Segments)`。
    Segments(Vec<Segment>),
    /// `Started → Failed`（首个 Partial **之前**失败）。
    Fail(JobError),
    /// `Started → Usage(快照) → Failed`：失败前对方自报了用量（测失败终态按快照记账）。
    FailWithUsage(luna_core::Usage, JobError),
    /// `Started → Partial → Failed`（首个 Partial **之后**失败；测 §4 流式硬边界）。
    FailAfterPartial(JobError),
    /// `Started` 后挂起，直到 `cancel`（测取消传播，§5.5）。
    HangUntilCancel,
}

/// 脚本化 Provider。
pub struct MockProvider {
    id: ProviderId,
    caps: CapabilitySet,
    script: MockScript,
}

impl MockProvider {
    /// 构造。
    pub fn new(id: impl Into<ProviderId>, caps: CapabilitySet, script: MockScript) -> Self {
        Self {
            id: id.into(),
            caps,
            script,
        }
    }
}

#[async_trait]
impl AIProvider for MockProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.clone(),
            display_name: self.id.as_str().to_string(),
            // Mock 不跑模型——`None` 是如实回答，不是占位。
            model: None,
            location: self.caps.location,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        self.caps.clone()
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let script = self.script.clone();
        let provider_id = self.id.clone();
        let job_id = job.id;
        Box::pin(stream! {
            yield JobEvent::Started;
            match script {
                MockScript::TextResult(text) => {
                    for ch in text.chars() {
                        if cancel.is_cancelled() {
                            yield JobEvent::Cancelled;
                            return;
                        }
                        yield JobEvent::Partial(PartialPayload::Token(ch.to_string()));
                    }
                    yield JobEvent::Finished(JobResult {
                        job_id,
                        provider_id,
                        model_id: None,
                        payload: ResultPayload::Text(text),
                        usage: Usage::default(),
                        timings: Timings::default(),
                        output_truncated: None,
                        finished_at: Timestamp::ZERO,
                    });
                }
                MockScript::Segments(segs) => {
                    for seg in &segs {
                        if cancel.is_cancelled() {
                            yield JobEvent::Cancelled;
                            return;
                        }
                        yield JobEvent::Partial(PartialPayload::Asr {
                            committed_delta: vec![seg.clone()],
                            hypothesis: String::new(),
                            stability: 1.0,
                        });
                    }
                    yield JobEvent::Finished(JobResult {
                        job_id,
                        provider_id,
                        model_id: None,
                        payload: ResultPayload::Segments(segs),
                        usage: Usage::default(),
                        timings: Timings::default(),
                        output_truncated: None,
                        finished_at: Timestamp::ZERO,
                    });
                }
                MockScript::Fail(err) => {
                    yield JobEvent::Failed(err);
                }
                MockScript::FailWithUsage(usage, err) => {
                    yield JobEvent::Usage(usage);
                    yield JobEvent::Failed(err);
                }
                MockScript::FailAfterPartial(err) => {
                    yield JobEvent::Partial(PartialPayload::Token("…".to_string()));
                    yield JobEvent::Failed(err);
                }
                MockScript::HangUntilCancel => loop {
                    if cancel.is_cancelled() {
                        yield JobEvent::Cancelled;
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                },
            }
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None
    }
}
