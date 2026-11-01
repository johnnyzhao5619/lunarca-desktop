//! 远程实时转写 Provider（ADR-038）。
//!
//! 与批式的 [`crate::asr_remote`] 是**两条通道**：那条是「整段上传 → 建任务 → 轮询」，
//! 这条是「边说边传 → 边说边出，且已定稿的前缀永不回退」。用批式模拟实时会让每一小段
//! 都付一次建任务 + 排队 + 冷启的钱，延迟只会比本机更差（ADR-038 ①）。
//!
//! ```text
//! GET /inference/v1/asr/stream?lang=…   （Upgrade: websocket，Bearer 走请求头）
//!   上行：二进制帧 = **一个 Opus 包**（16 kHz mono，VoIP 档，20 ms/帧）
//!         文本帧 "END" = 说完了
//!   下行：{"executed_by":{…}} / {"committed":[Segment…]} / {"hypothesis":[Segment…]}
//!         / {"error":"CODE"}
//! ```
//!
//! # 接缝
//!
//! 实现 [`luna_core::StreamingAsr`]：采集侧经 `register_stream` 拿句柄投窗，**不知道
//! 自己在喂本机 whisper 还是这里**。两条通道拿同一种句柄，是「断线切回本地」的前提。
//!
//! # 四条定死的
//!
//! 1. **`location: Cloud`**——严格本地模式按它过滤。麦克风内容是隐私量级最高的一类，
//!    标错就等于在「仅本地」下把用户说的每句话发出去，且不会有任何报错。
//! 2. **上行 Opus**（ADR-038 ③）：f32 直传是 512 kbps、16-bit PCM 是 256 kbps，
//!    两者在移动网络上都不成立。参数与依据全在 [`crate::opus_uplink`]，本文件不复制。
//! 3. **committed 只追加**：定稿过 [`CommittedLedger`]，只把增量发给下游。定稿一旦
//!    发出，用户可能已经复制或导出过它。
//! 4. **断线先进静默重连窗，再谈降级**（ADR-038 ⑧）：瞬断不等于这条腿不能用了，
//!    而 `asr_failover` 的移交是单向的。窗、退避与重放缓冲全在 [`crate::asr_reconnect`]。
//!
//! # 每一次外发都问闸，重放也不例外
//!
//! ADR-038 ④ 的 `OutboundGate` 要的是「用户在会议中途按下『仅本地』，麦克风内容**当场**
//! 停止外发」。重连后的重放是一次不折不扣的外发——它发的还是断线之前录下的音频，
//! 而用户可能正是在断线期间按下那个开关的。让重放绕过闸，等于给这条隐私硬拦开了一个
//! 只有抓包才看得见的口子。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_stream::stream;
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, CapabilitySet, CommittedLedger, ExecutorInfo,
    ExecutorKind, Job, JobError, JobEvent, JobEventStream, JobKind, JobResult, ModelId,
    OutboundGate, PartialPayload, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation,
    ReconnectPhase, ResultPayload, Segment, StreamId, StreamLangControl, StreamRegistration,
    StreamRegistry, StreamingAsr, Timestamp, Timings, Usage, UsageEstimate, WindowInput,
};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use crate::asr_reconnect::{frame_offset, watermark_frame, ReconnectWindow, ReplayLog};
use crate::asr_remote::BearerSource;
use crate::opus_uplink::{OpusUplink, UPLINK_SAMPLE_RATE};

/// 本 Provider 的稳定 id。
pub const REALTIME_ASR_PROVIDER_ID: &str = "remote-asr-stream";

/// 窗口队列深度。
///
/// 采集侧是实时的：队列满时**丢窗比阻塞好**——阻塞会一路顶到音频回调，
/// 那是 ADR-009 的红线（回调里不许有任何等待）。
const WINDOW_QUEUE: usize = 64;

/// 取消轮询间隔。
///
/// [`CancelToken`] 是个原子布尔、没有异步唤醒，只能轮。不能只在「收到窗口时」顺带检查：
/// 客户端 VAD 让静音不上行（ADR-038 ②），于是用户在沉默中按下停止时可能长时间没有窗口
/// 到达——那恰恰是他最期待立刻停下的时刻。
const CANCEL_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// 「收到服务端认下的内容」之前，允许连续刷新几次凭据。
///
/// 有界的理由：`refresh` 是**一次性**的——每轮都消耗服务端一枚 refresh token
/// （见 [`BearerSource::refresh`]）。正常情形下 token 过期发生在长时间正常转写**之后**，
/// 一收到 committed/hypothesis 就把计数清零，长会议里每 ~15 分钟自然刷一次都落在预算内。
/// 只有「握手过了却每次都立刻回 unauthorized、且不吐任何内容」的坏服务端会连撞——撞到
/// 这个上界就当真过期，交给重新登录，而不是无休止地烧 refresh 空转（还每轮打一次网络）。
/// 取 3：一次真过期只需刷 1 次，留两次余量给偶发的双重 401。
const MAX_REAUTH_WITHOUT_PROGRESS: u32 = 3;

/// 上行统计的上报节流：每窗都打会淹掉 stdout，而排障要的是「此刻上行了多少、丢了多少」。
///
/// 取 5s 的推导：分段器在连续语音下每秒产数个窗（增长快照 + 终窗），逐窗打印的密度与
/// 音频本身同阶，读起来等于没读；节流窗必须覆盖多个窗才成其为"统计"。上界由现场判据定死
/// ——「说了一分钟、服务端只收到 3 秒」这个比对要靠**累计上行毫秒数 vs 墙钟**做，读数间隔
/// 若到分钟级，做一次比对就得等一分钟。5s 同时与 `luna-audio` 的电平分布窗、`recording`
/// 的跳窗节流一致：几条诊断在 stdout 上按同一节奏交替出现，肉眼可对齐时间轴。
const UPLINK_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 连续几次「接上了却一个字都没出」之后，判定这条腿不通。
///
/// **静默重连必须有界**（ADR-065 增补）。窗本身是有界的（5 秒），但**接上就把窗清掉**
/// ——于是「连上 → 服务端立刻正常关流 → 再连」这个循环每一圈都在窗内成功，永远不会
/// 走到 `ReconnectWindow::step()` 返回 `None` 那一支。真机上它长这样：服务端每 ~0.5 秒
/// 关一次流，客户端每次重连都把攒得更多的重放包发过去、再被关掉，**字幕从此不动、界面
/// 一句提示都没有**（`Resyncing` 刻意不打扰用户），而 ADR-038 ⑤ 的本机兜底永远不接管。
///
/// 判据是「有没有进展」而不是「重连了几次」：上游正常关流本身是合法且频繁的
/// （WhisperLive 每段解码收尾即关），只要每一圈都吐出了 committed / hypothesis，那就是
/// 它在正常工作，重连多少次都不该被打断——收到任何一帧内容即清零（同
/// [`MAX_REAUTH_WITHOUT_PROGRESS`] 的判据）。
///
/// 取 3：一次偶发的空会话（用户恰好在这一圈里没说话）不该触发降级，而连着三圈一个字
/// 都没有，等下去也不会有——那几秒之后本机 whisper 接管才是用户要的。
const MAX_RESYNC_WITHOUT_PROGRESS: u32 = 3;

/// 这条腿的**生命周期痕迹**（零内容，只有状态与稳定错误码）。
///
/// # 为什么必须有
///
/// 本模块此前**一行日志都没有**：会话有没有建起来、握手 401 之后换没换新令牌、重连到了
/// 第几圈、有没有交给本机兜底、为什么收场——全部不可见。代价在 2026-08-18 的现场诊断里
/// 兑现了：一次「字幕不出字」的定位，只能靠服务端的连接表与预留表反推客户端做了什么，
/// 而**服务端看不见客户端的决定**（刷新成功却没重连、切腿到本机、窗被丢弃，在服务端侧
/// 全都长成「一条安静的连接」）。
///
/// 走 `eprintln!` 而不是引入日志框架：本仓客户端没有装 `tracing_subscriber`（`RUST_LOG`
/// 不生效），而既有的 `[clock]` / `[engine]` 诊断就是这个形状——加一个只有这里用得上的
/// 框架，等于给一条诊断线配一整套基础设施。
///
/// **绝不带转写正文**（安全铁律：日志不含转写文本）。允许出现的只有三类：状态词、
/// [`JobError::code`] 那样的稳定码、以及**传输层错误文本**（连不上/超时/HTTP 状态）。
/// 第三类之所以安全，是因为凭据走 `Authorization` 头而**不在 URL 里**（见 `build_request`）
/// ——`ws_url` 只带 api_base 与语言码，故传输层错误里没有可泄露的东西。
fn trace(event: &str, detail: &str) {
    if detail.is_empty() {
        eprintln!("[asr/cloud] {event}");
    } else {
        eprintln!("[asr/cloud] {event} {detail}");
    }
}

/// `uplink.stats` 一行的成文口径（丢弃分支与正常分支共用，**格式不得分叉**——现场是
/// 靠这一行在时间轴上前后相减来判断"还在不在上行"的，两处写法不同就没法相减）。
///
/// 毫秒数按 [`UPLINK_SAMPLE_RATE`] 折算：那正是服务端计费的折算口径，于是「客户端说
/// 发了多少」可以直接和服务端账单对上（现场判据：说了一分钟、服务端只收到 3 秒）。
fn uplink_stats(uplinked_samples: u64, skipped_windows: u64) -> String {
    format!(
        "已上行 {}ms，整窗重发丢弃 {skipped_windows} 个",
        uplinked_samples * 1000 / u64::from(UPLINK_SAMPLE_RATE)
    )
}

pub struct RealtimeAsrConfig {
    /// Lunarca 服务端基址（登录的那台服务器）。
    pub api_base: String,
    pub bearer: Arc<dyn BearerSource>,
    /// 外发闸（严格本地随时可掐断这条腿）。见 [`OutboundGate`] 的论证。
    pub gate: Arc<dyn OutboundGate>,
}

/// 会话语言控制面。
///
/// 语言在**握手时**随查询串发给服务端，会话中途改需要重连。故 `set_configured` 记下新值
/// 供**下次**会话使用，而不假装当场生效——假装生效的话，用户在设置里改了语言、看着
/// 转写继续用旧语言出错，却没有任何东西告诉他要重开一次。
#[derive(Default)]
struct RemoteLang {
    configured: Mutex<Option<String>>,
    detected: Mutex<Option<String>>,
}

impl RemoteLang {
    fn configured(&self) -> Option<String> {
        self.configured.lock().expect("lang poisoned").clone()
    }

    fn observe(&self, language: &str) {
        *self.detected.lock().expect("lang poisoned") = Some(language.to_owned());
    }
}

impl StreamLangControl for RemoteLang {
    fn set_configured(&self, configured: Option<String>) {
        *self.configured.lock().expect("lang poisoned") = configured;
    }

    /// 服务端回报的**检测**语言（`{"language":"en"}` 帧）；尚未回报时为 `None`。
    ///
    /// 绝不拿 `configured` 顶替：把「用户设了什么」显示成「系统听出了什么」，
    /// 会让自动检测失灵时完全看不出来（`PRD-04-046` 的回显要答的正是后者）。
    fn auto_detected(&self) -> Option<String> {
        self.detected.lock().expect("lang poisoned").clone()
    }
}

struct StreamEntry {
    window_rx: std::sync::mpsc::Receiver<Option<WindowInput>>,
    infer_tx: std::sync::mpsc::SyncSender<u64>,
    lang: Arc<RemoteLang>,
}

pub struct RealtimeAsrProvider {
    config: RealtimeAsrConfig,
    /// 登记表的回收语义在 [`StreamRegistry`] 里只写一次（三个流式实现共用）——本文件
    /// 此前那份 `Mutex<HashMap<..>>` 只在 `execute` 里 `remove`，走不到 `execute` 的
    /// 会话（准入被拒 / 路由失败）就永久滞留，连同它攥着的窗缓冲。
    registry: StreamRegistry<StreamEntry>,
}

impl RealtimeAsrProvider {
    #[must_use]
    pub fn new(config: RealtimeAsrConfig) -> Self {
        Self {
            config,
            registry: StreamRegistry::new(),
        }
    }
}

impl StreamingAsr for RealtimeAsrProvider {
    fn register_stream(&self, id: StreamId) -> StreamRegistration {
        let (window_tx, window_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(WINDOW_QUEUE);
        let lang = Arc::new(RemoteLang::default());
        let release = self.registry.register(
            id,
            StreamEntry {
                window_rx,
                infer_tx,
                lang: Arc::clone(&lang),
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
impl AIProvider for RealtimeAsrProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(REALTIME_ASR_PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
            display_name: "服务端实时转写".to_owned(),
            // 同批式：模型由服务端选，随首帧 `executed_by` 回报。
            model: None,
            // 见模块头第 1 条。
            location: ProviderLocation::Cloud,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Transcribe].into_iter().collect(),
            streaming: true,
            location: ProviderLocation::Cloud,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // 不主动探活：探一次就是一次云网络行为（ADR-037 零云网络行为不变量）。
        // 真实可用性在建会话时暴露，而那时用户是显式按下了录音。
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let JobKind::Transcribe { audio, .. } = job.kind.clone() else {
            return once_failed(JobError::InvalidInput("非转写 Job".into()));
        };
        let AudioRef::Stream(stream_id) = audio else {
            // 说清是「路由到了错误的 Provider」，而不是含糊的「输入非法」——
            // 后者会让人去查音频文件本身。
            return once_failed(JobError::InvalidInput(
                "服务端实时转写只接受 AudioRef::Stream（文件/PCM 走批式转写 Provider）".into(),
            ));
        };
        let Some(entry) = self.registry.take(stream_id) else {
            return once_failed(JobError::InvalidInput(
                "未注册的流式会话——采集侧必须先 register_stream".into(),
            ));
        };
        let StreamEntry {
            window_rx,
            infer_tx,
            lang,
        } = entry;

        // 严格本地：连都不连。放在取凭据**之前**——先探一次服务端再发现不该发，
        // 已经是一次可观测的云网络行为了。
        if !self.config.gate.allows_outbound() {
            return once_failed(JobError::StrictLocalBlocked {
                capability: Capability::Transcribe,
            });
        }
        let Some(token) = self.config.bearer.bearer().await else {
            // 未登录时一个字节都不发（ADR-037）。
            return once_failed(unauthorized());
        };

        let url = ws_url(&self.config.api_base, lang.configured().as_deref());
        let gate = Arc::clone(&self.config.gate);
        // 凭据来源要**跨重连存活**：会话可能跨过 access token 的有效期，届时收到
        // unauthorized（帧 / 握手 401）时得能换一枚再连（TDD-14 §3.3.1 ⑩、ADR-038 ⑧）。
        let bearer = Arc::clone(&self.config.bearer);
        let job_id = job.id;

        Box::pin(stream! {
            trace("session.start", "");
            yield JobEvent::Queued;

            // 建连用的凭据。会话中途可被 `bearer.refresh` 换枚（见循环顶部的刷新块）。
            let mut token = token;

            // **整场会话一个编码器**（见 `OpusUplink` 顶注）：重连不重建，重放的包
            // 与其后的新包出自同一条编码轨迹。
            let mut uplink = match OpusUplink::new() {
                Ok(u) => u,
                // 建不起编码器 = 这条腿此刻不能用。归 `ProviderUnavailable` 让兜底接管
                // ——用户要的是字幕，不是一句他无法处理的编解码错误。
                Err(_) => { yield JobEvent::Failed(unavailable()); return; }
            };

            // 采集侧给的是同步 `Receiver`（那条通道服务于本机引擎的阻塞线程池）。
            // 在阻塞线程里读、经 tokio channel 转进来——**不要在 async 上下文里直接
            // recv**，那会占住一个 runtime worker（与 ADR-010 同一条纪律）。
            // 采集侧丢掉 `window_tx` 时 `recv` 返回 Err，这个线程随之退出。
            //
            // 这条桥**跨重连存活**：断线期间采集侧还在说话，窗照常流进来、照常编码
            // 进重放缓冲（见 `asr_reconnect`）。每次重连都重建一次的话，退避那几秒的
            // 音频就没有任何地方接得住。
            let (win_tx, mut win_rx) =
                tokio::sync::mpsc::channel::<Option<WindowInput>>(WINDOW_QUEUE);
            tokio::task::spawn_blocking(move || {
                while let Ok(item) = window_rx.recv() {
                    let last = item.is_none();
                    if win_tx.blocking_send(item).is_err() || last {
                        break;
                    }
                }
            });

            // 唯一一处执行「committed 不回退」的地方（`CommittedLedger` 顶注）。
            let mut committed = CommittedLedger::default();
            // 水位线之后、服务端尚未认下的上行包。
            let mut replay = ReplayLog::default();
            // 采集侧的会话绝对采样游标：只上行它之后的真实语音。
            let mut sent_upto = 0u64;
            let mut uplink_done = false;
            // **当前连接的时间戳零点**。服务端每个连接从零重计，故重连之后它不再是 0
            // ——不校正的话，重连后的「0.0 秒」会和会话开头的「0.0 秒」撞在一起。
            let mut base;
            // 断线重连窗；`None` = 链路正常。
            let mut window: Option<ReconnectWindow> = None;
            // 待刷新凭据：收到 unauthorized（帧 / 握手 401 / 关连接前的错误帧）时置位，
            // 由循环顶部**唯一**一处换枚 token 再重连——把 refresh 集中在一处，避免每个
            // 触发点各写一遍又各错一遍（三处触发点见下）。
            let mut pending_reauth = false;
            // 「收到服务端认下的内容」之前允许连续刷新几次——见 `MAX_REAUTH_WITHOUT_PROGRESS`。
            let mut reauth_budget: u32 = 0;
            // 连续几次「接上了却一个字都没出」——见 `MAX_RESYNC_WITHOUT_PROGRESS`。
            let mut resync_without_progress: u32 = 0;
            let mut announced_start = false;
            // **游标丢弃也要有数**：下面那句 `fresh_after(sent_upto) else { continue }` 是
            // 这条链路上最后一处静默丢弃——窗产出了、没被跳、没被丢，却可能一个字节都不上行
            // （2026-08-18 现场：说了一分钟，服务端只收到 3 秒）。丢弃本身是对的（分段器给的是
            // 重解窗，整窗重发必须不发），但**丢了多少、游标停在哪**必须看得见，否则"客户端没发"
            // 与"服务端没收到"在现场无法区分。零内容，只有样本计数。
            let mut skipped_windows: u64 = 0;
            let mut uplinked_samples: u64 = 0;
            let mut last_uplink_report = std::time::Instant::now();
            let mut ticker = tokio::time::interval(CANCEL_POLL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            'session: loop {
                // ── 凭据刷新（收到 unauthorized 后置位；唯一一处 refresh）──────────
                // 会话跨过 access token 有效期时**换一枚再连**，而不是整条中止提示重登
                // （帧路径），也不是用旧 token 干等到窗关移交本机（握手 401 路径）。
                // 与 `send_authed`/`cloud_llm` 同一语义：传刚被拒的那枚（`BearerSource::refresh`
                // 据此分辨真过期与「别人已经换好」，TDD-14 §3.3.1 ⑩）。
                if std::mem::take(&mut pending_reauth) {
                    // 用户可能正是在断线期间按下「仅本地」：当场停，别把它当成登录问题。
                    if !gate.allows_outbound() {
                        yield JobEvent::Failed(JobError::StrictLocalBlocked {
                            capability: Capability::Transcribe,
                        });
                        return;
                    }
                    // 握手过了却每次都立刻 unauthorized、且不吐任何内容：当真过期，交给重新
                    // 登录，而不是无休止地烧服务端的一次性 refresh（见常量说明）。
                    if reauth_budget >= MAX_REAUTH_WITHOUT_PROGRESS {
                        yield JobEvent::Failed(unauthorized());
                        return;
                    }
                    reauth_budget += 1;
                    trace("reauth.refresh", &format!("budget={reauth_budget}"));
                    let Some(fresh) = bearer.refresh(&token).await else {
                        trace("reauth.failed", "刷新换不出会话，收在 unauthorized");
                        // 刷新也换不出可用会话 = 真的要重新登录（ADR-037 未登录零外发）。
                        yield JobEvent::Failed(unauthorized());
                        return;
                    };
                    token = fresh;
                    // 首次进入刷新时开窗——刷新换枚重连仍在 ADR-038 的重连窗语义内，
                    // 已开的窗不重置（否则「握手 ok 却反复 401 帧」会把窗一直续着）。
                    if window.is_none() {
                        window = Some(ReconnectWindow::open());
                        yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                    }
                }

                // ── 退避（只有重连才有；首次建连直接往下走）────────────────────
                if let Some(w) = window.as_mut() {
                    let Some(delay) = w.step() else {
                        // 窗关了才是「这条腿不通」。交给既有 failover（ADR-038 ⑤）。
                        trace("giveup.window_closed", "5 秒重连窗用尽，交给本机兜底");
                        yield JobEvent::Failed(unavailable());
                        return;
                    };
                    let deadline = tokio::time::Instant::now() + delay;
                    loop {
                        tokio::select! {
                            biased;
                            _ = tokio::time::sleep_until(deadline) => break,
                            // 断线不等于用户停了说话：照常收窗、照常编码进重放缓冲。
                            incoming = win_rx.recv(), if !uplink_done => match incoming {
                                Some(Some(w)) => {
                                    let Some(fresh) = w.fresh_after(sent_upto) else { continue };
                                    let Ok(packets) = uplink.push(fresh) else {
                                        yield JobEvent::Failed(unavailable());
                                        return;
                                    };
                                    for packet in packets {
                                        replay.record(packet);
                                    }
                                    sent_upto = w.span().end;
                                }
                                // 采集侧关流：记下来，接上之后先重放、再说 "END"。
                                Some(None) | None => uplink_done = true,
                            },
                            _ = ticker.tick() => {
                                if cancel.is_cancelled() {
                                    yield JobEvent::Cancelled;
                                    return;
                                }
                            }
                        }
                    }
                }

                // ── 建连 ──────────────────────────────────────────────────────
                // 每一跳都重新构造升级请求：`http::Request` 不可克隆，而把它提到循环
                // 外「构造一次反复用」在类型上就不成立——不是风格问题。
                let request = match build_request(&url, &token) {
                    Ok(r) => r,
                    Err(e) => { yield JobEvent::Failed(e); return; }
                };
                // 自签部署钉了根证书（ADR-153）就只认那张根；没钉走系统信任库（`None`）。
                let connector = match luna_model_fetch::pinned_tls::native_tls_connector(&url) {
                    Ok(connector) => connector.map(tokio_tungstenite::Connector::NativeTls),
                    // 不落回系统信任库；按「服务端不可用」交给兜底回落本机（同建连失败）。
                    Err(e) => {
                        eprintln!("[asr-realtime] 钉住的根证书构造 TLS 连接器失败：{e}");
                        yield JobEvent::Failed(unavailable());
                        return;
                    }
                };
                let attempt =
                    tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector);
                // 重连的建连必须被**剩余窗**封顶：一个挂住的 TCP 连接会把整个窗吃掉，
                // 而用户在那几秒里既没有云端字幕，也还没等到本机接管。
                // `Some(Ok)` = 接上；`Some(Err)` = 建连出错（可能是握手 401）；`None` = 超窗。
                let connect_result = match window.as_ref() {
                    Some(w) => tokio::time::timeout(w.remaining(), attempt).await.ok(),
                    None => Some(attempt.await),
                };
                let socket = match connect_result {
                    Some(Ok((socket, _))) => socket,
                    // 握手 401：会话跨过了 access token 有效期。置位刷新、换一枚再连，
                    // 而不是用旧 token 一路重试到窗关、单向降级本机 whisper 跑整场最小档
                    // （ADR-038 ⑧、TDD-14 §3.3.1 ⑩）。403/其它状态不是过期，不触发刷新。
                    Some(Err(err)) if is_unauthorized_handshake(&err) => {
                        trace("handshake.401", "将刷新凭据后重连");
                        pending_reauth = true;
                        continue 'session;
                    }
                    // 握手被账户状态拒绝（403 `account_frozen`，C3；402 `spend_cap_reached`，C4；
                    // 402 `free_quota_withheld`，C5）：
                    // 重连多少次都一样——当场以结构性错误收场，不开重连窗、不回落本机（同额度用尽）。
                    Some(Err(err)) if refused_handshake(&err).is_some() => {
                        let code = refused_handshake(&err).unwrap_or(crate::cloud_llm::ACCOUNT_FROZEN);
                        trace("handshake.refused", code);
                        yield JobEvent::Failed(map_stream_error(code));
                        return;
                    }
                    // 其它建连错误 / 超窗：连不上**不直接触发降级**（ADR-038 ⑧）——先开窗
                    // 安静地重试。移交本机是单向的，一次几百毫秒的瞬断不该让剩下一小时都跑在
                    // 最小档上。窗关了才归 `ProviderUnavailable`，Router 的兜底链据此回落
                    // 本机 whisper（ADR-038 ⑤）。
                    Some(Err(err)) => {
                        trace("connect.failed", &format!("err={err}"));
                        if window.is_none() {
                            window = Some(ReconnectWindow::open());
                            yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                        }
                        continue 'session;
                    }
                    None => {
                        trace("connect.timeout", "重连窗内没接上");
                        if window.is_none() {
                            window = Some(ReconnectWindow::open());
                            yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                        }
                        continue 'session;
                    }
                };
                trace("connected", "");
                if !announced_start {
                    announced_start = true;
                    yield JobEvent::Started;
                }

                let (mut ws_tx, mut ws_rx) = socket.split();

                // ── 重放（水位线之后的那些包）────────────────────────────────
                // 新连接的时间戳零点 = 我们即将重放的第一帧；没有待重放的（全都定稿了）
                // 就是下一个将要产出的帧。两者共用 `OpusUplink` 的帧计数器，故
                // 「重放对齐了、时间戳没对齐」这种只能靠听出来的错不会发生。
                base = frame_offset(replay.first_frame().unwrap_or_else(|| uplink.frames_emitted()));
                let pending: Vec<Vec<u8>> = replay.pending().map(|p| p.bytes.clone()).collect();
                let mut replay_broke = false;
                for bytes in pending {
                    // 重放也要问闸（见模块头）：用户可能正是在断线期间按下「仅本地」的。
                    if !gate.allows_outbound() {
                        let _ = ws_tx.close().await;
                        yield JobEvent::Failed(JobError::StrictLocalBlocked {
                            capability: Capability::Transcribe,
                        });
                        return;
                    }
                    if ws_tx.send(Message::Binary(bytes)).await.is_err() {
                        replay_broke = true;
                        break;
                    }
                }
                if replay_broke {
                    if let Some(reason) = drain_for_reason(&mut ws_rx).await {
                        // 服务端关连接前说的是 unauthorized：换枚重连（循环顶部刷新），
                        // 不在此中止。其余原因（额度耗尽…）照旧各归各的。
                        if matches!(reason, JobError::Unauthorized { .. }) {
                            pending_reauth = true;
                            continue 'session;
                        }
                        yield JobEvent::Failed(reason);
                        return;
                    }
                    if window.is_none() {
                        window = Some(ReconnectWindow::open());
                        yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                    }
                    continue 'session;
                }
                // 断线期间采集侧已经关流了：重放完就说完了。
                if uplink_done {
                    let _ = ws_tx.send(Message::Text("END".to_owned())).await;
                }
                if window.take().is_some() {
                    // **接上不等于通了**：这一圈若又什么都没出，就是在原地转圈。窗被清掉
                    // 之后重连次数本身没有上界（见 `MAX_RESYNC_WITHOUT_PROGRESS`），故在这里
                    // 记一笔；收到任何 committed / hypothesis 即清零。
                    resync_without_progress += 1;
                    trace("resync", &format!("第 {resync_without_progress} 圈接上但尚无产出"));
                    if resync_without_progress > MAX_RESYNC_WITHOUT_PROGRESS {
                        trace("giveup.no_progress", "连续接上却零产出，交给本机兜底");
                        // 交给既有 failover（ADR-038 ⑤）：用户要的是字幕，本机 whisper 恒在。
                        yield JobEvent::Failed(unavailable());
                        return;
                    }
                    // 重连后这条新连接会**重解刚重放的那段音频**，服务端从零重计、又吐出
                    // 相同文本，`shift(base)` 会把它推到「更晚」的绝对区间——单靠时间判据挡不住，
                    // 会把同一句复读成十几条（缺陷 B）。告诉账本进入 resume：接下来按文本判重，
                    // 直到放行了确属新的内容（ADR-065、`CommittedLedger::begin_resume`）。
                    committed.begin_resume();
                    // 窗内接上了——界面据此撤掉「重连中」。
                    yield JobEvent::Reconnecting(ReconnectPhase::Resumed);
                }

                // ── 正常收发 ──────────────────────────────────────────────────
                loop {
                    tokio::select! {
                        biased;
                        // 下行优先：已解出的结果尽快送达（延迟预算，ADR-038 ②）。
                        incoming = ws_rx.next() => match incoming {
                            Some(Ok(Message::Text(text))) => match parse_frame(&text) {
                                Frame::Executor(info) => yield JobEvent::ExecutedBy(info),
                                Frame::Committed(segments) => {
                                    // 收到认下的内容 = 这枚 token 确实在转写：清零刷新预算，
                                    // 长会议里每 ~15 分钟自然过期一次都落在预算内（见常量说明）。
                                    reauth_budget = 0;
                                    // 「有进展」的判据是**帧里真的有内容**，不是「收到过一帧」：
                                    // 一个只会回空列表的服务端在协议上完全合法，而它一个字也没转出来
                                    // ——拿空帧清零，`MAX_RESYNC_WITHOUT_PROGRESS` 那道上界就永远
                                    // 挂不上。（凭据那一笔判据不同：空帧同样证明这枚 token 被认了。）
                                    if !segments.is_empty() {
                                        resync_without_progress = 0;
                                    }
                                    let fresh = committed.admit(shift(segments, base));
                                    // 服务端认下的那段音频不必再留着重放。
                                    replay.retain_from(watermark_frame(committed.end()));
                                    if !fresh.is_empty() {
                                        yield JobEvent::Partial(PartialPayload::Asr {
                                            committed_delta: fresh,
                                            hypothesis: String::new(),
                                            stability: 1.0,
                                        });
                                    }
                                }
                                Frame::Hypothesis(text) => {
                                    // 暂定尾也是「token 在干活」的实据：同 committed，清零预算。
                                    reauth_budget = 0;
                                    // 同上：空的暂定尾不算进展。
                                    if !text.trim().is_empty() {
                                        resync_without_progress = 0;
                                    }
                                    yield JobEvent::Partial(PartialPayload::Asr {
                                        committed_delta: Vec::new(),
                                        hypothesis: text,
                                        // 暂定尾按定义就是会变的。给一个居中值，而不是编一个
                                        // 精确到小数的「稳定度」——我们并不知道那个数。
                                        stability: 0.5,
                                    });
                                }
                                Frame::Error(code) => {
                                    // unauthorized = 会话中途 token 过期：置位刷新、换一枚再
                                    // 重连（循环顶部统一处理），而不是整条中止提示重登——用户
                                    // 其实只需要一枚新 token（TDD-14 §3.3.1 ⑩、ADR-038 ⑧）。
                                    // 其余码（额度耗尽 / 上游不可用…）仍按 `map_stream_error` 归位。
                                    if code == "unauthorized" {
                                        trace("frame.unauthorized", "会话中途令牌过期，刷新后重连");
                                        pending_reauth = true;
                                        continue 'session;
                                    }
                                    trace("frame.error", &format!("code={code}"));
                                    yield JobEvent::Failed(map_stream_error(
                                        &code,
                                    ));
                                    return;
                                }
                                // 语言只更新会话控制面：采集侧按自己的节奏读它去回显，
                                // 不必为此产生一个 `JobEvent`（那是转写结果的通道）。
                                Frame::Language(code) => lang.observe(&code),
                                Frame::Ignored => {}
                            },
                            // 收尾**之后**的收场 = 会话结束，不是链路故障。
                            //
                            // 我们已经说过「没有更多音频了」，此时对端收工时未必留下一个
                            // 干净的 Close 帧。把它当成断线会做两件错事：为一个**已经结束**
                            // 的会话白白重连（窗里什么都不会发生），并在窗关时触发一次
                            // 本不该发生的降级（ADR-038 ⑤ 会真的被触发）。
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) if uplink_done => {
                                break 'session;
                            }
                            // 收尾**之前**服务端**优雅关流**（正常 close code）而采集侧还在
                            // 说话：上游把这条流结束了（WhisperLive 每段解码收尾即无条件
                            // `socket.close()`、不带错误帧），会话要继续就得再连一条——但收到
                            // Close **帧**本身就说明传输层没断，这**不是网络故障**（缺陷 A：
                            // 归成断线会让「网络不稳」横幅在链路完全正常时几乎常驻）。故走
                            // **静默重连**（`Resyncing`）：照样开窗、重连、重放，但界面不打扰
                            // 用户（ADR-065、ADR-038 ⑧）。真断连（无 close 握手）走下面那一支。
                            Some(Ok(Message::Close(frame))) if close_is_graceful(frame.as_ref()) => {
                                trace("upstream.closed", "正常码，静默重连");
                                window = Some(ReconnectWindow::open());
                                yield JobEvent::Reconnecting(ReconnectPhase::Resyncing);
                                continue 'session;
                            }
                            // 收尾之前的**真断连**：传输层 `Err` / 没有 close 握手的 `None` /
                            // 带异常 code 的 Close。服务端主动关闭前若有可执行原因，已作为 JSON
                            // 错误帧在上面那一支处理掉了；走到这里就是真的断了，弹「网络不稳」。
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                                window = Some(ReconnectWindow::open());
                                yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                                continue 'session;
                            }
                            Some(Ok(_)) => {}
                        },
                        window_in = win_rx.recv(), if !uplink_done => match window_in {
                            Some(Some(w)) => {
                                // 只上行**游标之后**的真实语音。
                                //
                                // 分段器给的是重解窗：同一句每 1.5 秒重发一次全量、补零到
                                // 10 秒、强切后还带 250 ms overlap（`WindowInput` 顶注有完整
                                // 论证）。照单发出去的话，一句 10 秒的话要传 ~70 秒的字节——
                                // 服务端按上行折算秒数计费，用户付 7 倍；而「麦克风内容持续
                                // 离开设备」这个 ADR-038 ④ 明说无法消除的代价，也跟着放大 7 倍。
                                // 每一窗都问一次闸：用户在一场一小时的会议中途按下
                                // 「仅本地」，麦克风内容必须**当场**停止外发，而不是等这场
                                // 录完。只在建连时查一次的话，开关看起来生效了、实际没有——
                                // 不报错、不留痕，只有抓包才看得见。
                                if !gate.allows_outbound() {
                                    let _ = ws_tx.close().await;
                                    yield JobEvent::Failed(JobError::StrictLocalBlocked {
                                        capability: Capability::Transcribe,
                                    });
                                    return;
                                }
                                let Some(fresh) = w.fresh_after(sent_upto) else {
                                    // 整窗都是重发，一个字节都不发——但要计数（见上）。
                                    skipped_windows += 1;
                                    if last_uplink_report.elapsed() >= UPLINK_REPORT_INTERVAL {
                                        last_uplink_report = std::time::Instant::now();
                                        let span = w.span();
                                        trace(
                                            "uplink.stats",
                                            &format!(
                                                "{}（游标 {sent_upto}，本窗 {}..{}）",
                                                uplink_stats(uplinked_samples, skipped_windows),
                                                span.start,
                                                span.end
                                            ),
                                        );
                                    }
                                    continue;
                                };
                                uplinked_samples += fresh.len() as u64;
                                if last_uplink_report.elapsed() >= UPLINK_REPORT_INTERVAL {
                                    last_uplink_report = std::time::Instant::now();
                                    trace(
                                        "uplink.stats",
                                        &uplink_stats(uplinked_samples, skipped_windows),
                                    );
                                }
                                let started = Instant::now();
                                let Ok(packets) = uplink.push(fresh) else {
                                    yield JobEvent::Failed(unavailable());
                                    return;
                                };
                                sent_upto = w.span().end;
                                let mut broke = false;
                                for packet in packets {
                                    // **先入重放缓冲再发**：发失败的那个包也必须留着，
                                    // 否则重连之后它就是一个谁也不知道的空洞。
                                    let bytes = packet.bytes.clone();
                                    replay.record(packet);
                                    if ws_tx.send(Message::Binary(bytes)).await.is_err() {
                                        broke = true;
                                        break;
                                    }
                                }
                                if broke {
                                    // 发不出去**往往不是"链路坏了"**，而是对端刚刚主动关了
                                    // 连接——而它关之前几乎总会先发一个错误帧说明原因
                                    // （额度用尽 / 未登录 / 没有可用上游）。那一帧此刻还躺在
                                    // 下行缓冲里没被读。
                                    //
                                    // 归成断线会做两件错事：把一个**可执行**的错误拖进重连窗
                                    // 白等五秒（而重试一万次都不会成功），并在窗关时触发一次
                                    // 本不该发生的降级——`asr_failover` 里「额度用尽不降级」的
                                    // 刻意排除会因此失效，用户于是几周后才发现自己的云额度
                                    // 早就没了。
                                    // （2026-08-08 真机：把额度压到 2 秒，客户端收到的是连接错误。）
                                    if let Some(reason) = drain_for_reason(&mut ws_rx).await {
                                        // 服务端关连接前说的是 unauthorized：换枚重连（循环
                                        // 顶部刷新），不在此中止。其余原因照旧各归各的。
                                        if matches!(reason, JobError::Unauthorized { .. }) {
                                            pending_reauth = true;
                                            continue 'session;
                                        }
                                        yield JobEvent::Failed(reason);
                                        return;
                                    }
                                    window = Some(ReconnectWindow::open());
                                    yield JobEvent::Reconnecting(ReconnectPhase::Retrying);
                                    continue 'session;
                                }
                                // 本机那条通道报的是推理耗时，这里报上行耗时——对「这一窗
                                // 花了多久」是同一个问题。`try_send`：消费侧不读时丢掉这条
                                // 统计，绝不因为一个指标把音频链路顶住。
                                let _ = infer_tx.try_send(started.elapsed().as_millis() as u64);
                            }
                            // 采集侧关流：把余样本补零成整帧发出去，再告诉服务端说完了，
                            // 然后继续读下行直到它收尾。
                            Some(None) | None => {
                                trace("uplink.done", "采集侧关流，发 END 后等上游收尾");
                                uplink_done = true;
                                if let Ok(Some(tail)) = uplink.flush() {
                                    let bytes = tail.bytes.clone();
                                    replay.record(tail);
                                    let _ = ws_tx.send(Message::Binary(bytes)).await;
                                }
                                let _ = ws_tx.send(Message::Text("END".to_owned())).await;
                            }
                        },
                        _ = ticker.tick() => {
                            if cancel.is_cancelled() {
                                // 关连接即取消：服务端据此停上游、按已用量结算
                                // （「只关下游、让上游跑完」是这条链路要防的事故）。
                                let _ = ws_tx.close().await;
                                yield JobEvent::Cancelled;
                                return;
                            }
                        }
                    }
                }
            }

            trace("session.finished", "");
            yield JobEvent::Finished(JobResult {
                job_id,
                provider_id: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
                model_id: Some(ModelId::from(REALTIME_ASR_PROVIDER_ID)),
                payload: ResultPayload::Segments(committed.into_segments()),
                // 计费口径在服务端（按解码后的 PCM 折算秒数）。客户端再报一份自己算的
                // 用量，只会和账单对不上——两个数字都「对」，用户不知道该信哪个。
                usage: Usage::default(),
                timings: Timings::default(),
                output_truncated: None,
                finished_at: Timestamp::ZERO,
            });
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        // 实时会话的时长事先不可知——服务端自己也是一段一段地预留（ADR-034）。
        None
    }
}

/// 服务端下行帧。
enum Frame {
    /// 谁在跑这次转写（PRD-17-033）。
    Executor(ExecutorInfo),
    Committed(Vec<Segment>),
    Hypothesis(String),
    Error(String),
    /// 上游检测到的语言（承 `PRD-04-046` 的「自动检测（中文）」回显）。
    Language(String),
    /// 认不出的帧**跳过而不是杀流**：服务端加一种新帧时，老客户端应当照常收后续分段。
    Ignored,
}

fn parse_frame(text: &str) -> Frame {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Frame::Ignored;
    };
    if let Some(code) = value.get("error").and_then(|v| v.as_str()) {
        return Frame::Error(code.to_owned());
    }
    if let Some(who) = value.get("executed_by") {
        let name = who.get("upstream_name").and_then(|v| v.as_str());
        let model = who.get("model").and_then(|v| v.as_str());
        let (Some(display_name), Some(model)) = (name, model) else {
            return Frame::Ignored;
        };
        return Frame::Executor(ExecutorInfo {
            display_name: display_name.to_owned(),
            // 缺字段 → `Unknown`，**不是** `LocalRuntime`：把不知道说成「没出组织」
            // 是一次实打实的隐私误述（见 `ExecutorKind::Unknown` 的理由）。
            kind: who
                .get("kind")
                .and_then(|v| v.as_str())
                .map_or(ExecutorKind::Unknown, ExecutorKind::parse),
            model: model.to_owned(),
            // 恒 `true`——见 `ExecutorInfo::leaves_device`：这条回报是**服务端**发来的，
            // 音频早已离开设备；服务端的 `kind` 描述的是它自己那一侧的边界。
            leaves_device: true,
        });
    }
    if let Some(language) = value.get("language").and_then(|v| v.as_str()) {
        return Frame::Language(language.to_owned());
    }
    if let Some(list) = value.get("committed") {
        return serde_json::from_value::<Vec<Segment>>(list.clone())
            .map_or(Frame::Ignored, Frame::Committed);
    }
    if let Some(list) = value.get("hypothesis") {
        let joined = serde_json::from_value::<Vec<Segment>>(list.clone())
            .map(|segs| {
                segs.iter()
                    .map(|s| s.text.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        return if joined.is_empty() {
            Frame::Ignored
        } else {
            Frame::Hypothesis(joined)
        };
    }
    Frame::Ignored
}

/// 服务端时间戳 → **会话**时间戳（ADR-038 ⑧）。
///
/// 服务端的时间轴以**每条连接**的第一个上行样本为零点，而会话可以跨好几条连接
/// （静默重连窗）。不校正的话，重连之后回来的第一段「0.0–1.2 秒」会与会话开头那段
/// 撞在一起：`CommittedLedger` 按绝对时间判重，于是它要么把真的新内容当成重复丢掉，
/// 要么把整场会话的开头再放行一遍——两种都只在弱网真机上才现形。
///
/// `base` 由帧序号算出（[`frame_offset`]），与重放的起点是同一个计数器。
fn shift(mut segments: Vec<Segment>, base: Duration) -> Vec<Segment> {
    if base.is_zero() {
        return segments;
    }
    for segment in &mut segments {
        segment.start += base;
        segment.end += base;
    }
    segments
}

/// `http(s)://host` + 语言 → `ws(s)://host/inference/v1/asr/stream?lang=…`。
fn ws_url(base: &str, lang: Option<&str>) -> String {
    let base = base.trim_end_matches('/');
    let origin = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    let mut url = format!("{origin}/inference/v1/asr/stream");
    // 语言只取 BCP-47 里那些安全字符——它来自配置文件，直接拼进查询串等于让一个
    // 手改的配置项能往请求行里注入任意内容。
    if let Some(lang) = lang.filter(|l| {
        !l.is_empty() && l.len() <= 16 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        url.push_str("?lang=");
        url.push_str(lang);
    }
    url
}

/// 服务端主动发来的 Close 帧是不是**优雅关流**（正常 close code）——据此把「上游正常
/// 结束了这条流」与「链路真的断了」分开（缺陷 A / ADR-065）。
///
/// 判据：能收到 Close **帧**本身就说明传输层没断（对端在一条活着的连接上完成了 close
/// 握手）。正常码（[`CloseCode::Normal`] 1000 / [`CloseCode::Away`] 1001 / 无帧）= 上游把
/// 这条流正常收尾了（WhisperLive 每段解码结束即无条件 `socket.close()`）——会话要继续得
/// 再连一条，但这不是网络故障，走静默重连、不弹「网络不稳」。真正的断连（Wi-Fi 掉、
/// 拔网线）留下的是没有 close 握手的传输层 `Err`/`None`，不会走到这里。异常码（如
/// [`CloseCode::Error`] 1011）当作问题对待，仍归「网络不稳」那一支。
fn close_is_graceful(frame: Option<&CloseFrame>) -> bool {
    match frame {
        None => true,
        Some(f) => matches!(f.code, CloseCode::Normal | CloseCode::Away),
    }
}

/// 建连握手是否以 **401** 收场——token 在长会话里过期了。
///
/// tungstenite 把「服务端对 Upgrade 回了非 101」表达成 [`Error::Http`]，我们只认 401：
/// 403/其它状态是别的问题（封禁、路由错），额度耗尽走的是连上后的错误帧，都不该触发
/// 凭据刷新。
///
/// [`Error::Http`]: tokio_tungstenite::tungstenite::Error::Http
fn is_unauthorized_handshake(err: &tokio_tungstenite::tungstenite::Error) -> bool {
    matches!(
        err,
        tokio_tungstenite::tungstenite::Error::Http(resp)
            if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED
    )
}

/// 建连握手是否被**账户状态**拒绝：403 `account_frozen`（纵切 3 C3）、402 `spend_cap_reached`
/// （本月预付上限，C4）或 402 `free_quota_withheld`（领不到免费额度，C5）。返回那个稳定码。只认「状态码 + 响应体稳定码」成对出现：别的 403 / 402
/// （封禁、路由错、额度用尽走错误帧）维持原处置。
fn refused_handshake(err: &tokio_tungstenite::tungstenite::Error) -> Option<&'static str> {
    use tokio_tungstenite::tungstenite::http::StatusCode;
    let tokio_tungstenite::tungstenite::Error::Http(resp) = err else {
        return None;
    };
    let code = resp
        .body()
        .as_deref()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
        .and_then(|body| {
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })?;
    match (resp.status(), code.as_str()) {
        (StatusCode::FORBIDDEN, crate::cloud_llm::ACCOUNT_FROZEN) => {
            Some(crate::cloud_llm::ACCOUNT_FROZEN)
        }
        (StatusCode::PAYMENT_REQUIRED, crate::cloud_llm::SPEND_CAP_REACHED) => {
            Some(crate::cloud_llm::SPEND_CAP_REACHED)
        }
        (StatusCode::PAYMENT_REQUIRED, crate::cloud_llm::FREE_QUOTA_WITHHELD) => {
            Some(crate::cloud_llm::FREE_QUOTA_WITHHELD)
        }
        _ => None,
    }
}

/// 带 Bearer 的升级请求。
///
/// 凭据走 HTTP 头、**不进 URL**：URL 会落进反代与浏览器历史的访问日志，
/// 把凭据放进去等于把它写进一堆我们控制不了的地方（隐私铁律：敏感数据不入查询串）。
fn build_request(
    url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, JobError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|e| JobError::InvalidInput(format!("服务端地址无效：{e}")))?;
    let value = format!("Bearer {token}")
        .parse()
        .map_err(|_| JobError::InvalidInput("凭据含非法字符".into()))?;
    request.headers_mut().insert("Authorization", value);
    Ok(request)
}

/// 对端关闭之后，把下行缓冲里**已经到达**的帧读完，看有没有它留下的原因。
///
/// 有界：只读已经到手的帧，且总时长封顶——挂在这里等一个可能永远不来的帧，比报错
/// 更糟（用户看到的是"一直在转写"，而其实什么都没发生）。
async fn drain_for_reason<S>(rx: &mut S) -> Option<JobError>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    const DRAIN: std::time::Duration = std::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + DRAIN;
    while let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout_at(deadline, rx.next()).await
    {
        if let Frame::Error(code) = parse_frame(&text) {
            return Some(map_stream_error(&code));
        }
    }
    None
}

/// 账户会话被拒 → 去重新登录（TDD-01 §2.4）。
fn unauthorized() -> JobError {
    JobError::Unauthorized {
        provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
    }
}

fn unavailable() -> JobError {
    JobError::ProviderUnavailable {
        provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
    }
}

/// 服务端稳定错误码 → `JobError`（TDD-16 §5.3 映射表）。
///
/// wire 值是**小写 snake_case**——服务端 `InferenceError::code()` 的原样输出，与批式
/// 通道同一套。这里曾经写成大写：编译、clippy、单测全绿，而每一个可执行错误都悄悄
/// 落进兜底分支，用户看到的是一句「服务端实时转写失败」，而不是「去账户页加额度」。
/// 下面的 `error_codes_match_the_server_wire_values` 把这几个串钉住。
///
/// `credential` 只影响 `unauthorized` 那一支：服务端只说「不认」，不认的是哪一份凭据
/// 由发出这条连接的那枚回答。
fn map_stream_error(code: &str) -> JobError {
    match code {
        "quota_exhausted" => JobError::QuotaExhausted {
            // 实时通道只可能撞上时长这一维（服务端按上行字节折算秒数）。
            dimension: Some("asr_seconds".to_owned()),
        },
        "unauthorized" => unauthorized(),
        crate::cloud_llm::CAPABILITY_NOT_ALLOWED => JobError::CapabilityNotAllowed {
            provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
        },
        // 账户被冻结（C3）：结构性，云腿到此为止——不重连、不回落本机（同额度用尽）。
        crate::cloud_llm::ACCOUNT_FROZEN => JobError::AccountFrozen {
            provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
        },
        // 本月预付上限（C4）：服务端按窗口预留，撞上即在下一个窗口发这一帧。结构性，云腿到此
        // 为止（已出的字幕保留）——不重连、不回落本机（同额度用尽）。
        crate::cloud_llm::SPEND_CAP_REACHED => JobError::SpendCapReached {
            provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
        },
        // 免费额度被扣下（C5）：同额度用尽——结构性，云腿到此为止（已出的字幕保留），不重连、
        // 不回落本机。
        crate::cloud_llm::FREE_QUOTA_WITHHELD => JobError::FreeQuotaWithheld {
            provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
        },
        // 上游不可用 → 让 Router 的兜底把它回落到本机 whisper。
        "upstream_unavailable" | "rate_limited" => unavailable(),
        other => JobError::Upstream {
            provider: ProviderId::from(REALTIME_ASR_PROVIDER_ID),
            code: Some(other.to_owned()),
            message: "服务端实时转写失败".to_owned(),
        },
    }
}

fn once_failed(error: JobError) -> JobEventStream {
    Box::pin(stream! { yield JobEvent::Failed(error); })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoToken;
    #[async_trait]
    impl BearerSource for NoToken {
        async fn bearer(&self) -> Option<String> {
            None
        }
        async fn refresh(&self, _rejected: &str) -> Option<String> {
            None
        }
    }

    fn provider() -> RealtimeAsrProvider {
        RealtimeAsrProvider::new(RealtimeAsrConfig {
            api_base: "https://example.test".into(),
            bearer: Arc::new(NoToken),
            gate: Arc::new(luna_core::AllowOutbound),
        })
    }

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: luna_core::SegmentId::new(),
            start: std::time::Duration::from_millis(start_ms),
            end: std::time::Duration::from_millis(end_ms),
            text: text.to_owned(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    fn wire(segments: &[Segment]) -> String {
        serde_json::to_string(segments).unwrap()
    }

    /// `location: Cloud`——麦克风内容是隐私量级最高的一类。
    ///
    /// 标成 `Local` 时编译、clippy、其余单测全绿，而「仅本地」模式下用户说的每句话
    /// 照样被发出去，且不会有任何报错。
    #[test]
    fn descriptor_is_honestly_cloud_and_streaming() {
        let p = provider();
        assert_eq!(p.descriptor().location, ProviderLocation::Cloud);
        let caps = p.capabilities();
        assert_eq!(caps.location, ProviderLocation::Cloud);
        assert!(
            caps.streaming,
            "实时档必须声明 streaming——否则 UI 不会准备接收 partial"
        );
        assert!(caps.capabilities.contains(&Capability::Transcribe));
    }

    /// 采集侧拿到的是**领域层句柄**，与本机 whisper 同一种（这是断线切回本地的前提）。
    #[test]
    fn register_stream_yields_the_domain_handle() {
        let p = provider();
        let reg = p.register_stream(StreamId::new());
        reg.lang.set_configured(Some("zh".into()));
        // 配置**不是**检测结果：服务端还没回报时必须是 `None`。拿 configured 顶替的话，
        // UI 会把「用户设了什么」显示成「系统听出了什么」——自动检测失灵时完全看不出来。
        assert_eq!(reg.lang.auto_detected(), None);
        reg.window_tx
            .send(Some(WindowInput {
                samples: vec![0.0; 8],
                abs_start: 0,
                speech_len: 8,
            }))
            .expect("投窗");
    }

    /// **`execute` 没跑到的会话，登记同样要回收。**
    ///
    /// 采集侧是先 `register_stream` 拿句柄、再 `submit(job)`，而 Job 有两条走不到
    /// `execute` 的出口（会话池准入到期即拒 `LocalSessionBusy`、路由解析失败）。只由
    /// `execute` 回收的话，这些登记会活到进程退出——本 Provider 实例在形状未变时是被
    /// 复用的，于是一次一条地叠加，每条最多攥着约 40 MB 窗缓冲。
    ///
    /// 反向验证：把 `StreamRegistration.release` 换成 `StreamRelease::detached()`
    /// （= 回到只有 `execute` 会 remove），本用例两条断言同时红。
    #[test]
    fn a_registration_execute_never_took_is_recycled_by_its_handle() {
        let p = provider();
        let id = StreamId::new();
        let reg = p.register_stream(id);
        assert_eq!(p.registry.len(), 1, "登记应在表里");

        // Job 在 execute 之前就失败了 —— 采集侧收到终态，丢掉回收句柄。
        drop(reg.release);

        assert!(p.registry.is_empty(), "登记滞留，它攥着的窗缓冲不会被释放");
        assert!(
            matches!(
                reg.window_tx.try_send(None),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            ),
            "接收端还在 = 那 64 窗还在"
        );
    }

    /// 非流式来源必须被明确拒绝，而不是悄悄当成一个空会话。
    #[tokio::test]
    async fn non_stream_audio_is_rejected_with_a_pointing_message() {
        let p = provider();
        let job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::File(std::path::PathBuf::from("/tmp/a.wav")),
                lang_hint: None,
                initial_prompt: None,
            },
            luna_core::JobOrigin::Batch,
        );
        let events: Vec<_> = p.execute(job, CancelToken::new()).await.collect().await;
        let Some(JobEvent::Failed(JobError::InvalidInput(msg))) = events.first() else {
            panic!("应以 InvalidInput 失败，实际：{events:?}");
        };
        assert!(msg.contains("批式"), "错误信息该指向正确的通道：{msg}");
    }

    /// 未登录时**一个字节都不发**（ADR-037 零云网络行为）。
    ///
    /// 这条不能靠「反正连不上」兜底：连不上也已经发出了 DNS 查询与 TCP SYN，
    /// 那已经是一次可观测的云网络行为。
    #[tokio::test]
    async fn without_credentials_nothing_is_sent() {
        let p = provider();
        let stream_id = StreamId::new();
        let _reg = p.register_stream(stream_id);
        let job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(stream_id),
                lang_hint: None,
                initial_prompt: None,
            },
            luna_core::JobOrigin::VoiceInput,
        );
        let events: Vec<_> = p.execute(job, CancelToken::new()).await.collect().await;
        assert!(
            matches!(
                events.as_slice(),
                [JobEvent::Failed(JobError::Unauthorized { .. })]
            ),
            "未登录应立即以 Unauthorized 收场，且不产生 Queued/Started：{events:?}"
        );
    }

    struct Blocked;
    impl luna_core::OutboundGate for Blocked {
        fn allows_outbound(&self) -> bool {
            false
        }
    }

    /// 闸关着时**一个连接都不建**（严格本地硬拦，ADR-038 ④）。
    ///
    /// 用一个**不可达的地址**：实现里若先连了再查闸，这条会以连接错误失败；
    /// 只有真的没连，才会走到 `StrictLocalBlocked`。同时钉住错误的**归类**——
    /// 归成 `Unauthorized` 会把用户支到「去登录」，而他要做的是改隐私档。
    #[tokio::test]
    async fn a_closed_gate_blocks_the_connection_entirely() {
        let p = RealtimeAsrProvider::new(RealtimeAsrConfig {
            // 保留地址段（RFC 5737），保证不可达。
            api_base: "http://192.0.2.1:9".into(),
            // 凭据**有效**：这条测的是闸，不是登录态。两者混在一起时，
            // 「未登录」会掩盖「闸没生效」。
            bearer: Arc::new(WithToken),
            gate: Arc::new(Blocked),
        });
        let stream_id = StreamId::new();
        let _reg = p.register_stream(stream_id);
        let job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Stream(stream_id),
                lang_hint: None,
                initial_prompt: None,
            },
            luna_core::JobOrigin::Recording,
        );
        let events: Vec<_> = p.execute(job, CancelToken::new()).await.collect().await;
        assert!(
            matches!(
                events.as_slice(),
                [JobEvent::Failed(JobError::StrictLocalBlocked { .. })]
            ),
            "闸关着却仍然建了连接（或归错了类）：{events:?}"
        );
    }

    struct WithToken;
    #[async_trait]
    impl BearerSource for WithToken {
        async fn bearer(&self) -> Option<String> {
            Some("t".into())
        }
        async fn refresh(&self, _rejected: &str) -> Option<String> {
            Some("t".into())
        }
    }

    /// 服务端回报检测语言后，会话控制面要能读到它（`PRD-04-046` 的回显）。
    ///
    /// 这条链路此前是断的：上游发 `{"language":"en"}`，网关把它当成认不出的帧丢掉，
    /// 于是服务端档下「自动检测（中文）」**永远不出现**——而本机档下有，用户会以为
    /// 是自己设置错了。
    #[test]
    fn the_detected_language_reaches_the_session_control() {
        let p = provider();
        let id = StreamId::new();
        let reg = p.register_stream(id);
        assert_eq!(reg.lang.auto_detected(), None);
        assert!(matches!(
            parse_frame(r#"{"language":"en","language_prob":0.98}"#),
            Frame::Language(code) if code == "en"
        ));
        // 走一遍 Provider 内部的落点（`execute` 里 `Frame::Language` 那一支）：
        // 摘走这条登记，和 `execute` 拿到的是同一份。
        let registered = p.registry.take(id).expect("会话已注册").lang;
        registered.observe("en");
        assert_eq!(reg.lang.auto_detected().as_deref(), Some("en"));
        // 检测结果不得被配置覆盖——那是两个不同的问题。
        reg.lang.set_configured(Some("zh".into()));
        assert_eq!(reg.lang.auto_detected().as_deref(), Some("en"));
    }

    /// **时间戳偏移校正**：服务端每条连接从零重计，重连后必须把它挪回会话时间轴。
    ///
    /// 不挪的话，重连后回来的第一段「0.0–1.2 秒」会与会话开头那段撞在一起，而
    /// [`CommittedLedger`] 按绝对时间判重——于是真正的新内容被当成重复丢掉。
    /// 这条与 `asr_reconnect` 里「偏移与重放起点同一个计数器」是同一件事的两半。
    #[test]
    fn a_reconnected_stream_is_moved_back_onto_the_session_timeline() {
        // 服务端认下到 30.0 秒后断线 → 新连接的零点是第 1500 帧（30.0 秒）。
        let base = frame_offset(watermark_frame(Duration::from_secs(30)));
        assert_eq!(base, Duration::from_secs(30));

        let mut ledger = CommittedLedger::default();
        ledger.admit(vec![seg(0, 30_000, "断线之前的三十秒")]);
        // 重连后服务端从零重计，第一段是它自己的 0.0–1.2 秒。
        let shifted = shift(vec![seg(0, 1_200, "重连之后的第一句")], base);
        assert_eq!(shifted[0].start, Duration::from_secs(30));
        let fresh = ledger.admit(shifted);
        assert_eq!(
            fresh.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            ["重连之后的第一句"],
            "校正后的新内容被当成重复丢掉了"
        );

        // 反证：不校正就会被账本吞掉——这正是这条测试要拦的事故。
        let mut naive = CommittedLedger::default();
        naive.admit(vec![seg(0, 30_000, "断线之前的三十秒")]);
        assert!(
            naive
                .admit(vec![seg(0, 1_200, "重连之后的第一句")])
                .is_empty(),
            "没有偏移校正时它本该被吞掉——若这里不再成立，说明判重口径变了，\
             上面那条断言也就不再证明校正生效"
        );
    }

    /// 零偏移（首条连接）时 `shift` 必须是恒等——否则整场会话的时间轴会整体漂。
    #[test]
    fn the_first_connection_needs_no_correction() {
        let original = vec![seg(0, 1_000, "第一句"), seg(1_000, 2_000, "第二句")];
        let shifted = shift(original.clone(), Duration::ZERO);
        assert_eq!(
            shifted.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
            original
                .iter()
                .map(|s| (s.start, s.end))
                .collect::<Vec<_>>()
        );
    }

    /// 下行帧解析：四种类型各自归位，认不出的跳过而不杀流。
    #[test]
    fn frames_parse_into_their_kinds() {
        let payload = format!(r#"{{"committed":{}}}"#, wire(&[seg(0, 1000, "hi")]));
        assert!(matches!(parse_frame(&payload), Frame::Committed(v) if v.len() == 1));
        let hyp = format!(r#"{{"hypothesis":{}}}"#, wire(&[seg(0, 500, " 在说 ")]));
        assert!(matches!(parse_frame(&hyp), Frame::Hypothesis(t) if t == "在说"));
        assert!(matches!(
            parse_frame(r#"{"error":"QUOTA_EXHAUSTED"}"#),
            Frame::Error(c) if c == "QUOTA_EXHAUSTED"
        ));
        // 未知帧与坏 JSON 都跳过。
        assert!(matches!(parse_frame(r#"{"whatever":1}"#), Frame::Ignored));
        assert!(matches!(parse_frame("not json"), Frame::Ignored));
    }

    /// 执行者性质缺失/不认识时归 `Unknown`，**不得**当成 `LocalRuntime`。
    ///
    /// 反过来写的话，新版服务端加一种性质时，界面会告诉用户「数据没出组织」，
    /// 而我们其实并不知道——一次不会报错的隐私误述。
    #[test]
    fn unknown_executor_kind_is_not_silently_local() {
        let known = parse_frame(
            r#"{"executed_by":{"upstream_name":"组织服务端","kind":"third_party","model":"large-v3"}}"#,
        );
        assert!(matches!(known, Frame::Executor(i) if i.kind == ExecutorKind::ThirdParty));
        for raw in [
            r#"{"executed_by":{"upstream_name":"x","kind":"brand_new","model":"m"}}"#,
            r#"{"executed_by":{"upstream_name":"x","model":"m"}}"#,
        ] {
            assert!(
                matches!(parse_frame(raw), Frame::Executor(i) if i.kind == ExecutorKind::Unknown),
                "未知/缺失的执行者性质被当成了已知：{raw}"
            );
        }
    }

    /// 错误码映射：额度用尽与上游不可用各有归宿。
    #[test]
    fn stream_errors_map_to_actionable_kinds() {
        assert!(matches!(
            map_stream_error("quota_exhausted"),
            JobError::QuotaExhausted { dimension: Some(d) } if d == "asr_seconds"
        ));
        // 上游不可用要归 `ProviderUnavailable`：Router 的兜底据此回落本机 whisper。
        // 归 `Upstream` 的话兜底不触发，用户直接看到失败——而本机明明能跑。
        for code in ["upstream_unavailable", "rate_limited"] {
            assert!(
                matches!(map_stream_error(code), JobError::ProviderUnavailable { .. }),
                "{code} 应可回落"
            );
        }
        assert!(!map_stream_error("quota_exhausted").is_retriable());
    }

    /// wire 值按**服务端原样**钉住，且大写形态必须落进兜底分支。
    ///
    /// 这条拦的是一次真事故：映射写成了大写，于是额度用尽、未登录、上游不可用
    /// 三种各有可执行动作的错误，全部退化成一句「服务端实时转写失败」——编译、
    /// clippy、其余单测全绿，只有用户会发现按钮点了没用。
    #[test]
    fn error_codes_match_the_server_wire_values() {
        for wrong in ["QUOTA_EXHAUSTED", "UNAUTHORIZED", "UPSTREAM_UNAVAILABLE"] {
            assert!(
                matches!(map_stream_error(wrong), JobError::Upstream { .. }),
                "{wrong} 不是服务端会发的形态（它发小写 snake_case），\
                 却被当成已知码——说明映射两头都认，那等于没有契约"
            );
        }
        assert!(matches!(
            map_stream_error("unauthorized"),
            JobError::Unauthorized { .. }
        ));
        assert!(matches!(
            map_stream_error("capability_not_allowed"),
            JobError::CapabilityNotAllowed { .. }
        ));
        assert!(matches!(
            map_stream_error("account_frozen"),
            JobError::AccountFrozen { .. }
        ));
        // 握手被 403 `account_frozen` 拒：认得出；别的 403 不当成冻结。
        let refused = |status: u16, body: &str| {
            let resp = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(Some(body.as_bytes().to_vec()))
                .unwrap();
            tokio_tungstenite::tungstenite::Error::Http(resp)
        };
        assert_eq!(
            refused_handshake(&refused(403, r#"{"error":"account_frozen"}"#)),
            Some("account_frozen")
        );
        assert_eq!(
            refused_handshake(&refused(403, r#"{"error":"forbidden"}"#)),
            None
        );
        assert_eq!(
            refused_handshake(&refused(401, r#"{"error":"account_frozen"}"#)),
            None
        );
        // 本月预付上限（C4）：握手 402 体与错误帧两条路都认；码与状态不成对的不算。
        assert_eq!(
            refused_handshake(&refused(402, r#"{"error":"spend_cap_reached"}"#)),
            Some("spend_cap_reached")
        );
        assert_eq!(
            refused_handshake(&refused(402, r#"{"error":"quota_exhausted"}"#)),
            None
        );
        assert_eq!(
            refused_handshake(&refused(403, r#"{"error":"spend_cap_reached"}"#)),
            None
        );
        let capped = map_stream_error("spend_cap_reached");
        assert!(matches!(capped, JobError::SpendCapReached { .. }));
        assert!(!capped.is_retriable() && capped.is_structural());
        // 免费额度被扣下（C5）：握手 402 体与错误帧两条路都认；码与状态不成对的不算。
        assert_eq!(
            refused_handshake(&refused(402, r#"{"error":"free_quota_withheld"}"#)),
            Some("free_quota_withheld")
        );
        assert_eq!(
            refused_handshake(&refused(403, r#"{"error":"free_quota_withheld"}"#)),
            None
        );
        let withheld = map_stream_error("free_quota_withheld");
        assert!(matches!(withheld, JobError::FreeQuotaWithheld { .. }));
        assert!(!withheld.is_retriable() && withheld.is_structural());
        // 认不出的码归 `Upstream` 并**保留原码**，否则运维拿不到任何线索。
        assert!(matches!(
            map_stream_error("egress_blocked"),
            JobError::Upstream { code: Some(c), .. } if c == "egress_blocked"
        ));
    }

    #[test]
    fn ws_url_maps_scheme_and_carries_language() {
        assert_eq!(
            ws_url("http://10.0.0.62:8090/", None),
            "ws://10.0.0.62:8090/inference/v1/asr/stream"
        );
        assert_eq!(
            ws_url("https://api.example", Some("zh-Hans")),
            "wss://api.example/inference/v1/asr/stream?lang=zh-Hans"
        );
        // 配置里的怪值不得原样拼进请求行。
        assert_eq!(
            ws_url("https://api.example", Some("zh&x=1 y")),
            "wss://api.example/inference/v1/asr/stream"
        );
    }

    /// 凭据走 HTTP 头，**不进 URL**：URL 会落进反代与访问日志。
    #[test]
    fn credentials_never_reach_the_url() {
        let request = build_request("ws://h/inference/v1/asr/stream", "secret-token").unwrap();
        assert!(
            !request.uri().to_string().contains("secret-token"),
            "凭据出现在 URL 里：{}",
            request.uri()
        );
        assert_eq!(
            request.headers().get("Authorization").unwrap(),
            "Bearer secret-token"
        );
    }
}
