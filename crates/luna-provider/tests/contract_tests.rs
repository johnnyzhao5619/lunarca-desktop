//! Provider 契约测试（TDD-01 §8）：对 `MockProvider` 的各脚本校验流式不变量。
//! 新增 Provider 须通过同一套契约。

use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, Job, JobError, JobKind, JobOrigin, Lang,
    ProviderId, ProviderLocation, Segment, SegmentId, StreamId, TextRef,
};
use luna_provider::{capability_set, check_invariants, MockProvider, MockScript, TerminalKind};
use std::time::Duration;

fn translate_job() -> Job {
    Job::new(
        JobKind::Translate {
            text: TextRef::Inline("hi".into()),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        JobOrigin::PopupTranslate,
    )
}

fn translate_provider(script: MockScript) -> MockProvider {
    MockProvider::new(
        "mock",
        capability_set(&[Capability::Translate], ProviderLocation::LocalService),
        script,
    )
}

#[tokio::test]
async fn normal_text_terminates_with_finished() {
    let p = translate_provider(MockScript::TextResult("hello".into()));
    let summary = check_invariants(p.execute(translate_job(), CancelToken::new()).await).await;
    assert_eq!(summary.terminal, Some(TerminalKind::Finished));
    assert!(!summary.saw_event_after_terminal);
    // Started + 5 tokens + Finished
    assert_eq!(summary.events, 7);
}

#[tokio::test]
async fn fail_before_partial_is_terminal_failed() {
    let p = translate_provider(MockScript::Fail(JobError::Timeout));
    let summary = check_invariants(p.execute(translate_job(), CancelToken::new()).await).await;
    assert_eq!(summary.terminal, Some(TerminalKind::Failed));
    assert!(!summary.saw_event_after_terminal);
}

#[tokio::test]
async fn fail_after_partial_keeps_output_and_is_terminal() {
    // §4 流式硬边界：首个 Partial 之后失败 → 终态 Failed，已产出部分保留、不跨 Provider 重来。
    let p = translate_provider(MockScript::FailAfterPartial(
        JobError::ProviderUnavailable {
            provider: ProviderId::from("mock"),
        },
    ));
    let summary = check_invariants(p.execute(translate_job(), CancelToken::new()).await).await;
    assert_eq!(summary.terminal, Some(TerminalKind::Failed));
    assert!(summary.events >= 3); // Started + Partial + Failed
    assert!(!summary.saw_event_after_terminal);
}

#[tokio::test]
async fn cancellation_reaches_cancelled_terminal() {
    let p = translate_provider(MockScript::HangUntilCancel);
    let cancel = CancelToken::new();
    let stream = p.execute(translate_job(), cancel.clone()).await;
    let task = tokio::spawn(async move { check_invariants(stream).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel.cancel();
    let summary = task.await.unwrap();
    assert_eq!(summary.terminal, Some(TerminalKind::Cancelled));
    assert!(!summary.saw_event_after_terminal);
}

#[tokio::test]
async fn committed_segments_are_monotonic() {
    let seg = |start_ms: u64, end_ms: u64, text: &str| Segment {
        id: SegmentId::new(),
        start: Duration::from_millis(start_ms),
        end: Duration::from_millis(end_ms),
        text: text.to_string(),
        translation: None,
        speaker: None,
        confidence: None,
        words: None,
    };
    let p = MockProvider::new(
        "asr",
        capability_set(&[Capability::Transcribe], ProviderLocation::Device),
        MockScript::Segments(vec![seg(0, 1000, "a"), seg(1000, 2000, "b")]),
    );
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Stream(StreamId::new()),
            lang_hint: None,
            initial_prompt: None,
        },
        JobOrigin::Recording,
    );
    let summary = check_invariants(p.execute(job, CancelToken::new()).await).await;
    assert_eq!(summary.terminal, Some(TerminalKind::Finished));
    assert_eq!(summary.committed.len(), 2);
    assert!(summary.committed_monotonic);
}

// ── 云 Provider 的契约（不需要网络的那三条失败路径）────────────────────────────

/// 恒无凭据的来源（云 Provider 会据此本地短路，一个字节都不发）。
struct NoBearer;

#[async_trait::async_trait]
impl luna_provider::asr_remote::BearerSource for NoBearer {
    async fn bearer(&self) -> Option<String> {
        None
    }
    async fn refresh(&self, _rejected: &str) -> Option<String> {
        None
    }
}

/// 恒拦截的外发闸（严格本地）。
struct BlockOutbound;

impl luna_core::OutboundGate for BlockOutbound {
    fn allows_outbound(&self) -> bool {
        false
    }
}

fn cloud_llm(gate: std::sync::Arc<dyn luna_core::OutboundGate>) -> luna_provider::CloudLlmProvider {
    luna_provider::CloudLlmProvider::new(luna_provider::CloudLlmConfig {
        // 不可达地址：这三条路径都在**发请求之前**就收尾，故永远不会用到它。
        api_base: "http://server.invalid".into(),
        display_name: "测试".into(),
        bearer: std::sync::Arc::new(NoBearer),
        gate,
        allowed: None,
    })
}

fn cloud_embed(
    gate: std::sync::Arc<dyn luna_core::OutboundGate>,
) -> luna_provider::CloudEmbedProvider {
    luna_provider::CloudEmbedProvider::new(luna_provider::CloudEmbedConfig {
        api_base: "http://server.invalid".into(),
        display_name: "测试".into(),
        profile_id: "bge-m3:v1:1024:l2".to_owned(),
        bearer: std::sync::Arc::new(NoBearer),
        gate,
    })
}

fn embed_job() -> Job {
    Job::new(
        JobKind::Embed {
            texts: vec!["一段文本".into()],
            purpose: luna_core::EmbeddingPurpose::Document,
        },
        JobOrigin::Recording,
    )
}

/// 两个云 Provider 必须过同一套流式契约（本文件头一句就是这条规矩）。
///
/// 覆盖的是**不需要网络**的三条收尾路径，而它们恰恰是手写 `once_failed` 的地方：
/// 外发闸拦截 / 无凭据 / Job 类型不匹配。写错的表现是「终态后还有事件」或「压根没有
/// 终态」——前者让上层的单槽运行态错乱，后者让任务永远挂着，两种都不会编译失败。
#[tokio::test]
async fn cloud_providers_terminate_cleanly_on_every_no_network_path() {
    let allow = || {
        std::sync::Arc::new(luna_core::AllowOutbound) as std::sync::Arc<dyn luna_core::OutboundGate>
    };
    let block =
        || std::sync::Arc::new(BlockOutbound) as std::sync::Arc<dyn luna_core::OutboundGate>;

    for (name, summary) in [
        (
            "对话 · 严格本地拦截",
            check_invariants(
                cloud_llm(block())
                    .execute(translate_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
        (
            "对话 · 无凭据",
            check_invariants(
                cloud_llm(allow())
                    .execute(translate_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
        (
            "对话 · Job 类型不适用（嵌入 Job 派给对话 Provider）",
            check_invariants(
                cloud_llm(allow())
                    .execute(embed_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
        (
            "嵌入 · 严格本地拦截",
            check_invariants(
                cloud_embed(block())
                    .execute(embed_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
        (
            "嵌入 · 无凭据",
            check_invariants(
                cloud_embed(allow())
                    .execute(embed_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
        (
            "嵌入 · 空文本批",
            check_invariants(
                cloud_embed(allow())
                    .execute(translate_job(), CancelToken::new())
                    .await,
            )
            .await,
        ),
    ] {
        assert_eq!(
            summary.terminal,
            Some(TerminalKind::Failed),
            "{name}：没有以失败收尾——任务会永远挂着"
        );
        assert!(
            !summary.saw_event_after_terminal,
            "{name}：终态之后还有事件——上层的单槽运行态会错乱"
        );
    }
}
