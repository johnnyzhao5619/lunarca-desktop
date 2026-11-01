//! 实时冒烟：**客户端流式 Provider 对着真 Lunarca 服务端**跑一次实时转写。默认 `#[ignore]`。
//!
//! 补的是「采集侧 → Provider → 服务端 → 上游」整条 wire 的缝，且**这条缝比批式那条
//! 更容易无声地断**：批式错了会报错，实时错了往往只是「一直没有字出来」——上行帧格式
//! 差一个字节序、`END` 的拼写不一致、`committed` 的字段名一改，两侧单测照样全绿。
//!
//! # 必须走真的 `StreamSegmenter`
//!
//! 本文件第一版自己按定长切片投窗，端到端跑通了——**而链路上最贵的那个错误恰好被绕开**：
//! 真实分段器产的是**重解窗**（同一句每 1.5 秒重发全量、补零到 10 秒、强切后带 250 ms
//! overlap），照单上行意味着一句 10 秒的话要传 ~70 秒的字节。服务端按上行字节计费，
//! 用户付 7 倍；而「麦克风内容持续离开设备」这个 ADR-038 ④ 明说无法消除的代价，也跟着
//! 放大 7 倍。测试形状与真实采集路径不一致时，跑绿了也证明不了什么。
//!
//! 故本文件用**真 VAD + 真分段器**产窗，并把「服务端记了多少秒」直接读回来断言。
//!
//! 与批式冒烟同一条纪律：真机不通就如实记「未验」，不拿单测绿冒充。
//!
//! ```sh
//! LUNA_ASR_API_BASE=http://10.0.0.62:8090 \
//! LUNA_ASR_TOKEN=... \
//! LUNA_ASR_WAV=/path/to/probe.wav \
//!   cargo test -p luna-provider --test asr_realtime_live -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use luna_audio::{EnergyVad, SegmenterConfig, SpeechWindow, StreamSegmenter, Vad, VadSensitivity};
use luna_core::{
    AIProvider, AudioRef, CancelToken, ExecutorKind, Job, JobEvent, JobKind, JobOrigin,
    PartialPayload, ProviderLocation, Segment, StreamId, StreamingAsr, WindowInput,
};
use luna_provider::asr_realtime::{RealtimeAsrConfig, RealtimeAsrProvider};
use luna_provider::asr_remote::BearerSource;

const SAMPLE_RATE: u32 = 16_000;
/// VAD 定长帧（20 ms @ 16 kHz）——直接引采集侧的定义，**不在这里再写一个 320**：
/// 两处各写一份的话，帧长一改这个夹具会静默地变成另一个形状而照样全绿。
const VAD_FRAME: usize = luna_audio::VAD_FRAME_SAMPLES;
/// 服务端的计费换算：16 kHz × 2 字节。
const BYTES_PER_SECOND: f64 = 32_000.0;

struct StaticToken(String);

#[async_trait::async_trait]
impl BearerSource for StaticToken {
    async fn bearer(&self) -> Option<String> {
        Some(self.0.clone())
    }
    async fn refresh(&self, _rejected: &str) -> Option<String> {
        Some(self.0.clone())
    }
}

struct NoToken;

#[async_trait::async_trait]
impl BearerSource for NoToken {
    async fn bearer(&self) -> Option<String> {
        None
    }
    async fn refresh(&self, _rejected: &str) -> Option<String> {
        None
    }
}

/// 读一个 16 kHz mono 16-bit WAV 的样本（只认这一种——采集侧本来就只产出这一种）。
fn read_wav_mono16(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("读取 wav");
    assert_eq!(&bytes[0..4], b"RIFF", "不是 RIFF 文件");
    assert_eq!(&bytes[8..12], b"WAVE", "不是 WAVE 文件");
    let mut cursor = 12usize;
    let mut data: Option<&[u8]> = None;
    let (mut channels, mut rate, mut bits) = (0u16, 0u32, 0u16);
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let body = &bytes[cursor + 8..(cursor + 8 + size).min(bytes.len())];
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        cursor += 8 + size + (size & 1);
    }
    assert_eq!(
        (channels, rate, bits),
        (1, SAMPLE_RATE, 16),
        "探针需 16 kHz mono 16-bit"
    );
    data.expect("缺 data 块")
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / f32::from(i16::MAX))
        .collect()
}

/// 走**真** VAD + **真**分段器，产出与录音/语音输入完全同形的窗序列。
fn segment_like_capture(samples: &[f32]) -> Vec<SpeechWindow> {
    let mut vad = EnergyVad::new(VadSensitivity::Normal);
    let mut segmenter = StreamSegmenter::new(SegmenterConfig::new(SAMPLE_RATE));
    let mut windows = Vec::new();
    for frame in samples.chunks(VAD_FRAME) {
        let label = vad.score(frame);
        windows.extend(segmenter.push(frame, label));
    }
    windows.extend(segmenter.flush());
    windows
}

/// 读回**服务端记的** `asr_seconds`（累计值）。
///
/// 用同一个 token 查自己的用量——闭环因此完整：客户端说「我只发了这么多」，
/// 服务端说「我按这么多计费」，两者必须对得上。
async fn asr_seconds_used(api_base: &str, token: &str) -> f64 {
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/v1/usage", api_base.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await
        .expect("查用量")
        .json()
        .await
        .expect("用量响应不是 JSON");
    // 用量按**账本**分组（订阅按周期重置、预付不重置），`asr_seconds` 只在订阅账本上。
    // 这里此前读的是一个平铺的 `quota.dimensions`，那个形状在双账本落地时就没有了——
    // 于是这条冒烟对着一台完全正常的服务端恒失败，且它的报错说「配额策略未指派？」，
    // 把一次协议漂移诬陷成部署配置问题。这类误导比直接失败更贵：查的人会去翻策略指派。
    body.get("ledgers")
        .and_then(serde_json::Value::as_array)
        .and_then(|ledgers| {
            ledgers
                .iter()
                .find(|l| l.get("kind").and_then(|v| v.as_str()) == Some("subscription"))
        })
        .and_then(|ledger| ledger.get("dimensions"))
        .and_then(serde_json::Value::as_array)
        .and_then(|dims| {
            dims.iter()
                .find(|d| d.get("dimension").and_then(|v| v.as_str()) == Some("asr_seconds"))
        })
        .and_then(|d| d.get("used"))
        .and_then(serde_json::Value::as_f64)
        .expect("订阅账本里没有 asr_seconds 维（配额策略未指派？）")
}

/// 等服务端起来（打断类测试会重启它；抢在它就绪前开会话测到的是另一个场景）。
async fn wait_until_ready(api_base: &str, token: &str) {
    let url = format!("{}/v1/usage", api_base.trim_end_matches('/'));
    for attempt in 0..30 {
        let ok = reqwest::Client::new()
            .get(&url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if ok {
            if attempt > 0 {
                println!("服务端在第 {attempt} 次探测后就绪");
            }
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("服务端 30 秒内没有就绪——先确认它在跑，再来验断连接管");
}

fn provider_with(api_base: String, token: String) -> Arc<RealtimeAsrProvider> {
    Arc::new(RealtimeAsrProvider::new(RealtimeAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::new(luna_core::AllowOutbound),
    }))
}

fn stream_job(stream_id: StreamId, origin: JobOrigin) -> Job {
    Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Stream(stream_id),
            lang_hint: None,
            initial_prompt: None,
        },
        origin,
    )
}

#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端（含流式上游）、有效 token 与一个 16k mono wav"]
async fn realtime_transcription_end_to_end() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    let samples = read_wav_mono16(&wav);
    let windows = segment_like_capture(&samples);
    // 分段器认定的真实语音总量（去重后的绝对覆盖），也就是**应该**上行的量。
    let speech_samples = windows
        .iter()
        .map(|w| w.abs_start + w.speech_len as u64)
        .max()
        .expect("分段器一个窗都没产出——探针里没有可识别的语音？");
    let naive_samples: usize = windows.iter().map(|w| w.samples.len()).sum();
    let speech_secs = speech_samples as f64 / f64::from(SAMPLE_RATE);
    println!(
        "探针 {:.2}s → 真分段器产 {} 窗；语音 {:.2}s，照单上行则是 {:.2}s（{:.1} 倍）",
        samples.len() as f64 / f64::from(SAMPLE_RATE),
        windows.len(),
        speech_secs,
        naive_samples as f64 / f64::from(SAMPLE_RATE),
        naive_samples as f64 / speech_samples as f64,
    );

    let before = asr_seconds_used(&api_base, &token).await;
    let provider = provider_with(api_base.clone(), token.clone());
    assert_eq!(
        provider.descriptor().location,
        ProviderLocation::Cloud,
        "location 标错，严格本地模式就拦不住它"
    );

    let stream_id = StreamId::new();
    let reg = provider.register_stream(stream_id);
    // 语言控制面与投窗端同属一次注册；投窗端会被搬进喂窗线程，故先留一份句柄。
    let reg_lang = Arc::clone(&reg.lang);
    let job = stream_job(stream_id, JobOrigin::Recording);

    // 按真实节奏投窗（比真实快 10 倍——够慢到不是一次性灌完，又不必等满一分钟）。
    let feeder = tokio::task::spawn_blocking(move || {
        for w in windows {
            let sent = reg.window_tx.send(Some(WindowInput {
                speech_len: w.speech_len,
                samples: w.samples,
                abs_start: w.abs_start,
            }));
            if sent.is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = reg.window_tx.send(None);
        // 句柄留到这里才丢：提前丢掉会让服务端在解出最后一段之前就看到连接结束。
        std::thread::sleep(Duration::from_secs(1));
    });

    let mut events = provider.execute(job, CancelToken::new()).await;
    let mut committed: Vec<Segment> = Vec::new();
    let mut hypotheses = 0usize;
    let mut executor = None;
    let mut failure = None;
    // 上限兜底：Apple Silicon 上的 faster-whisper 慢于实时（ADR-038 ⑦），给足余量，
    // 但绝不无限等——挂住的测试等于没有测试。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        match event {
            JobEvent::ExecutedBy(info) => {
                println!(
                    "执行者：{} / {:?} / {}",
                    info.display_name, info.kind, info.model
                );
                executor = Some(info);
            }
            JobEvent::Partial(PartialPayload::Asr {
                committed_delta,
                hypothesis,
                ..
            }) => {
                if !hypothesis.is_empty() {
                    hypotheses += 1;
                }
                for segment in committed_delta {
                    println!(
                        "  定稿 [{:.2}s → {:.2}s] {}",
                        segment.start.as_secs_f64(),
                        segment.end.as_secs_f64(),
                        segment.text
                    );
                    committed.push(segment);
                }
            }
            JobEvent::Failed(e) => failure = Some(format!("{e:?}")),
            JobEvent::Finished(_) => break,
            _ => {}
        }
    }
    feeder.await.ok();

    assert!(failure.is_none(), "服务端实时转写失败：{failure:?}");
    assert!(
        !committed.is_empty(),
        "一段定稿都没有：要么上行帧格式对不上（服务端在解一堆噪声），要么 `committed` \
         的 wire 形状变了导致整份解析成空。两者在界面上同形（「在录、但一个字都不出」），\
         这里必须由真机把它们分开"
    );
    assert!(
        committed.iter().any(|s| !s.text.trim().is_empty()),
        "定稿里没有任何正文"
    );
    assert!(
        committed.iter().any(|s| s.end > s.start),
        "时间戳全为零说明 `Duration` 的 wire 形状对不上——正文可能还在，\
         但导出/边车/回放三条链路会全废"
    );
    // committed 只增不退：这是流式 ASR 唯一不能出错的地方。
    for pair in committed.windows(2) {
        assert!(
            pair[1].start >= pair[0].start,
            "定稿回退了：{:?} 之后来了 {:?}",
            pair[0],
            pair[1]
        );
    }
    // 自动检测语言必须回到会话控制面（`PRD-04-046` 的「自动检测（中文）」回显）。
    // 上游确实发 `{"language":...}`，此前被网关当成认不出的帧丢掉——服务端档下这个
    // 回显于是永远不出现，而本机档下有，用户会以为是自己设置错了。
    let detected = reg_lang.auto_detected();
    println!("检测语言：{detected:?}");
    assert!(
        detected.is_some(),
        "服务端没有把检测到的语言送回来——`PRD-04-046` 的回显在服务端档下会是空的"
    );

    let info = executor.expect("服务端必须如实告知执行者（PRD-17-033）");
    assert_ne!(
        info.kind,
        ExecutorKind::Unknown,
        "执行者性质解析成了 Unknown——wire 值与客户端的映射对不上"
    );

    // 计费闭环：服务端记的秒数必须约等于真实语音时长。
    //
    // 这条是本文件的**核心断言**。它一旦回归（重解窗被照单上行），billed 会变成
    // speech 的好几倍——而转写结果看上去完全正常，没有任何其它信号会变红。
    // 服务端按 `ceil` 计费，故允许 1 秒的取整余量。
    let billed = asr_seconds_used(&api_base, &token).await - before;
    println!("服务端计费 {billed:.0}s，真实语音 {speech_secs:.2}s");
    assert!(
        billed <= speech_secs + 1.0,
        "服务端记了 {billed:.0} 秒，而真实语音只有 {speech_secs:.2} 秒——\
         重解窗被照单上行了（增长快照/padding/overlap 至少有一种没被游标滤掉）。\
         用户会为同一段话付好几次钱，且明文上行量同比放大"
    );
    assert!(
        billed >= speech_secs - 1.0,
        "服务端只记了 {billed:.0} 秒而语音有 {speech_secs:.2} 秒——上行漏了内容"
    );
    println!(
        "定稿 {} 段、暂定帧 {} 次、执行者 {}、计费 {billed:.0}s",
        committed.len(),
        hypotheses,
        info.display_name
    );
}

/// 取消要**当场**生效，哪怕此刻没有窗口在投。
///
/// `CancelToken` 是个原子布尔、没有异步唤醒，只能轮。若实现只在「收到窗口时」顺带检查，
/// 用户在沉默中按下停止就会等到下一个窗口才停——而客户端 VAD 让静音不上行（ADR-038 ②），
/// 那段沉默可能很长。这正是它最该立刻停下的时刻。
///
/// 关连接同时也是给服务端的取消信号：它据此停上游、按已用量结算，而不是让上游跑完。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端（含流式上游）与有效 token"]
async fn cancelling_in_silence_stops_promptly() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    let provider = provider_with(api_base, token);
    let stream_id = StreamId::new();
    let reg = provider.register_stream(stream_id);
    let job = stream_job(stream_id, JobOrigin::Recording);
    let cancel = CancelToken::new();

    // 投前几窗，然后**停止投窗**——模拟用户说完之后的沉默。
    for w in segment_like_capture(&read_wav_mono16(&wav))
        .into_iter()
        .take(2)
    {
        reg.window_tx
            .send(Some(WindowInput {
                speech_len: w.speech_len,
                samples: w.samples,
                abs_start: w.abs_start,
            }))
            .expect("投窗");
    }

    let mut events = provider.execute(job, cancel.clone()).await;
    // 让会话真的建起来（拿到 executed_by 即说明上游已选定）。
    let warmed = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.next().await {
            if matches!(event, JobEvent::ExecutedBy(_)) {
                return true;
            }
        }
        false
    })
    .await
    .expect("等待会话建立超时");
    assert!(warmed, "没能建起会话");

    let asked_at = tokio::time::Instant::now();
    cancel.cancel();
    let elapsed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            if matches!(event, JobEvent::Cancelled) {
                return Some(asked_at.elapsed());
            }
        }
        None
    })
    .await
    .expect("取消后 5 秒内没有收场——说明只在收到窗口时才检查取消")
    .expect("应以 Cancelled 收场");
    println!("沉默中取消 → {elapsed:?} 内收场");
    assert!(
        elapsed < Duration::from_secs(2),
        "取消耗时 {elapsed:?}，太久了——用户按下停止后界面会一直显示在录"
    );
    // 句柄留到断言之后再丢，确保收场是**取消**触发的，而不是采集侧关流。
    drop(reg);
}

/// 会议中途按下「仅本地」：麦克风内容必须**当场**停止外发。
///
/// 装配层「不构造云腿」只管新会话——一场一小时的会议里，正在跑的那条腿仍持有自己的
/// `Arc`。没有闸的话，隐私档已显示「仅本地」而字节还在往外走，**不报错、不留痕，
/// 只有抓包才看得见**。这条用真服务端把它钉住：翻闸之后会话立刻以 `StrictLocalBlocked`
/// 收场，且服务端记的秒数**停在翻闸那一刻**（之后一个字节都没再收到）。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端（含流式上游）、有效 token 与一个 16k mono wav"]
async fn flipping_to_strict_local_stops_the_uplink_at_once() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    let gate = Arc::new(FlippableGate::default());
    let provider = Arc::new(RealtimeAsrProvider::new(RealtimeAsrConfig {
        api_base: api_base.clone(),
        bearer: Arc::new(StaticToken(token.clone())),
        gate: Arc::clone(&gate) as Arc<dyn luna_core::OutboundGate>,
    }));
    let stream_id = StreamId::new();
    let reg = provider.register_stream(stream_id);
    let job = stream_job(stream_id, JobOrigin::Recording);

    let windows = segment_like_capture(&read_wav_mono16(&wav));
    assert!(windows.len() >= 2, "探针太短，测不出「中途」翻闸");
    let before = asr_seconds_used(&api_base, &token).await;

    // 投第一窗，等它真的上行了，再翻闸，然后继续投剩下的。
    let flip = Arc::clone(&gate);
    let feeder = tokio::task::spawn_blocking(move || {
        let mut windows = windows.into_iter();
        let first = windows.next().expect("至少一窗");
        let first_secs = (first.abs_start + first.speech_len as u64) as f64 / 16_000.0;
        let _ = reg.window_tx.send(Some(WindowInput {
            speech_len: first.speech_len,
            samples: first.samples,
            abs_start: first.abs_start,
        }));
        std::thread::sleep(Duration::from_millis(500));
        flip.close();
        for w in windows {
            let _ = reg.window_tx.send(Some(WindowInput {
                speech_len: w.speech_len,
                samples: w.samples,
                abs_start: w.abs_start,
            }));
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs(1));
        first_secs
    });

    let mut events = provider.execute(job, CancelToken::new()).await;
    let mut outcome = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let JobEvent::Failed(e) = event {
            outcome = Some(e);
            break;
        }
    }
    let uplinked_secs = feeder.await.expect("投窗线程");

    assert!(
        matches!(
            outcome,
            Some(luna_core::JobError::StrictLocalBlocked { .. })
        ),
        "翻闸后会话没有以 StrictLocalBlocked 收场：{outcome:?}"
    );
    // 服务端只该收到翻闸之前那一窗（ceil 到秒，故给 1 秒余量）。
    let billed = asr_seconds_used(&api_base, &token).await - before;
    println!("翻闸前上行 {uplinked_secs:.2}s，服务端记 {billed:.0}s");
    assert!(
        billed <= uplinked_secs + 1.0,
        "翻闸后服务端还收到了字节（记 {billed:.0}s vs 翻闸前 {uplinked_secs:.2}s）\
         ——「仅本地」按下之后麦克风内容仍在离开设备"
    );
}

/// 可在测试中途关掉的外发闸。
#[derive(Default)]
struct FlippableGate(std::sync::atomic::AtomicBool);

impl FlippableGate {
    fn close(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl luna_core::OutboundGate for FlippableGate {
    fn allows_outbound(&self) -> bool {
        !self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// 额度用尽必须以 **`QuotaExhausted`** 收场，而不是「服务不可用」。
///
/// 两者的差别是用户下一步做什么：前者去加额度，后者干等或重试——而重试一万次都不会
/// 成功。更要紧的是 `asr_failover` 刻意**不对额度用尽降级**（那样会让用户几周后才发现
/// 云额度早就没了），归错类会让那条排除整个失效。
///
/// 这条真机打出来过一次：服务端确实发了 `{"error":"quota_exhausted"}` 然后关连接，
/// 而客户端此刻正在发窗口 → `send` 失败 → 直接报 `ProviderUnavailable`，**那一帧还躺在
/// 下行缓冲里没被读**。于是**所有**服务端可执行错误在「正在发窗口」时都会退化。
///
/// 跑法：先把账户的 `asr_seconds` 上限压到已用量附近（`LUNA_ASR_EXPECT_QUOTA_EXHAUSTED=1`
/// 表示「我已经压过了，就是要验这条」），否则跳过——不压额度就跑等于什么都没验。
#[tokio::test]
#[ignore = "需要把账户的 asr_seconds 额度压到已用量附近，见文档"]
async fn running_out_of_quota_says_so_instead_of_service_unavailable() {
    if std::env::var("LUNA_ASR_EXPECT_QUOTA_EXHAUSTED").is_err() {
        panic!("缺 LUNA_ASR_EXPECT_QUOTA_EXHAUSTED——额度没压过就跑，这条什么都证明不了");
    }
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    let provider = provider_with(api_base, token);
    let stream_id = StreamId::new();
    let reg = provider.register_stream(stream_id);
    let job = stream_job(stream_id, JobOrigin::Recording);

    let feeder = tokio::task::spawn_blocking(move || {
        for w in segment_like_capture(&read_wav_mono16(&wav)) {
            if reg
                .window_tx
                .send(Some(WindowInput {
                    speech_len: w.speech_len,
                    samples: w.samples,
                    abs_start: w.abs_start,
                }))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    let mut events = provider.execute(job, CancelToken::new()).await;
    let mut outcome = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let JobEvent::Failed(e) = event {
            outcome = Some(e);
            break;
        }
    }
    feeder.await.ok();

    match outcome {
        Some(luna_core::JobError::QuotaExhausted { dimension }) => {
            println!("额度用尽 → QuotaExhausted，维度 {dimension:?}");
            assert_eq!(
                dimension.as_deref(),
                Some("asr_seconds"),
                "维度必须说清——用户要看出是转写分钟不够，而不是 token 不够"
            );
        }
        other => panic!(
            "额度用尽被归成了 {other:?}——用户会去重试或干等，而他真正该做的是加额度；\
             归成 `ProviderUnavailable` 还会触发一次本不该发生的降级"
        ),
    }
}

/// **断连时本地接管，且已定稿的前缀永不回退**（ADR-038 ⑤ 的核心不变量）。
///
/// 这条此前只有单测——单测里两条腿都是假的，证明的是「账本逻辑对」；而真正会出错的
/// 是**真实断连长什么样**：服务端进程被重启时，客户端读到的可能是 `Close` 帧、也可能
/// 是没有握手的 TCP FIN，两者在 `tungstenite` 里是不同的分支，只有一条会走到降级。
///
/// 打断方式由 `LUNA_ASR_BREAK_CMD` 给（如 `ssh host 'launchctl kickstart -k …'`）——
/// 不硬编码任何部署细节；没给就跳过而不是假装验过。
///
/// 兜底腿用一个**可控的假本机腿**：这里要验的是「切过去了、且定稿没被重写」，
/// 而不是 whisper 本身（那有它自己的覆盖）。假腿刻意产出与云端**重叠**的时间区间，
/// 模拟本机从头重解——账本必须把落在已发区间内的段滤掉。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效 token、一个长 wav 与 LUNA_ASR_BREAK_CMD"]
async fn a_broken_link_hands_over_to_local_without_rewriting_committed() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_LONG_WAV").expect("缺 LUNA_ASR_LONG_WAV");
    let Ok(break_cmd) = std::env::var("LUNA_ASR_BREAK_CMD") else {
        panic!("缺 LUNA_ASR_BREAK_CMD——没有真实的打断手段就不算验过，不跑空壳");
    };

    // 先等服务端真的就绪再开跑。上一次打断刚重启过它，抢在它起来之前开会话的话，
    // 测到的是「云端一开局就不可用 → 直接降级」——那是另一个场景，用它冒充「中途断连」
    // 就等于没验 ADR-038 ⑤ 要验的那件事。
    wait_until_ready(&api_base, &token).await;
    let cloud = provider_with(api_base, token);
    let local = Arc::new(FakeLocalLeg::default());
    let seen = Arc::clone(&local.windows);
    let failover = Arc::new(luna_provider::asr_failover::FailoverAsrProvider::new(
        Arc::clone(&cloud) as Arc<dyn StreamingAsr>,
        Arc::clone(&local) as Arc<dyn StreamingAsr>,
    ));

    let stream_id = StreamId::new();
    let reg = failover.register_stream(stream_id);
    let job = stream_job(stream_id, JobOrigin::Recording);

    // 长音频：够跑到「云端已经定稿了几段」再打断。
    let windows = segment_like_capture(&read_wav_mono16(&wav));
    assert!(windows.len() >= 6, "探针太短，来不及在定稿之后打断");
    let feeder = tokio::task::spawn_blocking(move || {
        for w in windows {
            if reg
                .window_tx
                .send(Some(WindowInput {
                    speech_len: w.speech_len,
                    samples: w.samples,
                    abs_start: w.abs_start,
                }))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let _ = reg.window_tx.send(None);
        std::thread::sleep(Duration::from_secs(3));
    });

    let mut events = failover.execute(job, CancelToken::new()).await;
    let mut committed: Vec<Segment> = Vec::new();
    // 每次执行者回报时「音频是否离开设备」；降级到本机后必须出现 `false`。
    let mut executors: Vec<bool> = Vec::new();
    let mut broke = false;
    let mut committed_before_break = 0usize;
    let mut before_break: Vec<(std::time::Duration, String)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        match event {
            JobEvent::ExecutedBy(info) => {
                println!(
                    "执行者 → {} / {:?} / 离开设备={}",
                    info.display_name, info.kind, info.leaves_device
                );
                // 判「降级到本机」要看 `leaves_device`，**不是** `kind`：服务端把它
                // 自己的 WhisperLive 也报成 `local_runtime`，用 kind 判断会把「云端在跑」
                // 误认成「已经切回本机」——这条断言第一次跑就是这么被骗过去的。
                executors.push(info.leaves_device);
            }
            JobEvent::Partial(PartialPayload::Asr {
                committed_delta, ..
            }) => {
                for segment in committed_delta {
                    println!(
                        "  定稿 [{:.2}s → {:.2}s] {}",
                        segment.start.as_secs_f64(),
                        segment.end.as_secs_f64(),
                        segment.text
                    );
                    committed.push(segment);
                }
                // 等**云端**定稿几段之后再打断。两个条件缺一不可：
                // - 当前腿仍是云端（`leaves_device == true`）——否则打断的是本机腿，无意义；
                // - 已经有几段——只等一段的话，接管后的重叠区几乎覆盖全部内容，
                //   测不出「已发出的前缀有没有被重写」。
                if !broke && executors.last() == Some(&true) && committed.len() >= 3 {
                    broke = true;
                    committed_before_break = committed.len();
                    before_break = committed
                        .iter()
                        .map(|s| (s.start, s.text.clone()))
                        .collect();
                    println!("--- 打断链路：{break_cmd}");
                    let status = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&break_cmd)
                        .status()
                        .expect("执行打断命令");
                    assert!(status.success(), "打断命令失败：{status}");
                }
            }
            JobEvent::Failed(e) => panic!("断连没有触发降级，整条会话失败了：{e:?}"),
            JobEvent::Finished(_) => break,
            _ => {}
        }
    }
    feeder.await.ok();

    assert!(
        broke,
        "一直没等到**云端**产出定稿就结束了——多半是会话开局就降级了（云端不可用），\
         那是另一个场景，不能拿来冒充「中途断连」"
    );
    assert!(
        executors.contains(&false),
        "断连后没有切到本机——ADR-038 ⑤ 的「本地接管」没生效（执行者序列的\
         「是否离开设备」始终为真）：{executors:?}"
    );
    assert!(
        seen.load(std::sync::atomic::Ordering::Acquire) > 0,
        "切腿了但采集侧的窗没有改道——降级之后就再也没有转写了"
    );
    // 定稿只增不退：这是整条链路唯一不能出错的地方。
    for pair in committed.windows(2) {
        assert!(
            pair[1].start >= pair[0].start,
            "定稿回退了：{:?} 之后来了 {:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(
        committed.len() > committed_before_break,
        "接管之后一段新定稿都没有（本机腿没产出或被账本全滤掉了）"
    );
    // 断点**之前**发出的那些定稿必须原样还在：用户可能已经复制或导出过它们。
    // 本机腿是从 0 秒重解的，账本一旦漏掉去重，这里就会看到它们被改写。
    for (index, (start, text)) in before_break.iter().enumerate() {
        assert_eq!(
            (committed[index].start, &committed[index].text),
            (*start, text),
            "第 {index} 段在接管之后被改写了——云端已发出的定稿被本机重解覆盖"
        );
    }
    println!(
        "断点前 {committed_before_break} 段、总共 {} 段，执行者序列 {executors:?}",
        committed.len()
    );
}

/// 可控的假本机腿：刻意产出与云端**重叠**的区间，模拟本机从头重解。
#[derive(Default)]
struct FakeLocalLeg {
    windows: Arc<std::sync::atomic::AtomicUsize>,
}

impl StreamingAsr for FakeLocalLeg {
    fn register_stream(&self, _id: StreamId) -> luna_core::StreamRegistration {
        let (window_tx, window_rx) = std::sync::mpsc::sync_channel(64);
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(64);
        let seen = Arc::clone(&self.windows);
        std::thread::spawn(move || {
            while let Ok(Some(_)) = window_rx.recv() {
                seen.fetch_add(1, std::sync::atomic::Ordering::Release);
                let _ = infer_tx.try_send(1);
            }
        });
        luna_core::StreamRegistration {
            window_tx,
            infer_rx,
            lang: Arc::new(NoLang),
            // 本假腿不维护登记表（会话状态就是上面那个线程），没有可回收的登记。
            release: luna_core::StreamRelease::detached(),
        }
    }
}

struct NoLang;
impl luna_core::StreamLangControl for NoLang {
    fn set_configured(&self, _configured: Option<String>) {}
    fn auto_detected(&self) -> Option<String> {
        Some("zh".into())
    }
}

#[async_trait::async_trait]
impl luna_core::AIProvider for FakeLocalLeg {
    fn id(&self) -> luna_core::ProviderId {
        luna_core::ProviderId::from("fake-local-whisper")
    }
    fn descriptor(&self) -> luna_core::ProviderDescriptor {
        luna_core::ProviderDescriptor {
            id: self.id(),
            display_name: "本地 Whisper（测试替身）".into(),
            model: Some("ggml-test".into()),
            location: ProviderLocation::Device,
            license_summary: None,
        }
    }
    fn capabilities(&self) -> luna_core::CapabilitySet {
        luna_core::CapabilitySet {
            capabilities: [luna_core::Capability::Transcribe].into_iter().collect(),
            streaming: true,
            location: ProviderLocation::Device,
            explicit_only: false,
        }
    }
    async fn health(&self) -> luna_core::ProviderHealth {
        luna_core::ProviderHealth::ready()
    }
    async fn execute(&self, _job: Job, _cancel: CancelToken) -> luna_core::JobEventStream {
        // 从 0 秒开始重解，并一直覆盖到**远超断点**的位置——真实的本机腿接管后会继续
        // 处理后续音频，时间轴自然越过断点。
        //
        // 这两段缺一不可：前半段验「云端已定稿的区间会被账本滤掉」，后半段验「接管之后
        // 确实还在出字」。只写前半段的话，一个把新内容也一并滤掉的账本同样能通过。
        let mut segments = Vec::new();
        for i in 0..40u64 {
            segments.push(Segment {
                id: luna_core::SegmentId::new(),
                start: Duration::from_secs(i * 3),
                end: Duration::from_secs(i * 3 + 3),
                text: format!("本机重解第 {i} 段"),
                translation: None,
                speaker: None,
                confidence: None,
                words: None,
            });
        }
        Box::pin(futures::stream::iter(vec![
            JobEvent::Started,
            JobEvent::Partial(PartialPayload::Asr {
                committed_delta: segments,
                hypothesis: String::new(),
                stability: 1.0,
            }),
        ]))
    }
    fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
        None
    }
}

/// **真机验收：停止字幕之后，这条会话必须在几秒内真的收尾**（ADR-079）。
///
/// 跑的是**生产装配** `Failover(云腿, 本机腿)`——装了本机模型的机器走的就是这个组合，
/// 而缺陷只在组合里成立（直接跑云腿是通的）。关流用**丢掉发送端**，因为采集侧
/// （`recording.rs` 停录那一段）做的就是 `drop(window_tx.take())`。
///
/// 客户端这一侧能自己判的是「从关流到收尾花了多久」，本用例把它打出来并封顶 60 秒——
/// 与服务端侧的验收判据同一个数。**服务端那一半要另外看**（本用例够不着，如实记在这里）：
///
/// ```sh
/// # 活连接归零（rem_address 里不该再有到 AssemblyAI 的 ESTABLISHED）
/// ssh 10.0.0.62 'docker exec lunarca-server-1 sh -c "cat /proc/net/tcp"'
/// # 该会话的按窗预留必须结算（不留 state='held'）
/// psql -c "select state, count(*) from upstream_spend_holds group by 1"
/// ```
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端（含流式上游）、有效 token 与一个 16k mono wav"]
async fn stopping_the_capture_side_tears_the_live_session_down() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");
    wait_until_ready(&api_base, &token).await;

    let windows = segment_like_capture(&read_wav_mono16(&wav));
    let remote = provider_with(api_base, token) as Arc<dyn StreamingAsr>;
    let local = Arc::new(FakeLocalLeg::default());
    let failover = luna_provider::asr_failover::FailoverAsrProvider::new(
        remote,
        Arc::clone(&local) as Arc<dyn StreamingAsr>,
    );

    let stream_id = StreamId::new();
    let reg = failover.register_stream(stream_id);
    let job = stream_job(stream_id, JobOrigin::Recording);

    // 投完窗就**丢掉发送端**（= 用户按下停止），并记下那一刻。
    let closed_at = Arc::new(std::sync::Mutex::new(None::<tokio::time::Instant>));
    let closed_slot = Arc::clone(&closed_at);
    let feeder = tokio::task::spawn_blocking(move || {
        for w in windows {
            let sent = reg.window_tx.send(Some(WindowInput {
                speech_len: w.speech_len,
                samples: w.samples,
                abs_start: w.abs_start,
            }));
            if sent.is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        *closed_slot.lock().expect("closed poisoned") = Some(tokio::time::Instant::now());
        drop(reg.window_tx);
    });

    let mut events = failover.execute(job, CancelToken::new()).await;
    // 两个数：**60 秒是验收判据**（下面那条断言），而这里的 300 秒只是「绝不无限等」
    // 的外层封顶。刻意让外层松于判据——超了 60 秒时我们要的是一个写着**实际花了多久**
    // 的失败，而不是一句「超时」：前者能回答「差多少、卡在哪一步」，后者不能。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    let mut finished = None;
    let mut failure = None;
    loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        match event {
            JobEvent::Finished(_) => {
                finished = Some(tokio::time::Instant::now());
                break;
            }
            JobEvent::Failed(e) => {
                failure = Some(e);
                break;
            }
            _ => {}
        }
    }
    let _ = feeder.await;

    assert!(failure.is_none(), "正常停止不该失败：{failure:?}");
    let finished = finished.expect("采集侧关流之后会话没有收尾——服务端那条上游连接会一直挂着计费");
    let closed = closed_at
        .lock()
        .expect("closed poisoned")
        .expect("喂窗线程没跑到关流");
    let took = finished.duration_since(closed);
    println!("关流 → 收尾：{:.2}s", took.as_secs_f64());
    assert!(
        took < Duration::from_secs(60),
        "关流到收尾花了 {:.2}s，超过 60 秒的验收判据——按连接时长计费的上游会一直计下去",
        took.as_secs_f64(),
    );
    assert_eq!(
        local.windows.load(std::sync::atomic::Ordering::Acquire),
        0,
        "正常停止不该触发降级（云腿好好的，切过去只会让这一场的后半段跑在本机最小档上）"
    );
}

/// 未登录时**一个字节都不发**（ADR-037 零云网络行为不变量）。
///
/// 用一个**不可达的地址**：如果实现里在取 token 之前就连了，这条会以连接错误失败；
/// 只有真的没连，才会走到「需要登录」这一支。
#[tokio::test]
async fn no_session_means_no_connection_at_all() {
    let provider = RealtimeAsrProvider::new(RealtimeAsrConfig {
        // 保留地址段（RFC 5737），保证不可达。
        api_base: "http://192.0.2.1:9".to_owned(),
        bearer: Arc::new(NoToken),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    let stream_id = StreamId::new();
    let _reg = provider.register_stream(stream_id);
    let events: Vec<_> = provider
        .execute(
            stream_job(stream_id, JobOrigin::VoiceInput),
            CancelToken::new(),
        )
        .await
        .collect()
        .await;
    assert!(
        matches!(
            events.as_slice(),
            [JobEvent::Failed(luna_core::JobError::Unauthorized { .. })]
        ),
        "未登录应立即以 Unauthorized 收场且不产生任何连接：{events:?}"
    );
}

/// 真分段器的窗**确实**是重解窗——这条不成立的话，上面那条计费断言就是空跑。
///
/// 存在的理由：本文件第一版自己切片投窗，跑绿了却什么都没证明。这条把「测试形状与
/// 真实采集路径一致」变成一个会变红的事实，而不是一句注释。
#[test]
fn the_fixture_really_produces_redecode_windows() {
    // 5 秒连续「语音」，足以触发多个增长快照。
    // 电平取 ±0.4（= −8 dBFS）不是随手写的：`EnergyVad` 的门限相对噪声底自适应，
    // 而底被上夹取在 F_FLOOR_MAX(−30 dBFS)，故 normal 档门限最高只到 −20 dBFS——
    // −8 dBFS 的持续信号必然全程判语音（这正是 `VadSaturated` 诊断盯着的那个状态）。
    // 若改用更低的电平，底会在 3 秒内跟上来把它判成静音，分段器不再出快照，本用例失效。
    let samples: Vec<f32> = (0..SAMPLE_RATE as usize * 5)
        .map(|i| if i % 2 == 0 { 0.4 } else { -0.4 })
        .collect();
    let windows = segment_like_capture(&samples);
    assert!(
        windows.len() >= 3,
        "连续 5 秒语音只产了 {} 窗——分段器没在产增长快照，夹具失去意义",
        windows.len()
    );
    let naive: usize = windows.iter().map(|w| w.samples.len()).sum();
    let speech = windows
        .iter()
        .map(|w| w.abs_start + w.speech_len as u64)
        .max()
        .unwrap() as usize;
    assert!(
        naive >= speech * 2,
        "照单上行只有 {naive} 样本 vs 真实 {speech}——重解窗的重复没有出现，\
         夹具与真实采集路径已经不同形了"
    );
    // 游标法必须把它收敛回真实语音量（与 Provider 内部同一条判据）。
    let mut sent = 0u64;
    let mut deduped = 0usize;
    for w in &windows {
        let input = WindowInput {
            samples: w.samples.clone(),
            abs_start: w.abs_start,
            speech_len: w.speech_len,
        };
        deduped += input.fresh_after(sent).map_or(0, <[f32]>::len);
        sent = sent.max(input.span().end);
    }
    assert_eq!(
        deduped, speech,
        "游标去重后 {deduped} ≠ 真实语音 {speech}——上行会多算或漏算"
    );
    println!(
        "夹具自检：{} 窗，照单 {:.2}s / 去重 {:.2}s（{:.1} 倍），按 {:.0} B/s 计费差 {:.0} 字节",
        windows.len(),
        naive as f64 / f64::from(SAMPLE_RATE),
        deduped as f64 / f64::from(SAMPLE_RATE),
        naive as f64 / deduped as f64,
        BYTES_PER_SECOND,
        (naive - deduped) as f64 * 2.0,
    );
}
