//! 静默重连窗的**行为**测试（ADR-038 ⑧），对着一台可编排的本地假服务端跑。
//!
//! # 为什么不是单测，也不是真机冒烟
//!
//! 单测证明不了这里的任何一条：要验的全是「连接被拔掉之后会发生什么」——重放发了没有、
//! 发的是不是水位线之后那一段、时间戳有没有挪回会话时间轴、窗关了会不会如实交给
//! failover。这些只有在**真的建过一条 WebSocket、真的把它拔掉**之后才存在。
//!
//! 而真机冒烟（`asr_realtime_live.rs`）需要一台真服务端 + 一个能把它打断的通道，默认
//! `#[ignore]`——于是这条链路在日常门禁里就是零覆盖。假服务端补的正是这一段：wire
//! 是真的，断线是真的，只有上游的转写内容是编排出来的。
//!
//! # 断线用「直接丢掉 socket」而不是发 Close 帧
//!
//! 真实的断线（Wi-Fi 漫游、服务端进程被杀、反代换后端）留下的是没有握手的 TCP FIN /
//! RST，在 `tungstenite` 里与 `Close` 帧是**不同的分支**。只测 Close 那一支的话，
//! 客户端真正会遇到的那一支永远没人验过。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, CapabilitySet, Job, JobEvent, JobKind,
    JobOrigin, OutboundGate, PartialPayload, ProviderDescriptor, ProviderHealth, ProviderId,
    ProviderLocation, ReconnectPhase, Segment, StreamId, StreamLangControl, StreamRegistration,
    StreamingAsr, UsageEstimate, WindowInput,
};
use luna_provider::asr_failover::FailoverAsrProvider;
use luna_provider::asr_realtime::{RealtimeAsrConfig, RealtimeAsrProvider};
use luna_provider::asr_remote::BearerSource;
use luna_provider::opus_uplink::FRAME_SAMPLES;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::{Response as HttpResponse, StatusCode};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

// ─── 测试替身 ────────────────────────────────────────────────────────────────

struct StaticToken;

#[async_trait::async_trait]
impl BearerSource for StaticToken {
    async fn bearer(&self) -> Option<String> {
        Some("t".into())
    }
    async fn refresh(&self, _rejected: &str) -> Option<String> {
        Some("t".into())
    }
}

/// 会轮换的凭据源：`bearer()` 给 `t0`，每次 `refresh` 记下**刚被拒的那枚**并发下一枚
/// （`t1`、`t2`…）。用来验「传对被拒的 token」（TDD-14 §3.3.1 ⑩）与「换枚重连」。
///
/// `fail_after` 之后 `refresh` 返回 `None`——模拟真的登录失效（刷新也换不出会话）。
struct RotatingToken {
    rejected: Mutex<Vec<String>>,
    issued: Mutex<u32>,
    fail_after: u32,
}

impl RotatingToken {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            rejected: Mutex::new(Vec::new()),
            issued: Mutex::new(0),
            fail_after: u32::MAX,
        })
    }

    /// 前 `n` 次 `refresh` 正常轮换，第 `n+1` 次起返回 `None`（真过期）。
    fn failing_after(n: u32) -> Arc<Self> {
        Arc::new(Self {
            rejected: Mutex::new(Vec::new()),
            issued: Mutex::new(0),
            fail_after: n,
        })
    }

    fn rejected(&self) -> Vec<String> {
        self.rejected.lock().expect("rejected poisoned").clone()
    }
}

#[async_trait::async_trait]
impl BearerSource for RotatingToken {
    async fn bearer(&self) -> Option<String> {
        Some("t0".into())
    }
    async fn refresh(&self, rejected: &str) -> Option<String> {
        self.rejected
            .lock()
            .expect("rejected poisoned")
            .push(rejected.to_owned());
        let mut issued = self.issued.lock().expect("issued poisoned");
        if *issued >= self.fail_after {
            return None;
        }
        *issued += 1;
        Some(format!("t{issued}"))
    }
}

/// 可在会话中途翻的外发闸（ADR-038 ④）。
#[derive(Default)]
struct FlippableGate(AtomicBool);

impl FlippableGate {
    fn open() -> Arc<Self> {
        Arc::new(Self(AtomicBool::new(true)))
    }
    fn close(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl OutboundGate for FlippableGate {
    fn allows_outbound(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 一条连接上，服务端收到的东西。
#[derive(Default, Debug)]
struct ConnLog {
    /// 收到的二进制帧数（= Opus 包数）。
    packets: usize,
    /// 收到的二进制字节总数。
    bytes: usize,
    /// 收到 `"END"` 了吗。
    ended: bool,
}

/// 一台可编排的假实时 ASR 服务端。
///
/// 第 `n` 条连接按 `script[n]` 演；脚本用完之后**不再 accept**（模拟服务端一直没起来）。
struct FakeServer {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<ConnLog>>>,
    /// 每一次握手尝试（无论最终接受还是 401 拒绝）呈上的 `Authorization` 头，按序。
    /// 用来断言重连**换了新 token**，而不是原样重发旧的。
    auth: Arc<Mutex<Vec<Option<String>>>>,
}

/// 一条连接的剧本。
#[derive(Clone)]
enum Act {
    /// 收够 `after_packets` 个上行包就发这些定稿（秒），然后**直接丢掉 socket**（硬断）。
    CommitThenDrop {
        after_packets: usize,
        commit: Vec<(f64, f64, &'static str)>,
    },
    /// 收够 `after_packets` 个上行包就发这些定稿，然后等 `"END"`、干净收场。
    CommitThenFinish {
        after_packets: usize,
        commit: Vec<(f64, f64, &'static str)>,
    },
    /// 收够 `after_packets` 个上行包就发这些定稿，然后服务端**优雅关流**——发一个带
    /// **正常 close code**（1000）的 Close 帧、**不等 END**，模拟 WhisperLive 每段解码收尾即
    /// 无条件 `socket.close()`。用来验缺陷 A：客户端不该把它误报成「网络不稳」，而应静默
    /// 重连（`Resyncing`）。与 `CommitThenDrop`（硬断、无 close 握手 → 真断连）成对。
    CommitThenCloseGracefully {
        after_packets: usize,
        commit: Vec<(f64, f64, &'static str)>,
    },
    /// 什么都不发，等对端走。
    Silent,
    /// **握手阶段回 401**：模拟这条连接呈上的 token 已过期。tungstenite 把它表达成
    /// `Error::Http(401)`，客户端应据此刷新换枚再连（而不是用旧 token 干等到窗关降级）。
    RejectUnauthorized,
    /// **握手过、连上后下发 `unauthorized` 错误帧**：模拟会话中途 token 过期。
    /// 客户端应据此刷新换枚重连（而不是整条中止提示重登）。
    UnauthorizedFrame,
}

/// 一条连接发完定稿后的收尾方式。
#[derive(Clone, Copy)]
enum Ending {
    /// 硬断：不发 close 帧、直接丢 socket（真断连，客户端应「网络不稳」重连）。
    Drop,
    /// 优雅关流：发正常 close code（1000）的 Close 帧（上游正常结束这条流，客户端应静默重连）。
    Graceful,
    /// 等 `"END"`、干净收场。
    WaitEnd,
}

impl FakeServer {
    // `accept_hdr_async` 的握手回调签名由 tungstenite 定死：`Err` 变体是
    // `http::Response<Option<String>>`，本就偏大——这是契约要求的类型，不是我们能改的。
    #[allow(clippy::result_large_err)]
    async fn start(script: Vec<Act>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let log: Arc<Mutex<Vec<ConnLog>>> = Arc::new(Mutex::new(Vec::new()));
        let auth: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let log_for_task = Arc::clone(&log);
        let auth_for_task = Arc::clone(&auth);
        tokio::spawn(async move {
            for act in script {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                // 握手回调：记下呈上的 token，并按剧本决定接受 / 401 拒绝。
                let reject = matches!(act, Act::RejectUnauthorized);
                let auth_slot = Arc::clone(&auth_for_task);
                let callback = move |req: &HandshakeRequest,
                                     resp: HandshakeResponse|
                      -> Result<HandshakeResponse, ErrorResponse> {
                    let seen = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    auth_slot.lock().expect("auth poisoned").push(seen);
                    if reject {
                        Err(HttpResponse::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Some("unauthorized".to_owned()))
                            .expect("build 401"))
                    } else {
                        Ok(resp)
                    }
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
                    // 401 拒绝或握手失败：客户端会换枚重连，进下一条剧本。
                    continue;
                };
                let index = {
                    let mut log = log_for_task.lock().expect("log poisoned");
                    log.push(ConnLog::default());
                    log.len() - 1
                };
                let _ = ws
                    .send(Message::Text(
                        r#"{"executed_by":{"upstream_name":"假上游","kind":"local_runtime","model":"tiny"}}"#
                            .to_owned(),
                    ))
                    .await;
                let (after, commit, ending) = match &act {
                    Act::CommitThenDrop {
                        after_packets,
                        commit,
                    } => (*after_packets, commit.clone(), Ending::Drop),
                    Act::CommitThenFinish {
                        after_packets,
                        commit,
                    } => (*after_packets, commit.clone(), Ending::WaitEnd),
                    Act::CommitThenCloseGracefully {
                        after_packets,
                        commit,
                    } => (*after_packets, commit.clone(), Ending::Graceful),
                    Act::Silent => {
                        while let Some(Ok(_)) = ws.next().await {}
                        continue;
                    }
                    Act::UnauthorizedFrame => {
                        // 连上后立刻下发 unauthorized 帧，然后读/丢直到对端离开——给客户端
                        // 一拍读到这帧、据此换枚重连（它不会在这条连接上再发东西）。
                        let _ = ws
                            .send(Message::Text(r#"{"error":"unauthorized"}"#.to_owned()))
                            .await;
                        while let Some(Ok(_)) = ws.next().await {}
                        continue;
                    }
                    Act::RejectUnauthorized => unreachable!("已在握手回调里 401 拒绝"),
                };
                let mut committed_sent = false;
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Binary(bytes) => {
                            let mut log = log_for_task.lock().expect("log poisoned");
                            log[index].packets += 1;
                            log[index].bytes += bytes.len();
                            if log[index].packets < after {
                                continue;
                            }
                        }
                        Message::Text(t) if t.trim() == "END" => {
                            log_for_task.lock().expect("log poisoned")[index].ended = true;
                            if committed_sent || commit.is_empty() {
                                let _ = ws.close(None).await;
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => continue,
                    }
                    if committed_sent {
                        continue;
                    }
                    committed_sent = true;
                    let _ = ws.send(Message::Text(wire_committed(&commit))).await;
                    match ending {
                        Ending::Drop => {
                            // 硬断：不发 Close、不 flush，直接丢掉 socket。
                            // 给客户端一拍去读上面那帧定稿——否则它会和断线竞速，
                            // 测的就不再是「定稿之后断线」这个场景了。
                            tokio::time::sleep(Duration::from_millis(80)).await;
                            drop(ws);
                            break;
                        }
                        Ending::Graceful => {
                            // 优雅关流：先给客户端一拍读到上面那帧定稿，再发一个带**正常
                            // close code**（1000）的 Close 帧。传输层没断（close 握手走完了）——
                            // 客户端据此静默重连，而不是报「网络不稳」（缺陷 A）。
                            tokio::time::sleep(Duration::from_millis(80)).await;
                            let _ = ws
                                .send(Message::Close(Some(CloseFrame {
                                    code: CloseCode::Normal,
                                    reason: std::borrow::Cow::Borrowed(""),
                                })))
                                .await;
                            tokio::time::sleep(Duration::from_millis(80)).await;
                            break;
                        }
                        Ending::WaitEnd => {}
                    }
                }
            }
        });
        Self { addr, log, auth }
    }

    fn api_base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn connections(&self) -> Vec<(usize, usize, bool)> {
        self.log
            .lock()
            .expect("log poisoned")
            .iter()
            .map(|c| (c.packets, c.bytes, c.ended))
            .collect()
    }

    /// 每一次握手尝试呈上的 `Authorization` 头，按序。
    fn auth_seen(&self) -> Vec<Option<String>> {
        self.auth.lock().expect("auth poisoned").clone()
    }
}

/// 用**真的** [`Segment`] 序列化，而不是手搓一份 JSON。
///
/// 手搓的那份会随字段增删悄悄与真实 wire 分叉，而分叉的表现是「客户端把整帧当成
/// 认不出的帧跳过」——测试照样绿，因为它断言的是自己编的那份形状。
fn wire_committed(segments: &[(f64, f64, &'static str)]) -> String {
    let list: Vec<Segment> = segments
        .iter()
        .map(|(start, end, text)| Segment {
            id: luna_core::SegmentId::new(),
            start: Duration::from_secs_f64(*start),
            end: Duration::from_secs_f64(*end),
            text: (*text).to_owned(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        })
        .collect();
    serde_json::json!({ "committed": list }).to_string()
}

// ─── 采集侧 ──────────────────────────────────────────────────────────────────

fn tone(samples: usize, from: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| ((i + from) as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin() * 0.5)
        .collect()
}

/// 采集侧怎么告诉下游「说完了」。
///
/// **两种都是关流，而且生产里用的是后一种**：`StreamRegistration::window_tx` 的契约
/// 是「投 `None` 或丢掉发送端」，两条腿也都按这两种写（whisper 的 `Ok(None) | Err(_)`、
/// 实时腿的 `Some(None) | None`）。而采集侧（`recording.rs` 停录那一段）从头到尾只做
/// `drop(window_tx.take())`——它**从不投 `None`**。此前这个文件里每一条用例都走
/// [`Farewell::Sentinel`]，于是生产上唯一走的那条路在门禁里零覆盖。
#[derive(Clone, Copy)]
enum Farewell {
    /// 投 `None` 关流。
    Sentinel,
    /// 丢掉发送端关流——**采集侧真正做的那一种**。
    Drop,
}

/// 投 `windows` 个窗（每窗恰好一帧 = 20 ms），间隔 `gap`，然后静默 `hold`、再投 `None` 关流。
///
/// **一窗一帧**是刻意的：这样「服务端收到几个包」与「客户端上行了几帧音频」是同一个
/// 数，重放到底重放了哪一段才能被**精确**断言，而不是「大于零就算过」。重解窗（同一句
/// 反复重发全量）的覆盖归 `asr_realtime_live.rs` 那条真分段器测试，与这里要验的断线正交。
fn feed(
    tx: std::sync::mpsc::SyncSender<Option<WindowInput>>,
    windows: usize,
    gap: Duration,
    hold: Duration,
) -> std::thread::JoinHandle<()> {
    feed_with(tx, windows, gap, hold, Farewell::Sentinel)
}

/// 同 [`feed`]，但用**丢掉发送端**关流（见 [`Farewell`]）。
fn feed_then_drop(
    tx: std::sync::mpsc::SyncSender<Option<WindowInput>>,
    windows: usize,
    gap: Duration,
    hold: Duration,
) -> std::thread::JoinHandle<()> {
    feed_with(tx, windows, gap, hold, Farewell::Drop)
}

fn feed_with(
    tx: std::sync::mpsc::SyncSender<Option<WindowInput>>,
    windows: usize,
    gap: Duration,
    hold: Duration,
    farewell: Farewell,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for i in 0..windows {
            if tx
                .send(Some(WindowInput {
                    samples: tone(FRAME_SAMPLES, i * FRAME_SAMPLES),
                    abs_start: (i * FRAME_SAMPLES) as u64,
                    speech_len: FRAME_SAMPLES,
                }))
                .is_err()
            {
                return;
            }
            std::thread::sleep(gap);
        }
        std::thread::sleep(hold);
        match farewell {
            Farewell::Sentinel => {
                let _ = tx.send(None);
            }
            Farewell::Drop => drop(tx),
        }
    })
}

fn stream_job(stream_id: StreamId) -> Job {
    Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Stream(stream_id),
            lang_hint: None,
            initial_prompt: None,
        },
        JobOrigin::Recording,
    )
}

fn provider(api_base: String, gate: Arc<dyn OutboundGate>) -> RealtimeAsrProvider {
    RealtimeAsrProvider::new(RealtimeAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken),
        gate,
    })
}

fn provider_with(
    api_base: String,
    gate: Arc<dyn OutboundGate>,
    bearer: Arc<dyn BearerSource>,
) -> RealtimeAsrProvider {
    RealtimeAsrProvider::new(RealtimeAsrConfig {
        api_base,
        bearer,
        gate,
    })
}

/// 跑一整条会话，带**死线**——本仓因无死线的假服务端挂死过整轮 `cargo test`，故所有
/// 会话都必须封顶：超时即 panic，绝不可能永久阻塞。
async fn run_session(provider: &dyn AIProvider, stream_id: StreamId) -> Vec<JobEvent> {
    let session = async {
        provider
            .execute(stream_job(stream_id), CancelToken::new())
            .await
            .collect::<Vec<JobEvent>>()
            .await
    };
    match tokio::time::timeout(Duration::from_secs(10), session).await {
        Ok(events) => events,
        Err(_) => panic!("会话超过 10 秒未收敛——死线触发（大概率是没换 token、干等到某处挂住）"),
    }
}

fn committed_segments(events: &[JobEvent]) -> Vec<Segment> {
    events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Partial(PartialPayload::Asr {
                committed_delta, ..
            }) => Some(committed_delta.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn phases(events: &[JobEvent]) -> Vec<ReconnectPhase> {
    events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Reconnecting(phase) => Some(*phase),
            _ => None,
        })
        .collect()
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

/// **窗内接上就继续云腿**：不降级、不重复、定稿不回退、时间戳挪回会话时间轴。
///
/// 这一条同时钉住四件事，因为它们在实现里是同一段代码的四个后果——拆开写会让每一条
/// 都要重搭一次断线现场，而断线现场本身就是这里最贵的部分。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_comes_back_inside_the_window_keeps_the_cloud_leg() {
    // 采集侧一共投 60 帧（1.2 秒）就不再出声；断线发生在**全部投完之后**，于是
    // 「第二条连接收到几个包」= 「水位线之后还剩几帧」，可以精确断言。
    const FRAMES: usize = 60;
    /// 服务端认下到这里（0.4 秒 = 第 20 帧）。
    const COMMITTED_SECS: f64 = 0.4;
    let watermark = (COMMITTED_SECS * 1000.0 / 20.0) as usize;

    let server = FakeServer::start(vec![
        // 第一条连接：收满全部 60 帧后定稿到 0.4 秒，然后硬断。
        Act::CommitThenDrop {
            after_packets: FRAMES,
            commit: vec![(0.0, COMMITTED_SECS, "断线之前")],
        },
        // 第二条：客户端应当重放水位线之后的音频。服务端这次从零重计时间戳。
        Act::CommitThenFinish {
            after_packets: 5,
            commit: vec![(0.0, 0.2, "重连之后")],
        },
    ])
    .await;

    let gate = FlippableGate::open();
    let provider = provider(server.api_base(), gate);
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        FRAMES,
        Duration::from_millis(5),
        Duration::from_secs(3),
    );

    let events: Vec<JobEvent> = provider
        .execute(stream_job(stream_id), CancelToken::new())
        .await
        .collect()
        .await;
    let _ = feeder.join();

    assert_eq!(
        phases(&events),
        [ReconnectPhase::Retrying, ReconnectPhase::Resumed],
        "重连窗没有如实开合——界面上的「重连中」也就永远不会撤：{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::Failed(_) | JobEvent::Cancelled)),
        "窗内接上了却仍然失败/取消（那会白白触发一次降级）：{events:?}"
    );

    let committed = committed_segments(&events);
    let texts: Vec<&str> = committed.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, ["断线之前", "重连之后"], "定稿重复或丢失：{texts:?}");

    // 时间戳偏移校正：服务端第二条连接从 0.0 秒重计，客户端必须把它挪到水位线之后。
    let resumed = &committed[1];
    assert!(
        resumed.start >= Duration::from_millis(380),
        "重连之后的段仍停在服务端的连接内时间轴（{:?}）——\
         不校正的话它会与会话开头撞在一起，被账本当成重复丢掉",
        resumed.start
    );
    // committed 单调不回退。
    assert!(
        committed.windows(2).all(|w| w[1].start >= w[0].start),
        "定稿时间轴回退了：{committed:?}"
    );

    // 重放的是**水位线之后的那一段**，不多不少。
    //
    // 断线时采集侧早已收声，故第二条连接上不可能有任何新音频——它收到的每一个包
    // 都是重放。收到 0 个 = 断线期间说的话真的丢了；收到 60 个 = 把服务端已经认下的
    // 前 0.4 秒又发了一遍（用户为同一段话付两次钱，且服务端会再解一遍）。
    let connections = server.connections();
    assert_eq!(connections.len(), 2, "没有重连：{connections:?}");
    assert_eq!(connections[0].0, FRAMES, "第一条连接收到的帧数不对");
    assert_eq!(
        connections[1].0,
        FRAMES - watermark,
        "重放的不是水位线之后那一段：{connections:?}"
    );
    assert!(connections[1].2, "第二条连接没有收到 END：{connections:?}");
}

/// **超窗才降级**（ADR-038 ⑤ 一个字不动）：服务端一直不回来，最终必须如实报
/// `ProviderUnavailable`，让 `asr_failover` 把会话移交本机。
///
/// 归错类（比如报 `Upstream`）的话兜底不触发，用户直接看到失败——而本机明明能跑。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_never_returns_falls_through_to_the_existing_failover() {
    // 只有一条连接的剧本：断了之后服务端不再 accept。
    let server = FakeServer::start(vec![Act::CommitThenDrop {
        after_packets: 10,
        commit: vec![(0.0, 0.2, "断线之前")],
    }])
    .await;

    let gate = FlippableGate::open();
    let provider = provider(server.api_base(), gate);
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        600,
        Duration::from_millis(20),
        Duration::ZERO,
    );

    let started = std::time::Instant::now();
    let events: Vec<JobEvent> = provider
        .execute(stream_job(stream_id), CancelToken::new())
        .await
        .collect()
        .await;
    let elapsed = started.elapsed();
    drop(feeder);

    assert_eq!(
        phases(&events),
        [ReconnectPhase::Retrying],
        "开了窗却报了「已恢复」，或者根本没开窗：{events:?}"
    );
    assert!(
        matches!(
            events.last(),
            Some(JobEvent::Failed(
                luna_core::JobError::ProviderUnavailable { .. }
            ))
        ),
        "超窗必须归 `ProviderUnavailable`——Router 的兜底链据此回落本机 whisper。\
         归别的类会让降级整个不触发：{events:?}"
    );
    // 窗是**上界**：不该等得比它长太多（连不上会立刻失败，不必等满每一跳退避）。
    assert!(
        elapsed < Duration::from_secs(9),
        "从断线到交给 failover 花了 {elapsed:?}——窗封顶失效了，\
         用户在这段时间里既没有云端字幕也没有本机字幕"
    );
    // 断线之前已经发出的定稿不受影响。
    assert_eq!(
        committed_segments(&events)
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>(),
        ["断线之前"]
    );
}

/// **重放照样过闸**（ADR-038 ④ 的既有纪律，重放路径不得绕过）。
///
/// 用户完全可能正是在断线的那几秒里按下「仅本地」。重放发的是断线**之前**录下的音频，
/// 绕过闸就等于：开关看起来生效了、实际那段麦克风内容还是出去了——不报错、不留痕，
/// 只有抓包才看得见。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_still_asks_the_outbound_gate() {
    let server = FakeServer::start(vec![
        Act::CommitThenDrop {
            after_packets: 10,
            commit: vec![],
        },
        // 第二条连接会被接起来，但客户端在重放第一个包之前就该发现闸关了。
        Act::Silent,
    ])
    .await;

    let gate = FlippableGate::open();
    let provider = provider(
        server.api_base(),
        Arc::clone(&gate) as Arc<dyn OutboundGate>,
    );
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        600,
        Duration::from_millis(20),
        Duration::ZERO,
    );

    // 断线发生在第 10 个包（约 200 ms）；在窗内（5 秒）把闸关掉。
    let flipper = Arc::clone(&gate);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        flipper.close();
    });

    let events: Vec<JobEvent> = provider
        .execute(stream_job(stream_id), CancelToken::new())
        .await
        .collect()
        .await;
    drop(feeder);

    assert!(
        matches!(
            events.last(),
            Some(JobEvent::Failed(
                luna_core::JobError::StrictLocalBlocked { .. }
            ))
        ),
        "翻闸之后重放照发——这条隐私硬拦被重放路径绕过了：{events:?}"
    );
    let connections = server.connections();
    assert!(
        connections.get(1).is_none_or(|c| c.0 == 0),
        "闸关着，第二条连接却收到了 {:?} 个包",
        connections.get(1).map(|c| c.0)
    );
}

/// **握手 401 → 刷新换枚 → 重连**（TDD-14 §3.3.1 ⑩、ADR-038 ⑧）。
///
/// 会话跨过 access token 有效期时，重连的握手会 401。此前实现始终复用建流那一枚，
/// 于是**用旧 token 一路重试到窗关、单向降级本机 whisper 跑整场最小档**。修复后：调
/// `bearer.refresh(那枚被拒的 token)` 换一枚**再重连**，会话自愈、不降级、不中止。
///
/// 反向验证：把顶部刷新块里的 `bearer.refresh` 去掉（或改回复用旧 token），这条当场红
/// ——第二条连接仍呈上 `Bearer t0`，服务端一直 401，最终归 `ProviderUnavailable`（降级）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handshake_401_refreshes_the_token_and_reconnects() {
    let server = FakeServer::start(vec![
        // 第一次握手：401（呈上的 t0 已过期）。
        Act::RejectUnauthorized,
        // 换枚之后重连：接受，认下一段，等 END 干净收场。
        Act::CommitThenFinish {
            after_packets: 3,
            commit: vec![(0.0, 0.2, "重连之后")],
        },
    ])
    .await;

    let bearer = RotatingToken::new();
    let gate = FlippableGate::open();
    let provider = provider_with(
        server.api_base(),
        gate,
        Arc::clone(&bearer) as Arc<dyn BearerSource>,
    );
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        30,
        Duration::from_millis(5),
        Duration::from_secs(2),
    );

    let events = run_session(&provider, stream_id).await;
    let _ = feeder.join();

    // 不降级、不中止：整条会话没有任何失败/取消。
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::Failed(_) | JobEvent::Cancelled)),
        "握手 401 后没有换枚重连，反而失败/降级了：{events:?}"
    );
    // 重连窗如实开合（开 → 恢复），而不是开了就再没合上。
    assert_eq!(
        phases(&events),
        [ReconnectPhase::Retrying, ReconnectPhase::Resumed],
        "重连窗没有如实开合：{events:?}"
    );
    // 换枚之后真的把字幕接了回来。
    let texts: Vec<String> = committed_segments(&events)
        .iter()
        .map(|s| s.text.clone())
        .collect();
    assert_eq!(texts, ["重连之后"], "换枚重连后没有拿回字幕：{texts:?}");

    // `refresh` 收到的必须是**刚被拒的那一枚**（t0），而不是回头再读一次凭据。
    assert_eq!(
        bearer.rejected(),
        ["t0"],
        "refresh 没有拿到被拒的那枚 token（TDD-14 §3.3.1 ⑩ 判据）"
    );
    // 重连呈上的是**新** token（t1），不是原样重发旧的。
    assert_eq!(
        server.auth_seen(),
        [Some("Bearer t0".to_owned()), Some("Bearer t1".to_owned())],
        "重连没有换 token（第二次握手仍呈上旧的）：{:?}",
        server.auth_seen()
    );
}

/// **连上后的 `unauthorized` 帧 → 刷新换枚 → 重连**（TDD-14 §3.3.1 ⑩、ADR-038 ⑧）。
///
/// 会话中途 token 过期时，服务端下发 `{"error":"unauthorized"}`。此前 `map_stream_error`
/// 把它归 `JobError::Unauthorized`，**整条中止提示重登**——而其实只需换一枚 token。
/// 修复后：换枚重连，已提交字幕不回退（committed 前缀不变）。
///
/// 反向验证：把 `Frame::Error` 里 `code == "unauthorized"` 的换枚分支去掉（改回
/// `yield Failed(map_stream_error)`），这条当场红——会话以 `Unauthorized` 中止，拿不到
/// 「重连之后」。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unauthorized_frame_refreshes_the_token_and_reconnects() {
    let server = FakeServer::start(vec![
        // 第一条连接握手过、连上后下发 unauthorized 帧（会话中途 token 过期）。
        Act::UnauthorizedFrame,
        // 换枚之后重连：接受，认下一段，干净收场。
        Act::CommitThenFinish {
            after_packets: 3,
            commit: vec![(0.0, 0.2, "重连之后")],
        },
    ])
    .await;

    let bearer = RotatingToken::new();
    let gate = FlippableGate::open();
    let provider = provider_with(
        server.api_base(),
        gate,
        Arc::clone(&bearer) as Arc<dyn BearerSource>,
    );
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        30,
        Duration::from_millis(5),
        Duration::from_secs(2),
    );

    let events = run_session(&provider, stream_id).await;
    let _ = feeder.join();

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::Failed(_) | JobEvent::Cancelled)),
        "unauthorized 帧后整条中止了（实则只需刷新换枚）：{events:?}"
    );
    let texts: Vec<String> = committed_segments(&events)
        .iter()
        .map(|s| s.text.clone())
        .collect();
    assert_eq!(texts, ["重连之后"], "换枚重连后没有拿回字幕：{texts:?}");
    assert_eq!(
        bearer.rejected(),
        ["t0"],
        "refresh 没有拿到被拒的那枚 token"
    );
    assert_eq!(
        server.auth_seen(),
        [Some("Bearer t0".to_owned()), Some("Bearer t1".to_owned())],
        "重连没有换 token：{:?}",
        server.auth_seen()
    );
}

/// **刷新也换不出会话 = 真的登录失效 → `Unauthorized`（提示重登），不是静默降级。**
///
/// 这是修复的另一半（判据②）：只有 `refresh` 返回 `None` 才归 `Unauthorized`／移交本机，
/// 而不是把「token 过期」一律当成瞬断降级本机。同时钉住「refresh 确实收到被拒的那枚」。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_login_that_cannot_be_refreshed_ends_in_unauthorized() {
    // 只有一条剧本：握手 401，之后服务端不再 accept。
    let server = FakeServer::start(vec![Act::RejectUnauthorized]).await;

    // 第一次 refresh 就返回 None（真的登录失效）。
    let bearer = RotatingToken::failing_after(0);
    let gate = FlippableGate::open();
    let provider = provider_with(
        server.api_base(),
        gate,
        Arc::clone(&bearer) as Arc<dyn BearerSource>,
    );
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        30,
        Duration::from_millis(5),
        Duration::ZERO,
    );

    let events = run_session(&provider, stream_id).await;
    drop(feeder);

    assert!(
        matches!(
            events.last(),
            Some(JobEvent::Failed(luna_core::JobError::Unauthorized { .. }))
        ),
        "刷新失败却没有归 Unauthorized（要么静默降级、要么干等）：{events:?}"
    );
    // 不得静默降级本机：那会掩盖一次真实的登录失效。
    assert!(
        !events.iter().any(|e| matches!(
            e,
            JobEvent::Failed(luna_core::JobError::ProviderUnavailable { .. })
        )),
        "真登录失效被当成瞬断、静默降级了本机：{events:?}"
    );
    assert_eq!(
        bearer.rejected(),
        ["t0"],
        "refresh 没有拿到被拒的那枚 token"
    );
}

/// **缺陷 A/B 合并会红守护**：上游**正常关流**（带正常 close code 的 Close 帧）而采集侧
/// 还在说话——客户端要**静默重连**（`Resyncing`，不弹「网络不稳」），且重连后服务端**重解
/// 出相同文本**不得复读。这两件事在真机上是同一根：服务端每 ~2 秒正常关一次流，旧代码把
/// 每一次都报成「网络不稳」（横幅几乎常驻），又因 base 前移把重解出的同一句复读成十几条。
///
/// 反向验证：
/// - **A**：把 `asr_realtime` 里 graceful-close 那一支改回 `Retrying`，`phases` 断言当场红。
/// - **B**：删掉重连处的 `committed.begin_resume()`（或账本的文本判重），"同一句" 会出现两次，
///   去重断言当场红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graceful_upstream_close_resyncs_silently_without_replaying_the_same_text() {
    // 采集侧投满 60 帧（1.2 秒）后进入静默保持；服务端在保持期内正常关流，故 `!uplink_done`。
    const FRAMES: usize = 60;
    /// 第一条连接认下到这里（0.4 秒 = 第 20 帧）。
    const COMMITTED_SECS: f64 = 0.4;

    let server = FakeServer::start(vec![
        // 第一条连接：收满全部帧后认下 "同一句"，然后**优雅关流**（正常 close code）。
        Act::CommitThenCloseGracefully {
            after_packets: FRAMES,
            commit: vec![(0.0, COMMITTED_SECS, "同一句")],
        },
        // 第二条：重连后服务端从零重计、重解重放音频，**吐出相同文本**——客户端必须去重。
        Act::CommitThenFinish {
            after_packets: 5,
            commit: vec![(0.0, 0.2, "同一句")],
        },
    ])
    .await;

    let gate = FlippableGate::open();
    let provider = provider(server.api_base(), gate);
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        FRAMES,
        Duration::from_millis(5),
        Duration::from_secs(3),
    );

    let events = run_session(&provider, stream_id).await;
    let _ = feeder.join();

    // A：上游正常关流走**静默重连**（Resyncing），不是「网络不稳」（Retrying）。
    assert_eq!(
        phases(&events),
        [ReconnectPhase::Resyncing, ReconnectPhase::Resumed],
        "上游正常关流被误报成网络故障（Retrying → 「网络不稳」横幅几乎常驻）：{:?}",
        phases(&events)
    );
    // 不降级、不中止：正常关流是会话内的事，不该触发失败/取消。
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::Failed(_) | JobEvent::Cancelled)),
        "正常关流后没有静默重连，反而失败/降级了：{events:?}"
    );
    // B：重解出的相同文本不复读——"同一句" 只出现一次。
    let texts: Vec<String> = committed_segments(&events)
        .iter()
        .map(|s| s.text.clone())
        .collect();
    assert_eq!(
        texts,
        ["同一句"],
        "跨重连重解的相同文本被复读了（base 前移绕过了去重）：{texts:?}"
    );
}

/// **静默重连必须有界**（ADR-065 增补，2026-08-17 部署机复现）：服务端一连上就正常关流、
/// 一个字都不出时，客户端不得无限地「连上 → 被关 → 再连」——那正是真机上看到的形态：
/// 每 ~0.5 秒一条新会话，字幕从此不动，而 `Resyncing` 刻意不打扰用户，于是界面上一句提示
/// 都没有、ADR-038 ⑤ 的本机兜底永远不接管（窗每次都在窗内接上，`step()` 那条上界够不到）。
///
/// 判据是「有没有进展」而不是「重连了几次」：上游正常关流本身合法且频繁（WhisperLive 每段
/// 解码收尾即关）。故连续 `MAX_RESYNC_WITHOUT_PROGRESS` 圈**一帧内容都没有**才归
/// `ProviderUnavailable`，交给既有 failover。
///
/// 反向验证：删掉 `asr_realtime` 里那段计数（或把清零改成无条件），本用例会在 10 秒死线上
/// 超时——那恰恰是缺陷本身的形状（永远不收敛）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_keeps_closing_without_producing_anything_stops_being_retried() {
    const FRAMES: usize = 60;
    // 每一条连接都：收一个包就正常关流，一个定稿都不发。剧本给得比上界多，
    // 用例要证明的正是「客户端自己停下来了」，而不是「服务端剧本演完了」。
    let script = (0..12)
        .map(|_| Act::CommitThenCloseGracefully {
            after_packets: 1,
            commit: Vec::new(),
        })
        .collect::<Vec<_>>();
    let server = FakeServer::start(script).await;

    let gate = FlippableGate::open();
    let provider = provider(server.api_base(), gate);
    let stream_id = StreamId::new();
    let registration = provider.register_stream(stream_id);
    let feeder = feed(
        registration.window_tx,
        FRAMES,
        Duration::from_millis(5),
        Duration::from_secs(3),
    );

    let events = run_session(&provider, stream_id).await;
    let _ = feeder.join();

    assert!(
        matches!(
            events.last(),
            Some(JobEvent::Failed(luna_core::JobError::ProviderUnavailable { .. }))
        ),
        "一直连得上却一个字都没出，客户端还在转圈（真机上就是「字幕不动、界面无提示」）：{events:?}"
    );
    // 静默重连是对的（不弹「网络不稳」），但它必须**收敛**：相位全是 Resyncing/Resumed，
    // 而且圈数有上界。
    let resyncs = phases(&events)
        .iter()
        .filter(|p| **p == ReconnectPhase::Resyncing)
        .count();
    assert!(
        resyncs <= 5,
        "重连圈数没有上界（{resyncs} 圈）——这条腿会一直空转到用户自己发现"
    );
    assert!(
        committed_segments(&events).is_empty(),
        "服务端一个定稿都没发过，客户端却产出了内容"
    );
}

// ─── 采集侧关流 → 这条会话必须真的收尾 ────────────────────────────────────────

/// 兜底腿的占位。
///
/// 本用例里它一次都不该被用到（云腿正常收尾，不会降级）；它存在的唯一理由是**生产装配
/// 就是这个组合**——`ipc/providers.rs` 里装了本机模型就走 `Failover(云腿, whisper)`，
/// 而下面那条缺陷**只在组合里成立**。
struct IdleLocalLeg;

impl StreamLangControl for IdleLocalLeg {
    fn set_configured(&self, _configured: Option<String>) {}
    fn auto_detected(&self) -> Option<String> {
        None
    }
}

impl StreamingAsr for IdleLocalLeg {
    fn register_stream(&self, _id: StreamId) -> StreamRegistration {
        let (window_tx, _window_rx) = std::sync::mpsc::sync_channel(8);
        let (_infer_tx, infer_rx) = std::sync::mpsc::sync_channel(8);
        StreamRegistration {
            window_tx,
            infer_rx,
            lang: Arc::new(IdleLocalLeg),
            // 本占位腿不维护登记表（它一次都不该被用到），没有可回收的登记。
            release: luna_core::StreamRelease::detached(),
        }
    }
}

#[async_trait::async_trait]
impl AIProvider for IdleLocalLeg {
    fn id(&self) -> ProviderId {
        ProviderId::from("test-idle-local")
    }
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: "本机占位".into(),
            model: None,
            location: ProviderLocation::Device,
            license_summary: None,
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Transcribe].into_iter().collect(),
            streaming: true,
            location: ProviderLocation::Device,
            explicit_only: false,
        }
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }
    async fn execute(&self, _job: Job, _cancel: CancelToken) -> luna_core::JobEventStream {
        Box::pin(futures::stream::empty())
    }
    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None
    }
}

/// **停止字幕 = 这条会话必须真的收尾**：采集侧一丢掉 `window_tx`，云腿就得发 `END`、
/// 等上游收尾、关掉连接。
///
/// 真机（2026-08-18）看到的反面：停止实时字幕之后，服务端到 AssemblyAI 的连接仍
/// ESTABLISHED 挂了 9 分钟，按窗预留（`upstream_spend_holds`，`state='held'`）一直占着
/// 不结算——而 AssemblyAI 是**按 WebSocket 连接时长计费**的。四条会话最后一起以
/// `terminal=cancelled` 收尾的时刻，是**客户端进程退出**那一刻：只有 TCP 断了服务端才
/// 知道对端早就不说话了。
///
/// 用例必须跑在 `Failover(云腿, 本机腿)` 这个**生产装配**上。直接跑云腿是通的——
/// 缺陷只在组合里成立：转投泵把「采集侧丢掉了发送端」这件事吞掉了（它自己退出，却没让
/// 腿上那个发送端跟着没）。
///
/// 关流方式必须是**丢掉发送端**（[`Farewell::Drop`]）而不是投 `None`：采集侧
/// （`recording.rs` 停录那一段）做的就是 `drop(window_tx.take())`，它从不投 `None`。
/// 用 `Sentinel` 写这条用例会绿——而绿的是生产上不存在的那条路。
///
/// 反向验证（已实跑）：把腿放回泵之外也持有一份的共享容器里（缺陷版的
/// `Arc<Mutex<Leg>>`，`execute` 与泵各攥一份），本用例在 10 秒死线上超时，且 stdout 上
/// 恰好停在 `session.start` / `connected` 两行、没有 `uplink.done`、没有 `session.finished`
/// ——与真机日志同形。那正是缺陷本身的形状：连接永远不关。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_capture_side_sender_ends_the_cloud_session() {
    const FRAMES: usize = 20;

    // 一条连接就够：收满 FRAMES 个包 → 发定稿 → 等 `"END"` → 干净关流。
    let server = FakeServer::start(vec![Act::CommitThenFinish {
        after_packets: FRAMES,
        commit: vec![(0.0, 0.4, "说完了")],
    }])
    .await;

    let gate = FlippableGate::open();
    let remote: Arc<dyn StreamingAsr> = Arc::new(provider(server.api_base(), gate));
    let failover = FailoverAsrProvider::new(remote, Arc::new(IdleLocalLeg));

    let stream_id = StreamId::new();
    let registration = failover.register_stream(stream_id);
    let feeder = feed_then_drop(
        registration.window_tx,
        FRAMES,
        Duration::from_millis(5),
        Duration::from_millis(200),
    );

    let events = run_session(&failover, stream_id).await;
    let _ = feeder.join();

    assert!(
        matches!(events.last(), Some(JobEvent::Finished(_))),
        "采集侧关流了，会话却没收尾——服务端那条到上游的连接会一直挂着计费：{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, JobEvent::Failed(_) | JobEvent::Cancelled)),
        "正常停止不该失败/取消（那会白白触发一次本机兜底）：{events:?}"
    );

    let connections = server.connections();
    assert_eq!(connections.len(), 1, "正常停止不该有重连：{connections:?}");
    assert!(
        connections[0].2,
        "服务端没收到 `END`——它于是继续等上行，上游连接与那笔按窗预留都不会释放：{connections:?}"
    );

    let committed = committed_segments(&events);
    let texts: Vec<&str> = committed.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, ["说完了"], "收尾之前的定稿丢了：{texts:?}");
}
