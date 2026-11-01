//! 准入分层 + 卸载方向 + 等待可见性的守护用例（ADR-078），外加**被准入挡在门外的那些
//! Job 有没有把会话登记还回去**（ADR-081，用例 6）——那两条出口正是本文件里的准入拒绝
//! 与路由失败，跑的也是同一套真队列，分成两个文件只会让「谁在拒、拒完谁收尾」被割开。
//!
//! 现场那一幕：**批式二次转写进行期间，实时字幕/实时转写无法工作**。全进程只有一个信号量、
//! 四个额度，四类消费者（实时主轨 ASR / 实时副轨 ASR / 实时转译 / 批式二次转写）全是
//! `Priority::Normal`，Both 档一场会话自己就占 3 个 —— 零余量。而流式 ASR 的额度是在
//! `execute()` **之前**发的（远程实时转写的 WebSocket 在 `execute()` 里才连），于是排队
//! 直接等于失声：一条 `session.start` 都没有，采集侧照常投窗、窗队列填满、逐窗丢弃。
//!
//! 本文件的每条用例都能反向验证：把缺陷放回去（两条泳道合回一个信号量 / 卸载方向调回实时侧
//! / 把等待信号删掉）对应的那一条必然变红。
//!
//! **确定性**：不依赖真机、不依赖墙钟精度。断言的对象是**事件**而不是 stdout——
//! `tracing` 的 callsite interest 是进程全局缓存的，靠抓日志断言的用例会在并行测试下
//! 偶发失败（本周在服务端仓踩过）。这里连日志断言都不需要：等待与拒绝都有 typed event。

use luna_audio::{DegradationState, SheddingTarget};
use luna_core::{
    AIProvider, AllowOutbound, AudioRef, CancelToken, Capability, CapabilitySet, Job, JobError,
    JobEvent, JobEventStream, JobKind, JobOrigin, JobResult, Lang, PcmBuffer, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, StreamId, StreamRelease,
    StreamingAsr, TextRef, Timestamp, Timings, Usage, WindowInput,
};
use luna_jobqueue::{
    AdmissionCapacity, AsrLoadGauge, JobHandle, JobQueue, JobSink, STREAM_ADMISSION_LIMIT,
    STREAM_ADMISSION_NOTICE,
};
use luna_provider::asr_realtime::{
    RealtimeAsrConfig, RealtimeAsrProvider, REALTIME_ASR_PROVIDER_ID,
};
use luna_provider::asr_remote::BearerSource;
use luna_provider::ProviderRegistry;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;

/// 放行闸：**锁存**语义，不是一次性唤醒。
///
/// 用 `watch<bool>` 而不是 `Notify::notify_waiters()`：后者只唤醒**此刻正在等**的人，
/// 而本文件里恰恰有一条用例是「堵塞者放行之后，排在后面的那件才被准入并开始等」——
/// 它会永远等在一个早已发生过的通知上。这不是理论风险，是这条用例第一次跑就挂死了 60 秒。
#[derive(Clone)]
struct Release(watch::Sender<bool>);

impl Release {
    fn new() -> Self {
        Self(watch::channel(false).0)
    }

    fn open(&self) {
        let _ = self.0.send(true);
    }

    async fn wait(&self) {
        let mut rx = self.0.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

// ─── 受控 Provider ────────────────────────────────────────────────────────────

/// `execute` 一被调用就往 `entered` 报一声，然后**停在那里**直到 `release` 被通知。
///
/// 为什么不用现成的 `MockScript::HangUntilCancel`：那个脚本挂住之后**没有任何对外可见的
/// 信号**（队列在首个实质事件之前不转发 `Started`），于是「堵塞者是否真的已经占住额度」
/// 只能靠 sleep 去猜——而靠 sleep 排序的并发用例正是 flaky 的标准配方。这里用一个显式的
/// 「我进来了」通道把顺序钉死。
struct GatedProvider {
    id: ProviderId,
    caps: CapabilitySet,
    entered: UnboundedSender<ProviderId>,
    release: Release,
}

impl GatedProvider {
    fn register(
        id: &str,
        caps: &[Capability],
        entered: UnboundedSender<ProviderId>,
        release: Release,
    ) -> Arc<dyn AIProvider> {
        Arc::new(Self {
            id: ProviderId::from(id),
            caps: CapabilitySet {
                capabilities: caps.iter().copied().collect::<BTreeSet<_>>(),
                streaming: true,
                location: ProviderLocation::Device,
                explicit_only: false,
            },
            entered,
            release,
        })
    }
}

#[async_trait::async_trait]
impl AIProvider for GatedProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.clone(),
            display_name: self.id.as_str().to_string(),
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

    fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
        None
    }

    async fn execute(&self, job: Job, _cancel: CancelToken) -> JobEventStream {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let id = self.id.clone();
        let job_id = job.id;
        Box::pin(async_stream::stream! {
            yield JobEvent::Started;
            let _ = entered.send(id.clone());
            release.wait().await;
            yield JobEvent::Finished(JobResult {
                job_id,
                provider_id: id,
                model_id: None,
                payload: ResultPayload::Text(String::new()),
                usage: Usage::default(),
                timings: Timings::default(),
                output_truncated: None,
                finished_at: Timestamp::ZERO,
            });
        })
    }
}

// ─── 装配 ─────────────────────────────────────────────────────────────────────

struct Harness {
    queue: Arc<JobQueue>,
    entered: UnboundedReceiver<ProviderId>,
    release: Release,
}

fn harness(capacity: AdmissionCapacity) -> Harness {
    let (entered_tx, entered) = unbounded_channel();
    let release = Release::new();
    let registry = ProviderRegistry::new();
    registry.register(GatedProvider::register(
        "asr",
        &[Capability::Transcribe],
        entered_tx.clone(),
        release.clone(),
    ));
    registry.register(GatedProvider::register(
        "llm",
        &[Capability::Translate],
        entered_tx,
        release.clone(),
    ));
    Harness {
        queue: Arc::new(JobQueue::new(Arc::new(registry), capacity)),
        entered,
        release,
    }
}

fn stream_asr_job() -> Job {
    stream_job_on(StreamId::new(), "asr")
}

/// 同上，但「哪条流、钉在谁身上」由调用方给——采集侧的真实顺序是**先 `register_stream`
/// 拿句柄、再 `submit`**，于是那条流的 id 在建 Job 之前就已经存在了。
fn stream_job_on(stream_id: StreamId, provider: &str) -> Job {
    let mut job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Stream(stream_id),
            lang_hint: None,
            initial_prompt: None,
        },
        JobOrigin::Recording,
    );
    // 流式会话必须钉在具体 Provider 实例上（`Router::resolve` 的流式分支）。
    job.routing.preferred_provider = Some(ProviderId::from(provider));
    job
}

fn batch_asr_job() -> Job {
    Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Pcm(PcmBuffer {
                sample_rate: 16_000,
                channels: 1,
                samples: vec![0.0; 16],
            }),
            lang_hint: None,
            initial_prompt: None,
        },
        JobOrigin::SecondPass,
    )
}

fn translate_job(origin: JobOrigin) -> Job {
    Job::new(
        JobKind::Translate {
            text: TextRef::Inline("hi".into()),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        origin,
    )
}

/// 等一件 Job 真正进到 Provider 里（= 它已经拿到额度）。
///
/// 上界给到 2 秒纯粹是防挂死：修好之后这一步在微秒级完成，而缺陷在场时它**永远**不会
/// 发生——两者相差好几个数量级，这个上界不构成竞态。
async fn expect_entered(entered: &mut UnboundedReceiver<ProviderId>, who: &str) {
    let got = tokio::time::timeout(Duration::from_secs(2), entered.recv())
        .await
        .unwrap_or_else(|_| panic!("{who} 没能拿到准入额度——它被另一条泳道堵住了"));
    assert!(got.is_some(), "{who}：Provider 通道意外关闭");
}

/// 断言某件 Job 在给定时间内**没有**拿到准入（用于「池满时确实挡住了」的正向确认）。
async fn expect_not_entered(entered: &mut UnboundedReceiver<ProviderId>) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), entered.recv())
            .await
            .is_err(),
        "本该被闸挡住的 Job 却拿到了额度"
    );
}

/// 在**远大于任何准入阈值**的上界内取下一个事件。
///
/// 上界只是「缺陷在场时给出一条说得清的失败」的手段，不参与任何正向判定：`start_paused`
/// 下时钟由最早的那个定时器驱动，准入阈值恒早于这个上界，所以正向路径永远不会碰到它。
async fn expect_soon(handle: &mut JobHandle) -> Option<JobEvent> {
    tokio::time::timeout(STREAM_ADMISSION_LIMIT * 60, handle.recv())
        .await
        .expect("等了六十倍宽限期都没有下一个事件——准入等待这条链整个没接上")
}

// ─── 用例 1：批式在跑时，流式 ASR 立即获得准入 ────────────────────────────────

/// **现场那一幕的直接回归。**
///
/// 计算池只有 1 个额度、且已被一件批式二次转写占满；此时提交一条流式 ASR Job。
/// 修好之后它走会话池，与批式毫无关系 → 立刻进来。
///
/// 把缺陷放回去（两条泳道合回一个信号量）：这条会一直等到批式结束——而批式是几分钟。
#[tokio::test]
async fn a_stream_job_is_admitted_while_a_batch_transcription_holds_the_compute_lane() {
    let mut h = harness(AdmissionCapacity {
        compute: 1,
        session: 2,
    });

    let _batch = h.queue.submit(batch_asr_job());
    expect_entered(&mut h.entered, "批式二次转写").await;
    // 计算池此刻确实满了（不是「碰巧没人跟它抢」）。
    let _second_batch = h.queue.submit(batch_asr_job());
    expect_not_entered(&mut h.entered).await;

    let _stream = h.queue.submit(stream_asr_job());
    expect_entered(&mut h.entered, "实时 ASR 腿").await;

    h.release.open();
}

// ─── 用例 2：两条泳道互不饥饿 ─────────────────────────────────────────────────

/// 计算池被填满 → 流式不受影响。
#[tokio::test]
async fn a_full_compute_lane_does_not_starve_the_session_lane() {
    let mut h = harness(AdmissionCapacity {
        compute: 2,
        session: 1,
    });

    // 用「有人在等结果」的计算 Job（弹窗翻译）填满计算池——刻意不用批式，
    // 以证明这条隔离与卸载闸无关，是准入分层本身给的。
    let _a = h.queue.submit(translate_job(JobOrigin::PopupTranslate));
    let _b = h.queue.submit(translate_job(JobOrigin::PopupTranslate));
    expect_entered(&mut h.entered, "计算池 Job #1").await;
    expect_entered(&mut h.entered, "计算池 Job #2").await;
    let _c = h.queue.submit(translate_job(JobOrigin::PopupTranslate));
    expect_not_entered(&mut h.entered).await;

    let _stream = h.queue.submit(stream_asr_job());
    expect_entered(&mut h.entered, "实时 ASR 腿").await;

    h.release.open();
}

/// 会话池被填满 → 批式不受影响（反方向同样成立，否则只是把堵塞挪了个方向）。
#[tokio::test]
async fn a_full_session_lane_does_not_starve_the_compute_lane() {
    let mut h = harness(AdmissionCapacity {
        compute: 2,
        session: 1,
    });

    let _stream = h.queue.submit(stream_asr_job());
    expect_entered(&mut h.entered, "实时 ASR 腿").await;
    let _stream2 = h.queue.submit(stream_asr_job());
    expect_not_entered(&mut h.entered).await;

    let _batch = h.queue.submit(batch_asr_job());
    expect_entered(&mut h.entered, "批式二次转写").await;

    h.release.open();
}

// ─── 用例 3：高负载先卸批式，实时窗照常解码 ──────────────────────────────────

/// **被节流的必须是批式。**
///
/// 闸一关，批式 ASR 的**准入**就停下；同一时刻，会话池的腿与计算池里有人在等的活
/// 全都照常放行。档位语义（Yellow/Orange 跳快照）一个字没改——改的是「谁先买单」，
/// 那一半的判定在 `luna_audio::degradation` 的单测里逐档钉住。
#[tokio::test]
async fn high_load_sheds_the_batch_lane_and_leaves_the_live_legs_running() {
    let mut h = harness(AdmissionCapacity {
        compute: 4,
        session: 3,
    });
    let load = h.queue.asr_load();

    // 实时会话判定「该卸批式」→ 扳闸。
    load.set_batch_admission_paused(true);

    let _batch = h.queue.submit(batch_asr_job());
    expect_not_entered(&mut h.entered).await;

    // 闸关着的同时：流式腿照进、实时转译照进、有人在等的计算 Job 也照进。
    let _stream = h.queue.submit(stream_asr_job());
    expect_entered(&mut h.entered, "实时 ASR 腿").await;
    let _live_translate = h.queue.submit(translate_job(JobOrigin::Recording));
    expect_entered(&mut h.entered, "实时转译").await;
    let _popup = h.queue.submit(translate_job(JobOrigin::PopupTranslate));
    expect_entered(&mut h.entered, "弹窗翻译").await;

    // 负载回落 → 闸开，批式自己接上（不需要任何新的提交来唤醒调度循环）。
    load.set_batch_admission_paused(false);
    expect_entered(&mut h.entered, "闸开后的批式二次转写").await;

    h.release.open();
}

/// **整条闭环：批式在跑 ⇒ 实时快照窗照常解码，被停的是批式。**
///
/// 队列如实报「有几件批式在飞」→ `RealtimeSession`（唯一持有降级控制器的地方）据此裁决
/// →裁决落回队列的批式准入闸。缺陷在场时这条链根本不存在：控制器只看得见被批式推高的
/// RTF，于是升到 Yellow/Orange 就跳掉**全部**快照窗——用户点了「二次转写」，代价是同一
/// 时刻的实时字幕停更。
#[tokio::test]
async fn a_running_second_pass_never_costs_the_live_captions_their_snapshot_windows() {
    let mut h = harness(AdmissionCapacity {
        compute: 2,
        session: 2,
    });
    let load = h.queue.asr_load();
    let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel::<u64>(16);
    let (mut session, _deg_rx) =
        luna_jobqueue::RealtimeSession::new(None, CancelToken::new(), Some(infer_rx), load.clone());

    // 现场那一幕的前提：一件批式二次转写正在跑。
    let _batch = h.queue.submit(batch_asr_job());
    expect_entered(&mut h.entered, "批式二次转写").await;
    assert_eq!(load.batch_inflight(), 1, "队列应如实报出在飞的批式件数");

    // 实时腿的 RTF 被推高（滑窗 5 点，喂满）。
    for _ in 0..6 {
        infer_tx.send(900).unwrap();
        session.on_audio_window(1000.0);
    }

    assert_ne!(
        session.current_tier(),
        DegradationState::Green,
        "面向 UI 的档位应如实反映「这台机器吃不消」"
    );
    assert_eq!(
        session.shedding_target(),
        SheddingTarget::BatchAsr,
        "先卸的必须是没有截止期的那一份活"
    );
    assert_eq!(
        session.realtime_decode_tier(),
        DegradationState::Green,
        "还有批式可卸时，实时快照窗必须照常解码——这正是「二次转写期间字幕停更」的根因"
    );
    assert!(
        load.batch_admission_paused(),
        "裁决必须真的落到队列的批式准入闸上，否则只是一个没人读的档位"
    );

    // 批式让路之后负载仍不降 → 才轮到实时侧按原档位语义跳窗。
    h.release.open();
    while load.batch_inflight() > 0 {
        tokio::task::yield_now().await;
    }
    for _ in 0..6 {
        infer_tx.send(900).unwrap();
        session.on_audio_window(1000.0);
    }
    assert_eq!(
        session.shedding_target(),
        SheddingTarget::RealtimeWindows,
        "批式已经卸干净，负载还是这台机器自己的"
    );
    assert_eq!(
        session.realtime_decode_tier(),
        session.current_tier(),
        "此时实时侧按原档位语义跳窗（Yellow/Orange 跳快照、Red 只放探测窗），语义一个字没改"
    );
    assert!(
        !load.batch_admission_paused(),
        "没有批式可卸时不该继续关着批式的闸"
    );
}

/// 卸载闸的归还必须由 `RealtimeSession` 的 `Drop` 保证——会话可能因为 part 收尾、
/// 提前 return 或 panic 展开而被直接 drop，而一个没人归还的闸意味着用户**此后再也
/// 跑不动一次二次转写**，且界面上没有任何东西会说明为什么。
#[tokio::test]
async fn dropping_a_realtime_session_always_reopens_the_batch_gate() {
    let load = AsrLoadGauge::new();
    let (session, _rx) =
        luna_jobqueue::RealtimeSession::new(None, CancelToken::new(), None, load.clone());
    load.set_batch_admission_paused(true);
    drop(session);
    assert!(
        !load.batch_admission_paused(),
        "会话已经没了，批式闸却还关着——用户此后再也跑不动一次二次转写"
    );
}

// ─── 用例 4：等待超阈值必须说出口，再放弃 ────────────────────────────────────

/// **回归防哑。**
///
/// `JobEvent::Queued` 此前是哑的：`recording.rs` 的事件臂根本没有它的分支，不打日志、
/// 不发 UI 事件、无超时——「实时转写在等一个永远不会来的额度」与「一切正常」在任何一处
/// 都无法区分。这条钉住两件事：等待到点**先说出口**（`AdmissionDelayed`），再到点**才拒**
/// （`LocalSessionBusy`，可行动的错误码；录音与落盘不受影响）。
///
/// 时间用 `start_paused`：阈值是十几秒量级，靠真实等待既慢又是竞态。
#[tokio::test(start_paused = true)]
async fn a_stream_job_that_cannot_get_in_says_so_before_it_gives_up() {
    let mut h = harness(AdmissionCapacity {
        compute: 2,
        session: 1,
    });

    let _holder = h.queue.submit(stream_asr_job());
    expect_entered(&mut h.entered, "先占住会话额度的那条腿").await;

    let mut waiting: JobHandle = h.queue.submit(stream_asr_job());
    assert_eq!(
        waiting.recv().await,
        Some(JobEvent::Queued),
        "提交回执恒发一次（§5.3 事件流提交即绑定）"
    );

    // 说出口：`waited_ms` 只有时长，无任何内容（脱敏铁律）。
    //
    // 每一步都套一个远大于阈值的上界，只为**让缺陷失败而不是挂死**：把等待信号删掉
    // （或把流式 Job 塞回计算池，那里根本没有等待时钟）时，这里再也不会有任何定时器，
    // 而 `start_paused` 的自动推进需要有定时器才推得动——不套上界的话这条用例的表现是
    // 「跑了 60 秒还在跑」，而不是一条说得清的失败。挂死的守护用例只会被人加 `--skip`。
    match expect_soon(&mut waiting).await {
        Some(JobEvent::AdmissionDelayed { waited_ms }) => assert!(
            waited_ms >= STREAM_ADMISSION_NOTICE.as_millis() as u64,
            "报出来的等待时长比阈值还短：{waited_ms}ms"
        ),
        other => panic!("等待超阈值必须产出可见事件，实际是 {other:?}"),
    }

    // 再等一个窗仍进不来 → 拒，且是可行动的稳定错误码。
    match expect_soon(&mut waiting).await {
        Some(JobEvent::Failed(err)) => {
            assert_eq!(err.code(), "LOCAL_SESSION_BUSY");
            assert!(
                !err.is_retriable(),
                "额度不会因为再问一次就出现——自动重试只会再排一次同一条队"
            );
            assert!(
                matches!(err, JobError::LocalSessionBusy { waited_ms }
                    if waited_ms >= STREAM_ADMISSION_LIMIT.as_millis() as u64),
                "放弃时报的等待时长必须到达放弃阈值：{err:?}"
            );
        }
        other => panic!("等满宽限期必须以可行动的错误终态收尾，实际是 {other:?}"),
    }
    assert_eq!(waiting.recv().await, None, "终态之后事件通道应关闭");

    h.release.open();
}

/// 计算池里排队是常态（整批导入本来就是「入队时人已经走开了」），**不得**产出等待事件、
/// 更不得被这套宽限期拒掉——否则一次批量导入会被自己的排队判成失败。
#[tokio::test(start_paused = true)]
async fn queueing_in_the_compute_lane_is_normal_and_stays_silent() {
    let mut h = harness(AdmissionCapacity {
        compute: 1,
        session: 2,
    });

    let _holder = h.queue.submit(batch_asr_job());
    expect_entered(&mut h.entered, "先占住计算额度的那件").await;

    let mut queued = h.queue.submit(translate_job(JobOrigin::Batch));
    assert_eq!(queued.recv().await, Some(JobEvent::Queued));

    // 让时间走过两个阈值：计算池的 Job 一声不吭地继续排。
    tokio::time::advance(STREAM_ADMISSION_LIMIT * 3).await;
    h.release.open();

    // 堵塞者放行后它正常跑完——中间不该夹着任何 `AdmissionDelayed` / `Failed`。
    let mut saw_terminal = false;
    while let Some(ev) = queued.recv().await {
        match ev {
            JobEvent::AdmissionDelayed { .. } => panic!("计算池排队被报成了异常"),
            JobEvent::Failed(err) => panic!("计算池排队被判成了失败：{err:?}"),
            JobEvent::Finished(_) => saw_terminal = true,
            _ => {}
        }
    }
    assert!(saw_terminal, "堵塞者放行后计算池 Job 应正常收尾");
}

// ─── 用例 5：现场那一幕的完整形状 ────────────────────────────────────────────

/// **Both 档两轨 + 实时转译 + 批式二次转写同时在场，四者都要能推进。**
///
/// 这正是用户报告的那一幕的完整配置。缺陷在场时：一个信号量、4 个额度，实时会话自己占 3、
/// 批式占 1，零余量；而额度是在 `execute()` 之前发的，所以先到的那一批把后到的**永久**挡在
/// 门外。分池之后 3 条会话腿走会话池、批式走计算池，互不相干。
///
/// 容量刻意用生产同款推导（`for_capture_tracks(2)` = 会话 3 / 计算 4），而不是为了让用例
/// 通过而放大的数字——用例要证的就是**生产配置**下这四者能共存。
#[tokio::test]
async fn both_tracks_live_translation_and_a_second_pass_all_make_progress_together() {
    let mut h = harness(AdmissionCapacity::for_capture_tracks(2));

    let _mic_track = h.queue.submit(stream_asr_job());
    let _system_track = h.queue.submit(stream_asr_job());
    let _live_translate = h.queue.submit(translate_job(JobOrigin::Recording));
    let _second_pass = h.queue.submit(batch_asr_job());

    for who in ["主轨实时 ASR", "副轨实时 ASR", "实时转译", "批式二次转写"] {
        expect_entered(&mut h.entered, who).await;
    }

    h.release.open();
}

// ─── 用例 6：走不到 `execute` 的 Job 必须把会话登记还回去 ─────────────────────

/// 一枚**不会通过**的凭据：万一 Job 真的跑进了 `execute`（= 用例前提被破坏），这条腿会
/// 以 `Unauthorized` 当场收场，**一个字节都不发**（`without_credentials_nothing_is_sent`
/// 钉的就是这条）。用例因此不可能碰到网络。
struct NoCredentials;

#[async_trait::async_trait]
impl BearerSource for NoCredentials {
    async fn bearer(&self) -> Option<String> {
        None
    }
    async fn refresh(&self, _rejected: &str) -> Option<String> {
        None
    }
}

/// 装一条**真的**流式 ASR Provider（登记表在它自己里面），外加一个占位用的闸控 Provider。
///
/// `provider_registered = false` = 会话所属的那条 Provider **不在注册表里**，也就是
/// 「模型被换掉 / 服务端转写开关被关」在路由层的形状。
fn streaming_harness(
    capacity: AdmissionCapacity,
    provider_registered: bool,
) -> (
    Arc<JobQueue>,
    Arc<RealtimeAsrProvider>,
    UnboundedReceiver<ProviderId>,
    Release,
) {
    let (entered_tx, entered) = unbounded_channel();
    let release = Release::new();
    let registry = ProviderRegistry::new();
    registry.register(GatedProvider::register(
        "asr",
        &[Capability::Transcribe],
        entered_tx,
        release.clone(),
    ));
    let streaming = Arc::new(RealtimeAsrProvider::new(RealtimeAsrConfig {
        // 保留地址段（RFC 5737）：连凭据都没有，这条腿走不到网络；地址也不可达。
        api_base: "http://192.0.2.1:9".into(),
        bearer: Arc::new(NoCredentials),
        gate: Arc::new(AllowOutbound),
    }));
    if provider_registered {
        registry.register(Arc::clone(&streaming) as Arc<dyn AIProvider>);
    }
    (
        Arc::new(JobQueue::new(Arc::new(registry), capacity)),
        streaming,
        entered,
        release,
    )
}

fn one_window() -> Option<WindowInput> {
    Some(WindowInput {
        samples: vec![0.0; 16],
        abs_start: 0,
        speech_len: 16,
    })
}

/// 等这件 Job 的终态（中间的 `Queued` / `AdmissionDelayed` 一路吞掉）。
async fn terminal_of(handle: &mut JobHandle) -> JobEvent {
    loop {
        match expect_soon(handle).await {
            Some(event @ (JobEvent::Failed(_) | JobEvent::Finished(_) | JobEvent::Cancelled)) => {
                break event
            }
            Some(_) => {}
            None => panic!("事件通道在终态之前就关了"),
        }
    }
}

/// **被会话池拒掉的流式 Job，必须把会话登记还回去。**
///
/// 采集侧是**先 `register_stream` 拿句柄、再 `submit(job)`**，而登记此前只由 `execute`
/// 摘走——于是准入到期即拒（ADR-078 ③，「停录后立刻再录」正是它要处理的瞬态）这条出口
/// 上，登记永久滞留。代价不是一个空 map 条目：条目攥着 `window_rx` 那一端，而采集侧在
/// Job 失败之前一直在投窗（通道深 64、单窗最大 ~10 秒 16 kHz f32 = 640 KB，故一条最多
/// 约 40 MB），并且流式 Provider 实例在形状未变时是复用的 —— 这些条目活到进程退出，
/// 一次一次叠加。
///
/// 断言的是**缓冲真的被释放**（再投窗得到 `Disconnected`），而不只是 map 少了一条：
/// `std::sync::mpsc` 的缓冲随**接收端**被丢才释放，只丢发送端的话那 64 窗仍好端端挂着。
///
/// 反向验证（已实跑，两个方向）：① 把采集侧那句 `drop(stream_release.take())` 去掉
/// ——句柄仍握在手里，末尾那条断言即红；② 把回收路径本身停掉（`StreamRelease` 的 `Drop`
/// 清空 = 回到「只有 `execute` 会 remove」），同样红，且三处实现的单测一并红。
#[tokio::test(start_paused = true)]
async fn a_stream_job_the_session_lane_rejects_gives_its_registration_back() {
    let (queue, streaming, mut entered, release) = streaming_harness(
        AdmissionCapacity {
            compute: 2,
            session: 1,
        },
        true,
    );

    // 唯一那个会话额度先被占住。
    let _holder = queue.submit(stream_asr_job());
    expect_entered(&mut entered, "先占住会话额度的那条腿").await;

    // 采集侧：拿句柄 → 提交 Job → 一路投窗。
    let stream_id = StreamId::new();
    let reg = streaming.register_stream(stream_id);
    let mut stream_release: Option<StreamRelease> = Some(reg.release);
    let mut victim = queue.submit(stream_job_on(stream_id, REALTIME_ASR_PROVIDER_ID));
    reg.window_tx
        .try_send(one_window())
        .expect("登记还在，投窗应成功");

    match terminal_of(&mut victim).await {
        JobEvent::Failed(err) => assert_eq!(
            err.code(),
            "LOCAL_SESSION_BUSY",
            "本用例要的是「准入拒绝」这条出口，实际是 {err:?}"
        ),
        other => panic!("等满宽限期应以 LocalSessionBusy 收场，实际是 {other:?}"),
    }

    // 采集侧收到终态 → 注销登记（`recording.rs` / `voice_input.rs` 的那一步）。
    drop(stream_release.take());

    assert!(
        matches!(
            reg.window_tx.try_send(one_window()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        ),
        "登记连同它攥着的窗缓冲滞留在 Provider 里——而 execute 永远不会再来摘它"
    );

    release.open();
}

/// 同一条不变量的另一条出口：**会话所属 Provider 掉线**（模型被换掉、服务端转写
/// 开关被关）时 `Router::resolve` 在 `execute` 之前就返回失败。
///
/// 与准入那条分开测：两者是 `run_job` 里两个不同的提前返回点，一条修好不代表另一条也修
/// 好，而它们在采集侧长得一模一样（都只是一个终态事件）。
#[tokio::test]
async fn a_stream_job_whose_provider_went_offline_gives_its_registration_back() {
    // 会话所属的 Provider **不进注册表**：那正是「掉线」在路由层的形状。
    let (queue, streaming, _entered, _release) = streaming_harness(
        AdmissionCapacity {
            compute: 2,
            session: 2,
        },
        false,
    );

    let stream_id = StreamId::new();
    let reg = streaming.register_stream(stream_id);
    let mut stream_release: Option<StreamRelease> = Some(reg.release);
    let mut victim = queue.submit(stream_job_on(stream_id, REALTIME_ASR_PROVIDER_ID));
    reg.window_tx
        .try_send(one_window())
        .expect("登记还在，投窗应成功");

    match terminal_of(&mut victim).await {
        JobEvent::Failed(JobError::ProviderUnavailable { .. }) => {}
        other => panic!("会话所属 Provider 不在时应以 ProviderUnavailable 收场，实际是 {other:?}"),
    }

    drop(stream_release.take());

    assert!(
        matches!(
            reg.window_tx.try_send(one_window()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        ),
        "路由失败这条出口上，登记连同窗缓冲滞留在 Provider 里"
    );
}
