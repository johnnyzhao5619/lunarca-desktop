//! JobQueue 调度核心。

use futures::StreamExt;
use luna_audio::{SegmenterConfig, TARGET_SAMPLE_RATE};
use luna_core::{
    AdmissionLane, CancelToken, Job, JobError, JobEvent, JobId, TerminalAttribution, Timestamp,
    Usage, UsageRecord,
};
use luna_provider::{ProviderRegistry, RouteError, Router};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{watch, Semaphore};

// ─── 准入容量（ADR-078 ①）──────────────────────────────────────────────────────

/// 计算池下面的**引擎条数**：进程内 whisper（ADR-010 专用阻塞线程池）+ 进程外随包引擎。
///
/// 计算池的额度**不随 CPU 核数走**，这一点是刻意的：whisper 那台引擎自己的线程池已经把
/// 「核数」这一维吃掉了（ADR-010 边界，本批不动它的线程策略），外层再按核数开一遍并发就是
/// 平方级过订阅；随包引擎那边更直接——多发几个并发请求只会排在它自己的槽位队列里，一点吞吐都
/// 换不到，只换来更长的首 token 延迟。真正决定这个数的是**有几台引擎**。
const COMPUTE_ENGINES: usize = 2;

/// 每台引擎的在途额度：**一条在算 + 一条已就绪等着接班**。
///
/// 第二条不是冗余：Job 的准入发生在路由与 `execute()` **之前**，一个 Job 从拿到额度到真正
/// 开始烧 CPU 之间还隔着路由解析、模型加载检查、（远程时）建连——只留一条的话，引擎每算完
/// 一件都要空转等下一件走完这段路。
const INFLIGHT_PER_ENGINE: usize = 2;

/// 实时转译在一条会话里的在途 Job 数上限。
///
/// **由实现直接决定，不是拍的**：`RealtimeTranslationPipeline::spawn` 起的是**单个**串行
/// worker，它 `await` 完上一句才提交下一句——所以任一时刻最多一条在飞。双源会话的两条轨
/// 共用这一个 worker（句界缓冲按轨分，提交仍是串行的），故这个数与轨数无关、恒为 1。
const REALTIME_TRANSLATE_LANES: usize = 1;

/// 实时路径的**首窗延迟上界**：连续说话时最坏情况下多久必定产出一个窗。
///
/// 直接取自分段器配置而不是另写一个秒数——窗长上限这一周刚从 30 s 改成 ~10 s，任何在别处
/// 复写的秒数都会静默陈旧（且不会有任何东西变红）。
const FIRST_WINDOW_LIMIT: Duration = SegmenterConfig::new(TARGET_SAMPLE_RATE).window_limit();

/// 流式 Job 等会话额度**等到该说出口**的阈值（ADR-078 ③）。
///
/// # 推导
///
/// 取 **1 × [`FIRST_WINDOW_LIMIT`]**。在这段时间之内，采集侧连第一个窗都还没产出，解码腿
/// 就算此刻已经就绪也无事可做——这段等待对用户**在构造上不可观测**，报出来只会变成一条
/// 每次开始录音都闪一下的噪声（而噪声横幅的下场就是被无视，`transcribe_stopped` 那条已经
/// 付过一次这个学费）。跨过它就完全不同了：采集侧手上至少有一个窗而解码腿还没开张，从这
/// 一刻起每多等一秒都是永久丢失的实时字幕——这正是「等待」变成「缺陷」的那个点。
pub const STREAM_ADMISSION_NOTICE: Duration = FIRST_WINDOW_LIMIT;

/// 流式 Job 等会话额度的**放弃**阈值（ADR-078 ①）。
///
/// # 推导
///
/// 取 **2 × [`FIRST_WINDOW_LIMIT`]** —— 越过说出口阈值之后，再给这条腿整整一个窗的时间把
/// 瞬态耗完（最常见的瞬态是「停录→立刻再录」：上一场的 whisper 解码在 cancel 之后还要一小
/// 会儿才收干净）。到了这里还没进来，等待就不再是瞬态而是结构性的，第三个窗也不会改变
/// 答案。
///
/// 为什么是**拒**而不是继续排队：用户此刻按下的是录音，他要的是**立刻开始**，而不是几分钟
/// 后。一条几分钟后才拿到额度的流式 ASR Job 已经毫无价值——它该转写的那段音频早就过去了，
/// 而它还会占着会话额度不放。拒的代价被限制在**这一条腿**：录音本身照常进行、音频照常
/// 落盘，停录后的二次转写补齐权威结果（PRD-04-030），用户拿到的是一条可行动的错误
/// （`LOCAL_SESSION_BUSY`）而不是一场静默的空录。
pub const STREAM_ADMISSION_LIMIT: Duration =
    Duration::from_millis(FIRST_WINDOW_LIMIT.as_millis() as u64 * 2);

/// 两条准入泳道各自的额度（[`AdmissionLane`]，ADR-078 ①）。
///
/// **一个 `JobQueue`、一个 dispatcher、两个闸门**——不是两个队列。优先级排序、取消传播、
/// 兜底路由、用量记账全都只有一份；分开的只有「向谁要额度」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionCapacity {
    /// 计算池：领一份活、算完就还的重计算单元。
    pub compute: usize,
    /// 会话池：在飞的实时会话的腿（流式 ASR 轨 + 该会话的实时转译）。
    pub session: usize,
}

impl AdmissionCapacity {
    /// 计算池的额度 = 引擎条数 × 每台引擎的在途额度（推导见 [`COMPUTE_ENGINES`]）。
    #[must_use]
    pub const fn compute_default() -> usize {
        COMPUTE_ENGINES * INFLIGHT_PER_ENGINE
    }

    /// 按**采集轨数上限**派生会话池额度。
    ///
    /// `max_capture_tracks` 由采集源枚举给出（双轨会议 = 麦克风 + 系统回环 = 2），
    /// 而麦克风硬互斥（TDD-05 §5.3）保证同一时刻只有一路采集会话——两者合起来就是
    /// 全进程的流式 ASR 腿上限。再加 [`REALTIME_TRANSLATE_LANES`] 条给实时转译。
    ///
    /// **不写死**：加一种采集源（比如第三条轨）时该跟着变的是传进来的这个数，
    /// 而不是某处一个需要有人记得改的常量。
    #[must_use]
    pub const fn for_capture_tracks(max_capture_tracks: usize) -> Self {
        Self {
            compute: Self::compute_default(),
            session: max_capture_tracks + REALTIME_TRANSLATE_LANES,
        }
    }
}

// ─── 本机 ASR 负载探针（ADR-078 ②）──────────────────────────────────────────────

/// 批式 ASR 的在飞计数 + 准入闸：**队列记，实时编排层读并扳闸**。
///
/// 这是 TDD-02 §5.2「决策归属」的接缝：`JobQueue` 只知道「有几件批式转写在飞」这个事实，
/// 由谁买单的裁决在 `RealtimeSession`（唯一持有 `DegradationController` 的地方）。
#[derive(Clone)]
pub struct AsrLoadGauge {
    inner: Arc<AsrLoadInner>,
}

struct AsrLoadInner {
    /// 在飞的批式 ASR Job 数。
    batch_inflight: AtomicUsize,
    /// 闸位的同步读（dispatcher 与调用方都可能在 `watch` 之外读它）。
    paused: AtomicBool,
    /// 闸位变化的唤醒（dispatcher 在 `select!` 里等它）。
    paused_tx: watch::Sender<bool>,
}

impl Default for AsrLoadGauge {
    fn default() -> Self {
        Self::new()
    }
}

impl AsrLoadGauge {
    /// 新建一个空闲探针。
    #[must_use]
    pub fn new() -> Self {
        let (paused_tx, _) = watch::channel(false);
        Self {
            inner: Arc::new(AsrLoadInner {
                batch_inflight: AtomicUsize::new(0),
                paused: AtomicBool::new(false),
                paused_tx,
            }),
        }
    }

    /// 此刻在飞的批式 ASR Job 数。
    ///
    /// **不区分跑在本机还是服务端**，这是刻意的：卸载与否的判据是「错过了还能不能补回来」，
    /// 而不是「跑在哪」。一件远程批式转写被推迟三十秒同样零损失，把它一并卸掉不会让任何人
    /// 少拿到一个字；而按落点区分则要等路由完成才知道答案，那时准入早就发生了。
    #[must_use]
    pub fn batch_inflight(&self) -> usize {
        self.inner.batch_inflight.load(AtomicOrdering::Relaxed)
    }

    /// 扳批式 ASR 的准入闸（`true` = 暂停放行新的批式 ASR Job）。
    pub fn set_batch_admission_paused(&self, paused: bool) {
        if self.inner.paused.swap(paused, AtomicOrdering::Relaxed) != paused {
            let _ = self.inner.paused_tx.send(paused);
        }
    }

    /// 批式 ASR 准入闸是否关着。
    #[must_use]
    pub fn batch_admission_paused(&self) -> bool {
        self.inner.paused.load(AtomicOrdering::Relaxed)
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.paused_tx.subscribe()
    }

    /// 计一件批式 ASR 上飞；返回的守卫落地即减。
    fn enter_batch(&self) -> BatchInflight {
        self.inner
            .batch_inflight
            .fetch_add(1, AtomicOrdering::Relaxed);
        BatchInflight {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// 在飞计数的 RAII 守卫——**必须是守卫而不是两处手写加减**：Job 有五条终态路径
/// （完成/失败/取消/路由失败/消费者提前走），漏掉任何一条都会把计数永久钉在高位，
/// 而那意味着实时会话此后一直以为「还有批式可卸」，于是它自己永远不降档。
struct BatchInflight {
    inner: Arc<AsrLoadInner>,
}

impl Drop for BatchInflight {
    fn drop(&mut self) {
        self.inner
            .batch_inflight
            .fetch_sub(1, AtomicOrdering::Relaxed);
    }
}

/// 提交 Job 的接口（GUI 与 CLI 共用同一 `JobSink`，TDD-01 §5.3）。
pub trait JobSink {
    /// 提交一个 Job，**同步返回**已绑定事件流的句柄（事件流在 Job 开始执行前就已就绪）。
    fn submit(&self, job: Job) -> JobHandle;
}

/// 用量记录接收端。具体持久化归属由上层实现，JobQueue 只负责终态投递。
pub trait UsageSink: Send + Sync {
    /// 记录一次终态用量。
    fn record(&self, record: UsageRecord);
}

/// 默认 no-op sink，保持队列 crate 与存储解耦。
#[derive(Debug, Default)]
pub struct NoopUsageSink;

impl UsageSink for NoopUsageSink {
    fn record(&self, _record: UsageRecord) {}
}

/// 已提交 Job 的句柄：订阅事件 + 取消。
pub struct JobHandle {
    id: JobId,
    events: UnboundedReceiver<JobEvent>,
    cancel: CancelToken,
}

impl JobHandle {
    /// Job id。
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// 触发取消（一路传播到 Provider 的 `CancelToken`，§5.5）。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 克隆取消令牌（供上层 session manager 在异步命令外触发停止）。
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// 取下一个事件（终态后通道关闭返回 `None`）。
    pub async fn recv(&mut self) -> Option<JobEvent> {
        self.events.recv().await
    }

    /// 收集直到流结束（测试便利）。
    pub async fn collect(mut self) -> Vec<JobEvent> {
        let mut out = Vec::new();
        while let Some(ev) = self.events.recv().await {
            out.push(ev);
        }
        out
    }
}

/// 入队的 Job（携带事件汇与序号，用于优先级 + FIFO 排序）。
struct QueuedJob {
    job: Job,
    cancel: CancelToken,
    events_tx: UnboundedSender<JobEvent>,
    seq: u64,
    /// 提交时刻。会话池的两个等待阈值都从这里算起——**从 `submit` 而不是从入堆**，
    /// 因为提交到入堆之间还隔着一次通道跳转，而用户感知的等待从他按下按钮就开始了。
    queued_at: tokio::time::Instant,
    /// 是否已经就「还在等」说过一次（去重：一条腿只报一次，不做每秒刷新的噪声）。
    admission_notified: bool,
}

impl QueuedJob {
    /// 本 Job 下一个该到期的等待时刻（会话池专用；`None` = 不受等待阈值管辖）。
    fn admission_deadline(&self) -> Option<tokio::time::Instant> {
        if self.job.admission_lane() != AdmissionLane::Session {
            // 计算池里排队是常态：批量导入本来就是「入队时人已经走开了」。
            return None;
        }
        Some(if self.admission_notified {
            self.queued_at + STREAM_ADMISSION_LIMIT
        } else {
            self.queued_at + STREAM_ADMISSION_NOTICE
        })
    }
}

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}
impl Eq for QueuedJob {}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // 优先级高者为"大"（BinaryHeap 是最大堆）；同优先级下 seq 小者为"大"（FIFO）。
        self.job
            .priority
            .cmp(&other.job.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Job 调度队列。
///
/// 必须在 tokio 运行时内构造（内部 spawn 调度循环）。
pub struct JobQueue {
    submit_tx: UnboundedSender<QueuedJob>,
    seq: AtomicU64,
    load: AsrLoadGauge,
    /// 路由策略句柄（与 worker 里的 `Router` 共享同一个原子，PRD-10-008）。
    route_strategy: luna_provider::RouteStrategyHandle,
}

impl JobQueue {
    /// 用一个 Provider 注册表与两条泳道的额度构造。
    #[must_use]
    pub fn new(registry: Arc<ProviderRegistry>, capacity: AdmissionCapacity) -> Self {
        Self::with_usage_sink(registry, capacity, Arc::new(NoopUsageSink))
    }

    /// 路由策略句柄（设置侧热切换预设，PRD-10-008）。
    #[must_use]
    pub fn route_strategy(&self) -> luna_provider::RouteStrategyHandle {
        self.route_strategy.clone()
    }

    /// 用一个 Provider 注册表、两条泳道的额度与用量 sink 构造。
    #[must_use]
    pub fn with_usage_sink(
        registry: Arc<ProviderRegistry>,
        capacity: AdmissionCapacity,
        usage_sink: Arc<dyn UsageSink>,
    ) -> Self {
        let (submit_tx, submit_rx) = mpsc::unbounded_channel();
        let router = Arc::new(Router::new(registry.clone()));
        let route_strategy = router.strategy_handle();
        let load = AsrLoadGauge::new();
        tokio::spawn(dispatcher(
            submit_rx,
            router,
            registry,
            capacity,
            usage_sink,
            load.clone(),
        ));
        Self {
            route_strategy,
            submit_tx,
            seq: AtomicU64::new(0),
            load,
        }
    }

    /// 本机 ASR 负载探针（实时编排层据此判断「还有没有批式可卸」并扳闸，ADR-078 ②）。
    #[must_use]
    pub fn asr_load(&self) -> AsrLoadGauge {
        self.load.clone()
    }
}

impl JobSink for JobQueue {
    fn submit(&self, job: Job) -> JobHandle {
        let id = job.id;
        let cancel = CancelToken::new();
        let (events_tx, events) = mpsc::unbounded_channel();
        // 通道已存在 → 先发 Queued，消费者即便在 dispatch 前订阅也不丢（§5.3）。
        let _ = events_tx.send(JobEvent::Queued);
        let seq = self.seq.fetch_add(1, AtomicOrdering::Relaxed);
        // 队列已关闭（dispatcher 退出）时，events_tx 随 QueuedJob 一并 drop，consumer 自然收尾。
        let _ = self.submit_tx.send(QueuedJob {
            job,
            cancel: cancel.clone(),
            events_tx,
            seq,
            queued_at: tokio::time::Instant::now(),
            admission_notified: false,
        });
        JobHandle { id, events, cancel }
    }
}

/// 调度循环：收 Job 入堆，按优先级在**两个**并发闸门下派发（ADR-078 ①）。
///
/// # 三个堆、两个信号量
///
/// - `session` ← 会话池：流式 ASR 腿 + 该会话的实时转译。它们不占计算额度。
/// - `compute` ← 计算池里**有人在等结果**的那些（翻译/总结/对话/嵌入…）。
/// - `batch_asr` ← 计算池里的批式转写。与 `compute` 共用同一个信号量（同一批引擎），
///   但**单独一个堆**：高负载时它整堆让路（ADR-078 ②），而堆分开才能让它让路时不把
///   排在它后面的交互式 Job 一起堵住。
async fn dispatcher(
    mut submit_rx: UnboundedReceiver<QueuedJob>,
    router: Arc<Router>,
    registry: Arc<ProviderRegistry>,
    capacity: AdmissionCapacity,
    usage_sink: Arc<dyn UsageSink>,
    load: AsrLoadGauge,
) {
    let compute_sem = Arc::new(Semaphore::new(capacity.compute.max(1)));
    let session_sem = Arc::new(Semaphore::new(capacity.session.max(1)));
    let mut session: BinaryHeap<QueuedJob> = BinaryHeap::new();
    let mut compute: BinaryHeap<QueuedJob> = BinaryHeap::new();
    let mut batch_asr: BinaryHeap<QueuedJob> = BinaryHeap::new();
    let mut batch_gate = load.subscribe();

    loop {
        // 批式让路时**只**让批式那一堆停下；`compute` 与 `session` 照跑。
        let batch_admissible = !batch_asr.is_empty()
            && !load.batch_admission_paused()
            // 跨堆保持优先级序：有更高优先级的计算 Job 在等时，批式不插队。
            && compute
                .peek()
                .is_none_or(|top| top.job.priority <= batch_asr.peek().expect("已判非空").job.priority);
        // 会话堆里最早到期的那条等待时刻（堆很小——上限就是会话池容量）。
        let session_deadline = session
            .iter()
            .filter_map(QueuedJob::admission_deadline)
            .min();

        tokio::select! {
            maybe_job = submit_rx.recv() => {
                match maybe_job {
                    Some(queued) => match queued.job.admission_lane() {
                        AdmissionLane::Session => session.push(queued),
                        AdmissionLane::Compute if queued.job.is_batch_asr() => batch_asr.push(queued),
                        AdmissionLane::Compute => compute.push(queued),
                    },
                    None => break, // 所有提交端 drop → 收摊
                }
            }
            permit = session_sem.clone().acquire_owned(), if !session.is_empty() => {
                let queued = session.pop().expect("guard 保证堆非空");
                spawn_job(
                    permit.expect("semaphore 不会被关闭"), queued, &router, &registry, &usage_sink, None,
                );
            }
            permit = compute_sem.clone().acquire_owned(), if !compute.is_empty() => {
                let queued = compute.pop().expect("guard 保证堆非空");
                spawn_job(
                    permit.expect("semaphore 不会被关闭"), queued, &router, &registry, &usage_sink, None,
                );
            }
            permit = compute_sem.clone().acquire_owned(), if batch_admissible => {
                let queued = batch_asr.pop().expect("guard 保证堆非空");
                let inflight = load.enter_batch();
                spawn_job(
                    permit.expect("semaphore 不会被关闭"), queued, &router, &registry, &usage_sink,
                    Some(inflight),
                );
            }
            // 闸位翻转（实时会话升/降档）→ 重算 `batch_admissible`。
            // 没有这一臂时闸打开后没有任何事件会唤醒本循环，批式会一直停到下一次提交。
            _ = batch_gate.changed() => {}
            () = sleep_until_opt(session_deadline) => {
                enforce_admission_deadlines(&mut session);
            }
        }
    }
}

/// 领了额度就开跑：额度与批式在飞守卫都随任务 move 进去，Job 结束即一并释放。
fn spawn_job(
    permit: tokio::sync::OwnedSemaphorePermit,
    queued: QueuedJob,
    router: &Arc<Router>,
    registry: &Arc<ProviderRegistry>,
    usage_sink: &Arc<dyn UsageSink>,
    inflight: Option<BatchInflight>,
) {
    let router = Arc::clone(router);
    let registry = Arc::clone(registry);
    let usage_sink = Arc::clone(usage_sink);
    tokio::spawn(async move {
        let _permit = permit; // 持有至 Job 结束，结束即释放并发额度
        let _inflight = inflight; // 同上：批式在飞计数随 Job 终态自动归还
        run_job(queued, router, registry, usage_sink).await;
    });
}

/// `Some(deadline)` → 睡到那一刻；`None` → 永不醒（本臂在 `select!` 里等价于关闭）。
async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// 会话池的等待到期处理（ADR-078 ①③）：先说出口，再放弃。
///
/// **等待与失败是两件事，故是两个信号**。此前 `JobEvent::Queued` 是哑的——发出去没有
/// 任何消费者，于是「实时转写在等一个永远不会来的额度」与「一切正常」在界面上、在 stdout
/// 上、在测试里都完全无法区分。这是 TDD-16「失败必须说出口」的同族缺口：漏掉的是「等待」。
fn enforce_admission_deadlines(session: &mut BinaryHeap<QueuedJob>) {
    let now = tokio::time::Instant::now();
    let mut survivors = Vec::with_capacity(session.len());
    for mut queued in session.drain() {
        let Some(deadline) = queued.admission_deadline() else {
            survivors.push(queued);
            continue;
        };
        if deadline > now {
            survivors.push(queued);
            continue;
        }
        let waited_ms = now.saturating_duration_since(queued.queued_at).as_millis() as u64;
        if queued.admission_notified {
            // 二次到期 = 放弃。可行动的错误码 `LOCAL_SESSION_BUSY`：录音继续，
            // 停录后的二次转写补齐权威结果。
            eprintln!(
                "[jobqueue] 流式 Job 等会话额度 {waited_ms}ms 未获准入，判定为拒（会话池额度被占；\
                 录音与落盘不受影响，停录后二次转写补齐）。"
            );
            let _ = queued
                .events_tx
                .send(JobEvent::Failed(JobError::LocalSessionBusy { waited_ms }));
            // 不入 survivors：events_tx 随之 drop，消费者收到终态后通道关闭。
            continue;
        }
        eprintln!(
            "[jobqueue] 流式 Job 已等会话额度 {waited_ms}ms 仍未获准入（超过首窗延迟上界 \
             {}ms）——从此刻起每多等一秒都是补不回来的实时字幕。",
            STREAM_ADMISSION_NOTICE.as_millis(),
        );
        let _ = queued
            .events_tx
            .send(JobEvent::AdmissionDelayed { waited_ms });
        queued.admission_notified = true;
        survivors.push(queued);
    }
    for queued in survivors {
        session.push(queued);
    }
}

/// 执行单个 Job：路由 → 选定 Provider → 转发事件，带"选定前一跳兜底"（§4 硬边界保守子集）。
async fn run_job(
    queued: QueuedJob,
    router: Arc<Router>,
    registry: Arc<ProviderRegistry>,
    usage_sink: Arc<dyn UsageSink>,
) {
    let QueuedJob {
        job,
        cancel,
        events_tx,
        ..
    } = queued;

    let candidates = match router.resolve(&job) {
        Ok(c) => c,
        Err(e) => {
            let err = match e {
                RouteError::NoLocalProvider(capability) => {
                    JobError::StrictLocalBlocked { capability }
                }
                // 会话所属 Provider 掉线（模型被换掉、服务端转写开关被关）：
                // 归 `ProviderUnavailable` 而不是 `InvalidInput`——Job 本身没有问题，
                // 是那条腿没了，UI 据此给的是"重试/换引擎"而不是"你的输入不合法"。
                RouteError::StreamProviderUnavailable(provider) => {
                    JobError::ProviderUnavailable { provider }
                }
                // Job 的形状可能完全有效；这里失败的是装配，不是输入。尤其是冷启动时
                // 服务端凭据暂不可读，注册表不能再以空集合把这条事实伪装成 INVALID_INPUT。
                RouteError::NoProvider(capability) => {
                    JobError::ProviderNotConfigured { capability }
                }
                RouteError::StreamNotPinned => JobError::ProviderNotConfigured {
                    capability: job.kind.capability(),
                },
            };
            usage_sink.record(UsageRecord::from_terminal(
                &job,
                "failed",
                Usage::default(),
                now_timestamp(),
                TerminalAttribution::unrouted(&err),
            ));
            let _ = events_tx.send(JobEvent::Failed(err));
            return;
        }
    };

    let total = candidates.len();
    let mut last_failure: Option<JobError> = None;
    let mut usage = Usage::default();

    for (i, pid) in candidates.iter().enumerate() {
        let Some(provider) = registry.get(pid) else {
            continue;
        };
        let mut stream = provider.execute(job.clone(), cancel.clone()).await;
        // committed = 是否已向消费者转发过 Started（即已"选定"该 Provider）。
        let mut committed = false;
        let mut fell_back = false;

        while let Some(ev) = stream.next().await {
            match ev {
                // Started 暂不转发，待首个实质事件再发（杜绝兜底时的重复 Started）。
                JobEvent::Started => {}
                JobEvent::Failed(err) => {
                    if !committed && i + 1 < total {
                        let current_location = provider.descriptor().location;
                        let next_location = candidates[i + 1..]
                            .iter()
                            .find_map(|next_id| registry.get(next_id))
                            .map(|next| next.descriptor().location);
                        // 第四档「本机优先，不可用时用服务端」放开本地 → 外部这一条（ADR-117）；
                        // 判据从路由策略句柄现读，改设置即刻生效。
                        let allow_local_to_cloud =
                            router.strategy_handle().get().allows_local_to_cloud();
                        if next_location.is_some_and(|next| {
                            err.failover_disposition_with(
                                current_location,
                                next,
                                allow_local_to_cloud,
                            )
                            .continues()
                        }) {
                            last_failure = Some(err);
                            fell_back = true;
                            break;
                        }
                    }
                    usage_sink.record(UsageRecord::from_terminal(
                        &job,
                        "failed",
                        usage,
                        now_timestamp(),
                        TerminalAttribution {
                            provider: Some(&provider.descriptor()),
                            error: Some(&err),
                        },
                    ));
                    let _ = events_tx.send(JobEvent::Failed(err));
                    return;
                }
                JobEvent::Usage(snapshot) => {
                    // 快照语义：留最后一份，失败 / 取消终态按它记账（TDD-14 §3 修订注）。
                    usage = snapshot;
                    if !committed {
                        if events_tx.send(JobEvent::Started).is_err() {
                            return; // 消费者已走
                        }
                        committed = true;
                    }
                    if events_tx.send(JobEvent::Usage(snapshot)).is_err() {
                        return;
                    }
                }
                JobEvent::Finished(mut result) => {
                    // 时间戳由持有系统时钟的调度层统一盖章。Provider 只负责计算结果，不能
                    // 各自读时钟；若把它们的占位 ZERO 原样落库，最新任务会排到 1970 年。
                    result.finished_at = now_timestamp();
                    usage_sink.record(UsageRecord::from_finished(
                        &job,
                        &result,
                        provider.descriptor().location,
                    ));
                    if !send_started_if_needed(&events_tx, committed) {
                        return; // 消费者已走
                    }
                    let _ = events_tx.send(JobEvent::Finished(result));
                    return;
                }
                JobEvent::Cancelled => {
                    usage_sink.record(UsageRecord::from_terminal(
                        &job,
                        "cancelled",
                        usage,
                        now_timestamp(),
                        TerminalAttribution {
                            provider: Some(&provider.descriptor()),
                            // 取消不是失败：它没有码，写一个就是在账本上凭空造一次故障。
                            error: None,
                        },
                    ));
                    if !send_started_if_needed(&events_tx, committed) {
                        return; // 消费者已走
                    }
                    let _ = events_tx.send(JobEvent::Cancelled);
                    return;
                }
                other => {
                    if !committed {
                        if events_tx.send(JobEvent::Started).is_err() {
                            return; // 消费者已走
                        }
                        committed = true;
                    }
                    if events_tx.send(other).is_err() {
                        return;
                    }
                }
            }
        }

        if !fell_back && committed {
            // 已选定但流未达终态而结束：Phase 1 视为该 Job 结束（不再尝试其它候选）。
            return;
        }
    }

    // 所有候选都在"选定前"失败 → 终态 Failed。
    let err = last_failure.unwrap_or_else(|| JobError::ProviderNotConfigured {
        capability: job.kind.capability(),
    });
    usage_sink.record(UsageRecord::from_terminal(
        &job,
        "failed",
        usage,
        now_timestamp(),
        // 候选链跑完仍未选定：账本上如实是「没有任何一条腿站住」，Provider 归 `unresolved`，
        // 而错误码是**最后一条腿**给的那个——它就是用户屏幕上看到的那一个。
        TerminalAttribution::unrouted(&err),
    ));
    let _ = events_tx.send(JobEvent::Failed(err));
}

fn now_timestamp() -> Timestamp {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    Timestamp::from_millis(ms)
}

fn send_started_if_needed(events_tx: &UnboundedSender<JobEvent>, committed: bool) -> bool {
    committed || events_tx.send(JobEvent::Started).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::{
        ChatMessage, ChatRole, JobKind, JobOrigin, Lang, Priority, SummaryStyle, TextRef,
    };

    fn queued(priority: Priority, seq: u64) -> QueuedJob {
        let (events_tx, _rx) = mpsc::unbounded_channel();
        let mut job = Job::new(
            JobKind::Refine {
                text: TextRef::Inline("x".into()),
                instruction: None,
            },
            JobOrigin::Cli,
        );
        job.priority = priority;
        QueuedJob {
            job,
            cancel: CancelToken::new(),
            events_tx,
            seq,
            queued_at: tokio::time::Instant::now(),
            admission_notified: false,
        }
    }

    #[test]
    fn heap_orders_by_priority_then_fifo() {
        let mut heap = BinaryHeap::new();
        heap.push(queued(Priority::Background, 0));
        heap.push(queued(Priority::Interactive, 1));
        heap.push(queued(Priority::Normal, 2));
        heap.push(queued(Priority::Interactive, 3));

        let order: Vec<(Priority, u64)> =
            std::iter::from_fn(|| heap.pop().map(|q| (q.job.priority, q.seq))).collect();

        assert_eq!(
            order,
            vec![
                (Priority::Interactive, 1), // 高优先级先出；同级 FIFO（1 在 3 前）
                (Priority::Interactive, 3),
                (Priority::Normal, 2),
                (Priority::Background, 0),
            ]
        );
    }

    /// 有效文本任务没有可路由的 LLM 时，失败的是 Provider 装配，绝不是用户输入。
    ///
    /// 四种入口都通过同一个 `JobQueue → Router` 主干；逐一钉住，避免任一路径把
    /// `RouteError::NoProvider` 偷换回 `INVALID_INPUT`，让用户去修改本来有效的正文。
    #[tokio::test]
    async fn every_text_llm_entry_reports_provider_not_configured_when_registry_is_empty() {
        let kinds = [
            JobKind::Chat {
                messages: vec![ChatMessage::new(ChatRole::User, "hello")],
                tools: Vec::new(),
            },
            JobKind::Translate {
                text: TextRef::Inline("hello".into()),
                source: Some(Lang::from("en")),
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobKind::Summarize {
                text: TextRef::Inline("hello".into()),
                style: SummaryStyle::Bullets,
                instruction: None,
            },
            JobKind::Refine {
                text: TextRef::Inline("hello".into()),
                instruction: None,
            },
        ];

        for kind in kinds {
            let capability = kind.capability();
            let queue = JobQueue::new(
                Arc::new(ProviderRegistry::new()),
                AdmissionCapacity::for_capture_tracks(2),
            );
            let events = queue.submit(Job::new(kind, JobOrigin::Cli)).collect().await;
            assert!(
                matches!(
                    events.last(),
                    Some(JobEvent::Failed(JobError::ProviderNotConfigured { capability: actual }))
                        if *actual == capability
                ),
                "{capability:?} 未装配时应报告可操作的 Provider 状态：{events:?}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, JobEvent::Failed(JobError::InvalidInput(_)))),
                "{capability:?} 的有效输入不得被误报为 INVALID_INPUT：{events:?}"
            );
        }
    }
}
