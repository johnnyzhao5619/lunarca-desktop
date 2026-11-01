//! 实时冒烟：**客户端 Provider 对着真 Lunarca 服务端**跑一次批式转写。默认 `#[ignore]`。
//!
//! 补的是「Provider ↔ 服务端 wire 契约」那道缝。两侧各有单测：客户端有映射与拒绝
//! 路径的单测，服务端有解析与状态机的单测。而真正决定用户看到什么的是这段往返——
//! 字段名一改（`upload_id` → `id`）、`Duration` 的形状一变（`{secs,nanos}` → 毫秒整数），
//! 两侧单测照样绿，用户拿到的是「转写成功但一句话都没有」。
//!
//! ```sh
//! LUNA_ASR_API_BASE=http://10.0.0.62:8090 \
//! LUNA_ASR_TOKEN=... \
//! LUNA_ASR_WAV=/path/to/probe.wav \
//!   cargo test -p luna-provider --test asr_remote_live -- --ignored --nocapture
//! ```

use std::sync::Arc;

use futures::StreamExt;
use luna_core::{
    AIProvider, AudioRef, CancelToken, Job, JobEvent, JobKind, JobOrigin, Lang, ProviderLocation,
    ResultPayload,
};
use luna_provider::asr_remote::{BearerSource, RemoteAsrConfig, RemoteAsrProvider};

struct StaticToken(String);

#[async_trait::async_trait]
impl BearerSource for StaticToken {
    async fn bearer(&self) -> Option<String> {
        Some(self.0.clone())
    }
    /// 冒烟用固定 token；刷新即原样返回（真实实现由装配层接 luna-cloud 的会话）。
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

#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效 token 与一个 wav 文件"]
async fn batch_transcription_end_to_end() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    assert_eq!(
        provider.descriptor().location,
        ProviderLocation::Cloud,
        "location 标错，严格本地模式就拦不住它"
    );

    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::File(wav.into()),
            lang_hint: Some(Lang::from("en")),
            initial_prompt: None,
        },
        JobOrigin::Batch,
    );

    let mut stream = provider.execute(job, CancelToken::new()).await;
    let mut segments = None;
    let mut failure = None;
    let mut events = 0usize;
    while let Some(ev) = stream.next().await {
        events += 1;
        match ev {
            JobEvent::Finished(result) => {
                segments = match result.payload {
                    ResultPayload::Segments(s) => Some(s),
                    other => panic!("转写应回 Segments，实得 {other:?}"),
                };
            }
            JobEvent::Failed(e) => failure = Some(format!("{e:?}")),
            _ => {}
        }
    }

    assert!(failure.is_none(), "服务端转写失败：{failure:?}");
    let segments = segments.expect("应当收到 Finished");
    for s in &segments {
        println!(
            "  [{:.2}s → {:.2}s] {}",
            s.start.as_secs_f64(),
            s.end.as_secs_f64(),
            s.text
        );
    }
    assert!(
        !segments.is_empty(),
        "分段为空：要么服务端确实没识别出内容，要么 `Segment` 的 wire 形状变了导致\
         整份解析成空——两者在界面上同形（「转写成功但一句话都没有」），\
         这里必须由真机把它们分开"
    );
    assert!(
        segments.iter().any(|s| !s.text.trim().is_empty()),
        "分段里没有任何正文"
    );
    assert!(
        segments.iter().any(|s| s.end > s.start),
        "时间戳全为零说明 `Duration` 的 wire 形状对不上（`{{secs,nanos}}` 被改成了\
         毫秒整数之类）——正文可能还在，但导出/边车/回放三条链路会全废"
    );
    println!("事件数 {events}，分段 {}", segments.len());
}

/// 上传途中切到「仅本地」：必须**当场**停，而不是等这份传完（ADR-038 ④）。
///
/// 批式看起来「一次任务就结束了」，但上传一份会议录音是几分钟的持续外发，之后的轮询
/// 同样在联网。此前只有上传那一段受外发闸管——传完之后翻闸，客户端会继续轮询，
/// 而服务端还在解码用户的录音直到自己跑完。用户按下的是「仅本地」，不是「传完这一份」。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效 token 与一个长 wav"]
async fn flipping_to_strict_local_aborts_the_upload() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_LONG_WAV").expect("缺 LUNA_ASR_LONG_WAV");

    let gate = Arc::new(FlippableGate::default());
    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::clone(&gate) as Arc<dyn luna_core::OutboundGate>,
    });
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::File(wav.into()),
            lang_hint: Some(Lang::from("en")),
            initial_prompt: None,
        },
        JobOrigin::Batch,
    );

    // 收到第一个进度点就翻闸——那说明第一片已经上去了，接下来必须停。
    let flip = Arc::clone(&gate);
    let mut stream = provider.execute(job, CancelToken::new()).await;
    let mut outcome = None;
    let mut progress_seen = 0usize;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match event {
            JobEvent::Progress(p) => {
                progress_seen += 1;
                if progress_seen == 1 {
                    println!("首个进度点 {p:.2} → 切到「仅本地」");
                    flip.close();
                }
            }
            JobEvent::Failed(e) => {
                outcome = Some(e);
                break;
            }
            JobEvent::Finished(_) => {
                panic!("翻闸之后这份上传照样跑完了——「仅本地」按下去没有生效");
            }
            JobEvent::Cancelled => panic!("不该是取消——用户按的是隐私开关"),
            _ => {}
        }
    }

    assert!(progress_seen > 0, "一个进度点都没有，翻闸时机没测到");
    assert!(
        matches!(
            outcome,
            Some(luna_core::JobError::StrictLocalBlocked { .. })
        ),
        "翻闸后没有以 StrictLocalBlocked 收场：{outcome:?}——\
         错误归类也重要，用户要看出是自己刚按下的隐私开关在生效，而不是「上游出错了」"
    );
    println!("上传途中翻闸 → {outcome:?}（进度点 {progress_seen} 个）");
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

/// 未登录时**一个字节都不发**（ADR-037 零云网络行为不变量）。
///
/// 用一个**不可达的地址**：如果实现里在取 token 之前就发了请求，这条会以连接错误
/// 失败；只有真的没发，才会走到「需要登录」这一支。
#[tokio::test]
async fn no_session_means_no_request_at_all() {
    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        // 保留地址段，保证不可达；真发了请求就会是连接错误而不是登录错误。
        api_base: "http://192.0.2.1:9".to_owned(),
        bearer: Arc::new(NoToken),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::File("/nonexistent.wav".into()),
            lang_hint: None,
            initial_prompt: None,
        },
        JobOrigin::Batch,
    );
    let mut stream = provider.execute(job, CancelToken::new()).await;
    let mut saw = None;
    while let Some(ev) = stream.next().await {
        if let JobEvent::Failed(e) = ev {
            saw = Some(e);
        }
    }
    assert!(
        matches!(saw, Some(luna_core::JobError::Unauthorized { .. })),
        "未登录必须走本地短路（`Unauthorized`），而不是先发一个请求再看它失败，实得 {saw:?}"
    );
}

/// 长音频端到端：多分片上传 + 真实进度 + 分段连续性。
///
/// 短音频（一个分片、两个分段）照不到的三件事：① 分片循环是否真的按序把整个
/// 文件传完（少传最后一片 → 服务端一直 `Receiving`，任务建不出来，而 4 秒的样本
/// 只有一片，循环写错也发现不了）；② 进度是否**单调递增且封顶 0.9**；
/// ③ 分段时间戳是否覆盖到音频末尾——只解了前 30 秒也会「成功返回」，
/// 与完整结果在界面上同形。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效 token 与一个长 wav"]
async fn long_audio_multi_chunk_upload_and_full_coverage() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_LONG_WAV").expect("缺 LUNA_ASR_LONG_WAV");
    let expect_secs: f64 = std::env::var("LUNA_ASR_LONG_SECONDS")
        .expect("缺 LUNA_ASR_LONG_SECONDS")
        .parse()
        .expect("秒数应为数字");

    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::File(wav.into()),
            lang_hint: Some(Lang::from("en")),
            initial_prompt: None,
        },
        JobOrigin::Batch,
    );

    let started = std::time::Instant::now();
    let mut stream = provider.execute(job, CancelToken::new()).await;
    let mut progress: Vec<f32> = Vec::new();
    let mut segments = None;
    let mut failure = None;
    while let Some(ev) = stream.next().await {
        match ev {
            JobEvent::Progress(p) => progress.push(p),
            JobEvent::Finished(result) => {
                segments = match result.payload {
                    ResultPayload::Segments(s) => Some(s),
                    other => panic!("转写应回 Segments，实得 {other:?}"),
                };
            }
            JobEvent::Failed(e) => failure = Some(format!("{e:?}")),
            _ => {}
        }
    }
    assert!(failure.is_none(), "长音频转写失败：{failure:?}");
    let segments = segments.expect("应当收到 Finished");
    println!(
        "耗时 {:.1}s，分段 {}，进度点 {:?}",
        started.elapsed().as_secs_f64(),
        segments.len(),
        progress
    );

    // ① 多分片：进度点不止一个（单分片只会有一个 0.9）。
    assert!(
        progress.len() >= 2,
        "只有 {} 个进度点——分片循环可能没真的分片，长上传在界面上会一直卡在同一格",
        progress.len()
    );
    // ② 单调递增，且上传段封顶 0.9（剩下的留给服务端解码）。
    assert!(
        progress.windows(2).all(|w| w[1] >= w[0]),
        "进度必须单调递增：{progress:?}"
    );
    let upload_max = progress
        .iter()
        .copied()
        .filter(|p| *p < 1.0)
        .fold(0.0_f32, f32::max);
    assert!(
        (upload_max - 0.9).abs() < 0.01,
        "上传段应恰好封顶 0.9（实得 {upload_max}）——冲到 1.0 会让进度先满再干等着"
    );

    // ③ 覆盖到音频末尾：只解了前一段也会「成功返回」，与完整结果同形。
    let last_end = segments
        .iter()
        .map(|s| s.end.as_secs_f64())
        .fold(0.0_f64, f64::max);
    assert!(
        last_end >= expect_secs * 0.9,
        "末段止于 {last_end:.1}s，音频长 {expect_secs:.1}s——只解了开头一段也会\
         「成功返回」，与完整结果在界面上完全同形"
    );
    assert!(
        segments.len() >= 10,
        "{:.1} 分钟的音频只回了 {} 个分段，可疑",
        expect_secs / 60.0,
        segments.len()
    );
}

/// 解码中途取消：客户端以 `Cancelled` 收尾，且**服务端任务真被取消**。
///
/// 这条防的是「只关下游连接、让上游跑完」——用户点了取消、界面停了，而算力仍在
/// 消耗、账单仍在计数。验收由本用例（客户端侧终态）与随后的库内核对（服务端终态、
/// 计量为 0、音频已删）合起来构成。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效 token 与一个长 wav"]
async fn cancel_mid_decode_ends_as_cancelled() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_LONG_WAV").expect("缺 LUNA_ASR_LONG_WAV");
    // 取消时机：由调用方给，好覆盖「上传中」与「解码中」两个不同的窗口。
    let after_ms: u64 = std::env::var("LUNA_ASR_CANCEL_AFTER_MS")
        .unwrap_or_else(|_| "1200".into())
        .parse()
        .expect("毫秒应为数字");

    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::File(wav.into()),
            lang_hint: Some(Lang::from("en")),
            initial_prompt: None,
        },
        JobOrigin::Batch,
    );

    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(after_ms)).await;
        trigger.cancel();
    });

    let mut stream = provider.execute(job, cancel).await;
    let mut terminal = None;
    while let Some(ev) = stream.next().await {
        match ev {
            JobEvent::Cancelled => terminal = Some("cancelled".to_owned()),
            JobEvent::Finished(_) => terminal = Some("finished".to_owned()),
            JobEvent::Failed(e) => terminal = Some(format!("failed: {e:?}")),
            _ => {}
        }
    }
    assert_eq!(
        terminal.as_deref(),
        Some("cancelled"),
        "取消后必须以 Cancelled 收尾。若是 finished，说明取消完全没生效；\
         若是 failed，说明取消被当成了错误——两者在 UI 上都会误导用户"
    );
    println!("取消于 {after_ms}ms，终态 cancelled");
}

/// **走 `AudioRef::Pcm` 的那条路**——批量转写流水线只会给这个。
///
/// 存在的理由：`pipeline::transcribe_pcm` 一律先本地解码成 16k mono PCM 再提交，
/// 从不给文件路径。只验 `File` 的话，本 Provider 在真实链路上一次都跑不到，而单测
/// 与文件路径的冒烟会全绿——「做了个谁都不会走的 Provider」这件事只有这条能拦住。
///
/// 同时钉住 WAV 头：头拼错时服务端只会回一句语焉不详的格式错。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端与有效 token"]
async fn pcm_source_round_trips_through_wav_container() {
    let api_base = std::env::var("LUNA_ASR_API_BASE").expect("缺 LUNA_ASR_API_BASE");
    let token = std::env::var("LUNA_ASR_TOKEN").expect("缺 LUNA_ASR_TOKEN");
    let wav = std::env::var("LUNA_ASR_WAV").expect("缺 LUNA_ASR_WAV");

    // 读现成的 16-bit wav，还原成 f32 PCM —— 模拟流水线交给我们的东西。
    let raw = std::fs::read(&wav).expect("读 wav");
    let samples: Vec<f32> = raw[44..]
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / f32::from(i16::MAX))
        .collect();
    assert!(!samples.is_empty(), "样本不该为空");

    let provider = RemoteAsrProvider::new(RemoteAsrConfig {
        api_base,
        bearer: Arc::new(StaticToken(token)),
        gate: Arc::new(luna_core::AllowOutbound),
    });
    let job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Pcm(luna_core::PcmBuffer {
                sample_rate: 16000,
                channels: 1,
                samples,
            }),
            lang_hint: Some(Lang::from("en")),
            initial_prompt: None,
        },
        JobOrigin::SecondPass,
    );

    let mut stream = provider.execute(job, CancelToken::new()).await;
    let mut segments = None;
    let mut failure = None;
    let mut executor = None;
    while let Some(ev) = stream.next().await {
        match ev {
            JobEvent::ExecutedBy(info) => executor = Some(info),
            JobEvent::Finished(result) => {
                segments = match result.payload {
                    ResultPayload::Segments(s) => Some(s),
                    other => panic!("应回 Segments，实得 {other:?}"),
                };
            }
            JobEvent::Failed(e) => failure = Some(format!("{e:?}")),
            _ => {}
        }
    }
    assert!(failure.is_none(), "PCM 路径失败：{failure:?}");
    let segments = segments.expect("应当收到 Finished");
    for s in &segments {
        println!("  [{:.2}s] {}", s.start.as_secs_f64(), s.text);
    }
    assert!(
        segments.iter().any(|s| !s.text.trim().is_empty()),
        "分段为空 —— WAV 头拼错时服务端只会回一句语焉不详的格式错，\
         而根因在客户端那 44 字节里"
    );

    // 执行者必须如实下发（PRD-17-033）。
    let executor = executor.expect("没有收到 ExecutedBy —— 用户无从知道是谁跑的");
    println!("执行者：{executor:?}");
    assert!(!executor.display_name.is_empty());
    assert_ne!(
        executor.kind,
        luna_core::ExecutorKind::Unknown,
        "本机 whisper 上游应被识别为 local_runtime；归 Unknown 说明 wire 值对不上"
    );
}
