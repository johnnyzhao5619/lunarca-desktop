//! JobQueue 端到端：提交→路由→流式→兜底→取消（TDD-01 §4/§5）。

use luna_core::{
    AIProvider, Capability, Job, JobError, JobEvent, JobKind, JobOrigin, Lang, ProviderLocation,
    ResultPayload, RoutingHint, TextRef, UsageRecord,
};
use luna_jobqueue::{AdmissionCapacity, JobQueue, JobSink, UsageSink};
use luna_provider::{capability_set, MockProvider, MockScript, ProviderRegistry};
use std::sync::Arc;
use std::sync::Mutex;
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

fn translate_provider(id: &str, script: MockScript) -> Arc<dyn AIProvider> {
    translate_provider_at(id, ProviderLocation::LocalService, script)
}

fn translate_provider_at(
    id: &str,
    location: ProviderLocation,
    script: MockScript,
) -> Arc<dyn AIProvider> {
    Arc::new(MockProvider::new(
        id,
        capability_set(&[Capability::Translate], location),
        script,
    ))
}

fn queue_with(providers: Vec<Arc<dyn AIProvider>>, concurrency: usize) -> JobQueue {
    let reg = ProviderRegistry::new();
    for p in providers {
        reg.register(p);
    }
    JobQueue::new(
        Arc::new(reg),
        AdmissionCapacity {
            compute: concurrency,
            session: concurrency,
        },
    )
}

#[derive(Default)]
struct CapturingUsageSink {
    records: Mutex<Vec<UsageRecord>>,
}

impl UsageSink for CapturingUsageSink {
    fn record(&self, record: UsageRecord) {
        self.records.lock().unwrap().push(record);
    }
}

#[tokio::test]
async fn submit_runs_to_finished_with_queued_first() {
    let q = queue_with(
        vec![translate_provider(
            "llm",
            MockScript::TextResult("ok".into()),
        )],
        2,
    );
    let events = q.submit(translate_job()).collect().await;

    assert!(matches!(events.first(), Some(JobEvent::Queued)));
    assert!(events.iter().any(|e| matches!(e, JobEvent::Started)));
    assert!(matches!(
        events.last(),
        Some(JobEvent::Finished(r)) if matches!(&r.payload, ResultPayload::Text(t) if t == "ok")
    ));
}

#[tokio::test]
async fn terminal_jobs_emit_usage_records_to_injected_sink() {
    let reg = ProviderRegistry::new();
    reg.register(translate_provider(
        "llm",
        MockScript::TextResult("ok".into()),
    ));
    let sink = Arc::new(CapturingUsageSink::default());
    let q = JobQueue::with_usage_sink(
        Arc::new(reg),
        AdmissionCapacity {
            compute: 2,
            session: 2,
        },
        sink.clone(),
    );

    let events = q.submit(translate_job()).collect().await;

    let finished_at = match events.last() {
        Some(JobEvent::Finished(result)) => result.finished_at.as_millis(),
        other => panic!("expected finished event, got {other:?}"),
    };
    assert!(
        finished_at > 0,
        "调度层必须把 Provider 的占位时间戳替换为真实完成时间"
    );
    let records = sink.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].origin, JobOrigin::PopupTranslate);
    assert_eq!(records[0].capability, Capability::Translate);
    assert_eq!(records[0].provider_id.as_str(), "llm");
    assert_eq!(records[0].usage.tokens_in, None);
    assert_eq!(records[0].usage.tokens_out, None);
    assert_eq!(records[0].finished_at_ms, finished_at);
    assert_eq!(
        records[0].service_class,
        Some(luna_core::ServiceClass::Realtime),
        "弹窗翻译是实时档（ADR-083 ③）；账本上少了这一格，两条链的记录就分不开了"
    );
    assert_eq!(records[0].error_code, None, "成功不该带错误码");
}

/// 失败终态按最后一份用量快照记账（TDD-14 §3 修订注，2026-09-06 收口）：Provider 在
/// `Failed` 前发的 `JobEvent::Usage` 就是账本上那一行的用量——不再恒记 0。
#[tokio::test]
async fn a_failed_job_records_the_last_usage_snapshot() {
    let reg = ProviderRegistry::new();
    let reported = luna_core::Usage {
        tokens_in: Some(120),
        tokens_out: Some(7),
        ..Default::default()
    };
    reg.register(translate_provider(
        "llm",
        MockScript::FailWithUsage(reported, JobError::Timeout),
    ));
    let sink = Arc::new(CapturingUsageSink::default());
    let q = JobQueue::with_usage_sink(
        Arc::new(reg),
        AdmissionCapacity {
            compute: 2,
            session: 2,
        },
        sink.clone(),
    );

    let events = q.submit(translate_job()).collect().await;
    assert!(events
        .iter()
        .any(|e| matches!(e, JobEvent::Usage(u) if *u == reported)));
    assert!(matches!(
        events.last(),
        Some(JobEvent::Failed(JobError::Timeout))
    ));

    let records = sink.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].terminal_status, "failed");
    assert_eq!(records[0].usage, reported, "失败行必须带上对方自报的用量");
}

/// **一条失败必须答得出「哪条腿、哪一档、什么码」**。
///
/// 真机 2026-08-31：弹窗翻译连续失败 9 次，本机账本上确实有 9 行，而每一行都是
/// `provider_id = "unresolved"` + `terminal_status = "failed"` + 没有码——于是「有记录」
/// 与「没记录」在排查上完全等价，只能靠复现。用量库是这台设备上唯一的失败历史面
/// （服务端账本只记它受理过的那些，而那次失败正发生在它派发之前）。
#[tokio::test]
async fn a_failed_job_records_which_leg_which_lane_and_which_code() {
    let reg = ProviderRegistry::new();
    reg.register(translate_provider(
        "llm",
        MockScript::Fail(JobError::Upstream {
            provider: luna_core::ProviderId::from("llm"),
            code: Some("503".into()),
            message: "overloaded".into(),
        }),
    ));
    let sink = Arc::new(CapturingUsageSink::default());
    let q = JobQueue::with_usage_sink(
        Arc::new(reg),
        AdmissionCapacity {
            compute: 2,
            session: 2,
        },
        sink.clone(),
    );

    let events = q.submit(translate_job()).collect().await;
    assert!(matches!(events.last(), Some(JobEvent::Failed(_))));

    let records = sink.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].terminal_status, "failed");
    assert_eq!(
        records[0].provider_id.as_str(),
        "llm",
        "跑起来过的那条腿必须留名——`unresolved` 只该属于「压根没路由到」"
    );
    assert_eq!(records[0].error_code.as_deref(), Some("UPSTREAM"));
    assert_eq!(
        records[0].service_class,
        Some(luna_core::ServiceClass::Realtime)
    );
}

/// 路由阶段就没选出腿：Provider 如实是 `unresolved`，**而码仍然在**。
///
/// 两者的区别本身就是信息：「注册表里没有能干这件事的 Provider」与「某条腿跑起来后报错」
/// 指向完全不同的排查方向。
#[tokio::test]
async fn a_job_that_never_routed_records_unresolved_but_still_records_the_code() {
    let sink = Arc::new(CapturingUsageSink::default());
    let q = JobQueue::with_usage_sink(
        Arc::new(ProviderRegistry::new()),
        AdmissionCapacity {
            compute: 2,
            session: 2,
        },
        sink.clone(),
    );

    let events = q.submit(translate_job()).collect().await;
    assert!(matches!(events.last(), Some(JobEvent::Failed(_))));

    let records = sink.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].provider_id.as_str(),
        luna_core::UNRESOLVED_PROVIDER
    );
    assert_eq!(records[0].model_id, None);
    assert_eq!(
        records[0].error_code.as_deref(),
        Some("PROVIDER_NOT_CONFIGURED")
    );
}

#[tokio::test]
async fn falls_back_to_next_provider_before_commit() {
    // 首选在产出前失败（可重试）→ 一跳回退到第二个，最终拿到第二个的结果。
    let q = queue_with(
        vec![
            translate_provider("flaky", MockScript::Fail(JobError::Timeout)),
            translate_provider("good", MockScript::TextResult("recovered".into())),
        ],
        2,
    );
    let events = q.submit(translate_job()).collect().await;

    assert!(matches!(
        events.last(),
        Some(JobEvent::Finished(r)) if matches!(&r.payload, ResultPayload::Text(t) if t == "recovered")
    ));
    // 没有泄漏首选的 Failed（兜底发生在转发任何事件之前）。
    assert!(!events.iter().any(|e| matches!(e, JobEvent::Failed(_))));
    // Started 只出现一次（无可见的 Provider 跳变）。
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, JobEvent::Started))
            .count(),
        1
    );
}

#[tokio::test]
async fn cloud_unreachable_falls_back_to_local_before_commit() {
    let q = queue_with(
        vec![
            translate_provider_at(
                "local",
                ProviderLocation::LocalService,
                MockScript::TextResult("local recovery".into()),
            ),
            translate_provider_at(
                "cloud",
                ProviderLocation::Cloud,
                MockScript::Fail(JobError::ProviderUnavailable {
                    provider: "cloud".into(),
                }),
            ),
        ],
        2,
    );
    let events = q.submit(translate_job()).collect().await;
    assert!(matches!(
        events.last(),
        Some(JobEvent::Finished(result))
            if matches!(&result.payload, ResultPayload::Text(text) if text == "local recovery")
    ));
}

#[derive(Clone, Copy, Debug)]
enum OutputStage {
    BeforeVisibleOutput,
    AfterVisibleOutput,
}

#[derive(Clone, Copy, Debug)]
struct FailoverCase {
    name: &'static str,
    current: ProviderLocation,
    next: ProviderLocation,
    stage: OutputStage,
    expected: luna_core::error::FailoverDisposition,
    assert_through_queue: bool,
    allows_switch: bool,
}

/// 错误类别 × 执行位置 × 输出阶段的回退矩阵。错误是否可重试与能否换腿是正交事实；
/// 这里故意通过真实 `JobQueue` 跑完整个矩阵，避免 Provider 映射或调度器任一侧单独漂移。
#[tokio::test]
async fn failover_matrix_only_switches_unreachable_routes_before_visible_output() {
    let cases = [
        FailoverCase {
            name: "cloud unavailable before visible output falls back locally",
            current: ProviderLocation::Cloud,
            next: ProviderLocation::LocalService,
            stage: OutputStage::BeforeVisibleOutput,
            expected: luna_core::error::FailoverDisposition::FallBackToLocal,
            assert_through_queue: true,
            allows_switch: true,
        },
        FailoverCase {
            name: "cloud timeout before visible output falls back locally",
            current: ProviderLocation::Cloud,
            next: ProviderLocation::LocalService,
            stage: OutputStage::BeforeVisibleOutput,
            expected: luna_core::error::FailoverDisposition::FallBackToLocal,
            assert_through_queue: true,
            allows_switch: true,
        },
        FailoverCase {
            name: "reachable native upstream failure never falls back locally",
            current: ProviderLocation::Cloud,
            next: ProviderLocation::LocalService,
            stage: OutputStage::BeforeVisibleOutput,
            expected: luna_core::error::FailoverDisposition::Stop,
            assert_through_queue: true,
            allows_switch: false,
        },
        FailoverCase {
            name: "reachable third party 5xx never falls back locally",
            current: ProviderLocation::Cloud,
            next: ProviderLocation::LocalService,
            stage: OutputStage::BeforeVisibleOutput,
            expected: luna_core::error::FailoverDisposition::Stop,
            assert_through_queue: true,
            allows_switch: false,
        },
        FailoverCase {
            name: "local timeout never uploads to cloud",
            current: ProviderLocation::LocalService,
            next: ProviderLocation::Cloud,
            stage: OutputStage::BeforeVisibleOutput,
            expected: luna_core::error::FailoverDisposition::Stop,
            // 默认路由本来就会把云候选排在本地前；此格只验领域处置，避免测试夹具
            // 倒置候选顺序而误把 Router 的默认策略当成 JobQueue 的行为。
            assert_through_queue: false,
            allows_switch: false,
        },
        FailoverCase {
            name: "cloud timeout after visible output never switches",
            current: ProviderLocation::Cloud,
            next: ProviderLocation::LocalService,
            stage: OutputStage::AfterVisibleOutput,
            expected: luna_core::error::FailoverDisposition::FallBackToLocal,
            assert_through_queue: true,
            allows_switch: false,
        },
    ];
    let errors = [
        JobError::ProviderUnavailable {
            provider: "primary".into(),
        },
        JobError::Timeout,
        JobError::Upstream {
            provider: "primary".into(),
            code: Some("upstream_unavailable".into()),
            message: "native service responded".into(),
        },
        JobError::Upstream {
            provider: "primary".into(),
            code: Some("503".into()),
            message: "third party responded".into(),
        },
        JobError::Timeout,
        JobError::Timeout,
    ];

    for (case, error) in cases.into_iter().zip(errors) {
        assert_eq!(
            error.failover_disposition(case.current, case.next),
            case.expected,
            "{}: error × execution location classification drifted",
            case.name
        );
        if !case.assert_through_queue {
            continue;
        }
        let q = queue_with(
            vec![
                translate_provider_at(
                    "primary",
                    case.current,
                    match case.stage {
                        OutputStage::BeforeVisibleOutput => MockScript::Fail(error.clone()),
                        OutputStage::AfterVisibleOutput => {
                            MockScript::FailAfterPartial(error.clone())
                        }
                    },
                ),
                translate_provider_at(
                    "fallback",
                    case.next,
                    MockScript::TextResult("fallback result".into()),
                ),
            ],
            2,
        );
        let events = q.submit(translate_job()).collect().await;
        let switched = matches!(events.last(), Some(JobEvent::Finished(result))
            if matches!(&result.payload, ResultPayload::Text(text) if text == "fallback result"));
        assert_eq!(
            switched, case.allows_switch,
            "{}: events = {events:?}",
            case.name
        );
        if !case.allows_switch {
            assert!(matches!(events.last(), Some(JobEvent::Failed(actual)) if actual == &error));
        }
    }
}

#[tokio::test]
async fn no_fallback_after_commit_yields_terminal_failed() {
    // 首选产出 Partial 后才失败（已选定）→ 不回退，终态 Failed，绝不切到能成功的第二个。
    let q = queue_with(
        vec![
            translate_provider("flaky", MockScript::FailAfterPartial(JobError::Timeout)),
            translate_provider("good", MockScript::TextResult("should-not-be-used".into())),
        ],
        2,
    );
    let events = q.submit(translate_job()).collect().await;

    assert!(matches!(events.last(), Some(JobEvent::Failed(_))));
    assert!(!events.iter().any(|e| matches!(e, JobEvent::Finished(_))));
}

#[tokio::test]
async fn cancel_propagates_to_cancelled_terminal() {
    let q = queue_with(
        vec![translate_provider("hang", MockScript::HangUntilCancel)],
        2,
    );
    let handle = q.submit(translate_job());
    tokio::time::sleep(Duration::from_millis(15)).await;
    handle.cancel();
    let events = handle.collect().await;

    assert!(matches!(events.last(), Some(JobEvent::Cancelled)));
}

#[tokio::test]
async fn no_provider_yields_failed_not_panic() {
    let q = queue_with(vec![], 2);
    let events = q.submit(translate_job()).collect().await;

    assert!(matches!(events.first(), Some(JobEvent::Queued)));
    assert!(matches!(events.last(), Some(JobEvent::Failed(_))));
}

#[tokio::test]
async fn strict_local_rejects_when_only_cloud() {
    let cloud = Arc::new(MockProvider::new(
        "cloud",
        capability_set(&[Capability::Translate], ProviderLocation::Cloud),
        MockScript::TextResult("from cloud".into()),
    ));
    let q = queue_with(vec![cloud], 2);

    let mut job = translate_job();
    job.routing = RoutingHint {
        local_only: true,
        ..Default::default()
    };
    let events = q.submit(job).collect().await;

    // 严格本地模式下拒绝外发 → Failed（路由），绝不静默上云。
    assert!(matches!(
        events.last(),
        Some(JobEvent::Failed(JobError::StrictLocalBlocked {
            capability: Capability::Translate
        }))
    ));
    assert!(!events.iter().any(|e| matches!(e, JobEvent::Finished(_))));
}
