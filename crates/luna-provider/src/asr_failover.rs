//! 实时转写的降级组合（ADR-038 ⑤）。
//!
//! > 「连接中断时本地 whisper 接管，且 `committed` 一旦发出永不回退这条不变量**必须
//! > 跨降级保持**——云端已提交的前缀不能因为切回本地而被重写。」
//!
//! # 为什么是**组合**而不是让远程 Provider 自己兜底
//!
//! 让 [`crate::asr_realtime`] 内部去调本机引擎，就要让 `luna-provider` 反向依赖
//! `luna-engine`——正是把流式接缝提到领域层时刚拆掉的那类耦合。这里两条腿都只是
//! `Arc<dyn StreamingAsr>`：本组合既不知道云那边是 WebSocket，也不知道本机那边是
//! whisper-rs，于是「不回退」这条不变量**只在这一个文件里被执行一次**。
//!
//! # 兜底腿是**懒注册**的
//!
//! 只有真的要切过去时才 `register_stream(fallback)`。提前注册的话，每一次正常完成的
//! 云端会话都会在本机引擎的登记表里留一份没人来取的条目——一场三小时会议下来是一堆
//! 永不回收的通道。
//!
//! # 哪些失败会降级
//!
//! ADR-038 ⑤ 说的是**连接中断**。故只有「这条链路当下不通」才切：
//! [`JobError::ProviderUnavailable`] / [`JobError::Timeout`]。服务端 5xx、限流、鉴权与
//! 额度错误都说明端点可达，必须原样告诉用户，不能静默改变算力与质量。
//!
//! 额度用尽与未登录**不降级**，尽管那样能让转写继续跑。这两个错误各有一个用户
//! **必须知道并且能处理**的动作（去账户页 / 去登录）；把它们悄悄用本机算力盖过去，
//! 用户会在几周后才发现自己的云额度早就没了，而中间那些会议的转写质量已经变了。

use std::sync::{Arc, Mutex};

use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use luna_core::{
    AIProvider, AudioRef, CancelToken, CapabilitySet, CommittedLedger, ExecutorInfo, ExecutorKind,
    Job, JobError, JobEvent, JobEventStream, JobKind, PartialPayload, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, StreamId, StreamLangControl,
    StreamRegistration, StreamRegistry, StreamRelease, StreamingAsr, UsageEstimate, WindowInput,
};

/// 本组合的稳定 id。
pub const FAILOVER_ASR_PROVIDER_ID: &str = "asr-failover";

/// 窗口队列深度。与两条腿同深，避免本层成为新的瓶颈。
const WINDOW_QUEUE: usize = 64;

/// 当前该往哪条腿喂，以及从哪条腿收耗时统计。
struct Leg {
    window_tx: std::sync::mpsc::SyncSender<Option<WindowInput>>,
    infer_rx: std::sync::mpsc::Receiver<u64>,
}

/// 语言控制面的分发。
///
/// 注册时只有主腿；兜底腿是切换时才加进来的，故这里记住最后设过的值，
/// 新腿加入时**补设一次**——否则用户在设置里选的语言会在降级后悄悄失效。
#[derive(Default)]
struct FanoutLang {
    configured: Mutex<Option<String>>,
    legs: Mutex<Vec<Arc<dyn StreamLangControl>>>,
}

impl FanoutLang {
    fn join(&self, leg: Arc<dyn StreamLangControl>) {
        leg.set_configured(self.configured.lock().expect("lang poisoned").clone());
        self.legs.lock().expect("lang poisoned").push(leg);
    }
}

impl StreamLangControl for FanoutLang {
    fn set_configured(&self, configured: Option<String>) {
        *self.configured.lock().expect("lang poisoned") = configured.clone();
        for leg in self.legs.lock().expect("lang poisoned").iter() {
            leg.set_configured(configured.clone());
        }
    }

    /// 取**当前那条腿**的检测结果（最后加入的一条即当前腿）。
    ///
    /// 不做「谁有值取谁」：降级之后本机检测出的语言才是此刻正在用的那个，
    /// 拿云端那条早已断开的旧值回答，会让界面一直显示一个不再成立的语言。
    fn auto_detected(&self) -> Option<String> {
        self.legs
            .lock()
            .expect("lang poisoned")
            .last()
            .and_then(|leg| leg.auto_detected())
    }
}

struct Session {
    /// 采集侧写入端的对端。
    window_rx: std::sync::mpsc::Receiver<Option<WindowInput>>,
    /// 采集侧读取端的对端（每窗耗时，录音的自适应降级据此判断负载）。
    infer_tx: std::sync::mpsc::SyncSender<u64>,
    /// 起始腿（主腿）。降级时经 [`spawn_pump`] 返回的那条通道整体替换。
    leg: Leg,
    lang: Arc<FanoutLang>,
    /// 主腿那一侧的登记回收句柄。
    ///
    /// **回收要连坐**：本组合的一条登记背后还有主腿的一条（`register_stream` 里同步
    /// 登记的）。本组合这条被回收时若不带上它，主腿的表里就留下一条谁也不会再摘走的
    /// 会话——同一个缺陷只是挪了一层。句柄住在 `Session` 里，于是「本条登记没了」
    /// 与「主腿那条也没了」由所有权绑成一件事。
    primary_release: StreamRelease,
}

/// 「先云、断了就切本机」的流式转写。
pub struct FailoverAsrProvider {
    primary: Arc<dyn StreamingAsr>,
    fallback: Arc<dyn StreamingAsr>,
    /// 回收语义在 [`StreamRegistry`] 里只写一次（三个流式实现共用）。
    sessions: StreamRegistry<Session>,
}

impl FailoverAsrProvider {
    /// `primary` 断了就切 `fallback`。
    #[must_use]
    pub fn new(primary: Arc<dyn StreamingAsr>, fallback: Arc<dyn StreamingAsr>) -> Self {
        Self {
            primary,
            fallback,
            sessions: StreamRegistry::new(),
        }
    }
}

impl StreamingAsr for FailoverAsrProvider {
    fn register_stream(&self, id: StreamId) -> StreamRegistration {
        let (window_tx, window_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
        // 只注册主腿——兜底腿见模块头「懒注册」。
        let primary = self.primary.register_stream(id);
        let lang = Arc::new(FanoutLang::default());
        lang.join(primary.lang);
        let release = self.sessions.register(
            id,
            Session {
                window_rx,
                infer_tx,
                leg: Leg {
                    window_tx: primary.window_tx,
                    infer_rx: primary.infer_rx,
                },
                lang: Arc::clone(&lang),
                primary_release: primary.release,
            },
        );
        StreamRegistration {
            window_tx,
            infer_rx,
            lang,
            release,
        }
    }
}

#[async_trait]
impl AIProvider for FailoverAsrProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(FAILOVER_ASR_PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from(FAILOVER_ASR_PROVIDER_ID),
            display_name: self.primary.descriptor().display_name,
            // 主腿的模型：会话还没开始，将要用的就是它。降级之后由 `executor_of`
            // 按兜底腿的自述重新回报（`JobEvent::ExecutedBy`）。
            model: self.primary.descriptor().model,
            location: self.location(),
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        let primary = self.primary.capabilities();
        CapabilitySet {
            // 两条腿都能做的才算数：只有一条腿会的能力，降级之后就没了。
            capabilities: primary
                .capabilities
                .intersection(&self.fallback.capabilities().capabilities)
                .copied()
                .collect(),
            streaming: primary.streaming && self.fallback.capabilities().streaming,
            location: self.location(),
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // 主腿不健康也不代表本组合不可用——兜底腿正是为此存在的。
        let primary = self.primary.health().await;
        if primary.is_ready() {
            primary
        } else {
            self.fallback.health().await
        }
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let JobKind::Transcribe {
            audio: AudioRef::Stream(stream_id),
            ..
        } = job.kind
        else {
            return once_failed(JobError::InvalidInput(
                "降级组合只接受流式会话（AudioRef::Stream）".into(),
            ));
        };
        let Some(session) = self.sessions.take(stream_id) else {
            return once_failed(JobError::InvalidInput(
                "未注册的流式会话——采集侧必须先 register_stream".into(),
            ));
        };

        let primary = Arc::clone(&self.primary);
        let fallback = Arc::clone(&self.fallback);
        let fallback_executor = executor_of(fallback.as_ref());

        Box::pin(stream! {
            // `_primary_release` 必须活到本事件流收场：它是主腿那条登记的回收句柄，
            // 提前丢掉等于在 `primary.execute` 摘走之前把会话从主腿的表里撤了（那边会报
            // 「未注册的流式会话」）。本流不论以哪种方式收场，它都随作用域回收。
            let Session { window_rx, infer_tx, leg, lang, primary_release: _primary_release } = session;
            let switch = spawn_pump(window_rx, infer_tx, leg);

            // 「committed 不回退」在这里执行一次：两条腿产出的定稿都先过这个账本。
            // 账本本身住在领域层（[`CommittedLedger`]）——实时腿的静默重连窗要用同一份
            // 判定，两处各写一份迟早分叉，而分叉的表现是「同一句话出现两次」。
            let mut ledger = CommittedLedger::default();

            let mut events = primary.execute(job.clone(), cancel.clone()).await;
            let mut on_fallback = false;
            // 兜底腿登记的回收句柄**必须活到本事件流收场**，而不是活到切腿那个 arm 结束：
            // 本机 whisper 腿的 `execute` 只构造生成器，登记表的 `take` 发生在**第一次拉动**
            // 兜底流的时候——那已经在 arm 之外了。此前句柄随 arm 作用域回收，兜底腿一拉动就
            // 报「StreamId 未注册到 WhisperProvider」，整场语音输入以 `invalid_input` 收场
            // （2026-09-05 真机；此前的假腿在 `execute` 返回前就摘走登记，故用例看不见）。
            // 本流被丢掉（用户中途停了）时它随生成器一起回收，仍守
            // `a_lazily_registered_fallback_leg_is_recycled_when_the_stream_is_dropped`。
            // 下划线前缀：它的全部作用就是**活着**（Drop 即注销），不会再被读。
            let mut _fallback_release: Option<StreamRelease> = None;
            while let Some(event) = events.next().await {
                match event {
                    JobEvent::Failed(error) if !on_fallback && should_fail_over(&error) => {
                        // **切腿必须留痕**（零内容，只有稳定错误码）：这是「用户的字幕从
                        // 云端换成了本机」那一刻，而界面上它只表现为一个小圆点变暗——
                        // 2026-08-18 的现场诊断里，"到底切没切"只能靠服务端连接表反推。
                        eprintln!("[asr/failover] 切到本机兜底 code={}", error.code());
                        // 切腿：兜底腿此刻才注册，采集侧的投窗随下一个窗口自动改道。
                        //
                        // `reg` 剩下的那半（登记的回收句柄）交给流级的 `fallback_release`
                        // 接住——活到本流收场，而不只活过下面那句 `fallback.execute`：见上方
                        // 那段注释，真腿摘登记的时刻在 execute 之后。守护在
                        // `committed_prefix_survives_the_switch`（假腿现在和真腿一样，拉动流
                        // 时才摘登记，摘不到即失败）。
                        let reg = fallback.register_stream(stream_id);
                        lang.join(reg.lang);
                        _fallback_release = Some(reg.release);
                        // 换腿只经泵这一条路，于是**泵已经收工时新腿当场随 `Err` 被丢掉**
                        // ——采集侧关流恰好撞上切腿（用户按停止的同一瞬间云腿断了）时，
                        // 不会留下一条谁也不再喂、也永远不会收尾的兜底腿。
                        let _ = switch.send(Leg {
                            window_tx: reg.window_tx,
                            infer_rx: reg.infer_rx,
                        });
                        on_fallback = true;
                        // 谁在跑变了就必须说（PRD-17-033）——尤其这次是「数据不再离开
                        // 设备」，用户有权当场知道。
                        yield JobEvent::ExecutedBy(fallback_executor.clone());
                        events = fallback.execute(job.clone(), cancel.clone()).await;
                    }
                    // 已经报过一次排队/开始了，兜底腿的这两帧要吞掉——否则 UI 上
                    // 一次会话会出现两段「开始」。
                    JobEvent::Queued | JobEvent::Started if on_fallback => {}
                    JobEvent::Partial(PartialPayload::Asr {
                        committed_delta,
                        hypothesis,
                        stability,
                    }) => {
                        let fresh = ledger.admit(committed_delta);
                        if !fresh.is_empty() || !hypothesis.is_empty() {
                            yield JobEvent::Partial(PartialPayload::Asr {
                                committed_delta: fresh,
                                hypothesis,
                                stability,
                            });
                        }
                    }
                    JobEvent::Finished(mut result) => {
                        // 终态载荷换成**整场会话**的定稿——兜底腿自己只知道切换之后
                        // 那一段，直接交出去的话，前半场的转写就凭空消失了。
                        result.payload = ResultPayload::Segments(ledger.into_segments());
                        yield JobEvent::Finished(result);
                        return;
                    }
                    other => yield other,
                }
            }
        })
    }

    fn estimate(&self, job: &Job) -> Option<UsageEstimate> {
        self.primary.estimate(job)
    }
}

impl FailoverAsrProvider {
    /// 取两条腿里**走得最远**的那个位置。
    ///
    /// 只要有一条腿会外发，本组合就必须报 `Cloud`。报成设备内的话，严格本地模式的
    /// 既有过滤（`is_external`）会把它放行，而主腿照样把麦克风内容发出去——一次不会
    /// 报错、只能靠抓包才发现的隐私事故（ADR-038 ④）。
    fn location(&self) -> ProviderLocation {
        let rank = |l: ProviderLocation| match l {
            ProviderLocation::Device => 0,
            ProviderLocation::LocalService => 1,
            ProviderLocation::Cloud => 2,
        };
        let primary = self.primary.descriptor().location;
        let fallback = self.fallback.descriptor().location;
        if rank(fallback) > rank(primary) {
            fallback
        } else {
            primary
        }
    }
}

/// 这个失败是「链路当下不通」吗（见模块头「哪些失败会降级」）。
fn should_fail_over(error: &JobError) -> bool {
    match error {
        JobError::ProviderUnavailable { .. } | JobError::Timeout => true,
        // 用户中途切到「仅本地」把云腿掐了——这里**必须**降级，而且是唯一符合他
        // 意图的处置：他要的是「别外发」，不是「别转写」。本机 whisper 恰好两者都满足。
        // 归入不降级的话，按下隐私开关的代价就变成了「这场会议剩下的部分没有字幕」。
        JobError::StrictLocalBlocked { .. } => true,
        // 组织没开放服务端转写（R4b）：本机 whisper 正是这时候该接手的那一条腿。
        JobError::CapabilityNotAllowed { .. } => true,
        _ => false,
    }
}

/// 兜底腿的执行者信息。
///
/// 只有**不外发**的腿才敢说 `LocalRuntime`。兜底腿要是也在云上（两级云部署），
/// 我们并不知道它是自己的运行时还是第三方——不知道就报 `Unknown`，不猜一个更好听的。
fn executor_of(provider: &dyn StreamingAsr) -> ExecutorInfo {
    let descriptor = provider.descriptor();
    ExecutorInfo {
        // `model` 是**模型标识**（`ggml-base`），不是 provider id、也不是展示名。
        // 它归档后会被写进 `MediaRef.model`，将来要回答「这段转写当时用了哪个权重」。
        // 腿自己答不上来时留空——不拿展示名顶替，那会让「本地 Whisper」被当成权重名。
        model: descriptor.model.unwrap_or_default(),
        display_name: descriptor.display_name,
        kind: if descriptor.location.is_external() {
            ExecutorKind::Unknown
        } else {
            ExecutorKind::LocalRuntime
        },
        // 这条腿在客户端手里，位置我们自己知道——切回本机时联网指示据此熄灭。
        leaves_device: descriptor.location.is_external(),
    }
}

/// 把采集侧的窗口转投到当前腿，并把当前腿的每窗耗时回送给采集侧。返回**换腿通道**
/// （降级时把新腿交给泵，见 `execute`）。
///
/// 独立阻塞线程：两端都是 `std::sync::mpsc`（那条通道服务于本机引擎的阻塞线程池），
/// 在 async 上下文里直接 `recv` 会占住一个 runtime worker。
///
/// 生命周期由**采集侧**决定：它投 `None` 或丢掉 `window_tx` 时本线程退出。不另设停止
/// 标志——那种标志只能在下一个窗口到达时被看到，而那时采集侧本来就还在喂。
///
/// # 泵**独占**当前腿的发送端，于是「采集侧关流」必然传到腿上
///
/// 这是本组合最容易漏、而漏掉之后只在真机上要钱的一处。`StreamRegistration::window_tx`
/// 的关流契约有两种写法——投 `None`，或**丢掉发送端**——两条腿也都按两种写
/// （whisper 的 `Ok(None) | Err(_)`、实时腿的 `Some(None) | None`）。而采集侧
/// （`recording.rs` 停录那一段）从头到尾只做 `drop(window_tx.take())`：它**从不投 `None`**。
///
/// 此前腿的发送端住在 `Arc<Mutex<Leg>>` 里，泵与 `execute` 的事件循环**各持一份**。
/// 于是采集侧一丢发送端，泵这边 `recv()` 立刻 `Err` 并退出——却什么都没往下传：那份
/// `Arc` 还被事件循环攥着，腿上的发送端因此不死，实时腿的 `window_rx` 永远等得到下一窗。
/// 真机后果（2026-08-18）：停止实时字幕之后客户端不发 `END`、不关连接，服务端到
/// AssemblyAI 的连接 ESTABLISHED 挂了 9 分钟（**按连接时长计费**），按窗预留
/// （`upstream_spend_holds`，`state='held'`）一直占着不结算；四条会话最后一起以
/// `terminal=cancelled` 收尾的时刻，是**客户端进程退出**那一刻。
///
/// 故腿归泵独占：出循环即丢掉它，关流一路传到底。换腿也只能经这条通道进来——泵收工后
/// 再送进来的腿会随 `SendError` 当场被丢掉，同一条不变量对兜底腿一样成立。
fn spawn_pump(
    window_rx: std::sync::mpsc::Receiver<Option<WindowInput>>,
    infer_tx: std::sync::mpsc::SyncSender<u64>,
    initial: Leg,
) -> std::sync::mpsc::Sender<Leg> {
    let (switch_tx, switch_rx) = std::sync::mpsc::channel::<Leg>();
    std::thread::spawn(move || {
        let mut current = initial;
        while let Ok(item) = window_rx.recv() {
            // 换腿在**收到这一窗之后、投出去之前**生效，不是在等窗之前——泵绝大部分时间
            // 都阻塞在 `recv()` 上，切腿正是在那期间发生的。先看再等的话，切腿之后的**第一
            // 窗仍会落到旧腿**（`windows_follow_the_switch` 就是数这个的）。这也是旧实现的
            // 语义：它同样是收到窗之后才去取那把锁。
            while let Ok(next) = switch_rx.try_recv() {
                current = next;
            }
            let last = item.is_none();
            // `try_send`：满了就丢这一窗。阻塞会一路顶回采集侧，而那一侧连着
            // 音频回调（ADR-009 禁止任何等待）。切腿的瞬间尤其如此——旧腿已经不
            // 再消费了。
            let _ = current.window_tx.try_send(item);
            while let Ok(elapsed) = current.infer_rx.try_recv() {
                let _ = infer_tx.try_send(elapsed);
            }
            if last {
                break;
            }
        }
        // 采集侧关流 = 腿也关流。作用域结束本来也会丢，写出来是为了让「这里是关流点」
        // 有个明确的位置——**真正承重的是所有权**：泵之外没有第二份持有者。把腿放回
        // 一个共享容器（此前的 `Arc<Mutex<Leg>>`）里，这一行就一点用都没有了。
        drop(current);
    });
    switch_tx
}

fn once_failed(error: JobError) -> JobEventStream {
    Box::pin(stream! { yield JobEvent::Failed(error); })
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::{JobOrigin, JobResult, ModelId, Segment, Timestamp, Timings, Usage};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    /// 一条可编排的假腿：按脚本吐事件，并如实记录自己收到了多少窗。
    struct FakeLeg {
        id: &'static str,
        location: ProviderLocation,
        script: Mutex<Vec<JobEvent>>,
        windows: Arc<AtomicUsize>,
        registered: Arc<AtomicBool>,
        /// 这条腿的窗口通道**关掉了**吗（投 `None` 或发送端被丢）。
        ///
        /// 与 `windows` 分开记：一条从没收到过窗的腿，「还在等下一窗」与「已经收到关流」
        /// 是两个截然不同的结局，而只数窗的话它们长得一模一样。
        closed: Arc<AtomicBool>,
        /// 这条腿自己的登记表（真的那一个类型）：用来看**连坐回收**有没有传到这一层。
        registry: StreamRegistry<()>,
    }

    impl FakeLeg {
        fn new(id: &'static str, location: ProviderLocation, script: Vec<JobEvent>) -> Arc<Self> {
            Arc::new(Self {
                id,
                location,
                script: Mutex::new(script),
                windows: Arc::new(AtomicUsize::new(0)),
                registered: Arc::new(AtomicBool::new(false)),
                closed: Arc::new(AtomicBool::new(false)),
                registry: StreamRegistry::new(),
            })
        }

        /// 等这条腿收到关流（有界；返回是否等到）。
        fn wait_closed(&self) -> bool {
            for _ in 0..200 {
                if self.closed.load(Ordering::Acquire) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        }
    }

    impl StreamingAsr for FakeLeg {
        fn register_stream(&self, id: StreamId) -> StreamRegistration {
            self.registered.store(true, Ordering::Release);
            let (window_tx, window_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
            let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
            let seen = Arc::clone(&self.windows);
            let closed = Arc::clone(&self.closed);
            std::thread::spawn(move || {
                while let Ok(Some(_)) = window_rx.recv() {
                    seen.fetch_add(1, Ordering::Release);
                    let _ = infer_tx.try_send(1);
                }
                closed.store(true, Ordering::Release);
            });
            StreamRegistration {
                window_tx,
                infer_rx,
                lang: Arc::new(NoLang),
                release: self.registry.register(id, ()),
            }
        }
    }

    struct NoLang;
    impl StreamLangControl for NoLang {
        fn set_configured(&self, _configured: Option<String>) {}
        fn auto_detected(&self) -> Option<String> {
            None
        }
    }

    #[async_trait]
    impl AIProvider for FakeLeg {
        fn id(&self) -> ProviderId {
            ProviderId::from(self.id)
        }
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: ProviderId::from(self.id),
                display_name: self.id.to_owned(),
                location: self.location,
                model: None,
                license_summary: None,
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet {
                capabilities: [luna_core::Capability::Transcribe].into_iter().collect(),
                streaming: true,
                location: self.location,
                explicit_only: false,
            }
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::ready()
        }
        /// **和真腿一样：`execute` 摘走登记，摘不到就是「未注册」。**
        ///
        /// 假腿此前对登记表一无所知，于是「本组合把腿的登记提前回收了」这类错误在用例里
        /// 完全看不出来——而在真腿上它表现为切腿之后整条会话报「未注册的流式会话」。
        ///
        /// **摘登记发生在事件流第一次被拉动时，而不是 `execute` 返回之前**——真腿
        /// （`luna_engine::WhisperProvider`）正是这么做的：`execute` 只构造 `stream!`，登记表
        /// 的 `take` 在生成器体内。假腿此前在返回流之前就摘走，于是「切腿之后、拉动兜底流
        /// 之前把登记回收了」这个错在用例里永远看不见，而在真机上它就是 2026-09-05 的
        /// 语音输入 `invalid_input`。
        async fn execute(&self, job: Job, _cancel: CancelToken) -> JobEventStream {
            let registry = self.registry.clone();
            let script = std::mem::take(&mut *self.script.lock().unwrap());
            Box::pin(stream! {
                if let JobKind::Transcribe {
                    audio: AudioRef::Stream(id),
                    ..
                } = job.kind
                {
                    if registry.take(id).is_none() {
                        yield JobEvent::Failed(JobError::InvalidInput("未注册的流式会话".into()));
                        return;
                    }
                }
                for event in script {
                    yield event;
                }
            })
        }
        fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
            None
        }
    }

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: luna_core::SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_owned(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    fn committed(segments: Vec<Segment>) -> JobEvent {
        JobEvent::Partial(PartialPayload::Asr {
            committed_delta: segments,
            hypothesis: String::new(),
            stability: 1.0,
        })
    }

    fn finished(job: &Job) -> JobEvent {
        JobEvent::Finished(JobResult {
            job_id: job.id,
            provider_id: ProviderId::from("leg"),
            model_id: Some(ModelId::from("m")),
            payload: ResultPayload::Segments(Vec::new()),
            usage: Usage::default(),
            timings: Timings::default(),
            output_truncated: None,
            finished_at: Timestamp::ZERO,
        })
    }

    fn stream_job() -> (StreamId, Job) {
        let id = StreamId::new();
        (
            id,
            Job::new(
                JobKind::Transcribe {
                    audio: AudioRef::Stream(id),
                    lang_hint: None,
                    initial_prompt: None,
                },
                JobOrigin::Recording,
            ),
        )
    }

    async fn run(provider: &FailoverAsrProvider, job: Job) -> Vec<JobEvent> {
        provider
            .execute(job, CancelToken::new())
            .await
            .collect()
            .await
    }

    fn committed_texts(events: &[JobEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                JobEvent::Partial(PartialPayload::Asr {
                    committed_delta, ..
                }) => Some(committed_delta.iter().map(|s| s.text.clone())),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 这条是本文件存在的理由：降级之后，云端已发出的前缀不得被本机重写。
    ///
    /// 兜底腿是另一个引擎，切分点不同、会把整段重解一遍。不按绝对时间去重的话，
    /// 用户会看到前半场的每一句都被重新播了一遍——而他可能已经复制走了。
    #[tokio::test]
    async fn committed_prefix_survives_the_switch() {
        let (stream_id, job) = stream_job();
        let cloud = FakeLeg::new(
            "cloud",
            ProviderLocation::Cloud,
            vec![
                JobEvent::Queued,
                JobEvent::Started,
                committed(vec![seg(0, 1000, "第一句"), seg(1000, 2000, "第二句")]),
                JobEvent::Failed(JobError::ProviderUnavailable {
                    provider: ProviderId::from("cloud"),
                }),
            ],
        );
        let local = FakeLeg::new(
            "local",
            ProviderLocation::Device,
            vec![
                JobEvent::Queued,
                JobEvent::Started,
                // 本机从头重解：前两句是重复的，第三句才是新的。
                committed(vec![
                    seg(0, 1000, "第一句"),
                    seg(1000, 2000, "第二句"),
                    seg(2000, 3000, "第三句"),
                ]),
                finished(&job),
            ],
        );
        let provider = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
        let _reg = provider.register_stream(stream_id);
        let events = run(&provider, job).await;

        assert_eq!(
            committed_texts(&events),
            vec!["第一句", "第二句", "第三句"],
            "降级后已发出的定稿被重发了：{events:?}"
        );
        // 一次会话只该有一段「开始」。
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, JobEvent::Started))
                .count(),
            1
        );
        // 终态载荷是整场，而不是切换之后那一段。
        let Some(JobEvent::Finished(result)) = events.last() else {
            panic!("应以 Finished 收场：{events:?}");
        };
        let ResultPayload::Segments(all) = &result.payload else {
            panic!("转写终态该是 Segments");
        };
        assert_eq!(all.len(), 3, "终态丢了降级之前的转写：{all:?}");
    }

    /// 额度用尽/未登录**不降级**——见模块头。悄悄用本机盖过去，用户几周后才发现。
    #[tokio::test]
    async fn quota_and_auth_failures_surface_instead_of_being_papered_over() {
        for error in [
            JobError::QuotaExhausted {
                dimension: Some("asr_seconds".into()),
            },
            JobError::Unauthorized {
                provider: ProviderId::from("cloud"),
            },
            // 账户被冻结（C3）：与额度用尽同一处置——云腿停、录音继续，不悄悄换成本机。
            JobError::AccountFrozen {
                provider: ProviderId::from("cloud"),
            },
            // 本月预付上限（C4）：同一处置——云腿停在已产出处、录音继续，不悄悄换成本机。
            JobError::SpendCapReached {
                provider: ProviderId::from("cloud"),
            },
            // 免费额度被扣下（C5）：同一处置。
            JobError::FreeQuotaWithheld {
                provider: ProviderId::from("cloud"),
            },
        ] {
            let (stream_id, job) = stream_job();
            let cloud = FakeLeg::new(
                "cloud",
                ProviderLocation::Cloud,
                vec![JobEvent::Started, JobEvent::Failed(error.clone())],
            );
            let local = FakeLeg::new("local", ProviderLocation::Device, vec![finished(&job)]);
            let registered = Arc::clone(&local.registered);
            let provider =
                FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
            let _reg = provider.register_stream(stream_id);
            let events = run(&provider, job).await;
            assert!(
                matches!(events.last(), Some(JobEvent::Failed(_))),
                "{} 应如实失败：{events:?}",
                error.code()
            );
            assert!(
                !registered.load(Ordering::Acquire),
                "{} 不该触发降级",
                error.code()
            );
        }
    }

    /// **`execute` 没跑到时，本组合与主腿的登记必须一起消失。**
    ///
    /// 采集侧先 `register_stream` 拿句柄、再 `submit(job)`，而 Job 有两条走不到 `execute`
    /// 的出口（会话池准入到期即拒 `LocalSessionBusy`、路由解析失败）。只由 `execute`
    /// 回收的话，滞留的条目攥着 `window_rx` 那一端 —— 采集侧在 Job 失败之前一直在投窗，
    /// 通道深 64、单窗最大 ~10 秒 16 kHz f32，一条就是约 40 MB，而 Provider 实例是复用的。
    ///
    /// 连坐是这一层特有的那一半：本组合的一条登记背后还有主腿的一条。
    ///
    /// 反向验证：把 `Session.primary_release` 去掉（或在 `register_stream` 里改回
    /// `StreamRelease::detached()`），主腿那条断言即红。
    #[test]
    fn a_registration_execute_never_took_is_recycled_down_to_the_primary_leg() {
        let stream_id = StreamId::new();
        let cloud = FakeLeg::new("cloud", ProviderLocation::Cloud, vec![]);
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![]);
        let provider = FailoverAsrProvider::new(
            Arc::clone(&cloud) as Arc<dyn StreamingAsr>,
            Arc::clone(&local) as Arc<dyn StreamingAsr>,
        );

        let reg = provider.register_stream(stream_id);
        assert_eq!(provider.sessions.len(), 1, "本组合这一条");
        assert_eq!(
            cloud.registry.len(),
            1,
            "主腿那一条（`register_stream` 里同步登的）"
        );

        // Job 在 execute 之前就失败了 —— 采集侧收到终态，丢掉回收句柄。
        drop(reg.release);

        assert!(provider.sessions.is_empty(), "本组合的登记滞留");
        assert!(
            cloud.registry.is_empty(),
            "主腿的登记滞留——回收只挪了一层，没到底"
        );
        assert!(
            matches!(
                reg.window_tx.try_send(None),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            ),
            "接收端还在 = 那 64 窗还在"
        );
    }

    /// **切了腿、但兜底腿的 `execute` 没跑成，也不能在兜底腿的表里留条目。**
    ///
    /// 兜底腿是**懒注册**的（见模块头），于是它的登记从建立到被摘走之间有一段窗口期。
    /// 用户恰在这一瞬停下（事件流被丢掉）时，`fallback.execute` 永远不会被调用——那条
    /// 登记于是没有任何人来摘，而它攥着一份窗缓冲。
    ///
    /// 用例把这个瞬间钉死：只读到 `ExecutedBy`（切腿已完成、execute 尚未调用）就丢掉事件流。
    ///
    /// 反向验证：让那半个 `reg`（登记回收句柄）不随生成器回收（`std::mem::forget`），
    /// 末尾那条断言即红。
    #[tokio::test]
    async fn a_lazily_registered_fallback_leg_is_recycled_when_the_stream_is_dropped() {
        let (stream_id, job) = stream_job();
        let cloud = FakeLeg::new(
            "cloud",
            ProviderLocation::Cloud,
            vec![JobEvent::Started, JobEvent::Failed(JobError::Timeout)],
        );
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![finished(&job)]);
        let provider = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
        let _reg = provider.register_stream(stream_id);

        let mut events = provider.execute(job, CancelToken::new()).await;
        while let Some(event) = events.next().await {
            if matches!(event, JobEvent::ExecutedBy(_)) {
                break;
            }
        }
        assert!(
            local.registered.load(Ordering::Acquire),
            "用例没走到切腿那一支，它证明不了任何事"
        );
        assert_eq!(
            local.registry.len(),
            1,
            "此刻兜底腿应正好有一条登记（execute 还没来摘）——否则本用例证不了它会被回收"
        );

        // 用户此刻停了：事件流被丢，兜底腿的 `execute` 永远不会被调用。
        drop(events);

        assert!(
            local.registry.is_empty(),
            "兜底腿的登记留在表里——没有人会再来摘它，而它攥着一份窗缓冲"
        );
    }

    /// 兜底腿**懒注册**：正常收场的会话不得在本机引擎里留下没人取的条目。
    #[tokio::test]
    async fn healthy_session_never_touches_the_fallback() {
        let (stream_id, job) = stream_job();
        let cloud = FakeLeg::new(
            "cloud",
            ProviderLocation::Cloud,
            vec![JobEvent::Started, finished(&job)],
        );
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![]);
        let registered = Arc::clone(&local.registered);
        let provider = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
        let _reg = provider.register_stream(stream_id);
        let _ = run(&provider, job).await;
        assert!(
            !registered.load(Ordering::Acquire),
            "云端正常跑完却注册了本机腿——每场会议都会留一堆不回收的通道"
        );
    }

    /// 切腿之后，采集侧继续投的窗要落到**新腿**上（采集侧对切换一无所知）。
    #[tokio::test]
    async fn windows_follow_the_switch() {
        let (stream_id, job) = stream_job();
        let cloud = FakeLeg::new(
            "cloud",
            ProviderLocation::Cloud,
            vec![
                JobEvent::Started,
                JobEvent::Failed(JobError::Timeout),
                // 注意：Failed 之后脚本还剩内容也不会被读到——流已终止。
            ],
        );
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![finished(&job)]);
        let seen = Arc::clone(&local.windows);
        let provider = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
        let reg = provider.register_stream(stream_id);
        let events = run(&provider, job).await;
        assert!(
            events.iter().any(
                |e| matches!(e, JobEvent::ExecutedBy(i) if i.kind == ExecutorKind::LocalRuntime)
            ),
            "降级必须如实告知执行者变了：{events:?}"
        );
        for i in 0..3 {
            reg.window_tx
                .send(Some(WindowInput {
                    samples: vec![0.0; 4],
                    abs_start: i * 100,
                    speech_len: 4,
                }))
                .expect("投窗");
        }
        for _ in 0..50 {
            if seen.load(Ordering::Acquire) == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            seen.load(Ordering::Acquire),
            3,
            "切腿后采集侧的窗没有改道——降级之后就再也没有转写了"
        );
    }

    /// `ExecutorInfo.model` 必须是**模型标识**，不是 provider id、也不是展示名。
    ///
    /// 它会被归档进 `MediaRef.model`，将来要回答「这段转写当时用了哪个权重」。
    /// 塞 provider id 时 UI 的「模型」列会显示 `local-whisper`；拿展示名顶替时会显示
    /// 「本地 Whisper」——两者都会被后来的人当成权重名读。腿自己答不上来就留空。
    #[test]
    fn the_reported_model_is_a_model_id_not_a_label() {
        struct Weighted;
        #[async_trait]
        impl StreamingAsr for Weighted {
            fn register_stream(&self, _id: StreamId) -> StreamRegistration {
                unreachable!("本测试不建会话")
            }
        }
        #[async_trait]
        impl AIProvider for Weighted {
            fn id(&self) -> ProviderId {
                ProviderId::from("local-whisper")
            }
            fn descriptor(&self) -> ProviderDescriptor {
                ProviderDescriptor {
                    id: self.id(),
                    display_name: "本地 Whisper".into(),
                    model: Some("ggml-base".into()),
                    location: ProviderLocation::Device,
                    license_summary: None,
                }
            }
            fn capabilities(&self) -> CapabilitySet {
                CapabilitySet {
                    capabilities: Default::default(),
                    streaming: true,
                    location: ProviderLocation::Device,
                    explicit_only: false,
                }
            }
            async fn health(&self) -> ProviderHealth {
                ProviderHealth::ready()
            }
            async fn execute(&self, _job: Job, _cancel: CancelToken) -> JobEventStream {
                Box::pin(futures::stream::empty())
            }
            fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
                None
            }
        }

        let info = executor_of(&Weighted);
        assert_eq!(info.model, "ggml-base", "报的不是权重标识");
        assert_ne!(info.model, "本地 Whisper", "展示名被当成了模型名");
        assert_ne!(info.model, "local-whisper", "provider id 被当成了模型名");
        assert_eq!(info.display_name, "本地 Whisper");
        assert_eq!(info.kind, ExecutorKind::LocalRuntime);
    }

    /// 一条腿在云上，整个组合就是云——否则严格本地模式的既有过滤会放行它。
    #[test]
    fn any_cloud_leg_makes_the_composite_cloud() {
        let cloud = FakeLeg::new("cloud", ProviderLocation::Cloud, vec![]);
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![]);
        let mixed = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);
        assert_eq!(mixed.descriptor().location, ProviderLocation::Cloud);
        assert_eq!(mixed.capabilities().location, ProviderLocation::Cloud);

        let both_local = FailoverAsrProvider::new(
            FakeLeg::new("l1", ProviderLocation::Device, vec![]),
            local as Arc<dyn StreamingAsr>,
        );
        assert_eq!(both_local.descriptor().location, ProviderLocation::Device);
    }

    /// 语言设置必须跟着降级走——否则用户选的语言在切回本机后悄悄失效。
    #[test]
    fn language_setting_reaches_a_leg_that_joins_later() {
        let recorded: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        struct Recorder(Arc<Mutex<Vec<Option<String>>>>);
        impl StreamLangControl for Recorder {
            fn set_configured(&self, configured: Option<String>) {
                self.0.lock().unwrap().push(configured);
            }
            fn auto_detected(&self) -> Option<String> {
                None
            }
        }
        let fanout = FanoutLang::default();
        fanout.set_configured(Some("zh".into()));
        fanout.join(Arc::new(Recorder(Arc::clone(&recorded))));
        assert_eq!(
            recorded.lock().unwrap().as_slice(),
            [Some("zh".to_owned())],
            "后加入的腿没有拿到已设的语言"
        );
    }

    /// 降级判据本身：只有「链路当下不通」才切。
    #[test]
    fn only_connectivity_failures_fail_over() {
        assert!(should_fail_over(&JobError::Timeout));
        assert!(should_fail_over(&JobError::ProviderUnavailable {
            provider: ProviderId::from("x")
        }));
        // 严格本地掐断 → 降级到本机（他要的是「别外发」，不是「别转写」）。
        assert!(should_fail_over(&JobError::StrictLocalBlocked {
            capability: luna_core::Capability::Transcribe
        }));
        assert!(!should_fail_over(&JobError::Cancelled));
        assert!(!should_fail_over(&JobError::InvalidInput("x".into())));
        assert!(should_fail_over(&JobError::CapabilityNotAllowed {
            provider: luna_core::ProviderId::from("remote-asr-stream"),
        }));
        assert!(!should_fail_over(&JobError::QuotaExhausted {
            dimension: None
        }));
        assert!(!should_fail_over(&JobError::AccountFrozen {
            provider: luna_core::ProviderId::from("remote-asr-stream"),
        }));
        assert!(!should_fail_over(&JobError::SpendCapReached {
            provider: luna_core::ProviderId::from("remote-asr-stream"),
        }));
        assert!(!should_fail_over(&JobError::FreeQuotaWithheld {
            provider: luna_core::ProviderId::from("remote-asr-stream"),
        }));
    }

    /// **停止录音恰好撞上切腿，也不能留下一条永不收尾的腿。**
    ///
    /// 泵收工之后才注册出来的兜底腿，是这条不变量唯一的漏点：采集侧已经关流了，没有任何
    /// 东西会再喂它，而它自己在等下一窗——本机腿是一个不退出的解码线程，远程腿是一条不发
    /// `END`、不关闭的 WebSocket。换腿只走泵那一条通道，正是为了让这种腿随 `SendError`
    /// 当场被丢掉。
    ///
    /// 反向验证（已实跑）：让那份新腿在泵之外活下来（把 `switch.send(...)` 的 `Err` 泄漏
    /// 掉即可，等价于此前那个共享容器），本用例在 2 秒有界等待后判失败——兜底腿永远等不到
    /// 关流。
    #[tokio::test]
    async fn a_leg_swapped_in_after_the_capture_side_stopped_is_closed_too() {
        let (stream_id, job) = stream_job();
        let cloud = FakeLeg::new(
            "cloud",
            ProviderLocation::Cloud,
            vec![JobEvent::Failed(JobError::ProviderUnavailable {
                provider: ProviderId::from("cloud"),
            })],
        );
        let local = FakeLeg::new("local", ProviderLocation::Device, vec![finished(&job)]);
        let provider = FailoverAsrProvider::new(cloud, Arc::clone(&local) as Arc<dyn StreamingAsr>);

        // 采集侧**先**关流（丢掉发送端）——用户按下停止的那一刻，云腿正好断了。
        let reg = provider.register_stream(stream_id);
        drop(reg.window_tx);

        let _events = run(&provider, job).await;

        assert!(
            local.registered.load(Ordering::Acquire),
            "用例没走到切腿那一支，它证明不了任何事"
        );
        assert!(
            local.wait_closed(),
            "泵收工后换进来的兜底腿没有收到关流——它会一直等下一窗，而没有任何东西会再喂它"
        );
    }
}
